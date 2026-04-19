// Read-only commands: GQL.QUERY and GQL.EXPLAIN.

use redis_module::{Context, NextArg, RedisError, RedisResult, RedisValue};

use cypher_core::ast::Clause;
use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::storage::GraphStore;

use crate::commands::serialize_rows;
use crate::state::{clear_module_context, set_module_context, with_store};

/// GQL.QUERY <graph> <query>
///
/// Executes a Cypher read (or mixed) query. Returns an Array of JSON-encoded
/// rows, one element per row.
pub(crate) fn cmd_gql_query(ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
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

/// GQL.EXPLAIN <graph> <query>
///
/// Returns the physical plan as a Debug-formatted string.
pub(crate) fn cmd_gql_explain(ctx: &Context, args: Vec<redis_module::RedisString>) -> RedisResult {
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
