//! Entry point for the `sedandelivery-bolt` binary.
//!
//! This file is intentionally thin: bootstrap logic lives in submodules under
//! [`server`] (env validation, tracing init, config loading, accept loop).

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::Instant;

use cypher_core::storage::GraphStore;

mod server;

use server::accept_loop;
use server::config::ServerConfig;
use server::env_check::{check_bolt_password_env, check_bolt_tls_env, fatal};

/// Create the graph store based on environment configuration.
///
/// Delegates to [`sd2_redis_store::create_store::create_store`].
fn create_store() -> Arc<dyn GraphStore> {
    sd2_redis_store::create_store::create_store()
}

#[tokio::main]
async fn main() {
    server::tracing_init::init();

    // In production mode, require password auth. TLS is enforced separately
    // via BOLT_REQUIRE_TLS=1 so operators can choose between in-process TLS
    // (for raw TCP ingress on port 7687) and external TLS termination (e.g.
    // a sidecar or private network). Azure Container Apps handles TLS for
    // HTTPS ingress but NOT for raw TCP transport — set BOLT_REQUIRE_TLS=1
    // when Bolt is publicly exposed on TCP.
    let production_mode = std::env::var("ENVIRONMENT").as_deref() == Ok("production");
    if let Err(e) = check_bolt_password_env(production_mode) {
        fatal(&e);
    }
    // Validate TLS env-var consistency (cert without key or vice versa) but
    // do not hard-require TLS based on ENVIRONMENT alone. BOLT_REQUIRE_TLS=1
    // is the opt-in enforcement (checked below, after TLS config is loaded).
    if let Err(e) = check_bolt_tls_env(false) {
        fatal(&e);
    }

    let cfg = ServerConfig::from_env();

    let store = create_store();
    // Pre-create the default "neo4j" database so it exists before any client
    // connects. Always attempt creation rather than check-then-create: the
    // check would route through `route_read()` which may use a replica that
    // hasn't yet replicated the graph registration, causing a spurious
    // create attempt or — if the replica errors — a startup panic.
    // `GraphAlreadyExists` is idempotent and safe to ignore.
    match store.create_graph("neo4j") {
        Ok(()) => {}
        Err(cypher_core::error::StorageError::GraphAlreadyExists { .. }) => {
            tracing::info!("default 'neo4j' graph already exists");
        }
        Err(e) => panic!("failed to create default 'neo4j' database: {e}"),
    }

    // Load TLS configuration (returns None if BOLT_TLS_CERT / BOLT_TLS_KEY are not set).
    let tls_acceptor = sedandelivery_bolt::tls::load_tls_config();
    let required_password = std::env::var("BOLT_PASSWORD").ok();
    if tls_acceptor.is_some() {
        tracing::info!("TLS enabled for Bolt connections");
    } else {
        tracing::info!("TLS not configured, using plaintext");
        if required_password.is_some() {
            tracing::warn!(
                "BOLT_PASSWORD is set but TLS is not configured \
                 — credentials will be transmitted in plaintext"
            );
        }
    }

    if cfg.require_tls && tls_acceptor.is_none() {
        fatal(
            "BOLT_REQUIRE_TLS is set but TLS is not configured \
             (set BOLT_TLS_CERT and BOLT_TLS_KEY)",
        );
    }

    // Spawn periodic auth rate limiter sweep (removes stale per-conn entries).
    sedandelivery_bolt::protocol::handler::spawn_auth_rate_limiter_sweep();

    // Initialize Prometheus metrics recorder.
    let prometheus_handle = sedandelivery_bolt::metrics::init_recorder();

    let active_connections = Arc::new(AtomicU64::new(0));
    let start_time = Instant::now();

    // Spawn the health/metrics HTTP server.
    sedandelivery_bolt::health::spawn_health_server(sedandelivery_bolt::health::HealthState {
        prometheus_handle,
        active_connections: Arc::clone(&active_connections),
        start_time,
        store: Arc::clone(&store),
    });

    accept_loop::serve(cfg, store, tls_acceptor, active_connections).await;
}
