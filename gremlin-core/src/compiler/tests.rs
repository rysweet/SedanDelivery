//! Compiler test suite. Extracted from the original `compiler.rs` body —
//! contents identical, only the file boundary changed.

use super::*;

#[test]
fn compile_v_scan() {
    let mut compiler = GremlinCompiler::new();
    let op = compiler.compile(&[GremlinStep::V(None)]).unwrap();
    assert!(matches!(op, Operator::NodeScan { label: None, .. }));
}

#[test]
fn compile_v_with_id() {
    let mut compiler = GremlinCompiler::new();
    let op = compiler
        .compile(&[GremlinStep::V(Some(vec![GremlinValue::Integer(1)]))])
        .unwrap();
    assert!(matches!(op, Operator::Filter { .. }));
}

#[test]
fn compile_v_with_string_id_normalizes_to_integer() {
    let mut compiler = GremlinCompiler::new();
    let op = compiler
        .compile(&[GremlinStep::V(Some(vec![GremlinValue::String("1".into())]))])
        .unwrap();
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                right,
                ..
            } if matches!(right.as_ref(), Expr::Integer(1))
        ));
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_has_filter() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into()))),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Filter { .. }));
}

#[test]
fn compile_out_expand() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Out(vec!["knows".into()])];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(
        op,
        Operator::Expand {
            direction: Direction::Outgoing,
            ..
        }
    ));
}

#[test]
fn compile_full_traversal() {
    // g.V().has("name","marko").out("knows").values("name")
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into()))),
        GremlinStep::Out(vec!["knows".into()]),
        GremlinStep::Values(vec!["name".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    // Top-level should be Filter (null filter from values) wrapping Project
    assert!(matches!(op, Operator::Filter { .. }));

    // Walk down to verify structure
    if let Operator::Filter { child: proj, .. } = &op {
        if let Operator::Project { child, items, .. } = proj.as_ref() {
            assert_eq!(items.len(), 1);
            assert!(matches!(**child, Operator::Expand { .. }));
        }
    }
}

#[test]
fn compile_count() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Count];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Aggregate { .. }));
}

#[test]
fn extract_by_traversal_key_supports_math() {
    assert_eq!(
        extract_by_traversal_key(&[GremlinStep::Math("_+_".into())]),
        "~math:_+_"
    );
}

#[test]
fn extract_by_traversal_key_supports_in_values_min() {
    assert_eq!(
        extract_by_traversal_key(&[
            GremlinStep::In(vec!["created".into()]),
            GremlinStep::Values(vec!["age".into()]),
            GremlinStep::Min,
        ]),
        "~in_values_min:created:age"
    );
}

#[test]
fn extract_by_traversal_key_supports_multi_select_map() {
    assert_eq!(
        extract_by_traversal_key(&[GremlinStep::Select(None, vec!["src".into(), "tgt".into()],)]),
        "~select_map:src,tgt"
    );
}

#[test]
fn extract_select_traversal_label_supports_simple_select() {
    assert_eq!(
        extract_select_traversal_label(r#"__.select("a")"#).as_deref(),
        Some("a")
    );
}

#[test]
fn compile_order_by_limit() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Order,
        GremlinStep::By("age".into(), true),
        GremlinStep::Limit(10),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Limit { .. }));
    if let Operator::Limit { child, .. } = &op {
        assert!(matches!(**child, Operator::Sort { .. }));
    }
}

#[test]
fn compile_dedup() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Dedup(vec![])];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Distinct { .. }));
}

#[test]
fn compile_add_v() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::AddV("person".into()),
        GremlinStep::Property("name".into(), GremlinValue::String("alice".into())),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Set { .. }));
}

#[test]
fn compile_drop() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Drop];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Delete { detach: true, .. }));
}

#[test]
fn compile_has_label() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::HasLabel(vec!["person".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Filter { .. }));
}

#[test]
fn compile_in_expand() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::In(vec!["created".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(
        op,
        Operator::Expand {
            direction: Direction::Incoming,
            ..
        }
    ));
}

#[test]
fn compile_empty_steps_returns_single_row() {
    let mut compiler = GremlinCompiler::new();
    let op = compiler.compile(&[]).unwrap();
    assert!(matches!(op, Operator::SingleRow));
}

