//! HELLO and LOGON message handlers.
//!
//! Submodules:
//! - [`rate_limit`] — failure-counter backends (in-memory + Redis) and sweep task.
//! - [`auth`] — `BOLT_PASSWORD` configuration and basic-auth validation.

use std::sync::OnceLock;

use crate::message::types::ServerMessage;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

use super::{bolt_map, ProtocolError, Session};

mod auth;
mod rate_limit;

pub use rate_limit::spawn_auth_rate_limiter_sweep;

#[cfg(test)]
pub(crate) use auth::with_required_password_override_for_tests;
#[cfg(test)]
pub(crate) use rate_limit::clear_auth_rate_limit;

/// Server version string, configurable via `BOLT_SERVER_VERSION` env var.
fn server_version() -> &'static str {
    static VER: OnceLock<String> = OnceLock::new();
    VER.get_or_init(|| {
        std::env::var("BOLT_SERVER_VERSION").unwrap_or_else(|_| "Neo4j/4.4.0".to_string())
    })
    .as_str()
}

/// Handle the HELLO message (initial client authentication).
///
/// Validates credentials when `BOLT_PASSWORD` is set, negotiates the UTC
/// patch, and transitions the session to `Ready`.
pub(crate) async fn handle_hello(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    if let Some(failure) =
        auth::validate_basic_auth(session, &extra, auth::required_password().as_deref()).await
    {
        return Ok(failure);
    }

    // Check if the client requests the UTC patch (Bolt 4.3+).
    // When echoed back, the driver uses UTC-based DateTime encoding (tags 0x49/0x46)
    // instead of wall-clock encoding (tags 0x69/0x66).
    let utc_patch = match extra.get_value("patch_bolt") {
        Some(BoltValue::List(items)) => items
            .iter()
            .any(|v| matches!(v, BoltValue::String(s) if s == "utc")),
        _ => false,
    };

    session.state = ServerState::Ready;
    let mut success = bolt_map! {
        "server" => BoltValue::String(server_version().to_string()),
        "connection_id" => BoltValue::String(format!("sedandelivery-bolt-{}", session.conn_id))
    };
    if utc_patch {
        success.push((
            "patch_bolt".to_string(),
            BoltValue::List(vec![BoltValue::String("utc".to_string())]),
        ));
    }
    Ok(vec![ServerMessage::Success(success)])
}

/// Handle the LOGON message (deferred authentication for Bolt 5.1+).
///
/// Validates credentials when `BOLT_PASSWORD` is set and transitions the
/// session to `Ready`.
pub(crate) async fn handle_logon(
    session: &mut Session,
    auth_map: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    if let Some(failure) =
        auth::validate_basic_auth(session, &auth_map, auth::required_password().as_deref()).await
    {
        return Ok(failure);
    }

    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::IpAddr;
    use std::sync::Arc;

    use cypher_core::storage::InMemoryGraphStore;

    fn auth_session(ip: IpAddr) -> Session {
        Session {
            state: ServerState::Authentication,
            store: Arc::new(InMemoryGraphStore::new()),
            conn_id: 1,
            current_graph: "neo4j".to_string(),
            pending_results: HashMap::new(),
            next_qid: 0,
            global_bookmark: Arc::new(crate::bookmark::BookmarkCounter::from_env()),
            tx_metadata: Vec::new(),
            tx_timeout: None,
            tx_store: None,
            write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
            write_permit: None,
            saved_graph: None,
            tx_had_write: false,
            peer_ip: Some(ip),
            last_write_at: None,
        }
    }

    fn basic_auth(credentials: &str) -> BoltMap {
        bolt_map! {
            "scheme" => BoltValue::String("basic".to_string()),
            "credentials" => BoltValue::String(credentials.to_string())
        }
    }

    fn response_code(messages: &[ServerMessage]) -> Option<&str> {
        match messages.first() {
            Some(ServerMessage::Success(_)) => Some("SUCCESS"),
            Some(ServerMessage::Failure(meta)) => meta.get_str("code"),
            _ => None,
        }
    }

    #[tokio::test]
    async fn hello_without_password_does_not_rate_limit_repeated_attempts() {
        let ip = "10.0.0.11".parse().unwrap();
        clear_auth_rate_limit(ip).await;

        with_required_password_override_for_tests(None, async move {
            for _ in 0..16 {
                let mut session = auth_session(ip);
                let responses = handle_hello(&mut session, basic_auth("wrong"))
                    .await
                    .unwrap();
                assert_eq!(response_code(&responses), Some("SUCCESS"));
            }
        })
        .await;
    }

    #[tokio::test]
    async fn successful_hello_auth_clears_failed_attempt_budget() {
        let ip = "10.0.0.12".parse().unwrap();
        clear_auth_rate_limit(ip).await;

        with_required_password_override_for_tests(Some("secret"), async move {
            for _ in 0..10 {
                let mut session = auth_session(ip);
                let responses = handle_hello(&mut session, basic_auth("wrong"))
                    .await
                    .unwrap();
                assert_eq!(
                    response_code(&responses),
                    Some("Neo.ClientError.Security.Unauthorized")
                );
            }

            let mut success_session = auth_session(ip);
            let success = handle_hello(&mut success_session, basic_auth("secret"))
                .await
                .unwrap();
            assert_eq!(response_code(&success), Some("SUCCESS"));

            let mut retry_session = auth_session(ip);
            let retry = handle_hello(&mut retry_session, basic_auth("wrong"))
                .await
                .unwrap();
            assert_eq!(
                response_code(&retry),
                Some("Neo.ClientError.Security.Unauthorized")
            );
        })
        .await;
    }

    #[tokio::test]
    async fn failed_logon_attempts_are_rate_limited() {
        let ip = "10.0.0.13".parse().unwrap();
        clear_auth_rate_limit(ip).await;

        with_required_password_override_for_tests(Some("secret"), async move {
            for _ in 0..10 {
                let mut session = auth_session(ip);
                let responses = handle_logon(&mut session, basic_auth("wrong"))
                    .await
                    .unwrap();
                assert_eq!(
                    response_code(&responses),
                    Some("Neo.ClientError.Security.Unauthorized")
                );
            }

            let mut session = auth_session(ip);
            let responses = handle_logon(&mut session, basic_auth("wrong"))
                .await
                .unwrap();
            assert_eq!(
                response_code(&responses),
                Some("Neo.ClientError.Security.AuthenticationRateLimit")
            );
        })
        .await;
    }
}
