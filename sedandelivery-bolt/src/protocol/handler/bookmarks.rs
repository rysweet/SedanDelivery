//! Bookmark formatting, validation, and parsing helpers.

use crate::bookmark::BookmarkCounter;
use crate::message::types::ServerMessage;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};

use super::bolt_map;
use super::session::READ_AFTER_WRITE_WINDOW;

// -- Bookmark helpers --------------------------------------------------------

/// Format a bookmark value using the Neo4j convention.
///
/// The format `neo4j:bookmark:v1:tx{N}` is recognized by all Neo4j drivers
/// and the testkit. Using a shared atomic counter ensures that bookmarks
/// produced by session A can be meaningfully consumed by session B.
pub(crate) fn format_bookmark(n: u64) -> String {
    format!("neo4j:bookmark:v1:tx{}", n)
}

/// Increment the global bookmark counter and return a formatted bookmark string.
pub(crate) fn next_bookmark(counter: &BookmarkCounter) -> String {
    let n = counter.next();
    format_bookmark(n)
}

pub(crate) fn record_write_bookmark(counter: &BookmarkCounter, bookmark: &str) {
    if let Some(tx_id) = bookmark_tx_id(bookmark) {
        counter.record_issued_at(tx_id);
    }
}

fn bookmark_strings(extra: &BoltMap) -> Option<Vec<&str>> {
    let bookmarks_val = extra.get_value("bookmarks")?;

    Some(match bookmarks_val {
        BoltValue::List(items) => items
            .iter()
            .filter_map(|v| {
                if let BoltValue::String(s) = v {
                    Some(s.as_str())
                } else {
                    None
                }
            })
            .collect(),
        BoltValue::String(s) => vec![s.as_str()],
        _ => return None,
    })
}

fn bookmark_tx_id(bookmark: &str) -> Option<u64> {
    if let Some(rest) = bookmark.strip_prefix("neo4j:bookmark:") {
        let after_v = rest.strip_prefix('v')?;
        let colon_pos = after_v.find(':')?;
        let tx_part = &after_v[colon_pos + 1..];
        return tx_part.strip_prefix("tx")?.parse().ok();
    }

    bookmark.strip_prefix("sedan:tx:")?.parse().ok()
}

pub(crate) fn recent_bookmark_age(
    extra: &BoltMap,
    counter: &BookmarkCounter,
) -> Option<std::time::Duration> {
    let freshest = bookmark_strings(extra)?
        .into_iter()
        .filter_map(bookmark_tx_id)
        .filter_map(|tx_id| counter.issued_age(tx_id))
        .filter(|age| *age < READ_AFTER_WRITE_WINDOW)
        .min()?;

    Some(freshest)
}

// -- Bookmark validation -----------------------------------------------------

/// Check whether a bookmark string has a recognized format.
/// Valid formats: "neo4j:bookmark:v{V}:tx{N}" or "sedan:tx:{N}" where V and N are digits.
pub(crate) fn is_valid_bookmark(bookmark: &str) -> bool {
    if bookmark.is_empty() {
        return true;
    }
    // neo4j:bookmark:vN:txM format -- strip prefix, expect "vN:txM" with digits
    if let Some(rest) = bookmark.strip_prefix("neo4j:bookmark:") {
        // rest should be "vN:txM" where N and M are digit sequences
        if let Some(after_v) = rest.strip_prefix('v') {
            if let Some(colon_pos) = after_v.find(':') {
                let version_part = &after_v[..colon_pos];
                let tx_part = &after_v[colon_pos + 1..];
                if let Some(tx_num) = tx_part.strip_prefix("tx") {
                    return !version_part.is_empty()
                        && version_part.chars().all(|c| c.is_ascii_digit())
                        && !tx_num.is_empty()
                        && tx_num.chars().all(|c| c.is_ascii_digit());
                }
            }
        }
        return false;
    }
    // sedan:tx:N format
    if let Some(suffix) = bookmark.strip_prefix("sedan:tx:") {
        return !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit());
    }
    false
}