#[test]
fn compile_range() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Range(5, 15)];
    let op = compiler.compile(&steps).unwrap();
    // Should be Limit wrapping Skip
    assert!(matches!(op, Operator::Limit { .. }));
    if let Operator::Limit { child, count } = &op {
        assert!(matches!(count, Expr::Integer(10)));
        assert!(matches!(**child, Operator::Skip { .. }));
    }
}

// ── New tests for expanded compiler coverage ─────────────────────────────

#[test]
fn compile_e_scan() {
    let mut compiler = GremlinCompiler::new();
    let op = compiler.compile(&[GremlinStep::E(None)]).unwrap();
    // E(None) → Expand (all edges) over NodeScan
    assert!(matches!(op, Operator::Expand { .. }));
    if let Operator::Expand {
        child, rel_types, ..
    } = &op
    {
        assert!(rel_types.is_empty());
        assert!(matches!(**child, Operator::NodeScan { .. }));
    }
}

#[test]
fn compile_e_with_single_id() {
    let mut compiler = GremlinCompiler::new();
    let op = compiler
        .compile(&[GremlinStep::E(Some(vec![GremlinValue::Integer(42)]))])
        .unwrap();
    assert!(matches!(op, Operator::Filter { .. }));
    if let Operator::Filter { child, .. } = &op {
        assert!(matches!(**child, Operator::Expand { .. }));
    }
}

#[test]
fn compile_e_with_string_id_normalizes_to_integer() {
    let mut compiler = GremlinCompiler::new();
    let op = compiler
        .compile(&[GremlinStep::E(Some(vec![GremlinValue::String(
            "42".into(),
        )]))])
        .unwrap();
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                right,
                ..
            } if matches!(right.as_ref(), Expr::Integer(42))
        ));
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_e_with_multiple_ids() {
    let mut compiler = GremlinCompiler::new();
    let op = compiler
        .compile(&[GremlinStep::E(Some(vec![
            GremlinValue::Integer(1),
            GremlinValue::Integer(2),
        ]))])
        .unwrap();
    assert!(matches!(op, Operator::Filter { .. }));
}

#[test]
fn compile_has_existence() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Has("age".into(), None)];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Filter { .. }));
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(predicate, Expr::IsNotNull(_)));
    }
}

#[test]
fn compile_has_id_single() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::HasId(vec![GremlinValue::Integer(7)]),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Filter { .. }));
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                ..
            }
        ));
    }
}

#[test]
fn compile_has_id_string_normalizes_to_integer() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::HasId(vec![GremlinValue::String("7".into())]),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                right,
                ..
            } if matches!(right.as_ref(), Expr::Integer(7))
        ));
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_has_tid_string_uses_id_function_and_integer_rhs() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Has("~id".into(), Some(GremlinValue::String("10".into()))),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                left,
                right,
            } if matches!(
                left.as_ref(),
                Expr::FunctionCall { name, .. } if name.as_str() == "id"
            ) && matches!(right.as_ref(), Expr::Integer(10))
        ));
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_has_id_multiple() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::HasId(vec![
            GremlinValue::Integer(1),
            GremlinValue::Integer(2),
            GremlinValue::Integer(3),
        ]),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Filter { .. }));
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::In,
                ..
            }
        ));
    }
}

#[test]
fn compile_has_label_multiple() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::HasLabel(vec!["person".into(), "software".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Filter { .. }));
    if let Operator::Filter { predicate, .. } = &op {
        // Multiple labels → OR of HasLabel checks
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Or,
                ..
            }
        ));
    }
}

#[test]
fn compile_both_expand() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Both(vec!["knows".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(
        op,
        Operator::Expand {
            direction: Direction::Undirected,
            ..
        }
    ));
}

#[test]
fn compile_out_e() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::OutE(vec!["created".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Expand {
        direction,
        rel_alias,
        ..
    } = &op
    {
        assert_eq!(*direction, Direction::Outgoing);
        assert!(rel_alias.is_some(), "outE should set rel_alias");
    } else {
        panic!("expected Expand");
    }
}

#[test]
fn compile_in_e() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::InE(vec!["knows".into()])];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Expand {
        direction,
        rel_alias,
        ..
    } = &op
    {
        assert_eq!(*direction, Direction::Incoming);
        assert!(rel_alias.is_some());
    } else {
        panic!("expected Expand");
    }
}

