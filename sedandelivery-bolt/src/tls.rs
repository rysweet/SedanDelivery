//! TLS configuration for the sedandelivery-bolt server.
//!
//! Reads `BOLT_TLS_CERT` and `BOLT_TLS_KEY` environment variables to
//! load a PEM certificate chain and private key. When both are set,
//! returns a [`TlsAcceptor`] that can wrap incoming TCP connections.
//! When either is missing, returns `None` so the server falls back to
//! plaintext.

use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

use rustls::pki_types::PrivateKeyDer;
use tokio_rustls::TlsAcceptor;

/// Load TLS configuration from environment variables.
///
/// Returns `Some(TlsAcceptor)` when both `BOLT_TLS_CERT` and `BOLT_TLS_KEY`
/// point to valid PEM files. Returns `None` if either variable is unset,
/// allowing the server to operate in plaintext mode.
///
/// # Panics
///
/// Panics if the environment variables are set but the files cannot be read
/// or parsed. This is intentional: a misconfigured TLS setup should fail
/// loudly at startup rather than silently falling back to plaintext.
pub fn load_tls_config() -> Option<TlsAcceptor> {
    let cert_path = std::env::var("BOLT_TLS_CERT").ok()?;
    let key_path = std::env::var("BOLT_TLS_KEY").ok()?;

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

    #[test]
    fn returns_none_without_env_vars() {
        // Ensure the env vars are not set in this test process.
        // Since tests run in the same process, we temporarily remove them.
        let prev_cert = std::env::var("BOLT_TLS_CERT").ok();
        let prev_key = std::env::var("BOLT_TLS_KEY").ok();
        std::env::remove_var("BOLT_TLS_CERT");
        std::env::remove_var("BOLT_TLS_KEY");

        let result = load_tls_config();
        assert!(
            result.is_none(),
            "expected None when TLS env vars are unset"
        );

        // Restore previous values if they existed
        if let Some(v) = prev_cert {
            std::env::set_var("BOLT_TLS_CERT", v);
        }
        if let Some(v) = prev_key {
            std::env::set_var("BOLT_TLS_KEY", v);
        }
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

        // Save and set env vars
        let prev_cert = std::env::var("BOLT_TLS_CERT").ok();
        let prev_key = std::env::var("BOLT_TLS_KEY").ok();
        std::env::set_var("BOLT_TLS_CERT", cert_path.to_str().unwrap());
        std::env::set_var("BOLT_TLS_KEY", key_path.to_str().unwrap());

        let result = load_tls_config();
        assert!(
            result.is_some(),
            "expected Some(TlsAcceptor) with valid cert/key"
        );

        // Cleanup
        if let Some(v) = prev_cert {
            std::env::set_var("BOLT_TLS_CERT", v);
        } else {
            std::env::remove_var("BOLT_TLS_CERT");
        }
        if let Some(v) = prev_key {
            std::env::set_var("BOLT_TLS_KEY", v);
        } else {
            std::env::remove_var("BOLT_TLS_KEY");
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
