// sd2-redis-module: Valkey/Redis module entry point.
//
// Exposes the following commands:
//   GQL.QUERY <graph> <query>       -- execute read query, return JSON rows
//   GQL.MUTATE <graph> <query>      -- execute write query, return OK + stats
//   GQL.CREATE-GRAPH <graph>        -- create a named graph
//   GQL.DELETE-GRAPH <graph>        -- delete a named graph
//   GQL.LIST-GRAPHS                 -- list all graph names
//   GQL.EXPLAIN <graph> <query>     -- return the physical plan as a string
//
// Safety: every command handler uses std::panic::catch_unwind to prevent a
// panic from killing the Redis/Valkey process (and all connected clients).
//
// Architecture: graph data is stored in Redis via ModuleRedisStore which uses
// ctx.call() (the Valkey Module internal dispatch) to execute Redis commands.
// This avoids the TCP loopback deadlock that prevents a regular RedisGraphStore
// from working inside a module, while using the EXACT same key schema so that
// the Bolt server, REST API, and GQL module all read/write the same data.
//
// ModuleRedisStore is wrapped in a ChangeLogStore that records every write as
// a ChangeEvent flushed to the __sd2:changelog Valkey stream after each
// successful write command (when GQL_FABRIC_CHANGELOG=1 is set).
//
// Changelog: set GQL_FABRIC_CHANGELOG=1 (or GQL_FABRIC_ENDPOINT=<url>) at
// module load time to enable async replication to Microsoft Fabric via the
// sedandelivery-fabric-sync sidecar service.

mod module_store;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use redis_module::{redis_module, Context, NextArg, RedisError, RedisResult, RedisValue};

use cypher_core::ast::Clause;
use cypher_core::changelog::{changelog_stream_for_graph, ChangeLogStore};
use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::serde_value::value_to_json;
use cypher_core::storage::GraphStore;

use module_store::ModuleRedisStore;

// -- Global engine state --

/// The global store, initialised once at module load.
///
/// `ChangeLogStore` wraps `ModuleRedisStore` and records write operations
/// for optional replication to the __sd2:changelog Valkey stream.
///
/// `ModuleRedisStore` uses the Valkey Module API (ctx.call()) instead of TCP
/// connections, avoiding the loopback deadlock while sharing the same Redis key
/// schema as `RedisGraphStore`.
static STORE: OnceLock<ChangeLogStore<ModuleRedisStore>> = OnceLock::new();

/// Set to `true` at module init when `GQL_FABRIC_CHANGELOG=1` or any
/// `GQL_FABRIC_ENDPOINT` variable is present.  When false, drain_events()
/// results are discarded and no XADD calls are made.
static CHANGELOG_ENABLED: AtomicBool = AtomicBool::new(false);

/// Acquire a reference to the store.  Returns a RedisError if not initialised.
fn with_store<F, T>(f: F) -> RedisResult<T>
where
    F: FnOnce(&ChangeLogStore<ModuleRedisStore>) -> RedisResult<T>,
{
    match STORE.get() {
        Some(store) => f(store),
        None => Err(RedisError::Str("GQL module not initialised")),
    }
}

// -- Context lifecycle --

/// Set the module context for ModuleRedisStore so that GraphStore methods can
/// call Redis commands via ctx.call(). Must be called at the start of every
/// command handler; `clear_module_context` must be called at the end.
fn set_module_context(ctx: &Context) {
    ModuleRedisStore::set_context(ctx);
}

/// Clear the stashed context after a command handler completes. This prevents
/// a stale pointer from being used by a subsequent command (though in practice
/// Valkey dispatches commands sequentially on the same thread).
fn clear_module_context() {
    ModuleRedisStore::clear_context();
}

// -- Changelog helpers --

/// After a successful write command, drain the ChangeLogStore's pending events
/// and append each one to the __sd2:changelog Valkey stream via ctx.call.
///
/// This is a no-op when CHANGELOG_ENABLED is false — zero overhead in the
/// common case where Fabric sync is not configured.
///
/// XADD failures are logged as warnings but do NOT fail the command — the
/// Redis mutation already committed and must not be rolled back.
fn emit_changelog(ctx: &Context) {
    if !CHANGELOG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let store = match STORE.get() {
        Some(s) => s,
        None => return,
    };
    for event in store.drain_events() {
        let stream_key = changelog_stream_for_graph(event.graph_name());
        match serde_json::to_string(&event) {
            Ok(json) => {
                if let Err(e) = ctx.call("XADD", &[&stream_key, "*", "e", json.as_str()]) {
                    // Log warning but continue — changelog lag is preferable to
                    // rejecting the client's write after it already succeeded.
                    tracing::warn!(stream = %stream_key, "changelog XADD failed: {e:?}");
                }
            }
            Err(e) => {
                tracing::warn!("changelog serialize failed: {e}");
            }
        }
    }
}