#[test]
fn compile_both_e() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::BothE(vec![])];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Expand {
        direction,
        rel_alias,
        ..
    } = &op
    {
        assert_eq!(*direction, Direction::Undirected);
        assert!(rel_alias.is_some());
    } else {
        panic!("expected Expand");
    }
}

#[test]
fn compile_in_v() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::OutE(vec![]),
        GremlinStep::InV,
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Project { .. }));
}

#[test]
fn compile_out_v() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::OutE(vec![]),
        GremlinStep::OutV,
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Project { .. }));
}

#[test]
fn compile_values_multiple_keys() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Values(vec!["name".into(), "age".into(), "city".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    // Multiple keys: Project (with gremlin_values_keys) → Unwind
    if let Operator::Unwind { child, .. } = &op {
        assert!(matches!(**child, Operator::Project { .. }));
    } else {
        panic!("expected Unwind(Project)");
    }
}

#[test]
fn compile_value_map_with_keys() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::ValueMap(vec!["name".into(), "age".into()], 0),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Project { items, .. } = &op {
        // Single gremlin_valuemap call with element + key args
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0].0, Expr::FunctionCall { .. }));
    } else {
        panic!("expected Project");
    }
}

#[test]
fn compile_value_map_empty() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::ValueMap(vec![], 0)];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Project { items, .. } = &op {
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0].0, Expr::FunctionCall { .. }));
    } else {
        panic!("expected Project");
    }
}

#[test]
fn compile_properties_with_keys() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Properties(vec!["name".into(), "age".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    // Multi-key properties() produces Unwind { Project { list_compact(...) } }
    assert!(matches!(&op, Operator::Unwind { .. }), "expected Unwind");
}

#[test]
fn compile_id_step() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Id];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Project { items, .. } = &op {
        assert_eq!(items.len(), 1);
        assert!(matches!(
            &items[0].0,
            Expr::FunctionCall { name, .. } if name.as_str() == "id"
        ));
    } else {
        panic!("expected Project");
    }
}

#[test]
fn compile_label_step() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Label];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Project { items, .. } = &op {
        assert_eq!(items.len(), 1);
        assert!(matches!(
            &items[0].0,
            Expr::FunctionCall { name, .. } if name.as_str() == "gremlin_single_label"
        ));
    } else {
        panic!("expected Project");
    }
}

#[test]
fn compile_sum() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Values(vec!["age".into()]),
        GremlinStep::Sum,
    ];
    let op = compiler.compile(&steps).unwrap();
    // Sum now uses Filter { Project { gremlin_sum_local, Aggregate(Collect) } }
    assert!(matches!(op, Operator::Filter { .. }));
}

#[test]
fn compile_min() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Values(vec!["age".into()]),
        GremlinStep::Min,
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Filter { child, .. } = &op {
        if let Operator::Aggregate { aggregations, .. } = child.as_ref() {
            assert!(matches!(aggregations[0].0, AggFunction::Min));
        } else {
            panic!("expected Aggregate inside Filter");
        }
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_max() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Values(vec!["age".into()]),
        GremlinStep::Max,
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Filter { child, .. } = &op {
        if let Operator::Aggregate { aggregations, .. } = child.as_ref() {
            assert!(matches!(aggregations[0].0, AggFunction::Max));
        } else {
            panic!("expected Aggregate inside Filter");
        }
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_mean() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Values(vec!["age".into()]),
        GremlinStep::Mean,
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Filter { child, .. } = &op {
        if let Operator::Aggregate { aggregations, .. } = child.as_ref() {
            assert!(matches!(aggregations[0].0, AggFunction::Avg));
        } else {
            panic!("expected Aggregate inside Filter");
        }
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_group() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Group(None)];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::GremlinGroup { agg_fn, .. } = &op {
        assert!(matches!(agg_fn, AggFunction::Collect));
    } else {
        panic!("expected GremlinGroup");
    }
}

#[test]
fn compile_group_count() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::GroupCount(None)];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::GremlinGroup { agg_fn, .. } = &op {
        assert!(matches!(agg_fn, AggFunction::Count));
    } else {
        panic!("expected GremlinGroup");
    }
}

