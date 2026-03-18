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
//
// When `GQL_REDIS_URL` is set, auth failure counters are shared across all
// Bolt replicas via Redis INCR + EXPIRE. When no Redis URL is configured,
// an in-process DashMap provides per-replica rate limiting as a fallback.
//
// On Redis unavailability the limiter **fails open** (allows the attempt)
// and logs a warning, so a Redis outage never locks out legitimate users.

/// Maximum number of distinct IPs tracked simultaneously (in-memory mode).
const MAX_TRACKED_IPS: usize = 100_000;

/// Redis key prefix for distributed auth rate limiting counters.
const REDIS_KEY_PREFIX: &str = "__sd2:auth_rate:";

/// Lua script: atomically INCR the counter and set EXPIRE on first increment.
const INCR_WITH_EXPIRE_LUA: &str = "\
local count = redis.call('INCR', KEYS[1])\n\
if count == 1 then\n\
  redis.call('EXPIRE', KEYS[1], ARGV[1])\n\
end\n\
return count";

/// Maximum failed auth attempts per IP before returning RateLimit.
/// Configurable via `BOLT_AUTH_MAX_ATTEMPTS` env var (default: 10).
fn max_failures_per_ip() -> u32 {
    static VAL: OnceLock<u32> = OnceLock::new();
    *VAL.get_or_init(|| gql_core::config::parse_env_range("BOLT_AUTH_MAX_ATTEMPTS", 10, 1, 10000))
}

/// Seconds after which an entry is considered stale and eligible for sweep.
/// Configurable via `BOLT_AUTH_WINDOW_SECS` env var (default: 3600).
fn stale_after_secs() -> u64 {
    static VAL: OnceLock<u64> = OnceLock::new();
    *VAL.get_or_init(|| {
        gql_core::config::parse_env_range("BOLT_AUTH_WINDOW_SECS", 3600u64, 1, 86400)
    })
}

/// Canonical string form of an IP address for use as a rate-limit key.
fn normalize_ip(ip: IpAddr) -> String {
    // Rust's `IpAddr::to_string()` already produces canonical form for both
    // IPv4 and IPv6 (RFC 5952 compressed notation).
    ip.to_string()
}

// -- In-memory backend -------------------------------------------------------

struct InMemoryLimiter {
    map: DashMap<String, (AtomicU32, AtomicU64)>,
}

impl InMemoryLimiter {
    fn new() -> Self {
        Self {
            map: DashMap::new(),
        }
    }

    fn record_failure(&self, key: &str) -> bool {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if let Some(entry) = self.map.get(key) {
            entry.1.store(now_secs, Ordering::Relaxed);
            let prev = entry.0.fetch_add(1, Ordering::Relaxed);
            return prev + 1 > max_failures_per_ip();
        }

        if self.map.len() < MAX_TRACKED_IPS {
            self.map
                .entry(key.to_string())
                .or_insert_with(|| (AtomicU32::new(0), AtomicU64::new(now_secs)))
                .0
                .fetch_add(1, Ordering::Relaxed);
        }
        false
    }

    fn record_success(&self, key: &str) {
        self.map.remove(key);
    }

    fn sweep(&self) {
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.map.retain(|_, v| {
            now_secs.saturating_sub(v.1.load(Ordering::Relaxed)) < stale_after_secs()
        });
    }
}

// -- Redis backend -----------------------------------------------------------

struct RedisLimiter {
    client: redis::Client,
    conn: tokio::sync::Mutex<Option<redis::aio::MultiplexedConnection>>,
}

impl RedisLimiter {
    fn new(client: redis::Client) -> Self {
        Self {
            client,
            conn: tokio::sync::Mutex::new(None),
        }
    }

    async fn get_conn(&self) -> Result<redis::aio::MultiplexedConnection, redis::RedisError> {
        let mut guard = self.conn.lock().await;
        if let Some(ref conn) = *guard {
            return Ok(conn.clone());
        }
        let conn: redis::aio::MultiplexedConnection =
            self.client.get_multiplexed_async_connection().await?;
        *guard = Some(conn.clone());
        Ok(conn)
    }

    async fn clear_cached_conn(&self) {
        *self.conn.lock().await = None;
    }

    /// Increment failure counter via Lua script. Returns `Some(true)` if
    /// rate-limited, `Some(false)` if not, or `None` on Redis failure (fail open).
    async fn record_failure(&self, key: &str) -> Option<bool> {
        let redis_key = format!("{REDIS_KEY_PREFIX}{key}");
        let ttl = stale_after_secs();

        let mut conn = match self.get_conn().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "Redis unavailable for auth rate limiting; failing open");
                self.clear_cached_conn().await;
                return None;
            }
        };

        let result: Result<i64, redis::RedisError> = redis::cmd("EVAL")
            .arg(INCR_WITH_EXPIRE_LUA)
            .arg(1_i32)
            .arg(&redis_key)
            .arg(ttl as i64)
            .query_async(&mut conn)
            .await;

        match result {
            Ok(count) => Some(count > i64::from(max_failures_per_ip())),
            Err(e) => {
                tracing::warn!(error = %e, "Redis auth rate limit EVAL failed; failing open");
                self.clear_cached_conn().await;
                None
            }
        }
    }

    async fn record_success(&self, key: &str) {
        let redis_key = format!("{REDIS_KEY_PREFIX}{key}");
        let mut conn = match self.get_conn().await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "Redis unavailable for auth rate limit reset");
                self.clear_cached_conn().await;
                return;
            }
        };

        let result: Result<i64, redis::RedisError> = redis::cmd("DEL")
            .arg(&redis_key)
            .query_async(&mut conn)
            .await;
        if let Err(e) = result {
            tracing::warn!(error = %e, "Redis auth rate limit DEL failed");
            self.clear_cached_conn().await;
        }
    }
}