/// Discard any pending events accumulated during a failed command so they
/// don't bleed into the next command's changelog batch.
fn discard_changelog() {
    if let Some(store) = STORE.get() {
        let _ = store.drain_events();
    }
}

// -- Initialisation --

fn module_init(_ctx: &Context, _args: &[redis_module::RedisString]) -> redis_module::Status {
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

// -- Shared helpers --

/// Serialise query result rows to JSON-encoded RedisValue bulk strings.
///
/// Used by both GQL.QUERY and GQL.MUTATE to convert `QueryResult.rows`
/// into the wire format returned to clients.
fn serialize_rows(result: &cypher_core::executor::QueryResult) -> Vec<RedisValue> {
    result
        .rows
        .iter()
        .map(|row| {
            // Emit every declared column — missing columns become JSON null
            // (consistent with Neo4j wire protocol for optional-match results).
            let json_row: serde_json::Map<String, serde_json::Value> = result
                .columns
                .iter()
                .map(|col| {
                    let jv = match row.get(col) {
                        Some(v) => match value_to_json(v) {
                            Ok(jv) => jv,
                            Err(e) => {
                                tracing::warn!(column = %col, "value_to_json failed, substituting null: {e}");
                                serde_json::Value::Null
                            }
                        },
                        None => serde_json::Value::Null,
                    };
                    (col.clone(), jv)
                })
                .collect();
            RedisValue::BulkString(
                serde_json::to_string(&json_row)
                    .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
            )
        })
        .collect()
}

// -- Command: GQL.QUERY --

/// GQL.QUERY <graph> <query>
///
/// Executes a Cypher read (or mixed) query. Returns an Array of JSON-encoded
/// rows, one element per row.
fn cmd_gql_query(ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    set_module_context(ctx);
    let result = std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1); // skip command name
        let graph = args.next_string()?;
        let query = args.next_string()?;
        args.done()?;

        with_store(|store| {
            let ast =
                parse_query(&query).map_err(|e| RedisError::String(format!("parse error: {e}")))?;

            // GQL.QUERY is registered as "readonly": reject queries that contain
            // write clauses (Create/Set/Delete/Remove/Merge).  Use GQL.MUTATE instead.
            let has_write = ast.clauses.iter().any(|c| {
                matches!(
                    c,
                    Clause::Create(_)
                        | Clause::Set(_)
                        | Clause::Delete(_)
                        | Clause::Remove(_)
                        | Clause::Merge(_)
                        | Clause::Foreach(_)
                )
            });
            if has_write {
                return Err(RedisError::String(
                    "GQL.QUERY is read-only; use GQL.MUTATE for write operations".into(),
                ));
            }

            let planner = QueryPlanner::new(store as &dyn GraphStore, &graph);
            let plan = planner
                .plan(&ast)
                .map_err(|e| RedisError::String(format!("plan error: {e}")))?;
            let mut budget = ExecutionBudget::default_budget();
            let result = QueryExecutor::new(store as &dyn GraphStore)
                .execute(&graph, &plan, &mut budget)
                .map_err(|e| RedisError::String(format!("execution error: {e}")))?;

            Ok(RedisValue::Array(serialize_rows(&result)))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.QUERY: internal panic")));
    clear_module_context();
    result
}

// -- Command: GQL.MUTATE --

/// GQL.MUTATE <graph> <query>
///
/// Executes a write (mutating) query. Returns "OK" followed by stats.
fn cmd_gql_mutate(ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    set_module_context(ctx);
    let result = std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        let graph = args.next_string()?;
        let query = args.next_string()?;
        args.done()?;

        with_store(|store| {
            let ast = parse_query(&query).map_err(|e| {
                RedisError::String(format!("parse error: {e}"))
            })?;
            let planner = QueryPlanner::new(store as &dyn GraphStore, &graph);
            let plan = planner.plan(&ast).map_err(|e| {
                RedisError::String(format!("plan error: {e}"))
            })?;
            let mut budget = ExecutionBudget::default_budget();
            let result = QueryExecutor::new(store as &dyn GraphStore)
                .execute(&graph, &plan, &mut budget)
                .map_err(|e| RedisError::String(format!("execution error: {e}")))?;

            let json_rows = serialize_rows(&result);
            let s = result.stats;
            let stats_str = format!(
                "nodes_created={} nodes_deleted={} edges_created={} edges_deleted={} properties_set={} labels_added={}",
                s.nodes_created, s.nodes_deleted, s.edges_created, s.edges_deleted,
                s.properties_set, s.labels_added,
            );

            // Return [rows_array, stats_string] so callers get both data and stats.
            Ok(RedisValue::Array(vec![
                RedisValue::Array(json_rows),
                RedisValue::BulkString(stats_str),
            ]))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.MUTATE: internal panic")));

    // Emit changelog events on success; discard partial events on failure.
    // ctx.call("XADD") uses internal dispatch — no TCP, no deadlock.
    //
    // Both emit_changelog and discard_changelog are wrapped in catch_unwind
    // to prevent a panic from unwinding into the Redis event loop (see #397).
    if result.is_ok() {
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emit_changelog(ctx);
        })) {
            tracing::warn!("GQL.MUTATE: panic in emit_changelog: {e:?}");
        }
    } else if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        discard_changelog();
    })) {
        tracing::warn!("GQL.MUTATE: panic in discard_changelog: {e:?}");
    }

    clear_module_context();
    result
}