#[test]
fn compile_fold() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Fold];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Aggregate {
        keys, aggregations, ..
    } = &op
    {
        assert!(keys.is_empty(), "fold has no group keys");
        assert!(matches!(aggregations[0].0, AggFunction::CollectAll));
    } else {
        panic!("expected Aggregate");
    }
}

#[test]
fn compile_unfold() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Fold, GremlinStep::Unfold];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Unwind { .. }));
}

#[test]
fn compile_as_select() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::As("a".into()),
        GremlinStep::Out(vec!["knows".into()]),
        GremlinStep::As("b".into()),
        GremlinStep::Select(None, vec!["a".into(), "b".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::GremlinSelect { keys, .. } = &op {
        assert_eq!(keys.len(), 2);
        assert_eq!(keys[0].1, "a");
        assert_eq!(keys[1].1, "b");
    } else {
        panic!("expected GremlinSelect");
    }
}

#[test]
fn compile_select_by_traversal_records_sub_traversal() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::As("a".into()),
        GremlinStep::Select(None, vec!["a".into()]),
        GremlinStep::ByTraversal(vec![GremlinStep::Values(vec!["name".into()])], false),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::GremlinSelect { by_traversals, .. } = &op {
        assert!(matches!(
            by_traversals.first(),
            Some(Some(GremlinSelectTraversal::Traversal { .. }))
        ));
    } else {
        panic!("expected GremlinSelect");
    }
}

#[test]
fn compile_where_filter() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Where(vec![GremlinStep::Has(
            "age".into(),
            Some(GremlinValue::Integer(30)),
        )]),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Filter { .. }));
}

#[test]
fn finalize_where_by_modulation_cycles_targets() {
    let a = Identifier::new("a");
    let b = Identifier::new("b");
    let c = Identifier::new("c");
    let d = Identifier::new("d");
    let mut predicate = Expr::BinaryOp {
        op: BinaryOp::And,
        left: Box::new(Expr::BinaryOp {
            op: BinaryOp::Or,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(Expr::Variable(a.clone())),
                right: Box::new(Expr::Variable(b.clone())),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(Expr::Variable(a.clone())),
                right: Box::new(Expr::Variable(c.clone())),
            }),
        }),
        right: Box::new(Expr::BinaryOp {
            op: BinaryOp::Neq,
            left: Box::new(Expr::Variable(a.clone())),
            right: Box::new(Expr::Variable(d.clone())),
        }),
    };

    finalize_where_by_modulation(
        &mut predicate,
        &[
            WhereByTarget::Identifier(a.clone()),
            WhereByTarget::Identifier(b.clone()),
            WhereByTarget::Identifier(c.clone()),
            WhereByTarget::Identifier(d.clone()),
        ],
        &[
            "age".to_string(),
            "weight".to_string(),
            "~in_values_min:created:age".to_string(),
        ],
        false,
    );

    let Expr::BinaryOp { left, right, .. } = &predicate else {
        panic!("expected top-level binary op");
    };
    let Expr::BinaryOp {
        left: first,
        right: second,
        ..
    } = left.as_ref()
    else {
        panic!("expected OR branch");
    };
    let Expr::BinaryOp {
        left: first_left,
        right: first_right,
        ..
    } = first.as_ref()
    else {
        panic!("expected first comparison");
    };
    assert_eq!(
        *first_left.clone(),
        Expr::Property(Box::new(Expr::Variable(a.clone())), SmolStr::new("age"))
    );
    assert_eq!(
        *first_right.clone(),
        Expr::Property(Box::new(Expr::Variable(b.clone())), SmolStr::new("weight"))
    );

    let Expr::BinaryOp {
        left: second_left,
        right: second_right,
        ..
    } = second.as_ref()
    else {
        panic!("expected second comparison");
    };
    assert_eq!(
        *second_left.clone(),
        Expr::Property(Box::new(Expr::Variable(a.clone())), SmolStr::new("age"))
    );
    assert_eq!(
        *second_right.clone(),
        Expr::Property(
            Box::new(Expr::Variable(c.clone())),
            SmolStr::new("~in_values_min:created:age")
        )
    );

    let Expr::BinaryOp {
        left: third_left,
        right: third_right,
        ..
    } = right.as_ref()
    else {
        panic!("expected third comparison");
    };
    assert_eq!(
        *third_left.clone(),
        Expr::Property(Box::new(Expr::Variable(a)), SmolStr::new("age"))
    );
    assert_eq!(
        *third_right.clone(),
        Expr::Property(Box::new(Expr::Variable(d)), SmolStr::new("age"))
    );
}

