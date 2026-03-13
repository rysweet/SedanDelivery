//! HELLO and LOGON message handlers.

use std::borrow::Cow;
use std::net::IpAddr;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use dashmap::DashMap;

use crate::message::types::ServerMessage;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

use super::{bolt_map, constant_time_eq, ProtocolError, Session};

// -- Auth rate limiting ------------------------------------------------------

/// Maximum number of per-IP entries tracked at once.  Entries that push the
/// map beyond this limit are dropped (oldest insertion order) to prevent
/// unbounded memory growth from adversarial IP churn.
const MAX_TRACKED_IPS: usize = 100_000;

/// How long an entry must be inactive before the periodic sweep removes it.
const STALE_AFTER_SECS: u64 = 3600;

/// How often the background sweep runs (seconds).
const SWEEP_INTERVAL_SECS: u64 = 300;

struct RateLimitEntry {
    count: u64,
    window_start: Instant,
}

impl AuthRateLimiter {
    fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    /// Record a failed auth attempt for `key` (IP string).
    /// Returns `true` if the IP is now rate-limited (exceeded MAX_FAILURES_PER_IP).
    fn record_failure(&self, key: &str) -> bool {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

/// Spawn a background task that periodically removes stale per-IP entries and
/// enforces the [`MAX_TRACKED_IPS`] cap.  Call once from `main` after the
/// Tokio runtime is started.
pub fn spawn_auth_rate_limit_sweep() -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let interval = tokio::time::Duration::from_secs(SWEEP_INTERVAL_SECS);
        let stale_threshold = std::time::Duration::from_secs(STALE_AFTER_SECS);
        loop {
            tokio::time::sleep(interval).await;
            let mut map = auth_rate_limiter()
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let before = map.len();
            let now = std::time::Instant::now();
            // Remove entries that have been idle longer than STALE_AFTER_SECS.
            map.retain(|_, entry| now.duration_since(entry.window_start) < stale_threshold);
            // If still over cap, drop arbitrary excess entries.
            if map.len() > MAX_TRACKED_IPS {
                let excess = map.len() - MAX_TRACKED_IPS;
                let to_remove: Vec<IpAddr> = map.keys().take(excess).copied().collect();
                for ip in to_remove {
                    map.remove(&ip);
                }
            }
            let after = map.len();
            if before != after {
                tracing::debug!(
                    removed = before - after,
                    remaining = after,
                    "auth rate-limit sweep complete"
                );
            }
        }
    })
}

fn auth_max_attempts() -> u64 {
    static VAL: OnceLock<u64> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("BOLT_AUTH_MAX_ATTEMPTS")
            .ok()
            .and_then(|v| match v.parse::<u64>() {
                Ok(val) if val > 0 => Some(val),
                _ => {
                    tracing::warn!("invalid BOLT_AUTH_MAX_ATTEMPTS value, using default 10");
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
                    tracing::warn!("invalid BOLT_AUTH_WINDOW_SECS value, using default 60");
                    None
                }
            })
            .unwrap_or(60)
    })
}

/// Check whether the given IP has exceeded the failed-auth rate limit.
/// Returns `true` if the request should be rejected (rate limited).
/// Increments the counter for the IP on every failed-auth attempt.
///
/// Inline stale-entry pruning keeps memory bounded between periodic sweeps.
/// New IPs are silently dropped (rate-limited) when the map is at capacity.
pub(crate) fn check_auth_rate_limit(ip: IpAddr) -> bool {
    let max = auth_max_attempts();
    let window = Duration::from_secs(auth_window_secs());
    let mut map = auth_rate_limiter()
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    // Inline prune of expired windows (defence-in-depth; the background sweep
    // handles bulk cleanup every SWEEP_INTERVAL_SECS seconds).
    map.retain(|_, entry| now.duration_since(entry.window_start) < window);
    // Enforce hard cap: if already at MAX_TRACKED_IPS and this is a new IP,
    // reject the connection to prevent memory exhaustion.
    if map.len() >= MAX_TRACKED_IPS && !map.contains_key(&ip) {
        tracing::warn!(
            cap = MAX_TRACKED_IPS,
            "auth rate-limit table at capacity; rejecting new IP"
        );
        return true;
    }
    let entry = map.entry(ip).or_insert(RateLimitEntry {
        count: 0,
        window_start: now,
    });
    if now.duration_since(entry.window_start) >= window {
        // Reset the window.
        entry.count = 1;
        entry.window_start = now;
        false
    }

    /// Clear the failure counter for `key` (after a successful auth).
    fn record_success(&self, key: &str) {
        self.map.remove(key);
    }

    /// Remove entries that have been stale for longer than STALE_AFTER_SECS.
    fn sweep(&self) {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.map
            .retain(|_, v| now_secs.saturating_sub(v.1.load(Ordering::Relaxed)) < STALE_AFTER_SECS);
    }
}

