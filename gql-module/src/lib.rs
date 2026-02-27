// gql-module: Valkey/Redis module entry point.
//
// Exposes the following commands:
//   GQL.QUERY <graph> <query>       — execute read query, return JSON rows
//   GQL.MUTATE <graph> <query>      — execute write query, return OK + stats
//   GQL.CREATE-GRAPH <graph>        — create a named graph
//   GQL.DELETE-GRAPH <graph>        — delete a named graph
//   GQL.LIST-GRAPHS                 — list all graph names
//   GQL.EXPLAIN <graph> <query>     — return the physical plan as a string
//
// Safety: every command handler uses std::panic::catch_unwind to prevent a
// panic from killing the Redis/Valkey process (and all connected clients).

use std::sync::Mutex;

use once_cell::sync::Lazy;
use redis_module::{redis_module, Context, NextArg, RedisError, RedisResult, RedisValue};

use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::storage::GraphStore;
use gql_redis_store::RedisGraphStore;

// ── Global engine state ───────────────────────────────────────────────────────

/// The global RedisGraphStore, initialised at module load.
///
/// We store it in a Mutex so it is `Sync`. The inner `RedisGraphStore` already
/// has its own internal `Mutex<Connection>`, so the outer Mutex here guards
/// the `Option` slot before the store is initialised.
static STORE: Lazy<Mutex<Option<RedisGraphStore>>> = Lazy::new(|| Mutex::new(None));

/// Acquire a reference to the store.  Returns a RedisError if the store is
/// not yet initialised (should not happen after OnLoad succeeds).
fn with_store<F, T>(f: F) -> RedisResult<T>
where
    F: FnOnce(&RedisGraphStore) -> RedisResult<T>,
{
    let guard = STORE.lock().map_err(|e| {
        RedisError::String(format!("store mutex poisoned: {e}"))
    })?;
    match guard.as_ref() {
        Some(store) => f(store),
        None => Err(RedisError::Str("GQL module not initialised")),
    }
}

// ── Initialisation ────────────────────────────────────────────────────────────

fn module_init(_ctx: &Context, _args: &[redis_module::RedisString]) -> redis_module::Status {
    let url = std::env::var("GQL_REDIS_URL")
        .or_else(|_| std::env::var("REDIS_URL"))
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".into());

    match RedisGraphStore::from_url(&url) {
        Ok(store) => {
            let mut guard = match STORE.lock() {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("gql-module: store mutex poisoned on init: {e}");
                    return redis_module::Status::Err;
                }
            };
            *guard = Some(store);
            redis_module::Status::Ok
        }
        Err(e) => {
            eprintln!("gql-module: failed to connect to Redis at {url}: {e}");
            // Log the error but don't fail the module load — the store may
            // become available later (e.g. Azure Cache for Redis with retries).
            // Commands will return errors until the store is initialised.
            redis_module::Status::Ok
        }
    }
}

// ── Command: GQL.QUERY ───────────────────────────────────────────────────────

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
                        .map(|(k, v)| (k.clone(), cypher_value_to_json(v)))
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

// ── Command: GQL.MUTATE ──────────────────────────────────────────────────────

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

            let s = result.stats;
            let stats_str = format!(
                "nodes_created={} nodes_deleted={} edges_created={} edges_deleted={} properties_set={} labels_added={}",
                s.nodes_created, s.nodes_deleted, s.edges_created, s.edges_deleted,
                s.properties_set, s.labels_added,
            );
            Ok(RedisValue::Array(vec![
                RedisValue::SimpleStringStatic("OK"),
                RedisValue::BulkString(stats_str),
            ]))
        })
    })
    .unwrap_or_else(|_| Err(RedisError::Str("GQL.MUTATE: internal panic")))
}

// ── Command: GQL.CREATE-GRAPH ────────────────────────────────────────────────

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

// ── Command: GQL.DELETE-GRAPH ────────────────────────────────────────────────

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

// ── Command: GQL.LIST-GRAPHS ─────────────────────────────────────────────────

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

// ── Command: GQL.EXPLAIN ─────────────────────────────────────────────────────

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

// ── Value serialisation ───────────────────────────────────────────────────────

fn cypher_value_to_json(v: &gql_core::types::Value) -> serde_json::Value {
    use gql_core::types::Value;
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::Value::Bool(*b),
        Value::Int(i) => serde_json::Value::Number((*i).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Str(s) => serde_json::Value::String(s.to_string()),
        Value::Date(d) => serde_json::Value::String(d.to_string()),
        Value::LocalTime(t) => serde_json::Value::String(t.to_string()),
        Value::LocalDateTime(dt) => serde_json::Value::String(dt.to_string()),
        Value::DateTime(dt) => serde_json::Value::String(dt.to_string()),
        Value::Duration(sp) => serde_json::Value::String(sp.to_string()),
        Value::Point(p) => {
            serde_json::json!({"x": p.x, "y": p.y})
        }
        Value::List(items) => {
            serde_json::Value::Array(items.iter().map(cypher_value_to_json).collect())
        }
        Value::Map(m) => {
            let obj: serde_json::Map<_, _> = m
                .iter()
                .map(|(k, v)| (k.to_string(), cypher_value_to_json(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        Value::Node(id) => serde_json::json!({"__node": id.0}),
        Value::Edge(id) => serde_json::json!({"__edge": id.0}),
        Value::Path { nodes, edges } => {
            let ns: Vec<_> = nodes.iter().map(|n| n.0).collect();
            let es: Vec<_> = edges.iter().map(|e| e.0).collect();
            serde_json::json!({"__path": {"nodes": ns, "edges": es}})
        }
    }
}

// ── Module registration ───────────────────────────────────────────────────────

redis_module! {
    name: "gql",
    version: 1,
    allocator: (redis_module::alloc::RedisAlloc, redis_module::alloc::RedisAlloc),
    data_types: [],
    init: module_init,
    commands: [
        ["GQL.QUERY",        cmd_gql_query,        "readonly",  0, 0, 0],
        ["GQL.MUTATE",       cmd_gql_mutate,       "write",     0, 0, 0],
        ["GQL.CREATE-GRAPH", cmd_gql_create_graph, "write",     0, 0, 0],
        ["GQL.DELETE-GRAPH", cmd_gql_delete_graph, "write",     0, 0, 0],
        ["GQL.LIST-GRAPHS",  cmd_gql_list_graphs,  "readonly",  0, 0, 0],
        ["GQL.EXPLAIN",      cmd_gql_explain,      "readonly",  0, 0, 0],
    ],
}
