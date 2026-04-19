//! Property-map and label filter expression construction.

use crate::ast::{Expr, Identifier, Label, Properties};

/// Convert an `Option<&Properties>` to a `Vec<(String, Expr)>` for the planner operator layer.
///
/// For `Properties::Map`, converts inline property pairs.
/// For `Properties::Parameter`, emits a single sentinel entry with an empty key
/// and an `Expr::Parameter` value. The executor detects this sentinel and expands
/// the parameter map at runtime.
pub(crate) fn properties_to_prop_pairs(props: Option<&Properties>) -> Vec<(String, Expr)> {
    match props {
        Some(Properties::Map(map)) => map
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        Some(Properties::Parameter(name)) => {
            // Sentinel: empty key signals "expand this parameter map at runtime".
            vec![("".to_string(), Expr::Parameter(name.to_string()))]
        }
        None => Vec::new(),
    }
}

/// Build a filter expression that checks whether an entity's properties match
/// all key-value pairs from a parameter map.
///
/// Generates `propertiesMatch(alias, $param_name)` which the executor evaluates
/// by comparing all entries in the resolved parameter map against the entity's
/// properties.
pub(crate) fn param_props_filter_expr(alias: &Identifier, param_name: &smol_str::SmolStr) -> Expr {
    Expr::FunctionCall {
        name: "propertiesMatch".into(),
        args: vec![
            Expr::Variable(alias.clone()),
            Expr::Parameter(param_name.to_string()),
        ],
        distinct: false,
    }
}

pub(crate) fn props_to_filter_expr(
    alias: &Identifier,
    props: &[(smol_str::SmolStr, Expr)],
) -> Expr {
    let mut parts: Vec<Expr> = props
        .iter()
        .map(|(key, val)| Expr::BinaryOp {
            op: crate::ast::BinaryOp::Eq,
            left: Box::new(Expr::Property(
                Box::new(Expr::Variable(alias.clone())),
                key.clone(),
            )),
            right: Box::new(val.clone()),
        })
        .collect();

    if parts.len() == 1 {
        parts.remove(0)
    } else {
        parts
            .into_iter()
            .reduce(|acc, e| Expr::BinaryOp {
                op: crate::ast::BinaryOp::And,
                left: Box::new(acc),
                right: Box::new(e),
            })
            .unwrap_or(Expr::Bool(true))
    }
}

/// Build a filter expression that checks whether a node has a specific label.
///
/// In Phase 2, label membership is enforced at the `NodeScan` level.  After an
/// `Expand`, the executor does not re-check labels automatically.  We synthesise
/// a `HAS LABEL` predicate using `FunctionCall { name: "hasLabel", ... }` which
/// `eval_expr` understands (see filter.rs).
pub(crate) fn label_filter_expr(alias: &Identifier, label: &Label) -> Expr {
    // Use hasLabel() built-in which is evaluated by eval_expr in filter.rs.
    Expr::FunctionCall {
        name: "hasLabel".into(),
        args: vec![
            Expr::Variable(alias.clone()),
            Expr::Str(label.as_str().into()),
        ],
        distinct: false,
    }
}
