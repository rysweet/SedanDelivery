//! Row-shaping operators: filter, project, distinct, sort, skip, limit.

use crate::ast::Expr;
use crate::error::GqlError;
use crate::planner::Operator;
use crate::types::Value;

use super::helpers::{
    cmp_values, eval_count_expr, expr_to_column_name, gremlin_cmp_values, inject_parameters,
    RowDedupSet,
};
use super::{ExecContext, QueryExecutor, Row};

/// Compute a dedup key for a value, treating Gremlin edge property Maps specially:
/// Edge property Maps (containing `~key`/`~value`/`~element` where element is an Edge)
/// are compared by `~key` + `~value` only, ignoring `~element` (the parent edge).
/// Vertex property Maps keep the `~element` in the comparison key since VertexProperty
/// equality considers the parent vertex.
fn property_aware_dedup_key(v: &Value) -> String {
    if let Value::Map(m) = v {
        if m.contains_key("~key") && m.contains_key("~value") {
            // Edge properties: compare by key+value only
            if let Some(Value::Edge(_)) = m.get("~element") {
                let key = m.get("~key").map(|v| v.canonical_key()).unwrap_or_default();
                let val = m
                    .get("~value")
                    .map(|v| v.canonical_key())
                    .unwrap_or_default();
                return format!("EdgeProp({key}, {val})");
            }
        }
    }
    v.canonical_key()
}

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_filter(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Filter { predicate, child } = op else {
            debug_assert!(false, "expected Filter operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Filter operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for row in input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, &row)?;
            // Use eval_with_storage so that property access on Node/Edge IDs
            // is resolved against the store rather than returning Null.
            let result = self.eval_with_storage(cx.graph, predicate, &row, &ctx)?;
            if result == Value::Bool(true) {
                out.push(row);
            }
        }
        Ok(out)
    }

    pub(crate) fn exec_project(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Project { items, child } = op else {
            debug_assert!(false, "expected Project operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Project operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        for row in &input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, row)?;
            let mut new_row = Row::new();
            for (expr, alias) in items {
                match self.eval_with_storage(cx.graph, expr, row, &ctx) {
                    Ok(val) => {
                        let col_name = match alias {
                            Some(id) => id.0.clone(),
                            None => expr_to_column_name(expr),
                        };
                        new_row.insert(col_name, val);
                    }
                    Err(e) => {
                        // In Gremlin mode, label_carry_items() may reference
                        // step-alias variables (v_N, e_N) that are not present
                        // in all row contexts (e.g., after multi-key select).
                        // Skip these carry-forward items silently rather than
                        // erroring — they are auxiliary and not critical.
                        if self.gremlin_mode {
                            if let Expr::Variable(id) = expr {
                                let name = id.0.as_str();
                                // loops() outside repeat context: default to 0
                                if name == "__gremlin_loop_count" {
                                    let col_name = match alias {
                                        Some(a) => a.0.clone(),
                                        None => name.to_string(),
                                    };
                                    new_row.insert(col_name, Value::Int(0));
                                    continue;
                                }
                                if alias.as_ref() == Some(id) {
                                    // Only skip step-alias carry-forward variables
                                    // (v_N, e_N patterns from Gremlin compiler).
                                    let is_step_alias = (name.starts_with("v_")
                                        || name.starts_with("e_")
                                        || name.starts_with("u_")
                                        || name.starts_with("grp_"))
                                        && name[2..].chars().all(|c| c.is_ascii_digit());
                                    if is_step_alias {
                                        continue;
                                    }
                                }
                            }
                        }
                        // If we already produced at least one row, treat
                        // this as a streaming error: return the partial
                        // rows and stash the error for the caller.
                        if !out.is_empty() {
                            *self.streaming_error.borrow_mut() = Some(e);
                            return Ok(out);
                        }
                        // No partial rows yet: propagate the error
                        // immediately (standard behaviour).
                        return Err(e);
                    }
                }
            }
            // In Gremlin mode, preserve the __sack column through
            // intermediate projections so sack() can read it later.
            if self.gremlin_mode {
                if let Some(v) = row.get("__sack") {
                    if !new_row.contains_key("__sack") {
                        new_row.insert("__sack".to_string(), v.clone());
                    }
                }
            }
            // Preserve query parameters through scope changes.
            inject_parameters(&mut new_row, &self.parameters);
            cx.budget.record_row();
            out.push(new_row);
        }
        Ok(out)
    }

    pub(crate) fn exec_distinct(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Distinct { child, by_key } = op else {
            debug_assert!(false, "expected Distinct operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Distinct operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        if let Some(key) = by_key {
            use std::collections::HashSet;
            let mut seen: HashSet<String> = HashSet::new();

            // Multi-key dedup (keys separated by \x1f)
            if key.contains('\x1f') {
                let labels: Vec<&str> = key.split('\x1f').collect();
                for row in input {
                    cx.budget.check_or_err()?;
                    let composite: String = labels
                        .iter()
                        .map(|l| row.get(*l).map(|v| v.canonical_key()).unwrap_or_default())
                        .collect::<Vec<_>>()
                        .join("\x1f");
                    if seen.insert(composite) {
                        out.push(row);
                    }
                }
            } else if let Some(rest) = key.strip_prefix("__dedup_prop__") {
                // Property-based dedup: extract property from the element.
                // For Node/Edge values where the property doesn't exist,
                // drop the traverser (non-productive by-modulation).
                // For other value types, use empty string as dedup key.
                let parts: Vec<&str> = rest.splitn(2, "__").collect();
                let prop_name = parts.first().copied().unwrap_or("");
                let alias = parts.get(1).copied().unwrap_or("");
                for row in input {
                    cx.budget.check_or_err()?;
                    if let Some(val) = row.get(alias) {
                        let extracted = self.extract_property_from_value(cx.graph, val, prop_name);
                        let is_element = matches!(val, Value::Node(_) | Value::Edge(_));
                        if extracted.is_none() && is_element {
                            // Property missing on node/edge → drop traverser
                            continue;
                        }
                        let dedup_val = extracted.map(|v| v.canonical_key()).unwrap_or_default();
                        if seen.insert(dedup_val) {
                            out.push(row);
                        }
                    }
                }
            } else {
                // Simple column-based dedup
                for row in input {
                    cx.budget.check_or_err()?;
                    let dedup_val = row
                        .get(key.as_str())
                        .map(property_aware_dedup_key)
                        .unwrap_or_default();
                    if seen.insert(dedup_val) {
                        out.push(row);
                    }
                }
            }
        } else {
            let mut seen = RowDedupSet::new();
            for row in input {
                cx.budget.check_or_err()?;
                if seen.insert(&row) {
                    out.push(row);
                }
            }
        }
        Ok(out)
    }

    /// Extract a property from a Value (node, edge, or map).
    fn extract_property_from_value(&self, graph: &str, val: &Value, prop: &str) -> Option<Value> {
        match val {
            Value::Node(nid) => {
                // Handle special Gremlin tokens
                if prop == "~label" {
                    return self.cached_get_node(graph, *nid).ok().flatten().map(|n| {
                        let label = n.labels.first().map(|l| l.as_str()).unwrap_or("");
                        Value::Str(Box::from(label))
                    });
                }
                if prop == "~id" {
                    return Some(Value::Int(nid.0 as i64));
                }
                self.cached_get_node(graph, *nid)
                    .ok()
                    .flatten()
                    .and_then(|n| n.properties.get(prop).cloned())
            }
            Value::Edge(eid) => {
                if prop == "~label" {
                    return self
                        .cached_get_edge(graph, *eid)
                        .ok()
                        .flatten()
                        .map(|e| Value::Str(Box::from(e.rel_type.as_str())));
                }
                if prop == "~id" {
                    return Some(Value::Int(eid.0 as i64));
                }
                self.cached_get_edge(graph, *eid)
                    .ok()
                    .flatten()
                    .and_then(|e| e.properties.get(prop).cloned())
            }
            Value::Map(m) => m.get(prop).cloned(),
            _ => None,
        }
    }

    pub(crate) fn exec_sort(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Sort {
            items,
            gremlin_order,
            child,
        } = op
        else {
            debug_assert!(false, "expected Sort operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Sort operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        cx.budget.check_or_err()?;

        // Evaluate all sort keys up front to avoid repeated store lookups.
        let mut keyed: Vec<(Vec<Value>, Row)> = input
            .into_iter()
            .map(|row| {
                let ctx = self.build_filter_ctx(cx.graph, &row)?;
                let keys: Result<Vec<Value>, GqlError> = items
                    .iter()
                    .map(|(expr, _)| self.eval_with_storage(cx.graph, expr, &row, &ctx))
                    .collect();
                Ok((keys?, row))
            })
            .collect::<Result<Vec<_>, GqlError>>()?;

        let use_gremlin = *gremlin_order;
        keyed.sort_by(|(ka, _), (kb, _)| {
            for i in 0..items.len() {
                let va = &ka[i];
                let vb = &kb[i];
                let asc = items[i].1;
                let ord = if use_gremlin {
                    gremlin_cmp_values(va, vb)
                } else {
                    cmp_values(va, vb)
                };
                if ord != std::cmp::Ordering::Equal {
                    return if asc { ord } else { ord.reverse() };
                }
            }
            std::cmp::Ordering::Equal
        });

        Ok(keyed.into_iter().map(|(_, row)| row).collect())
    }

    pub(crate) fn exec_limit(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Limit { count, child } = op else {
            debug_assert!(false, "expected Limit operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Limit operator".into(),
                },
            ));
        };
        let n = eval_count_expr(count, &self.parameters)?;

        // Early termination optimisation: tell leaf scan operators to stop
        // producing rows once we have enough. This is only safe when:
        // 1. The child subtree contains no mutation operators (Create,
        //    Delete, Set, Remove, Merge) — mutations must execute on ALL
        //    qualifying rows regardless of the result-set LIMIT.
        // 2. The child subtree is a "simple pipeline" — no row-multiplying
        //    operators (Expand, VarLenExpand, Unwind, Apply, LeftOuterApply,
        //    Union) between the Limit and the leaf scan. Multiplying
        //    operators break the 1:1 mapping between leaf rows produced
        //    and output rows, making a leaf-level cap unreliable.
        let can_early_terminate = Self::subtree_is_simple_pipeline(child);
        let prev_cap = if can_early_terminate {
            let needed = n.saturating_add(Self::child_skip_amount(child, &self.parameters));
            Some(cx.budget.set_row_cap(needed))
        } else {
            None
        };
        let input = self.exec_operator(cx, child)?;
        if let Some(prev) = prev_cap {
            cx.budget.restore_row_cap(prev);
        }

        cx.budget.check_or_err()?;
        Ok(input.into_iter().take(n).collect())
    }

    /// If `op` is a `Skip` operator, evaluate and return its skip count.
    /// Otherwise return 0. This is used by `exec_limit` to account for
    /// rows consumed by an immediate Skip child.
    fn child_skip_amount(op: &Operator, params: &super::Row) -> usize {
        if let Operator::Skip { count, .. } = op {
            eval_count_expr(count, params).unwrap_or(0)
        } else {
            0
        }
    }

    /// Returns `true` if `op` is a "simple pipeline" — a linear chain of
    /// 1:1 operators ending at a single leaf scan. Early termination is
    /// only safe when every row produced by the leaf scan maps to exactly
    /// one output row, so no row-multiplying (Expand, VarLenExpand, Unwind)
    /// or joining (Apply, LeftOuterApply, Union) operators are allowed,
    /// and no mutation operators (Create, Delete, Set, Remove, Merge, Call)
    /// are present.
    fn subtree_is_simple_pipeline(op: &Operator) -> bool {
        match op {
            // Leaf operators — pipeline terminus.
            Operator::SingleRow
            | Operator::NodeScan { .. }
            | Operator::IndexScan { .. }
            | Operator::NodeByIdSeek { .. } => true,

            // 1:1 pass-through operators that don't reduce rows — safe.
            Operator::Project { child, .. }
            | Operator::Limit { child, .. }
            | Operator::Skip { child, .. }
            | Operator::AssemblePath { child, .. }
            | Operator::ProduceResults { child, .. } => Self::subtree_is_simple_pipeline(child),

            // Filter may reject rows, so the leaf-level cap doesn't guarantee
            // enough output rows. Sort/Distinct/Aggregate need ALL input
            // rows to produce correct output. All of these are barriers.
            Operator::Filter { .. }
            | Operator::Sort { .. }
            | Operator::Distinct { .. }
            | Operator::Aggregate { .. } => false,

            // Row-multiplying, joining, or mutating operators — not safe.
            Operator::Expand { .. }
            | Operator::VarLenExpand { .. }
            | Operator::Unwind { .. }
            | Operator::ShortestPath { .. }
            | Operator::Union { .. }
            | Operator::Apply { .. }
            | Operator::LeftOuterApply { .. }
            | Operator::Create { .. }
            | Operator::Delete { .. }
            | Operator::Set { .. }
            | Operator::Remove { .. }
            | Operator::Merge { .. }
            | Operator::MergeRelationship { .. }
            | Operator::Call { .. }
            | Operator::ForEach { .. }
            | Operator::Repeat { .. }
            | Operator::Choose { .. }
            | Operator::Coalesce { .. }
            | Operator::GremlinSideEffect { .. }
            | Operator::GremlinAggregate { .. }
            | Operator::GremlinPath { .. }
            | Operator::GremlinMatch { .. }
            | Operator::Cap { .. }
            | Operator::ChooseMap { .. }
            | Operator::GremlinSelect { .. }
            | Operator::GremlinGroup { .. }
            | Operator::GremlinMapSlice { .. }
            | Operator::GremlinSimplePath { .. }
            | Operator::GremlinCyclicPath { .. }
            | Operator::GremlinIndex { .. }
            | Operator::GremlinProject { .. } => false,
        }
    }

    pub(crate) fn exec_skip(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Skip { count, child } = op else {
            debug_assert!(false, "expected Skip operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Skip operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        cx.budget.check_or_err()?;
        let n = eval_count_expr(count, &self.parameters)?;
        Ok(input.into_iter().skip(n).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{BinaryOp, Expr, Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use std::collections::HashMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn filter_keeps_matching() {
        let store = make_store();
        let mut p = HashMap::new();
        p.insert("x".to_string(), Value::Int(10));
        store.create_node("g", vec!["N".to_string()], p).unwrap();
        store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Filter {
            predicate: Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("n"))),
                    "x".into(),
                )),
                right: Box::new(Expr::Integer(5)),
            },
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn distinct_removes_duplicates() {
        let store = make_store();
        store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        store
            .create_node("g", vec!["N".to_string()], HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Distinct {
            child: Box::new(Operator::Project {
                items: vec![(Expr::Integer(1), Some(Identifier::new("x")))],
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
            }),
            by_key: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 1);
    }

    #[test]
    fn sort_ascending() {
        let store = make_store();
        for name in &["C", "A", "B"] {
            let mut p = HashMap::new();
            p.insert("name".to_string(), Value::Str((*name).into()));
            store.create_node("g", vec!["P".to_string()], p).unwrap();
        }
        let executor = QueryExecutor::new(&store);
        let plan = Operator::ProduceResults {
            columns: vec![(
                "n.name".to_string(),
                Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("n"))),
                    "name".into(),
                ),
            )],
            child: Box::new(Operator::Sort {
                items: vec![(
                    Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("n"))),
                        "name".into(),
                    ),
                    true,
                )],
                gremlin_order: false,
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("P")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows[0]["n.name"], Value::Str("A".into()));
        assert_eq!(result.rows[2]["n.name"], Value::Str("C".into()));
    }

    #[test]
    fn limit_and_skip() {
        let store = make_store();
        for _ in 0..5 {
            store.create_node("g", vec![], HashMap::new()).unwrap();
        }
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Limit {
            count: Expr::Integer(2),
            child: Box::new(Operator::Skip {
                count: Expr::Integer(1),
                child: Box::new(Operator::NodeScan {
                    label: None,
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2);
    }

    /// LIMIT over a bare NodeScan triggers early termination: on 1000 nodes,
    /// LIMIT 10 should NOT materialise all 1000 rows. Verify that the
    /// budget's leaf_rows_produced counter stays well below the total node
    /// count.
    #[test]
    fn limit_early_termination_avoids_full_scan() {
        let store = make_store();
        let total_nodes = 1000;
        for _ in 0..total_nodes {
            store
                .create_node("g", vec!["N".to_string()], HashMap::new())
                .unwrap();
        }
        let executor = QueryExecutor::new(&store);
        // Plan: Limit { 10, NodeScan }
        let plan = Operator::Limit {
            count: Expr::Integer(10),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 10, "LIMIT should return exactly 10 rows");
        // The total rows_produced should be far less than 1000.
        // With early termination, NodeScan produces only ~10 leaf rows.
        // budget.rows_produced() includes both leaf and intermediate rows,
        // but should still be much less than 1000.
        assert!(
            budget.rows_produced() < 100,
            "Expected early termination to avoid full scan; \
             rows_produced={} but total_nodes={}",
            budget.rows_produced(),
            total_nodes,
        );
    }

    /// LIMIT over Project(NodeScan) also triggers early termination.
    #[test]
    fn limit_over_project_early_termination() {
        let store = make_store();
        let total_nodes = 500;
        for i in 0..total_nodes {
            let mut p = HashMap::new();
            p.insert("i".to_string(), Value::Int(i));
            store.create_node("g", vec!["N".to_string()], p).unwrap();
        }
        let executor = QueryExecutor::new(&store);
        // Plan: Limit { 5, Project { items: [n], child: NodeScan } }
        let plan = Operator::Limit {
            count: Expr::Integer(5),
            child: Box::new(Operator::Project {
                items: vec![(
                    Expr::Variable(Identifier::new("n")),
                    Some(Identifier::new("n")),
                )],
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 5);
        assert!(
            budget.rows_produced() < 100,
            "Expected early termination through Project; rows_produced={}",
            budget.rows_produced(),
        );
    }

    /// LIMIT over Sort(NodeScan) does NOT early-terminate (sort needs all rows).
    #[test]
    fn limit_over_sort_no_early_termination() {
        let store = make_store();
        let total_nodes = 50;
        for i in 0..total_nodes {
            let mut p = HashMap::new();
            p.insert("i".to_string(), Value::Int(i));
            store.create_node("g", vec!["N".to_string()], p).unwrap();
        }
        let executor = QueryExecutor::new(&store);
        // Plan: Limit { 5, Sort { child: NodeScan } }
        let plan = Operator::Limit {
            count: Expr::Integer(5),
            child: Box::new(Operator::Sort {
                items: vec![(
                    Expr::Property(Box::new(Expr::Variable(Identifier::new("n"))), "i".into()),
                    true,
                )],
                gremlin_order: false,
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 5);
        // Sort needs all rows so early termination must NOT fire.
        // rows_produced >= total_nodes because NodeScan produces 50 rows.
        assert!(
            budget.rows_produced() >= total_nodes as usize,
            "Sort should prevent early termination; rows_produced={}",
            budget.rows_produced(),
        );
    }

    /// SKIP 3 LIMIT 2 with early termination: should produce exactly 2 rows
    /// and the scan should materialise only skip+limit=5 rows.
    #[test]
    fn skip_limit_early_termination() {
        let store = make_store();
        let total_nodes = 200;
        for _ in 0..total_nodes {
            store
                .create_node("g", vec!["N".to_string()], HashMap::new())
                .unwrap();
        }
        let executor = QueryExecutor::new(&store);
        // Plan: Limit { 2, Skip { 3, NodeScan } }
        let plan = Operator::Limit {
            count: Expr::Integer(2),
            child: Box::new(Operator::Skip {
                count: Expr::Integer(3),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 2, "SKIP 3 LIMIT 2 should return 2 rows");
        // With early termination, NodeScan should produce only 5 leaf rows
        // (skip=3 + limit=2). Budget rows_produced includes both leaf and
        // intermediate counts, but should be far less than 200.
        assert!(
            budget.rows_produced() < 50,
            "Expected early termination for SKIP+LIMIT; rows_produced={} but total_nodes={}",
            budget.rows_produced(),
            total_nodes,
        );
    }
}
