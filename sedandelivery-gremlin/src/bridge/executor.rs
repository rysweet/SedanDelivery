//! Gremlin bytecode → cypher-core execution bridge.
//!
//! Primary path: deserialize bytecode into `GremlinStep`s, compile to an
//! `Operator` tree via `GremlinCompiler`, and execute directly.
//!
//! Fallback: if the compiler returns `UnsupportedStep`, translate bytecode
//! to Cypher and run through parse → plan → execute.

use std::collections::HashMap;

use cypher_core::error::{ExecutionError, GqlError};
use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::procedure::ProcedureRegistry;
use cypher_core::storage::GraphStore;
use cypher_core::types::Value;
use gremlin_core::bytecode::deserialize_bytecode;
use gremlin_core::compiler::{CompileError, GremlinCompiler};

use super::cypher_translation::bytecode_to_cypher;
use super::ensure_graph_exists;
use super::types::query_result_to_graphson;

/// Shorthand for creating a GqlError from a message string.
fn gremlin_err(msg: impl Into<String>) -> GqlError {
    GqlError::Execution(ExecutionError::UnsupportedFeature {
        feature: msg.into(),
    })
}

/// Unwrap the optional `@value` envelope from GraphSON bytecode.
fn unwrap_bytecode_value(json: &serde_json::Value) -> &serde_json::Value {
    json.get("@value").unwrap_or(json)
}

/// Execute a Gremlin traversal via direct compilation.
///
/// Falls back to Cypher translation when the compiler encounters an
/// unsupported step.
pub fn execute_gremlin(
    store: &dyn GraphStore,
    graph: &str,
    bytecode_json: &serde_json::Value,
) -> Result<Vec<serde_json::Value>, GqlError> {
    ensure_graph_exists(store, graph)?;

    let inner = unwrap_bytecode_value(bytecode_json);

    // Deserialize bytecode into typed steps.
    let steps = deserialize_bytecode(inner).map_err(|e| gremlin_err(e.to_string()))?;

    // Compile to an Operator tree.
    let mut compiler = GremlinCompiler::new();
    let operator = match compiler.compile(&steps) {
        Ok(op) => op,
        Err(CompileError::UnsupportedStep(step)) => {
            tracing::info!(step, "unsupported step, falling back to Cypher bridge");
            return execute_gremlin_via_cypher(store, graph, bytecode_json);
        }
        Err(e) => return Err(gremlin_err(e.to_string())),
    };

    tracing::debug!(?operator, "compiled gremlin to operator tree");

    // Execute the operator tree.
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::default_budget();
    let result = executor.execute(graph, &operator, &mut budget)?;

    Ok(query_result_to_graphson(&result))
}

/// Fallback: translate bytecode to Cypher, then parse → plan → execute.
pub fn execute_gremlin_via_cypher(
    store: &dyn GraphStore,
    graph: &str,
    bytecode_json: &serde_json::Value,
) -> Result<Vec<serde_json::Value>, GqlError> {
    ensure_graph_exists(store, graph)?;

    let cypher = bytecode_to_cypher(bytecode_json)?;
    tracing::debug!(%cypher, "translated bytecode to cypher (fallback)");

    let params: HashMap<String, Value> = HashMap::new();
    let registry = ProcedureRegistry::default();

    let ast = parse_query(&cypher)?;
    let planner = QueryPlanner::with_context(store, graph, &registry, &params);
    let plan = planner.plan(&ast)?;
    let executor = QueryExecutor::with_context(store, &registry, params);
    let mut budget = ExecutionBudget::default_budget();
    let result = executor.execute(graph, &plan, &mut budget)?;

    Ok(query_result_to_graphson(&result))
}