#[test]
fn compile_not_filter() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Not(vec![GremlinStep::Has(
            "deleted".into(),
            Some(GremlinValue::Bool(true)),
        )]),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::UnaryOp {
                op: cypher_core::ast::UnaryOp::Not,
                ..
            }
        ));
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_and_filter() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::And(vec![
            vec![GremlinStep::Has(
                "name".into(),
                Some(GremlinValue::String("marko".into())),
            )],
            vec![GremlinStep::Has(
                "age".into(),
                Some(GremlinValue::Integer(29)),
            )],
        ]),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::And,
                ..
            }
        ));
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_or_filter() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Or(vec![
            vec![GremlinStep::HasLabel(vec!["person".into()])],
            vec![GremlinStep::HasLabel(vec!["software".into()])],
        ]),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Or,
                ..
            }
        ));
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_is_predicate() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Values(vec!["age".into()]),
        GremlinStep::Is(Predicate::Gt(GremlinValue::Integer(25))),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                ..
            }
        ));
    } else {
        panic!("expected Filter");
    }
}

#[test]
fn compile_constant() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::Constant(GremlinValue::Integer(42))];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Project { items, .. } = &op {
        assert_eq!(items.len(), 1);
        assert!(matches!(&items[0].0, Expr::Integer(42)));
    } else {
        panic!("expected Project");
    }
}

#[test]
fn loops_is_predicate_targets_loop_counter_column() {
    let pred = steps_to_predicate(
        &[
            GremlinStep::Loops(Some("a".into())),
            GremlinStep::Is(Predicate::Eq(GremlinValue::Integer(0))),
        ],
        &Identifier::new("v_1"),
        &HashMap::new(),
        &HashSet::new(),
    );
    let rendered = format!("{pred:?}");
    assert!(rendered.contains("__gremlin_loop_count_a"));
}

#[test]
fn compile_inject() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::Inject(vec![
        GremlinValue::Integer(1),
        GremlinValue::Integer(2),
        GremlinValue::Integer(3),
    ])];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Unwind { .. }));
    if let Operator::Unwind { list_expr, .. } = &op {
        if let Expr::List(items) = list_expr {
            assert_eq!(items.len(), 3);
        } else {
            panic!("expected List expr");
        }
    }
}

#[test]
fn compile_union() {
    let mut compiler = GremlinCompiler::new();
    // Both branches start with V() (own source) → child is not embedded.
    // The parent V() is a real operator (not SingleRow) so embed_child is
    // true, but has_source is also true for each branch → compile_seeded.
    // Result is a Union (no Apply wrapper).
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Union(vec![
            vec![GremlinStep::V(None), GremlinStep::Out(vec!["knows".into()])],
            vec![
                GremlinStep::V(None),
                GremlinStep::Out(vec!["created".into()]),
            ],
        ]),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Union { .. }));
}

#[test]
fn compile_top_level_union_with_source_branches_without_apply() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::Union(vec![
        vec![
            GremlinStep::V(None),
            GremlinStep::Out(vec![]),
            GremlinStep::Out(vec![]),
        ],
        vec![
            GremlinStep::V(None),
            GremlinStep::HasLabel(vec!["software".into()]),
        ],
    ])];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Union { .. }));
}

#[test]
fn compile_nested_union_path_does_not_leak_seed_alias_into_path_acc() {
    let resolve = |_name: &str| -> Option<GremlinValue> { None };
    let steps = crate::text_parser::parse_traversal(
        r#"g.union(__.union(__.V().out().out()), __.V().hasLabel("software")).path().by("name")"#,
        &resolve,
    )
    .unwrap();
    let mut compiler = GremlinCompiler::new();
    let op = compiler.compile(&steps).unwrap();
    let rendered = format!("{op:#?}");
    assert!(!rendered.contains("\"__gremlin_v\""));
}

