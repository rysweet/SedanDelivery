//! Shared MERGE property evaluation + canonical-key helpers.

use indexmap::IndexMap;

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::executor::engine::{QueryExecutor, Row};
use crate::executor::filter::Row as FilterRow;
use crate::types::Value;

impl<'a> QueryExecutor<'a> {
    pub(super) fn merge_node_props_key(properties: &IndexMap<String, Value>) -> String {
        let canonical_props = properties
            .iter()
            .map(|(k, v)| (k.clone().into_boxed_str(), v.clone()))
            .collect();
        Value::Map(canonical_props).canonical_key()
    }

    pub(super) fn merge_relationship_props_key(properties: &IndexMap<String, Value>) -> String {
        let canonical_props = properties
            .iter()
            .map(|(k, v)| (k.clone().into_boxed_str(), v.clone()))
            .collect();
        Value::Map(canonical_props).canonical_key()
    }

    pub(super) fn merge_relationship_props_map(
        prop_vals: Vec<(String, Value)>,
    ) -> IndexMap<String, Value> {
        prop_vals.into_iter().collect()
    }
    pub(crate) fn eval_merge_properties(
        &self,
        graph: &str,
        properties: &[(String, Expr)],
        row: &Row,
        ctx: &FilterRow<'_>,
    ) -> Result<Vec<(String, Value)>, GqlError> {
        let mut prop_vals: Vec<(String, Value)> = Vec::new();
        for (k, expr) in properties {
            if k.is_empty() {
                // Parameter map expansion sentinel.
                let val = self.eval_with_storage(graph, expr, row, ctx)?;
                match val {
                    Value::Map(entries) => {
                        for (mk, mv) in entries {
                            if mv.is_null() {
                                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                    expected: "non-null property value in MERGE".to_string(),
                                    actual: "Null".to_string(),
                                }));
                            }
                            prop_vals.push((mk.to_string(), mv));
                        }
                    }
                    Value::Null => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "non-null Map for MERGE parameter properties".to_string(),
                            actual: "Null".to_string(),
                        }));
                    }
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "Map for MERGE parameter properties".to_string(),
                            actual: other.type_name().to_string(),
                        }));
                    }
                }
            } else {
                let val = self.eval_with_storage(graph, expr, row, ctx)?;
                // MERGE with null property is a SemanticError (MergeReadOwnWrites)
                if val.is_null() {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "non-null property value in MERGE".to_string(),
                        actual: "Null".to_string(),
                    }));
                }
                prop_vals.push((k.clone(), val));
            }
        }
        Ok(prop_vals)
    }
}
