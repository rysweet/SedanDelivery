//! TLS configuration for the sedandelivery-bolt server.
//!
//! Reads `BOLT_TLS_CERT` and `BOLT_TLS_KEY` environment variables to
//! load a PEM certificate chain and private key. When both are set,
//! returns a [`TlsAcceptor`] that can wrap incoming TCP connections.
//! When both are absent, returns `None` so the server falls back to
//! plaintext. Partial or empty TLS configuration is treated as a
//! startup error.
//!
//! ## Self-signed certificates
//!
//! Set `BOLT_ALLOW_SELF_SIGNED=1` to suppress the self-signed certificate
//! warning. This is intended for local development only — never set in
//! production. When absent (or set to any value other than `"1"`), a warning
//! is logged if the certificate appears to be self-signed.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::PrivateKeyDer;
use tokio_rustls::TlsAcceptor;

/// Check whether the PEM file at `path` contains a self-signed certificate.
///
/// A certificate is self-signed when its Subject and Issuer are identical.
/// The function is **fail-secure**: all ambiguous cases return `true` so
/// that callers treat unknown certificates conservatively.
///
/// The five fail-secure chain conditions (all return `true`):
/// 1. `path` is a relative path — cannot be safely resolved
/// 2. `path` does not exist — no cert to inspect
/// 3. File contains no PEM CERTIFICATE block — unreadable content
/// 4. DER parse fails — malformed certificate
/// 5. Subject == Issuer — standard self-signed definition
///
/// Returns `false` only when the certificate is demonstrably CA-signed
/// (absolute path, file exists, parses cleanly, Subject ≠ Issuer).
pub fn is_self_signed_pem(path: &Path) -> bool {
    // Condition 1: relative paths cannot be resolved safely.
    if path.is_relative() {
        tracing::debug!(
            path = %path.display(),
            "is_self_signed_pem: relative path — treated as self-signed (fail-secure)"
        );
        return true;
    }

    // Condition 2: file does not exist.
    if !path.exists() {
        tracing::debug!(
            path = %path.display(),
            "is_self_signed_pem: file not found — treated as self-signed (fail-secure)"
        );
        return true;
    }

    // Read file content.
    let pem_bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            tracing::debug!(
                path = %path.display(),
                error = %e,
                "is_self_signed_pem: read error — treated as self-signed (fail-secure)"
            );
            return true;
        }
    };

    let pem_str = match std::str::from_utf8(&pem_bytes) {
        Ok(s) => s,
        Err(_) => return true,
    };

    // Condition 3: no PEM CERTIFICATE block.
    if !pem_str.contains("-----BEGIN CERTIFICATE-----") {
        tracing::debug!(
            path = %path.display(),
            "is_self_signed_pem: no PEM CERTIFICATE block — treated as self-signed (fail-secure)"
        );
        return true;
    }

    // Extract base64-encoded DER bytes between the first PEM header/footer.
    let b64: String = pem_str
        .lines()
        .skip_while(|l| !l.starts_with("-----BEGIN CERTIFICATE-----"))
        .skip(1)
        .take_while(|l| !l.starts_with("-----END CERTIFICATE-----"))
        .map(|l| l.trim())
        .collect();

    let der_bytes = match base64_decode_std(&b64) {
        Some(b) => b,
        None => {
            tracing::debug!(
                path = %path.display(),
                "is_self_signed_pem: base64 decode failed — treated as self-signed (fail-secure)"
            );
            return true;
        }
    };

    // Condition 4: DER parse fails.
    use x509_parser::prelude::*;
    let (_, cert) = match X509Certificate::from_der(&der_bytes) {
        Ok(result) => result,
        Err(e) => {
            tracing::debug!(
                path = %path.display(),
                error = ?e,
                "is_self_signed_pem: x509 parse error — treated as self-signed (fail-secure)"
            );
            return true;
        }
    };

    // Condition 5: Subject == Issuer → self-signed.
    cert.subject() == cert.issuer()
}

/// Minimal base64 standard-alphabet decoder (RFC 4648 §4) using only `std`.
///
/// Input may omit padding; the function adds it as needed.
/// Returns `None` on any decode error (invalid character, wrong length, etc.).
fn base64_decode_std(s: &str) -> Option<Vec<u8>> {
    let alphabet = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut table = [0xffu8; 256];
    for (i, &c) in alphabet.iter().enumerate() {
        table[c as usize] = i as u8;
    }

    // Strip whitespace, then pad to multiple of 4.
    let cleaned: String = s.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    let pad = (4 - cleaned.len() % 4) % 4;
    let padded = format!("{cleaned}{}", "=".repeat(pad));

    let bytes = padded.as_bytes();
    if bytes.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for chunk in bytes.chunks(4) {
        let a = table[chunk[0] as usize];
        let b = table[chunk[1] as usize];
        let c = if chunk[2] == b'=' {
            0
        } else {
            table[chunk[2] as usize]
        };
        let d = if chunk[3] == b'=' {
            0
        } else {
            table[chunk[3] as usize]
        };
        if a == 0xff || b == 0xff || c == 0xff || d == 0xff {
            return None;
        }
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            out.push((c << 6) | d);
        }
    }
    Some(out)
}

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

    // --- is_self_signed_pem tests ---

    #[test]
    fn relative_path_is_self_signed_fail_secure() {
        // Condition 1: relative path → true (fail-secure)
        assert!(
            is_self_signed_pem(std::path::Path::new("relative/path/cert.pem")),
            "relative path must return true (fail-secure)"
        );
    }

    #[test]
    fn nonexistent_file_is_self_signed_fail_secure() {
        // Condition 2: file does not exist → true (fail-secure)
        assert!(
            is_self_signed_pem(std::path::Path::new(
                "/nonexistent-sedandelivery-test-cert.pem"
            )),
            "missing file must return true (fail-secure)"
        );
    }

    #[test]
    fn empty_file_is_self_signed_fail_secure() {
        // Condition 3: no PEM block → true (fail-secure)
        let tmp = std::env::temp_dir().join("sd-tls-test-empty-cert.pem");
        std::fs::write(&tmp, b"").unwrap();
        assert!(
            is_self_signed_pem(&tmp),
            "empty file (no PEM block) must return true (fail-secure)"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn garbage_pem_is_self_signed_fail_secure() {
        // Condition 4: header present but DER parse fails → true (fail-secure)
        let tmp = std::env::temp_dir().join("sd-tls-test-garbage-cert.pem");
        std::fs::write(
            &tmp,
            b"-----BEGIN CERTIFICATE-----\nZ29yYmFnZQ==\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        assert!(
            is_self_signed_pem(&tmp),
            "malformed DER must return true (fail-secure)"
        );
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn self_signed_cert_returns_true() {
        // Condition 5: Subject == Issuer → true
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("failed to generate self-signed cert");
        let tmp = std::env::temp_dir().join("sd-tls-test-self-signed.pem");
        std::fs::write(&tmp, cert.cert.pem().as_bytes()).unwrap();
        assert!(
            is_self_signed_pem(&tmp),
            "self-signed cert must return true"
        );
        let _ = std::fs::remove_file(&tmp);
    }
}