#[test]
fn compile_seeded_union_uses_stream_semantics_not_apply() {
    let mut compiler = GremlinCompiler::new();
    let op = compiler
        .compile_seeded(
            &[GremlinStep::Union(vec![
                vec![
                    GremlinStep::Constant(GremlinValue::String("y".into())),
                    GremlinStep::Limit(1),
                ],
                vec![GremlinStep::Identity],
            ])],
            &Identifier::new("n"),
        )
        .unwrap();
    assert!(matches!(op, Operator::Union { .. }));
}

#[test]
fn compile_repeat_times() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Repeat(
            None,
            vec![GremlinStep::V(None), GremlinStep::Out(vec!["knows".into()])],
        ),
        GremlinStep::Times(3),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Repeat { times, body, .. } = &op {
        assert_eq!(*times, Some(3));
        assert!(matches!(**body, Operator::Expand { .. }));
    } else {
        panic!("expected Repeat");
    }
}

#[test]
fn compile_choose() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Choose(
            vec![
                GremlinStep::V(None),
                GremlinStep::Has("age".into(), Some(GremlinValue::Integer(30))),
            ],
            vec![
                GremlinStep::V(None),
                GremlinStep::Values(vec!["name".into()]),
            ],
            Some(vec![GremlinStep::Constant(GremlinValue::String(
                "unknown".into(),
            ))]),
        ),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Choose {
        if_true, if_false, ..
    } = &op
    {
        // values() now wraps Project in Filter (null filtering)
        assert!(matches!(
            **if_true,
            Operator::Filter { .. } | Operator::Project { .. }
        ));
        assert!(if_false.is_some());
    } else {
        panic!("expected Choose");
    }
}

#[test]
fn compile_coalesce() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Coalesce(vec![
            vec![
                GremlinStep::V(None),
                GremlinStep::Values(vec!["nickname".into()]),
            ],
            vec![
                GremlinStep::V(None),
                GremlinStep::Values(vec!["name".into()]),
            ],
        ]),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Coalesce { alternatives, .. } = &op {
        assert_eq!(alternatives.len(), 2);
    } else {
        panic!("expected Coalesce");
    }
}

#[test]
fn compile_order_default_sort() {
    // Order without By → default ascending sort on current alias
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Order];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Sort { items, .. } = &op {
        assert_eq!(items.len(), 1);
        assert!(items[0].1, "default sort should be ascending");
    } else {
        panic!("expected Sort");
    }
}

#[test]
fn compile_v_multiple_ids() {
    let mut compiler = GremlinCompiler::new();
    let op = compiler
        .compile(&[GremlinStep::V(Some(vec![
            GremlinValue::Integer(1),
            GremlinValue::Integer(2),
            GremlinValue::Integer(3),
        ]))])
        .unwrap();
    if let Operator::Filter { predicate, .. } = &op {
        assert!(matches!(
            predicate,
            Expr::BinaryOp {
                op: BinaryOp::In,
                ..
            }
        ));
    } else {
        panic!("expected Filter for multi-id V()");
    }
}

#[test]
fn compile_path_produces_gremlin_path() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Path];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::GremlinPath { .. }));
}

#[test]
fn compile_match_produces_gremlin_match() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Match(vec![vec![GremlinStep::Out(vec!["knows".into()])]]),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::GremlinMatch { .. }));
}

#[test]
fn compile_aggregate_produces_gremlin_aggregate() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Aggregate("x".into())];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::GremlinAggregate { .. }));
}

#[test]
fn compile_add_e() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::AddV("person".into()),
        GremlinStep::AddE("knows".into()),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::Create { .. }));
}

#[test]
fn compile_add_e_to_label() {
    // g.V(1).as("a").out("created").addE("createdBy").to("a")
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(Some(vec![GremlinValue::Integer(1)])),
        GremlinStep::As("a".into()),
        GremlinStep::Out(vec!["created".into()]),
        GremlinStep::AddE("createdBy".into()),
        GremlinStep::To(vec![GremlinStep::Select(None, vec!["a".into()])]),
    ];
    let op = compiler.compile(&steps).unwrap();
    // Verify Create operator has non-placeholder src/dst
    fn check_create(op: &Operator) -> bool {
        match op {
            Operator::Create { patterns, .. } => {
                for pat in patterns {
                    for edge in &pat.edges {
                        if edge.1.as_str() == "__src" || edge.2.as_str() == "__dst" {
                            return false;
                        }
                    }
                }
                true
            }
            Operator::Apply { outer, inner } => check_create(outer) && check_create(inner),
            Operator::Set { child, .. } => check_create(child),
            _ => true,
        }
    }
    assert!(
        check_create(&op),
        "Create operator still has __src or __dst placeholders"
    );
}

