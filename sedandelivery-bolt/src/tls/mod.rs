//! TLS configuration for the sedandelivery-bolt server.
//!
//! Reads `BOLT_TLS_CERT` and `BOLT_TLS_KEY` environment variables to
//! load a PEM certificate chain and private key. When both are set,
//! returns a [`tokio_rustls::TlsAcceptor`] that can wrap incoming TCP
//! connections. When both are absent, returns `None` so the server
//! falls back to plaintext. Partial or empty TLS configuration is
//! treated as a startup error.
//!
//! ## Self-signed certificates
//!
//! Set `BOLT_ALLOW_SELF_SIGNED=1` to suppress the self-signed certificate
//! warning. This is intended for local development only — never set in
//! production. When absent (or set to any value other than `"1"`), a warning
//! is logged if the certificate appears to be self-signed.
//!
//! ## Submodules
//!
//! * [`self_signed`] — fail-secure inspection of PEM certificates.
//! * [`env`] — strict parsing of TLS-related environment variables.
//! * [`keys`] — private key loading (PKCS8 / RSA / EC).
//! * [`config`] — top-level [`load_tls_config`] entry point.

mod config;
mod env;
mod keys;
mod self_signed;

pub use config::load_tls_config;
pub use self_signed::is_self_signed_pem;
