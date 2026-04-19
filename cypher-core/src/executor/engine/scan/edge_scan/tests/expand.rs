//! Expand operator tests.

use super::*;

#[test]
fn expand_outgoing() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("a"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
}

#[test]
fn expand_incoming() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    let plan = Operator::Expand {
        direction: Direction::Incoming,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("b"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("a"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("b"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor_result(&store, &plan, &mut budget);
    // Only b has an incoming edge, so we get 1 result
    assert_eq!(result.len(), 1);
}

#[test]
fn expand_no_matching_type() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    let plan = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("NONEXISTENT")],
        src_alias: Identifier::new("a"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor_result(&store, &plan, &mut budget);
    assert_eq!(result.len(), 0);
}

#[test]
fn expand_batches_adjacency_by_unique_source_node() {
    let base = Arc::new(make_store());
    let a = base
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let b = base
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let c = base
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    base.create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    base.create_edge("g", a, c, "R".to_string(), IndexMap::new())
        .unwrap();

    let store = TrackingStore::new(Arc::clone(&base));
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("a"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::Apply {
            outer: Box::new(Operator::Unwind {
                list_expr: Expr::List(vec![Expr::Integer(1), Expr::Integer(2)]),
                alias: Identifier::new("dup"),
                child: Box::new(Operator::SingleRow),
            }),
            inner: Box::new(Operator::NodeByIdSeek {
                id_expr: Expr::Integer(a.0 as i64),
                alias: Identifier::new("a"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 4);
    assert_eq!(store.out_edges_by_type_calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        store.out_edges_by_type_batch_calls.load(Ordering::Relaxed),
        1
    );
}

#[test]
fn expand_respects_budget_before_batched_adjacency_reads() {
    let base = Arc::new(make_store());
    let a = base
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let b = base
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    base.create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let store = TrackingStore::new(Arc::clone(&base));
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("a"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeByIdSeek {
            id_expr: Expr::Integer(a.0 as i64),
            alias: Identifier::new("a"),
        }),
    };
    let mut budget = ExecutionBudget::new(std::time::Duration::from_secs(3600), 0);
    let err = executor.execute("g", &plan, &mut budget).unwrap_err();

    assert!(matches!(
        err,
        GqlError::Execution(ExecutionError::ResultSetTooLarge { .. })
    ));
    assert_eq!(
        store.out_edges_by_type_batch_calls.load(Ordering::Relaxed),
        0
    );
}

#[test]
fn expand_batches_large_source_sets_in_chunks() {
    let base = Arc::new(make_store());
    for i in 0..300 {
        let src = base
            .create_node(
                "g",
                vec!["Seed".to_string()],
                IndexMap::from([("id".to_string(), Value::Int(i))]),
            )
            .unwrap();
        let dst = base
            .create_node(
                "g",
                vec!["Leaf".to_string()],
                IndexMap::from([("id".to_string(), Value::Int(i + 1000))]),
            )
            .unwrap();
        base.create_edge("g", src, dst, "R".to_string(), IndexMap::new())
            .unwrap();
    }

    let store = TrackingStore::new(Arc::clone(&base));
    let executor = QueryExecutor::new(&store);
    let plan = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("a"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("Seed")),
            alias: Identifier::new("a"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 300);
    assert_eq!(store.out_edges_by_type_calls.load(Ordering::Relaxed), 0);
    assert_eq!(
        store.out_edges_by_type_batch_calls.load(Ordering::Relaxed),
        2
    );
}

#[test]
fn expand_isomorphism_prevents_edge_reuse() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let c = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let e1 = store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    let _e2 = store
        .create_edge("g", b, c, "R".to_string(), IndexMap::new())
        .unwrap();

    let plan = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("b"),
        rel_alias: Some(Identifier::new("r2")),
        dst_alias: Identifier::new("c"),
        child: Box::new(Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![RelType::new("R")],
            src_alias: Identifier::new("a"),
            rel_alias: Some(Identifier::new("r1")),
            dst_alias: Identifier::new("b"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("a"),
            }),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor_result(&store, &plan, &mut budget);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].get("r1"), Some(&Value::Edge(e1)));
}

#[test]
fn expand_outer_edge_vars_allows_reuse_across_scopes() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let e1 = store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let mut input_row = Row::new();
    input_row.insert("a".to_string(), Value::Node(a));
    input_row.insert("r1".to_string(), Value::Edge(e1));
    input_row.insert("b2".to_string(), Value::Node(a));
    input_row.insert(
        "__outer_edge_vars__".to_string(),
        Value::List(vec![Value::Str("r1".to_string().into_boxed_str())]),
    );

    let executor = QueryExecutor::new(&store);
    let plan = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("b2"),
        rel_alias: Some(Identifier::new("r2")),
        dst_alias: Identifier::new("c"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("b2"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let mut stats = MutationStats::default();
    let rows = executor
        .exec_operator(
            &mut ExecContext {
                graph: "g",
                budget: &mut budget,
                outer_rows: &[input_row],
                stats: &mut stats,
            },
            &plan,
        )
        .unwrap();
    let matching: Vec<_> = rows
        .iter()
        .filter(|r| r.get("r2") == Some(&Value::Edge(e1)))
        .collect();
    assert!(
        !matching.is_empty(),
        "outer edge var r1 should not block r2 from using the same edge"
    );
}
#[test]
fn expand_self_loop() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, a, "R".to_string(), IndexMap::new())
        .unwrap();
    let plan = Operator::Expand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("a"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor_result(&store, &plan, &mut budget);
    // Self-loop: a -> a, so expand from a finds 1 edge back to a
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].get("b"), Some(&Value::Node(a)));
}

