//! Direct unit tests for all 8 compiler::helpers submodules.
//!
//! These tests exercise helper functions directly rather than via end-to-end
//! traversal compilation, targeting uncovered branches to raise line coverage
//! from ~21% toward 70%+.

use std::collections::{HashMap, HashSet};

use cypher_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType, UnaryOp};
use cypher_core::planner::{AggFunction, CreateEdgeSpec, CreatePattern, Operator};
use cypher_core::types::Value;
use indexmap::IndexMap;
use smol_str::SmolStr;

use crate::bytecode::{GremlinStep, Predicate};
use crate::types::GremlinValue;

use super::helpers::*;

// ═══════════════════════════════════════════════════════════════════════════════
// conversion.rs tests
// ═══════════════════════════════════════════════════════════════════════════════

mod conversion_tests {
    use super::*;

    // ── gremlin_value_to_expr ────────────────────────────────────────────

    #[test]
    fn null_to_expr() {
        assert_eq!(gremlin_value_to_expr(&GremlinValue::Null), Expr::Null);
    }

    #[test]
    fn bool_true_to_expr() {
        assert_eq!(
            gremlin_value_to_expr(&GremlinValue::Bool(true)),
            Expr::Bool(true)
        );
    }

    #[test]
    fn bool_false_to_expr() {
        assert_eq!(
            gremlin_value_to_expr(&GremlinValue::Bool(false)),
            Expr::Bool(false)
        );
    }

    #[test]
    fn integer_to_expr() {
        assert_eq!(
            gremlin_value_to_expr(&GremlinValue::Integer(42)),
            Expr::Integer(42)
        );
    }

    #[test]
    fn float_to_expr() {
        assert_eq!(
            gremlin_value_to_expr(&GremlinValue::Float(3.14)),
            Expr::Float(3.14)
        );
    }

    #[test]
    fn plain_string_to_expr() {
        let gv = GremlinValue::String("hello".into());
        assert_eq!(gremlin_value_to_expr(&gv), Expr::Str("hello".into()));
    }

