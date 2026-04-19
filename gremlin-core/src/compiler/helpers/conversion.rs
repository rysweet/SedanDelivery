//! Conversions between `GremlinValue`, AST `Expr`, and `Value`.
//!
//! Extracted from `helpers.rs` as part of the helpers/ submodule split.
//! All items are `pub(in crate::compiler)` and behave identically to their pre-split
//! definitions.

use cypher_core::ast::{Expr, Identifier};
use cypher_core::types::Value;
use smol_str::SmolStr;

use crate::types::GremlinValue;

/// Convert a `GremlinValue` to an `Expr` literal for use in ID-comparison
/// contexts (V(id), E(id), hasId(id)).
///
/// TinkerPop allows string representations of numeric IDs: when a vertex
/// is looked up as `V("1")`, the string `"1"` should match the numeric node id
/// `1`. This helper tries to parse the string as an i64 and emits
/// `Expr::Integer` so the downstream filter `id(v) == 1` succeeds.
pub(in crate::compiler) fn gremlin_value_to_id_expr(gv: &GremlinValue) -> Expr {
    if let GremlinValue::String(s) = gv {
        if let Ok(n) = s.parse::<i64>() {
            return Expr::Integer(n);
        }
    }
    gremlin_value_to_expr(gv)
}

pub(in crate::compiler) fn is_gremlin_id_key(key: &str) -> bool {
    matches!(key, "~id" | "T.id" | "t[id]")
}

pub(in crate::compiler) fn gremlin_has_key_expr(key: &str, current_alias: &Identifier) -> Expr {
    if is_gremlin_id_key(key) {
        Expr::FunctionCall {
            name: SmolStr::new("id"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else {
        Expr::Property(
            Box::new(Expr::Variable(current_alias.clone())),
            SmolStr::new(key),
        )
    }
}

/// Convert a `GremlinValue` to an `Expr` literal.
pub(in crate::compiler) fn gremlin_value_to_expr(gv: &GremlinValue) -> Expr {
    match gv {
        GremlinValue::Null => Expr::Null,
        GremlinValue::Bool(b) => Expr::Bool(*b),
        GremlinValue::Integer(i) => Expr::Integer(*i),
        GremlinValue::Float(f) => Expr::Float(*f),
        GremlinValue::String(s) => {
            if let Some(iso) = s.strip_prefix("__gremlin_dt__:") {
                Expr::FunctionCall {
                    name: SmolStr::new("gremlin_make_datetime"),
                    args: vec![Expr::Str(iso.to_string().into_boxed_str())],
                    distinct: false,
                }
            } else if let Some(uuid) = s.strip_prefix("__gremlin_uuid__:") {
                Expr::FunctionCall {
                    name: SmolStr::new("gremlin_make_uuid"),
                    args: vec![Expr::Str(uuid.to_string().into_boxed_str())],
                    distinct: false,
                }
            } else {
                Expr::Str(s.clone().into_boxed_str())
            }
        }
        GremlinValue::List(items) => Expr::List(items.iter().map(gremlin_value_to_expr).collect()),
        GremlinValue::Map(entries) => Expr::MapLiteral(
            entries
                .iter()
                .map(|(k, v)| (SmolStr::new(k), gremlin_value_to_expr(v)))
                .collect(),
        ),
        GremlinValue::Vertex { id, .. } => Expr::Integer(*id),
        GremlinValue::Edge { id, .. } => Expr::Integer(*id),
        GremlinValue::Path { .. } => Expr::Null,
    }
}

/// Convert a `GremlinValue` to a core `Value` for use in ChooseMap option keys.
pub(in crate::compiler) fn gremlin_value_to_value(gv: &GremlinValue) -> Value {
    match gv {
        GremlinValue::Null => Value::Null,
        GremlinValue::Bool(b) => Value::Bool(*b),
        GremlinValue::Integer(i) => Value::Int(*i),
        GremlinValue::Float(f) => Value::Float(*f),
        GremlinValue::String(s) => Value::Str(s.clone().into_boxed_str()),
        GremlinValue::List(items) => {
            Value::List(items.iter().map(gremlin_value_to_value).collect())
        }
        GremlinValue::Map(_)
        | GremlinValue::Vertex { .. }
        | GremlinValue::Edge { .. }
        | GremlinValue::Path { .. } => Value::Null,
    }
}

/// Build an `Expr` from a `by(key)` modulator key string and the current alias.
/// Handles T.id, T.label, Column.keys/values, common traversal patterns, and property names.
pub(in crate::compiler) fn make_by_expr(key: &str, current_alias: &Identifier) -> Expr {
    if key == "~id" || key == "T.id" || key == "t[id]" {
        Expr::FunctionCall {
            name: SmolStr::new("id"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "~label"
        || key == "T.label"
        || key == "t[label]"
        || key == "__.label()"
        || key == "label()"
    {
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_single_label"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if let Some(prop) = key
        .strip_prefix("__.values(")
        .or_else(|| key.strip_prefix("values("))
        .and_then(|s| s.strip_suffix(')'))
    {
        // by(values("name")) or by(__.values("name")) → property access
        let prop = prop.trim_matches(|c| c == '\'' || c == '"');
        Expr::Property(
            Box::new(Expr::Variable(current_alias.clone())),
            SmolStr::new(prop),
        )
    } else if key == "~key" || key == "T.key" || key == "t[key]" {
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_property_key"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "~value" || key == "T.value" || key == "t[value]" {
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_property_value"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "Column.keys" || key == "column.keys" {
        // Extract the key from a single-entry Map produced by unfold() on a Map.
        // gremlin_unfold_map_entry_key returns a scalar Str for single-entry maps.
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_unfold_map_entry_key"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "Column.values" || key == "column.values" {
        // Extract the value from a single-entry Map produced by unfold() on a Map.
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_unfold_map_entry_value"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else {
        Expr::Property(
            Box::new(Expr::Variable(current_alias.clone())),
            SmolStr::new(key),
        )
    }
}