/// Validate bookmarks from a message's extra map.
///
/// Returns `Some(ServerMessage::Failure(...))` if any bookmark is invalid,
/// or `None` if all bookmarks are valid (or no bookmarks are present).
pub(crate) fn validate_bookmarks(extra: &BoltMap) -> Option<ServerMessage> {
    let bookmark_strings = bookmark_strings(extra)?;

    for bm in bookmark_strings {
        if !is_valid_bookmark(bm) {
            return Some(ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String(
                    "Neo.ClientError.Transaction.InvalidBookmark".to_string(),
                ),
                "message" => BoltValue::String(
                    format!("Supplied bookmark '{}' does not conform to pattern {{database_id}}:{{tx_id}}", bm),
                )
            }));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Bookmark validation unit tests --------------------------------------

    #[test]
    fn valid_bookmark_sedan_prefix() {
        assert!(is_valid_bookmark("sedan:tx:1"));
        assert!(is_valid_bookmark("sedan:tx:42"));
        assert!(is_valid_bookmark("sedan:tx:999999"));
    }

    #[test]
    fn valid_bookmark_neo4j_prefix() {
        assert!(is_valid_bookmark("neo4j:bookmark:v1:tx100"));
        assert!(is_valid_bookmark("neo4j:bookmark:v2:tx1"));
    }

    #[test]
    fn valid_bookmark_empty() {
        assert!(is_valid_bookmark(""));
    }

    #[test]
    fn invalid_bookmark_random_string() {
        assert!(!is_valid_bookmark("invalid_bookmark"));
        assert!(!is_valid_bookmark("random"));
        assert!(!is_valid_bookmark("tx:1"));
        assert!(!is_valid_bookmark("bookmark:1"));
    }

    #[test]
    fn validate_bookmarks_none_present() {
        let extra: BoltMap = vec![];
        assert!(validate_bookmarks(&extra).is_none());
    }

    #[test]
    fn validate_bookmarks_empty_list() {
        let extra: BoltMap = vec![("bookmarks".to_string(), BoltValue::List(vec![]))];
        assert!(validate_bookmarks(&extra).is_none());
    }

    #[test]
    fn validate_bookmarks_valid_sedan() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String("sedan:tx:1".to_string())]),
        )];
        assert!(validate_bookmarks(&extra).is_none());
    }

    #[test]
    fn validate_bookmarks_valid_neo4j() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String(
                "neo4j:bookmark:v1:tx100".to_string(),
            )]),
        )];
        assert!(validate_bookmarks(&extra).is_none());
    }

    #[test]
    fn validate_bookmarks_invalid_returns_failure() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![BoltValue::String("invalid_bookmark".to_string())]),
        )];
        let failure = validate_bookmarks(&extra);
        assert!(failure.is_some());
        match failure.unwrap() {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Transaction.InvalidBookmark");
                let msg = meta.get_str("message").unwrap();
                assert!(msg.contains("invalid_bookmark"));
            }
            other => panic!("expected Failure, got {:?}", other),
        }
    }

    #[test]
    fn validate_bookmarks_mixed_valid_and_invalid() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::List(vec![
                BoltValue::String("sedan:tx:1".to_string()),
                BoltValue::String("bad_bookmark".to_string()),
            ]),
        )];
        let failure = validate_bookmarks(&extra);
        assert!(failure.is_some());
        match failure.unwrap() {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Transaction.InvalidBookmark");
            }
            other => panic!("expected Failure, got {:?}", other),
        }
    }

    #[test]
    fn validate_bookmarks_single_string_valid() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::String("neo4j:bookmark:v1:tx1".to_string()),
        )];
        assert!(validate_bookmarks(&extra).is_none());
    }

    #[test]
    fn validate_bookmarks_single_string_invalid() {
        let extra: BoltMap = vec![(
            "bookmarks".to_string(),
            BoltValue::String("not_a_valid_bookmark".to_string()),
        )];
        let failure = validate_bookmarks(&extra);
        assert!(failure.is_some());
        match failure.unwrap() {
            ServerMessage::Failure(meta) => {
                let code = meta.get_str("code").unwrap();
                assert_eq!(code, "Neo.ClientError.Transaction.InvalidBookmark");
            }
            other => panic!("expected Failure, got {:?}", other),
        }
    }

    #[test]
    fn validate_bookmarks_non_list_non_string_ignored() {
        let extra: BoltMap = vec![("bookmarks".to_string(), BoltValue::Int(42))];
        assert!(validate_bookmarks(&extra).is_none());
    }

    // -- Bookmark format tests -----------------------------------------------

    #[test]
    fn bookmark_format_matches_neo4j_convention() {
        assert_eq!(format_bookmark(1), "neo4j:bookmark:v1:tx1");
        assert_eq!(format_bookmark(42), "neo4j:bookmark:v1:tx42");
    }

    #[test]
    fn next_bookmark_increments_counter() {
        let counter = crate::bookmark::BookmarkCounter::from_env();
        // We only verify that consecutive bookmarks are distinct and well-formed.
        let b1 = next_bookmark(&counter);
        let b2 = next_bookmark(&counter);
        assert!(b1.starts_with("neo4j:bookmark:v1:tx"), "b1={b1}");
        assert!(b2.starts_with("neo4j:bookmark:v1:tx"), "b2={b2}");
        assert_ne!(b1, b2, "consecutive bookmarks must differ");
    }
}
