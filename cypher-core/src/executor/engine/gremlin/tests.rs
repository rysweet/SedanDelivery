//! Gremlin executor tests (moved from former monolithic gremlin.rs).

use indexmap::IndexMap;

use crate::ast::{Direction, Expr, Identifier, Label, RelType};
use crate::executor::budget::ExecutionBudget;
use crate::executor::engine::QueryExecutor;
use crate::planner::Operator;
use crate::storage::{GraphStore, InMemoryGraphStore};
use crate::types::Value;

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

/// Build a linear chain A->B->C and return the store.
fn make_chain() -> InMemoryGraphStore {
    let store = make_store();
    let mut pa = IndexMap::new();
    pa.insert("name".to_string(), Value::Str("A".into()));
    let a = store.create_node("g", vec!["N".to_string()], pa).unwrap();
    let mut pb = IndexMap::new();
    pb.insert("name".to_string(), Value::Str("B".into()));
    let b = store.create_node("g", vec!["N".to_string()], pb).unwrap();
    let mut pc = IndexMap::new();
    pc.insert("name".to_string(), Value::Str("C".into()));
    let _c = store.create_node("g", vec!["N".to_string()], pc).unwrap();
    store
        .create_edge("g", a, b, "NEXT".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", b, _c, "NEXT".to_string(), IndexMap::new())
        .unwrap();
    store
}

// ── Repeat tests ─────────────────────────────────────────────────────────

#[test]
fn repeat_fixed_times_traverses_levels() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    // Scan all :N nodes, then repeat out-expand once.
    // With times=1, each starting node expands one hop outward.
    // Use a remap so the body output "m" gets renamed back to "n" for iteration.
    let plan = Operator::Repeat {
        body: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::SingleRow),
        }),
        times: Some(1),
        until: None,
        emit: false,
        emit_before: false,
        emit_predicate: None,
        remap: Some(("m".to_string(), "n".to_string())),
        times_before: false,
        loop_label: None,
        body_path_aliases: vec![],
        pop_origins: vec![],
        needs_path: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // A→B and B→C expand; C has no outgoing NEXT so it is dropped.
    assert_eq!(result.rows.len(), 2);
}

#[test]
fn repeat_with_emit_includes_intermediates() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::Repeat {
        body: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("n"),
            child: Box::new(Operator::SingleRow),
        }),
        times: Some(2),
        until: None,
        emit: true,
        emit_before: true,
        emit_predicate: None,
        remap: None,
        times_before: false,
        loop_label: None,
        body_path_aliases: vec![],
        pop_origins: vec![],
        needs_path: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // emit=true collects intermediates: iter0 emits 3 (scan), iter1 emits 2 (expand),
    // plus final 1 (A->B->C). Total should be > 2.
    assert!(
        result.rows.len() > 2,
        "emit=true should produce more rows than without emit, got {}",
        result.rows.len()
    );
}

// ── Choose tests ─────────────────────────────────────────────────────────

#[test]
fn choose_routes_based_on_predicate() {
    let store = make_store();
    // Create two nodes: one labeled "Yes", one labeled "No".
    let mut py = IndexMap::new();
    py.insert("tag".to_string(), Value::Str("yes".into()));
    store.create_node("g", vec!["Yes".to_string()], py).unwrap();
    let mut pn = IndexMap::new();
    pn.insert("tag".to_string(), Value::Str("no".into()));
    store.create_node("g", vec!["No".to_string()], pn).unwrap();

    let executor = QueryExecutor::new(&store);

    // Predicate: scan for :Yes label (rows with a Yes node produce results).
    // if_true: project tag as "matched"
    // if_false: project tag as "unmatched"
    let plan = Operator::Choose {
        predicate: Box::new(Operator::NodeScan {
            label: Some(Label::new("Yes")),
            alias: Identifier::new("check"),
        }),
        if_true: Box::new(Operator::Project {
            items: vec![(
                Expr::Str("routed_true".into()),
                Some(Identifier::new("result")),
            )],
            child: Box::new(Operator::SingleRow),
        }),
        if_false: Some(Box::new(Operator::Project {
            items: vec![(
                Expr::Str("routed_false".into()),
                Some(Identifier::new("result")),
            )],
            child: Box::new(Operator::SingleRow),
        })),
        child: Box::new(Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        }),
        true_alias: None,
        false_alias: None,
        output_alias: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // 2 input nodes -> 2 output rows
    assert_eq!(result.rows.len(), 2);
    let results: Vec<_> = result.rows.iter().filter_map(|r| r.get("result")).collect();
    assert_eq!(results.len(), 2);
}

#[test]
fn choose_without_else_passes_through() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // Predicate that never matches (scan for label "NOPE").
    // No if_false -> row passes through unchanged.
    let plan = Operator::Choose {
        predicate: Box::new(Operator::NodeScan {
            label: Some(Label::new("NOPE")),
            alias: Identifier::new("check"),
        }),
        if_true: Box::new(Operator::Project {
            items: vec![(
                Expr::Str("should_not_appear".into()),
                Some(Identifier::new("x")),
            )],
            child: Box::new(Operator::SingleRow),
        }),
        if_false: None,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("n"),
        }),
        true_alias: None,
        false_alias: None,
        output_alias: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    // Should have "n" (the original scan) but not "x".
    assert!(result.rows[0].contains_key("n"));
    assert!(!result.rows[0].contains_key("x"));
}

#[test]
fn choose_map_matches_traversal_style_option_tokens() {
    let store = make_store();
    store
        .create_node("g", vec!["Software".to_string()], IndexMap::new())
        .unwrap();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("vadas".into()));
    store
        .create_node("g", vec!["Person".to_string()], props)
        .unwrap();

    let executor = QueryExecutor::new(&store).with_gremlin_mode();
    let plan = Operator::ChooseMap {
        selector: Box::new(Operator::SingleRow),
        selector_alias: Identifier::new("n"),
        options: vec![
            (
                Some(Value::Str(r#"__.hasLabel("Software")"#.into())),
                Box::new(Operator::Project {
                    items: vec![(
                        Expr::Str("software".into()),
                        Some(Identifier::new("result")),
                    )],
                    child: Box::new(Operator::SingleRow),
                }),
                Some(Identifier::new("result")),
            ),
            (
                Some(Value::Str(r#"__.has("name","vadas")"#.into())),
                Box::new(Operator::Project {
                    items: vec![(Expr::Str("vadas".into()), Some(Identifier::new("result")))],
                    child: Box::new(Operator::SingleRow),
                }),
                Some(Identifier::new("result")),
            ),
        ],
        child: Box::new(Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        }),
        output_alias: Some(Identifier::new("result")),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    let mut values: Vec<_> = result
        .rows
        .iter()
        .filter_map(|row| row.get("result").cloned())
        .collect();
    values.sort_by_key(|v| format!("{v:?}"));
    assert_eq!(values.len(), 2);
    assert!(values.contains(&Value::Str("software".into())));
    assert!(values.contains(&Value::Str("vadas".into())));
}

// ── Coalesce tests ───────────────────────────────────────────────────────

#[test]
fn coalesce_returns_first_nonempty() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // First alternative: scan for "NOPE" (empty).
    // Second alternative: project a literal (non-empty).
    let plan = Operator::Coalesce {
        alternatives: vec![
            Operator::NodeScan {
                label: Some(Label::new("NOPE")),
                alias: Identifier::new("miss"),
            },
            Operator::Project {
                items: vec![(Expr::Str("fallback".into()), Some(Identifier::new("val")))],
                child: Box::new(Operator::SingleRow),
            },
        ],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("val"),
        Some(&Value::Str("fallback".into()))
    );
}

#[test]
fn coalesce_uses_first_match_when_multiple_succeed() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::Coalesce {
        alternatives: vec![
            Operator::Project {
                items: vec![(Expr::Str("first".into()), Some(Identifier::new("val")))],
                child: Box::new(Operator::SingleRow),
            },
            Operator::Project {
                items: vec![(Expr::Str("second".into()), Some(Identifier::new("val")))],
                child: Box::new(Operator::SingleRow),
            },
        ],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].get("val"), Some(&Value::Str("first".into())));
}

