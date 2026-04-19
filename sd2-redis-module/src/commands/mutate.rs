// Write command: GQL.MUTATE.

use redis_module::{Context, NextArg, RedisError, RedisResult, RedisValue};

use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::storage::GraphStore;

use crate::changelog::{discard_changelog, emit_changelog};
use crate::commands::serialize_rows;
use crate::state::{clear_module_context, set_module_context, with_store};

/// GQL.MUTATE <graph> <query>
///
/// Executes a write (mutating) query. Returns "OK" followed by stats.
pub(crate) fn cmd_gql_mutate(ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
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
