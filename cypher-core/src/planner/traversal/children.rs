//! `Operator::children` — direct child operator references for structural traversal.

use crate::planner::operators::{GremlinSelectTraversal, Operator};

impl Operator {
    /// Return references to all direct child operators.
    pub fn children(&self) -> Vec<&Operator> {
        match self {
            Operator::SingleRow
            | Operator::NodeScan { .. }
            | Operator::IndexScan { .. }
            | Operator::NodeByIdSeek { .. }
            | Operator::ShortestPath { .. } => vec![],

            Operator::Filter { child, .. }
            | Operator::Project { child, .. }
            | Operator::Aggregate { child, .. }
            | Operator::Distinct { child, .. }
            | Operator::Sort { child, .. }
            | Operator::Limit { child, .. }
            | Operator::Skip { child, .. }
            | Operator::Unwind { child, .. }
            | Operator::Set { child, .. }
            | Operator::Remove { child, .. }
            | Operator::Delete { child, .. }
            | Operator::ProduceResults { child, .. }
            | Operator::Expand { child, .. }
            | Operator::VarLenExpand { child, .. }
            | Operator::AssemblePath { child, .. }
            | Operator::MergeRelationship { child, .. }
            | Operator::GremlinSideEffect { child, .. }
            | Operator::Cap { child, .. }
            | Operator::GremlinAggregate { child, .. }
            | Operator::GremlinPath { child, .. }
            | Operator::GremlinTree { child, .. }
            | Operator::GremlinSample { child, .. }
            | Operator::GremlinMapSlice { child, .. }
            | Operator::GremlinSimplePath { child, .. }
            | Operator::GremlinCyclicPath { child, .. }
            | Operator::GremlinIndex { child, .. }
            | Operator::GremlinBarrier { child, .. } => vec![child],

            Operator::GremlinSelect {
                child,
                by_traversals,
                ..
            } => {
                let mut v = vec![child.as_ref()];
                for entry in by_traversals {
                    if let Some(GremlinSelectTraversal::Traversal { op, .. }) = entry {
                        v.push(op.as_ref());
                    }
                }
                v
            }

            Operator::GremlinGroup {
                child,
                key_traversal,
                value_traversal,
                ..
            } => {
                let mut v = vec![child.as_ref()];
                if let Some((op, _)) = key_traversal {
                    v.push(op.as_ref());
                }
                if let Some((op, _)) = value_traversal {
                    v.push(op.as_ref());
                }
                v
            }

            Operator::GremlinProject { entries, child, .. } => {
                let mut v = vec![child.as_ref()];
                for (_, op, _, _) in entries {
                    v.push(op);
                }
                v
            }

            Operator::ForEach { body, child, .. } | Operator::Repeat { body, child, .. } => {
                vec![child, body]
            }

            Operator::Apply { outer, inner }
            | Operator::LeftOuterApply {
                preserved: outer,
                optional_pattern: inner,
            } => vec![outer, inner],

            Operator::Union { left, right, .. } => vec![left, right],

            Operator::Create { child, .. }
            | Operator::Merge { child, .. }
            | Operator::Call { child, .. } => match child {
                Some(c) => vec![c],
                None => vec![],
            },

            Operator::Choose {
                predicate,
                if_true,
                if_false,
                child,
                ..
            } => {
                let mut v = vec![child.as_ref(), predicate.as_ref(), if_true.as_ref()];
                if let Some(f) = if_false {
                    v.push(f.as_ref());
                }
                v
            }

            Operator::ChooseMap {
                selector,
                options,
                child,
                ..
            } => {
                let mut v = vec![child.as_ref(), selector.as_ref()];
                for (_, branch, _) in options {
                    v.push(branch.as_ref());
                }
                v
            }

            Operator::Coalesce {
                alternatives,
                child,
            } => {
                let mut v = vec![child.as_ref()];
                v.extend(alternatives.iter());
                v
            }

            Operator::GremlinMatch {
                patterns, child, ..
            } => {
                let mut v = vec![child.as_ref()];
                v.extend(patterns.iter());
                v
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::ast::{Direction, Expr, Identifier, Label, RelType};
    use crate::planner::operators::aggregation::AggFunction;
    use crate::planner::operators::gremlin_types::GremlinSelectTraversal;
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
    fn boxed(op: Operator) -> Box<Operator> {
        Box::new(op)
    }

    #[test]
    fn leaf_operators_have_no_children() {
        assert!(Operator::SingleRow.children().is_empty());
        assert!(ns("n").children().is_empty());
        let idx = Operator::IndexScan {
            label: Label::new("L"),
            property: "p".into(),
            value: None,
            alias: id("n"),
        };
        assert!(idx.children().is_empty());
        let seek = Operator::NodeByIdSeek {
            id_expr: Expr::Integer(1),
            alias: id("n"),
        };
        assert!(seek.children().is_empty());
        let sp = Operator::ShortestPath {
            src_alias: id("a"),
            dst_alias: id("b"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 2,
            path_alias: id("p"),
            all_paths: false,
        };
        assert!(sp.children().is_empty());
    }

    #[test]
    fn single_child_operators_return_one_child() {
        let cases: Vec<Operator> = vec![
            Operator::Filter {
                child: boxed(ns("n")),
                predicate: Expr::Bool(true),
            },
            Operator::Project {
                child: boxed(ns("n")),
                items: vec![],
            },
            Operator::Aggregate {
                child: boxed(ns("n")),
                keys: vec![],
                aggregations: vec![],
            },
            Operator::Distinct {
                child: boxed(ns("n")),
                by_key: None,
                productive: false,
            },
            Operator::Sort {
                child: boxed(ns("n")),
                items: vec![],
                gremlin_order: false,
            },
            Operator::Limit {
                child: boxed(ns("n")),
                count: Expr::Integer(1),
            },
            Operator::Skip {
                child: boxed(ns("n")),
                count: Expr::Integer(1),
            },
            Operator::Unwind {
                child: boxed(ns("n")),
                list_expr: Expr::List(vec![]),
                alias: id("x"),
            },
            Operator::Set {
                child: boxed(ns("n")),
                items: vec![],
            },
            Operator::Remove {
                child: boxed(ns("n")),
                items: vec![],
            },
            Operator::Delete {
                child: boxed(ns("n")),
                exprs: vec![],
                detach: false,
            },
            Operator::ProduceResults {
                child: boxed(ns("n")),
                columns: vec![],
            },
            Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![],
                src_alias: id("a"),
                rel_alias: None,
                dst_alias: id("b"),
                child: boxed(ns("n")),
            },
            Operator::VarLenExpand {
                direction: Direction::Outgoing,
                rel_types: vec![],
                min: 1,
                max: 2,
                src_alias: id("a"),
                path_alias: None,
                path_mode: false,
                dst_alias: id("b"),
                child: boxed(ns("n")),
                rel_props: None,
            },
            Operator::AssemblePath {
                child: boxed(ns("n")),
                path_alias: id("p"),
                node_aliases: vec![],
                edge_aliases: vec![],
            },
            Operator::MergeRelationship {
                src_alias: id("a"),
                dst_alias: id("b"),
                rel_alias: None,
                rel_type: RelType::new("R"),
                direction: Direction::Outgoing,
                properties: vec![],
                on_create: vec![],
                on_match: vec![],
                child: boxed(ns("n")),
            },
            Operator::GremlinSideEffect {
                effect: boxed(ns("e")),
                child: boxed(ns("n")),
            },
            Operator::Cap {
                child: boxed(ns("n")),
                keys: vec![],
                reductions: vec![],
            },
            Operator::GremlinAggregate {
                key: "k".into(),
                child: boxed(ns("n")),
                value_alias: id("v"),
                by_keys: vec![],
                by_traversal: None,
                productive: false,
                set_semantics: false,
                assign_semantics: false,
            },
            Operator::GremlinPath {
                child: boxed(ns("n")),
                path_aliases: vec![],
                by_keys: vec![],
                productive: false,
                from_label: None,
                to_label: None,
            },
            Operator::GremlinTree {
                child: boxed(ns("n")),
                path_aliases: vec![],
                side_effect_key: None,
                alias: id("t"),
            },
            Operator::GremlinSample {
                count: 1,
                weight: None,
                seed: None,
                child: boxed(ns("n")),
            },
            Operator::GremlinMapSlice {
                alias: id("a"),
                lo: 0,
                hi: 1,
                child: boxed(ns("n")),
            },
            Operator::GremlinSimplePath {
                child: boxed(ns("n")),
                path_aliases: vec![],
                by_keys: vec![],
                from_label: None,
                to_label: None,
            },
            Operator::GremlinCyclicPath {
                child: boxed(ns("n")),
                path_aliases: vec![],
                by_keys: vec![],
                from_label: None,
                to_label: None,
            },
            Operator::GremlinIndex {
                child: boxed(ns("n")),
                alias: id("a"),
                out_alias: id("o"),
                with_map: false,
            },
            Operator::GremlinBarrier {
                child: boxed(ns("n")),
                current_alias: id("a"),
                out_alias: id("o"),
                norm_sack: false,
                sack_merge_op: None,
                with_bulk: false,
            },
        ];
        for op in cases {
            assert_eq!(op.children().len(), 1, "single-child op: {op:?}");
        }
    }

    #[test]
    fn binary_operators_return_two_children() {
        let apply = Operator::Apply {
            outer: boxed(ns("a")),
            inner: boxed(ns("b")),
        };
        assert_eq!(apply.children().len(), 2);
        let loa = Operator::LeftOuterApply {
            preserved: boxed(ns("a")),
            optional_pattern: boxed(ns("b")),
        };
        assert_eq!(loa.children().len(), 2);
        let union = Operator::Union {
            left: boxed(ns("a")),
            right: boxed(ns("b")),
            all: false,
        };
        assert_eq!(union.children().len(), 2);
        let foreach = Operator::ForEach {
            variable: id("v"),
            list_expr: Expr::List(vec![]),
            body: boxed(ns("b")),
            child: boxed(ns("c")),
        };
        assert_eq!(foreach.children().len(), 2);
    }

    #[test]
    fn create_merge_call_optional_child() {
        let create_with = Operator::Create {
            patterns: vec![],
            child: Some(boxed(ns("n"))),
        };
        assert_eq!(create_with.children().len(), 1);
        let create_without = Operator::Create {
            patterns: vec![],
            child: None,
        };
        assert!(create_without.children().is_empty());
        let merge_with = Operator::Merge {
            node_alias: None,
            labels: vec![],
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: Some(boxed(ns("n"))),
        };
        assert_eq!(merge_with.children().len(), 1);
        let merge_without = Operator::Merge {
            node_alias: None,
            labels: vec![],
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: None,
        };
        assert!(merge_without.children().is_empty());
        let call_with = Operator::Call {
            procedure: "p".into(),
            args: vec![],
            yield_items: vec![],
            yield_star: false,
            standalone: false,
            child: Some(boxed(ns("n"))),
        };
        assert_eq!(call_with.children().len(), 1);
        let call_without = Operator::Call {
            procedure: "p".into(),
            args: vec![],
            yield_items: vec![],
            yield_star: false,
            standalone: true,
            child: None,
        };
        assert!(call_without.children().is_empty());
    }

    #[test]
    fn choose_with_and_without_false_branch() {
        let with_false = Operator::Choose {
            predicate: boxed(ns("p")),
            if_true: boxed(ns("t")),
            if_false: Some(boxed(ns("f"))),
            child: boxed(ns("c")),
            true_alias: None,
            false_alias: None,
            output_alias: None,
        };
        assert_eq!(with_false.children().len(), 4);
        let no_false = Operator::Choose {
            predicate: boxed(ns("p")),
            if_true: boxed(ns("t")),
            if_false: None,
            child: boxed(ns("c")),
            true_alias: None,
            false_alias: None,
            output_alias: None,
        };
        assert_eq!(no_false.children().len(), 3);
    }

    #[test]
    fn choose_map_includes_all_branches() {
        let cm = Operator::ChooseMap {
            selector: boxed(ns("sel")),
            selector_alias: id("s"),
            options: vec![
                (None, boxed(ns("a")), None),
                (Some(crate::types::Value::Int(1)), boxed(ns("b")), None),
            ],
            child: boxed(ns("c")),
            output_alias: None,
        };
        // child + selector + 2 branches
        assert_eq!(cm.children().len(), 4);
    }

    #[test]
    fn coalesce_includes_alternatives() {
        let co = Operator::Coalesce {
            alternatives: vec![ns("a"), ns("b"), ns("c")],
            child: boxed(ns("ch")),
        };
        assert_eq!(co.children().len(), 4);
    }

    #[test]
    fn repeat_returns_child_then_body() {
        let r = Operator::Repeat {
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
        assert_eq!(r.children().len(), 2);
    }

    #[test]
    fn gremlin_match_includes_patterns() {
        let m = Operator::GremlinMatch {
            patterns: vec![ns("p1"), ns("p2")],
            pattern_labels: vec![],
            label_map: vec![],
            result_alias: id("r"),
            child: boxed(ns("c")),
        };
        assert_eq!(m.children().len(), 3);
    }

    #[test]
    fn gremlin_select_includes_by_traversals() {
        let sel = Operator::GremlinSelect {
            keys: vec![],
            by_traversals: vec![
                None,
                Some(GremlinSelectTraversal::Traversal {
                    op: boxed(ns("a")),
                    out_alias: id("o"),
                }),
                Some(GremlinSelectTraversal::Traversal {
                    op: boxed(ns("b")),
                    out_alias: id("o"),
                }),
            ],
            productive: false,
            pop: None,
            label_stacks: vec![],
            child: boxed(ns("c")),
        };
        // child + 2 traversals (None entry is skipped)
        assert_eq!(sel.children().len(), 3);
    }

    #[test]
    fn gremlin_group_includes_optional_traversals() {
        let g = Operator::GremlinGroup {
            key_expr: Expr::Null,
            value_expr: Expr::Null,
            agg_fn: AggFunction::Count,
            alias: id("g"),
            side_effect_key: None,
            key_traversal: Some((boxed(ns("k")), id("ko"))),
            value_traversal: Some((boxed(ns("v")), id("vo"))),
            productive: false,
            key_is_identity: false,
            explicit_fold: false,
            child: boxed(ns("c")),
        };
        assert_eq!(g.children().len(), 3);
        let g2 = Operator::GremlinGroup {
            key_expr: Expr::Null,
            value_expr: Expr::Null,
            agg_fn: AggFunction::Count,
            alias: id("g"),
            side_effect_key: None,
            key_traversal: None,
            value_traversal: None,
            productive: false,
            key_is_identity: true,
            explicit_fold: false,
            child: boxed(ns("c")),
        };
        assert_eq!(g2.children().len(), 1);
    }

    #[test]
    fn gremlin_project_includes_entries() {
        let p = Operator::GremlinProject {
            entries: vec![
                ("k1".into(), ns("a"), id("o"), false),
                ("k2".into(), ns("b"), id("o"), false),
            ],
            child: boxed(ns("c")),
            current_alias: id("cur"),
            out_alias: id("o"),
            productive: false,
        };
        assert_eq!(p.children().len(), 3);
    }
}
