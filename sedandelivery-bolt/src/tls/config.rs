//! Top-level TLS configuration loader.

use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

use tokio_rustls::TlsAcceptor;

use super::env::read_tls_env_var;
use super::keys::read_private_key;
use super::self_signed::is_self_signed_pem;

/// Load TLS configuration from environment variables.
///
/// Returns `Some(TlsAcceptor)` when both `BOLT_TLS_CERT` and `BOLT_TLS_KEY`
/// point to valid PEM files. Returns `None` only when both variables are
/// unset, allowing the server to operate in plaintext mode.
///
/// When `BOLT_ALLOW_SELF_SIGNED=1` is set, a warning is logged if the cert
/// is self-signed but startup proceeds. Without it, a warning is also logged
/// (the cert is accepted — strict CA enforcement is handled by clients).
///
/// # Panics
///
/// Panics if the environment variables are set but the files cannot be read
/// or parsed. This is intentional: a misconfigured TLS setup should fail
/// loudly at startup rather than silently falling back to plaintext.
pub fn load_tls_config() -> Option<TlsAcceptor> {
    // Strict "1"-only boolean parse — matches BOLT_REQUIRE_TLS convention.
    let allow_self_signed = std::env::var("BOLT_ALLOW_SELF_SIGNED")
        .map(|v| v.trim() == "1")
        .unwrap_or(false);

    let cert_path = read_tls_env_var("BOLT_TLS_CERT");
    let key_path = read_tls_env_var("BOLT_TLS_KEY");

    let (cert_path, key_path) = match (cert_path, key_path) {
        (None, None) => return None,
        (Some(cert_path), Some(key_path)) => (cert_path, key_path),
        (Some(_), None) => panic!("BOLT_TLS_CERT is set but BOLT_TLS_KEY is missing"),
        (None, Some(_)) => panic!("BOLT_TLS_KEY is set but BOLT_TLS_CERT is missing"),
    };

    tracing::info!("TLS enabled: cert={cert_path}, key={key_path}");

    // Warn when the certificate appears to be self-signed.
    // BOLT_ALLOW_SELF_SIGNED=1 suppresses the warning (dev mode only).
    let cert_p = std::path::Path::new(&cert_path);
    if is_self_signed_pem(cert_p) {
        if allow_self_signed {
            tracing::warn!(
                cert = cert_path,
                "BOLT_ALLOW_SELF_SIGNED=1: self-signed certificate detected \
                 (acceptable in development; never set in production)"
            );
        } else {
            tracing::warn!(
                cert = cert_path,
                "self-signed certificate detected; set BOLT_ALLOW_SELF_SIGNED=1 \
                 to suppress this warning in development environments"
            );
        }
    }

    // Load certificate chain
    let cert_file = File::open(&cert_path)
        .unwrap_or_else(|e| panic!("failed to open TLS cert file {cert_path}: {e}"));
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| panic!("failed to parse TLS cert file {cert_path}: {e}"));
    if certs.is_empty() {
        panic!("TLS cert file {cert_path} contains no certificates");
    }

    // Load private key (try PKCS8, then RSA, then EC)
    let key_file = File::open(&key_path)
        .unwrap_or_else(|e| panic!("failed to open TLS key file {key_path}: {e}"));
    let mut key_reader = BufReader::new(key_file);
    let key = read_private_key(&mut key_reader)
        .unwrap_or_else(|| panic!("TLS key file {key_path} contains no private key"));

    let provider = rustls::crypto::ring::default_provider();
    let config = rustls::ServerConfig::builder_with_provider(std::sync::Arc::new(provider))
        .with_safe_default_protocol_versions()
        .expect("failed to set TLS protocol versions")
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("failed to build TLS ServerConfig");

    Some(TlsAcceptor::from(Arc::new(config)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::any::Any;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn unique_test_dir(name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "sedandelivery-bolt-{name}-{}-{suffix}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("failed to create temp test dir");
        dir
    }

    fn write_test_cert_and_key(dir: &Path) -> (PathBuf, PathBuf) {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("failed to generate self-signed cert");
        let cert_path = dir.join("test-cert.pem");
        let key_path = dir.join("test-key.pem");
        std::fs::write(&cert_path, cert.cert.pem()).expect("failed to write test cert");
        std::fs::write(&key_path, cert.signing_key.serialize_pem())
            .expect("failed to write test key");
        (cert_path, key_path)
    }

    fn panic_message(payload: Box<dyn Any + Send>) -> String {
        match payload.downcast::<String>() {
            Ok(message) => *message,
            Err(payload) => match payload.downcast::<&'static str>() {
                Ok(message) => (*message).to_string(),
                Err(_) => "non-string panic payload".to_string(),
            },
        }
    }

    fn expect_panic<F>(f: F, context: &str) -> String
    where
        F: FnOnce() -> Option<TlsAcceptor> + std::panic::UnwindSafe,
    {
        match std::panic::catch_unwind(f) {
            Ok(_) => panic!("{context}"),
            Err(payload) => panic_message(payload),
        }
    }

    struct TestEnv {
        prev_cert: Option<String>,
        prev_key: Option<String>,
        _lock: MutexGuard<'static, ()>,
    }

    impl TestEnv {
        fn set(cert: Option<&str>, key: Option<&str>) -> Self {
            let lock = ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let prev_cert = std::env::var("BOLT_TLS_CERT").ok();
            let prev_key = std::env::var("BOLT_TLS_KEY").ok();

            match cert {
                Some(value) => std::env::set_var("BOLT_TLS_CERT", value),
                None => std::env::remove_var("BOLT_TLS_CERT"),
            }
            match key {
                Some(value) => std::env::set_var("BOLT_TLS_KEY", value),
                None => std::env::remove_var("BOLT_TLS_KEY"),
            }

            Self {
                prev_cert,
                prev_key,
                _lock: lock,
            }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            if let Some(value) = &self.prev_cert {
                std::env::set_var("BOLT_TLS_CERT", value);
            } else {
                std::env::remove_var("BOLT_TLS_CERT");
            }

            if let Some(value) = &self.prev_key {
                std::env::set_var("BOLT_TLS_KEY", value);
            } else {
                std::env::remove_var("BOLT_TLS_KEY");
            }
        }
    }

    #[test]
    fn returns_none_without_env_vars() {
        let _env = TestEnv::set(None, None);

        let result = load_tls_config();
        assert!(
            result.is_none(),
            "expected None when TLS env vars are unset"
        );
    }

    #[test]
    fn returns_some_with_valid_cert_and_key() {
        let tmp_dir = unique_test_dir("valid-cert");
        let (cert_path, key_path) = write_test_cert_and_key(&tmp_dir);

        let _env = TestEnv::set(
            Some(cert_path.to_str().unwrap()),
            Some(key_path.to_str().unwrap()),
        );

        let result = load_tls_config();
        assert!(
            result.is_some(),
            "expected Some(TlsAcceptor) with valid cert/key"
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn panics_when_cert_file_is_missing() {
        let tmp_dir = unique_test_dir("missing-cert");
        let missing_cert = tmp_dir.join("missing-cert.pem");
        let (_, key_path) = write_test_cert_and_key(&tmp_dir);
        let _env = TestEnv::set(
            Some(missing_cert.to_str().unwrap()),
            Some(key_path.to_str().unwrap()),
        );

        let message = expect_panic(load_tls_config, "missing cert file must panic");
        assert!(
            message.contains("failed to open TLS cert file"),
            "unexpected panic message: {message}"
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    fn panics_when_key_file_is_missing() {
        let tmp_dir = unique_test_dir("missing-key");
        let (cert_path, _) = write_test_cert_and_key(&tmp_dir);
        let missing_key = tmp_dir.join("missing-key.pem");
        let _env = TestEnv::set(
            Some(cert_path.to_str().unwrap()),
            Some(missing_key.to_str().unwrap()),
        );

        let message = expect_panic(load_tls_config, "missing key file must panic");
        assert!(
            message.contains("failed to open TLS key file"),
            "unexpected panic message: {message}"
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[cfg(unix)]
    #[test]
    fn panics_when_cert_file_is_unreadable() {
        use std::os::unix::fs::PermissionsExt;

        let tmp_dir = unique_test_dir("unreadable-cert");
        let (cert_path, key_path) = write_test_cert_and_key(&tmp_dir);
        let original_mode = std::fs::metadata(&cert_path)
            .expect("cert metadata missing")
            .permissions()
            .mode();
        std::fs::set_permissions(&cert_path, std::fs::Permissions::from_mode(0o000))
            .expect("failed to remove cert permissions");
        let _env = TestEnv::set(
            Some(cert_path.to_str().unwrap()),
            Some(key_path.to_str().unwrap()),
        );

        let message = expect_panic(load_tls_config, "unreadable cert file must panic");
        std::fs::set_permissions(&cert_path, std::fs::Permissions::from_mode(original_mode))
            .expect("failed to restore cert permissions");
        assert!(
            message.contains("failed to open TLS cert file"),
            "unexpected panic message: {message}"
        );

        let _ = std::fs::remove_dir_all(&tmp_dir);
    }

    #[test]
    #[should_panic]
    fn panics_when_only_cert_is_set() {
        let _env = TestEnv::set(Some("/tmp/dummy-cert.pem"), None);
        load_tls_config();
    }

    #[test]
    #[should_panic]
    fn panics_when_only_key_is_set() {
        let _env = TestEnv::set(None, Some("/tmp/dummy-key.pem"));
        load_tls_config();
    }

    #[test]
    #[should_panic]
    fn panics_when_cert_is_empty_string() {
        let _env = TestEnv::set(Some(""), None);
        load_tls_config();
    }

    #[test]
    #[should_panic]
    fn panics_when_key_is_empty_string() {
        let _env = TestEnv::set(None, Some(""));
        load_tls_config();
    }
}
