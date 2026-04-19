//! VarLenExpand operator tests.

use super::*;

#[test]
fn varlen_expand_chain() {
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
    store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();
    store
        .create_edge("g", b, c, "R".to_string(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::VarLenExpand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        min: 1,
        max: 2,
        src_alias: Identifier::new("a"),
        path_alias: Some(Identifier::new("p")),
        path_mode: true,
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
        rel_props: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 3);
}

#[test]
fn varlen_expand_deep_chain() {
    let store = make_store();
    let mut nodes = Vec::new();
    for _ in 0..6 {
        nodes.push(
            store
                .create_node("g", vec!["N".to_string()], IndexMap::new())
                .unwrap(),
        );
    }
    for i in 0..5 {
        store
            .create_edge(
                "g",
                nodes[i],
                nodes[i + 1],
                "R".to_string(),
                IndexMap::new(),
            )
            .unwrap();
    }
    let executor = QueryExecutor::new(&store);
    let plan = Operator::VarLenExpand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        min: 1,
        max: 5,
        src_alias: Identifier::new("a"),
        path_alias: Some(Identifier::new("p")),
        path_mode: true,
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
        rel_props: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    // 5+4+3+2+1 = 15
    assert_eq!(result.rows.len(), 15);

    let depth5_rows: Vec<_> = result
        .rows
        .iter()
        .filter(|r| {
            if let Some(Value::Path { nodes: pn, .. }) = r.get("p") {
                pn.len() == 6
            } else {
                false
            }
        })
        .collect();
    assert_eq!(depth5_rows.len(), 1);
    if let Some(Value::Path {
        nodes: pn,
        edges: pe,
    }) = depth5_rows[0].get("p")
    {
        assert_eq!(pn.len(), 6);
        assert_eq!(pe.len(), 5);
        for i in 0..6 {
            assert_eq!(pn[i], nodes[i]);
        }
    }
}

#[test]
fn varlen_expand_rel_variable_mode() {
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
    let e2 = store
        .create_edge("g", b, c, "R".to_string(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::VarLenExpand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        min: 2,
        max: 2,
        src_alias: Identifier::new("a"),
        path_alias: Some(Identifier::new("rs")),
        path_mode: false,
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
        rel_props: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 1);
    if let Some(Value::List(edges)) = result.rows[0].get("rs") {
        assert_eq!(edges.len(), 2);
        assert_eq!(edges[0], Value::Edge(e1));
        assert_eq!(edges[1], Value::Edge(e2));
    } else {
        panic!("expected list of edges for relationship variable mode");
    }
}

#[test]
fn varlen_expand_min_zero() {
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
    let plan = Operator::VarLenExpand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        min: 0,
        max: 1,
        src_alias: Identifier::new("a"),
        path_alias: Some(Identifier::new("p")),
        path_mode: true,
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
        rel_props: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 3);
}

#[test]
fn varlen_expand_cycle_edge_isomorphism() {
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
    store
        .create_edge("g", b, a, "R".to_string(), IndexMap::new())
        .unwrap();
    let executor = QueryExecutor::new(&store);
    let plan = Operator::VarLenExpand {
        direction: Direction::Outgoing,
        rel_types: vec![RelType::new("R")],
        min: 1,
        max: 3,
        src_alias: Identifier::new("a"),
        path_alias: Some(Identifier::new("p")),
        path_mode: true,
        dst_alias: Identifier::new("b"),
        child: Box::new(Operator::NodeScan {
            label: Some(Label::new("N")),
            alias: Identifier::new("a"),
        }),
        rel_props: None,
    };
    let mut budget = ExecutionBudget::unlimited();
    let result = executor.execute("g", &plan, &mut budget).unwrap();
    assert_eq!(result.rows.len(), 4);
}
