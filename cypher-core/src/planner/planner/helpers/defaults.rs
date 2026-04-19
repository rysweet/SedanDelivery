//! Static defaults and shared type aliases used across planner helpers.

use std::collections::HashMap;

use crate::ast::Identifier;
use crate::procedure::ProcedureRegistry;
use crate::types::Value;

/// Named path binding: `(path_alias, [node_aliases], [edge_aliases])`.
/// Used during CREATE planning to wrap the result with `AssemblePath`.
pub(crate) type PathBinding = (Identifier, Vec<Identifier>, Vec<Identifier>);

static EMPTY_REGISTRY: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
static EMPTY_PARAMS: std::sync::OnceLock<HashMap<String, Value>> = std::sync::OnceLock::new();

pub(crate) fn empty_registry() -> &'static ProcedureRegistry {
    EMPTY_REGISTRY.get_or_init(ProcedureRegistry::new)
}

pub(crate) fn empty_params() -> &'static HashMap<String, Value> {
    EMPTY_PARAMS.get_or_init(HashMap::new)
}
