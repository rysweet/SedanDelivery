//! Token-based authentication for the Gremlin WebSocket server.
//!
//! Supports two auth flows:
//! 1. **Upgrade-phase**: `Authorization: Bearer <token>` header during WebSocket handshake
//! 2. **First-message**: `"bearer": "<token>"` field in the first Gremlin request
//!
//! Uses constant-time comparison to prevent timing attacks (mirrors sedandelivery-bolt).

/// Authentication state for a WebSocket connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthState {
    /// Auth is disabled (no token configured). All messages pass through.
    Disabled,
    /// Client authenticated during WebSocket upgrade via Authorization header.
    Authenticated,
    /// Upgrade succeeded without Authorization header; first message must carry bearer token.
    PendingFirstMessage,
}

/// Constant-time byte comparison to prevent timing side-channels.
///
/// Always compares all bytes regardless of mismatch position.
/// Mirrors the implementation in `sedandelivery-bolt`.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let max_len = a.len().max(b.len());
    let mut diff = if a.len() != b.len() { 1u8 } else { 0u8 };
    for i in 0..max_len {
        let x = if i < a.len() { a[i] } else { 0 };
        let y = if i < b.len() { b[i] } else { 0 };
        diff |= x ^ y;
    }
    diff == 0
}

/// Extract the token from a `Bearer <token>` string.
///
/// Returns `Some(token)` if the string starts with `Bearer ` (case-insensitive prefix),
/// `None` otherwise. Leading/trailing whitespace on the token is trimmed.
pub fn parse_bearer_token(header_value: &str) -> Option<&str> {
    let trimmed = header_value.trim();
    if trimmed.len() < 8 {
        return None;
    }
    if trimmed[..7].eq_ignore_ascii_case("bearer ") {
        let token = trimmed[7..].trim();
        if token.is_empty() {
            None
        } else {
            Some(token)
        }
    } else {
        None
    }
}

/// Validate a presented token against the expected token using constant-time comparison.
pub fn validate_token(presented: &str, expected: &str) -> bool {
    constant_time_eq(presented.as_bytes(), expected.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── constant_time_eq ────────────────────────────────────────────────

    #[test]
    fn constant_time_eq_identical_strings() {
        assert!(constant_time_eq(b"secret-token", b"secret-token"));
    }

    #[test]
    fn constant_time_eq_different_strings() {
        assert!(!constant_time_eq(b"secret-token", b"wrong-token"));
    }

    #[test]
    fn constant_time_eq_different_lengths() {
        assert!(!constant_time_eq(b"short", b"much-longer-string"));
    }

    #[test]
    fn constant_time_eq_empty_strings() {
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn constant_time_eq_one_empty() {
        assert!(!constant_time_eq(b"notempty", b""));
        assert!(!constant_time_eq(b"", b"notempty"));
    }

    #[test]
    fn constant_time_eq_single_byte_diff() {
        assert!(!constant_time_eq(b"aaaa", b"aaab"));
    }

    // ── parse_bearer_token ──────────────────────────────────────────────

    #[test]
    fn parse_valid_bearer_header() {
        assert_eq!(
            parse_bearer_token("Bearer my-secret-token"),
            Some("my-secret-token")
        );
    }

    #[test]
    fn parse_bearer_case_insensitive() {
        assert_eq!(parse_bearer_token("bearer my-token"), Some("my-token"));
        assert_eq!(parse_bearer_token("BEARER my-token"), Some("my-token"));
        assert_eq!(parse_bearer_token("BeArEr my-token"), Some("my-token"));
    }

    #[test]
    fn parse_bearer_with_extra_whitespace() {
        assert_eq!(
            parse_bearer_token("  Bearer   my-token  "),
            Some("my-token")
        );
    }

    #[test]
    fn parse_bearer_missing_prefix() {
        assert_eq!(parse_bearer_token("my-token"), None);
    }

    #[test]
    fn parse_bearer_wrong_scheme() {
        assert_eq!(parse_bearer_token("Basic dXNlcjpwYXNz"), None);
    }

    #[test]
    fn parse_bearer_empty_token() {
        assert_eq!(parse_bearer_token("Bearer "), None);
        assert_eq!(parse_bearer_token("Bearer   "), None);
    }

    #[test]
    fn parse_bearer_empty_string() {
        assert_eq!(parse_bearer_token(""), None);
    }

    #[test]
    fn parse_bearer_just_prefix() {
        assert_eq!(parse_bearer_token("Bearer"), None);
    }

    // ── validate_token ──────────────────────────────────────────────────

    #[test]
    fn validate_token_correct() {
        assert!(validate_token("my-secret", "my-secret"));
    }

    #[test]
    fn validate_token_incorrect() {
        assert!(!validate_token("wrong-secret", "my-secret"));
    }

    #[test]
    fn validate_token_empty_presented() {
        assert!(!validate_token("", "my-secret"));
    }

    // ── AuthState ───────────────────────────────────────────────────────

    #[test]
    fn auth_state_debug_does_not_leak_tokens() {
        // AuthState enum variants contain no token data — verify Debug output is safe
        let state = AuthState::PendingFirstMessage;
        let debug = format!("{state:?}");
        assert!(!debug.contains("secret"));
        assert!(!debug.contains("token"));
        assert!(debug.contains("PendingFirstMessage"));
    }

    #[test]
    fn auth_state_equality() {
        assert_eq!(AuthState::Disabled, AuthState::Disabled);
        assert_eq!(AuthState::Authenticated, AuthState::Authenticated);
        assert_eq!(
            AuthState::PendingFirstMessage,
            AuthState::PendingFirstMessage
        );
        assert_ne!(AuthState::Disabled, AuthState::Authenticated);
        assert_ne!(AuthState::Authenticated, AuthState::PendingFirstMessage);
    }
}
