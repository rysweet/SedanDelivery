//! Bolt protocol message handler.
//!
//! Dispatches incoming `ClientMessage` values to the appropriate handler
//! function, manages session state transitions, and produces
//! `ServerMessage` responses.

mod bookmarks;
mod explain;
mod hello;
mod pull;
mod run;
mod session;
mod transaction;

use crate::message::types::{ClientMessage, ServerMessage};
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

// Re-export public types from sub-modules.
pub(crate) use bookmarks::{
    next_bookmark, recent_bookmark_age, record_write_bookmark, validate_bookmarks,
};
pub use hello::spawn_auth_rate_limiter_sweep;
#[cfg(test)]
pub(crate) use hello::with_required_password_override_for_tests;
pub(crate) use session::constant_time_eq;
pub(crate) use session::normalize_graph_name;
pub use session::READ_AFTER_WRITE_WINDOW;
pub use session::{AdminStats, PendingQuery, Session};

// -- Error type --------------------------------------------------------------

/// An error produced when the handler cannot process a message.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// The message is not valid in the current session state.
    #[error("message not accepted in current state {state:?}")]
    InvalidState { state: ServerState },

    /// An internal (non-protocol) error occurred.
    #[error("internal error: {0}")]
    Internal(String),
}

// -- Helper macro for building BoltMap literals ------------------------------

macro_rules! bolt_map {
    ($($key:expr => $val:expr),* $(,)?) => {{
        let entries: Vec<(String, BoltValue)> = vec![$(($key.to_string(), $val)),*];
        entries
    }};
}

// Re-export the macro for use in sub-modules.
pub(crate) use bolt_map;

// -- TX metadata extraction --------------------------------------------------

/// Extract transaction metadata from a message's extra map.
///
/// Drivers send metadata under either `"tx_metadata"` (Neo4j driver convention)
/// or `"metadata"` (Bolt spec). Returns the inner map, or an empty map if absent.
pub(crate) fn extract_tx_metadata(extra: &BoltMap) -> BoltMap {
    for key in &["tx_metadata", "metadata"] {
        if let Some(BoltValue::Map(m)) = extra.get_value(key) {
            return m.clone();
        }
    }
    Vec::new()
}

// -- Message handler ---------------------------------------------------------

/// Dispatch a `ClientMessage` to the appropriate handler, returning zero or
/// more `ServerMessage` responses.
///
/// This is the main entry point for protocol message processing. It enforces
/// the state machine (Failed/Interrupted handling) before delegating to the
/// per-message handler functions.
pub async fn handle_message(
    session: &mut Session,
    msg: ClientMessage,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    // In Failed state, only RESET and GOODBYE are handled normally;
    // everything else gets IGNORED.
    if session.state == ServerState::Failed {
        match &msg {
            ClientMessage::Reset => {}
            ClientMessage::Goodbye => {}
            _ => return Ok(vec![ServerMessage::Ignored]),
        }
    }

    // In Interrupted state, only RESET is handled normally;
    // everything else gets IGNORED.
    if session.state == ServerState::Interrupted && !matches!(&msg, ClientMessage::Reset) {
        return Ok(vec![ServerMessage::Ignored]);
    }

    // Check state accepts the message.
    if !session.state.accepts(&msg) {
        let prev_state = session.state;
        session.state = ServerState::Failed;
        return Ok(vec![ServerMessage::Failure(bolt_map! {
            "code" => BoltValue::String("Neo.ClientError.Request.Invalid".to_string()),
            "message" => BoltValue::String(format!(
                "message not accepted in current state {:?}",
                prev_state
            ))
        })]);
    }

    match msg {
        ClientMessage::Hello(extra) => hello::handle_hello(session, extra).await,
        ClientMessage::Logon(auth) => hello::handle_logon(session, auth).await,
        ClientMessage::Run {
            query,
            params,
            extra,
        } => run::handle_run(session, query, params, extra).await,
        ClientMessage::Pull(extra) => pull::handle_pull(session, extra).await,
        ClientMessage::Discard(extra) => pull::handle_discard(session, extra).await,
        ClientMessage::Begin(extra) => transaction::handle_begin(session, extra).await,
        ClientMessage::Commit => transaction::handle_commit(session).await,
        ClientMessage::Rollback => transaction::handle_rollback(session),
        ClientMessage::Reset => transaction::handle_reset(session),
        ClientMessage::Goodbye => {
            // Handled in session loop; should not reach here.
            Ok(vec![])
        }
    }
}

#[cfg(test)]
mod tests;