    #[test]
    fn datetime_prefixed_string_to_expr() {
        let gv = GremlinValue::String("__gremlin_dt__:2024-01-01T00:00:00Z".into());
        match gremlin_value_to_expr(&gv) {
            Expr::FunctionCall { name, args, .. } => {
                assert_eq!(name.as_str(), "gremlin_make_datetime");
                assert_eq!(args.len(), 1);
                assert_eq!(args[0], Expr::Str("2024-01-01T00:00:00Z".into()));
            }
            other => panic!("expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn uuid_prefixed_string_to_expr() {
        let gv = GremlinValue::String("__gremlin_uuid__:abc-123".into());
        match gremlin_value_to_expr(&gv) {
            Expr::FunctionCall { name, args, .. } => {
                assert_eq!(name.as_str(), "gremlin_make_uuid");
                assert_eq!(args[0], Expr::Str("abc-123".into()));
            }
            other => panic!("expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn list_to_expr() {
        let gv = GremlinValue::List(vec![GremlinValue::Integer(1), GremlinValue::Integer(2)]);
        match gremlin_value_to_expr(&gv) {
            Expr::List(items) => {
                assert_eq!(items.len(), 2);
                assert_eq!(items[0], Expr::Integer(1));
                assert_eq!(items[1], Expr::Integer(2));
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn map_to_expr() {
        let mut map = IndexMap::new();
        map.insert("k".to_string(), GremlinValue::Integer(1));
        let gv = GremlinValue::Map(map);
        match gremlin_value_to_expr(&gv) {
            Expr::MapLiteral(entries) => {
                assert_eq!(entries.len(), 1);
                assert_eq!(entries[0].0.as_str(), "k");
                assert_eq!(entries[0].1, Expr::Integer(1));
            }
            other => panic!("expected MapLiteral, got {:?}", other),
        }
    }

    #[test]
    fn vertex_to_expr() {
        let gv = GremlinValue::Vertex {
            id: 7,
            label: "person".into(),
            properties: IndexMap::new(),
        };
        assert_eq!(gremlin_value_to_expr(&gv), Expr::Integer(7));
    }

    #[test]
    fn edge_to_expr() {
        let gv = GremlinValue::Edge {
            id: 3,
            label: "knows".into(),
            in_v: 1,
            out_v: 2,
            properties: IndexMap::new(),
        };
        assert_eq!(gremlin_value_to_expr(&gv), Expr::Integer(3));
    }

    #[test]
    fn path_to_expr() {
        let gv = GremlinValue::Path {
            labels: vec![],
            objects: vec![],
        };
        assert_eq!(gremlin_value_to_expr(&gv), Expr::Null);
    }

    // ── gremlin_value_to_id_expr ─────────────────────────────────────────

    #[test]
    fn id_expr_string_numeric() {
        let gv = GremlinValue::String("42".into());
        assert_eq!(gremlin_value_to_id_expr(&gv), Expr::Integer(42));
    }

    #[test]
    fn id_expr_string_non_numeric() {
        let gv = GremlinValue::String("abc".into());
        assert_eq!(gremlin_value_to_id_expr(&gv), Expr::Str("abc".into()));
    }

    #[test]
    fn id_expr_integer_passthrough() {
        let gv = GremlinValue::Integer(99);
        assert_eq!(gremlin_value_to_id_expr(&gv), Expr::Integer(99));
    }

    // ── gremlin_value_to_value ───────────────────────────────────────────

    #[test]
    fn value_null() {
        assert_eq!(gremlin_value_to_value(&GremlinValue::Null), Value::Null);
    }

    #[test]
    fn value_bool() {
        assert_eq!(
            gremlin_value_to_value(&GremlinValue::Bool(true)),
            Value::Bool(true)
        );
    }

    #[test]
    fn value_integer() {
        assert_eq!(
            gremlin_value_to_value(&GremlinValue::Integer(7)),
            Value::Int(7)
        );
    }

    #[test]
    fn value_float() {
        assert_eq!(
            gremlin_value_to_value(&GremlinValue::Float(1.5)),
            Value::Float(1.5)
        );
    }

    #[test]
    fn value_string() {
        assert_eq!(
            gremlin_value_to_value(&GremlinValue::String("hi".into())),
            Value::Str("hi".into())
        );
    }

    #[test]
    fn value_list() {
        let gv = GremlinValue::List(vec![GremlinValue::Integer(1)]);
        match gremlin_value_to_value(&gv) {
            Value::List(items) => {
                assert_eq!(items.len(), 1);
                assert_eq!(items[0], Value::Int(1));
            }
            other => panic!("expected List, got {:?}", other),
        }
    }

    #[test]
    fn value_map_returns_null() {
        let gv = GremlinValue::Map(IndexMap::new());
        assert_eq!(gremlin_value_to_value(&gv), Value::Null);
    }

    #[test]
    fn value_vertex_returns_null() {
        let gv = GremlinValue::Vertex {
            id: 1,
            label: "x".into(),
            properties: IndexMap::new(),
        };
        assert_eq!(gremlin_value_to_value(&gv), Value::Null);
    }

    #[test]
    fn value_edge_returns_null() {
        let gv = GremlinValue::Edge {
            id: 1,
            label: "x".into(),
            in_v: 0,
            out_v: 0,
            properties: IndexMap::new(),
        };
        assert_eq!(gremlin_value_to_value(&gv), Value::Null);
    }

    #[test]
    fn value_path_returns_null() {
        let gv = GremlinValue::Path {
            labels: vec![],
            objects: vec![],
        };
        assert_eq!(gremlin_value_to_value(&gv), Value::Null);
    }

    // ── is_gremlin_id_key ────────────────────────────────────────────────

    #[test]
    fn id_key_tilde_id() {
        assert!(is_gremlin_id_key("~id"));
    }

    #[test]
    fn id_key_t_dot_id() {
        assert!(is_gremlin_id_key("T.id"));
    }

    #[test]
    fn id_key_bracket_id() {
        assert!(is_gremlin_id_key("t[id]"));
    }

    #[test]
    fn id_key_non_id() {
        assert!(!is_gremlin_id_key("name"));
    }

    // ── gremlin_has_key_expr ─────────────────────────────────────────────

    #[test]
    fn has_key_expr_id() {
        let alias = Identifier::new("v");
        let expr = gremlin_has_key_expr("~id", &alias);
        match expr {
            Expr::FunctionCall { name, args, .. } => {
                assert_eq!(name.as_str(), "id");
                assert_eq!(args[0], Expr::Variable(Identifier::new("v")));
            }
            other => panic!("expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn has_key_expr_property() {
        let alias = Identifier::new("v");
        let expr = gremlin_has_key_expr("name", &alias);
        match expr {
            Expr::Property(base, key) => {
                assert_eq!(*base, Expr::Variable(Identifier::new("v")));
                assert_eq!(key.as_str(), "name");
            }
            other => panic!("expected Property, got {:?}", other),
        }
    }

    // ── make_by_expr ─────────────────────────────────────────────────────

    #[test]
    fn make_by_id() {
        let alias = Identifier::new("v");
        for key in &["~id", "T.id", "t[id]"] {
            match make_by_expr(key, &alias) {
                Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "id"),
                other => panic!("expected id FunctionCall for {}, got {:?}", key, other),
            }
        }
    }

    #[test]
    fn make_by_label() {
        let alias = Identifier::new("v");
        for key in &["~label", "T.label", "t[label]", "__.label()", "label()"] {
            match make_by_expr(key, &alias) {
                Expr::FunctionCall { name, .. } => {
                    assert_eq!(name.as_str(), "gremlin_single_label")
                }
                other => panic!("expected label FunctionCall for {}, got {:?}", key, other),
            }
        }
    }

    #[test]
    fn make_by_values_traversal() {
        let alias = Identifier::new("v");
        let expr = make_by_expr("__.values(\"name\")", &alias);
        match expr {
            Expr::Property(_, key) => assert_eq!(key.as_str(), "name"),
            other => panic!("expected Property, got {:?}", other),
        }
    }

    #[test]
    fn make_by_values_without_prefix() {
        let alias = Identifier::new("v");
        let expr = make_by_expr("values('age')", &alias);
        match expr {
            Expr::Property(_, key) => assert_eq!(key.as_str(), "age"),
            other => panic!("expected Property, got {:?}", other),
        }
    }

    #[test]
    fn make_by_key() {
        let alias = Identifier::new("v");
        for key in &["~key", "T.key", "t[key]"] {
            match make_by_expr(key, &alias) {
                Expr::FunctionCall { name, .. } => {
                    assert_eq!(name.as_str(), "gremlin_property_key")
                }
                other => panic!("expected property_key for {}, got {:?}", key, other),
            }
        }
    }

    #[test]
    fn make_by_value() {
        let alias = Identifier::new("v");
        for key in &["~value", "T.value", "t[value]"] {
            match make_by_expr(key, &alias) {
                Expr::FunctionCall { name, .. } => {
                    assert_eq!(name.as_str(), "gremlin_property_value")
                }
                other => panic!("expected property_value for {}, got {:?}", key, other),
            }
        }
    }

    #[test]
    fn make_by_column_keys() {
        let alias = Identifier::new("v");
        for key in &["Column.keys", "column.keys"] {
            match make_by_expr(key, &alias) {
                Expr::FunctionCall { name, .. } => {
                    assert_eq!(name.as_str(), "gremlin_unfold_map_entry_key")
                }
                other => panic!("expected entry_key for {}, got {:?}", key, other),
            }
        }
    }

    #[test]
    fn make_by_column_values() {
        let alias = Identifier::new("v");
        for key in &["Column.values", "column.values"] {
            match make_by_expr(key, &alias) {
                Expr::FunctionCall { name, .. } => {
                    assert_eq!(name.as_str(), "gremlin_unfold_map_entry_value")
                }
                other => panic!("expected entry_value for {}, got {:?}", key, other),
            }
        }
    }

    #[test]
    fn make_by_plain_property() {
        let alias = Identifier::new("v");
        let expr = make_by_expr("age", &alias);
        match expr {
            Expr::Property(_, key) => assert_eq!(key.as_str(), "age"),
            other => panic!("expected Property, got {:?}", other),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// extract.rs tests
// ═══════════════════════════════════════════════════════════════════════════════

mod extract_tests {
    use super::*;

    // ── extract_by_traversal_key ─────────────────────────────────────────

    #[test]
    fn values_single_key() {
        let steps = vec![GremlinStep::Values(vec!["name".into()])];
        assert_eq!(extract_by_traversal_key(&steps), "name");
    }

    #[test]
    fn label_step() {
        let steps = vec![GremlinStep::Label];
        assert_eq!(extract_by_traversal_key(&steps), "~label");
    }

    #[test]
    fn id_step() {
        let steps = vec![GremlinStep::Id];
        assert_eq!(extract_by_traversal_key(&steps), "~id");
    }

    #[test]
    fn select_multi_key() {
        let steps = vec![GremlinStep::Select(None, vec!["a".into(), "b".into()])];
        assert_eq!(extract_by_traversal_key(&steps), "~select_map:a,b");
    }

    #[test]
    fn limit_local() {
        let steps = vec![GremlinStep::LimitLocal(3)];
        assert_eq!(extract_by_traversal_key(&steps), "~limit_local:3");
    }

    #[test]
    fn tail_local() {
        let steps = vec![GremlinStep::TailLocal(2)];
        assert_eq!(extract_by_traversal_key(&steps), "~tail_local:2");
    }

    #[test]
    fn math_expr() {
        let steps = vec![GremlinStep::Math("a + b".into())];
        assert_eq!(extract_by_traversal_key(&steps), "~math:a + b");
    }

    #[test]
    fn values_to_upper() {
        let steps = vec![
            GremlinStep::Values(vec!["name".into()]),
            GremlinStep::ToUpper,
        ];
        assert_eq!(extract_by_traversal_key(&steps), "~toupper:name");
    }

    #[test]
    fn values_to_lower() {
        let steps = vec![
            GremlinStep::Values(vec!["name".into()]),
            GremlinStep::ToLower,
        ];
        assert_eq!(extract_by_traversal_key(&steps), "~tolower:name");
    }

    #[test]
    fn unfold_values_fold() {
        let steps = vec![
            GremlinStep::Unfold,
            GremlinStep::Values(vec!["name".into()]),
            GremlinStep::Fold,
        ];
        assert_eq!(extract_by_traversal_key(&steps), "~unfold_values_fold:name");
    }

    #[test]
    fn in_values_min() {
        let steps = vec![
            GremlinStep::In(vec!["created".into()]),
            GremlinStep::Values(vec!["age".into()]),
            GremlinStep::Min,
        ];
        assert_eq!(
            extract_by_traversal_key(&steps),
            "~in_values_min:created:age"
        );
    }

    #[test]
    fn tail_local_values() {
        let steps = vec![
            GremlinStep::TailLocal(1),
            GremlinStep::Values(vec!["name".into()]),
        ];
        assert_eq!(extract_by_traversal_key(&steps), "name");
    }

    #[test]
    fn unrecognized_returns_empty() {
        let steps = vec![GremlinStep::Out(vec![])];
        assert_eq!(extract_by_traversal_key(&steps), "");
    }

    #[test]
    fn empty_steps_returns_empty() {
        assert_eq!(extract_by_traversal_key(&[]), "");
    }

    // ── extract_select_traversal_label ───────────────────────────────────

    #[test]
    fn select_traversal_label_double_underscore() {
        assert_eq!(
            extract_select_traversal_label("__.select(\"a\")"),
            Some("a".into())
        );
    }

    #[test]
    fn select_traversal_label_no_prefix() {
        assert_eq!(
            extract_select_traversal_label("select('b')"),
            Some("b".into())
        );
    }

    #[test]
    fn select_traversal_label_multi_key_returns_none() {
        assert_eq!(extract_select_traversal_label("select(\"a\", \"b\")"), None);
    }

    #[test]
    fn select_traversal_label_empty_returns_none() {
        assert_eq!(extract_select_traversal_label("select()"), None);
    }

    #[test]
    fn select_traversal_label_non_select() {
        assert_eq!(extract_select_traversal_label("values(\"name\")"), None);
    }

    // ── extract_math_variables ───────────────────────────────────────────

    #[test]
    fn math_vars_simple() {
        let vars = extract_math_variables("a + b");
        assert_eq!(vars, vec!["a", "b"]);
    }

    #[test]
    fn math_vars_underscore_excluded() {
        let vars = extract_math_variables("_ + 1");
        assert!(vars.is_empty());
    }

    #[test]
    fn math_vars_numeric_excluded() {
        let vars = extract_math_variables("2 * 3");
        assert!(vars.is_empty());
    }

    #[test]
    fn math_vars_dedup() {
        let vars = extract_math_variables("x + x");
        assert_eq!(vars, vec!["x"]);
    }

    #[test]
    fn math_vars_multiple_operators() {
        let vars = extract_math_variables("a - b");
        assert!(vars.contains(&"a".to_string()));
        assert!(vars.contains(&"b".to_string()));
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// predicate_expr.rs tests
// ═══════════════════════════════════════════════════════════════════════════════

mod predicate_expr_tests {
    use super::*;

    fn alias() -> Identifier {
        Identifier::new("x")
    }

    // ── predicate_to_expr: null branches ─────────────────────────────────

    #[test]
    fn eq_null_is_null() {
        let expr = predicate_to_expr(&Predicate::Eq(GremlinValue::Null), &alias());
        assert!(matches!(expr, Expr::IsNull(_)));
    }

    #[test]
    fn neq_null_is_not_null() {
        let expr = predicate_to_expr(&Predicate::Neq(GremlinValue::Null), &alias());
        assert!(matches!(expr, Expr::IsNotNull(_)));
    }

    #[test]
    fn gt_null_is_false() {
        let expr = predicate_to_expr(&Predicate::Gt(GremlinValue::Null), &alias());
        assert_eq!(expr, Expr::Bool(false));
    }

    #[test]
    fn gte_null_is_null_check() {
        let expr = predicate_to_expr(&Predicate::Gte(GremlinValue::Null), &alias());
        assert!(matches!(expr, Expr::IsNull(_)));
    }

    #[test]
    fn lt_null_is_false() {
        let expr = predicate_to_expr(&Predicate::Lt(GremlinValue::Null), &alias());
        assert_eq!(expr, Expr::Bool(false));
    }

    #[test]
    fn lte_null_is_null_check() {
        let expr = predicate_to_expr(&Predicate::Lte(GremlinValue::Null), &alias());
        assert!(matches!(expr, Expr::IsNull(_)));
    }

    // ── predicate_to_expr: non-null branches ─────────────────────────────

    #[test]
    fn eq_non_null() {
        let expr = predicate_to_expr(&Predicate::Eq(GremlinValue::Integer(5)), &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And wrapping IsNotNull + Eq, got {:?}", other),
        }
    }

    #[test]
    fn neq_non_null() {
        let expr = predicate_to_expr(&Predicate::Neq(GremlinValue::Integer(5)), &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Or, ..
            } => {}
            other => panic!("expected Or wrapping IsNull + Neq, got {:?}", other),
        }
    }

    #[test]
    fn gt_non_null() {
        let expr = predicate_to_expr(&Predicate::Gt(GremlinValue::Integer(5)), &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Gt, ..
            } => {}
            other => panic!("expected Gt, got {:?}", other),
        }
    }

    #[test]
    fn gte_non_null() {
        let expr = predicate_to_expr(&Predicate::Gte(GremlinValue::Integer(5)), &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Gte, ..
            } => {}
            other => panic!("expected Gte, got {:?}", other),
        }
    }

    #[test]
    fn lt_non_null() {
        let expr = predicate_to_expr(&Predicate::Lt(GremlinValue::Integer(5)), &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Lt, ..
            } => {}
            other => panic!("expected Lt, got {:?}", other),
        }
    }

    #[test]
    fn lte_non_null() {
        let expr = predicate_to_expr(&Predicate::Lte(GremlinValue::Integer(5)), &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Lte, ..
            } => {}
            other => panic!("expected Lte, got {:?}", other),
        }
    }

    #[test]
    fn within() {
        let expr = predicate_to_expr(
            &Predicate::Within(vec![GremlinValue::Integer(1), GremlinValue::Integer(2)]),
            &alias(),
        );
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::In, ..
            } => {}
            other => panic!("expected In, got {:?}", other),
        }
    }

    #[test]
    fn without() {
        let expr = predicate_to_expr(
            &Predicate::Without(vec![GremlinValue::Integer(1)]),
            &alias(),
        );
        match &expr {
            Expr::UnaryOp {
                op: UnaryOp::Not, ..
            } => {}
            other => panic!("expected Not(In(...)), got {:?}", other),
        }
    }

    #[test]
    fn between() {
        let expr = predicate_to_expr(
            &Predicate::Between(GremlinValue::Integer(1), GremlinValue::Integer(10)),
            &alias(),
        );
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And(Gte, Lt), got {:?}", other),
        }
    }

    #[test]
    fn inside() {
        let expr = predicate_to_expr(
            &Predicate::Inside(GremlinValue::Integer(1), GremlinValue::Integer(10)),
            &alias(),
        );
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And(Gt, Lt), got {:?}", other),
        }
    }

    #[test]
    fn text_containing_delegates() {
        let expr = predicate_to_expr(
            &Predicate::Containing(GremlinValue::String("x".into())),
            &alias(),
        );
        match &expr {
            Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "gremlin_containing"),
            other => panic!("expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn and_compound_delegates() {
        let expr = predicate_to_expr(
            &Predicate::And(vec![
                Predicate::Gt(GremlinValue::Integer(1)),
                Predicate::Lt(GremlinValue::Integer(10)),
            ]),
            &alias(),
        );
        // The And compound delegates to predicate_to_prop_expr which builds And chain
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And, got {:?}", other),
        }
    }

    #[test]
    fn or_compound_delegates() {
        let expr = predicate_to_expr(
            &Predicate::Or(vec![
                Predicate::Eq(GremlinValue::Integer(1)),
                Predicate::Eq(GremlinValue::Integer(2)),
            ]),
            &alias(),
        );
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Or, ..
            } => {}
            other => panic!("expected Or, got {:?}", other),
        }
    }

    #[test]
    fn not_compound_delegates() {
        let expr = predicate_to_expr(
            &Predicate::Not(Box::new(Predicate::Eq(GremlinValue::Integer(1)))),
            &alias(),
        );
        match &expr {
            Expr::UnaryOp {
                op: UnaryOp::Not, ..
            } => {}
            other => panic!("expected Not, got {:?}", other),
        }
    }

    // ── predicate_to_prop_expr ───────────────────────────────────────────

    #[test]
    fn prop_expr_eq() {
        let prop = Expr::Property(Box::new(Expr::Variable(alias())), SmolStr::new("age"));
        let result = predicate_to_prop_expr(&Predicate::Eq(GremlinValue::Integer(5)), prop);
        match &result {
            Expr::BinaryOp {
                op: BinaryOp::Eq, ..
            } => {}
            other => panic!("expected Eq, got {:?}", other),
        }
    }

    #[test]
    fn prop_expr_neq() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(&Predicate::Neq(GremlinValue::Integer(5)), prop);
        match &result {
            Expr::BinaryOp {
                op: BinaryOp::Neq, ..
            } => {}
            other => panic!("expected Neq, got {:?}", other),
        }
    }

    #[test]
    fn prop_expr_containing() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(
            &Predicate::Containing(GremlinValue::String("x".into())),
            prop,
        );
        match &result {
            Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "gremlin_containing"),
            other => panic!("expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn prop_expr_not_containing() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(
            &Predicate::NotContaining(GremlinValue::String("x".into())),
            prop,
        );
        assert!(matches!(
            result,
            Expr::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn prop_expr_starting_with() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(
            &Predicate::StartingWith(GremlinValue::String("a".into())),
            prop,
        );
        match &result {
            Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "gremlin_startingWith"),
            other => panic!("expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn prop_expr_not_starting_with() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(
            &Predicate::NotStartingWith(GremlinValue::String("a".into())),
            prop,
        );
        assert!(matches!(
            result,
            Expr::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn prop_expr_ending_with() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(
            &Predicate::EndingWith(GremlinValue::String("z".into())),
            prop,
        );
        match &result {
            Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "gremlin_endingWith"),
            other => panic!("expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn prop_expr_not_ending_with() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(
            &Predicate::NotEndingWith(GremlinValue::String("z".into())),
            prop,
        );
        assert!(matches!(
            result,
            Expr::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn prop_expr_regex() {
        let prop = Expr::Variable(alias());
        let result =
            predicate_to_prop_expr(&Predicate::Regex(GremlinValue::String("^a.*".into())), prop);
        match &result {
            Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "gremlin_regex"),
            other => panic!("expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn prop_expr_not_regex() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(
            &Predicate::NotRegex(GremlinValue::String("^a.*".into())),
            prop,
        );
        assert!(matches!(
            result,
            Expr::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn prop_expr_is_type() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(
            &Predicate::IsType(GremlinValue::String("string".into())),
            prop,
        );
        match &result {
            Expr::FunctionCall { name, .. } => assert_eq!(name.as_str(), "gremlin_istype"),
            other => panic!("expected FunctionCall, got {:?}", other),
        }
    }

    #[test]
    fn prop_expr_between() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(
            &Predicate::Between(GremlinValue::Integer(1), GremlinValue::Integer(10)),
            prop,
        );
        match &result {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And, got {:?}", other),
        }
    }

    #[test]
    fn prop_expr_inside() {
        let prop = Expr::Variable(alias());
        let result = predicate_to_prop_expr(
            &Predicate::Inside(GremlinValue::Integer(1), GremlinValue::Integer(10)),
            prop,
        );
        match &result {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And, got {:?}", other),
        }
    }

    // ── predicate_to_args ────────────────────────────────────────────────

    #[test]
    fn args_eq() {
        let (name, _) = predicate_to_args(&Predicate::Eq(GremlinValue::Integer(1)));
        assert_eq!(&*name, "eq");
    }

    #[test]
    fn args_neq() {
        let (name, _) = predicate_to_args(&Predicate::Neq(GremlinValue::Integer(1)));
        assert_eq!(&*name, "neq");
    }

    #[test]
    fn args_gt() {
        let (name, _) = predicate_to_args(&Predicate::Gt(GremlinValue::Integer(1)));
        assert_eq!(&*name, "gt");
    }

    #[test]
    fn args_gte() {
        let (name, _) = predicate_to_args(&Predicate::Gte(GremlinValue::Integer(1)));
        assert_eq!(&*name, "gte");
    }

    #[test]
    fn args_lt() {
        let (name, _) = predicate_to_args(&Predicate::Lt(GremlinValue::Integer(1)));
        assert_eq!(&*name, "lt");
    }

    #[test]
    fn args_lte() {
        let (name, _) = predicate_to_args(&Predicate::Lte(GremlinValue::Integer(1)));
        assert_eq!(&*name, "lte");
    }

    #[test]
    fn args_within() {
        let (name, expr) = predicate_to_args(&Predicate::Within(vec![
            GremlinValue::Integer(1),
            GremlinValue::Integer(2),
        ]));
        assert_eq!(&*name, "within");
        assert!(matches!(expr, Expr::List(_)));
    }

    #[test]
    fn args_without() {
        let (name, _) = predicate_to_args(&Predicate::Without(vec![GremlinValue::Integer(1)]));
        assert_eq!(&*name, "without");
    }

    #[test]
    fn args_between() {
        let (name, _) = predicate_to_args(&Predicate::Between(
            GremlinValue::Integer(1),
            GremlinValue::Integer(10),
        ));
        assert_eq!(&*name, "within");
    }

    #[test]
    fn args_inside() {
        let (name, _) = predicate_to_args(&Predicate::Inside(
            GremlinValue::Integer(1),
            GremlinValue::Integer(10),
        ));
        assert_eq!(&*name, "within");
    }

    #[test]
    fn args_fallback() {
        let (name, expr) =
            predicate_to_args(&Predicate::Containing(GremlinValue::String("x".into())));
        assert_eq!(&*name, "neq");
        assert_eq!(expr, Expr::Null);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// predicate_label.rs tests
// ═══════════════════════════════════════════════════════════════════════════════

mod predicate_label_tests {
    use super::*;

    fn make_alias_map() -> HashMap<String, Identifier> {
        let mut m = HashMap::new();
        m.insert("a".into(), Identifier::new("__a"));
        m.insert("b".into(), Identifier::new("__b"));
        m
    }

    fn make_agg_keys() -> HashSet<String> {
        let mut s = HashSet::new();
        s.insert("x".into());
        s
    }

    // ── predicate_string_ref ─────────────────────────────────────────────

    #[test]
    fn string_ref_eq() {
        let pred = Predicate::Eq(GremlinValue::String("hello".into()));
        assert_eq!(predicate_string_ref(&pred), Some("hello"));
    }

    #[test]
    fn string_ref_neq() {
        let pred = Predicate::Neq(GremlinValue::String("hello".into()));
        assert_eq!(predicate_string_ref(&pred), Some("hello"));
    }

    #[test]
    fn string_ref_gt() {
        let pred = Predicate::Gt(GremlinValue::String("hi".into()));
        assert_eq!(predicate_string_ref(&pred), Some("hi"));
    }

    #[test]
    fn string_ref_gte() {
        let pred = Predicate::Gte(GremlinValue::String("hi".into()));
        assert_eq!(predicate_string_ref(&pred), Some("hi"));
    }

    #[test]
    fn string_ref_lt() {
        let pred = Predicate::Lt(GremlinValue::String("hi".into()));
        assert_eq!(predicate_string_ref(&pred), Some("hi"));
    }

    #[test]
    fn string_ref_lte() {
        let pred = Predicate::Lte(GremlinValue::String("hi".into()));
        assert_eq!(predicate_string_ref(&pred), Some("hi"));
    }

    #[test]
    fn string_ref_non_string() {
        let pred = Predicate::Eq(GremlinValue::Integer(1));
        assert_eq!(predicate_string_ref(&pred), None);
    }

    #[test]
    fn string_ref_within_returns_none() {
        let pred = Predicate::Within(vec![GremlinValue::String("a".into())]);
        assert_eq!(predicate_string_ref(&pred), None);
    }

    // ── predicate_to_binary_op ───────────────────────────────────────────

    #[test]
    fn binary_op_eq() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Eq(GremlinValue::Integer(1))),
            BinaryOp::Eq
        );
    }

    #[test]
    fn binary_op_neq() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Neq(GremlinValue::Integer(1))),
            BinaryOp::Neq
        );
    }

    #[test]
    fn binary_op_gt() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Gt(GremlinValue::Integer(1))),
            BinaryOp::Gt
        );
    }

    #[test]
    fn binary_op_gte() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Gte(GremlinValue::Integer(1))),
            BinaryOp::Gte
        );
    }

    #[test]
    fn binary_op_lt() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Lt(GremlinValue::Integer(1))),
            BinaryOp::Lt
        );
    }

    #[test]
    fn binary_op_lte() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Lte(GremlinValue::Integer(1))),
            BinaryOp::Lte
        );
    }

    #[test]
    fn binary_op_within() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Within(vec![])),
            BinaryOp::In
        );
    }

    #[test]
    fn binary_op_without() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Without(vec![])),
            BinaryOp::In
        );
    }

    #[test]
    fn binary_op_between() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Between(
                GremlinValue::Integer(1),
                GremlinValue::Integer(2)
            )),
            BinaryOp::And
        );
    }

    #[test]
    fn binary_op_inside() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Inside(
                GremlinValue::Integer(1),
                GremlinValue::Integer(2)
            )),
            BinaryOp::And
        );
    }

    #[test]
    fn binary_op_text_defaults_to_eq() {
        assert_eq!(
            predicate_to_binary_op(&Predicate::Containing(GremlinValue::String("x".into()))),
            BinaryOp::Eq
        );
    }

    // ── predicate_has_label_refs ─────────────────────────────────────────

    #[test]
    fn has_label_refs_eq_with_known_alias() {
        let pred = Predicate::Eq(GremlinValue::String("a".into()));
        assert!(predicate_has_label_refs(
            &pred,
            &make_alias_map(),
            &make_agg_keys()
        ));
    }

    #[test]
    fn has_label_refs_eq_unknown_alias() {
        let pred = Predicate::Eq(GremlinValue::String("unknown".into()));
        assert!(!predicate_has_label_refs(
            &pred,
            &make_alias_map(),
            &make_agg_keys()
        ));
    }

    #[test]
    fn has_label_refs_within_single_agg_key() {
        let pred = Predicate::Within(vec![GremlinValue::String("x".into())]);
        assert!(predicate_has_label_refs(
            &pred,
            &make_alias_map(),
            &make_agg_keys()
        ));
    }

    #[test]
    fn has_label_refs_within_single_alias() {
        let pred = Predicate::Within(vec![GremlinValue::String("a".into())]);
        assert!(predicate_has_label_refs(
            &pred,
            &make_alias_map(),
            &make_agg_keys()
        ));
    }

    #[test]
    fn has_label_refs_within_non_string() {
        let pred = Predicate::Within(vec![GremlinValue::Integer(1)]);
        assert!(!predicate_has_label_refs(
            &pred,
            &make_alias_map(),
            &make_agg_keys()
        ));
    }

    #[test]
    fn has_label_refs_or_compound() {
        let pred = Predicate::Or(vec![
            Predicate::Eq(GremlinValue::String("a".into())),
            Predicate::Eq(GremlinValue::String("unknown".into())),
        ]);
        assert!(predicate_has_label_refs(
            &pred,
            &make_alias_map(),
            &make_agg_keys()
        ));
    }

    #[test]
    fn has_label_refs_not_compound() {
        let pred = Predicate::Not(Box::new(Predicate::Eq(GremlinValue::String("a".into()))));
        assert!(predicate_has_label_refs(
            &pred,
            &make_alias_map(),
            &make_agg_keys()
        ));
    }

    #[test]
    fn has_label_refs_non_matching() {
        let pred = Predicate::Gt(GremlinValue::Integer(1));
        assert!(!predicate_has_label_refs(
            &pred,
            &make_alias_map(),
            &make_agg_keys()
        ));
    }

    // ── predicate_label_ref ──────────────────────────────────────────────

    #[test]
    fn label_ref_eq_known_alias() {
        let pred = Predicate::Eq(GremlinValue::String("a".into()));
        let result = predicate_label_ref(&pred, &make_alias_map(), &make_agg_keys());
        assert_eq!(result, Some(Expr::Variable(Identifier::new("__a"))));
    }

    #[test]
    fn label_ref_eq_agg_key() {
        let pred = Predicate::Eq(GremlinValue::String("x".into()));
        let result = predicate_label_ref(&pred, &make_alias_map(), &make_agg_keys());
        assert_eq!(
            result,
            Some(Expr::Variable(Identifier::new("__gremlin_agg_x")))
        );
    }

    #[test]
    fn label_ref_within_known_alias() {
        let pred = Predicate::Within(vec![GremlinValue::String("a".into())]);
        let result = predicate_label_ref(&pred, &make_alias_map(), &make_agg_keys());
        assert_eq!(result, Some(Expr::Variable(Identifier::new("__a"))));
    }

    #[test]
    fn label_ref_without_agg_key() {
        let pred = Predicate::Without(vec![GremlinValue::String("x".into())]);
        let result = predicate_label_ref(&pred, &make_alias_map(), &make_agg_keys());
        assert_eq!(
            result,
            Some(Expr::Variable(Identifier::new("__gremlin_agg_x")))
        );
    }

    #[test]
    fn label_ref_unknown_returns_none() {
        let pred = Predicate::Eq(GremlinValue::String("unknown".into()));
        let result = predicate_label_ref(&pred, &make_alias_map(), &make_agg_keys());
        assert_eq!(result, None);
    }

    #[test]
    fn label_ref_within_non_string_returns_none() {
        let pred = Predicate::Within(vec![GremlinValue::Integer(1)]);
        let result = predicate_label_ref(&pred, &make_alias_map(), &make_agg_keys());
        assert_eq!(result, None);
    }

    #[test]
    fn label_ref_without_non_string_returns_none() {
        let pred = Predicate::Without(vec![GremlinValue::Integer(1)]);
        let result = predicate_label_ref(&pred, &make_alias_map(), &make_agg_keys());
        assert_eq!(result, None);
    }

    #[test]
    fn label_ref_non_matching_returns_none() {
        let pred = Predicate::Gt(GremlinValue::Integer(1));
        let result = predicate_label_ref(&pred, &make_alias_map(), &make_agg_keys());
        assert_eq!(result, None);
    }

    // ── predicate_to_label_aware_expr ────────────────────────────────────

    #[test]
    fn label_aware_eq_known_alias() {
        let left = Expr::Variable(Identifier::new("v"));
        let pred = Predicate::Eq(GremlinValue::String("a".into()));
        let result =
            predicate_to_label_aware_expr(&pred, &left, &make_alias_map(), &make_agg_keys());
        match &result {
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                right,
                ..
            } => {
                assert_eq!(**right, Expr::Variable(Identifier::new("__a")));
            }
            other => panic!("expected Eq with alias variable, got {:?}", other),
        }
    }

    #[test]
    fn label_aware_within_single_alias() {
        let left = Expr::Variable(Identifier::new("v"));
        let pred = Predicate::Within(vec![GremlinValue::String("a".into())]);
        let result =
            predicate_to_label_aware_expr(&pred, &left, &make_alias_map(), &make_agg_keys());
        match &result {
            Expr::BinaryOp {
                op: BinaryOp::In, ..
            } => {}
            other => panic!("expected In, got {:?}", other),
        }
    }

    #[test]
    fn label_aware_within_single_agg_key() {
        let left = Expr::Variable(Identifier::new("v"));
        let pred = Predicate::Within(vec![GremlinValue::String("x".into())]);
        let result =
            predicate_to_label_aware_expr(&pred, &left, &make_alias_map(), &make_agg_keys());
        match &result {
            Expr::BinaryOp {
                op: BinaryOp::In,
                right,
                ..
            } => {
                assert_eq!(**right, Expr::Variable(Identifier::new("__gremlin_agg_x")));
            }
            other => panic!("expected In with agg var, got {:?}", other),
        }
    }

    #[test]
    fn label_aware_without_single_alias() {
        let left = Expr::Variable(Identifier::new("v"));
        let pred = Predicate::Without(vec![GremlinValue::String("a".into())]);
        let result =
            predicate_to_label_aware_expr(&pred, &left, &make_alias_map(), &make_agg_keys());
        assert!(matches!(
            result,
            Expr::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn label_aware_without_single_agg_key() {
        let left = Expr::Variable(Identifier::new("v"));
        let pred = Predicate::Without(vec![GremlinValue::String("x".into())]);
        let result =
            predicate_to_label_aware_expr(&pred, &left, &make_alias_map(), &make_agg_keys());
        assert!(matches!(
            result,
            Expr::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn label_aware_or_compound() {
        let left = Expr::Variable(Identifier::new("v"));
        let pred = Predicate::Or(vec![
            Predicate::Eq(GremlinValue::String("a".into())),
            Predicate::Eq(GremlinValue::String("b".into())),
        ]);
        let result =
            predicate_to_label_aware_expr(&pred, &left, &make_alias_map(), &make_agg_keys());
        match &result {
            Expr::BinaryOp {
                op: BinaryOp::Or, ..
            } => {}
            other => panic!("expected Or, got {:?}", other),
        }
    }

    #[test]
    fn label_aware_and_compound() {
        let left = Expr::Variable(Identifier::new("v"));
        let pred = Predicate::And(vec![
            Predicate::Eq(GremlinValue::String("a".into())),
            Predicate::Gt(GremlinValue::String("b".into())),
        ]);
        let result =
            predicate_to_label_aware_expr(&pred, &left, &make_alias_map(), &make_agg_keys());
        match &result {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And, got {:?}", other),
        }
    }

    #[test]
    fn label_aware_not_compound() {
        let left = Expr::Variable(Identifier::new("v"));
        let pred = Predicate::Not(Box::new(Predicate::Eq(GremlinValue::String("a".into()))));
        let result =
            predicate_to_label_aware_expr(&pred, &left, &make_alias_map(), &make_agg_keys());
        assert!(matches!(
            result,
            Expr::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn label_aware_fallback() {
        let left = Expr::Variable(Identifier::new("v"));
        let pred = Predicate::Gt(GremlinValue::Integer(10));
        let result =
            predicate_to_label_aware_expr(&pred, &left, &make_alias_map(), &make_agg_keys());
        match &result {
            Expr::BinaryOp {
                op: BinaryOp::Gt, ..
            } => {}
            other => panic!("expected Gt fallback, got {:?}", other),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// predicates.rs tests
// ═══════════════════════════════════════════════════════════════════════════════

mod predicates_tests {
    use super::*;

    fn alias() -> Identifier {
        Identifier::new("v")
    }

    fn empty_alias_map() -> HashMap<String, Identifier> {
        HashMap::new()
    }

    fn empty_agg_keys() -> HashSet<String> {
        HashSet::new()
    }

    // ── step_to_predicate ────────────────────────────────────────────────

    #[test]
    fn has_key_value() {
        let step = GremlinStep::Has("name".into(), Some(GremlinValue::String("alice".into())));
        let expr = step_to_predicate(&step, &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Eq, ..
            } => {}
            other => panic!("expected Eq, got {:?}", other),
        }
    }

    #[test]
    fn has_key_only() {
        let step = GremlinStep::Has("name".into(), None);
        let expr = step_to_predicate(&step, &alias());
        assert!(matches!(expr, Expr::IsNotNull(_)));
    }

    #[test]
    fn has_predicate() {
        let step =
            GremlinStep::HasPredicate("age".into(), Predicate::Gt(GremlinValue::Integer(20)));
        let expr = step_to_predicate(&step, &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And(IsNotNull, pred), got {:?}", other),
        }
    }

    #[test]
    fn has_label_single() {
        let step = GremlinStep::HasLabel(vec!["person".into()]);
        let expr = step_to_predicate(&step, &alias());
        match &expr {
            Expr::HasLabel(_, label) => assert_eq!(label.0.as_str(), "person"),
            other => panic!("expected HasLabel, got {:?}", other),
        }
    }

    #[test]
    fn has_label_multi() {
        let step = GremlinStep::HasLabel(vec!["person".into(), "software".into()]);
        let expr = step_to_predicate(&step, &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Or, ..
            } => {}
            other => panic!("expected Or of HasLabels, got {:?}", other),
        }
    }

    #[test]
    fn is_step() {
        let step = GremlinStep::Is(Predicate::Eq(GremlinValue::Integer(5)));
        let expr = step_to_predicate(&step, &alias());
        // Eq with non-null should produce And(IsNotNull, Eq)
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And, got {:?}", other),
        }
    }

    #[test]
    fn has_id_single() {
        let step = GremlinStep::HasId(vec![GremlinValue::Integer(1)]);
        let expr = step_to_predicate(&step, &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Eq, ..
            } => {}
            other => panic!("expected Eq, got {:?}", other),
        }
    }

    #[test]
    fn has_id_multi() {
        let step = GremlinStep::HasId(vec![GremlinValue::Integer(1), GremlinValue::Integer(2)]);
        let expr = step_to_predicate(&step, &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::In, ..
            } => {}
            other => panic!("expected In, got {:?}", other),
        }
    }

    #[test]
    fn has_id_with_nested_list() {
        let step = GremlinStep::HasId(vec![
            GremlinValue::Integer(1),
            GremlinValue::List(vec![GremlinValue::Integer(2), GremlinValue::Integer(3)]),
        ]);
        let expr = step_to_predicate(&step, &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::In,
                right,
                ..
            } => {
                if let Expr::List(items) = right.as_ref() {
                    assert_eq!(items.len(), 3);
                }
            }
            other => panic!("expected In with flattened list, got {:?}", other),
        }
    }

    #[test]
    fn has_id_predicate() {
        let step = GremlinStep::HasIdPredicate(Predicate::Neq(GremlinValue::Integer(1)));
        let expr = step_to_predicate(&step, &alias());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Neq, ..
            } => {}
            other => panic!("expected Neq, got {:?}", other),
        }
    }

    #[test]
    fn unknown_step_returns_true() {
        let step = GremlinStep::Out(vec![]);
        let expr = step_to_predicate(&step, &alias());
        assert_eq!(expr, Expr::Bool(true));
    }

    // ── steps_to_predicate ───────────────────────────────────────────────

    #[test]
    fn empty_steps_is_true() {
        let expr = steps_to_predicate(&[], &alias(), &empty_alias_map(), &empty_agg_keys());
        assert_eq!(expr, Expr::Bool(true));
    }

    #[test]
    fn single_or_step() {
        let steps = vec![GremlinStep::Or(vec![
            vec![GremlinStep::Has(
                "name".into(),
                Some(GremlinValue::String("a".into())),
            )],
            vec![GremlinStep::Has(
                "name".into(),
                Some(GremlinValue::String("b".into())),
            )],
        ])];
        let expr = steps_to_predicate(&steps, &alias(), &empty_alias_map(), &empty_agg_keys());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Or, ..
            } => {}
            other => panic!("expected Or, got {:?}", other),
        }
    }

    #[test]
    fn single_and_step() {
        let steps = vec![GremlinStep::And(vec![
            vec![GremlinStep::Has(
                "age".into(),
                Some(GremlinValue::Integer(20)),
            )],
            vec![GremlinStep::Has(
                "name".into(),
                Some(GremlinValue::String("a".into())),
            )],
        ])];
        let expr = steps_to_predicate(&steps, &alias(), &empty_alias_map(), &empty_agg_keys());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And, got {:?}", other),
        }
    }

    #[test]
    fn single_not_step() {
        let steps = vec![GremlinStep::Not(vec![GremlinStep::Has(
            "name".into(),
            Some(GremlinValue::String("x".into())),
        )])];
        let expr = steps_to_predicate(&steps, &alias(), &empty_alias_map(), &empty_agg_keys());
        assert!(matches!(
            expr,
            Expr::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn two_step_values_is() {
        let steps = vec![
            GremlinStep::Values(vec!["age".into()]),
            GremlinStep::Is(Predicate::Gt(GremlinValue::Integer(30))),
        ];
        let expr = steps_to_predicate(&steps, &alias(), &empty_alias_map(), &empty_agg_keys());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Gt, ..
            } => {}
            other => panic!("expected Gt, got {:?}", other),
        }
    }

    #[test]
    fn two_step_loops_is() {
        let steps = vec![
            GremlinStep::Loops(None),
            GremlinStep::Is(Predicate::Lt(GremlinValue::Integer(3))),
        ];
        let expr = steps_to_predicate(&steps, &alias(), &empty_alias_map(), &empty_agg_keys());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Lt, ..
            } => {}
            other => panic!("expected Lt on loop var, got {:?}", other),
        }
    }

    #[test]
    fn two_step_loops_labeled() {
        let steps = vec![
            GremlinStep::Loops(Some("myloop".into())),
            GremlinStep::Is(Predicate::Lt(GremlinValue::Integer(3))),
        ];
        let expr = steps_to_predicate(&steps, &alias(), &empty_alias_map(), &empty_agg_keys());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Lt, ..
            } => {}
            other => panic!("expected Lt on labeled loop var, got {:?}", other),
        }
    }

    #[test]
    fn two_step_label_is_eq() {
        let steps = vec![
            GremlinStep::Label,
            GremlinStep::Is(Predicate::Eq(GremlinValue::String("person".into()))),
        ];
        let expr = steps_to_predicate(&steps, &alias(), &empty_alias_map(), &empty_agg_keys());
        match &expr {
            Expr::HasLabel(_, label) => assert_eq!(label.0.as_str(), "person"),
            other => panic!("expected HasLabel, got {:?}", other),
        }
    }

    #[test]
    fn two_step_label_is_other_pred() {
        let steps = vec![
            GremlinStep::Label,
            GremlinStep::Is(Predicate::Neq(GremlinValue::String("person".into()))),
        ];
        let expr = steps_to_predicate(&steps, &alias(), &empty_alias_map(), &empty_agg_keys());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Neq, ..
            } => {}
            other => panic!("expected Neq, got {:?}", other),
        }
    }

    #[test]
    fn two_step_id_is() {
        let steps = vec![
            GremlinStep::Id,
            GremlinStep::Is(Predicate::Eq(GremlinValue::Integer(42))),
        ];
        let expr = steps_to_predicate(&steps, &alias(), &empty_alias_map(), &empty_agg_keys());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Eq, ..
            } => {}
            other => panic!("expected Eq on id, got {:?}", other),
        }
    }

    #[test]
    fn multi_step_has_chain() {
        let steps = vec![
            GremlinStep::Has("age".into(), Some(GremlinValue::Integer(30))),
            GremlinStep::Has("name".into(), Some(GremlinValue::String("alice".into()))),
        ];
        let expr = steps_to_predicate(&steps, &alias(), &empty_alias_map(), &empty_agg_keys());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::And, ..
            } => {}
            other => panic!("expected And chain, got {:?}", other),
        }
    }

    #[test]
    fn is_label_ref_single_step() {
        let mut alias_map = HashMap::new();
        alias_map.insert("b".into(), Identifier::new("__b"));
        let steps = vec![GremlinStep::Is(Predicate::Eq(GremlinValue::String(
            "b".into(),
        )))];
        let expr = steps_to_predicate(&steps, &alias(), &alias_map, &empty_agg_keys());
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                right,
                ..
            } => {
                assert_eq!(**right, Expr::Variable(Identifier::new("__b")));
            }
            other => panic!("expected Eq with label var, got {:?}", other),
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// steps.rs tests
// ═══════════════════════════════════════════════════════════════════════════════