fn auth_rate_limiter() -> &'static AuthRateLimiter {
    static LIMITER: OnceLock<AuthRateLimiter> = OnceLock::new();
    LIMITER.get_or_init(AuthRateLimiter::new)
}

/// Spawn a background task that periodically sweeps stale rate-limiter entries.
pub fn spawn_auth_rate_limiter_sweep() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            auth_rate_limiter().sweep();
        }
    });
}

// Keep the old public API used by tests so they still compile.
#[cfg(test)]
pub(crate) fn clear_auth_rate_limit(ip: IpAddr) {
    auth_rate_limiter().record_success(&ip.to_string());
}

#[cfg(test)]
tokio::task_local! {
    static REQUIRED_PASSWORD_OVERRIDE: Option<Box<str>>;
}

/// Read `BOLT_PASSWORD` from the environment for the current auth attempt.
pub(crate) fn required_password() -> Option<Cow<'static, str>> {
    #[cfg(test)]
    if let Ok(required_pw) = REQUIRED_PASSWORD_OVERRIDE.try_with(Clone::clone) {
        return required_pw.map(|pw| Cow::Owned(pw.into()));
    }

    static PW: OnceLock<Option<Box<str>>> = OnceLock::new();
    PW.get_or_init(|| {
        std::env::var("BOLT_PASSWORD")
            .ok()
            .map(String::into_boxed_str)
    })
    .as_deref()
    .map(Cow::Borrowed)
}

#[cfg(test)]
pub(crate) async fn with_required_password_override_for_tests<F, T>(
    required_password: Option<&str>,
    future: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    REQUIRED_PASSWORD_OVERRIDE
        .scope(required_password.map(Box::<str>::from), future)
        .await
}

fn auth_rate_limit_failure(session: &mut Session, ip: IpAddr) -> Vec<ServerMessage> {
    tracing::warn!(%ip, conn_id = session.conn_id, "auth rate limit exceeded");
    session.state = ServerState::Defunct;
    vec![ServerMessage::Failure(bolt_map! {
        "code" => BoltValue::String("Neo.ClientError.Security.AuthenticationRateLimit".to_string()),
        "message" => BoltValue::String("Too many authentication attempts".to_string())
    })]
}

fn unauthorized_failure(session: &mut Session, message: &str) -> Vec<ServerMessage> {
    session.state = ServerState::Defunct;
    vec![ServerMessage::Failure(bolt_map! {
        "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
        "message" => BoltValue::String(message.to_string())
    })]
}

fn register_failed_auth_attempt(session: &mut Session) -> Option<Vec<ServerMessage>> {
    let ip = session.peer_ip?;
    let key = ip.to_string();
    if auth_rate_limiter().record_failure(&key) {
        Some(auth_rate_limit_failure(session, ip))
    } else {
        None
    }
}

