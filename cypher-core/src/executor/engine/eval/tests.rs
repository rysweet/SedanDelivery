//! Tests for the `eval` submodule.
//!
//! Section 1 (`eval_property_on_node` … `build_filter_ctx_basic`) is the
//! original 32-test corpus moved verbatim from the previous `eval.rs`.
//!
//! Section 2 (`null_*` / `division_*` / `case_*` / `in_*` …) targets
//! low-coverage branches of the evaluator: NULL handling, division/modulo
//! by zero, type-coercion edges, regex errors, list/map slice/access edges,
//! Kleene-logic AND/OR with NULL, list comprehension predicate filtering,
//! and string ops on NULL.

use super::*;
use crate::ast::{BinaryOp, Identifier, UnaryOp};
use crate::storage::{GraphStore, InMemoryGraphStore};
use indexmap::IndexMap;

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

#[test]
fn eval_property_on_node() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let nid = store
        .create_node("g", vec!["Person".to_string()], props)
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("n".to_string(), Value::Node(nid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::Property(
        Box::new(Expr::Variable(Identifier::new("n"))),
        "name".into(),
    );
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Str("Alice".into()));
}

#[test]
fn eval_property_on_edge() {
    let store = make_store();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let mut props = IndexMap::new();
    props.insert("weight".to_string(), Value::Int(42));
    let eid = store
        .create_edge("g", a, b, "R".to_string(), props)
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("r".to_string(), Value::Edge(eid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::Property(
        Box::new(Expr::Variable(Identifier::new("r"))),
        "weight".into(),
    );
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Int(42));
}

#[test]
fn eval_property_on_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("n".to_string(), Value::Null);

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::Property(
        Box::new(Expr::Variable(Identifier::new("n"))),
        "name".into(),
    );
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn eval_property_on_map() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut map = indexmap::IndexMap::new();
    map.insert("key".to_string().into_boxed_str(), Value::Int(99));
    let mut row = Row::new();
    row.insert("m".to_string(), Value::Map(map));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::Property(Box::new(Expr::Variable(Identifier::new("m"))), "key".into());
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Int(99));
}

#[test]
fn eval_is_null_and_is_not_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Null);
    row.insert("y".to_string(), Value::Int(1));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let is_null_expr = Expr::IsNull(Box::new(Expr::Variable(Identifier::new("x"))));
    assert_eq!(
        executor
            .eval_with_storage("g", &is_null_expr, &row, &ctx)
            .unwrap(),
        Value::Bool(true)
    );

    let is_not_null_expr = Expr::IsNotNull(Box::new(Expr::Variable(Identifier::new("y"))));
    assert_eq!(
        executor
            .eval_with_storage("g", &is_not_null_expr, &row, &ctx)
            .unwrap(),
        Value::Bool(true)
    );
}

#[test]
fn eval_binary_op_addition() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("a".to_string(), Value::Int(10));
    row.insert("b".to_string(), Value::Int(20));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::BinaryOp {
        op: BinaryOp::Add,
        left: Box::new(Expr::Variable(Identifier::new("a"))),
        right: Box::new(Expr::Variable(Identifier::new("b"))),
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Int(30));
}

