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
            (Failed, _) => false, // only Reset and Goodbye accepted in Failed state
            (Interrupted, _) => true, // accept but respond with IGNORED
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Authentication state ────────────────────────────────────────────

    #[test]
    fn authentication_accepts_hello() {
        assert!(ServerState::Authentication.accepts(&ClientMessage::Hello(vec![])));
    }

    #[test]
    fn authentication_accepts_logon() {
        assert!(ServerState::Authentication.accepts(&ClientMessage::Logon(vec![])));
    }

    #[test]
    fn authentication_rejects_run() {
        let msg = ClientMessage::Run {
            query: String::new(),
            params: vec![],
            extra: vec![],
        };
        assert!(!ServerState::Authentication.accepts(&msg));
    }

    #[test]
    fn authentication_rejects_goodbye() {
        assert!(!ServerState::Authentication.accepts(&ClientMessage::Goodbye));
    }

    // ── Ready state ─────────────────────────────────────────────────────

    #[test]
    fn ready_accepts_run() {
        let msg = ClientMessage::Run {
            query: "RETURN 1".into(),
            params: vec![],
            extra: vec![],
        };
        assert!(ServerState::Ready.accepts(&msg));
    }

    #[test]
    fn ready_accepts_begin() {
        assert!(ServerState::Ready.accepts(&ClientMessage::Begin(vec![])));
    }

    #[test]
    fn ready_accepts_reset() {
        assert!(ServerState::Ready.accepts(&ClientMessage::Reset));
    }

    #[test]
    fn ready_accepts_goodbye() {
        assert!(ServerState::Ready.accepts(&ClientMessage::Goodbye));
    }

    #[test]
    fn ready_rejects_pull() {
        assert!(!ServerState::Ready.accepts(&ClientMessage::Pull(vec![])));
    }

    #[test]
    fn ready_rejects_commit() {
        assert!(!ServerState::Ready.accepts(&ClientMessage::Commit));
    }

    // ── Streaming state ─────────────────────────────────────────────────

    #[test]
    fn streaming_accepts_pull() {
        assert!(ServerState::Streaming.accepts(&ClientMessage::Pull(vec![])));
    }

    #[test]
    fn streaming_accepts_discard() {
        assert!(ServerState::Streaming.accepts(&ClientMessage::Discard(vec![])));
    }

    #[test]
    fn streaming_accepts_run() {
        let msg = ClientMessage::Run {
            query: String::new(),
            params: vec![],
            extra: vec![],
        };
        assert!(ServerState::Streaming.accepts(&msg));
    }

    #[test]
    fn streaming_accepts_reset() {
        assert!(ServerState::Streaming.accepts(&ClientMessage::Reset));
    }

    #[test]
    fn streaming_rejects_begin() {
        assert!(!ServerState::Streaming.accepts(&ClientMessage::Begin(vec![])));
    }

    // ── TxReady state ───────────────────────────────────────────────────

    #[test]
    fn tx_ready_accepts_run() {
        let msg = ClientMessage::Run {
            query: String::new(),
            params: vec![],
            extra: vec![],
        };
        assert!(ServerState::TxReady.accepts(&msg));
    }

    #[test]
    fn tx_ready_accepts_commit() {
        assert!(ServerState::TxReady.accepts(&ClientMessage::Commit));
    }

    #[test]
    fn tx_ready_accepts_rollback() {
        assert!(ServerState::TxReady.accepts(&ClientMessage::Rollback));
    }

    #[test]
    fn tx_ready_rejects_pull() {
        assert!(!ServerState::TxReady.accepts(&ClientMessage::Pull(vec![])));
    }

    // ── TxStreaming state ───────────────────────────────────────────────

    #[test]
    fn tx_streaming_accepts_pull() {
        assert!(ServerState::TxStreaming.accepts(&ClientMessage::Pull(vec![])));
    }

    #[test]
    fn tx_streaming_accepts_discard() {
        assert!(ServerState::TxStreaming.accepts(&ClientMessage::Discard(vec![])));
    }

    #[test]
    fn tx_streaming_rejects_commit() {
        assert!(!ServerState::TxStreaming.accepts(&ClientMessage::Commit));
    }

    // ── Failed state ────────────────────────────────────────────────────

    #[test]
    fn failed_accepts_reset() {
        assert!(ServerState::Failed.accepts(&ClientMessage::Reset));
    }

    #[test]
    fn failed_accepts_goodbye() {
        assert!(ServerState::Failed.accepts(&ClientMessage::Goodbye));
    }

    #[test]
    fn failed_rejects_run() {
        let msg = ClientMessage::Run {
            query: String::new(),
            params: vec![],
            extra: vec![],
        };
        assert!(!ServerState::Failed.accepts(&msg));
    }

    #[test]
    fn failed_rejects_pull() {
        assert!(!ServerState::Failed.accepts(&ClientMessage::Pull(vec![])));
    }

    #[test]
    fn failed_rejects_begin() {
        assert!(!ServerState::Failed.accepts(&ClientMessage::Begin(vec![])));
    }

    // ── Interrupted state ───────────────────────────────────────────────

    #[test]
    fn interrupted_accepts_everything() {
        // Interrupted state accepts any message (responds with IGNORED).
        assert!(ServerState::Interrupted.accepts(&ClientMessage::Reset));
        assert!(ServerState::Interrupted.accepts(&ClientMessage::Goodbye));
        assert!(ServerState::Interrupted.accepts(&ClientMessage::Pull(vec![])));
        let run = ClientMessage::Run {
            query: String::new(),
            params: vec![],
            extra: vec![],
        };
        assert!(ServerState::Interrupted.accepts(&run));
        assert!(ServerState::Interrupted.accepts(&ClientMessage::Begin(vec![])));
        assert!(ServerState::Interrupted.accepts(&ClientMessage::Commit));
    }

    // ── Enum properties ─────────────────────────────────────────────────

    #[test]
    fn server_state_clone_and_eq() {
        let s = ServerState::Ready;
        let s2 = s;
        assert_eq!(s, s2);
    }

    #[test]
    fn server_state_debug() {
        let dbg = format!("{:?}", ServerState::Defunct);
        assert_eq!(dbg, "Defunct");
    }
}
