//! Write-permit acquisition and write-keyword detection.
//!
//! Manages the global Bolt write semaphore for serializing mutating
//! queries against contention-sensitive backends, plus a small lexer
//! for detecting whether a query string contains write keywords.

use std::sync::Arc;

use crate::message::types::ServerMessage;
use crate::metrics as bolt_metrics;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

use super::super::{bolt_map, Session};

/// Acquire the write semaphore if the query is a write and contention
/// management is needed.
///
/// Returns `Some(failure_messages)` if acquisition fails or times out, `None`
/// on success (permit stored in session) or when no acquisition is needed.
pub(super) async fn acquire_write_semaphore(
    session: &mut Session,
    query: &str,
    extra: &BoltMap,
) -> Option<Vec<ServerMessage>> {
    let is_write_query = query_requires_write_permit(query);
    if !is_write_query || session.write_permit.is_some() {
        return None;
    }

    let in_explicit_tx = matches!(
        session.state,
        ServerState::TxReady | ServerState::TxStreaming
    );

    let timeout_ms = session
        .tx_timeout
        .or(extra
            .get_int("tx_timeout")
            .filter(|&v| v > 0)
            .map(|v| v as u64))
        .or_else(|| in_explicit_tx.then_some(default_explicit_tx_write_timeout_ms()));

    if let Some(ms) = timeout_ms {
        let sem = Arc::clone(&session.write_semaphore);
        match tokio::time::timeout(std::time::Duration::from_millis(ms), sem.acquire_owned()).await
        {
            Ok(Ok(permit)) => {
                session.write_permit = Some(permit);
                None
            }
            Ok(Err(_closed)) => {
                // Semaphore closed (shutdown)
                session.state = ServerState::Failed;
                Some(vec![ServerMessage::Failure(bolt_map! {
                    "code" => BoltValue::String(
                        "Neo.TransientError.Transaction.LockClientStopped".to_string()
                    ),
                    "message" => BoltValue::String(
                        "Server is shutting down".to_string()
                    )
                })])
            }
            Err(_) => {
                // Semaphore acquisition timed out
                bolt_metrics::error_recorded("Neo.TransientError.Transaction.LockClientStopped");
                session.state = ServerState::Failed;
                Some(vec![ServerMessage::Failure(bolt_map! {
                    "code" => BoltValue::String(
                        "Neo.TransientError.Transaction.LockClientStopped".to_string()
                    ),
                    "message" => BoltValue::String(
                        format!("The transaction has been terminated. \
                                 Retry your operation in a new transaction, \
                                 and you should see a successful result. \
                                 Lock acquisition timeout after {} ms", ms)
                    )
                })])
            }
        }
    } else {
        // Autocommit write without tx_timeout -- skip semaphore entirely
        // for maximum concurrency. Redis handles atomicity at the command level.
        None
    }
}

fn default_explicit_tx_write_timeout_ms() -> u64 {
    static TIMEOUT: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *TIMEOUT.get_or_init(|| {
        cypher_core::config::parse_env_range("BOLT_DEFAULT_TX_TIMEOUT_MS", 30_000u64, 1, 300_000)
    })
}

pub(super) fn query_requires_write_permit(query: &str) -> bool {
    const WRITE_KEYWORDS: &[&str] = &["CREATE", "MERGE", "DELETE", "SET", "REMOVE"];

    let bytes = query.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' | b'`' => {
                let quote = bytes[i];
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' && quote != b'`' {
                        i += 2;
                        continue;
                    }
                    if bytes[i] == quote {
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() {
                    if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        i += 2;
                        break;
                    }
                    i += 1;
                }
            }
            b if b.is_ascii_alphabetic() => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                    i += 1;
                }
                let token = &query[start..i];
                if WRITE_KEYWORDS
                    .iter()
                    .any(|keyword| token.eq_ignore_ascii_case(keyword))
                {
                    return true;
                }
            }
            _ => i += 1,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::query_requires_write_permit as needs;

    #[test]
    fn read_only_queries_do_not_need_write_permit() {
        for q in [
            "MATCH (n) RETURN n",
            "RETURN 1",
            "WITH 1 AS x RETURN x",
            "CALL dbms.components()",
            "",
            "   ",
        ] {
            assert!(!needs(q), "query {q:?} should not require write permit");
        }
    }

    #[test]
    fn write_keywords_in_any_case_require_permit() {
        for q in [
            "CREATE (n)",
            "create (n)",
            "Merge (n:X {id:1})",
            "DELETE n",
            "set n.x = 1",
            "REMOVE n.x",
            "MATCH (n) DETACH DELETE n",
            "MATCH (n) SET n.x = 2",
        ] {
            assert!(needs(q), "query {q:?} should require write permit");
        }
    }

    #[test]
    fn write_keyword_inside_string_literal_is_ignored() {
        // Single quotes
        assert!(!needs("RETURN 'CREATE'"));
        assert!(!needs("RETURN 'MERGE me'"));
        // Double quotes
        assert!(!needs("RETURN \"DELETE this\""));
        // Backticks
        assert!(!needs("MATCH (n:`SET`) RETURN n"));
        // Escaped quote inside single-quoted string must not terminate the string
        assert!(!needs(r"RETURN 'it\'s CREATE'"));
    }

    #[test]
    fn write_keyword_inside_line_comment_is_ignored() {
        assert!(!needs("MATCH (n) RETURN n // CREATE this later"));
        assert!(!needs("// SET n.x = 1\nMATCH (n) RETURN n"));
    }

    #[test]
    fn write_keyword_inside_block_comment_is_ignored() {
        assert!(!needs("MATCH (n) /* DELETE n */ RETURN n"));
        // Unterminated block comment should not panic and should not match.
        assert!(!needs("MATCH (n) /* CREATE this RETURN n"));
    }

    #[test]
    fn write_keyword_after_block_comment_still_detected() {
        assert!(needs("/* read */ CREATE (n)"));
        assert!(needs("// preface\nMERGE (n {id:1})"));
    }

    #[test]
    fn substring_of_write_keyword_does_not_match() {
        // CREATED contains CREATE but is a longer alphanumeric token.
        assert!(!needs("RETURN CREATED"));
        assert!(!needs("MATCH (n {created: 1}) RETURN n"));
        assert!(!needs("RETURN MERGER"));
    }

    #[test]
    fn non_ascii_bytes_do_not_panic() {
        // Mixed unicode and a write keyword.
        assert!(needs("MATCH (n) WHERE n.name = 'café' SET n.x = 1"));
        // Pure unicode without keywords.
        assert!(!needs("RETURN 'café'"));
    }
}