mod steps_tests {
    use super::*;

    // ── is_simple_predicate_steps ────────────────────────────────────────

    #[test]
    fn simple_predicate_empty() {
        assert!(!is_simple_predicate_steps(&[]));
    }

    #[test]
    fn simple_predicate_has() {
        assert!(is_simple_predicate_steps(&[GremlinStep::Has(
            "name".into(),
            None,
        )]));
    }

    #[test]
    fn simple_predicate_has_label() {
        assert!(is_simple_predicate_steps(&[GremlinStep::HasLabel(vec![
            "person".into(),
        ])]));
    }

    #[test]
    fn simple_predicate_has_id() {
        assert!(is_simple_predicate_steps(&[GremlinStep::HasId(vec![
            GremlinValue::Integer(1),
        ])]));
    }

    #[test]
    fn simple_predicate_values() {
        assert!(is_simple_predicate_steps(&[GremlinStep::Values(vec![
            "name".into(),
        ])]));
    }

    #[test]
    fn simple_predicate_label() {
        assert!(is_simple_predicate_steps(&[GremlinStep::Label]));
    }

    #[test]
    fn simple_predicate_id() {
        assert!(is_simple_predicate_steps(&[GremlinStep::Id]));
    }

    #[test]
    fn simple_predicate_mixed() {
        assert!(is_simple_predicate_steps(&[
            GremlinStep::Has("name".into(), None),
            GremlinStep::Is(Predicate::Gt(GremlinValue::Integer(1))),
        ]));
    }