// -- Command: GQL.CREATE-GRAPH --

fn cmd_gql_create_graph(ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    set_module_context(ctx);
    let result = std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        let graph = args.next_string()?;
        args.done()?;

        with_store(|store| {
            store
                .create_graph(&graph)
                .map(|_| RedisValue::SimpleStringStatic("OK"))
                .map_err(|e| RedisError::String(format!("{e}")))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.CREATE-GRAPH: internal panic")));

    if result.is_ok() {
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emit_changelog(ctx);
        })) {
            tracing::warn!("GQL.CREATE-GRAPH: panic in emit_changelog: {e:?}");
        }
    } else if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        discard_changelog();
    })) {
        tracing::warn!("GQL.CREATE-GRAPH: panic in discard_changelog: {e:?}");
    }

    clear_module_context();
    result
}

// -- Command: GQL.DELETE-GRAPH --

fn cmd_gql_delete_graph(ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    set_module_context(ctx);
    let result = std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        let graph = args.next_string()?;
        args.done()?;

        with_store(|store| {
            store
                .delete_graph(&graph)
                .map(|_| RedisValue::SimpleStringStatic("OK"))
                .map_err(|e| RedisError::String(format!("{e}")))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.DELETE-GRAPH: internal panic")));

    if result.is_ok() {
        if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            emit_changelog(ctx);
        })) {
            tracing::warn!("GQL.DELETE-GRAPH: panic in emit_changelog: {e:?}");
        }
    } else if let Err(e) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        discard_changelog();
    })) {
        tracing::warn!("GQL.DELETE-GRAPH: panic in discard_changelog: {e:?}");
    }

    clear_module_context();
    result
}

// -- Command: GQL.LIST-GRAPHS --

fn cmd_gql_list_graphs(ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    set_module_context(ctx);
    let result = std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        args.done()?;

        with_store(|store| {
            let names = store
                .list_graphs()
                .map_err(|e| RedisError::String(format!("{e}")))?;
            let values: Vec<RedisValue> = names.into_iter().map(RedisValue::BulkString).collect();
            Ok(RedisValue::Array(values))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.LIST-GRAPHS: internal panic")));
    clear_module_context();
    result
}

// -- Command: GQL.EXPLAIN --

fn cmd_gql_explain(ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    set_module_context(ctx);
    let result = std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        let graph = args.next_string()?;
        let query = args.next_string()?;
        args.done()?;

        with_store(|store| {
            let ast =
                parse_query(&query).map_err(|e| RedisError::String(format!("parse error: {e}")))?;
            let planner = QueryPlanner::new(store as &dyn GraphStore, &graph);
            let plan = planner
                .plan(&ast)
                .map_err(|e| RedisError::String(format!("plan error: {e}")))?;
            // Use Debug formatting for the plan.
            Ok(RedisValue::BulkString(format!("{plan:#?}")))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.EXPLAIN: internal panic")));
    clear_module_context();
    result
}

// -- Module registration --

redis_module! {
    name: "gql",
    version: 1,
    allocator: (redis_module::alloc::RedisAlloc, redis_module::alloc::RedisAlloc),
    data_types: [],
    init: module_init,
    commands: [
        ["GQL.QUERY",        cmd_gql_query,        "readonly",  1, 1, 1],
        ["GQL.MUTATE",       cmd_gql_mutate,       "write",     1, 1, 1],
        ["GQL.CREATE-GRAPH", cmd_gql_create_graph, "write",     1, 1, 1],
        ["GQL.DELETE-GRAPH", cmd_gql_delete_graph, "write",     1, 1, 1],
        ["GQL.LIST-GRAPHS",  cmd_gql_list_graphs,  "readonly",  0, 0, 0],
        ["GQL.EXPLAIN",      cmd_gql_explain,      "readonly",  1, 1, 1],
    ],
}
