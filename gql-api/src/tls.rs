//! TLS configuration for the gql-api HTTP server.
//!
//! Reads `GQL_API_TLS_CERT` and `GQL_API_TLS_KEY` environment variables to
//! load a PEM certificate chain and private key. When both are set, returns
//! a [`TlsAcceptor`] that can wrap incoming TCP connections. When neither is
//! set, returns `None` so the server falls back to plaintext HTTP.
//!
//! # Panics
//!
//! Panics if only one of the two variables is set (ambiguous configuration)
//! or if the referenced PEM files cannot be read or parsed.

use std::fs::File;
use std::io::BufReader;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::connect_info::Connected;
use rustls::pki_types::PrivateKeyDer;
use tokio::io;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

/// Load TLS configuration from environment variables.
///
/// Returns `Some(TlsAcceptor)` when both `GQL_API_TLS_CERT` and `GQL_API_TLS_KEY`
/// point to valid PEM files. Returns `None` if neither variable is set, allowing
/// the server to operate in plaintext mode.
///
/// # Panics
///
/// - Panics if only one of the two environment variables is set.
/// - Panics if the files cannot be read or contain no valid certificate/key.
pub fn load_tls_config() -> Option<TlsAcceptor> {
    let cert_path = std::env::var("GQL_API_TLS_CERT").ok();
    let key_path = std::env::var("GQL_API_TLS_KEY").ok();

    match (&cert_path, &key_path) {
        (None, None) => return None,
        (Some(_), None) => {
            panic!(
                "GQL_API_TLS_CERT is set but GQL_API_TLS_KEY is not — \
                 set both to enable TLS, or neither to use plaintext HTTP"
            );
        }
        (None, Some(_)) => {
            panic!(
                "GQL_API_TLS_KEY is set but GQL_API_TLS_CERT is not — \
                 set both to enable TLS, or neither to use plaintext HTTP"
            );
        }
        (Some(_), Some(_)) => {}
    }

    let cert_path = cert_path.unwrap();
    let key_path = key_path.unwrap();

    tracing::info!("TLS enabled: cert={cert_path}, key={key_path}");

    // Load certificate chain.
    let cert_file = File::open(&cert_path)
        .unwrap_or_else(|e| panic!("failed to open TLS cert file {cert_path}: {e}"));
    let mut cert_reader = BufReader::new(cert_file);
    let certs: Vec<_> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .unwrap_or_else(|e| panic!("failed to parse TLS cert file {cert_path}: {e}"));
    if certs.is_empty() {
        panic!("TLS cert file {cert_path} contains no certificates");
    }

    // Load private key (supports PKCS8, RSA / PKCS1, and EC / SEC1 formats).
    let key_file = File::open(&key_path)
        .unwrap_or_else(|e| panic!("failed to open TLS key file {key_path}: {e}"));
    let mut key_reader = BufReader::new(key_file);
    let key = read_private_key(&mut key_reader)
        .unwrap_or_else(|| panic!("TLS key file {key_path} contains no private key"));

    let provider = rustls::crypto::ring::default_provider();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
        .with_safe_default_protocol_versions()
        .expect("failed to set TLS protocol versions")
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("failed to build TLS ServerConfig");

    Some(TlsAcceptor::from(Arc::new(config)))
}