fn validate_basic_auth(
    session: &mut Session,
    auth: &BoltMap,
    required_pw: Option<&str>,
) -> Option<Vec<ServerMessage>> {
    let required_pw = required_pw?;
    let scheme = auth.get_str("scheme").unwrap_or("");
    if scheme != "basic" {
        if let Some(rate_limited) = register_failed_auth_attempt(session) {
            return Some(rate_limited);
        }
        return Some(unauthorized_failure(
            session,
            "Authentication scheme 'basic' required",
        ));
    }

    let credentials = auth.get_str("credentials").unwrap_or("");
    if !constant_time_eq(credentials.as_bytes(), required_pw.as_bytes()) {
        if let Some(rate_limited) = register_failed_auth_attempt(session) {
            return Some(rate_limited);
        }
        return Some(unauthorized_failure(session, "Invalid credentials"));
    }

    if let Some(ip) = session.peer_ip {
        auth_rate_limiter().record_success(&ip.to_string());
    }

    None
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
    if let Some(failure) = validate_basic_auth(session, &extra, required_password().as_deref()) {
        return Ok(failure);
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
    if let Some(failure) = validate_basic_auth(session, &auth, required_password().as_deref()) {
        return Ok(failure);
    }

    session.state = ServerState::Ready;
    Ok(vec![ServerMessage::Success(bolt_map! {})])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use gql_core::storage::InMemoryGraphStore;

    fn auth_session(ip: IpAddr) -> Session {
        Session {
            state: ServerState::Authentication,
            store: Arc::new(InMemoryGraphStore::new()),
            conn_id: 1,
            current_graph: "neo4j".to_string(),
            pending_results: HashMap::new(),
            next_qid: 0,
            global_bookmark: Arc::new(crate::bookmark::BookmarkCounter::from_env()),
            tx_metadata: Vec::new(),
            tx_timeout: None,
            tx_store: None,
            write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
            write_permit: None,
            saved_graph: None,
            peer_ip: Some(ip),
        }
    }

    fn basic_auth(credentials: &str) -> BoltMap {
        bolt_map! {
            "scheme" => BoltValue::String("basic".to_string()),
            "credentials" => BoltValue::String(credentials.to_string())
        }
    }

    fn response_code(messages: &[ServerMessage]) -> Option<&str> {
        match messages.first() {
            Some(ServerMessage::Success(_)) => Some("SUCCESS"),
            Some(ServerMessage::Failure(meta)) => meta.get_str("code"),
            _ => None,
        }
    }

    #[tokio::test]
    async fn hello_without_password_does_not_rate_limit_repeated_attempts() {
        let ip = "10.0.0.11".parse().unwrap();
        clear_auth_rate_limit(ip);

        with_required_password_override_for_tests(None, async move {
            for _ in 0..16 {
                let mut session = auth_session(ip);
                let responses = handle_hello(&mut session, basic_auth("wrong"))
                    .await
                    .unwrap();
                assert_eq!(response_code(&responses), Some("SUCCESS"));
            }
        })
        .await;
    }

    #[tokio::test]
    async fn successful_hello_auth_clears_failed_attempt_budget() {
        let ip = "10.0.0.12".parse().unwrap();
        clear_auth_rate_limit(ip);

        with_required_password_override_for_tests(Some("secret"), async move {
            for _ in 0..10 {
                let mut session = auth_session(ip);
                let responses = handle_hello(&mut session, basic_auth("wrong"))
                    .await
                    .unwrap();
                assert_eq!(
                    response_code(&responses),
                    Some("Neo.ClientError.Security.Unauthorized")
                );
            }

            let mut success_session = auth_session(ip);
            let success = handle_hello(&mut success_session, basic_auth("secret"))
                .await
                .unwrap();
            assert_eq!(response_code(&success), Some("SUCCESS"));

            let mut retry_session = auth_session(ip);
            let retry = handle_hello(&mut retry_session, basic_auth("wrong"))
                .await
                .unwrap();
            assert_eq!(
                response_code(&retry),
                Some("Neo.ClientError.Security.Unauthorized")
            );
        })
        .await;
    }

    #[tokio::test]
    async fn failed_logon_attempts_are_rate_limited() {
        let ip = "10.0.0.13".parse().unwrap();
        clear_auth_rate_limit(ip);

        with_required_password_override_for_tests(Some("secret"), async move {
            for _ in 0..10 {
                let mut session = auth_session(ip);
                let responses = handle_logon(&mut session, basic_auth("wrong"))
                    .await
                    .unwrap();
                assert_eq!(
                    response_code(&responses),
                    Some("Neo.ClientError.Security.Unauthorized")
                );
            }

            let mut session = auth_session(ip);
            let responses = handle_logon(&mut session, basic_auth("wrong"))
                .await
                .unwrap();
            assert_eq!(
                response_code(&responses),
                Some("Neo.ClientError.Security.AuthenticationRateLimit")
            );
        })
        .await;
    }

    #[test]
    fn stale_rate_limit_entries_are_evicted() {
        // Insert a stale entry directly into the DashMap to verify sweep removes it.
        let stale_ip: IpAddr = "10.0.0.21".parse().unwrap();
        let stale_key = stale_ip.to_string();
        let limiter = auth_rate_limiter();
        // Simulate a stale entry by inserting with a very old timestamp.
        limiter.map.insert(
            stale_key.clone(),
            (
                AtomicU32::new(99),
                AtomicU64::new(0), // epoch = maximally stale
            ),
        );
        limiter.sweep();
        assert!(
            !limiter.map.contains_key(&stale_key),
            "expired auth-failure entries should be pruned"
        );
    }
}
