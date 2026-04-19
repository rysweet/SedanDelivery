//! `SET n = {map}` (replacement) and `SET n += {map}` (merge) implementations.

use indexmap::IndexMap;

use crate::ast::Expr;
use crate::ast::Identifier;
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use crate::executor::engine::helpers::undefined_var;
use crate::executor::engine::{MutationStats, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(super) fn apply_set_map(
        &self,
        graph: &str,
        row: &Row,
        alias: &Identifier,
        map: &Expr,
        merge: bool,
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
        let map_val = self.eval_with_storage(graph, map, row, &ctx)?;
        let props = match map_val {
            Value::Map(m) => m
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect::<IndexMap<String, Value>>(),
            Value::Null => IndexMap::new(),
            Value::Node(src_node_id) => {
                // SET a = b where b is a Node: copy all properties from b
                match self.store.get_node(graph, src_node_id)? {
                    Some(node_data) => node_data.properties,
                    None => IndexMap::new(),
                }
            }
            Value::Edge(src_edge_id) => {
                // SET r = r2 where r2 is a Relationship: copy all properties from r2
                match self.store.get_edge(graph, src_edge_id)? {
                    Some(edge_data) => edge_data.properties,
                    None => IndexMap::new(),
                }
            }
            other => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "Map for SET assignment".to_string(),
                    actual: other.type_name().to_string(),
                }));
            }
        };
        let non_null_keys: Vec<String> = props
            .iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, _)| k.clone())
            .collect();
        let (is_edge, raw_id) = match val {
            Value::Node(node_id) => {
                let id = *node_id;
                if merge {
                    self.store.set_node_properties(graph, id, props)?;
                } else {
                    self.store.replace_node_properties(graph, id, props)?;
                }
                (false, id.0)
            }
            Value::Edge(edge_id) => {
                let id = *edge_id;
                if merge {
                    self.store.set_edge_properties(graph, id, props)?;
                } else {
                    self.store.replace_edge_properties(graph, id, props)?;
                }
                (true, id.0)
            }
            other => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("Node or Relationship for variable '{}'", alias.0),
                    actual: other.type_name().to_string(),
                }));
            }
        };
        for k in non_null_keys {
            stats.props_seen.insert((is_edge, raw_id, k));
        }
        Ok(())
    }
}
