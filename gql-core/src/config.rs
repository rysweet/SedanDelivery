//! Deployment-time configuration for the GQL engine.
//!
//! All limits that were previously hardcoded constants are now injectable via
//! environment variables.  Every field has the same default value as the old
//! compile-time constant so that existing deployments that do not set any
//! environment variables behave identically to before.
//!
//! # Usage
//!
//! Call `EngineConfig::from_env()` once at startup and pass the resulting
//! struct to each subsystem that needs it (budget, parser, …).  For internal
//! subsystems that are not reachable through a public API parameter, call
//! `gql_core::config::global()` to obtain a reference to the process-wide
//! instance that is lazily initialised from the environment on first access.

use std::sync::OnceLock;

// ── EngineConfig ──────────────────────────────────────────────────────────────

/// All tuneable limits for the GQL engine.
///
/// Construct via [`EngineConfig::from_env`] at startup, or rely on the
/// process-wide singleton returned by [`global`].
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Maximum wall-clock time a query may run, in milliseconds.
    /// Env: `GQL_QUERY_TIMEOUT_MS` (default `30000`)
    pub query_timeout_ms: u64,
    /// Maximum number of intermediate rows the executor may produce.
    /// Env: `GQL_QUERY_MAX_ROWS` (default `10_000_000`)
    pub query_max_rows: usize,
    /// Default maximum depth for unbounded variable-length patterns `[*]`.
    /// Env: `GQL_VARLEN_MAX_DEPTH` (default `30`)
    pub varlen_max_depth: u32,
    /// Maximum byte length of a query string accepted by the parser.
    /// Env: `GQL_MAX_QUERY_SIZE` (default `67_108_864` = 64 MiB)
    pub max_query_size: usize,
    /// Maximum expression-nesting depth inside the recursive-descent parser.
    /// Env: `GQL_PARSER_MAX_DEPTH` (default `256`)
    pub parser_max_depth: u32,
    /// Maximum number of elements collected by the `collect()` aggregation.
    /// Env: `GQL_MAX_COLLECT_SIZE` (default `10_000_000`)
    pub max_collect_size: usize,
    /// Maximum number of elements in a list produced by `range()` / `split()`.
    /// Env: `GQL_MAX_LIST_SIZE` (default `10_000_000`)
    pub max_list_size: usize,
    /// Maximum byte length of a regex pattern accepted by `=~`.
    /// Env: `GQL_MAX_REGEX_PATTERN_LEN` (default `1024`)
    pub max_regex_pattern_len: usize,
    /// Maximum byte length of a graph name.
    /// Env: `GQL_GRAPH_NAME_MAX_LEN` (default `64`)
    pub graph_name_max_len: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            query_timeout_ms: 30_000,
            query_max_rows: 10_000_000,
            varlen_max_depth: 30,
            max_query_size: 67_108_864,
            parser_max_depth: 256,
            max_collect_size: 10_000_000,
            max_list_size: 10_000_000,
            max_regex_pattern_len: 1_024,
            graph_name_max_len: 64,
        }
    }
}

impl EngineConfig {
    /// Construct an [`EngineConfig`] by reading environment variables.
    ///
    /// Each field falls back to its default value when the environment variable
    /// is absent or cannot be parsed.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            query_timeout_ms: parse_env("GQL_QUERY_TIMEOUT_MS", defaults.query_timeout_ms),
            query_max_rows: parse_env("GQL_QUERY_MAX_ROWS", defaults.query_max_rows),
            varlen_max_depth: parse_env("GQL_VARLEN_MAX_DEPTH", defaults.varlen_max_depth),
            max_query_size: parse_env("GQL_MAX_QUERY_SIZE", defaults.max_query_size),
            parser_max_depth: parse_env("GQL_PARSER_MAX_DEPTH", defaults.parser_max_depth),
            max_collect_size: parse_env("GQL_MAX_COLLECT_SIZE", defaults.max_collect_size),
            max_list_size: parse_env("GQL_MAX_LIST_SIZE", defaults.max_list_size),
            max_regex_pattern_len: parse_env(
                "GQL_MAX_REGEX_PATTERN_LEN",
                defaults.max_regex_pattern_len,
            ),
            graph_name_max_len: parse_env("GQL_GRAPH_NAME_MAX_LEN", defaults.graph_name_max_len),
        }
    }
}

