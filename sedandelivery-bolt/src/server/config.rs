//! Server configuration loaded from environment variables.

use std::time::Duration;

/// All tunable connection-lifecycle / limits parameters parsed from env.
pub(crate) struct ServerConfig {
    pub bind_addr: String,
    pub max_connections: usize,
    pub idle_timeout: Duration,
    pub idle_timeout_secs: u64,
    pub max_connection_lifetime: Option<Duration>,
    pub max_connection_lifetime_secs: u64,
    pub write_concurrency: u32,
    pub handshake_timeout: Duration,
    pub handshake_timeout_ms: u64,
    pub max_conns_per_ip: usize,
    pub keepalive_secs: u64,
    pub require_tls: bool,
}

impl ServerConfig {
    /// Load all tunables from environment variables, applying defaults and
    /// range-clamping where appropriate.
    pub(crate) fn from_env() -> Self {
        let bind_addr = std::env::var("BOLT_BIND_ADDR").unwrap_or_else(|_| {
            tracing::info!("BOLT_BIND_ADDR not set, using default 0.0.0.0:7687");
            "0.0.0.0:7687".to_string()
        });

        // --- Connection lifecycle configuration ---
        //
        // Short idle timeouts and bounded connection lifetimes are critical for
        // L4 (TCP-level) load balancing such as Envoy in Container Apps.  L4
        // balancers assign a connection to a backend at connect-time and never
        // move it.  Long-lived connections therefore "pin" to the original
        // replica, starving new replicas after a scale-out event.  By capping
        // idle time (300 s) and absolute lifetime (1800 s) we force clients to
        // reconnect periodically, giving the load balancer a chance to spread
        // traffic across all healthy replicas.
        let max_connections: usize =
            cypher_core::config::parse_env_range("BOLT_MAX_CONNECTIONS", 1024, 1, 65535);

        let idle_timeout_secs: u64 =
            cypher_core::config::parse_env_range("BOLT_IDLE_TIMEOUT_SECS", 300, 1, 86400);
        let idle_timeout = Duration::from_secs(idle_timeout_secs);

        let max_connection_lifetime_secs: u64 = cypher_core::config::parse_env_range(
            "BOLT_MAX_CONNECTION_LIFETIME_SECS",
            1800,
            0,
            86400,
        );
        let max_connection_lifetime = if max_connection_lifetime_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(max_connection_lifetime_secs))
        };

        let write_concurrency: u32 =
            cypher_core::config::parse_env_range("BOLT_WRITE_CONCURRENCY", 128, 1, 10000);

        // Pre-handshake timeout: max time to complete TLS/WebSocket handshake
        // before the slot is reclaimed. Prevents slot exhaustion from slow clients.
        let handshake_timeout_ms: u64 = std::env::var("BOLT_HANDSHAKE_TIMEOUT_MS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(5_000);
        let handshake_timeout = Duration::from_millis(handshake_timeout_ms);

        // Per-IP connection limit (economic DoS protection).
        let max_conns_per_ip: usize = std::env::var("BOLT_MAX_CONN_PER_IP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(50);

        // --- TCP keepalive configuration ---
        //
        // Azure Load Balancer has a 4-minute (240s) TCP idle timeout. Connections
        // that sit idle longer than this are silently dropped, causing clients to
        // see unexpected `ConnectionResetError` with no server-side indication.
        //
        // To prevent this, we enable TCP keepalive on every accepted connection
        // with a probe interval well under Azure's 240s threshold (default 120s).
        // The kernel sends probes at TCP_KEEPINTVL (30s) intervals up to
        // TCP_KEEPCNT (3) times before declaring the connection dead.
        let keepalive_secs: u64 =
            cypher_core::config::parse_env_range("BOLT_TCP_KEEPALIVE_SECS", 120, 10, 300);

        // If BOLT_REQUIRE_TLS is set to exactly "1", refuse to start without TLS.
        // Only "1" is accepted — "true", "yes", and other truthy strings are ignored.
        // This is the 12-factor / container-boolean convention: set BOLT_REQUIRE_TLS=1
        // (not "true") to enforce TLS. Using a strict value prevents accidental
        // activation from leftover debug env vars like BOLT_REQUIRE_TLS=false.
        let require_tls = std::env::var("BOLT_REQUIRE_TLS")
            .map(|v| v.trim() == "1")
            .unwrap_or(false);

        Self {
            bind_addr,
            max_connections,
            idle_timeout,
            idle_timeout_secs,
            max_connection_lifetime,
            max_connection_lifetime_secs,
            write_concurrency,
            handshake_timeout,
            handshake_timeout_ms,
            max_conns_per_ip,
            keepalive_secs,
            require_tls,
        }
    }
}

