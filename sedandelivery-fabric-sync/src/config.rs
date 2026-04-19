//! Configuration helpers — env-var parsing, Redis timeouts, URL redaction.
//!
//! Pure functions and small value types that don't depend on a live Redis
//! connection. Kept separate from `main.rs` so unit tests don't need to
//! mutate the process environment (`std::env::set_var` is `unsafe` in
//! Rust 2024).

use std::time::Duration;

use tracing::warn;

/// Consumer group name used on every per-graph changelog stream.
pub(crate) const GROUP_NAME: &str = "sedandelivery-fabric-sync";

#[derive(Clone, Copy)]
pub(crate) struct RedisTimeouts {
    pub(crate) connect_timeout: Duration,
    pub(crate) read_timeout: Duration,
    pub(crate) write_timeout: Duration,
}

pub(crate) fn env_usize(key: &str, default: usize) -> usize {
    parse_env_usize(std::env::var(key).ok().as_deref(), default, key)
}

/// Pure parser for `env_usize` — returns `default` on `None` or an unparseable value
/// and emits the same warning. Separated out so unit tests don't have to mutate
/// the process environment (`std::env::set_var` is `unsafe` in Rust 2024).
pub(crate) fn parse_env_usize(value: Option<&str>, default: usize, key: &str) -> usize {
    match value {
        None => default,
        Some(v) => v.parse().unwrap_or_else(|e| {
            warn!("{key}=<invalid> is not a valid usize ({e}), using default {default}");
            default
        }),
    }
}

pub(crate) fn env_duration_ms(key: &str, default_ms: u64) -> Duration {
    Duration::from_millis(parse_env_u64(
        std::env::var(key).ok().as_deref(),
        default_ms,
        key,
    ))
}

/// Pure parser for `env_duration_ms` — see `parse_env_usize` for rationale.
pub(crate) fn parse_env_u64(value: Option<&str>, default: u64, key: &str) -> u64 {
    match value {
        None => default,
        Some(v) => v.parse().unwrap_or_else(|e| {
            warn!("{key}=<invalid> is not a valid u64 ({e}), using default {default}");
            default
        }),
    }
}

pub(crate) fn redis_timeouts(block_ms: usize) -> RedisTimeouts {
    let default_read_ms = ((block_ms as u64) + 5_000).max(5_000);
    RedisTimeouts {
        connect_timeout: env_duration_ms("SD2_SYNC_REDIS_CONNECT_TIMEOUT_MS", 5_000),
        read_timeout: env_duration_ms("SD2_SYNC_REDIS_READ_TIMEOUT_MS", default_read_ms),
        write_timeout: env_duration_ms("SD2_SYNC_REDIS_WRITE_TIMEOUT_MS", 5_000),
    }
}

pub(crate) fn redact_url_credentials(url: &str) -> String {
    let Some((scheme, remainder)) = url.split_once("://") else {
        return url.to_string();
    };

    let authority_end = remainder.find(['/', '?', '#']).unwrap_or(remainder.len());
    let authority = &remainder[..authority_end];
    let suffix = &remainder[authority_end..];

    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return url.to_string();
    };

    if userinfo.is_empty() {
        return url.to_string();
    }

    format!("{scheme}://***@{host}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_usize_returns_default_when_unset() {
        let val = env_usize("__FABRIC_SYNC_TEST_NEVER_SET_12345", 42);
        assert_eq!(val, 42);
    }

    // The pure parsers below let us exercise the parsing branches without
    // mutating the process environment (which `std::env::set_var` requires
    // and is `unsafe` in Rust 2024).

    #[test]
    fn parse_env_usize_returns_default_when_none() {
        assert_eq!(parse_env_usize(None, 42, "TEST_KEY"), 42);
    }

    #[test]
    fn parse_env_usize_parses_valid_value() {
        assert_eq!(parse_env_usize(Some("99"), 0, "TEST_KEY"), 99);
    }

    #[test]
    fn parse_env_usize_invalid_returns_default() {
        assert_eq!(parse_env_usize(Some("not_a_number"), 77, "TEST_KEY"), 77);
    }

    #[test]
    fn parse_env_u64_returns_default_when_none() {
        assert_eq!(parse_env_u64(None, 100, "TEST_KEY"), 100);
    }

    #[test]
    fn parse_env_u64_parses_valid_value() {
        assert_eq!(parse_env_u64(Some("12345"), 0, "TEST_KEY"), 12345);
    }

    #[test]
    fn parse_env_u64_invalid_returns_default() {
        assert_eq!(parse_env_u64(Some("nope"), 9, "TEST_KEY"), 9);
    }

    #[test]
    fn redact_url_credentials_preserves_host_and_path() {
        let redacted = redact_url_credentials("redis://user:secret@127.0.0.1:6379/0");
        assert_eq!(redacted, "redis://***@127.0.0.1:6379/0");
    }

    #[test]
    fn redact_url_credentials_handles_password_only_urls() {
        let redacted = redact_url_credentials("rediss://:secret@example.com:6380?tls=true");
        assert_eq!(redacted, "rediss://***@example.com:6380?tls=true");
    }

    #[test]
    fn redact_url_credentials_leaves_credential_free_urls_unchanged() {
        let redacted = redact_url_credentials("redis://127.0.0.1:6379");
        assert_eq!(redacted, "redis://127.0.0.1:6379");
    }
}