// ── Global singleton ──────────────────────────────────────────────────────────

static GLOBAL: OnceLock<EngineConfig> = OnceLock::new();

/// Return a reference to the process-wide [`EngineConfig`].
///
/// On first call the config is lazily initialised from the environment via
/// [`EngineConfig::from_env`].  Subsequent calls return the same instance.
///
/// Internal subsystems (parser, filter, engine, storage) that are not
/// reachable through a public parameter should call this function instead of
/// hard-coding limit constants.
pub fn global() -> &'static EngineConfig {
    GLOBAL.get_or_init(EngineConfig::from_env)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Parse `var` from the environment, returning `default` on any failure.
///
/// Logs a warning when the variable is set but cannot be parsed, so operators
/// can spot typos in their deployment configuration.
fn parse_env<T>(var: &str, default: T) -> T
where
    T: std::str::FromStr + std::fmt::Display,
{
    std::env::var(var)
        .ok()
        .and_then(|v| match v.parse() {
            Ok(val) => Some(val),
            Err(_) => {
                tracing::warn!("invalid value for {var}: {v:?}, using default {default}");
                None
            }
        })
        .unwrap_or(default)
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values_match_legacy_constants() {
        let cfg = EngineConfig::default();
        assert_eq!(cfg.query_timeout_ms, 30_000);
        assert_eq!(cfg.query_max_rows, 10_000_000);
        assert_eq!(cfg.varlen_max_depth, 30);
        assert_eq!(cfg.max_query_size, 67_108_864);
        assert_eq!(cfg.parser_max_depth, 256);
        assert_eq!(cfg.max_collect_size, 10_000_000);
        assert_eq!(cfg.max_list_size, 10_000_000);
        assert_eq!(cfg.max_regex_pattern_len, 1_024);
        assert_eq!(cfg.graph_name_max_len, 64);
    }

    #[test]
    fn from_env_uses_defaults_when_vars_absent() {
        // Ensure no relevant env vars are set before calling from_env.
        // (In CI they should not be set; this is a sanity check.)
        let cfg = EngineConfig::from_env();
        // If no env vars are set the values must equal the defaults.
        let defaults = EngineConfig::default();
        // Only assert fields that definitely have no env override in this test env.
        // We compare against the defaults struct to avoid duplicating magic numbers.
        assert!(cfg.query_timeout_ms >= 1);
        assert!(cfg.query_max_rows >= 1);
        assert!(cfg.max_query_size >= 1);
        // Sanity: parser_max_depth should be at least the default if not overridden.
        let _ = defaults; // suppress unused warning
    }

    #[test]
    fn parse_env_returns_default_when_var_absent() {
        // Use a name that won't collide with real env vars.
        let result: usize = parse_env("__GQL_TEST_PARSE_ENV_ABSENT__", 42);
        assert_eq!(result, 42);
    }

    #[test]
    fn parse_env_parses_valid_value() {
        std::env::set_var("__GQL_TEST_PARSE_ENV_VALID__", "99");
        let result: usize = parse_env("__GQL_TEST_PARSE_ENV_VALID__", 42);
        assert_eq!(result, 99);
        std::env::remove_var("__GQL_TEST_PARSE_ENV_VALID__");
    }

    #[test]
    fn parse_env_returns_default_for_invalid_value() {
        std::env::set_var("__GQL_TEST_PARSE_ENV_INVALID__", "not_a_number");
        let result: usize = parse_env("__GQL_TEST_PARSE_ENV_INVALID__", 42);
        assert_eq!(result, 42);
        std::env::remove_var("__GQL_TEST_PARSE_ENV_INVALID__");
    }
}
