//! Entra ID JWT claims.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct EntraClaims {
    pub aud: String,
    pub iss: String,
    pub sub: String,
    #[serde(default)]
    pub oid: String,
    #[serde(default)]
    pub preferred_username: String,
    pub exp: usize,
    pub iat: usize,
}
