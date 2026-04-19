//! Key/label/variable extraction utilities for `by`/`select`/`math` patterns.
//!
//! Extracted from `helpers.rs` as part of the helpers/ submodule split.
//! All items are `pub(in crate::compiler)` and behave identically to their pre-split
//! definitions.

use crate::bytecode::GremlinStep;

/// Extract a property key string from a simple `by(traversal)` pattern.
///
/// Recognizes patterns like:
///   `[Values(["name"])]`               → `"name"`
///   `[Label]`                           → `"~label"`
///   `[Id]`                              → `"~id"`
///   `[Select(_, ["src","tgt"])]`       → `"~select_map:src,tgt"`
///   `[Values(["name"]), ToUpper]`       → `"~toupper:name"`
///   `[Values(["name"]), ToLower]`       → `"~tolower:name"`
///   `[Unfold, Values(["name"]), Fold]`  → `"~unfold_values_fold:name"`
///   `[In(["created"]), Values(["age"]), Min]` → `"~in_values_min:created:age"`
///
/// Returns empty string for unrecognized patterns (identity behavior).
pub(in crate::compiler) fn extract_by_traversal_key(steps: &[GremlinStep]) -> String {
    match steps {
        [GremlinStep::Values(keys)] if keys.len() == 1 => keys[0].clone(),
        [GremlinStep::Label] => "~label".to_string(),
        [GremlinStep::Id] => "~id".to_string(),
        [GremlinStep::Select(_, keys)] if keys.len() > 1 => {
            format!("~select_map:{}", keys.join(","))
        }
        [GremlinStep::LimitLocal(n)] => format!("~limit_local:{n}"),
        [GremlinStep::TailLocal(n)] => format!("~tail_local:{n}"),
        [GremlinStep::Math(expr)] => format!("~math:{expr}"),
        [GremlinStep::Values(keys), GremlinStep::ToUpper] if keys.len() == 1 => {
            format!("~toupper:{}", keys[0])
        }
        [GremlinStep::Values(keys), GremlinStep::ToLower] if keys.len() == 1 => {
            format!("~tolower:{}", keys[0])
        }
        [GremlinStep::Unfold, GremlinStep::Values(keys), GremlinStep::Fold] if keys.len() == 1 => {
            format!("~unfold_values_fold:{}", keys[0])
        }
        [GremlinStep::In(labels), GremlinStep::Values(keys), GremlinStep::Min]
            if labels.len() == 1 && keys.len() == 1 =>
        {
            format!("~in_values_min:{}:{}", labels[0], keys[0])
        }
        // tail(local, N).values("prop") — for a single element, tail is identity
        [GremlinStep::TailLocal(_), GremlinStep::Values(keys)] if keys.len() == 1 => {
            keys[0].clone()
        }
        _ => String::new(),
    }
}

pub(in crate::compiler) fn extract_select_traversal_label(key: &str) -> Option<String> {
    let inner = key
        .strip_prefix("__.select(")
        .or_else(|| key.strip_prefix("select("))?;
    let body = inner.strip_suffix(')')?.trim();
    if body.contains(',') {
        return None;
    }
    let label = body.trim_matches(|c| c == '\'' || c == '"');
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

/// Extract variable names from a math expression.
/// Variables are non-numeric, non-"_" tokens that appear as operands in binary expressions.
pub(in crate::compiler) fn extract_math_variables(expr: &str) -> Vec<String> {
    let mut vars = Vec::new();
    let expr = expr.trim();
    // Check tokens around binary operators
    for op in &[" + ", " - ", " * ", " / ", " % "] {
        if let Some(pos) = expr.find(op) {
            let left = expr[..pos].trim();
            let right = expr[pos + op.len()..].trim();
            for token in &[left, right] {
                if *token != "_"
                    && !token.is_empty()
                    && token.parse::<f64>().is_err()
                    && !token.starts_with("ceil")
                    && !token.starts_with("floor")
                    && !token.starts_with("abs")
                    && !token.starts_with("sqrt")
                    && !vars.contains(&token.to_string())
                {
                    vars.push(token.to_string());
                }
            }
        }
    }
    vars
}