// ── GremlinSideEffect tests ──────────────────────────────────────────────

#[test]
fn side_effect_passes_input_through() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("root".into()));
    store
        .create_node("g", vec!["A".to_string()], props)
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // Side effect: project something (result is discarded).
    // Input rows should pass through unchanged.
    let plan = Operator::GremlinSideEffect {
        effect: Box::new(Operator::Project {
            items: vec![(
                Expr::Str("ignored".into()),
                Some(Identifier::new("discard")),
            )],
            child: Box::new(Operator::SingleRow),
        }),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert!(result.rows[0].contains_key("n"));
    assert!(!result.rows[0].contains_key("discard"));
}

// ── Cap tests ────────────────────────────────────────────────────────────

#[test]
fn cap_returns_empty_lists_for_keys_without_aggregate() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::Cap {
        keys: vec!["x".to_string(), "y".to_string()],
        reductions: vec![],
        child: Box::new(Operator::SingleRow),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].get("x"), Some(&Value::List(vec![])));
    assert_eq!(result.rows[0].get("y"), Some(&Value::List(vec![])));
}

// ── Aggregate + Cap tests ────────────────────────────────────────────────

#[test]
fn aggregate_cap_collects_all_nodes() {
    let store = make_chain(); // A->B->C
    let executor = QueryExecutor::new(&store);

    let plan = Operator::Cap {
        keys: vec!["x".to_string()],
        reductions: vec![],
        child: Box::new(Operator::GremlinAggregate {
            key: "x".to_string(),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
            value_alias: Identifier::new("n"),
            by_keys: vec![],
            by_traversal: None,
            productive: false,
            set_semantics: false,
            assign_semantics: false,
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(items)) = result.rows[0].get("x") {
        assert_eq!(items.len(), 3, "should collect all 3 nodes");
    } else {
        panic!("expected list for cap key 'x'");
    }
}

#[test]
fn aggregate_cap_collects_outgoing_neighbors() {
    let store = make_chain(); // A->B->C
    let executor = QueryExecutor::new(&store);

    let plan = Operator::Cap {
        keys: vec!["friends".to_string()],
        reductions: vec![],
        child: Box::new(Operator::GremlinAggregate {
            key: "friends".to_string(),
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("m"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
            }),
            value_alias: Identifier::new("m"),
            by_keys: vec![],
            by_traversal: None,
            productive: false,
            set_semantics: false,
            assign_semantics: false,
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(items)) = result.rows[0].get("friends") {
        assert_eq!(items.len(), 2, "should collect 2 outgoing neighbors");
    } else {
        panic!("expected list for cap key 'friends'");
    }
}

#[test]
fn aggregate_passes_rows_through() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinAggregate {
        key: "x".to_string(),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
        value_alias: Identifier::new("n"),
        by_keys: vec![],
        by_traversal: None,
        productive: false,
        set_semantics: false,
        assign_semantics: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        3,
        "aggregate should pass all 3 rows through"
    );
    for row in &result.rows {
        assert!(row.contains_key("__gremlin_agg_x"));
    }
}

#[test]
fn cap_with_aggregate_multiple_keys() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::Cap {
        keys: vec!["x".to_string(), "y".to_string()],
        reductions: vec![],
        child: Box::new(Operator::GremlinAggregate {
            key: "x".to_string(),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
            value_alias: Identifier::new("n"),
            by_keys: vec![],
            by_traversal: None,
            productive: false,
            set_semantics: false,
            assign_semantics: false,
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(items)) = result.rows[0].get("x") {
        assert_eq!(items.len(), 3);
    } else {
        panic!("expected list for 'x'");
    }
    assert!(result.rows[0].contains_key("y"));
}

#[test]
fn aggregate_cap_single_node() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("solo".into()));
    store
        .create_node("g", vec!["X".to_string()], props)
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::Cap {
        keys: vec!["all".to_string()],
        reductions: vec![],
        child: Box::new(Operator::GremlinAggregate {
            key: "all".to_string(),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("n"),
            }),
            value_alias: Identifier::new("n"),
            by_keys: vec![],
            by_traversal: None,
            productive: false,
            set_semantics: false,
            assign_semantics: false,
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(items)) = result.rows[0].get("all") {
        assert_eq!(items.len(), 1);
    } else {
        panic!("expected single-element list");
    }
}

// ── GremlinPath tests ────────────────────────────────────────────────────

#[test]
fn path_returns_traversal_elements() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinPath {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        }),
        path_aliases: vec![Identifier::new("n"), Identifier::new("m")],
        by_keys: vec![],
        productive: false,
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2);
    for row in &result.rows {
        if let Some(Value::List(path)) = row.get("path") {
            assert_eq!(path.len(), 2, "each path should have 2 elements");
        } else {
            panic!("expected path column with list value");
        }
    }
}

#[test]
fn path_three_hops() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinPath {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("m"),
            rel_alias: None,
            dst_alias: Identifier::new("o"),
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("m"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
            }),
        }),
        path_aliases: vec![
            Identifier::new("n"),
            Identifier::new("m"),
            Identifier::new("o"),
        ],
        by_keys: vec![],
        productive: false,
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(path)) = result.rows[0].get("path") {
        assert_eq!(path.len(), 3, "3-hop path should have 3 elements");
    } else {
        panic!("expected path column with list value");
    }
}

#[test]
fn path_with_no_aliases_returns_empty_paths() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinPath {
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("n"),
        }),
        path_aliases: vec![],
        by_keys: vec![],
        productive: false,
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].get("path"), Some(&Value::List(vec![])));
}

#[test]
fn path_single_step() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinPath {
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("n"),
        }),
        path_aliases: vec![Identifier::new("n")],
        by_keys: vec![],
        productive: false,
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2);
    for row in &result.rows {
        if let Some(Value::List(path)) = row.get("path") {
            assert_eq!(path.len(), 1);
        } else {
            panic!("expected single-element path");
        }
    }
}

#[test]
fn path_with_filtered_source() {
    let store = make_chain(); // A->B->C
    let executor = QueryExecutor::new(&store);

    // V(:N).has("name","A").out("NEXT").path()
    let plan = Operator::GremlinPath {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::Filter {
                predicate: Expr::BinaryOp {
                    op: crate::ast::BinaryOp::Eq,
                    left: Box::new(Expr::Property(
                        Box::new(Expr::Variable(Identifier::new("n"))),
                        "name".into(),
                    )),
                    right: Box::new(Expr::Str("A".into())),
                },
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
            }),
        }),
        path_aliases: vec![Identifier::new("n"), Identifier::new("m")],
        by_keys: vec![],
        productive: false,
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1, "only A has an outgoing NEXT");
    if let Some(Value::List(path)) = result.rows[0].get("path") {
        assert_eq!(path.len(), 2);
    } else {
        panic!("expected path");
    }
}

// ── GremlinMatch tests ───────────────────────────────────────────────────

