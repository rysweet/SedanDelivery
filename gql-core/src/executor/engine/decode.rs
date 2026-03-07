//! Path structure decoding: arena-based BFS tracking and path reconstruction.

use crate::storage::{EdgeId, NodeId};

/// Arena-allocated parent-pointer tree for BFS path tracking.
///
/// Instead of cloning `Vec<NodeId>`, `Vec<EdgeId>`, and `HashSet<EdgeId>` at
/// every BFS expansion step (O(depth) per step, O(d * b^d) total), we store a
/// flat arena of `(NodeId, EdgeId, parent_index)` entries.  Each BFS step
/// pushes one entry in O(1).  Full paths are reconstructed by walking parent
/// pointers only when producing output rows.
///
/// The root entry uses `parent == usize::MAX` as a sentinel.
pub(crate) struct BfsArena {
    /// Entries: `(node_id, edge_id, parent_index)`.
    /// Root entries have `parent == usize::MAX` and `edge_id == EdgeId(u64::MAX)` (unused).
    pub entries: Vec<(NodeId, EdgeId, usize)>,
}

impl BfsArena {
    /// Create a new arena with a root entry for the given source node.
    /// Returns `(arena, root_index)`.
    pub fn new(src: NodeId) -> (Self, usize) {
        let arena = BfsArena {
            entries: vec![(src, EdgeId(u64::MAX), usize::MAX)],
        };
        (arena, 0)
    }

    /// Push a new child entry and return its index.
    #[inline]
    pub fn push(&mut self, node: NodeId, edge: EdgeId, parent: usize) -> usize {
        let idx = self.entries.len();
        self.entries.push((node, edge, parent));
        idx
    }

    /// Check whether `edge` already appears on the path from `idx` back to root.
    /// Walks parent pointers -- O(depth) but depth is bounded by the query's `max`.
    #[inline]
    pub fn path_contains_edge(&self, mut idx: usize, edge: EdgeId) -> bool {
        while idx != usize::MAX {
            let (_, e, parent) = self.entries[idx];
            // Skip the root entry (sentinel edge)
            if parent != usize::MAX && e == edge {
                return true;
            }
            idx = parent;
        }
        false
    }

    /// Reconstruct the full path of nodes and edges from a given entry index
    /// back to the root, in forward (root-to-leaf) order.
    pub fn reconstruct_path(&self, mut idx: usize) -> (Vec<NodeId>, Vec<EdgeId>) {
        let mut nodes = Vec::new();
        let mut edges = Vec::new();
        while idx != usize::MAX {
            let (node, edge, parent) = self.entries[idx];
            nodes.push(node);
            if parent != usize::MAX {
                edges.push(edge);
            }
            idx = parent;
        }
        nodes.reverse();
        edges.reverse();
        (nodes, edges)
    }
}