/// Read `BOLT_DRAIN_TIMEOUT_SECS` (default 30) used during graceful shutdown.
pub(crate) fn drain_timeout() -> Duration {
    let secs: u64 = std::env::var("BOLT_DRAIN_TIMEOUT_SECS")
        .ok()
        .and_then(|v| match v.parse::<u64>() {
            Ok(val) if val > 0 => Some(val),
            _ => {
                tracing::warn!("invalid BOLT_DRAIN_TIMEOUT_SECS value, using default 30");
                None
            }
        })
        .unwrap_or(30);
    Duration::from_secs(secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    /// Serialise env-mutating tests because `from_env()` reads many keys.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Snapshot the BOLT_* env vars touched by `ServerConfig::from_env()` and
    /// `drain_timeout()`, then restore them on drop. Holds `ENV_LOCK` to keep
    /// concurrent tests from racing on shared process state.
    struct EnvGuard {
        prev: Vec<(&'static str, Option<String>)>,
        _lock: MutexGuard<'static, ()>,
    }

    impl EnvGuard {
        fn new() -> Self {
            let lock = match ENV_LOCK.lock() {
                Ok(g) => g,
                Err(p) => p.into_inner(),
            };
            let keys = [
                "BOLT_BIND_ADDR",
                "BOLT_MAX_CONNECTIONS",
                "BOLT_IDLE_TIMEOUT_SECS",
                "BOLT_MAX_CONNECTION_LIFETIME_SECS",
                "BOLT_WRITE_CONCURRENCY",
                "BOLT_HANDSHAKE_TIMEOUT_MS",
                "BOLT_MAX_CONN_PER_IP",
                "BOLT_TCP_KEEPALIVE_SECS",
                "BOLT_REQUIRE_TLS",
                "BOLT_DRAIN_TIMEOUT_SECS",
            ];
            let prev: Vec<(&'static str, Option<String>)> =
                keys.iter().map(|k| (*k, std::env::var(k).ok())).collect();
            for k in &keys {
                std::env::remove_var(k);
            }
            Self { prev, _lock: lock }
        }

        fn set(&self, k: &str, v: &str) {
            std::env::set_var(k, v);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (k, v) in self.prev.drain(..) {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn from_env_returns_documented_defaults_when_unset() {
        let _g = EnvGuard::new();
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.bind_addr, "0.0.0.0:7687");
        assert_eq!(cfg.max_connections, 1024);
        assert_eq!(cfg.idle_timeout_secs, 300);
        assert_eq!(cfg.idle_timeout, Duration::from_secs(300));
        assert_eq!(cfg.max_connection_lifetime_secs, 1800);
        assert_eq!(cfg.max_connection_lifetime, Some(Duration::from_secs(1800)));
        assert_eq!(cfg.write_concurrency, 128);
        assert_eq!(cfg.handshake_timeout_ms, 5_000);
        assert_eq!(cfg.handshake_timeout, Duration::from_millis(5_000));
        assert_eq!(cfg.max_conns_per_ip, 50);
        assert_eq!(cfg.keepalive_secs, 120);
        assert!(!cfg.require_tls);
    }

    #[test]
    fn from_env_picks_up_explicit_overrides() {
        let g = EnvGuard::new();
        g.set("BOLT_BIND_ADDR", "127.0.0.1:9999");
        g.set("BOLT_MAX_CONNECTIONS", "256");
        g.set("BOLT_IDLE_TIMEOUT_SECS", "60");
        g.set("BOLT_WRITE_CONCURRENCY", "8");
        g.set("BOLT_HANDSHAKE_TIMEOUT_MS", "1500");
        g.set("BOLT_MAX_CONN_PER_IP", "9");
        g.set("BOLT_TCP_KEEPALIVE_SECS", "30");
        g.set("BOLT_REQUIRE_TLS", "1");

        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.bind_addr, "127.0.0.1:9999");
        assert_eq!(cfg.max_connections, 256);
        assert_eq!(cfg.idle_timeout_secs, 60);
        assert_eq!(cfg.write_concurrency, 8);
        assert_eq!(cfg.handshake_timeout_ms, 1500);
        assert_eq!(cfg.max_conns_per_ip, 9);
        assert_eq!(cfg.keepalive_secs, 30);
        assert!(cfg.require_tls);
    }

    #[test]
    fn lifetime_zero_disables_max_connection_lifetime() {
        let g = EnvGuard::new();
        g.set("BOLT_MAX_CONNECTION_LIFETIME_SECS", "0");
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.max_connection_lifetime_secs, 0);
        assert!(cfg.max_connection_lifetime.is_none());
    }

    #[test]
    fn require_tls_only_activates_for_exact_one() {
        let g = EnvGuard::new();
        for v in &["true", "yes", "0", "TRUE", ""] {
            g.set("BOLT_REQUIRE_TLS", v);
            assert!(
                !ServerConfig::from_env().require_tls,
                "BOLT_REQUIRE_TLS={v:?} must not enable TLS enforcement",
            );
        }
        g.set("BOLT_REQUIRE_TLS", "1");
        assert!(ServerConfig::from_env().require_tls);
    }

    #[test]
    fn handshake_timeout_falls_back_to_default_on_garbage() {
        let g = EnvGuard::new();
        g.set("BOLT_HANDSHAKE_TIMEOUT_MS", "not-a-number");
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.handshake_timeout_ms, 5_000);
    }

    #[test]
    fn max_conns_per_ip_falls_back_to_default_on_garbage() {
        let g = EnvGuard::new();
        g.set("BOLT_MAX_CONN_PER_IP", "abc");
        let cfg = ServerConfig::from_env();
        assert_eq!(cfg.max_conns_per_ip, 50);
    }

    #[test]
    fn drain_timeout_default_when_unset() {
        let _g = EnvGuard::new();
        assert_eq!(drain_timeout(), Duration::from_secs(30));
    }

    #[test]
    fn drain_timeout_uses_explicit_value() {
        let g = EnvGuard::new();
        g.set("BOLT_DRAIN_TIMEOUT_SECS", "7");
        assert_eq!(drain_timeout(), Duration::from_secs(7));
    }

    #[test]
    fn drain_timeout_rejects_zero_and_garbage() {
        let g = EnvGuard::new();
        g.set("BOLT_DRAIN_TIMEOUT_SECS", "0");
        assert_eq!(drain_timeout(), Duration::from_secs(30));
        g.set("BOLT_DRAIN_TIMEOUT_SECS", "not-a-u64");
        assert_eq!(drain_timeout(), Duration::from_secs(30));
    }
}
