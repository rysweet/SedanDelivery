// gql-module: Valkey/Redis module entry point.
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
// Architecture: graph data is stored in-process via InMemoryGraphStore.
// This avoids the loopback deadlock that occurs when using a TCP-based
// RedisGraphStore from within a Redis module (Redis is single-threaded,
// so a command handler cannot send TCP commands to itself).
//
// Persistence: graph data lives in the module's heap memory. Valkey's
// RDB/AOF persistence saves the full server state, but InMemoryGraphStore
// data is not currently serialized via RDB save/load hooks. Persistence
// across restarts requires RDB save/load hooks (tracked as a follow-up).

use std::sync::OnceLock;

use redis_module::{redis_module, Context, NextArg, RedisError, RedisResult, RedisValue};

use gql_core::ast::Clause;
use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::serde_value::value_to_json;
use gql_core::storage::{GraphStore, InMemoryGraphStore};

// -- Global engine state --

/// The global InMemoryGraphStore, initialised once at module load.
///
/// Uses `OnceLock` so that query execution does not serialise behind an
/// outer lock -- the inner `InMemoryGraphStore` already guards its data
/// with its own internal synchronization.
static STORE: OnceLock<InMemoryGraphStore> = OnceLock::new();

/// Acquire a reference to the store.  Returns a RedisError if the store is
/// not yet initialised (should not happen after OnLoad succeeds).
fn with_store<F, T>(f: F) -> RedisResult<T>
where
    F: FnOnce(&InMemoryGraphStore) -> RedisResult<T>,
{
    match STORE.get() {
        Some(store) => f(store),
        None => Err(RedisError::Str("GQL module not initialised")),
    }
}

// -- Initialisation --

fn module_init(_ctx: &Context, _args: &[redis_module::RedisString]) -> redis_module::Status {
    let store = InMemoryGraphStore::new();
    let _ = STORE.set(store);
    redis_module::Status::Ok
}

// -- Command: GQL.QUERY --

/// GQL.QUERY <graph> <query>
///
/// Executes a Cypher read (or mixed) query. Returns an Array of JSON-encoded
/// rows, one element per row.
fn cmd_gql_query(_ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1); // skip command name
        let graph = args.next_string()?;
        let query = args.next_string()?;
        args.done()?;

        with_store(|store| {
            let ast = parse_query(&query).map_err(|e| {
                RedisError::String(format!("parse error: {e}"))
            })?;

            // GQL.QUERY is registered as "readonly": reject queries that contain
            // write clauses (Create/Set/Delete/Remove/Merge).  Use GQL.MUTATE instead.
            let has_write = ast.clauses.iter().any(|c| matches!(c,
                Clause::Create(_) | Clause::Set(_) | Clause::Delete(_) |
                Clause::Remove(_) | Clause::Merge(_) | Clause::Foreach(_)
            ));
            if has_write {
                return Err(RedisError::String(
                    "GQL.QUERY is read-only; use GQL.MUTATE for write operations".into(),
                ));
            }

            let planner = QueryPlanner::new(store, &graph);
            let plan = planner.plan(&ast).map_err(|e| {
                RedisError::String(format!("plan error: {e}"))
            })?;
            let mut budget = ExecutionBudget::default_budget();
            let result = QueryExecutor::new(store)
                .execute(&graph, &plan, &mut budget)
                .map_err(|e| RedisError::String(format!("execution error: {e}")))?;

            // Serialise each row to a JSON string.
            let rows: Vec<RedisValue> = result
                .rows
                .iter()
                .map(|row| {
                    let ordered: std::collections::BTreeMap<_, _> = result
                        .columns
                        .iter()
                        .filter_map(|col| row.get(col).map(|v| (col, v)))
                        .collect();
                    let json_row: serde_json::Map<String, serde_json::Value> = ordered
                        .into_iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                value_to_json(v).unwrap_or(serde_json::Value::Null),
                            )
                        })
                        .collect();
                    RedisValue::BulkString(
                        serde_json::to_string(&json_row).unwrap_or_else(|e| {
                            format!("{{\"error\":\"{e}\"}}")
                        }),
                    )
                })
                .collect();
            Ok(RedisValue::Array(rows))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.QUERY: internal panic")))
}

// -- Command: GQL.MUTATE --

/// GQL.MUTATE <graph> <query>
///
/// Executes a write (mutating) query. Returns "OK" followed by stats.
fn cmd_gql_mutate(_ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        let graph = args.next_string()?;
        let query = args.next_string()?;
        args.done()?;

        with_store(|store| {
            let ast = parse_query(&query).map_err(|e| {
                RedisError::String(format!("parse error: {e}"))
            })?;
            let planner = QueryPlanner::new(store, &graph);
            let plan = planner.plan(&ast).map_err(|e| {
                RedisError::String(format!("plan error: {e}"))
            })?;
            let mut budget = ExecutionBudget::default_budget();
            let result = QueryExecutor::new(store)
                .execute(&graph, &plan, &mut budget)
                .map_err(|e| RedisError::String(format!("execution error: {e}")))?;

            // Serialise each row to a JSON string (same as GQL.QUERY).
            let json_rows: Vec<RedisValue> = result
                .rows
                .iter()
                .map(|row| {
                    let ordered: std::collections::BTreeMap<_, _> = result
                        .columns
                        .iter()
                        .filter_map(|col| row.get(col).map(|v| (col, v)))
                        .collect();
                    let json_row: serde_json::Map<String, serde_json::Value> = ordered
                        .into_iter()
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                value_to_json(v).unwrap_or(serde_json::Value::Null),
                            )
                        })
                        .collect();
                    RedisValue::BulkString(
                        serde_json::to_string(&json_row).unwrap_or_else(|e| {
                            format!("{{\"error\":\"{e}\"}}")
                        }),
                    )
                })
                .collect();

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
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.MUTATE: internal panic")))
}

// -- Command: GQL.CREATE-GRAPH --

fn cmd_gql_create_graph(_ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    std::panic::catch_unwind(|| {
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
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.CREATE-GRAPH: internal panic")))
}

// -- Command: GQL.DELETE-GRAPH --

fn cmd_gql_delete_graph(_ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    std::panic::catch_unwind(|| {
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
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.DELETE-GRAPH: internal panic")))
}

// -- Command: GQL.LIST-GRAPHS --

fn cmd_gql_list_graphs(_ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        args.done()?;

        with_store(|store| {
            let names = store
                .list_graphs()
                .map_err(|e| RedisError::String(format!("{e}")))?;
            let values: Vec<RedisValue> = names
                .into_iter()
                .map(RedisValue::BulkString)
                .collect();
            Ok(RedisValue::Array(values))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.LIST-GRAPHS: internal panic")))
}

// -- Command: GQL.EXPLAIN --

fn cmd_gql_explain(_ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
    std::panic::catch_unwind(|| {
        let mut args = args.into_iter().skip(1);
        let graph = args.next_string()?;
        let query = args.next_string()?;
        args.done()?;

        with_store(|store| {
            let ast = parse_query(&query).map_err(|e| {
                RedisError::String(format!("parse error: {e}"))
            })?;
            let planner = QueryPlanner::new(store, &graph);
            let plan = planner.plan(&ast).map_err(|e| {
                RedisError::String(format!("plan error: {e}"))
            })?;
            // Use Debug formatting for the plan.
            Ok(RedisValue::BulkString(format!("{plan:#?}")))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.EXPLAIN: internal panic")))
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