    #[test]
    fn non_predicate_step_fails() {
        assert!(!is_simple_predicate_steps(&[GremlinStep::Out(vec![])]));
    }

    // ── steps_need_path_tracking ─────────────────────────────────────────

    #[test]
    fn path_needs_tracking() {
        assert!(steps_need_path_tracking(&[GremlinStep::Path]));
    }

    #[test]
    fn simple_path_needs_tracking() {
        assert!(steps_need_path_tracking(&[GremlinStep::SimplePath]));
    }

    #[test]
    fn cyclic_path_needs_tracking() {
        assert!(steps_need_path_tracking(&[GremlinStep::CyclicPath]));
    }

    #[test]
    fn tree_needs_tracking() {
        assert!(steps_need_path_tracking(&[GremlinStep::Tree(None)]));
    }

    #[test]
    fn nested_repeat_path() {
        assert!(steps_need_path_tracking(&[GremlinStep::Repeat(
            None,
            vec![GremlinStep::Path],
        )]));
    }

    #[test]
    fn nested_union_path() {
        assert!(steps_need_path_tracking(&[GremlinStep::Union(vec![vec![
            GremlinStep::Path
        ],])]));
    }

    #[test]
    fn nested_choose_path() {
        assert!(steps_need_path_tracking(&[GremlinStep::Choose(
            vec![GremlinStep::Path],
            vec![],
            None,
        )]));
    }

