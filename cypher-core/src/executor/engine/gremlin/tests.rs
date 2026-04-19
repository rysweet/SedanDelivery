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
