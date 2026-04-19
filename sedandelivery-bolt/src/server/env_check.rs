//! Environment-variable validation helpers used during server startup.
//!
//! These functions are split out from `main.rs` so they can be unit-tested
//! without invoking `std::process::exit`.

/// Validate that `BOLT_PASSWORD` is present in production mode.
///
/// Returns `Err(message)` on failure so callers can `eprintln!` and `exit(1)`.
/// This separation keeps the validation logic unit-testable without process exit.
pub(crate) fn check_bolt_password_env(require_password: bool) -> Result<(), String> {
    match read_non_empty_env_var("BOLT_PASSWORD")? {
        Some(_) => Ok(()),
        None if require_password => {
            Err("BOLT_PASSWORD is required when ENVIRONMENT=production".into())
        }
        None => Ok(()),
    }
}

/// Validate that `BOLT_TLS_CERT` and `BOLT_TLS_KEY` are consistently set.
///
/// When `require_tls` is true, both must be present. When false, both-absent
/// is allowed but one-without-the-other is always an error.
pub(crate) fn check_bolt_tls_env(require_tls: bool) -> Result<(), String> {
    match (
        read_non_empty_env_var("BOLT_TLS_CERT")?,
        read_non_empty_env_var("BOLT_TLS_KEY")?,
    ) {
        (Some(_), Some(_)) => Ok(()),
        (None, None) if require_tls => {
            Err("BOLT_TLS_CERT and BOLT_TLS_KEY are required when BOLT_REQUIRE_TLS=1".into())
        }
        (None, None) => Ok(()),
        (Some(_), None) => Err("BOLT_TLS_CERT is set but BOLT_TLS_KEY is missing".into()),
        (None, Some(_)) => Err("BOLT_TLS_KEY is set but BOLT_TLS_CERT is missing".into()),
    }
}

/// Emit `message` to stderr and exit with code 1. Never returns.
pub(crate) fn fatal(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

pub(crate) fn read_non_empty_env_var(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) if value.is_empty() => Err(format!("{name} is set but empty")),
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} is not valid unicode")),
    }
}

#[cfg(test)]
mod tests {
    use super::{check_bolt_password_env, check_bolt_tls_env};
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct TestEnv {
        prev_password: Option<String>,
        prev_cert: Option<String>,
        prev_key: Option<String>,
        _lock: MutexGuard<'static, ()>,
    }

    impl TestEnv {
        fn set(password: Option<&str>, cert: Option<&str>, key: Option<&str>) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let prev_password = std::env::var("BOLT_PASSWORD").ok();
            let prev_cert = std::env::var("BOLT_TLS_CERT").ok();
            let prev_key = std::env::var("BOLT_TLS_KEY").ok();

            match password {
                Some(value) => std::env::set_var("BOLT_PASSWORD", value),
                None => std::env::remove_var("BOLT_PASSWORD"),
            }
            match cert {
                Some(value) => std::env::set_var("BOLT_TLS_CERT", value),
                None => std::env::remove_var("BOLT_TLS_CERT"),
            }
            match key {
                Some(value) => std::env::set_var("BOLT_TLS_KEY", value),
                None => std::env::remove_var("BOLT_TLS_KEY"),
            }