    #[test]
    fn no_path_tracking() {
        assert!(!steps_need_path_tracking(&[GremlinStep::Out(vec![])]));
    }

    #[test]
    fn empty_no_path_tracking() {
        assert!(!steps_need_path_tracking(&[]));
    }

    // ── steps_need_label_stacks ──────────────────────────────────────────

    #[test]
    fn select_pop_needs_stacks() {
        use crate::bytecode::Pop;
        assert!(steps_need_label_stacks(&[GremlinStep::Select(
            Some(Pop::First),
            vec!["a".into()],
        )]));
    }

    #[test]
    fn select_no_pop_no_stacks() {
        assert!(!steps_need_label_stacks(&[GremlinStep::Select(
            None,
            vec!["a".into()],
        )]));
    }

    #[test]
    fn nested_repeat_label_stacks() {
        use crate::bytecode::Pop;
        assert!(steps_need_label_stacks(&[GremlinStep::Repeat(
            None,
            vec![GremlinStep::Select(Some(Pop::All), vec!["a".into()])],
        )]));
    }

    #[test]
    fn no_label_stacks() {
        assert!(!steps_need_label_stacks(&[GremlinStep::Out(vec![])]));
    }

    // ── collect_aggregate_keys_prescan ────────────────────────────────────

    #[test]
    fn collects_aggregate_keys() {
        let steps = vec![
            GremlinStep::Out(vec![]),
            GremlinStep::Aggregate("collected".into()),
        ];
        let mut keys = HashSet::new();
        collect_aggregate_keys_prescan(&steps, &mut keys);
        assert!(keys.contains("collected"));
    }

