//! Gremlin executor: path-related step implementations.
//!
//! Decomposed into focused submodules:
//! - [`path_step`]: `path()` traversal reconstruction.
//! - [`tree`]: `tree()` aggregation.
//! - [`simple`]: `simplePath()` filter.
//! - [`cyclic`]: `cyclicPath()` filter.
//!
//! The shared [`QueryExecutor::collect_path_values`] helper lives in this
//! module because it is used by both `simple` and `cyclic`.

mod cyclic;
mod path_step;
mod simple;
mod tree;

use crate::ast::Identifier;
use crate::types::Value;

use crate::executor::engine::{QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    /// Collect path values from a row, optionally slicing between from/to labels.
    pub(super) fn collect_path_values(
        &self,
        path_aliases: &[Identifier],
        row: &Row,
        from_label: Option<&str>,
        to_label: Option<&str>,
    ) -> Vec<Value> {
        // Build full path with alias tags for from/to slicing.
        let full_path: Vec<(&str, Value)> =
            if let Some(Value::List(acc)) = row.get("__gremlin_path_acc") {
                let mut p: Vec<(&str, Value)> = acc
                    .iter()
                    .filter(|v| !matches!(v, Value::Null))
                    .map(|v| ("", v.clone()))
                    .collect();
                if let Some(last_alias) = path_aliases.last() {
                    if let Some(val) = row.get(last_alias.as_str()) {
                        if !matches!(val, Value::Null) {
                            p.push((last_alias.as_str(), val.clone()));
                        }
                    }
                }
                p
            } else {
                path_aliases
                    .iter()
                    .filter_map(|alias| {
                        row.get(alias.as_str())
                            .filter(|v| !matches!(v, Value::Null))
                            .map(|v| (alias.as_str(), v.clone()))
                    })
                    .collect()
            };

        if from_label.is_none() && to_label.is_none() {
            return full_path.into_iter().map(|(_, v)| v).collect();
        }

        // Find the index of a label in path_aliases. The label is an
        // internal alias (resolved at compile time), so direct comparison
        // against path_aliases entries works.
        let find_idx = |label: &str, from_end: bool| -> Option<usize> {
            if from_end {
                path_aliases.iter().rposition(|a| a.as_str() == label)
            } else {
                path_aliases.iter().position(|a| a.as_str() == label)
            }
        };

        let start = from_label.and_then(|l| find_idx(l, false)).unwrap_or(0);
        let end = to_label
            .and_then(|l| find_idx(l, true))
            .map(|i| i + 1)
            .unwrap_or(full_path.len());

        if start <= end && end <= full_path.len() {
            full_path[start..end]
                .iter()
                .map(|(_, v)| v.clone())
                .collect()
        } else {
            full_path.into_iter().map(|(_, v)| v).collect()
        }
    }
}
