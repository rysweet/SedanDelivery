//! GQL query planner.
//!
//! Delegates to cypher-core's [`QueryPlanner`] for the shared operator tree,
//! then applies GQL-specific plan adjustments:
//!
//! - **Mandatory match**: wraps the plan with a runtime check that errors if
//!   the MATCH produced zero rows.
//! - **Path mode enforcement**: inserts filters for TRAIL (no repeated edges),
//!   SIMPLE (no repeated nodes except endpoints), ACYCLIC (no repeated nodes).
//! - **Isomorphism**: GQL defaults to isomorphic matching (no two pattern
//!   variables can bind to the same node), unlike openCypher's homomorphism.
//!   This is a future enforcement — noted here for architecture.

use std::collections::HashMap;

use cypher_core::error::GqlError;
use cypher_core::planner::{Operator, QueryPlanner};
use cypher_core::procedure::ProcedureRegistry;
use cypher_core::storage::GraphStore;
use cypher_core::types::Value;

use crate::parser::{GqlQuery, PathMode};

/// Plan a GQL query into a physical [`Operator`] tree.
///
/// This is the main entry point for GQL query planning. It:
/// 1. Delegates to cypher-core's planner for the shared operator tree.
/// 2. Applies GQL-specific plan adjustments based on [`GqlMetadata`].
pub fn plan_gql(
    gql_query: &GqlQuery,
    store: &dyn GraphStore,
    graph: &str,
) -> Result<GqlPlan, GqlError> {
    plan_gql_with_context(
        gql_query,
        store,
        graph,
        &ProcedureRegistry::default(),
        &HashMap::new(),
    )
}

/// Plan a GQL query with procedure registry and parameters.
pub fn plan_gql_with_context(
    gql_query: &GqlQuery,
    store: &dyn GraphStore,
    graph: &str,
    registry: &ProcedureRegistry,
    parameters: &HashMap<String, Value>,
) -> Result<GqlPlan, GqlError> {
    let planner = QueryPlanner::with_context(store, graph, registry, parameters);
    let operator = planner.plan(&gql_query.query)?;

    Ok(GqlPlan {
        operator,
        mandatory_match: gql_query.metadata.mandatory_match,
        path_mode: gql_query.metadata.path_mode,
    })
}

/// A planned GQL query: the physical operator tree plus GQL-specific flags.
#[derive(Debug)]
pub struct GqlPlan {
    /// The physical operator tree (shared with openCypher).
    pub operator: Operator,
    /// Whether MANDATORY MATCH semantics apply.
    pub mandatory_match: bool,
    /// Path mode restriction if any.
    pub path_mode: Option<PathMode>,
}