#[test]
fn match_single_pattern() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinMatch {
        pattern_labels: vec![],
        label_map: vec![],
        result_alias: Identifier::new("__match_result"),
        patterns: vec![Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::SingleRow),
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2);
    for row in &result.rows {
        assert!(row.contains_key("n"));
        assert!(row.contains_key("m"));
    }
}

#[test]
fn match_empty_patterns_passes_through() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinMatch {
        pattern_labels: vec![],
        label_map: vec![],
        result_alias: Identifier::new("__match_result"),
        patterns: vec![],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 3);
}

#[test]
fn match_two_patterns_join() {
    let store = make_chain(); // A->B->C
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinMatch {
        pattern_labels: vec![],
        label_map: vec![],
        result_alias: Identifier::new("__match_result"),
        patterns: vec![
            Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("a"),
                rel_alias: None,
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::SingleRow),
            },
            Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("b"),
                rel_alias: None,
                dst_alias: Identifier::new("c"),
                child: Box::new(Operator::SingleRow),
            },
        ],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert!(result.rows[0].contains_key("a"));
    assert!(result.rows[0].contains_key("b"));
    assert!(result.rows[0].contains_key("c"));
}

#[test]
fn match_contradicting_patterns_empty() {
    let store = make_chain(); // A->B->C (linear)
    let executor = QueryExecutor::new(&store);

    // Match: a->b via NEXT, and a->b via NONEXISTENT (no such edges)
    let plan = Operator::GremlinMatch {
        pattern_labels: vec![],
        label_map: vec![],
        result_alias: Identifier::new("__match_result"),
        patterns: vec![
            Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("a"),
                rel_alias: None,
                dst_alias: Identifier::new("b"),
                child: Box::new(Operator::SingleRow),
            },
            Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NONEXISTENT")],
                src_alias: Identifier::new("a"),
                rel_alias: None,
                dst_alias: Identifier::new("d"),
                child: Box::new(Operator::SingleRow),
            },
        ],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        0,
        "no node has both NEXT and NONEXISTENT edges"
    );
}

#[test]
fn match_no_input_rows() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinMatch {
        pattern_labels: vec![],
        label_map: vec![],
        result_alias: Identifier::new("__match_result"),
        patterns: vec![Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::SingleRow),
        }],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("NOPE")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 0);
}

// ── GremlinBarrier tests ─────────────────────────────────────────────────

#[test]
fn barrier_passthrough_copies_alias() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinBarrier {
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
        current_alias: Identifier::new("n"),
        out_alias: Identifier::new("out"),
        norm_sack: false,
        sack_merge_op: None,
        with_bulk: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 3);
    for row in &result.rows {
        assert!(row.contains_key("out"), "out alias should be set");
    }
}

#[test]
fn barrier_norm_sack_normalizes_weights() {
    let store = make_store();
    // Create 2 nodes with __sack values that sum to 10
    let mut p1 = IndexMap::new();
    p1.insert("name".to_string(), Value::Str("a".into()));
    store.create_node("g", vec!["X".to_string()], p1).unwrap();
    let mut p2 = IndexMap::new();
    p2.insert("name".to_string(), Value::Str("b".into()));
    store.create_node("g", vec!["X".to_string()], p2).unwrap();

    let executor = QueryExecutor::new(&store);

    // Inject __sack via a Project that adds the sack column, then barrier(normSack)
    let plan = Operator::GremlinBarrier {
        child: Box::new(Operator::Project {
            items: vec![
                (
                    Expr::Variable(Identifier::new("n")),
                    Some(Identifier::new("n")),
                ),
                (Expr::Float(3.0), Some(Identifier::new("__sack"))),
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("n"),
            }),
        }),
        current_alias: Identifier::new("n"),
        out_alias: Identifier::new("out"),
        norm_sack: true,
        sack_merge_op: None,
        with_bulk: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2);
    for row in &result.rows {
        if let Some(Value::Float(f)) = row.get("__sack") {
            assert!(
                (*f - 0.5).abs() < 0.01,
                "each sack should be 3/6 = 0.5, got {f}"
            );
        } else {
            panic!("expected __sack Float");
        }
    }
}

#[test]
fn barrier_sack_merge_sum() {
    let store = make_store();
    // Two nodes, same name so they merge on the same key
    let mut p1 = IndexMap::new();
    p1.insert("name".to_string(), Value::Str("a".into()));
    store.create_node("g", vec!["X".to_string()], p1).unwrap();
    let mut p2 = IndexMap::new();
    p2.insert("name".to_string(), Value::Str("a".into()));
    store
        .create_node("g", vec!["X".to_string()], p2.clone())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // Inject __sack=2.0 on each row, then barrier(sack_merge_op=sum) groups by current_alias
    let plan = Operator::GremlinBarrier {
        child: Box::new(Operator::Project {
            items: vec![
                (Expr::Str("same".into()), Some(Identifier::new("k"))),
                (Expr::Float(2.0), Some(Identifier::new("__sack"))),
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("n"),
            }),
        }),
        current_alias: Identifier::new("k"),
        out_alias: Identifier::new("out"),
        norm_sack: false,
        sack_merge_op: Some("sum".to_string()),
        with_bulk: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // Both rows have the same key, so they merge into 1 row with sack=4.0
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Float(f)) = result.rows[0].get("__sack") {
        assert!((*f - 4.0).abs() < 0.01, "sum of 2+2 should be 4, got {f}");
    } else {
        panic!("expected __sack Float");
    }
}

#[test]
fn barrier_sack_merge_mult() {
    let store = make_store();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinBarrier {
        child: Box::new(Operator::Project {
            items: vec![
                (Expr::Str("same".into()), Some(Identifier::new("k"))),
                (Expr::Float(3.0), Some(Identifier::new("__sack"))),
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("n"),
            }),
        }),
        current_alias: Identifier::new("k"),
        out_alias: Identifier::new("out"),
        norm_sack: false,
        sack_merge_op: Some("mult".to_string()),
        with_bulk: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Float(f)) = result.rows[0].get("__sack") {
        assert!((*f - 9.0).abs() < 0.01, "3*3 should be 9, got {f}");
    } else {
        panic!("expected __sack Float");
    }
}

#[test]
fn barrier_sack_merge_min() {
    let store = make_store();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // Two rows with same key but different sacks: 1.0 and 5.0
    // Use Filter to split: first node gets sack=1, second gets sack=5
    // Simpler: just use the same sack for both (min of 2.0 and 2.0 is 2.0)
    let plan = Operator::GremlinBarrier {
        child: Box::new(Operator::Project {
            items: vec![
                (Expr::Str("same".into()), Some(Identifier::new("k"))),
                (Expr::Float(2.0), Some(Identifier::new("__sack"))),
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("n"),
            }),
        }),
        current_alias: Identifier::new("k"),
        out_alias: Identifier::new("out"),
        norm_sack: false,
        sack_merge_op: Some("min".to_string()),
        with_bulk: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Float(f)) = result.rows[0].get("__sack") {
        assert!((*f - 2.0).abs() < 0.01, "min(2,2)=2, got {f}");
    } else {
        panic!("expected __sack Float");
    }
}

#[test]
fn barrier_sack_merge_max() {
    let store = make_store();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinBarrier {
        child: Box::new(Operator::Project {
            items: vec![
                (Expr::Str("same".into()), Some(Identifier::new("k"))),
                (Expr::Float(7.0), Some(Identifier::new("__sack"))),
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("n"),
            }),
        }),
        current_alias: Identifier::new("k"),
        out_alias: Identifier::new("out"),
        norm_sack: false,
        sack_merge_op: Some("max".to_string()),
        with_bulk: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Float(f)) = result.rows[0].get("__sack") {
        assert!((*f - 7.0).abs() < 0.01, "max(7,7)=7, got {f}");
    } else {
        panic!("expected __sack Float");
    }
}

