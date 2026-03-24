//! Utility functions for the query executor.

mod arithmetic;
mod comparison;
mod dedup;
mod expr_naming;

// Re-export the full public API so that all existing `use super::helpers::X`
// paths continue to work without modification.
pub(crate) use self::arithmetic::{eval_binary_values, value_to_expr};
pub use self::comparison::cmp_values;
pub use self::comparison::gremlin_cmp_values;
pub(crate) use self::dedup::row_key;
pub(crate) use self::dedup::RowDedupSet;
pub(crate) use self::expr_naming::expr_to_column_name;

use indexmap::IndexMap;
use std::collections::HashMap;

use crate::ast::{Direction, Expr};
use crate::error::{ExecutionError, GqlError};
use crate::executor::filter::eval_expr;
use crate::executor::filter::Row as FilterRow;
use crate::planner::Operator;
use crate::storage::{GraphStore, NodeId};
use crate::types::Value;

use super::Row;

/// Extract a `NodeId` from a `Value::Node(id)`, returning an error if the
/// value is not a node reference.
pub(crate) fn extract_node_id(val: &Value, var: &str) -> Result<NodeId, GqlError> {
    match val {
        Value::Node(id) => Ok(*id),
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("Node for variable '{var}'"),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Build an error for an undefined variable.
pub(crate) fn undefined_var(name: &str) -> GqlError {
    GqlError::Execution(ExecutionError::TypeMismatch {
        expected: format!("variable '{name}' in scope"),
        actual: "undefined".to_string(),
    })
}

pub(crate) fn get_adj_entries(
    store: &dyn GraphStore,
    graph: &str,
    node_id: NodeId,
    direction: &Direction,
    type_strs: &[&str],
) -> Result<Vec<crate::storage::AdjEntry>, GqlError> {
    let entries = match direction {
        Direction::Outgoing => store.out_edges_by_type(graph, node_id, type_strs)?,
        Direction::Incoming => store.in_edges_by_type(graph, node_id, type_strs)?,
        Direction::Undirected => {
            let mut out = store.out_edges_by_type(graph, node_id, type_strs)?;
            let in_ = store.in_edges_by_type(graph, node_id, type_strs)?;
            // Safety: deduplicate self-loop edges that appear in both out and in lists.
            let out_edge_ids: std::collections::HashSet<u64> =
                out.iter().map(|e| e.edge_id.0).collect();
            out.extend(
                in_.into_iter()
                    .filter(|e| !out_edge_ids.contains(&e.edge_id.0)),
            );
            out
        }
    };
    Ok(entries)
}

/// Discover the aliases (column names) that an operator tree would produce.
/// Used by LeftOuterApply to fill NULLs for the right side when it produces no rows.
pub(crate) fn discover_operator_aliases(op: &Operator) -> Vec<String> {
    let mut aliases = Vec::new();
    match op {
        Operator::NodeScan { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::IndexScan { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::NodeByIdSeek { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::Expand {
            rel_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            if let Some(ra) = rel_alias {
                aliases.push(ra.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::VarLenExpand {
            path_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            if let Some(pa) = path_alias {
                aliases.push(pa.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::AssemblePath {
            path_alias, child, ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            aliases.push(path_alias.0.clone());
        }
        Operator::Project { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::Aggregate { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::MergeRelationship {
            child, rel_alias, ..
        } => {
            aliases.extend(discover_operator_aliases(child));
            if let Some(ra) = rel_alias {
                aliases.push(ra.0.clone());
            }
        }
        Operator::Unwind { alias, child, .. } => {
            aliases.extend(discover_operator_aliases(child));
            aliases.push(alias.0.clone());
        }
        Operator::Apply { outer, inner } => {
            aliases.extend(discover_operator_aliases(outer));
            aliases.extend(discover_operator_aliases(inner));
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => {
            aliases.extend(discover_operator_aliases(preserved));
            aliases.extend(discover_operator_aliases(optional_pattern));
        }
        Operator::Create { patterns, child } => {
            if let Some(ch) = child {
                aliases.extend(discover_operator_aliases(ch));
            }
            for pattern in patterns {
                for (alias, _, _) in &pattern.nodes {
                    if let Some(a) = alias {
                        aliases.push(a.0.clone());
                    }
                }
                for (alias, _, _, _, _, _) in &pattern.edges {
                    if let Some(a) = alias {
                        aliases.push(a.0.clone());
                    }
                }
            }
        }
        Operator::Merge {
            child, node_alias, ..
        } => {
            if let Some(ch) = child {
                aliases.extend(discover_operator_aliases(ch));
            }
            if let Some(a) = node_alias {
                aliases.push(a.0.clone());
            }
        }
        Operator::Union { left, right, .. } => {
            aliases.extend(discover_operator_aliases(left));
            aliases.extend(discover_operator_aliases(right));
        }
        Operator::ShortestPath {
            path_alias,
            src_alias,
            dst_alias,
            ..
        } => {
            aliases.push(path_alias.0.clone());
            aliases.push(src_alias.0.clone());
            aliases.push(dst_alias.0.clone());
        }
        Operator::Call {
            yield_items, child, ..
        } => {
            if let Some(ch) = child {
                aliases.extend(discover_operator_aliases(ch));
            }
            for (name, alias) in yield_items {
                let col_name = alias.as_ref().unwrap_or(name);
                aliases.push(col_name.0.clone());
            }
        }
        Operator::ForEach { child, .. } => {
            // FOREACH variable is scoped — not visible after the block.
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::Repeat { child, .. }
        | Operator::Choose { child, .. }
        | Operator::ChooseMap { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::GremlinAggregate { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::Coalesce { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::GremlinPath { .. } => {
            aliases.push("path".to_string());
        }
        Operator::GremlinTree { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::GremlinMatch { child, .. } => {
            aliases.extend(discover_operator_aliases(child));
        }
        Operator::Cap { keys, .. } => {
            for key in keys {
                aliases.push(key.clone());
            }
        }
        Operator::SingleRow
        | Operator::ProduceResults { .. }
        | Operator::GremlinSelect { .. }
        | Operator::GremlinGroup { .. }
        | Operator::GremlinMapSlice { .. }
        | Operator::GremlinSimplePath { .. }
        | Operator::GremlinCyclicPath { .. }
        | Operator::GremlinIndex { .. }
        | Operator::GremlinProject { .. } => {}
    }
    aliases
}

/// Inject query parameters into a row.
///
/// Uses `entry().or_insert_with()` so that values already present in the row
/// (computed by earlier operators) are never overwritten by a parameter of the
/// same name.  On an empty row the behaviour is identical to plain `insert`.
pub(crate) fn inject_parameters(row: &mut Row, params: &HashMap<String, Value>) {
    for (k, v) in params {
        row.entry(k.clone()).or_insert_with(|| v.clone());
    }
}

/// Evaluate a `count` expression (LIMIT/SKIP argument) — must be a non-negative integer.
pub(crate) fn eval_count_expr(expr: &Expr, params: &Row) -> Result<usize, GqlError> {
    let ctx: FilterRow<'_> = params.iter().map(|(k, v)| (k.as_str(), v)).collect();
    match eval_expr(expr, &ctx)? {
        Value::Int(n) if n >= 0 => Ok(n as usize),
        Value::Int(n) => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "non-negative integer for LIMIT/SKIP".to_string(),
            actual: n.to_string(),
        })),
        other => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "integer for LIMIT/SKIP".to_string(),
            actual: other.type_name().to_string(),
        })),
    }
}

/// Evaluate a property map `[(key, Expr)]` into an `IndexMap<String, Value>`.
///
/// Supports parameter map expansion: when a property entry has an empty key (`""`)
/// and its value is an `Expr::Parameter`, the parameter is resolved and expected to
/// be a `Value::Map`. All entries from that map are expanded into the result.
pub(crate) fn eval_prop_map(
    graph: &str,
    executor: &super::QueryExecutor<'_>,
    pairs: &[(String, Expr)],
    row: &Row,
    ctx: &FilterRow<'_>,
) -> Result<IndexMap<String, Value>, GqlError> {
    let mut map = IndexMap::new();
    for (key, expr) in pairs {
        if key.is_empty() {
            // Sentinel: parameter map expansion.
            let val = executor.eval_with_storage(graph, expr, row, ctx)?;
            match val {
                Value::Map(entries) => {
                    for (k, v) in entries {
                        if !v.is_null() {
                            map.insert(k.to_string(), v);
                        }
                    }
                }
                Value::Null => {
                    // Null parameter means no properties — skip.
                }
                other => {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "Map for parameter property expansion".to_string(),
                        actual: other.type_name().to_string(),
                    }));
                }
            }
        } else {
            let val = executor.eval_with_storage(graph, expr, row, ctx)?;
            // Cypher: setting a property to null = removing it (don't store nulls)
            if !val.is_null() {
                map.insert(key.clone(), val);
            }
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Identifier, Label};

    #[test]
    fn extract_node_id_success() {
        assert_eq!(
            extract_node_id(&Value::Node(NodeId(42)), "n").unwrap(),
            NodeId(42)
        );
    }

    #[test]
    fn extract_node_id_wrong_type() {
        assert!(extract_node_id(&Value::Int(1), "n").is_err());
    }

    #[test]
    fn inject_params_no_overwrite() {
        let mut row = Row::new();
        row.insert("$x".to_string(), Value::Int(1));
        let mut params = HashMap::new();
        params.insert("$x".to_string(), Value::Int(999));
        params.insert("$y".to_string(), Value::Int(2));
        inject_parameters(&mut row, &params);
        assert_eq!(row.get("$x"), Some(&Value::Int(1)));
        assert_eq!(row.get("$y"), Some(&Value::Int(2)));
    }

    #[test]
    fn eval_count_expr_valid() {
        assert_eq!(eval_count_expr(&Expr::Integer(5), &Row::new()).unwrap(), 5);
    }

    #[test]
    fn eval_count_expr_negative() {
        assert!(eval_count_expr(&Expr::Integer(-1), &Row::new()).is_err());
    }

    #[test]
    fn discover_aliases_node_scan() {
        let op = Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        };
        assert_eq!(discover_operator_aliases(&op), vec!["n".to_string()]);
    }

    #[test]
    fn get_adj_entries_undirected_dedup_self_loop() {
        use crate::storage::InMemoryGraphStore;

        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        let n = store
            .create_node(
                "g",
                vec!["A".to_string()],
                std::collections::IndexMap::new(),
            )
            .unwrap();
        // Create a self-loop: (n)-[:SELF]->(n)
        let eid = store
            .create_edge("g", n, n, "SELF".to_string(), indexmap::IndexMap::new())
            .unwrap();

        let entries = get_adj_entries(&store, "g", n, &Direction::Undirected, &[]).unwrap();

        // The self-loop edge should appear exactly once, not twice.
        let matching: Vec<_> = entries.iter().filter(|e| e.edge_id == eid).collect();
        assert_eq!(
            matching.len(),
            1,
            "self-loop should appear once in undirected expand, got {}",
            matching.len()
        );
    }
}
