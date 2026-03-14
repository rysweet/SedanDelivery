//! TLS configuration for the sedandelivery-bolt server.
//!
//! Reads `BOLT_TLS_CERT` and `BOLT_TLS_KEY` environment variables to
//! load a PEM certificate chain and private key. When both are set,
//! returns a [`TlsAcceptor`] that can wrap incoming TCP connections.
//! When both are absent, returns `None` so the server falls back to
//! plaintext. Partial or empty TLS configuration is treated as a
//! startup error.

use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

use rustls::pki_types::PrivateKeyDer;
use tokio_rustls::TlsAcceptor;

/// Load TLS configuration from environment variables.
///
/// Returns `Some(TlsAcceptor)` when both `BOLT_TLS_CERT` and `BOLT_TLS_KEY`
/// point to valid PEM files. Returns `None` only when both variables are
/// unset, allowing the server to operate in plaintext mode.
///
/// # Panics
///
/// Panics if the environment variables are set but the files cannot be read
/// or parsed. This is intentional: a misconfigured TLS setup should fail
/// loudly at startup rather than silently falling back to plaintext.
pub fn load_tls_config() -> Option<TlsAcceptor> {
    let cert_path = read_tls_env_var("BOLT_TLS_CERT");
    let key_path = read_tls_env_var("BOLT_TLS_KEY");

    let (cert_path, key_path) = match (cert_path, key_path) {
        (None, None) => return None,
        (Some(cert_path), Some(key_path)) => (cert_path, key_path),
        (Some(_), None) => panic!("BOLT_TLS_CERT is set but BOLT_TLS_KEY is missing"),
        (None, Some(_)) => panic!("BOLT_TLS_KEY is set but BOLT_TLS_CERT is missing"),
    };

    tracing::info!("TLS enabled: cert={cert_path}, key={key_path}");

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

fn read_tls_env_var(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(value) if value.is_empty() => panic!("{name} is set but empty"),
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => panic!("{name} is not valid unicode"),
    }
}

/// Read the first private key found in PEM data. Supports PKCS8, RSA,
/// and EC key formats.
fn read_private_key(reader: &mut BufReader<File>) -> Option<PrivateKeyDer<'static>> {
    use rustls_pemfile::Item;

    for item in rustls_pemfile::read_all(reader) {
        match item {
            Ok(Item::Pkcs8Key(key)) => return Some(PrivateKeyDer::Pkcs8(key)),
            Ok(Item::Pkcs1Key(key)) => return Some(PrivateKeyDer::Pkcs1(key)),
            Ok(Item::Sec1Key(key)) => return Some(PrivateKeyDer::Sec1(key)),
            _ => continue,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

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
        // Generate a self-signed certificate using rcgen
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("failed to generate self-signed cert");

        let cert_pem = cert.cert.pem();
        let key_pem = cert.signing_key.serialize_pem();

        // Write to temp files
        let tmp_dir = std::env::temp_dir().join("sedandelivery-bolt-tls-test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let cert_path = tmp_dir.join("test-cert.pem");
        let key_path = tmp_dir.join("test-key.pem");
        std::fs::write(&cert_path, cert_pem).unwrap();
        std::fs::write(&key_path, key_pem).unwrap();

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