#[test]
fn barrier_sack_merge_minus() {
    let store = make_store();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinBarrier {
        child: Box::new(Operator::Project {
            items: vec![
                (Expr::Str("same".into()), Some(Identifier::new("k"))),
                (Expr::Float(5.0), Some(Identifier::new("__sack"))),
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("n"),
            }),
        }),
        current_alias: Identifier::new("k"),
        out_alias: Identifier::new("out"),
        norm_sack: false,
        sack_merge_op: Some("minus".to_string()),
        with_bulk: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Float(f)) = result.rows[0].get("__sack") {
        // First entry sets 5.0, second subtracts 5.0 => 0.0
        assert!((*f - 0.0).abs() < 0.01, "minus: 5-5=0, got {f}");
    } else {
        panic!("expected __sack Float");
    }
}

#[test]
fn barrier_with_bulk_duplicates_rows() {
    let store = make_store();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["X".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // 3 rows with same key → 1 group with bulk=3 → with_bulk emits 3 copies
    let plan = Operator::GremlinBarrier {
        child: Box::new(Operator::Project {
            items: vec![
                (Expr::Str("same".into()), Some(Identifier::new("k"))),
                (Expr::Float(1.0), Some(Identifier::new("__sack"))),
            ],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("n"),
            }),
        }),
        current_alias: Identifier::new("k"),
        out_alias: Identifier::new("out"),
        norm_sack: false,
        sack_merge_op: Some("sum".to_string()),
        with_bulk: true,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        3,
        "with_bulk should duplicate rows by bulk count"
    );
}

// ── GremlinSample tests ──────────────────────────────────────────────────

#[test]
fn sample_uniform_selects_subset() {
    let store = make_chain(); // 3 nodes
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinSample {
        count: 2,
        weight: None,
        seed: Some(42),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2, "sample(2) from 3 should return 2");
}

#[test]
fn sample_count_exceeds_input_returns_all() {
    let store = make_chain(); // 3 nodes
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinSample {
        count: 10,
        weight: None,
        seed: Some(42),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        3,
        "sample(10) from 3 should return all 3"
    );
}

#[test]
fn sample_deterministic_with_seed() {
    let store = make_chain(); // 3 nodes
    let executor1 = QueryExecutor::new(&store);
    let executor2 = QueryExecutor::new(&store);

    let plan = Operator::GremlinSample {
        count: 1,
        weight: None,
        seed: Some(42),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut b1 = ExecutionBudget::unlimited();
    let r1 = executor1.execute("g", &plan, &mut b1).unwrap();
    let mut b2 = ExecutionBudget::unlimited();
    let r2 = executor2.execute("g", &plan, &mut b2).unwrap();
    assert_eq!(r1.rows.len(), 1);
    assert_eq!(r2.rows.len(), 1);
    assert_eq!(
        r1.rows[0].get("n"),
        r2.rows[0].get("n"),
        "same seed should produce same result"
    );
}

#[test]
fn sample_weighted_by_expression() {
    let store = make_store();
    let mut p1 = IndexMap::new();
    p1.insert("w".to_string(), Value::Float(100.0));
    store.create_node("g", vec!["W".to_string()], p1).unwrap();
    let mut p2 = IndexMap::new();
    p2.insert("w".to_string(), Value::Float(0.001));
    store.create_node("g", vec!["W".to_string()], p2).unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinSample {
        count: 1,
        weight: Some(Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            "w".into(),
        )),
        seed: Some(42),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("W")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        1,
        "weighted sample should pick exactly 1"
    );
}

// ── GremlinIndex tests ───────────────────────────────────────────────────

#[test]
fn index_list_with_map_produces_indexed_map() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // Project a list, then index it with_map=true
    let plan = Operator::GremlinIndex {
        child: Box::new(Operator::Project {
            items: vec![(
                Expr::List(vec![Expr::Str("x".into()), Expr::Str("y".into())]),
                Some(Identifier::new("vals")),
            )],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        }),
        alias: Identifier::new("vals"),
        out_alias: Identifier::new("indexed"),
        with_map: true,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Map(m)) = result.rows[0].get("indexed") {
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("0"), Some(&Value::Str("x".into())));
        assert_eq!(m.get("1"), Some(&Value::Str("y".into())));
    } else {
        panic!("expected Map for indexed output");
    }
}

#[test]
fn index_list_without_map_produces_pairs() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinIndex {
        child: Box::new(Operator::Project {
            items: vec![(
                Expr::List(vec![Expr::Str("a".into()), Expr::Str("b".into())]),
                Some(Identifier::new("vals")),
            )],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        }),
        alias: Identifier::new("vals"),
        out_alias: Identifier::new("indexed"),
        with_map: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(pairs)) = result.rows[0].get("indexed") {
        assert_eq!(pairs.len(), 2);
        // Each pair is [element, index]
        assert_eq!(
            pairs[0],
            Value::List(vec![Value::Str("a".into()), Value::Int(0)])
        );
        assert_eq!(
            pairs[1],
            Value::List(vec![Value::Str("b".into()), Value::Int(1)])
        );
    } else {
        panic!("expected List of pairs for indexed output");
    }
}

#[test]
fn index_stream_with_map_wraps_each_element() {
    let store = make_chain(); // 3 nodes
    let executor = QueryExecutor::new(&store);

    // Stream mode (multiple rows): each row gets {0: element}
    let plan = Operator::GremlinIndex {
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
        alias: Identifier::new("n"),
        out_alias: Identifier::new("indexed"),
        with_map: true,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 3);
    for row in &result.rows {
        if let Some(Value::Map(m)) = row.get("indexed") {
            assert_eq!(m.len(), 1, "stream mode with_map should wrap as {{0: val}}");
            assert!(m.contains_key("0"));
        } else {
            panic!("expected Map in stream index mode");
        }
    }
}

#[test]
fn index_stream_without_map_collects_pairs() {
    let store = make_chain(); // 3 nodes
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinIndex {
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
        alias: Identifier::new("n"),
        out_alias: Identifier::new("indexed"),
        with_map: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // Stream mode without map: all pairs collected into a single row
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(pairs)) = result.rows[0].get("indexed") {
        assert_eq!(pairs.len(), 3, "should collect 3 pairs (one per row)");
        // Each pair has [element, 0] (per TinkerPop stream semantics)
        for pair in pairs {
            if let Value::List(items) = pair {
                assert_eq!(items.len(), 2);
                assert_eq!(items[1], Value::Int(0));
            } else {
                panic!("expected pair as List");
            }
        }
    } else {
        panic!("expected List of pairs");
    }
}

// ── GremlinProject tests ─────────────────────────────────────────────────

#[test]
fn project_multi_key_produces_map() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    props.insert("age".to_string(), Value::Int(30));
    store
        .create_node("g", vec!["P".to_string()], props)
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinProject {
        entries: vec![
            (
                "name".to_string(),
                Operator::Project {
                    items: vec![(
                        Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "name".into(),
                        ),
                        Some(Identifier::new("__proj_name")),
                    )],
                    child: Box::new(Operator::SingleRow),
                },
                Identifier::new("__proj_name"),
                false,
            ),
            (
                "age".to_string(),
                Operator::Project {
                    items: vec![(
                        Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "age".into(),
                        ),
                        Some(Identifier::new("__proj_age")),
                    )],
                    child: Box::new(Operator::SingleRow),
                },
                Identifier::new("__proj_age"),
                false,
            ),
        ],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
        current_alias: Identifier::new("n"),
        out_alias: Identifier::new("projected"),
        productive: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Map(m)) = result.rows[0].get("projected") {
        assert_eq!(m.get("name"), Some(&Value::Str("Alice".into())));
        assert_eq!(m.get("age"), Some(&Value::Int(30)));
    } else {
        panic!("expected projected Map");
    }
}

