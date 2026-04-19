//! ShortestPath operator tests.

use super::*;

#[test]
fn shortest_path_finds_path() {
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
    let mut start_row = Row::new();
    start_row.insert("src".to_string(), Value::Node(a));
    start_row.insert("dst".to_string(), Value::Node(b));
    let executor = QueryExecutor::new(&store);
    let plan = Operator::ShortestPath {
        src_alias: Identifier::new("src"),
        dst_alias: Identifier::new("dst"),
        rel_types: vec![],
        direction: Direction::Outgoing,
        min: 1,
        max: 10,
        path_alias: Identifier::new("p"),
        all_paths: false,
    };
    let mut budget = ExecutionBudget::unlimited();
    let mut stats = MutationStats::default();
    let rows = executor
        .exec_operator(
            &mut ExecContext {
                graph: "g",
                budget: &mut budget,
                outer_rows: &[start_row],
                stats: &mut stats,
            },
            &plan,
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn shortest_path_deep_chain() {
    let store = make_store();
    let mut nodes = Vec::new();
    for _ in 0..4 {
        nodes.push(
            store
                .create_node("g", vec!["N".to_string()], IndexMap::new())
                .unwrap(),
        );
    }
    for i in 0..3 {
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
    let mut start_row = Row::new();
    start_row.insert("src".to_string(), Value::Node(nodes[0]));
    start_row.insert("dst".to_string(), Value::Node(nodes[3]));
    let executor = QueryExecutor::new(&store);
    let plan = Operator::ShortestPath {
        src_alias: Identifier::new("src"),
        dst_alias: Identifier::new("dst"),
        rel_types: vec![],
        direction: Direction::Outgoing,
        min: 1,
        max: 10,
        path_alias: Identifier::new("p"),
        all_paths: false,
    };
    let mut budget = ExecutionBudget::unlimited();
    let mut stats = MutationStats::default();
    let rows = executor
        .exec_operator(
            &mut ExecContext {
                graph: "g",
                budget: &mut budget,
                outer_rows: &[start_row],
                stats: &mut stats,
            },
            &plan,
        )
        .unwrap();
    assert_eq!(rows.len(), 1);
    if let Some(Value::Path {
        nodes: pn,
        edges: pe,
    }) = rows[0].get("p")
    {
        assert_eq!(pn.len(), 4);
        assert_eq!(pe.len(), 3);
        for i in 0..4 {
            assert_eq!(pn[i], nodes[i]);
        }
    } else {
        panic!("expected path in result row");
    }
}

#[test]
fn shortest_path_no_path_returns_empty() {
    let store = make_store();
    let a = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .create_node("g", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    // No edge between a and b
    let mut start_row = Row::new();
    start_row.insert("src".to_string(), Value::Node(a));
    start_row.insert("dst".to_string(), Value::Node(b));
    let executor = QueryExecutor::new(&store);
    let plan = Operator::ShortestPath {
        src_alias: Identifier::new("src"),
        dst_alias: Identifier::new("dst"),
        rel_types: vec![],
        direction: Direction::Outgoing,
        min: 1,
        max: 10,
        path_alias: Identifier::new("p"),
        all_paths: false,
    };
    let mut budget = ExecutionBudget::unlimited();
    let mut stats = MutationStats::default();
    let rows = executor
        .exec_operator(
            &mut ExecContext {
                graph: "g",
                budget: &mut budget,
                outer_rows: &[start_row],
                stats: &mut stats,
            },
            &plan,
        )
        .unwrap();
    assert_eq!(rows.len(), 0, "no path should yield no rows");
}
