//! `SET n.prop = value` and `REMOVE n.prop` implementations.

use indexmap::IndexMap;

use crate::ast::Expr;
use crate::ast::Identifier;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use crate::executor::engine::helpers::undefined_var;
use crate::executor::engine::{MutationStats, QueryExecutor, Row};

/// Validate that `v` is a permissible property value.
///
/// Lists-of-maps are rejected, but standalone Maps are permitted (needed for
/// Gremlin `@AllowMapPropertyValues`). In Gremlin mode, a list of single-value
/// maps is treated as a multi-property bag and accepted.
fn is_invalid_prop_value(v: &Value, gremlin_mode: bool) -> bool {
    match v {
        Value::List(items) => {
            if gremlin_mode
                && !items.is_empty()
                && items
                    .iter()
                    .all(|item| matches!(item, Value::Map(map) if map.contains_key("value")))
            {
                false
            } else {
                items.iter().any(|item| matches!(item, Value::Map(_)))
            }
        }
        _ => false,
    }
}

impl<'a> QueryExecutor<'a> {
    pub(super) fn apply_set_property(
        &self,
        graph: &str,
        row: &Row,
        alias: &Identifier,
        key: &str,
        value: &Expr,
        stats: &mut MutationStats,
    ) -> Result<(), GqlError> {
        let val = row
            .get(alias.0.as_str())
            .ok_or_else(|| undefined_var(&alias.0))?;
        // SET on a null variable (from OPTIONAL MATCH) is silently ignored.
        if val.is_null() {
            return Ok(());
        }
        let ctx = self.build_filter_ctx(graph, row)?;
        let prop_val = self.eval_with_storage(graph, value, row, &ctx)?;
        let prop_val = if self.gremlin_mode {
            match prop_val {
                Value::Map(map) if map.contains_key("value") => Value::List(vec![Value::Map(map)]),
                other => other,
            }
        } else {
            prop_val
        };
        // SET n.prop = null means REMOVE the property (Cypher semantics)
        if prop_val.is_null() {
            match val {
                Value::Node(node_id) => {
                    self.store.remove_node_property(graph, *node_id, key)?;
                }
                Value::Edge(edge_id) => {
                    self.store.remove_edge_property(graph, *edge_id, key)?;
                }
                _ => {
                    tracing::debug!("SET property on non-node/non-edge value, ignoring");
                }
            }
            return Ok(());
        }
        if is_invalid_prop_value(&prop_val, self.gremlin_mode) {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "primitive property value (not Map or List of Maps)".to_string(),
                actual: prop_val.type_name().to_string(),
            }));
        }
        let mut props = IndexMap::new();
        props.insert(key.to_string(), prop_val);
        match val {
            Value::Node(node_id) => {
                self.store.set_node_properties(graph, *node_id, props)?;
                stats.props_seen.insert((false, node_id.0, key.to_string()));
            }
            Value::Edge(edge_id) => {
                self.store.set_edge_properties(graph, *edge_id, props)?;
                stats.props_seen.insert((true, edge_id.0, key.to_string()));
            }
            other => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("Node or Relationship for variable '{}'", alias.0),
                    actual: other.type_name().to_string(),
                }));
            }
        }
        Ok(())
    }

    pub(super) fn apply_remove_property(
        &self,
        graph: &str,
        row: &Row,
        alias: &Identifier,
        key: &str,
    ) -> Result<(), GqlError> {
        let val = row
            .get(alias.0.as_str())
            .ok_or_else(|| undefined_var(&alias.0))?;
        match val {
            Value::Null => {
                // REMOVE on null (from OPTIONAL MATCH) is a no-op
            }
            Value::Node(node_id) => {
                self.store.remove_node_property(graph, *node_id, key)?;
            }
            Value::Edge(edge_id) => {
                self.store.remove_edge_property(graph, *edge_id, key)?;
            }
            other => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Node or Relationship".to_string(),
                    actual: other.type_name().to_string(),
                }));
            }
        }
        // REMOVE property does not count as properties_set.
        Ok(())
    }
}
