//! Visible-alias collection for operator scope analysis.

use crate::ast::Expr;
use crate::planner::operators::Operator;

/// Collect user-visible variable aliases from an operator tree.
///
/// Returns aliases that would be available in the row at the output of this operator.
/// Filters out internal aliases (those starting with `__`).
pub fn collect_visible_aliases(op: &Operator) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for alias in collect_all_aliases(op) {
        if !alias.starts_with("__") && seen.insert(alias.clone()) {
            result.push(alias);
        }
    }
    result
}

fn collect_all_aliases(op: &Operator) -> Vec<String> {
    let mut aliases = Vec::new();
    match op {
        Operator::NodeScan { alias, .. }
        | Operator::IndexScan { alias, .. }
        | Operator::NodeByIdSeek { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::Expand {
            rel_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(collect_all_aliases(child));
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
            aliases.extend(collect_all_aliases(child));
            if let Some(pa) = path_alias {
                aliases.push(pa.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::AssemblePath {
            path_alias, child, ..
        } => {
            aliases.extend(collect_all_aliases(child));
            aliases.push(path_alias.0.clone());
        }
        Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::Unwind { alias, child, .. } => {
            aliases.extend(collect_all_aliases(child));
            aliases.push(alias.0.clone());
        }
        Operator::Apply { outer, inner } => {
            aliases.extend(collect_all_aliases(outer));
            aliases.extend(collect_all_aliases(inner));
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => {
            aliases.extend(collect_all_aliases(preserved));
            aliases.extend(collect_all_aliases(optional_pattern));
        }
        Operator::Create { patterns, child } => {
            if let Some(ch) = child {
                aliases.extend(collect_all_aliases(ch));
            }
            // Extract aliases from CREATE patterns (newly created nodes/edges).
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
                aliases.extend(collect_all_aliases(ch));
            }
            if let Some(a) = node_alias {
                aliases.push(a.0.clone());
            }
        }
        Operator::MergeRelationship {
            child, rel_alias, ..
        } => {
            aliases.extend(collect_all_aliases(child));
            if let Some(a) = rel_alias {
                aliases.push(a.0.clone());
            }
        }
        Operator::Project { items, .. } => {
            // After a Project (WITH), only the projected items are visible.
            // Child aliases are hidden (WITH introduces a new scope).
            for (expr, alias) in items {
                if let Some(a) = alias {
                    aliases.push(a.0.clone());
                } else {
                    // Derive column name from expression.
                    match expr {
                        Expr::Variable(id) => aliases.push(id.0.clone()),
                        Expr::Property(obj, key) => {
                            if let Expr::Variable(id) = obj.as_ref() {
                                aliases.push(format!("{}.{}", id.0, key));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Operator::Aggregate {
            keys, aggregations, ..
        } => {
            // After Aggregate, only the key and aggregation aliases are visible.
            for key in keys {
                if let Expr::Variable(id) = key {
                    aliases.push(id.0.clone());
                }
            }
            for (_, _, _, _, alias) in aggregations {
                aliases.push(alias.0.clone());
            }
        }
        Operator::Call {
            yield_items, child, ..
        } => {
            if let Some(ch) = child {
                aliases.extend(collect_all_aliases(ch));
            }
            for (name, alias) in yield_items {
                let col_name = alias.as_ref().unwrap_or(name);
                aliases.push(col_name.0.clone());
            }
        }
        Operator::Union { left, right, .. } => {
            aliases.extend(collect_all_aliases(left));
            aliases.extend(collect_all_aliases(right));
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
        Operator::ForEach { child, .. } => {
            // FOREACH variable is scoped — not visible after the block.
            // Only propagate aliases from the child pipeline.
            aliases.extend(collect_all_aliases(child));
        }
        Operator::Repeat { child, .. }
        | Operator::Choose { child, .. }
        | Operator::ChooseMap { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::GremlinAggregate { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::Coalesce { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinPath { .. } => {
            aliases.push("path".to_string());
        }
        Operator::GremlinTree { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::GremlinMatch { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinSelect { keys, .. } => {
            for (_, output_key) in keys {
                aliases.push(output_key.clone());
            }
        }
        Operator::GremlinGroup { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::GremlinSample { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinMapSlice { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinSimplePath { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinCyclicPath { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinIndex { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinProject { entries, .. } => {
            for (key, _, _, _) in entries {
                aliases.push(key.clone());
            }
        }
        Operator::Cap { keys, .. } => {
            for key in keys {
                aliases.push(key.clone());
            }
        }
        Operator::GremlinBarrier { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::SingleRow | Operator::ProduceResults { .. } => {}
    }
    aliases
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Identifier, Label};
    use crate::planner::operators::Operator;

    fn id(s: &str) -> Identifier {
        Identifier::new(s)
    }

    fn ns(name: &str) -> Operator {
        Operator::NodeScan {
            label: Some(Label::new("L")),
            alias: id(name),
        }
    }

    #[test]
    fn collect_visible_filters_internal_aliases() {
        // Build NodeScan(__hidden) under a Filter; should not surface.
        let op = Operator::Filter {
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: id("__hidden"),
            }),
            predicate: Expr::Bool(true),
        };
        let v = collect_visible_aliases(&op);
        assert!(!v.contains(&"__hidden".to_string()));
    }

    #[test]
    fn collect_visible_dedups_repeated_aliases() {
        // Apply combining the same alias on both sides → one occurrence after dedup.
        let op = Operator::Apply {
            outer: Box::new(ns("n")),
            inner: Box::new(ns("n")),
        };
        assert_eq!(collect_visible_aliases(&op), vec!["n".to_string()]);
    }

    #[test]
    fn collect_all_node_scan_returns_alias() {
        assert_eq!(collect_visible_aliases(&ns("n")), vec!["n".to_string()]);
    }

    #[test]
    fn collect_all_unwind_appends_loop_var() {
        let op = Operator::Unwind {
            child: Box::new(ns("n")),
            list_expr: Expr::List(vec![Expr::Integer(1)]),
            alias: id("x"),
        };
        let v = collect_visible_aliases(&op);
        assert!(v.contains(&"n".to_string()));
        assert!(v.contains(&"x".to_string()));
    }

    #[test]
    fn collect_all_distinct_passes_through() {
        let op = Operator::Distinct {
            child: Box::new(ns("n")),
            by_key: None,
            productive: true,
        };
        assert_eq!(collect_visible_aliases(&op), vec!["n".to_string()]);
    }

    #[test]
    fn collect_all_singlerow_yields_empty() {
        assert!(collect_visible_aliases(&Operator::SingleRow).is_empty());
    }

    #[test]
    fn collect_all_cap_appends_keys() {
        let op = Operator::Cap {
            child: Box::new(ns("n")),
            keys: vec!["a".to_string(), "b".to_string()],
            reductions: Vec::new(),
        };
        let v = collect_visible_aliases(&op);
        assert!(v.contains(&"a".to_string()));
        assert!(v.contains(&"b".to_string()));
    }

    // ── Extended coverage for the remaining Operator variants ──────────────

    use crate::ast::{Direction, RelType};
    use crate::planner::operators::aggregation::AggFunction;
    use crate::planner::operators::Operator as Op;

    fn boxed(op: Op) -> Box<Op> {
        Box::new(op)
    }

    #[test]
    fn project_uses_alias_or_derived_name() {
        let op = Op::Project {
            items: vec![
                (Expr::Variable(id("n")), None),
                (Expr::Integer(1), Some(id("one"))),
                (
                    Expr::Property(Box::new(Expr::Variable(id("n"))), "name".into()),
                    None,
                ),
                (Expr::Integer(0), None), // no alias and no derivable name
            ],
            child: boxed(ns("__hidden")),
        };
        let v = collect_visible_aliases(&op);
        assert!(v.contains(&"n".to_string()));
        assert!(v.contains(&"one".to_string()));
        assert!(v.contains(&"n.name".to_string()));
        // child alias does NOT propagate through Project (new scope)
        assert!(!v.iter().any(|a| a == "__hidden"));
    }

    #[test]
    fn aggregate_exposes_keys_and_aggregations() {
        let op = Op::Aggregate {
            keys: vec![Expr::Variable(id("k")), Expr::Integer(1)],
            aggregations: vec![(
                AggFunction::Count,
                Expr::Variable(id("k")),
                None,
                false,
                id("cnt"),
            )],
            child: boxed(ns("n")),
        };
        let v = collect_visible_aliases(&op);
        assert!(v.contains(&"k".to_string()));
        assert!(v.contains(&"cnt".to_string()));
    }

    #[test]
    fn call_yields_with_and_without_alias() {
        let op = Op::Call {
            procedure: "p".into(),
            args: vec![],
            yield_items: vec![(id("a"), None), (id("b"), Some(id("bb")))],
            yield_star: false,
            standalone: false,
            child: Some(boxed(ns("seed"))),
        };
        let v = collect_visible_aliases(&op);
        assert!(v.contains(&"seed".to_string()));
        assert!(v.contains(&"a".to_string()));
        assert!(v.contains(&"bb".to_string()));
    }

    #[test]
    fn shortest_path_exposes_path_and_endpoints() {
        let op = Op::ShortestPath {
            src_alias: id("a"),
            dst_alias: id("b"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 5,
            path_alias: id("p"),
            all_paths: false,
        };
        let v = collect_visible_aliases(&op);
        assert!(v.contains(&"a".to_string()));
        assert!(v.contains(&"b".to_string()));
        assert!(v.contains(&"p".to_string()));
    }

    #[test]
    fn union_combines_visible_from_both_sides() {
        let op = Op::Union {
            left: boxed(ns("a")),
            right: boxed(ns("b")),
            all: true,
        };
        let v = collect_visible_aliases(&op);
        assert!(v.contains(&"a".to_string()));
        assert!(v.contains(&"b".to_string()));
    }

    #[test]
    fn create_appends_named_pattern_aliases() {
        use crate::planner::operators::mutation::CreatePattern;
        let op = Op::Create {
            patterns: vec![CreatePattern {
                nodes: vec![(Some(id("n")), vec![Label::new("L")], vec![])],
                edges: vec![(
                    Some(id("r")),
                    id("n"),
                    id("n"),
                    RelType::new("R"),
                    Direction::Outgoing,
                    vec![],
                )],
            }],
            child: Some(boxed(ns("seed"))),
        };
        let v = collect_visible_aliases(&op);
        assert!(v.contains(&"seed".to_string()));
        assert!(v.contains(&"n".to_string()));
        assert!(v.contains(&"r".to_string()));
    }

    #[test]
    fn merge_and_merge_relationship_expose_aliases() {
        let m = Op::Merge {
            node_alias: Some(id("m")),
            labels: vec![],
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: Some(boxed(ns("seed"))),
        };
        let v = collect_visible_aliases(&m);
        assert!(v.contains(&"m".to_string()));
        assert!(v.contains(&"seed".to_string()));

        let mr = Op::MergeRelationship {
            src_alias: id("s"),
            dst_alias: id("d"),
            rel_alias: Some(id("r")),
            rel_type: RelType::new("R"),
            direction: Direction::Outgoing,
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: boxed(ns("seed")),
        };
        let v = collect_visible_aliases(&mr);
        assert!(v.contains(&"r".to_string()));
        assert!(v.contains(&"seed".to_string()));
    }

    #[test]
    fn foreach_does_not_leak_loop_var() {
        let op = Op::ForEach {
            variable: id("loopvar"),
            list_expr: Expr::List(vec![]),
            body: boxed(ns("body")),
            child: boxed(ns("seed")),
        };
        let v = collect_visible_aliases(&op);
        assert!(v.contains(&"seed".to_string()));
        assert!(!v.iter().any(|a| a == "loopvar"));
    }

    #[test]
    fn repeat_choose_choosemap_passthrough() {
        let r = Op::Repeat {
            body: boxed(ns("body")),
            times: None,
            until: None,
            emit: false,
            emit_before: false,
            emit_predicate: None,
            remap: None,
            times_before: false,
            loop_label: None,
            body_path_aliases: vec![],
            pop_origins: vec![],
            needs_path: false,
            child: boxed(ns("seed")),
        };
        assert!(collect_visible_aliases(&r).contains(&"seed".to_string()));
        let c = Op::Choose {
            predicate: boxed(ns("p")),
            if_true: boxed(ns("t")),
            if_false: None,
            child: boxed(ns("seed")),
            true_alias: None,
            false_alias: None,
            output_alias: None,
        };
        assert!(collect_visible_aliases(&c).contains(&"seed".to_string()));
        let cm = Op::ChooseMap {
            selector: boxed(ns("s")),
            selector_alias: id("s"),
            options: vec![],
            child: boxed(ns("seed")),
            output_alias: None,
        };
        assert!(collect_visible_aliases(&cm).contains(&"seed".to_string()));
    }

    #[test]
    fn coalesce_passes_through_child() {
        let op = Op::Coalesce {
            alternatives: vec![ns("alt")],
            child: boxed(ns("seed")),
        };
        let v = collect_visible_aliases(&op);
        assert!(v.contains(&"seed".to_string()));
    }

    #[test]
    fn gremlin_path_yields_path_alias() {
        let op = Op::GremlinPath {
            child: boxed(ns("seed")),
            path_aliases: vec![],
            by_keys: vec![],
            productive: false,
            from_label: None,
            to_label: None,
        };
        assert_eq!(collect_visible_aliases(&op), vec!["path".to_string()]);
    }

    #[test]
    fn gremlin_tree_match_select_group_project_sample_etc() {
        use crate::planner::operators::gremlin_types::GremlinSelectTraversal;
        let t = Op::GremlinTree {
            child: boxed(ns("seed")),
            path_aliases: vec![],
            side_effect_key: None,
            alias: id("tree"),
        };
        assert!(collect_visible_aliases(&t).contains(&"tree".to_string()));

        let m = Op::GremlinMatch {
            patterns: vec![],
            pattern_labels: vec![],
            label_map: vec![],
            result_alias: id("r"),
            child: boxed(ns("seed")),
        };
        assert!(collect_visible_aliases(&m).contains(&"seed".to_string()));

        let sel = Op::GremlinSelect {
            keys: vec![(id("a"), "outA".to_string()), (id("b"), "outB".to_string())],
            by_traversals: vec![
                None,
                Some(GremlinSelectTraversal::Traversal {
                    op: boxed(ns("byT")),
                    out_alias: id("byT"),
                }),
            ],
            productive: false,
            pop: None,
            label_stacks: vec![],
            child: boxed(ns("seed")),
        };
        let v = collect_visible_aliases(&sel);
        assert!(v.contains(&"outA".to_string()));
        assert!(v.contains(&"outB".to_string()));

        let g = Op::GremlinGroup {
            key_expr: Expr::Null,
            value_expr: Expr::Null,
            agg_fn: AggFunction::Count,
            alias: id("groupa"),
            side_effect_key: None,
            key_traversal: None,
            value_traversal: None,
            productive: false,
            key_is_identity: false,
            explicit_fold: false,
            child: boxed(ns("seed")),
        };
        assert!(collect_visible_aliases(&g).contains(&"groupa".to_string()));

        let p = Op::GremlinProject {
            entries: vec![
                ("k1".to_string(), ns("a"), id("a"), false),
                ("k2".to_string(), ns("b"), id("b"), false),
            ],
            child: boxed(ns("seed")),
            current_alias: id("cur"),
            out_alias: id("o"),
            productive: false,
        };
        let v = collect_visible_aliases(&p);
        assert!(v.contains(&"k1".to_string()));
        assert!(v.contains(&"k2".to_string()));

        let sample = Op::GremlinSample {
            count: 1,
            weight: None,
            seed: None,
            child: boxed(ns("seed")),
        };
        assert!(collect_visible_aliases(&sample).contains(&"seed".to_string()));

        let ms = Op::GremlinMapSlice {
            alias: id("a"),
            lo: 0,
            hi: 1,
            child: boxed(ns("seed")),
        };
        assert!(collect_visible_aliases(&ms).contains(&"seed".to_string()));

        let sp = Op::GremlinSimplePath {
            child: boxed(ns("seed")),
            path_aliases: vec![],
            by_keys: vec![],
            from_label: None,
            to_label: None,
        };
        assert!(collect_visible_aliases(&sp).contains(&"seed".to_string()));

        let cp = Op::GremlinCyclicPath {
            child: boxed(ns("seed")),
            path_aliases: vec![],
            by_keys: vec![],
            from_label: None,
            to_label: None,
        };
        assert!(collect_visible_aliases(&cp).contains(&"seed".to_string()));

        let idx = Op::GremlinIndex {
            child: boxed(ns("seed")),
            alias: id("a"),
            out_alias: id("o"),
            with_map: false,
        };
        assert!(collect_visible_aliases(&idx).contains(&"seed".to_string()));

        let bar = Op::GremlinBarrier {
            child: boxed(ns("seed")),
            current_alias: id("a"),
            out_alias: id("o"),
            norm_sack: false,
            sack_merge_op: None,
            with_bulk: false,
        };
        assert!(collect_visible_aliases(&bar).contains(&"seed".to_string()));
    }

    #[test]
    fn produce_results_yields_empty() {
        let op = Op::ProduceResults {
            child: boxed(ns("seed")),
            columns: vec![],
        };
        assert!(collect_visible_aliases(&op).is_empty());
    }

    #[test]
    fn expand_and_varlen_expand_expose_dst_and_optional_rel() {
        let e = Op::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            src_alias: id("a"),
            rel_alias: Some(id("r")),
            dst_alias: id("b"),
            child: boxed(ns("seed")),
        };
        let v = collect_visible_aliases(&e);
        assert!(v.contains(&"r".to_string()));
        assert!(v.contains(&"b".to_string()));

        let v_op = Op::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            min: 1,
            max: 2,
            src_alias: id("a"),
            path_alias: Some(id("p")),
            path_mode: false,
            dst_alias: id("b"),
            child: boxed(ns("seed")),
            rel_props: None,
        };
        let v = collect_visible_aliases(&v_op);
        assert!(v.contains(&"p".to_string()));
        assert!(v.contains(&"b".to_string()));
    }

    #[test]
    fn assemble_path_exposes_path_alias() {
        let op = Op::AssemblePath {
            child: boxed(ns("seed")),
            path_alias: id("pp"),
            node_aliases: vec![],
            edge_aliases: vec![],
        };
        assert!(collect_visible_aliases(&op).contains(&"pp".to_string()));
    }
}
