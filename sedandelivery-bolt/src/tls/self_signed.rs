//! Fail-secure self-signed certificate detection.
//!
//! The [`is_self_signed_pem`] function inspects a PEM file on disk and
//! returns `true` for any of five fail-secure conditions, in addition to
//! the standard "Subject == Issuer" definition of a self-signed cert.

use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::*;

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
