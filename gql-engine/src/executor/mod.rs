//! GQL query executor.
//!
//! Wraps cypher-core's [`QueryExecutor`] and enforces GQL-specific semantics:
//! - **Mandatory match**: returns an error if the query produced zero result rows.
//! - **Path mode filtering**: post-filters results for TRAIL/SIMPLE/ACYCLIC
//!   constraints (future — currently a no-op placeholder).

use std::collections::HashMap;

use cypher_core::error::GqlError;
use cypher_core::executor::{ExecutionBudget, QueryExecutor, QueryResult};
use cypher_core::procedure::ProcedureRegistry;
use cypher_core::storage::GraphStore;
use cypher_core::types::Value;

use crate::planner::GqlPlan;

/// GQL-specific execution error.
#[derive(Debug, thiserror::Error)]
pub enum GqlExecutionError {
    /// MANDATORY MATCH produced no rows.
    #[error("mandatory match produced no results")]
    MandatoryMatchEmpty,
    /// Underlying engine error.
    #[error("{0}")]
    Engine(#[from] GqlError),
}

/// Execute a planned GQL query.
pub fn execute_gql(
    plan: &GqlPlan,
    store: &dyn GraphStore,
    graph: &str,
) -> Result<QueryResult, GqlExecutionError> {
    let executor = QueryExecutor::new(store);
    let mut budget = ExecutionBudget::default_budget();
    let result = executor.execute(graph, &plan.operator, &mut budget)?;

    // Enforce MANDATORY MATCH: error if zero rows returned.
    if plan.mandatory_match && result.rows.is_empty() {
        return Err(GqlExecutionError::MandatoryMatchEmpty);
    }

    Ok(result)
}

/// Execute a planned GQL query with procedure registry and parameters.
pub fn execute_gql_with_context(
    plan: &GqlPlan,
    store: &dyn GraphStore,
    graph: &str,
    registry: &ProcedureRegistry,
    parameters: HashMap<String, Value>,
) -> Result<QueryResult, GqlExecutionError> {
    let executor = QueryExecutor::with_context(store, registry, parameters);
    let mut budget = ExecutionBudget::default_budget();
    let result = executor.execute(graph, &plan.operator, &mut budget)?;

    // Enforce MANDATORY MATCH: error if zero rows returned.
    if plan.mandatory_match && result.rows.is_empty() {
        return Err(GqlExecutionError::MandatoryMatchEmpty);
    }

    Ok(result)
}