#[test]
fn expand_undirected_self_loop_dedups_in_cypher_mode() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, a, "R".to_string(), IndexMap::new())
        .unwrap();
    let plan = Operator::Expand {
        direction: Direction::Undirected,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("a"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor_result(&store, &plan, &mut budget);
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].get("b"), Some(&Value::Node(a)));
}

#[test]
fn expand_undirected_self_loop_doubles_in_gremlin_mode() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    store
        .create_edge("g", a, a, "R".to_string(), IndexMap::new())
        .unwrap();
    let plan = Operator::Expand {
        direction: Direction::Undirected,
        rel_types: vec![RelType::new("R")],
        src_alias: Identifier::new("a"),
        rel_alias: Some(Identifier::new("r")),
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
    };
    let executor = QueryExecutor::new(&store).with_gremlin_mode();
    let mut budget = ExecutionBudget::unlimited();
    let mut stats = MutationStats::default();
    let mut cx = ExecContext {
        graph: "g",
        budget: &mut budget,
        outer_rows: &[],
        stats: &mut stats,
    };
    let rows = executor.exec_expand(&mut cx, &plan).unwrap();
    // Gremlin mode: self-loop appears twice (once per direction)
    assert_eq!(
        rows.len(),
        2,
        "Gremlin mode should produce 2 results for undirected self-loop"
    );
}

#[test]
fn gremlin_expand_undirected_orders_marko_neighbors_created_before_knows() {
    let store = make_store();

    let mut marko_props = IndexMap::new();
    marko_props.insert("name".to_string(), Value::Str("marko".into()));
    let marko = store
        .create_node("g", vec!["Person".to_string()], marko_props)
        .unwrap();

    let mut vadas_props = IndexMap::new();
    vadas_props.insert("name".to_string(), Value::Str("vadas".into()));
    let vadas = store
        .create_node("g", vec!["Person".to_string()], vadas_props)
        .unwrap();

    let mut lop_props = IndexMap::new();
    lop_props.insert("name".to_string(), Value::Str("lop".into()));
    let lop = store
        .create_node("g", vec!["Software".to_string()], lop_props)
        .unwrap();

    let mut josh_props = IndexMap::new();
    josh_props.insert("name".to_string(), Value::Str("josh".into()));
    let josh = store
        .create_node("g", vec!["Person".to_string()], josh_props)
        .unwrap();

    store
        .create_edge("g", marko, vadas, "knows".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", marko, josh, "knows".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", marko, lop, "created".to_string(), IndexMap::new())
        .unwrap();

    let plan = Operator::Expand {
        direction: Direction::Undirected,
        rel_types: vec![],
        src_alias: Identifier::new("a"),
        rel_alias: None,
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeByIdSeek {
            id_expr: Expr::Integer(marko.0 as i64),
            alias: Identifier::new("a"),
        }),
    };

    let mut budget = ExecutionBudget::unlimited();
    let result = executor_result_gremlin(&store, &plan, &mut budget);
    let neighbors: Vec<_> = result
        .iter()
        .filter_map(|row| row.get("b").cloned())
        .collect();
    assert_eq!(
        neighbors,
        vec![Value::Node(lop), Value::Node(vadas), Value::Node(josh)]
    );
}