    #[test]
    fn collects_nested_aggregate_keys() {
        let steps = vec![GremlinStep::Repeat(
            None,
            vec![GremlinStep::Aggregate("inner".into())],
        )];
        let mut keys = HashSet::new();
        collect_aggregate_keys_prescan(&steps, &mut keys);
        assert!(keys.contains("inner"));
    }

    #[test]
    fn collects_from_union_branches() {
        let steps = vec![GremlinStep::Union(vec![
            vec![GremlinStep::Aggregate("left".into())],
            vec![GremlinStep::Aggregate("right".into())],
        ])];
        let mut keys = HashSet::new();
        collect_aggregate_keys_prescan(&steps, &mut keys);
        assert!(keys.contains("left"));
        assert!(keys.contains("right"));
    }

    #[test]
    fn collects_from_choose_branches() {
        let steps = vec![GremlinStep::Choose(
            vec![GremlinStep::Aggregate("if".into())],
            vec![GremlinStep::Aggregate("else".into())],
            None,
        )];
        let mut keys = HashSet::new();
        collect_aggregate_keys_prescan(&steps, &mut keys);
        assert!(keys.contains("if"));
        assert!(keys.contains("else"));
    }

    // ── split_at_empty_connective ────────────────────────────────────────

    #[test]
    fn split_and_empty() {
        let steps = vec![
            GremlinStep::Has("a".into(), None),
            GremlinStep::And(vec![]),
            GremlinStep::Has("b".into(), None),
        ];
        let segments = split_at_empty_connective(&steps, true);
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn split_or_empty() {
        let steps = vec![
            GremlinStep::Has("a".into(), None),
            GremlinStep::Or(vec![]),
            GremlinStep::Has("b".into(), None),
        ];
        let segments = split_at_empty_connective(&steps, false);
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn split_no_match() {
        let steps = vec![GremlinStep::Has("a".into(), None)];
        let segments = split_at_empty_connective(&steps, true);
        assert_eq!(segments.len(), 1);
    }

    // ── preprocess_infix_connectives ─────────────────────────────────────

    #[test]
    fn no_infix_passthrough() {
        let steps = vec![GremlinStep::Has("a".into(), None)];
        let result = preprocess_infix_connectives(&steps);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn infix_and_creates_and_step() {
        let steps = vec![
            GremlinStep::Has("a".into(), None),
            GremlinStep::And(vec![]),
            GremlinStep::Has("b".into(), None),
        ];
        let result = preprocess_infix_connectives(&steps);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], GremlinStep::And(_)));
    }

    #[test]
    fn infix_or_creates_or_step() {
        let steps = vec![
            GremlinStep::Has("a".into(), None),
            GremlinStep::Or(vec![]),
            GremlinStep::Has("b".into(), None),
        ];
        let result = preprocess_infix_connectives(&steps);
        assert_eq!(result.len(), 1);
        assert!(matches!(result[0], GremlinStep::Or(_)));
    }