/// Return the TLS listen port from `GQL_API_TLS_PORT`, defaulting to 3443.
pub fn tls_port() -> u16 {
    std::env::var("GQL_API_TLS_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(3443)
}

/// Read the first private key found in PEM data. Supports PKCS8, RSA (PKCS1),
/// and EC (SEC1) key formats.
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

// ---------------------------------------------------------------------------
// TlsListener — implements axum's `Listener` trait so we can pass it directly
// to `axum::serve`.
// ---------------------------------------------------------------------------

/// A TCP listener that performs a TLS handshake on every accepted connection.
pub struct TlsListener {
    inner: TcpListener,
    acceptor: TlsAcceptor,
}

impl TlsListener {
    /// Create a new `TlsListener` wrapping the given TCP listener and TLS acceptor.
    pub fn new(listener: TcpListener, acceptor: TlsAcceptor) -> Self {
        Self {
            inner: listener,
            acceptor,
        }
    }
}

/// Newtype around [`SocketAddr`] so we can implement [`Connected`] without
/// hitting the orphan rule.
#[derive(Clone, Debug)]
pub struct TlsConnectInfo(pub SocketAddr);

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = TlsConnectInfo;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            match self.inner.accept().await {
                Ok((stream, addr)) => match self.acceptor.accept(stream).await {
                    Ok(tls_stream) => return (tls_stream, TlsConnectInfo(addr)),
                    Err(e) => {
                        tracing::debug!("TLS handshake failed: {e}");
                        continue;
                    }
                },
                Err(e) => {
                    if is_connection_error(&e) {
                        continue;
                    }
                    tracing::error!("accept error: {e}");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr().map(TlsConnectInfo)
    }
}

impl Connected<axum::serve::IncomingStream<'_, TlsListener>> for TlsConnectInfo {
    fn connect_info(stream: axum::serve::IncomingStream<'_, TlsListener>) -> Self {
        stream.remote_addr().clone()
    }
}

fn is_connection_error(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_none_without_env_vars() {
        // Save and clear env vars.
        let prev_cert = std::env::var("GQL_API_TLS_CERT").ok();
        let prev_key = std::env::var("GQL_API_TLS_KEY").ok();
        std::env::remove_var("GQL_API_TLS_CERT");
        std::env::remove_var("GQL_API_TLS_KEY");

        let result = load_tls_config();
        assert!(result.is_none(), "expected None when TLS env vars are unset");

        // Restore.
        if let Some(v) = prev_cert {
            std::env::set_var("GQL_API_TLS_CERT", v);
        }
        if let Some(v) = prev_key {
            std::env::set_var("GQL_API_TLS_KEY", v);
        }
    }

    #[test]
    #[should_panic(expected = "GQL_API_TLS_CERT is set but GQL_API_TLS_KEY is not")]
    fn panics_when_only_cert_is_set() {
        std::env::set_var("GQL_API_TLS_CERT", "/tmp/dummy-cert.pem");
        std::env::remove_var("GQL_API_TLS_KEY");
        load_tls_config();
    }

    #[test]
    #[should_panic(expected = "GQL_API_TLS_KEY is set but GQL_API_TLS_CERT is not")]
    fn panics_when_only_key_is_set() {
        std::env::remove_var("GQL_API_TLS_CERT");
        std::env::set_var("GQL_API_TLS_KEY", "/tmp/dummy-key.pem");
        load_tls_config();
    }

    #[test]
    fn returns_some_with_valid_cert_and_key() {
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
            .expect("failed to generate self-signed cert");

        let cert_pem = cert.cert.pem();
        let key_pem = cert.signing_key.serialize_pem();

        let tmp_dir = std::env::temp_dir().join("gql-api-tls-test");
        std::fs::create_dir_all(&tmp_dir).unwrap();
        let cert_path = tmp_dir.join("test-cert.pem");
        let key_path = tmp_dir.join("test-key.pem");
        std::fs::write(&cert_path, cert_pem).unwrap();
        std::fs::write(&key_path, key_pem).unwrap();

        let prev_cert = std::env::var("GQL_API_TLS_CERT").ok();
        let prev_key = std::env::var("GQL_API_TLS_KEY").ok();
        std::env::set_var("GQL_API_TLS_CERT", cert_path.to_str().unwrap());
        std::env::set_var("GQL_API_TLS_KEY", key_path.to_str().unwrap());

        let result = load_tls_config();
        assert!(
            result.is_some(),
            "expected Some(TlsAcceptor) with valid cert/key"
        );

        // Cleanup.
        if let Some(v) = prev_cert {
            std::env::set_var("GQL_API_TLS_CERT", v);
        } else {
            std::env::remove_var("GQL_API_TLS_CERT");
        }
        if let Some(v) = prev_key {
            std::env::set_var("GQL_API_TLS_KEY", v);
        } else {
            std::env::remove_var("GQL_API_TLS_KEY");
        }
        let _ = std::fs::remove_dir_all(&tmp_dir);
    }
}
