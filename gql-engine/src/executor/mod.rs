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
    run_and_enforce(plan, &executor, graph)
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
    run_and_enforce(plan, &executor, graph)
}

/// Run the operator tree and enforce GQL-specific constraints (e.g. MANDATORY MATCH).
fn run_and_enforce(
    plan: &GqlPlan,
    executor: &QueryExecutor<'_>,
    graph: &str,
) -> Result<QueryResult, GqlExecutionError> {
    let mut budget = ExecutionBudget::default_budget();
    let result = executor.execute(graph, &plan.operator, &mut budget)?;

    if plan.mandatory_match && result.rows.is_empty() {
        return Err(GqlExecutionError::MandatoryMatchEmpty);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::parse_gql;
    use crate::planner::plan_gql_with_context;
    use cypher_core::storage::InMemoryGraphStore;

    fn setup() -> InMemoryGraphStore {
        let s = InMemoryGraphStore::new();
        s.create_graph("g").unwrap();
        s
    }

    #[test]
    fn execute_gql_with_context_runs_and_returns_rows() {
        let s = setup();
        execute_gql(
            &plan_gql_with_context(
                &parse_gql("INSERT (n:T {x:1})").unwrap(),
                &s,
                "g",
                &ProcedureRegistry::default(),
                &HashMap::new(),
            )
            .unwrap(),
            &s,
            "g",
        )
        .unwrap();

        let plan = plan_gql_with_context(
            &parse_gql("MATCH (n:T) RETURN n.x").unwrap(),
            &s,
            "g",
            &ProcedureRegistry::default(),
            &HashMap::new(),
        )
        .unwrap();
        let qr = execute_gql_with_context(
            &plan,
            &s,
            "g",
            &ProcedureRegistry::default(),
            HashMap::new(),
        )
        .unwrap();
        assert_eq!(qr.rows.len(), 1);
    }

    #[test]
    fn execute_gql_mandatory_match_empty_errors() {
        let s = setup();
        let plan = plan_gql_with_context(
            &parse_gql("MANDATORY MATCH (n:Ghost) RETURN n").unwrap(),
            &s,
            "g",
            &ProcedureRegistry::default(),
            &HashMap::new(),
        )
        .unwrap();
        let err = execute_gql(&plan, &s, "g").unwrap_err();
        assert!(matches!(err, GqlExecutionError::MandatoryMatchEmpty));
        assert!(format!("{err}").contains("mandatory"));
    }

    #[test]
    fn execute_gql_engine_error_propagates_via_from() {
        // Trigger an engine error by querying a non-existent graph.
        let s = setup();
        let plan = plan_gql_with_context(
            &parse_gql("MATCH (n) RETURN n").unwrap(),
            &s,
            "g",
            &ProcedureRegistry::default(),
            &HashMap::new(),
        )
        .unwrap();
        let err = execute_gql(&plan, &s, "missing-graph").unwrap_err();
        assert!(matches!(err, GqlExecutionError::Engine(_)));
        // Display impl is forwarded from the inner engine error.
        let msg = format!("{err}");
        assert!(!msg.is_empty());
    }
}
