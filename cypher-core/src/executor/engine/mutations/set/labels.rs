//! `SET n:Label`, `SET n:$(expr)`, and `REMOVE n:Label` implementations.

use crate::ast::Expr;
use crate::ast::{Identifier, Label};
use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use crate::executor::engine::helpers::{extract_node_id, undefined_var};
use crate::executor::engine::{MutationStats, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(super) fn apply_set_labels(
        &self,
        graph: &str,
        row: &Row,
        alias: &Identifier,
        labels: &[Label],
        stats: &mut MutationStats,
    ) -> Result<(), GqlError> {
        let val = row
            .get(alias.0.as_str())
            .ok_or_else(|| undefined_var(&alias.0))?;
        // SET labels on null (from OPTIONAL MATCH) is a no-op
        if val.is_null() {
            return Ok(());
        }
        let node_id = extract_node_id(val, &alias.0)?;
        let label_strings: Vec<String> = labels.iter().map(|l| l.as_str().to_string()).collect();
        self.store.add_node_labels(graph, node_id, &label_strings)?;
        stats.labels_seen.extend(label_strings);
        Ok(())
    }

    pub(super) fn apply_set_dynamic_labels(
        &self,
        graph: &str,
        row: &Row,
        alias: &Identifier,
        value: &Expr,
        stats: &mut MutationStats,
    ) -> Result<(), GqlError> {
        let val = row
            .get(alias.0.as_str())
            .ok_or_else(|| undefined_var(&alias.0))?;
        if val.is_null() {
            return Ok(());
        }
        let node_id = extract_node_id(val, &alias.0)?;
        let ctx = self.build_filter_ctx(graph, row)?;
        let label_val = self.eval_with_storage(graph, value, row, &ctx)?;
        let label_strings: Vec<String> = match label_val {
            Value::Null => Vec::new(),
            Value::Str(label) => vec![label.into_string()],
            Value::List(items) => {
                let mut labels = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        Value::Str(label) => labels.push(label.into_string()),
                        other => {
                            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                                expected: "String or List<String> for dynamic labels".to_string(),
                                actual: other.type_name().to_string(),
                            }));
                        }
                    }
                }
                labels
            }
            other => {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "String or List<String> for dynamic labels".to_string(),
                    actual: other.type_name().to_string(),
                }));
            }
        };
        self.store.add_node_labels(graph, node_id, &label_strings)?;
        stats.labels_seen.extend(label_strings);
        Ok(())
    }

    pub(super) fn apply_remove_labels(
        &self,
        graph: &str,
        row: &Row,
        alias: &Identifier,
        labels: &[Label],
        stats: &mut MutationStats,
    ) -> Result<(), GqlError> {
        let val = row
            .get(alias.0.as_str())
            .ok_or_else(|| undefined_var(&alias.0))?;
        // REMOVE labels on null (from OPTIONAL MATCH) is a no-op
        if val.is_null() {
            return Ok(());
        }
        let node_id = extract_node_id(val, &alias.0)?;
        let label_strings: Vec<String> = labels.iter().map(|l| l.as_str().to_string()).collect();
        let removed_count = label_strings.len() as u64;
        self.store
            .remove_node_labels(graph, node_id, &label_strings)?;
        stats.labels_removed += removed_count;
        Ok(())
    }
}
