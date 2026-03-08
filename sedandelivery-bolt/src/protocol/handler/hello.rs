//! HELLO and LOGON message handlers.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use crate::message::types::ServerMessage;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

use super::{bolt_map, constant_time_eq, ProtocolError, Session};

// -- Auth rate limiting ------------------------------------------------------

struct RateLimitEntry {
    count: u64,
    window_start: Instant,
}

/// Per-IP authentication attempt tracker.
static AUTH_RATE_LIMITER: OnceLock<Mutex<HashMap<IpAddr, RateLimitEntry>>> = OnceLock::new();

fn auth_rate_limiter() -> &'static Mutex<HashMap<IpAddr, RateLimitEntry>> {
    AUTH_RATE_LIMITER.get_or_init(|| Mutex::new(HashMap::new()))
}

fn auth_max_attempts() -> u64 {
    static VAL: OnceLock<u64> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("BOLT_AUTH_MAX_ATTEMPTS")
            .ok()
            .and_then(|v| match v.parse::<u64>() {
                Ok(val) if val > 0 => Some(val),
                _ => {
                    tracing::warn!(
                        "invalid BOLT_AUTH_MAX_ATTEMPTS value, using default 10"
                    );
                    None
                }
            })
            .unwrap_or(10)
    })
}

fn auth_window_secs() -> u64 {
    static VAL: OnceLock<u64> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("BOLT_AUTH_WINDOW_SECS")
            .ok()
            .and_then(|v| match v.parse::<u64>() {
                Ok(val) if val > 0 => Some(val),
                _ => {
                    tracing::warn!(
                        "invalid BOLT_AUTH_WINDOW_SECS value, using default 60"
                    );
                    None
                }
            })
            .unwrap_or(60)
    })
}

/// Check whether the given IP has exceeded the auth attempt rate limit.
/// Returns `true` if the request should be rejected (rate limited).
/// Increments the counter for the IP on every call.
pub(crate) fn check_auth_rate_limit(ip: IpAddr) -> bool {
    let max = auth_max_attempts();
    let window = std::time::Duration::from_secs(auth_window_secs());
    let mut map = auth_rate_limiter().lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    let entry = map.entry(ip).or_insert(RateLimitEntry {
        count: 0,
        window_start: now,
    });
    if now.duration_since(entry.window_start) >= window {
        // Reset the window.
        entry.count = 1;
        entry.window_start = now;
        false
    } else {
        entry.count += 1;
        entry.count > max
    }
}

/// Read `BOLT_PASSWORD` from the environment once and cache the result for the
/// lifetime of the process. Avoids repeated `std::env::var` calls on every
/// HELLO / LOGON message.
pub(crate) fn required_password() -> Option<&'static str> {
    static PW: OnceLock<Option<String>> = OnceLock::new();
    PW.get_or_init(|| std::env::var("BOLT_PASSWORD").ok())
        .as_deref()
}

// -- HELLO -------------------------------------------------------------------

/// Handle the HELLO message (initial client authentication).
///
/// Validates credentials when `BOLT_PASSWORD` is set, negotiates the UTC
/// patch, and transitions the session to `Ready`.
/// Server version string, configurable via `BOLT_SERVER_VERSION` env var.
fn server_version() -> &'static str {
    static VER: OnceLock<String> = OnceLock::new();
    VER.get_or_init(|| {
        std::env::var("BOLT_SERVER_VERSION").unwrap_or_else(|_| "Neo4j/4.4.0".to_string())
    })
    .as_str()
}

pub(crate) async fn handle_hello(
    session: &mut Session,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    // Rate limit auth attempts per IP.
    if let Some(ip) = session.peer_ip {
        if check_auth_rate_limit(ip) {
            tracing::warn!(%ip, conn_id = session.conn_id, "auth rate limit exceeded");
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.AuthenticationRateLimit".to_string()),
                "message" => BoltValue::String("Too many authentication attempts".to_string())
            })]);
        }
    }

    if let Some(required_pw) = required_password() {
        let scheme = extra.get_str("scheme").unwrap_or("");
        if scheme != "basic" {
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
                "message" => BoltValue::String("Authentication scheme 'basic' required".to_string())
            })]);
        }
        let credentials = extra.get_str("credentials").unwrap_or("");
        if !constant_time_eq(credentials.as_bytes(), required_pw.as_bytes()) {
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
                "message" => BoltValue::String("Invalid credentials".to_string())
            })]);
        }
    }

    // Check if the client requests the UTC patch (Bolt 4.3+).
    // When echoed back, the driver uses UTC-based DateTime encoding (tags 0x49/0x46)
    // instead of wall-clock encoding (tags 0x69/0x66).
    let utc_patch = match extra.get_value("patch_bolt") {
        Some(BoltValue::List(items)) => items
            .iter()
            .any(|v| matches!(v, BoltValue::String(s) if s == "utc")),
        _ => false,
    };

    session.state = ServerState::Ready;
    let mut success = bolt_map! {
        "server" => BoltValue::String(server_version().to_string()),
        "connection_id" => BoltValue::String(format!("sedandelivery-bolt-{}", session.conn_id))
    };
    if utc_patch {
        success.push((
            "patch_bolt".to_string(),
            BoltValue::List(vec![BoltValue::String("utc".to_string())]),
        ));
    }
    Ok(vec![ServerMessage::Success(success)])
}

// -- LOGON -------------------------------------------------------------------

/// Handle the LOGON message (deferred authentication for Bolt 5.1+).
///
/// Validates credentials when `BOLT_PASSWORD` is set and transitions the
/// session to `Ready`.
pub(crate) async fn handle_logon(
    session: &mut Session,
    auth: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    // Rate limit auth attempts per IP.
    if let Some(ip) = session.peer_ip {
        if check_auth_rate_limit(ip) {
            tracing::warn!(%ip, conn_id = session.conn_id, "auth rate limit exceeded");
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.AuthenticationRateLimit".to_string()),
                "message" => BoltValue::String("Too many authentication attempts".to_string())
            })]);
        }
    }

    if let Some(required_pw) = required_password() {
        let scheme = auth.get_str("scheme").unwrap_or("");
        if scheme != "basic" {
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
                "message" => BoltValue::String("Authentication scheme 'basic' required".to_string())
            })]);
        }
        let credentials = auth.get_str("credentials").unwrap_or("");
        if !constant_time_eq(credentials.as_bytes(), required_pw.as_bytes()) {
            session.state = ServerState::Defunct;
            return Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
                "message" => BoltValue::String("Invalid credentials".to_string())
            })]);
        }
    }

    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}