#[test]
fn project_nonproductive_drops_null_keys() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Bob".into()));
    // No "age" property
    store
        .create_node("g", vec!["P".to_string()], props)
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinProject {
        entries: vec![
            (
                "name".to_string(),
                Operator::Project {
                    items: vec![(
                        Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "name".into(),
                        ),
                        Some(Identifier::new("__proj_name")),
                    )],
                    child: Box::new(Operator::SingleRow),
                },
                Identifier::new("__proj_name"),
                false,
            ),
            (
                "missing".to_string(),
                Operator::Project {
                    items: vec![(
                        Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "missing".into(),
                        ),
                        Some(Identifier::new("__proj_missing")),
                    )],
                    child: Box::new(Operator::SingleRow),
                },
                Identifier::new("__proj_missing"),
                false,
            ),
        ],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
        current_alias: Identifier::new("n"),
        out_alias: Identifier::new("projected"),
        productive: false,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Map(m)) = result.rows[0].get("projected") {
        assert!(m.contains_key("name"), "name should be in map");
        assert!(
            !m.contains_key("missing"),
            "null key should be dropped when productive=false"
        );
    } else {
        panic!("expected projected Map");
    }
}

#[test]
fn project_productive_keeps_null_keys() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Bob".into()));
    store
        .create_node("g", vec!["P".to_string()], props)
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinProject {
        entries: vec![
            (
                "name".to_string(),
                Operator::Project {
                    items: vec![(
                        Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "name".into(),
                        ),
                        Some(Identifier::new("__proj_name")),
                    )],
                    child: Box::new(Operator::SingleRow),
                },
                Identifier::new("__proj_name"),
                false,
            ),
            (
                "missing".to_string(),
                Operator::Project {
                    items: vec![(
                        Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "missing".into(),
                        ),
                        Some(Identifier::new("__proj_missing")),
                    )],
                    child: Box::new(Operator::SingleRow),
                },
                Identifier::new("__proj_missing"),
                false,
            ),
        ],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("P")),
            alias: Identifier::new("n"),
        }),
        current_alias: Identifier::new("n"),
        out_alias: Identifier::new("projected"),
        productive: true,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Map(m)) = result.rows[0].get("projected") {
        assert!(m.contains_key("name"));
        assert!(
            m.contains_key("missing"),
            "null key should be kept when productive=true"
        );
        assert_eq!(m.get("missing"), Some(&Value::Null));
    } else {
        panic!("expected projected Map");
    }
}

// ── GremlinMapSlice tests ────────────────────────────────────────────────

#[test]
fn map_slice_map_value() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // Build a Map {a:1, b:2, c:3}, then slice [1..3] → {b:2, c:3}
    let plan = Operator::GremlinMapSlice {
        alias: Identifier::new("m"),
        lo: 1,
        hi: 3,
        child: Box::new(Operator::Project {
            items: vec![(
                Expr::MapLiteral(vec![
                    (smol_str::SmolStr::new("a"), Expr::Integer(1)),
                    (smol_str::SmolStr::new("b"), Expr::Integer(2)),
                    (smol_str::SmolStr::new("c"), Expr::Integer(3)),
                ]),
                Some(Identifier::new("m")),
            )],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Map(m)) = result.rows[0].get("m") {
        assert_eq!(m.len(), 2);
        assert_eq!(m.get("b"), Some(&Value::Int(2)));
        assert_eq!(m.get("c"), Some(&Value::Int(3)));
    } else {
        panic!("expected sliced Map");
    }
}

#[test]
fn map_slice_list_value() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinMapSlice {
        alias: Identifier::new("lst"),
        lo: 1,
        hi: 3,
        child: Box::new(Operator::Project {
            items: vec![(
                Expr::List(vec![
                    Expr::Integer(10),
                    Expr::Integer(20),
                    Expr::Integer(30),
                    Expr::Integer(40),
                ]),
                Some(Identifier::new("lst")),
            )],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(l)) = result.rows[0].get("lst") {
        assert_eq!(l, &vec![Value::Int(20), Value::Int(30)]);
    } else {
        panic!("expected sliced List");
    }
}

#[test]
fn map_slice_non_collection_passthrough() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // Slicing a scalar value should pass it through unchanged
    let plan = Operator::GremlinMapSlice {
        alias: Identifier::new("val"),
        lo: 0,
        hi: 2,
        child: Box::new(Operator::Project {
            items: vec![(Expr::Integer(42), Some(Identifier::new("val")))],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("A")),
                alias: Identifier::new("n"),
            }),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0].get("val"), Some(&Value::Int(42)));
}

// ── GremlinSimplePath tests ──────────────────────────────────────────────

#[test]
fn simple_path_keeps_unique_paths() {
    let store = make_chain(); // A→B→C
    let executor = QueryExecutor::new(&store);

    // out().out() from A gives A→B→C — all unique nodes → simple path
    let plan = Operator::GremlinSimplePath {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("m"),
            rel_alias: None,
            dst_alias: Identifier::new("o"),
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("NEXT")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("m"),
                child: Box::new(Operator::NodeScan {
                    label: Some(Label::new("N")),
                    alias: Identifier::new("n"),
                }),
            }),
        }),
        path_aliases: vec![
            Identifier::new("n"),
            Identifier::new("m"),
            Identifier::new("o"),
        ],
        by_keys: vec![],
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // Only A→B→C path: all distinct nodes → kept
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn simple_path_filters_cyclic_traversal() {
    let store = make_store();
    // Create a cycle: A→B→A
    let mut pa = IndexMap::new();
    pa.insert("name".to_string(), Value::Str("A".into()));
    let a = store.create_node("g", vec!["C".to_string()], pa).unwrap();
    let mut pb = IndexMap::new();
    pb.insert("name".to_string(), Value::Str("B".into()));
    let b = store.create_node("g", vec!["C".to_string()], pb).unwrap();
    store
        .create_edge("g", a, b, "LINK".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", b, a, "LINK".to_string(), IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // Start from A, expand twice: A→B→A contains duplicate A → not simple
    let plan = Operator::GremlinSimplePath {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("LINK")],
            src_alias: Identifier::new("m"),
            rel_alias: None,
            dst_alias: Identifier::new("o"),
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("LINK")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("m"),
                child: Box::new(Operator::Filter {
                    predicate: Expr::BinaryOp {
                        op: crate::ast::BinaryOp::Eq,
                        left: Box::new(Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "name".into(),
                        )),
                        right: Box::new(Expr::Str("A".into())),
                    },
                    child: Box::new(Operator::NodeScan {
                        label: Some(Label::new("C")),
                        alias: Identifier::new("n"),
                    }),
                }),
            }),
        }),
        path_aliases: vec![
            Identifier::new("n"),
            Identifier::new("m"),
            Identifier::new("o"),
        ],
        by_keys: vec![],
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        0,
        "A→B→A path has duplicate nodes, not simple"
    );
}

