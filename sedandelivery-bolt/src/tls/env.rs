//! Strict parsing of TLS-related environment variables.

/// Read a TLS-related environment variable.
///
/// Returns `None` if the variable is unset, `Some(value)` if it is set to a
/// non-empty UTF-8 string. Panics if the variable is set to an empty string
/// or contains non-Unicode bytes — these are misconfigurations that should
/// fail loudly at startup.
pub(super) fn read_tls_env_var(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if value.is_empty() => panic!("{name} is set but empty"),
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => panic!("{name} is not valid unicode"),
    }
}