// -- Combined rate limiter ---------------------------------------------------

enum AuthRateLimiter {
    InMemory(InMemoryLimiter),
    Redis(RedisLimiter),
}

impl AuthRateLimiter {
    async fn record_failure(&self, key: &str) -> bool {
        match self {
            Self::InMemory(lim) => lim.record_failure(key),
            Self::Redis(lim) => {
                // Fail open: if Redis is unavailable, allow the attempt.
                lim.record_failure(key).await.unwrap_or(false)
            }
        }
    }

    async fn record_success(&self, key: &str) {
        match self {
            Self::InMemory(lim) => lim.record_success(key),
            Self::Redis(lim) => lim.record_success(key).await,
        }
    }

    fn sweep(&self) {
        match self {
            Self::InMemory(lim) => lim.sweep(),
            // Redis handles TTL via EXPIRE; nothing to sweep.
            Self::Redis(_) => {}
        }
    }
}

fn auth_rate_limiter() -> &'static AuthRateLimiter {
    static LIMITER: OnceLock<AuthRateLimiter> = OnceLock::new();
    LIMITER.get_or_init(|| {
        if let Ok(url) = std::env::var("GQL_REDIS_URL") {
            match redis::Client::open(url.as_str()) {
                Ok(client) => {
                    tracing::info!("Auth rate limiter using Redis-backed counters");
                    return AuthRateLimiter::Redis(RedisLimiter::new(client));
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to create Redis client for auth rate limiter; \
                         falling back to in-memory"
                    );
                }
            }
        }
        tracing::info!("Auth rate limiter using in-memory counters");
        AuthRateLimiter::InMemory(InMemoryLimiter::new())
    })
}

/// Spawn a background task that periodically sweeps stale rate-limiter entries.
/// In Redis mode this is a no-op (EXPIRE handles TTL), but the task still runs
/// so the sweep interval is consistent.
pub fn spawn_auth_rate_limiter_sweep() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(300));
        loop {
            interval.tick().await;
            auth_rate_limiter().sweep();
        }
    });
}

#[cfg(test)]
pub(crate) async fn clear_auth_rate_limit(ip: IpAddr) {
    auth_rate_limiter().record_success(&normalize_ip(ip)).await;
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

async fn register_failed_auth_attempt(session: &mut Session) -> Option<Vec<ServerMessage>> {
    let ip = session.peer_ip?;
    let key = normalize_ip(ip);
    if auth_rate_limiter().record_failure(&key).await {
        Some(auth_rate_limit_failure(session, ip))
    } else {
        None
    }
}

async fn validate_basic_auth(
    session: &mut Session,
    auth: &BoltMap,
    required_pw: Option<&str>,
) -> Option<Vec<ServerMessage>> {
    let required_pw = required_pw?;
    let scheme = auth.get_str("scheme").unwrap_or("");
    if scheme != "basic" {
        if let Some(rate_limited) = register_failed_auth_attempt(session).await {
            return Some(rate_limited);
        }
        return Some(unauthorized_failure(
            session,
            "Authentication scheme 'basic' required",
        ));
    }

    let credentials = auth.get_str("credentials").unwrap_or("");
    if !constant_time_eq(credentials.as_bytes(), required_pw.as_bytes()) {
        if let Some(rate_limited) = register_failed_auth_attempt(session).await {
            return Some(rate_limited);
        }
        return Some(unauthorized_failure(session, "Invalid credentials"));
    }

    if let Some(ip) = session.peer_ip {
        auth_rate_limiter().record_success(&normalize_ip(ip)).await;
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
    if let Some(failure) =
        validate_basic_auth(session, &extra, required_password().as_deref()).await
    {
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
    if let Some(failure) = validate_basic_auth(session, &auth, required_password().as_deref()).await
    {
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
            tx_had_write: false,
            peer_ip: Some(ip),
            last_write_at: None,
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
        clear_auth_rate_limit(ip).await;

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
        clear_auth_rate_limit(ip).await;

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
        clear_auth_rate_limit(ip).await;

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
        // In tests, GQL_REDIS_URL is not set so the limiter is always InMemory.
        let stale_ip: IpAddr = "10.0.0.21".parse().unwrap();
        let stale_key = stale_ip.to_string();
        let limiter = auth_rate_limiter();
        let map = match limiter {
            AuthRateLimiter::InMemory(lim) => &lim.map,
            AuthRateLimiter::Redis(_) => panic!("expected InMemory limiter in test"),
        };
        map.insert(
            stale_key.clone(),
            (
                AtomicU32::new(99),
                AtomicU64::new(0), // epoch = maximally stale
            ),
        );
        limiter.sweep();
        assert!(
            !map.contains_key(&stale_key),
            "expired auth-failure entries should be pruned"
        );
    }
}
