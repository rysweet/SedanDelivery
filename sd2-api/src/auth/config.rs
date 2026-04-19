//! Entra ID configuration loaded from environment variables.

use std::sync::OnceLock;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct EntraConfig {
    pub tenant_id: String,
    pub client_id: String,
}

impl EntraConfig {
    fn from_env() -> Option<Self> {
        let tenant_id = std::env::var("GQL_API_ENTRA_TENANT_ID").ok()?;
        let client_id = std::env::var("GQL_API_ENTRA_CLIENT_ID").ok()?;
        Some(Self {
            tenant_id,
            client_id,
        })
    }
}

pub fn entra_config() -> Option<&'static EntraConfig> {
    static CFG: OnceLock<Option<EntraConfig>> = OnceLock::new();
    CFG.get_or_init(EntraConfig::from_env).as_ref()
}

pub(super) fn env_duration_secs(name: &str, default: u64) -> Duration {
    Duration::from_secs(
        std::env::var(name)
            .ok()
            .and_then(|v| match v.parse::<u64>() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid non-numeric value for {name}: {v:?}, using default {default}"
                    );
                    None
                }
            })
            .unwrap_or(default),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the old version of this test was tautological — it
    /// branched on the current env state and always asserted the matching
    /// arm, so it could never fail.  The replacement explicitly clears the
    /// env vars, asserts `None`, then sets them and asserts `Some`.
    ///
    /// WARNING: mutates process-wide env vars. Not safe to run in parallel
    /// with other tests that read these variables.
    #[test]
    fn entra_config_from_env_requires_both_vars() {
        let saved_tenant = std::env::var("GQL_API_ENTRA_TENANT_ID").ok();
        let saved_client = std::env::var("GQL_API_ENTRA_CLIENT_ID").ok();

        // Neither var set → None
        std::env::remove_var("GQL_API_ENTRA_TENANT_ID");
        std::env::remove_var("GQL_API_ENTRA_CLIENT_ID");
        assert!(
            EntraConfig::from_env().is_none(),
            "expected None when both env vars are missing"
        );

        // Only tenant → None
        std::env::set_var("GQL_API_ENTRA_TENANT_ID", "t");
        assert!(
            EntraConfig::from_env().is_none(),
            "expected None when only TENANT_ID is set"
        );
        std::env::remove_var("GQL_API_ENTRA_TENANT_ID");

        // Only client → None
        std::env::set_var("GQL_API_ENTRA_CLIENT_ID", "c");
        assert!(
            EntraConfig::from_env().is_none(),
            "expected None when only CLIENT_ID is set"
        );

        // Both set → Some
        std::env::set_var("GQL_API_ENTRA_TENANT_ID", "t");
        let cfg = EntraConfig::from_env();
        assert!(cfg.is_some(), "expected Some when both env vars are set");
        let cfg = cfg.unwrap();
        assert_eq!(cfg.tenant_id, "t");
        assert_eq!(cfg.client_id, "c");

        // Restore
        std::env::remove_var("GQL_API_ENTRA_TENANT_ID");
        std::env::remove_var("GQL_API_ENTRA_CLIENT_ID");
        if let Some(v) = saved_tenant {
            std::env::set_var("GQL_API_ENTRA_TENANT_ID", v);
        }
        if let Some(v) = saved_client {
            std::env::set_var("GQL_API_ENTRA_CLIENT_ID", v);
        }
    }

    #[test]
    fn env_duration_secs_uses_default() {
        let d = env_duration_secs("__NONEXISTENT_ENV_VAR__", 42);
        assert_eq!(d, Duration::from_secs(42));
    }
}
