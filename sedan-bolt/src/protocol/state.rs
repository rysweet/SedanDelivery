use crate::message::types::ClientMessage;

/// Bolt server connection state (Bolt 4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Negotiation,
    Authentication,
    Ready,
    Streaming,
    TxReady,
    TxStreaming,
    Failed,
    Interrupted,
    Defunct,
}

impl ServerState {
    /// Returns true if this state accepts the given message type.
    pub fn accepts(&self, msg: &ClientMessage) -> bool {
        use ClientMessage::*;
        use ServerState::*;
        match (self, msg) {
            (Authentication, Hello(_)) => true,
            (Authentication, Logon(_)) => true,
            (Ready, Run { .. } | Begin(_) | Reset | Goodbye) => true,
            (Streaming, Pull(_) | Discard(_) | Run { .. } | Reset) => true,
            (TxReady, Run { .. } | Commit | Rollback | Reset) => true,
            (TxStreaming, Pull(_) | Discard(_) | Run { .. } | Reset) => true,
            (Failed, Reset | Goodbye) => true,
            (Failed, _) => true,      // accept but respond with IGNORED
            (Interrupted, _) => true, // accept but respond with IGNORED
            _ => false,
        }
    }
}