#[test]
fn compile_property_set() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Property("status".into(), GremlinValue::String("active".into())),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Set { items, .. } = &op {
        assert_eq!(items.len(), 1);
        if let SetItem::Property { key, .. } = &items[0] {
            assert_eq!(key, "status");
        } else {
            panic!("expected SetItem::Property");
        }
    } else {
        panic!("expected Set");
    }
}

#[test]
fn compile_limit() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Limit(5)];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Limit { count, .. } = &op {
        assert!(matches!(count, Expr::Integer(5)));
    } else {
        panic!("expected Limit");
    }
}

#[test]
fn compile_range_zero_start() {
    // range(0, 10) → Limit only (no Skip needed)
    let mut compiler = GremlinCompiler::new();
    let steps = vec![GremlinStep::V(None), GremlinStep::Range(0, 10)];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Limit { count, child } = &op {
        assert!(matches!(count, Expr::Integer(10)));
        // No skip wrapping since low=0
        assert!(matches!(**child, Operator::NodeScan { .. }));
    } else {
        panic!("expected Limit");
    }
}

#[test]
fn compile_side_effect() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::SideEffect(vec![
            GremlinStep::V(None),
            GremlinStep::Property("visited".into(), GremlinValue::Bool(true)),
        ]),
    ];
    let op = compiler.compile(&steps).unwrap();
    assert!(matches!(op, Operator::GremlinSideEffect { .. }));
}

#[test]
fn compile_cap() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Cap(vec!["x".into(), "y".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Cap { keys, .. } = &op {
        assert_eq!(keys, &["x", "y"]);
    } else {
        panic!("expected Cap");
    }
}

#[test]
fn compile_project_step() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Project(vec!["name".into(), "age".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    if let Operator::Project { items, .. } = &op {
        assert_eq!(items.len(), 2);
    } else {
        panic!("expected Project");
    }
}

#[test]
fn compile_predicate_at_toplevel_errors() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(None),
        GremlinStep::Eq(GremlinValue::Integer(1)),
    ];
    let err = compiler.compile(&steps).unwrap_err();
    assert!(matches!(err, CompileError::InvalidSequence(_)));
}

#[test]
fn compile_project_by_traversal() {
    let no_param = |_: &str| -> Option<GremlinValue> { None };
    let steps = crate::text_parser::parse_inner_traversal("__.inE().count()", &no_param).unwrap();
    eprintln!("Parsed sub-traversal steps: {steps:?}");
    assert!(!steps.is_empty(), "Should parse to non-empty steps");

    let input = "g.V().project(\"a\", \"b\").by(__.inE().count()).by(\"age\")";
    let full = crate::text_parser::parse_traversal(input, &no_param).unwrap();
    eprintln!("Full parsed steps: {full:?}");

    let mut compiler = GremlinCompiler::new();
    let op = compiler.compile(&full).unwrap();
    eprintln!("Compiled operator: {op:?}");
    // Should be GremlinProject, not just Project
    let op_str = format!("{op:?}");
    assert!(
        op_str.contains("GremlinProject"),
        "Expected GremlinProject, got: {op_str}"
    );
}

#[test]
fn compile_format_by_traversal() {
    let no_param = |_: &str| -> Option<GremlinValue> { None };
    let input = r#"g.V().format("%{name} has %{_} connections").by(bothE().count())"#;
    let full = crate::text_parser::parse_traversal(input, &no_param).unwrap();
    let mut compiler = GremlinCompiler::new();
    let op = compiler.compile(&full).unwrap();
    let op_str = format!("{op:?}");
    assert!(
        op_str.contains("gremlin_format_by"),
        "Expected gremlin_format_by in compiled plan"
    );
}

#[test]
fn debug_compile_v_id_out() {
    let mut compiler = GremlinCompiler::new();
    let steps = vec![
        GremlinStep::V(Some(vec![GremlinValue::Integer(1)])),
        GremlinStep::Out(vec!["knows".into()]),
    ];
    let op = compiler.compile(&steps).unwrap();
    eprintln!("Compiled op: {:#?}", op);
}