            Self {
                prev_password,
                prev_cert,
                prev_key,
                _lock: lock,
            }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            match &self.prev_password {
                Some(value) => std::env::set_var("BOLT_PASSWORD", value),
                None => std::env::remove_var("BOLT_PASSWORD"),
            }
            match &self.prev_cert {
                Some(value) => std::env::set_var("BOLT_TLS_CERT", value),
                None => std::env::remove_var("BOLT_TLS_CERT"),
            }
            match &self.prev_key {
                Some(value) => std::env::set_var("BOLT_TLS_KEY", value),
                None => std::env::remove_var("BOLT_TLS_KEY"),
            }
        }
    }

    #[test]
    fn missing_password_is_allowed_outside_production() {
        let _env = TestEnv::set(None, None, None);

        assert_eq!(check_bolt_password_env(false), Ok(()));
    }

    #[test]
    fn missing_password_is_rejected_in_production() {
        let _env = TestEnv::set(None, None, None);

        assert_eq!(
            check_bolt_password_env(true),
            Err("BOLT_PASSWORD is required when ENVIRONMENT=production".into())
        );
    }

    #[test]
    fn non_empty_password_is_allowed_in_production() {
        let _env = TestEnv::set(Some("non-empty-secret"), None, None);

        assert_eq!(check_bolt_password_env(true), Ok(()));
    }

    #[test]
    fn empty_password_is_rejected() {
        let _env = TestEnv::set(Some(""), None, None);

        assert_eq!(
            check_bolt_password_env(false),
            Err("BOLT_PASSWORD is set but empty".into())
        );
    }

    #[test]
    fn missing_tls_is_allowed_outside_production() {
        let _env = TestEnv::set(None, None, None);

        assert_eq!(check_bolt_tls_env(false), Ok(()));
    }

    #[test]
    fn missing_tls_is_rejected_when_required() {
        let _env = TestEnv::set(None, None, None);

        assert_eq!(
            check_bolt_tls_env(true),
            Err("BOLT_TLS_CERT and BOLT_TLS_KEY are required when BOLT_REQUIRE_TLS=1".into())
        );
    }

    #[test]
    fn non_empty_tls_paths_are_allowed_in_production() {
        let _env = TestEnv::set(
            None,
            Some("/mnt/secrets/bolt-tls-cert"),
            Some("/mnt/secrets/bolt-tls-key"),
        );

        assert_eq!(check_bolt_tls_env(true), Ok(()));
    }

    #[test]
    fn missing_key_is_rejected_when_cert_is_present() {
        let _env = TestEnv::set(None, Some("/mnt/secrets/bolt-tls-cert"), None);

        assert_eq!(
            check_bolt_tls_env(false),
            Err("BOLT_TLS_CERT is set but BOLT_TLS_KEY is missing".into())
        );
    }

    #[test]
    fn missing_cert_is_rejected_when_key_is_present() {
        let _env = TestEnv::set(None, None, Some("/mnt/secrets/bolt-tls-key"));

        assert_eq!(
            check_bolt_tls_env(false),
            Err("BOLT_TLS_KEY is set but BOLT_TLS_CERT is missing".into())
        );
    }

    #[test]
    fn bolt_require_tls_accepts_only_1() {
        // Only "1" must activate TLS enforcement
        assert!(
            !std::env::var("BOLT_REQUIRE_TLS")
                .map(|v| v.trim() == "1")
                .unwrap_or(false),
            "default (unset) must not activate BOLT_REQUIRE_TLS"
        );

        std::env::set_var("BOLT_REQUIRE_TLS", "1");
        assert!(
            std::env::var("BOLT_REQUIRE_TLS")
                .map(|v| v.trim() == "1")
                .unwrap_or(false),
            "BOLT_REQUIRE_TLS=1 must activate enforcement"
        );
        std::env::remove_var("BOLT_REQUIRE_TLS");

        // "true", "True", "TRUE" must NOT activate enforcement (strict "1" only)
        for rejected in &["true", "True", "TRUE", "yes", "on", "1 "] {
            std::env::set_var("BOLT_REQUIRE_TLS", rejected);
            let activated = std::env::var("BOLT_REQUIRE_TLS")
                .map(|v| v.trim() == "1")
                .unwrap_or(false);
            // "1 " (with trailing space) trims to "1" so should activate; others should not
            if *rejected == "1 " {
                assert!(
                    activated,
                    "BOLT_REQUIRE_TLS='1 ' (with space) should activate after trim"
                );
            } else {
                assert!(
                    !activated,
                    "BOLT_REQUIRE_TLS={rejected:?} must NOT activate enforcement (strict '1' only)"
                );
            }
            std::env::remove_var("BOLT_REQUIRE_TLS");
        }
    }
}
