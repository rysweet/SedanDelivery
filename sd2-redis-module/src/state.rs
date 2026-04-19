// Global engine state and lifecycle helpers for the GQL Redis module.
//
// Keeps the `STORE` singleton, the `CHANGELOG_ENABLED` flag, and the small
// helpers that command handlers use to access them. Splitting these out of
// `lib.rs` keeps the entry-point file focused on module registration.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use redis_module::{Context, RedisError, RedisResult};

use cypher_core::changelog::ChangeLogStore;

use crate::module_store::ModuleRedisStore;

/// The global store, initialised once at module load.
///
/// `ChangeLogStore` wraps `ModuleRedisStore` and records write operations
/// for optional replication to the __sd2:changelog Valkey stream.
///
/// `ModuleRedisStore` uses the Valkey Module API (ctx.call()) instead of TCP
/// connections, avoiding the loopback deadlock while sharing the same Redis
/// key schema as `RedisGraphStore`.
pub(crate) static STORE: OnceLock<ChangeLogStore<ModuleRedisStore>> = OnceLock::new();

/// Set to `true` at module init when `GQL_FABRIC_CHANGELOG=1` or any
/// `GQL_FABRIC_ENDPOINT` variable is present. When false, drain_events()
/// results are discarded and no XADD calls are made.
pub(crate) static CHANGELOG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Acquire a reference to the store. Returns a RedisError if not initialised.
pub(crate) fn with_store<F, T>(f: F) -> RedisResult<T>
where
    F: FnOnce(&ChangeLogStore<ModuleRedisStore>) -> RedisResult<T>,
{
    match STORE.get() {
        Some(store) => f(store),
        None => Err(RedisError::Str("GQL module not initialised")),
    }
}

/// Set the module context for ModuleRedisStore so that GraphStore methods can
/// call Redis commands via ctx.call(). Must be called at the start of every
/// command handler; `clear_module_context` must be called at the end.
pub(crate) fn set_module_context(ctx: &Context) {
    ModuleRedisStore::set_context(ctx);
}

/// Clear the stashed context after a command handler completes. This prevents
/// a stale pointer from being used by a subsequent command (though in practice
/// Valkey dispatches commands sequentially on the same thread).
pub(crate) fn clear_module_context() {
    ModuleRedisStore::clear_context();
}

/// Module initialisation: parse env vars, set the changelog flag, and
/// install the global store. Invoked by the `redis_module!` macro.
pub(crate) fn module_init(
    _ctx: &Context,
    _args: &[redis_module::RedisString],
) -> redis_module::Status {
    // Enable changelog when Fabric endpoint or explicit flag is set.
    let enabled = std::env::var("GQL_FABRIC_CHANGELOG")
        .map(|v| {
            if v == "1" || v.to_lowercase() == "true" {
                true
            } else if v == "0" || v.to_lowercase() == "false" || v.is_empty() {
                false
            } else {
                tracing::warn!(
                    "unrecognized value for GQL_FABRIC_CHANGELOG: {v:?}, expected \"1\"/\"true\" or \"0\"/\"false\"; treating as disabled"
                );
                false
            }
        })
        .unwrap_or(false)
        || std::env::var("GQL_FABRIC_ENDPOINT")
            .map(|v| !v.is_empty())
            .unwrap_or(false);
    CHANGELOG_ENABLED.store(enabled, Ordering::Relaxed);

    if enabled {
        tracing::info!(
            "GQL module: changelog enabled — writing to per-graph streams __sd2:changelog:{{<graph>}}"
        );
    }

    let store = ChangeLogStore::new(ModuleRedisStore::new());
    if STORE.set(store).is_err() {
        tracing::warn!("GQL module: STORE already initialised (double module_init?)");
    }
    redis_module::Status::Ok
}