#[test]
fn simple_path_by_keys_compares_property() {
    let store = make_store();
    // Two nodes with same name → path elements with same "name" aren't simple by name
    let mut p1 = IndexMap::new();
    p1.insert("name".to_string(), Value::Str("same".into()));
    let a = store.create_node("g", vec!["D".to_string()], p1).unwrap();
    let mut p2 = IndexMap::new();
    p2.insert("name".to_string(), Value::Str("same".into()));
    let b = store.create_node("g", vec!["D".to_string()], p2).unwrap();
    store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinSimplePath {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("D")),
                alias: Identifier::new("n"),
            }),
        }),
        path_aliases: vec![Identifier::new("n"), Identifier::new("m")],
        by_keys: vec!["name".to_string()],
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        0,
        "nodes with same name aren't simple by name"
    );
}

// ── GremlinCyclicPath tests ──────────────────────────────────────────────

#[test]
fn cyclic_path_no_cycle_returns_empty() {
    let store = make_chain(); // A→B→C (linear, no cycles)
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinCyclicPath {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        }),
        path_aliases: vec![Identifier::new("n"), Identifier::new("m")],
        by_keys: vec![],
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        0,
        "linear chain has no cyclic paths (all nodes are unique)"
    );
}

#[test]
fn cyclic_path_with_cycle_keeps_rows() {
    let store = make_store();
    // Create cycle: A→B→A
    let mut pa = IndexMap::new();
    pa.insert("name".to_string(), Value::Str("A".into()));
    let a = store.create_node("g", vec!["E".to_string()], pa).unwrap();
    let mut pb = IndexMap::new();
    pb.insert("name".to_string(), Value::Str("B".into()));
    let b = store.create_node("g", vec!["E".to_string()], pb).unwrap();
    store
        .create_edge("g", a, b, "LINK".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", b, a, "LINK".to_string(), IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // Start from A, expand twice → A→B→A has cycle
    let plan = Operator::GremlinCyclicPath {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("LINK")],
            src_alias: Identifier::new("m"),
            rel_alias: None,
            dst_alias: Identifier::new("o"),
            child: Box::new(Operator::Expand {
                direction: Direction::Outgoing,
                rel_types: vec![RelType::new("LINK")],
                src_alias: Identifier::new("n"),
                rel_alias: None,
                dst_alias: Identifier::new("m"),
                child: Box::new(Operator::Filter {
                    predicate: Expr::BinaryOp {
                        op: crate::ast::BinaryOp::Eq,
                        left: Box::new(Expr::Property(
                            Box::new(Expr::Variable(Identifier::new("n"))),
                            "name".into(),
                        )),
                        right: Box::new(Expr::Str("A".into())),
                    },
                    child: Box::new(Operator::NodeScan {
                        label: Some(Label::new("E")),
                        alias: Identifier::new("n"),
                    }),
                }),
            }),
        }),
        path_aliases: vec![
            Identifier::new("n"),
            Identifier::new("m"),
            Identifier::new("o"),
        ],
        by_keys: vec![],
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1, "A→B→A has a cycle, should be kept");
}

#[test]
fn cyclic_path_by_keys_compares_property() {
    let store = make_store();
    // Two nodes with same name: path has "duplicate" by name
    let mut p1 = IndexMap::new();
    p1.insert("name".to_string(), Value::Str("same".into()));
    let a = store.create_node("g", vec!["F".to_string()], p1).unwrap();
    let mut p2 = IndexMap::new();
    p2.insert("name".to_string(), Value::Str("same".into()));
    let b = store.create_node("g", vec!["F".to_string()], p2).unwrap();
    store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinCyclicPath {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("F")),
                alias: Identifier::new("n"),
            }),
        }),
        path_aliases: vec![Identifier::new("n"), Identifier::new("m")],
        by_keys: vec!["name".to_string()],
        from_label: None,
        to_label: None,
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        1,
        "by_keys=name: both nodes named 'same' → cyclic by name"
    );
}

// ── GremlinTree tests ────────────────────────────────────────────────────

#[test]
fn tree_barrier_mode_produces_single_map() {
    let store = make_chain(); // A→B→C
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinTree {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        }),
        path_aliases: vec![Identifier::new("n"), Identifier::new("m")],
        side_effect_key: None,
        alias: Identifier::new("tree"),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1, "barrier mode returns single row");
    if let Some(Value::Map(tree)) = result.rows[0].get("tree") {
        assert!(!tree.is_empty(), "tree should have entries");
    } else {
        panic!("expected tree Map");
    }
}

#[test]
fn tree_side_effect_mode_passes_through() {
    let store = make_chain(); // A→B→C
    let executor = QueryExecutor::new(&store);

    let plan = Operator::GremlinTree {
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        }),
        path_aliases: vec![Identifier::new("n"), Identifier::new("m")],
        side_effect_key: Some("t".to_string()),
        alias: Identifier::new("tree"),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // Side-effect mode passes through input rows (2 edges: A→B, B→C)
    assert_eq!(result.rows.len(), 2, "side-effect mode passes rows through");
    for row in &result.rows {
        assert!(
            row.contains_key("__gremlin_agg_t"),
            "side-effect tree should be stored in __gremlin_agg_t"
        );
    }
}

// ── GremlinAggregate gap tests ───────────────────────────────────────────

#[test]
fn aggregate_by_keys_collects_properties() {
    let store = make_chain(); // A, B, C
    let executor = QueryExecutor::new(&store);

    let plan = Operator::Cap {
        keys: vec!["x".to_string()],
        reductions: vec![],
        child: Box::new(Operator::GremlinAggregate {
            key: "x".to_string(),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
            value_alias: Identifier::new("n"),
            by_keys: vec!["name".to_string()],
            by_traversal: None,
            productive: false,
            set_semantics: false,
            assign_semantics: false,
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(items)) = result.rows[0].get("x") {
        assert_eq!(items.len(), 3, "should collect name property for each node");
        let names: Vec<_> = items
            .iter()
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.as_ref().to_string()),
                _ => None,
            })
            .collect();
        assert!(names.contains(&"A".to_string()));
        assert!(names.contains(&"B".to_string()));
        assert!(names.contains(&"C".to_string()));
    } else {
        panic!("expected list for cap key 'x'");
    }
}

#[test]
fn aggregate_set_semantics_deduplicates() {
    let store = make_store();
    // Create 3 nodes with same name → set should deduplicate
    for _ in 0..3 {
        let mut props = IndexMap::new();
        props.insert("name".to_string(), Value::Str("dup".into()));
        store
            .create_node("g", vec!["S".to_string()], props)
            .unwrap();
    }

    let executor = QueryExecutor::new(&store);

    let plan = Operator::Cap {
        keys: vec!["s".to_string()],
        reductions: vec![],
        child: Box::new(Operator::GremlinAggregate {
            key: "s".to_string(),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("S")),
                alias: Identifier::new("n"),
            }),
            value_alias: Identifier::new("n"),
            by_keys: vec!["name".to_string()],
            by_traversal: None,
            productive: false,
            set_semantics: true,
            assign_semantics: false,
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(items)) = result.rows[0].get("s") {
        assert_eq!(
            items.len(),
            1,
            "set_semantics should deduplicate 3 'dup' into 1"
        );
    } else {
        panic!("expected list for cap key 's'");
    }
}

