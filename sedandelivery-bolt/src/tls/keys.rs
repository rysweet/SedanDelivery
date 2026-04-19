//! Private key loading for the bolt TLS server.

use std::fs::File;
use std::io::BufReader;

use rustls::pki_types::PrivateKeyDer;

/// Read the first private key found in PEM data. Supports PKCS8, RSA,
/// and EC key formats.
pub(super) fn read_private_key(reader: &mut BufReader<File>) -> Option<PrivateKeyDer<'static>> {
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