#[cfg(test)]
mod tests {
    use super::super::helpers::cmp_values;
    use crate::ast::{Direction, Expr, Identifier, Label, RelType};
    use crate::executor::budget::ExecutionBudget;
    use crate::executor::engine::{MutationStats, QueryExecutor, Row};
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};
    use crate::types::Value;
    use std::collections::HashMap;

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn varlen_expand_linear_chain() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".into(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, c, "R".into(), HashMap::new())
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
        let paths: Vec<_> = result
            .rows
            .iter()
            .filter(|r| matches!(r.get("p"), Some(Value::Path { .. })))
            .collect();
        assert_eq!(paths.len(), 3);
    }

    #[test]
    fn varlen_expand_min_zero_includes_self() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".into(), HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            min: 0,
            max: 1,
            src_alias: Identifier::new("src"),
            path_alias: None,
            path_mode: false,
            dst_alias: Identifier::new("dst"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("src"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert!(result.rows.len() >= 2);
    }

    #[test]
    fn varlen_expand_avoids_cycles() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".into(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, a, "R".into(), HashMap::new())
            .unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::VarLenExpand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            min: 1,
            max: 5,
            src_alias: Identifier::new("src"),
            path_alias: None,
            path_mode: false,
            dst_alias: Identifier::new("dst"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("N")),
                alias: Identifier::new("src"),
            }),
            rel_props: None,
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert!(!result.rows.is_empty());
        assert!(result.rows.len() < 100);
    }

    #[test]
    fn shortest_path_finds_path() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".into(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, c, "R".into(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, c, "R".into(), HashMap::new())
            .unwrap();
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));
        start_row.insert("dst".to_string(), Value::Node(c));
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
            .exec_operator("g", &plan, &mut budget, &[start_row], &mut stats)
            .unwrap();
        assert_eq!(rows.len(), 1);
        match &rows[0]["p"] {
            Value::Path { nodes, edges } => {
                assert_eq!(edges.len(), 1);
                assert_eq!(nodes[0], a);
                assert_eq!(nodes[1], c);
            }
            other => panic!("expected Path, got {:?}", other),
        }
    }

    #[test]
    fn shortest_path_all_paths() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        let c = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        let d = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, b, "R".into(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", b, c, "R".into(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", a, d, "R".into(), HashMap::new())
            .unwrap();
        store
            .create_edge("g", d, c, "R".into(), HashMap::new())
            .unwrap();
        let mut start_row = Row::new();
        start_row.insert("src".to_string(), Value::Node(a));
        start_row.insert("dst".to_string(), Value::Node(c));
        let executor = QueryExecutor::new(&store);
        let plan = Operator::ShortestPath {
            src_alias: Identifier::new("src"),
            dst_alias: Identifier::new("dst"),
            rel_types: vec![],
            direction: Direction::Outgoing,
            min: 1,
            max: 10,
            path_alias: Identifier::new("p"),
            all_paths: true,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator("g", &plan, &mut budget, &[start_row], &mut stats)
            .unwrap();
        assert_eq!(rows.len(), 2);
        for row in &rows {
            match &row["p"] {
                Value::Path { edges, .. } => assert_eq!(edges.len(), 2),
                other => panic!("expected Path, got {:?}", other),
            }
        }
    }

    #[test]
    fn shortest_path_no_path_exists() {
        let store = make_store();
        let a = store
            .create_node("g", vec!["N".into()], HashMap::new())
            .unwrap();
        let b = store
            .create_node("g", vec!["N".into()], HashMap::new())
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
            max: 5,
            path_alias: Identifier::new("p"),
            all_paths: false,
        };
        let mut budget = ExecutionBudget::unlimited();
        let mut stats = MutationStats::default();
        let rows = executor
            .exec_operator("g", &plan, &mut budget, &[start_row], &mut stats)
            .unwrap();
        assert_eq!(rows.len(), 0);
    }

    #[test]
    fn cmp_values_cross_type_ordering() {
        use std::cmp::Ordering;
        assert_eq!(
            cmp_values(&Value::Str("z".into()), &Value::Bool(true)),
            Ordering::Less
        );
        assert_eq!(
            cmp_values(&Value::Bool(false), &Value::Str("a".into())),
            Ordering::Greater
        );
    }

    #[test]
    fn cmp_values_nan_sorts_after_regular_float() {
        use std::cmp::Ordering;
        let nan = Value::Float(f64::NAN);
        let regular = Value::Float(1.0);
        assert_eq!(cmp_values(&nan, &regular), Ordering::Greater);
        assert_eq!(cmp_values(&regular, &nan), Ordering::Less);
    }

    #[test]
    fn cmp_values_list_lexicographic() {
        use std::cmp::Ordering;
        let short = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let long = Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]);
        let diff = Value::List(vec![Value::Int(1), Value::Int(3)]);
        assert_eq!(cmp_values(&short, &long), Ordering::Less);
        assert_eq!(cmp_values(&short, &diff), Ordering::Less);
        assert_eq!(cmp_values(&short, &short.clone()), Ordering::Equal);
    }

    #[test]
    fn cmp_values_null_sorts_last() {
        use std::cmp::Ordering;
        assert_eq!(cmp_values(&Value::Null, &Value::Int(0)), Ordering::Greater);
        assert_eq!(
            cmp_values(&Value::Null, &Value::Str("z".into())),
            Ordering::Greater
        );
        assert_eq!(cmp_values(&Value::Bool(true), &Value::Null), Ordering::Less);
        assert_eq!(cmp_values(&Value::Null, &Value::Null), Ordering::Equal);
    }

    #[test]
    fn cmp_values_time_utc_normalized() {
        use std::cmp::Ordering;
        let a = Value::Time(jiff::civil::Time::new(10, 35, 0, 0).unwrap(), -28800);
        let d = Value::Time(jiff::civil::Time::new(12, 35, 15, 0).unwrap(), 18000);
        assert_eq!(
            cmp_values(&a, &d),
            Ordering::Greater,
            "A(18:35Z) should be > D(07:35Z)"
        );
        assert_eq!(
            cmp_values(&d, &a),
            Ordering::Less,
            "D(07:35Z) should be < A(18:35Z)"
        );
        let t1 = Value::Time(jiff::civil::Time::new(14, 0, 0, 0).unwrap(), 7200);
        let t2 = Value::Time(jiff::civil::Time::new(13, 0, 0, 0).unwrap(), 3600);
        assert_eq!(cmp_values(&t1, &t2), Ordering::Equal);
    }

    #[test]
    fn unwind_list() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::List(vec![Expr::Integer(1), Expr::Integer(2), Expr::Integer(3)]),
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 3);
        let values: Vec<&Value> = result.rows.iter().map(|r| &r["x"]).collect();
        assert_eq!(values, vec![&Value::Int(1), &Value::Int(2), &Value::Int(3)]);
    }

    #[test]
    fn unwind_null_produces_zero_rows() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::Null,
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn unwind_empty_list_produces_zero_rows() {
        let store = make_store();
        store.create_node("g", vec![], HashMap::new()).unwrap();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Unwind {
            list_expr: Expr::List(vec![]),
            alias: Identifier::new("x"),
            child: Box::new(Operator::NodeScan {
                label: None,
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows.len(), 0);
    }

    #[test]
    fn e2e_varlen_expand_through_planner() {
        use crate::parser::parse_query;
        use crate::planner::QueryPlanner;
        let store = make_store();
        let setup = "CREATE (a {name: 'A'}), (b {name: 'B'}), (c {name: 'C'}), (d {name: 'D'}) CREATE (a)-[:CONTAINS]->(b), (b)-[:CONTAINS]->(c), (c)-[:CONTAINS]->(d)";
        let ast = parse_query(setup).unwrap();
        let planner = QueryPlanner::new(&store, "g");
        let plan = planner.plan(&ast).unwrap();
        let mut budget = ExecutionBudget::unlimited();
        QueryExecutor::new(&store)
            .execute("g", &plan, &mut budget)
            .unwrap();
        let query = "MATCH (a {name: 'A'})-[*]->(x) RETURN x";
        let ast2 = parse_query(query).unwrap();
        let plan2 = planner.plan(&ast2).unwrap();
        let mut budget2 = ExecutionBudget::unlimited();
        let result = QueryExecutor::new(&store)
            .execute("g", &plan2, &mut budget2)
            .unwrap();
        assert_eq!(result.rows.len(), 3, "Expected 3 rows: B, C, D");
    }

    #[test]
    fn e2e_create_path_variable_returns_path() {
        use crate::parser::parse_query;
        use crate::planner::QueryPlanner;
        let store = make_store();
        let planner = QueryPlanner::new(&store, "g");
        let query = "CREATE p = (a:X)-[r:Y]->(b:Z) RETURN p";
        let ast = parse_query(query).unwrap();
        let plan = planner.plan(&ast).unwrap();
        let mut budget = ExecutionBudget::unlimited();
        let result = QueryExecutor::new(&store)
            .execute("g", &plan, &mut budget)
            .unwrap();
        assert_eq!(
            result.rows.len(),
            1,
            "CREATE should produce exactly one row"
        );
        let row = &result.rows[0];
        let p = row.get("p").expect("row should contain path variable 'p'");
        match p {
            Value::Path { nodes, edges } => {
                assert_eq!(nodes.len(), 2, "path should have 2 nodes");
                assert_eq!(edges.len(), 1, "path should have 1 edge");
            }
            other => panic!("expected Value::Path, got {:?}", other),
        }
    }
}