    #[test]
    fn infix_preserves_source_steps() {
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Has("a".into(), None),
            GremlinStep::And(vec![]),
            GremlinStep::Has("b".into(), None),
        ];
        let result = preprocess_infix_connectives(&steps);
        assert_eq!(result.len(), 2);
        assert!(matches!(result[0], GremlinStep::V(_)));
        assert!(matches!(result[1], GremlinStep::And(_)));
    }

    // ── split_trailing_agg ───────────────────────────────────────────────

    #[test]
    fn trailing_count() {
        let steps = vec![GremlinStep::Out(vec![]), GremlinStep::Count];
        let (rest, agg) = split_trailing_agg(&steps);
        assert_eq!(rest.len(), 1);
        assert_eq!(agg, Some(AggFunction::Count));
    }

    #[test]
    fn trailing_sum() {
        let steps = vec![GremlinStep::Out(vec![]), GremlinStep::Sum];
        let (rest, agg) = split_trailing_agg(&steps);
        assert_eq!(rest.len(), 1);
        assert_eq!(agg, Some(AggFunction::Sum));
    }

    #[test]
    fn trailing_fold() {
        let steps = vec![GremlinStep::Out(vec![]), GremlinStep::Fold];
        let (rest, agg) = split_trailing_agg(&steps);
        assert_eq!(rest.len(), 1);
        assert_eq!(agg, Some(AggFunction::Collect));
    }

    #[test]
    fn no_trailing_agg() {
        let steps = vec![GremlinStep::Out(vec![])];
        let (rest, agg) = split_trailing_agg(&steps);
        assert_eq!(rest.len(), 1);
        assert_eq!(agg, None);
    }

    #[test]
    fn empty_steps_no_agg() {
        let (rest, agg) = split_trailing_agg(&[]);
        assert!(rest.is_empty());
        assert_eq!(agg, None);
    }

    // ── split_trailing_group_value_agg ───────────────────────────────────

    #[test]
    fn group_value_fold_preserved() {
        let steps = vec![GremlinStep::Out(vec![]), GremlinStep::Fold];
        let (rest, agg) = split_trailing_group_value_agg(&steps);
        assert_eq!(rest.len(), 2);
        assert_eq!(agg, None);
    }

    #[test]
    fn group_value_count_split() {
        let steps = vec![GremlinStep::Out(vec![]), GremlinStep::Count];
        let (rest, agg) = split_trailing_group_value_agg(&steps);
        assert_eq!(rest.len(), 1);
        assert_eq!(agg, Some(AggFunction::Count));
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// operators.rs tests
// ═══════════════════════════════════════════════════════════════════════════════

mod operators_tests {
    use super::*;

    // ── inject_sack_init ─────────────────────────────────────────────────

    #[test]
    fn sack_init_single_row() {
        let mut op = Operator::SingleRow;
        inject_sack_init(&mut op, &Expr::Integer(0));
        match &op {
            Operator::Apply { outer, inner } => {
                assert!(matches!(outer.as_ref(), Operator::SingleRow));
                match inner.as_ref() {
                    Operator::Project { items, .. } => {
                        assert_eq!(items.len(), 1);
                        assert_eq!(items[0].0, Expr::Integer(0));
                    }
                    other => panic!("expected Project inner, got {:?}", other),
                }
            }
            other => panic!("expected Apply wrapping SingleRow, got {:?}", other),
        }
    }

    #[test]
    fn sack_init_recurses_through_filter() {
        let mut op = Operator::Filter {
            predicate: Expr::Bool(true),
            child: Box::new(Operator::SingleRow),
        };
        inject_sack_init(&mut op, &Expr::Integer(0));
        match &op {
            Operator::Filter { child, .. } => {
                assert!(matches!(child.as_ref(), Operator::Apply { .. }));
            }
            other => panic!("expected Filter with Apply child, got {:?}", other),
        }
    }

    #[test]
    fn sack_init_recurses_through_apply() {
        let mut op = Operator::Apply {
            outer: Box::new(Operator::SingleRow),
            inner: Box::new(Operator::SingleRow),
        };
        inject_sack_init(&mut op, &Expr::Integer(0));
        match &op {
            Operator::Apply { outer, .. } => {
                assert!(matches!(outer.as_ref(), Operator::Apply { .. }));
            }
            other => panic!("expected Apply with wrapped outer, got {:?}", other),
        }
    }

    #[test]
    fn sack_init_recurses_through_union() {
        let mut op = Operator::Union {
            left: Box::new(Operator::SingleRow),
            right: Box::new(Operator::SingleRow),
            all: true,
        };
        inject_sack_init(&mut op, &Expr::Integer(0));
        match &op {
            Operator::Union { left, right, .. } => {
                assert!(matches!(left.as_ref(), Operator::Apply { .. }));
                assert!(matches!(right.as_ref(), Operator::Apply { .. }));
            }
            other => panic!("expected Union with Apply children, got {:?}", other),
        }
    }

    // ── inject_scan_into_child ───────────────────────────────────────────

    #[test]
    fn scan_into_create_no_child() {
        let op = Operator::Create {
            patterns: vec![],
            child: None,
        };
        let scan = Operator::SingleRow;
        let result = inject_scan_into_child(op, scan);
        match &result {
            Operator::Create {
                child: Some(ch), ..
            } => {
                assert!(matches!(ch.as_ref(), Operator::SingleRow));
            }
            other => panic!("expected Create with scan child, got {:?}", other),
        }
    }

    #[test]
    fn scan_into_create_with_child() {
        let op = Operator::Create {
            patterns: vec![],
            child: Some(Box::new(Operator::SingleRow)),
        };
        let scan = Operator::SingleRow;
        let result = inject_scan_into_child(op, scan);
        match &result {
            Operator::Create {
                child: Some(ch), ..
            } => {
                assert!(matches!(ch.as_ref(), Operator::Apply { .. }));
            }
            other => panic!("expected Create with Apply child, got {:?}", other),
        }
    }

    #[test]
    fn scan_into_set_recurses() {
        let op = Operator::Set {
            items: vec![],
            child: Box::new(Operator::Create {
                patterns: vec![],
                child: None,
            }),
        };
        let scan = Operator::SingleRow;
        let result = inject_scan_into_child(op, scan);
        match &result {
            Operator::Set { child, .. } => {
                assert!(matches!(child.as_ref(), Operator::Create { .. }));
            }
            other => panic!("expected Set with Create child, got {:?}", other),
        }
    }

    #[test]
    fn scan_into_other_fallback() {
        let op = Operator::SingleRow;
        let scan = Operator::SingleRow;
        let result = inject_scan_into_child(op, scan);
        assert!(matches!(result, Operator::Apply { .. }));
    }

    // ── rewrite_create_alias ─────────────────────────────────────────────

    #[test]
    fn rewrite_alias_in_create_edges() {
        let edge: CreateEdgeSpec = (
            None,
            Identifier::new("__placeholder"),
            Identifier::new("dst"),
            RelType::new("KNOWS"),
            Direction::Outgoing,
            vec![],
        );
        let mut op = Operator::Create {
            patterns: vec![CreatePattern {
                nodes: vec![],
                edges: vec![edge],
            }],
            child: None,
        };
        rewrite_create_alias(&mut op, "__placeholder", &Identifier::new("real_src"));
        match &op {
            Operator::Create { patterns, .. } => {
                assert_eq!(patterns[0].edges[0].1, Identifier::new("real_src"));
            }
            other => panic!("expected Create, got {:?}", other),
        }
    }

    #[test]
    fn rewrite_alias_in_merge_relationship() {
        let mut op = Operator::MergeRelationship {
            src_alias: Identifier::new("__placeholder"),
            dst_alias: Identifier::new("__placeholder"),
            rel_alias: None,
            rel_type: RelType::new("KNOWS"),
            direction: Direction::Outgoing,
            properties: vec![],
            on_create: vec![],
            on_match: vec![],
            child: Box::new(Operator::SingleRow),
        };
        rewrite_create_alias(&mut op, "__placeholder", &Identifier::new("real"));
        match &op {
            Operator::MergeRelationship {
                src_alias,
                dst_alias,
                ..
            } => {
                assert_eq!(src_alias, &Identifier::new("real"));
                assert_eq!(dst_alias, &Identifier::new("real"));
            }
            other => panic!("expected MergeRelationship, got {:?}", other),
        }
    }

    #[test]
    fn rewrite_alias_recurses_apply() {
        let edge: CreateEdgeSpec = (
            None,
            Identifier::new("__placeholder"),
            Identifier::new("dst"),
            RelType::new("KNOWS"),
            Direction::Outgoing,
            vec![],
        );
        let mut op = Operator::Apply {
            outer: Box::new(Operator::Create {
                patterns: vec![CreatePattern {
                    nodes: vec![],
                    edges: vec![edge],
                }],
                child: None,
            }),
            inner: Box::new(Operator::SingleRow),
        };
        rewrite_create_alias(&mut op, "__placeholder", &Identifier::new("real_src"));
        match &op {
            Operator::Apply { outer, .. } => match outer.as_ref() {
                Operator::Create { patterns, .. } => {
                    assert_eq!(patterns[0].edges[0].1, Identifier::new("real_src"));
                }
                other => panic!("expected Create in outer, got {:?}", other),
            },
            other => panic!("expected Apply, got {:?}", other),
        }
    }

    #[test]
    fn rewrite_alias_set_recurses() {
        let mut op = Operator::Set {
            items: vec![],
            child: Box::new(Operator::Create {
                patterns: vec![CreatePattern {
                    nodes: vec![],
                    edges: vec![(
                        None,
                        Identifier::new("__placeholder"),
                        Identifier::new("d"),
                        RelType::new("K"),
                        Direction::Outgoing,
                        vec![],
                    )],
                }],
                child: None,
            }),
        };
        rewrite_create_alias(&mut op, "__placeholder", &Identifier::new("real"));
    }

    // ── rewrite_sack_by ──────────────────────────────────────────────────

    #[test]
    fn sack_by_replaces_variable() {
        let target = Identifier::new("__val");
        let replacement = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("v"))),
            SmolStr::new("age"),
        );
        let mut expr = Expr::Variable(target.clone());
        rewrite_sack_by(&mut expr, &target, &replacement);
        assert_eq!(
            expr,
            Expr::Property(
                Box::new(Expr::Variable(Identifier::new("v"))),
                SmolStr::new("age"),
            )
        );
    }

    #[test]
    fn sack_by_recurses_binary_op() {
        let target = Identifier::new("__val");
        let replacement = Expr::Integer(42);
        let mut expr = Expr::BinaryOp {
            op: BinaryOp::Add,
            left: Box::new(Expr::Variable(target.clone())),
            right: Box::new(Expr::Variable(target.clone())),
        };
        rewrite_sack_by(&mut expr, &target, &replacement);
        match &expr {
            Expr::BinaryOp { left, right, .. } => {
                assert_eq!(**left, Expr::Integer(42));
                assert_eq!(**right, Expr::Integer(42));
            }
            other => panic!("expected BinaryOp, got {:?}", other),
        }
    }

    #[test]
    fn sack_by_no_op_on_different_var() {
        let target = Identifier::new("__val");
        let replacement = Expr::Integer(42);
        let mut expr = Expr::Variable(Identifier::new("__other"));
        rewrite_sack_by(&mut expr, &target, &replacement);
        assert_eq!(expr, Expr::Variable(Identifier::new("__other")));
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// where_by.rs tests
// ═══════════════════════════════════════════════════════════════════════════════

mod where_by_tests {
    use super::*;

    fn make_alias_map() -> HashMap<String, Identifier> {
        let mut m = HashMap::new();
        m.insert("a".into(), Identifier::new("__a"));
        m.insert("b".into(), Identifier::new("__b"));
        m
    }

    // ── extract_where_by_targets ─────────────────────────────────────────

    #[test]
    fn pattern1_select_is_eq() {
        let steps = vec![
            GremlinStep::Select(None, vec!["a".into()]),
            GremlinStep::Is(Predicate::Eq(GremlinValue::String("b".into()))),
        ];
        let result = extract_where_by_targets(&steps, &make_alias_map(), &Identifier::new("v"));
        assert!(result.is_some());
        let targets = result.unwrap();
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn pattern1_select_column_keys() {
        let steps = vec![
            GremlinStep::Select(None, vec!["keys".into()]),
            GremlinStep::Is(Predicate::Eq(GremlinValue::String("b".into()))),
        ];
        let result = extract_where_by_targets(&steps, &make_alias_map(), &Identifier::new("v"));
        assert!(result.is_some());
    }

    #[test]
    fn pattern1_select_column_values() {
        let steps = vec![
            GremlinStep::Select(None, vec!["Column.values".into()]),
            GremlinStep::Is(Predicate::Eq(GremlinValue::String("b".into()))),
        ];
        let result = extract_where_by_targets(&steps, &make_alias_map(), &Identifier::new("v"));
        assert!(result.is_some());
    }

    #[test]
    fn pattern2_is_eq_label() {
        let steps = vec![GremlinStep::Is(Predicate::Eq(GremlinValue::String(
            "a".into(),
        )))];
        let result = extract_where_by_targets(&steps, &make_alias_map(), &Identifier::new("v"));
        assert!(result.is_some());
        let targets = result.unwrap();
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn no_label_ref_returns_none() {
        let steps = vec![GremlinStep::Is(Predicate::Eq(GremlinValue::Integer(5)))];
        let result = extract_where_by_targets(&steps, &make_alias_map(), &Identifier::new("v"));
        assert!(result.is_none());
    }

    #[test]
    fn unmatched_pattern_returns_none() {
        let steps = vec![GremlinStep::Out(vec![])];
        let result = extract_where_by_targets(&steps, &make_alias_map(), &Identifier::new("v"));
        assert!(result.is_none());
    }

    // ── collect_predicate_labels_in_order ─────────────────────────────────

    #[test]
    fn collects_eq_label() {
        let pred = Predicate::Eq(GremlinValue::String("a".into()));
        let mut targets = vec![];
        collect_predicate_labels_in_order(&pred, &make_alias_map(), &mut targets);
        assert_eq!(targets.len(), 1);
    }

    #[test]
    fn collects_within_label() {
        let pred = Predicate::Within(vec![GremlinValue::String("a".into())]);
        let mut targets = vec![];
        collect_predicate_labels_in_order(&pred, &make_alias_map(), &mut targets);
        assert_eq!(targets.len(), 1);
    }

    #[test]
    fn deduplicates_labels() {
        let pred = Predicate::And(vec![
            Predicate::Eq(GremlinValue::String("a".into())),
            Predicate::Eq(GremlinValue::String("a".into())),
        ]);
        let mut targets = vec![];
        collect_predicate_labels_in_order(&pred, &make_alias_map(), &mut targets);
        assert_eq!(targets.len(), 1);
    }

    #[test]
    fn collects_from_or_compound() {
        let pred = Predicate::Or(vec![
            Predicate::Eq(GremlinValue::String("a".into())),
            Predicate::Eq(GremlinValue::String("b".into())),
        ]);
        let mut targets = vec![];
        collect_predicate_labels_in_order(&pred, &make_alias_map(), &mut targets);
        assert_eq!(targets.len(), 2);
    }

    #[test]
    fn collects_from_not() {
        let pred = Predicate::Not(Box::new(Predicate::Eq(GremlinValue::String("a".into()))));
        let mut targets = vec![];
        collect_predicate_labels_in_order(&pred, &make_alias_map(), &mut targets);
        assert_eq!(targets.len(), 1);
    }

    #[test]
    fn unknown_label_not_collected() {
        let pred = Predicate::Eq(GremlinValue::String("unknown".into()));
        let mut targets = vec![];
        collect_predicate_labels_in_order(&pred, &make_alias_map(), &mut targets);
        assert!(targets.is_empty());
    }

    // ── wrap_where_by_null_eq ────────────────────────────────────────────

    #[test]
    fn wraps_eq_with_null_check() {
        let mut expr = Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Variable(Identifier::new("a"))),
            right: Box::new(Expr::Variable(Identifier::new("b"))),
        };
        wrap_where_by_null_eq(&mut expr);
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Or,
                right,
                ..
            } => match right.as_ref() {
                Expr::BinaryOp {
                    op: BinaryOp::And,
                    left,
                    right,
                } => {
                    assert!(matches!(left.as_ref(), Expr::IsNull(_)));
                    assert!(matches!(right.as_ref(), Expr::IsNull(_)));
                }
                other => panic!("expected And null check, got {:?}", other),
            },
            other => panic!("expected Or wrapper, got {:?}", other),
        }
    }

    #[test]
    fn recurses_through_and() {
        let mut expr = Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Eq,
                left: Box::new(Expr::Variable(Identifier::new("a"))),
                right: Box::new(Expr::Variable(Identifier::new("b"))),
            }),
            right: Box::new(Expr::Bool(true)),
        };
        wrap_where_by_null_eq(&mut expr);
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::And,
                left,
                ..
            } => {
                assert!(matches!(
                    left.as_ref(),
                    Expr::BinaryOp {
                        op: BinaryOp::Or,
                        ..
                    }
                ));
            }
            other => panic!("expected And with Or left, got {:?}", other),
        }
    }

    #[test]
    fn non_eq_not_wrapped() {
        let mut expr = Expr::BinaryOp {
            op: BinaryOp::Gt,
            left: Box::new(Expr::Variable(Identifier::new("a"))),
            right: Box::new(Expr::Integer(5)),
        };
        wrap_where_by_null_eq(&mut expr);
        match &expr {
            Expr::BinaryOp {
                op: BinaryOp::Gt, ..
            } => {}
            other => panic!("expected Gt unchanged, got {:?}", other),
        }
    }

    // ── finalize_where_by_modulation ─────────────────────────────────────

    #[test]
    fn finalize_empty_targets() {
        let mut expr = Expr::Bool(true);
        finalize_where_by_modulation(&mut expr, &[], &["name".into()], false);
        assert_eq!(expr, Expr::Bool(true));
    }

    #[test]
    fn finalize_empty_modulators() {
        let target = WhereByTarget::Identifier(Identifier::new("v"));
        let mut expr = Expr::Bool(true);
        finalize_where_by_modulation(&mut expr, &[target], &[], false);
        assert_eq!(expr, Expr::Bool(true));
    }

    // ── rewrite_where_by_expr ────────────────────────────────────────────

    #[test]
    fn rewrite_variable_with_key() {
        let target = WhereByTarget::Identifier(Identifier::new("v"));
        let mut expr = Expr::Variable(Identifier::new("v"));
        rewrite_where_by_expr(&mut expr, &target, "name");
        match &expr {
            Expr::Property(base, key) => {
                assert_eq!(**base, Expr::Variable(Identifier::new("v")));
                assert_eq!(key.as_str(), "name");
            }
            other => panic!("expected Property, got {:?}", other),
        }
    }

    #[test]
    fn rewrite_variable_empty_key() {
        let target = WhereByTarget::Identifier(Identifier::new("v"));
        let mut expr = Expr::Variable(Identifier::new("v"));
        rewrite_where_by_expr(&mut expr, &target, "");
        assert_eq!(expr, Expr::Variable(Identifier::new("v")));
    }

    #[test]
    fn rewrite_map_entry_key_target() {
        let target = WhereByTarget::MapEntryKey(Identifier::new("v"));
        let mut expr = Expr::Variable(Identifier::new("v"));
        rewrite_where_by_expr(&mut expr, &target, "name");
        match &expr {
            Expr::Property(base, _) => match base.as_ref() {
                Expr::FunctionCall { name, .. } => {
                    assert_eq!(name.as_str(), "gremlin_unfold_map_entry_key");
                }
                other => panic!("expected FunctionCall, got {:?}", other),
            },
            other => panic!("expected Property, got {:?}", other),
        }
    }

    #[test]
    fn rewrite_map_entry_value_target() {
        let target = WhereByTarget::MapEntryValue(Identifier::new("v"));
        let mut expr = Expr::Variable(Identifier::new("v"));
        rewrite_where_by_expr(&mut expr, &target, "name");
        match &expr {
            Expr::Property(base, _) => match base.as_ref() {
                Expr::FunctionCall { name, .. } => {
                    assert_eq!(name.as_str(), "gremlin_unfold_map_entry_value");
                }
                other => panic!("expected FunctionCall, got {:?}", other),
            },
            other => panic!("expected Property, got {:?}", other),
        }
    }

    #[test]
    fn rewrite_recurses_binary_op() {
        let target = WhereByTarget::Identifier(Identifier::new("v"));
        let mut expr = Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Variable(Identifier::new("v"))),
            right: Box::new(Expr::Integer(5)),
        };
        rewrite_where_by_expr(&mut expr, &target, "name");
        match &expr {
            Expr::BinaryOp { left, .. } => {
                assert!(matches!(left.as_ref(), Expr::Property(..)));
            }
            other => panic!("expected BinaryOp, got {:?}", other),
        }
    }

    #[test]
    fn rewrite_property_already_present() {
        let target = WhereByTarget::Identifier(Identifier::new("v"));
        let mut expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new("v"))),
            SmolStr::new("old"),
        );
        rewrite_where_by_expr(&mut expr, &target, "new");
        match &expr {
            Expr::Property(_, key) => assert_eq!(key.as_str(), "new"),
            other => panic!("expected Property, got {:?}", other),
        }
    }

    // ── rewrite_where_by_target ──────────────────────────────────────────

    #[test]
    fn target_binary_op() {
        let target = WhereByTarget::Identifier(Identifier::new("v"));
        let mut expr = Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Variable(Identifier::new("v"))),
            right: Box::new(Expr::Variable(Identifier::new("v"))),
        };
        rewrite_where_by_target(&mut expr, &target, "name");
        match &expr {
            Expr::BinaryOp { left, right, .. } => {
                assert!(matches!(left.as_ref(), Expr::Property(..)));
                assert!(matches!(right.as_ref(), Expr::Property(..)));
            }
            other => panic!("expected BinaryOp, got {:?}", other),
        }
    }

    #[test]
    fn target_unary_op() {
        let target = WhereByTarget::Identifier(Identifier::new("v"));
        let mut expr = Expr::UnaryOp {
            op: UnaryOp::Not,
            expr: Box::new(Expr::Variable(Identifier::new("v"))),
        };
        rewrite_where_by_target(&mut expr, &target, "name");
        match &expr {
            Expr::UnaryOp { expr: inner, .. } => {
                assert!(matches!(inner.as_ref(), Expr::Property(..)));
            }
            other => panic!("expected UnaryOp, got {:?}", other),
        }
    }

    #[test]
    fn target_is_null() {
        let target = WhereByTarget::Identifier(Identifier::new("v"));
        let mut expr = Expr::IsNull(Box::new(Expr::Variable(Identifier::new("v"))));
        rewrite_where_by_target(&mut expr, &target, "name");
        match &expr {
            Expr::IsNull(inner) => {
                assert!(matches!(inner.as_ref(), Expr::Property(..)));
            }
            other => panic!("expected IsNull, got {:?}", other),
        }
    }
}