#[test]
fn eval_has_label_on_node() {
    let store = make_store();
    let nid = store
        .create_node("g", vec!["Person".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("n".to_string(), Value::Node(nid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::HasLabel(
        Box::new(Expr::Variable(Identifier::new("n"))),
        crate::ast::Label::new("Person"),
    );
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Bool(true));

    let expr_wrong = Expr::HasLabel(
        Box::new(Expr::Variable(Identifier::new("n"))),
        crate::ast::Label::new("Movie"),
    );
    let result2 = executor
        .eval_with_storage("g", &expr_wrong, &row, &ctx)
        .unwrap();
    assert_eq!(result2, Value::Bool(false));
}

#[test]
fn eval_function_id() {
    let store = make_store();
    let nid = store.create_node("g", vec![], IndexMap::new()).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("n".to_string(), Value::Node(nid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "id".into(),
        args: vec![Expr::Variable(Identifier::new("n"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Int(nid.0 as i64));
}

#[test]
fn eval_function_labels() {
    let store = make_store();
    let nid = store
        .create_node("g", vec!["A".to_string(), "B".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("n".to_string(), Value::Node(nid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "labels".into(),
        args: vec![Expr::Variable(Identifier::new("n"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    match result {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
        }
        other => panic!("expected List, got {:?}", other),
    }
}

#[test]
fn eval_function_type() {
    let store = make_store();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let eid = store
        .create_edge("g", a, b, "KNOWS".to_string(), IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("r".to_string(), Value::Edge(eid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "type".into(),
        args: vec![Expr::Variable(Identifier::new("r"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Str("KNOWS".into()));
}

#[test]
fn eval_function_properties() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("name".to_string(), Value::Str("Alice".into()));
    let nid = store
        .create_node("g", vec!["Person".to_string()], props)
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("n".to_string(), Value::Node(nid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "properties".into(),
        args: vec![Expr::Variable(Identifier::new("n"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    match result {
        Value::Map(m) => {
            assert_eq!(m.get("name"), Some(&Value::Str("Alice".into())));
        }
        other => panic!("expected Map, got {:?}", other),
    }
}

#[test]
fn eval_function_keys() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert("a".to_string(), Value::Int(1));
    props.insert("b".to_string(), Value::Int(2));
    let nid = store.create_node("g", vec![], props).unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("n".to_string(), Value::Node(nid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "keys".into(),
        args: vec![Expr::Variable(Identifier::new("n"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    match result {
        Value::List(items) => {
            assert_eq!(items.len(), 2);
        }
        other => panic!("expected List, got {:?}", other),
    }
}

#[test]
fn eval_case_expression() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Int(1));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::Case {
        subject: Some(Box::new(Expr::Variable(Identifier::new("x")))),
        alternatives: vec![
            (Expr::Integer(1), Expr::Str("one".into())),
            (Expr::Integer(2), Expr::Str("two".into())),
        ],
        default: Some(Box::new(Expr::Str("other".into()))),
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Str("one".into()));
}

#[test]
fn eval_case_no_subject() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Int(5));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::Case {
        subject: None,
        alternatives: vec![(
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(Expr::Variable(Identifier::new("x"))),
                right: Box::new(Expr::Integer(3)),
            },
            Expr::Str("big".into()),
        )],
        default: Some(Box::new(Expr::Str("small".into()))),
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Str("big".into()));
}

#[test]
fn eval_case_default_fallback() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Int(99));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    // No matching alternative
    let expr = Expr::Case {
        subject: Some(Box::new(Expr::Variable(Identifier::new("x")))),
        alternatives: vec![(Expr::Integer(1), Expr::Str("one".into()))],
        default: None,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn eval_list_literal() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::List(vec![Expr::Integer(1), Expr::Integer(2), Expr::Integer(3)]);
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(
        result,
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)])
    );
}

#[test]
fn eval_map_literal() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::MapLiteral(vec![(smol_str::SmolStr::new("a"), Expr::Integer(1))]);
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    match result {
        Value::Map(m) => {
            assert_eq!(m.get("a"), Some(&Value::Int(1)));
        }
        other => panic!("expected Map, got {:?}", other),
    }
}

#[test]
fn eval_subscript_list() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert(
        "xs".to_string(),
        Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]),
    );

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::Subscript(
        Box::new(Expr::Variable(Identifier::new("xs"))),
        Box::new(Expr::Integer(1)),
    );
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Int(20));
}

#[test]
fn eval_subscript_negative_index() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert(
        "xs".to_string(),
        Value::List(vec![Value::Int(10), Value::Int(20), Value::Int(30)]),
    );

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::Subscript(
        Box::new(Expr::Variable(Identifier::new("xs"))),
        Box::new(Expr::Integer(-1)),
    );
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Int(30));
}

#[test]
fn eval_subscript_out_of_bounds() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("xs".to_string(), Value::List(vec![Value::Int(10)]));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::Subscript(
        Box::new(Expr::Variable(Identifier::new("xs"))),
        Box::new(Expr::Integer(5)),
    );
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn eval_startnode_endnode() {
    let store = make_store();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let eid = store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("r".to_string(), Value::Edge(eid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();

    let start_expr = Expr::FunctionCall {
        name: "startNode".into(),
        args: vec![Expr::Variable(Identifier::new("r"))],
        distinct: false,
    };
    let result = executor
        .eval_with_storage("g", &start_expr, &row, &ctx)
        .unwrap();
    assert_eq!(result, Value::Node(a));

    let end_expr = Expr::FunctionCall {
        name: "endNode".into(),
        args: vec![Expr::Variable(Identifier::new("r"))],
        distinct: false,
    };
    let result2 = executor
        .eval_with_storage("g", &end_expr, &row, &ctx)
        .unwrap();
    assert_eq!(result2, Value::Node(b));
}

#[test]
fn eval_nodes_and_relationships_on_path() {
    let store = make_store();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let eid = store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert(
        "p".to_string(),
        Value::Path {
            nodes: vec![a, b],
            edges: vec![eid],
        },
    );

    let ctx = executor.build_filter_ctx("g", &row).unwrap();

    let nodes_expr = Expr::FunctionCall {
        name: "nodes".into(),
        args: vec![Expr::Variable(Identifier::new("p"))],
        distinct: false,
    };
    let result = executor
        .eval_with_storage("g", &nodes_expr, &row, &ctx)
        .unwrap();
    assert_eq!(result, Value::List(vec![Value::Node(a), Value::Node(b)]));

    let rels_expr = Expr::FunctionCall {
        name: "relationships".into(),
        args: vec![Expr::Variable(Identifier::new("p"))],
        distinct: false,
    };
    let result2 = executor
        .eval_with_storage("g", &rels_expr, &row, &ctx)
        .unwrap();
    assert_eq!(result2, Value::List(vec![Value::Edge(eid)]));
}

#[test]
fn eval_length_on_path() {
    let store = make_store();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let eid = store
        .create_edge("g", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert(
        "p".to_string(),
        Value::Path {
            nodes: vec![a, b],
            edges: vec![eid],
        },
    );

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "length".into(),
        args: vec![Expr::Variable(Identifier::new("p"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Int(1));
}

#[test]
fn eval_list_comprehension() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert(
        "xs".to_string(),
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
    );

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::ListComp {
        variable: Identifier::new("x"),
        list: Box::new(Expr::Variable(Identifier::new("xs"))),
        filter: Some(Box::new(Expr::BinaryOp {
            op: BinaryOp::Gt,
            left: Box::new(Expr::Variable(Identifier::new("x"))),
            right: Box::new(Expr::Integer(1)),
        })),
        map: Box::new(Expr::BinaryOp {
            op: BinaryOp::Mul,
            left: Box::new(Expr::Variable(Identifier::new("x"))),
            right: Box::new(Expr::Integer(10)),
        }),
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::List(vec![Value::Int(20), Value::Int(30)]));
}

#[test]
fn eval_function_id_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Null);

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "id".into(),
        args: vec![Expr::Variable(Identifier::new("x"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn eval_function_labels_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Null);

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "labels".into(),
        args: vec![Expr::Variable(Identifier::new("x"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn eval_function_type_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Null);

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "type".into(),
        args: vec![Expr::Variable(Identifier::new("x"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn eval_function_nodes_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Null);

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "nodes".into(),
        args: vec![Expr::Variable(Identifier::new("x"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn eval_function_relationships_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Null);

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "relationships".into(),
        args: vec![Expr::Variable(Identifier::new("x"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Null);
}

#[test]
fn eval_function_id_on_edge() {
    let store = make_store();
    let a = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let b = store.create_node("g", vec![], IndexMap::new()).unwrap();
    let eid = store
        .create_edge("g", a, b, "KNOWS".to_string(), IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("r".to_string(), Value::Edge(eid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "id".into(),
        args: vec![Expr::Variable(Identifier::new("r"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    assert_eq!(result, Value::Int(eid.0 as i64));
}

#[test]
fn eval_function_labels_multi() {
    let store = make_store();
    let nid = store
        .create_node("g", vec!["A".to_string(), "B".to_string()], IndexMap::new())
        .unwrap();

    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("n".to_string(), Value::Node(nid));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let expr = Expr::FunctionCall {
        name: "labels".into(),
        args: vec![Expr::Variable(Identifier::new("n"))],
        distinct: false,
    };
    let result = executor.eval_with_storage("g", &expr, &row, &ctx).unwrap();
    match result {
        Value::List(items) => {
            let strs: Vec<&str> = items.iter().map(|v| v.as_str().unwrap()).collect();
            assert!(strs.contains(&"A"));
            assert!(strs.contains(&"B"));
            assert_eq!(strs.len(), 2);
        }
        other => panic!("expected List, got {:?}", other),
    }
}

#[test]
fn build_filter_ctx_basic() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("a".to_string(), Value::Int(1));
    row.insert("b".to_string(), Value::Str("hello".into()));

    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    assert_eq!(ctx.get("a"), Some(&&Value::Int(1)));
    assert_eq!(ctx.get("b"), Some(&&Value::Str("hello".into())));
}

// ── Section 2: low-coverage edge cases ────────────────────────────────────────

/// Helper: evaluate `expr` against an executor with `row`.
fn eval(executor: &QueryExecutor<'_>, row: &Row, expr: &Expr) -> Value {
    let ctx = executor.build_filter_ctx("g", row).unwrap();
    executor.eval_with_storage("g", expr, row, &ctx).unwrap()
}

fn binop(op: BinaryOp, l: Expr, r: Expr) -> Expr {
    Expr::BinaryOp {
        op,
        left: Box::new(l),
        right: Box::new(r),
    }
}

#[test]
fn null_addition_on_either_side_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();

    let l_null = binop(BinaryOp::Add, Expr::Null, Expr::Integer(1));
    assert_eq!(eval(&executor, &row, &l_null), Value::Null);

    let r_null = binop(BinaryOp::Add, Expr::Integer(1), Expr::Null);
    assert_eq!(eval(&executor, &row, &r_null), Value::Null);
}

#[test]
fn null_subtraction_multiplication_division_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();

    for op in [
        BinaryOp::Sub,
        BinaryOp::Mul,
        BinaryOp::Div,
        BinaryOp::Mod,
        BinaryOp::Pow,
    ] {
        let e = binop(op, Expr::Null, Expr::Integer(2));
        assert_eq!(
            eval(&executor, &row, &e),
            Value::Null,
            "op={:?} (null,int)",
            op
        );
        let e2 = binop(op, Expr::Integer(2), Expr::Null);
        assert_eq!(
            eval(&executor, &row, &e2),
            Value::Null,
            "op={:?} (int,null)",
            op
        );
    }
}

#[test]
fn integer_division_by_zero_is_error() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let e = binop(BinaryOp::Div, Expr::Integer(1), Expr::Integer(0));
    let r = executor.eval_with_storage("g", &e, &row, &ctx);
    assert!(r.is_err(), "1/0 should error, got {:?}", r);
}

#[test]
fn float_division_by_zero_yields_infinity_or_nan() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = binop(BinaryOp::Div, Expr::Float(1.0), Expr::Float(0.0));
    match eval(&executor, &row, &e) {
        Value::Float(f) => assert!(f.is_infinite() || f.is_nan()),
        other => panic!("expected Float, got {:?}", other),
    }
}

#[test]
fn integer_modulo_by_zero_is_error() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let e = binop(BinaryOp::Mod, Expr::Integer(5), Expr::Integer(0));
    let r = executor.eval_with_storage("g", &e, &row, &ctx);
    assert!(r.is_err(), "5%0 should error, got {:?}", r);
}

#[test]
fn int_plus_float_promotes_to_float() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = binop(BinaryOp::Add, Expr::Integer(1), Expr::Float(2.5));
    match eval(&executor, &row, &e) {
        Value::Float(f) => assert!((f - 3.5).abs() < 1e-9),
        other => panic!("expected Float, got {:?}", other),
    }
}

#[test]
fn string_plus_string_concatenates() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = binop(
        BinaryOp::Add,
        Expr::Str("foo".into()),
        Expr::Str("bar".into()),
    );
    assert_eq!(eval(&executor, &row, &e), Value::Str("foobar".into()));
}

#[test]
fn unary_minus_on_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = Expr::UnaryOp {
        op: UnaryOp::Neg,
        expr: Box::new(Expr::Null),
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn unary_minus_on_int_negates() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = Expr::UnaryOp {
        op: UnaryOp::Neg,
        expr: Box::new(Expr::Integer(7)),
    };
    assert_eq!(eval(&executor, &row, &e), Value::Int(-7));
}

#[test]
fn list_subscript_negative_out_of_range_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert(
        "xs".to_string(),
        Value::List(vec![Value::Int(1), Value::Int(2)]),
    );
    let e = Expr::Subscript(
        Box::new(Expr::Variable(Identifier::new("xs"))),
        Box::new(Expr::Integer(-99)),
    );
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn list_subscript_with_null_index_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("xs".to_string(), Value::List(vec![Value::Int(1)]));
    let e = Expr::Subscript(
        Box::new(Expr::Variable(Identifier::new("xs"))),
        Box::new(Expr::Null),
    );
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn map_missing_key_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut m = indexmap::IndexMap::new();
    m.insert(Box::from("present"), Value::Int(1));
    let mut row = Row::new();
    row.insert("m".to_string(), Value::Map(m));

    let e = Expr::Property(
        Box::new(Expr::Variable(Identifier::new("m"))),
        "absent".into(),
    );
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn nested_map_access_through_null_intermediate_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut inner = indexmap::IndexMap::new();
    inner.insert(Box::from("c"), Value::Int(42));
    let mut outer = indexmap::IndexMap::new();
    outer.insert(Box::from("a"), Value::Map(inner));
    // outer.b does not exist → property of null
    let mut row = Row::new();
    row.insert("m".to_string(), Value::Map(outer));

    // m.b.c — m.b is null → null.c is null
    let e = Expr::Property(
        Box::new(Expr::Property(
            Box::new(Expr::Variable(Identifier::new("m"))),
            "b".into(),
        )),
        "c".into(),
    );
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn case_no_match_no_else_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Int(99));
    let e = Expr::Case {
        subject: Some(Box::new(Expr::Variable(Identifier::new("x")))),
        alternatives: vec![(Expr::Integer(1), Expr::Str("one".into()))],
        default: None,
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn case_no_match_with_else_returns_else() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Int(99));
    let e = Expr::Case {
        subject: Some(Box::new(Expr::Variable(Identifier::new("x")))),
        alternatives: vec![(Expr::Integer(1), Expr::Str("one".into()))],
        default: Some(Box::new(Expr::Str("default".into()))),
    };
    assert_eq!(eval(&executor, &row, &e), Value::Str("default".into()));
}

#[test]
fn case_generic_no_when_matches_no_else_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = Expr::Case {
        subject: None,
        alternatives: vec![(Expr::Bool(false), Expr::Str("never".into()))],
        default: None,
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn function_id_wrong_arity_is_error() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let e = Expr::FunctionCall {
        name: "id".into(),
        args: vec![],
        distinct: false,
    };
    assert!(executor.eval_with_storage("g", &e, &row, &ctx).is_err());

    let e2 = Expr::FunctionCall {
        name: "id".into(),
        args: vec![Expr::Integer(1), Expr::Integer(2)],
        distinct: false,
    };
    assert!(executor.eval_with_storage("g", &e2, &row, &ctx).is_err());
}

#[test]
fn function_labels_wrong_arity_is_error() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let ctx = executor.build_filter_ctx("g", &row).unwrap();
    let e = Expr::FunctionCall {
        name: "labels".into(),
        args: vec![],
        distinct: false,
    };
    assert!(executor.eval_with_storage("g", &e, &row, &ctx).is_err());
}

#[test]
fn comparison_int_to_string_yields_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    // Cypher: incompatible types → NULL (not error) for ordering comparisons
    let e = binop(BinaryOp::Lt, Expr::Integer(1), Expr::Str("x".into()));
    let v = eval(&executor, &row, &e);
    assert!(matches!(v, Value::Null), "got {:?}", v);
}

#[test]
fn equality_int_to_string_is_false() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = binop(BinaryOp::Eq, Expr::Integer(1), Expr::Str("1".into()));
    assert_eq!(eval(&executor, &row, &e), Value::Bool(false));
}

#[test]
fn in_empty_list_is_false() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = binop(BinaryOp::In, Expr::Integer(1), Expr::List(vec![]));
    assert_eq!(eval(&executor, &row, &e), Value::Bool(false));
}

#[test]
fn in_list_containing_null_with_no_match_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    // 99 IN [1, null] → NULL  (Kleene three-valued IN)
    let e = binop(
        BinaryOp::In,
        Expr::Integer(99),
        Expr::List(vec![Expr::Integer(1), Expr::Null]),
    );
    let v = eval(&executor, &row, &e);
    assert!(matches!(v, Value::Null), "got {:?}", v);
}

#[test]
fn in_list_with_match_is_true_even_if_null_present() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = binop(
        BinaryOp::In,
        Expr::Integer(1),
        Expr::List(vec![Expr::Integer(1), Expr::Null]),
    );
    assert_eq!(eval(&executor, &row, &e), Value::Bool(true));
}

#[test]
fn list_comprehension_filter_eliminates_all_yields_empty_list() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert(
        "xs".to_string(),
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
    );
    let e = Expr::ListComp {
        variable: Identifier::new("x"),
        list: Box::new(Expr::Variable(Identifier::new("xs"))),
        filter: Some(Box::new(Expr::Bool(false))),
        map: Box::new(Expr::Variable(Identifier::new("x"))),
    };
    assert_eq!(eval(&executor, &row, &e), Value::List(vec![]));
}

#[test]
fn list_comprehension_on_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("xs".to_string(), Value::Null);
    let e = Expr::ListComp {
        variable: Identifier::new("x"),
        list: Box::new(Expr::Variable(Identifier::new("xs"))),
        filter: None,
        map: Box::new(Expr::Variable(Identifier::new("x"))),
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn nodes_on_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("p".to_string(), Value::Null);
    let e = Expr::FunctionCall {
        name: "nodes".into(),
        args: vec![Expr::Variable(Identifier::new("p"))],
        distinct: false,
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn relationships_on_null_returns_null_again() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("p".to_string(), Value::Null);
    let e = Expr::FunctionCall {
        name: "relationships".into(),
        args: vec![Expr::Variable(Identifier::new("p"))],
        distinct: false,
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn startnode_on_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("r".to_string(), Value::Null);
    let e = Expr::FunctionCall {
        name: "startNode".into(),
        args: vec![Expr::Variable(Identifier::new("r"))],
        distinct: false,
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn string_starts_with_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = binop(BinaryOp::StartsWith, Expr::Null, Expr::Str("abc".into()));
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn string_contains_with_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = binop(BinaryOp::Contains, Expr::Str("abc".into()), Expr::Null);
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn string_ends_with_returns_bool() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = binop(
        BinaryOp::EndsWith,
        Expr::Str("hello world".into()),
        Expr::Str("world".into()),
    );
    assert_eq!(eval(&executor, &row, &e), Value::Bool(true));
}

#[test]
fn and_or_kleene_logic_with_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();

    // TRUE OR NULL = TRUE
    assert_eq!(
        eval(
            &executor,
            &row,
            &binop(BinaryOp::Or, Expr::Bool(true), Expr::Null),
        ),
        Value::Bool(true)
    );
    // FALSE AND NULL = FALSE
    assert_eq!(
        eval(
            &executor,
            &row,
            &binop(BinaryOp::And, Expr::Bool(false), Expr::Null),
        ),
        Value::Bool(false)
    );
    // FALSE OR NULL = NULL
    assert_eq!(
        eval(
            &executor,
            &row,
            &binop(BinaryOp::Or, Expr::Bool(false), Expr::Null),
        ),
        Value::Null
    );
    // TRUE AND NULL = NULL
    assert_eq!(
        eval(
            &executor,
            &row,
            &binop(BinaryOp::And, Expr::Bool(true), Expr::Null),
        ),
        Value::Null
    );
    // NULL OR NULL = NULL
    assert_eq!(
        eval(
            &executor,
            &row,
            &binop(BinaryOp::Or, Expr::Null, Expr::Null),
        ),
        Value::Null
    );
}

#[test]
fn not_on_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let row = Row::new();
    let e = Expr::UnaryOp {
        op: UnaryOp::Not,
        expr: Box::new(Expr::Null),
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn properties_on_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Null);
    let e = Expr::FunctionCall {
        name: "properties".into(),
        args: vec![Expr::Variable(Identifier::new("x"))],
        distinct: false,
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn keys_on_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("x".to_string(), Value::Null);
    let e = Expr::FunctionCall {
        name: "keys".into(),
        args: vec![Expr::Variable(Identifier::new("x"))],
        distinct: false,
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}

#[test]
fn reduce_over_empty_list_returns_initial() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("xs".to_string(), Value::List(vec![]));
    let e = Expr::Reduce {
        accumulator: Identifier::new("acc"),
        initial: Box::new(Expr::Integer(0)),
        variable: Identifier::new("x"),
        list: Box::new(Expr::Variable(Identifier::new("xs"))),
        expression: Box::new(binop(
            BinaryOp::Add,
            Expr::Variable(Identifier::new("acc")),
            Expr::Variable(Identifier::new("x")),
        )),
    };
    assert_eq!(eval(&executor, &row, &e), Value::Int(0));
}

#[test]
fn reduce_sums_list() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert(
        "xs".to_string(),
        Value::List(vec![Value::Int(1), Value::Int(2), Value::Int(3)]),
    );
    let e = Expr::Reduce {
        accumulator: Identifier::new("acc"),
        initial: Box::new(Expr::Integer(0)),
        variable: Identifier::new("x"),
        list: Box::new(Expr::Variable(Identifier::new("xs"))),
        expression: Box::new(binop(
            BinaryOp::Add,
            Expr::Variable(Identifier::new("acc")),
            Expr::Variable(Identifier::new("x")),
        )),
    };
    assert_eq!(eval(&executor, &row, &e), Value::Int(6));
}

#[test]
fn reduce_on_null_returns_null() {
    let store = make_store();
    let executor = QueryExecutor::new(&store);
    let mut row = Row::new();
    row.insert("xs".to_string(), Value::Null);
    let e = Expr::Reduce {
        accumulator: Identifier::new("acc"),
        initial: Box::new(Expr::Integer(0)),
        variable: Identifier::new("x"),
        list: Box::new(Expr::Variable(Identifier::new("xs"))),
        expression: Box::new(Expr::Variable(Identifier::new("acc"))),
    };
    assert_eq!(eval(&executor, &row, &e), Value::Null);
}