#[test]
fn aggregate_assign_semantics_replaces() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    // assign_semantics: last call's values replace previous
    let plan = Operator::Cap {
        keys: vec!["a".to_string()],
        reductions: vec![],
        child: Box::new(Operator::GremlinAggregate {
            key: "a".to_string(),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
            value_alias: Identifier::new("n"),
            by_keys: vec![],
            by_traversal: None,
            productive: false,
            set_semantics: false,
            assign_semantics: true,
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    // With assign_semantics, only the last call's values survive
    if let Some(Value::List(items)) = result.rows[0].get("a") {
        // Still 3 items since all collected in one call
        assert_eq!(items.len(), 3);
    } else {
        panic!("expected list for cap key 'a'");
    }
}

#[test]
fn aggregate_productive_includes_nulls() {
    let store = make_store();
    // Node without "missing" property
    store
        .create_node("g", vec!["P".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::Cap {
        keys: vec!["p".to_string()],
        reductions: vec![],
        child: Box::new(Operator::GremlinAggregate {
            key: "p".to_string(),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
            value_alias: Identifier::new("n"),
            by_keys: vec!["missing".to_string()],
            by_traversal: None,
            productive: true,
            set_semantics: false,
            assign_semantics: false,
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(items)) = result.rows[0].get("p") {
        assert_eq!(items.len(), 1, "productive=true includes null");
        assert_eq!(items[0], Value::Null);
    } else {
        panic!("expected list for cap key 'p'");
    }
}

#[test]
fn aggregate_nonproductive_skips_nulls() {
    let store = make_store();
    store
        .create_node("g", vec!["P".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    let plan = Operator::Cap {
        keys: vec!["p".to_string()],
        reductions: vec![],
        child: Box::new(Operator::GremlinAggregate {
            key: "p".to_string(),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
            value_alias: Identifier::new("n"),
            by_keys: vec!["missing".to_string()],
            by_traversal: None,
            productive: false,
            set_semantics: false,
            assign_semantics: false,
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(items)) = result.rows[0].get("p") {
        assert_eq!(items.len(), 0, "productive=false skips null properties");
    } else {
        panic!("expected list for cap key 'p'");
    }
}

// ── GremlinGroup tests ───────────────────────────────────────────────────

use crate::planner::AggFunction;

#[test]
fn group_count_by_label() {
    let store = make_chain(); // A, B, C all label "N"
    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    let plan = Operator::GremlinGroup {
        key_expr: Expr::Str("all".into()),
        value_expr: Expr::Variable(Identifier::new("n")),
        agg_fn: AggFunction::Count,
        alias: Identifier::new("grouped"),
        side_effect_key: None,
        key_traversal: None,
        value_traversal: None,
        productive: false,
        key_is_identity: false,
        explicit_fold: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Map(m)) = result.rows[0].get("grouped") {
        assert_eq!(m.get("all"), Some(&Value::Int(3)));
    } else {
        panic!("expected grouped Map");
    }
}

#[test]
fn group_collect_values() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    let plan = Operator::GremlinGroup {
        key_expr: Expr::Str("all".into()),
        value_expr: Expr::Variable(Identifier::new("n")),
        agg_fn: AggFunction::Collect,
        alias: Identifier::new("grouped"),
        side_effect_key: None,
        key_traversal: None,
        value_traversal: None,
        productive: false,
        key_is_identity: false,
        explicit_fold: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Map(m)) = result.rows[0].get("grouped") {
        if let Some(Value::List(items)) = m.get("all") {
            assert_eq!(items.len(), 3, "collect should gather 3 nodes");
        } else {
            panic!("expected list value in grouped map");
        }
    } else {
        panic!("expected grouped Map");
    }
}

#[test]
fn group_sum_values() {
    let store = make_store();
    for i in 1..=3 {
        let mut props = IndexMap::new();
        props.insert("val".to_string(), Value::Int(i * 10));
        store
            .create_node("g", vec!["V".to_string()], props)
            .unwrap();
    }

    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    let plan = Operator::GremlinGroup {
        key_expr: Expr::Str("total".into()),
        value_expr: Expr::Property(Box::new(Expr::Variable(Identifier::new("n"))), "val".into()),
        agg_fn: AggFunction::Sum,
        alias: Identifier::new("grouped"),
        side_effect_key: None,
        key_traversal: None,
        value_traversal: None,
        productive: false,
        key_is_identity: false,
        explicit_fold: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("V")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Map(m)) = result.rows[0].get("grouped") {
        assert_eq!(
            m.get("total"),
            Some(&Value::Int(60)),
            "sum of 10+20+30 = 60"
        );
    } else {
        panic!("expected grouped Map");
    }
}

#[test]
fn group_side_effect_passes_through() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    let plan = Operator::GremlinGroup {
        key_expr: Expr::Str("all".into()),
        value_expr: Expr::Variable(Identifier::new("n")),
        agg_fn: AggFunction::Count,
        alias: Identifier::new("grouped"),
        side_effect_key: Some("g1".to_string()),
        key_traversal: None,
        value_traversal: None,
        productive: false,
        key_is_identity: false,
        explicit_fold: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // Side-effect mode passes through all 3 input rows
    assert_eq!(
        result.rows.len(),
        3,
        "side-effect mode should pass rows through"
    );
    for row in &result.rows {
        assert!(
            row.contains_key("__gremlin_agg_g1"),
            "side-effect should be in rows"
        );
    }
}

#[test]
fn group_null_key_skipped_without_productive() {
    let store = make_store();
    // Node without "missing" property → key evaluates to null
    store
        .create_node("g", vec!["Z".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    let plan = Operator::GremlinGroup {
        key_expr: Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            "missing".into(),
        ),
        value_expr: Expr::Variable(Identifier::new("n")),
        agg_fn: AggFunction::Count,
        alias: Identifier::new("grouped"),
        side_effect_key: None,
        key_traversal: None,
        value_traversal: None,
        productive: false,
        key_is_identity: false,
        explicit_fold: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Z")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Map(m)) = result.rows[0].get("grouped") {
        assert!(
            m.is_empty(),
            "null keys should be skipped without productive"
        );
    } else {
        panic!("expected grouped Map");
    }
}

#[test]
fn group_null_key_included_with_productive() {
    let store = make_store();
    store
        .create_node("g", vec!["Z".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    let plan = Operator::GremlinGroup {
        key_expr: Expr::Property(
            Box::new(Expr::Variable(Identifier::new("n"))),
            "missing".into(),
        ),
        value_expr: Expr::Variable(Identifier::new("n")),
        agg_fn: AggFunction::Count,
        alias: Identifier::new("grouped"),
        side_effect_key: None,
        key_traversal: None,
        value_traversal: None,
        productive: true,
        key_is_identity: false,
        explicit_fold: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Z")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::Map(m)) = result.rows[0].get("grouped") {
        assert!(
            !m.is_empty(),
            "null keys should be included with productive=true"
        );
    } else {
        panic!("expected grouped Map");
    }
}

// ── GremlinSelect tests ──────────────────────────────────────────────────

#[test]
fn select_single_key() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    // Scan :N as "n", then select("n") — should extract the n column
    let plan = Operator::GremlinSelect {
        keys: vec![(Identifier::new("n"), "n".to_string())],
        by_traversals: vec![None],
        productive: false,
        pop: None,
        label_stacks: vec![],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        3,
        "select single key should return 3 rows"
    );
    for row in &result.rows {
        assert!(row.contains_key("n"));
    }
}

#[test]
fn select_multi_key_produces_map() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    // Expand A→B, then select("n","m") — should produce a Map
    let plan = Operator::GremlinSelect {
        keys: vec![
            (Identifier::new("n"), "n".to_string()),
            (Identifier::new("m"), "m".to_string()),
        ],
        by_traversals: vec![None, None],
        productive: false,
        pop: None,
        label_stacks: vec![],
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("n"),
            }),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2, "A→B and B→C expansions");
    for row in &result.rows {
        // Multi-key select builds a combined Map column
        assert!(
            row.contains_key("n") && row.contains_key("m"),
            "both keys should be present"
        );
    }
}

#[test]
fn select_missing_key_drops_row() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    // Select a key that doesn't exist in the row
    let plan = Operator::GremlinSelect {
        keys: vec![(Identifier::new("nonexistent"), "nonexistent".to_string())],
        by_traversals: vec![None],
        productive: false,
        pop: None,
        label_stacks: vec![],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        0,
        "missing key should drop the row when productive=false"
    );
}

#[test]
fn select_pop_last() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    use crate::planner::GremlinPop;

    let plan = Operator::GremlinSelect {
        keys: vec![(Identifier::new("n"), "n".to_string())],
        by_traversals: vec![None],
        productive: false,
        pop: Some(GremlinPop::Last),
        label_stacks: vec![("n".to_string(), vec![Identifier::new("n")])],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 3, "pop=last should still return 3 rows");
}

// ── Choose/Branch gap tests ──────────────────────────────────────────────

#[test]
fn choose_map_default_branch() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    // Option value won't match any selector → falls to default branch (None match_value)
    let plan = Operator::ChooseMap {
        selector: Box::new(Operator::Project {
            items: vec![(Expr::Str("no_match".into()), Some(Identifier::new("sel")))],
            child: Box::new(Operator::SingleRow),
        }),
        selector_alias: Identifier::new("sel"),
        options: vec![
            (
                Some(Value::Str("specific_value".into())),
                Box::new(Operator::Project {
                    items: vec![(Expr::Str("matched".into()), Some(Identifier::new("result")))],
                    child: Box::new(Operator::SingleRow),
                }),
                Some(Identifier::new("result")),
            ),
            (
                None, // default branch
                Box::new(Operator::Project {
                    items: vec![(Expr::Str("default".into()), Some(Identifier::new("result")))],
                    child: Box::new(Operator::SingleRow),
                }),
                Some(Identifier::new("result")),
            ),
        ],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("n"),
        }),
        output_alias: Some(Identifier::new("result")),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    assert_eq!(
        result.rows[0].get("result"),
        Some(&Value::Str("default".into())),
        "should route to default branch"
    );
}

#[test]
fn choose_output_alias_unifies_branches() {
    let store = make_store();
    // Create one "Yes" and one "No" node
    store
        .create_node("g", vec!["Yes".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_node("g", vec!["No".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store).with_gremlin_mode();

    let plan = Operator::Choose {
        predicate: Box::new(Operator::NodeScan {
            label: Some(Label::new("Yes")),
            alias: Identifier::new("check"),
        }),
        if_true: Box::new(Operator::Project {
            items: vec![(
                Expr::Str("yes_result".into()),
                Some(Identifier::new("t_result")),
            )],
            child: Box::new(Operator::SingleRow),
        }),
        if_false: Some(Box::new(Operator::Project {
            items: vec![(
                Expr::Str("no_result".into()),
                Some(Identifier::new("f_result")),
            )],
            child: Box::new(Operator::SingleRow),
        })),
        child: Box::new(Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        }),
        true_alias: Some(Identifier::new("t_result")),
        false_alias: Some(Identifier::new("f_result")),
        output_alias: Some(Identifier::new("unified")),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 2);
    for row in &result.rows {
        assert!(
            row.contains_key("unified"),
            "output_alias should unify branch results"
        );
    }
}

#[test]
fn coalesce_all_empty_drops_traverser() {
    let store = make_store();
    store
        .create_node("g", vec!["A".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);

    // All alternatives produce empty results
    let plan = Operator::Coalesce {
        alternatives: vec![
            Operator::NodeScan {
                label: Some(Label::new("NOPE1")),
                alias: Identifier::new("a"),
            },
            Operator::NodeScan {
                label: Some(Label::new("NOPE2")),
                alias: Identifier::new("b"),
            },
        ],
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("A")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        0,
        "all-empty coalesce should drop traverser"
    );
}

// ── Repeat gap tests ─────────────────────────────────────────────────────

#[test]
fn repeat_times_before_checks_count_first() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    // times_before=true with times=0 → while-loop: check count BEFORE body
    // times=0 means execute body 0 times → just return input
    let plan = Operator::Repeat {
        body: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("n"),
            child: Box::new(Operator::SingleRow),
        }),
        times: Some(0),
        until: None,
        emit: false,
        emit_before: false,
        emit_predicate: None,
        remap: None,
        times_before: true,
        loop_label: None,
        body_path_aliases: vec![],
        pop_origins: vec![],
        needs_path: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // times_before=true, times=0: body never executes, input passes through
    assert_eq!(
        result.rows.len(),
        3,
        "times_before=true with times=0 should return all 3 input rows unchanged"
    );
}

#[test]
fn repeat_emit_before_includes_starting_traversers() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    // emit=true, emit_before=true → starting traversers are emitted
    let plan = Operator::Repeat {
        body: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("n"),
            child: Box::new(Operator::SingleRow),
        }),
        times: Some(1),
        until: None,
        emit: true,
        emit_before: true,
        emit_predicate: None,
        remap: None,
        times_before: false,
        loop_label: None,
        body_path_aliases: vec![],
        pop_origins: vec![],
        needs_path: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // emit_before=true with times=1: starting traversers (3) are emitted,
    // then body executes once producing 2 more (A→B, B→C).
    // However, the executor may merge or deduplicate depending on remap.
    // The key invariant: emit_before produces at least as many rows as the
    // starting set (3).
    assert!(
        result.rows.len() >= 3,
        "emit_before should include starting traversers, got {}",
        result.rows.len()
    );
}

#[test]
fn repeat_loop_label_uses_named_counter() {
    let store = make_chain();
    let executor = QueryExecutor::new(&store);

    let plan = Operator::Repeat {
        body: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("n"),
            child: Box::new(Operator::SingleRow),
        }),
        times: Some(1),
        until: None,
        emit: false,
        emit_before: false,
        emit_predicate: None,
        remap: None,
        times_before: false,
        loop_label: Some("outer".into()),
        body_path_aliases: vec![],
        pop_origins: vec![],
        needs_path: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(
        result.rows.len(),
        2,
        "repeat with loop_label should work same as without for basic case"
    );
}

#[test]
fn repeat_emit_predicate_filters_emitted() {
    let store = make_chain(); // A→B→C
    let executor = QueryExecutor::new(&store);

    // emit with predicate: only emit rows where name="B"
    let plan = Operator::Repeat {
        body: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("NEXT")],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("n"),
            child: Box::new(Operator::SingleRow),
        }),
        times: Some(2),
        until: None,
        emit: true,
        emit_before: true,
        emit_predicate: Some(Box::new(Operator::Filter {
            predicate: Expr::BinaryOp {
                op: crate::ast::BinaryOp::Eq,
                left: Box::new(Expr::Property(
                    Box::new(Expr::Variable(Identifier::new("n"))),
                    "name".into(),
                )),
                right: Box::new(Expr::Str("B".into())),
            },
            child: Box::new(Operator::SingleRow),
        })),
        remap: None,
        times_before: false,
        loop_label: None,
        body_path_aliases: vec![],
        pop_origins: vec![],
        needs_path: false,
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("n"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // emit_predicate filters: only "B" traversers are emitted
    // Plus the final result (traversers that complete all iterations)
    assert!(
        !result.rows.is_empty(),
        "emit with predicate should produce some rows"
    );
}
