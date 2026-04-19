//! Gremlin executor: shared helpers and the `JavaCompatibleRandom` RNG.

use indexmap::IndexMap;

use crate::ast::{Direction, Expr, Identifier};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::Row;

pub(super) struct JavaCompatibleRandom {
    seed: u64,
}

impl JavaCompatibleRandom {
    const MULTIPLIER: u64 = 0x5DEECE66D;
    const ADDEND: u64 = 0xB;
    const MASK: u64 = (1u64 << 48) - 1;

    pub(super) fn new(seed: i64) -> Self {
        Self {
            seed: ((seed as u64) ^ Self::MULTIPLIER) & Self::MASK,
        }
    }

    pub(super) fn next(&mut self, bits: u32) -> u32 {
        self.seed = (self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::ADDEND))
            & Self::MASK;
        (self.seed >> (48 - bits)) as u32
    }

    pub(super) fn next_double(&mut self) -> f64 {
        let high = self.next(26) as u64;
        let low = self.next(27) as u64;
        ((high << 27) | low) as f64 / ((1u64 << 53) as f64)
    }
}

/// Convert a Value to a string key for use in Gremlin group() Map output.
pub(super) fn value_to_map_key(v: &Value) -> Box<str> {
    match v {
        Value::Str(s) => s.clone(),
        Value::Int(i) => format!("{i}").into_boxed_str(),
        Value::Float(f) => {
            let s = format!("{f}");
            // Ensure floats always have a decimal point for consistent key format
            if s.contains('.') || s.contains('E') || s.contains('e') {
                s.into_boxed_str()
            } else {
                format!("{s}.0").into_boxed_str()
            }
        }
        Value::Bool(b) => format!("{b}").into_boxed_str(),
        Value::Null => "null".to_string().into_boxed_str(),
        Value::Node(nid) => format!("v[{}]", nid.0).into_boxed_str(),
        Value::Edge(eid) => format!("e[{}]", eid.0).into_boxed_str(),
        Value::List(items) => {
            // Gremlin list key format: l[elem1,elem2,...] (no spaces)
            let parts: Vec<String> = items
                .iter()
                .map(|item| value_to_map_key(item).to_string())
                .collect();
            format!("l[{}]", parts.join(",")).into_boxed_str()
        }
        Value::Map(m) => {
            let parts: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{k}={}", value_to_map_key(v)))
                .collect();
            format!("{{{}}}", parts.join(", ")).into_boxed_str()
        }
        _ => format!("{v:?}").into_boxed_str(),
    }
}

pub(super) fn merge_gremlin_map_entry(
    map: &mut IndexMap<Box<str>, Value>,
    key: &str,
    incoming: &Value,
) {
    match map.entry(Box::from(key)) {
        indexmap::map::Entry::Vacant(entry) => {
            entry.insert(incoming.clone());
        }
        indexmap::map::Entry::Occupied(mut entry) => match (entry.get().clone(), incoming) {
            (Value::Int(a), Value::Int(b)) => {
                entry.insert(Value::Int(a + b));
            }
            (Value::Float(a), Value::Float(b)) => {
                entry.insert(Value::Float(a + b));
            }
            (Value::List(mut a), Value::List(b)) => {
                a.extend(b.iter().cloned());
                entry.insert(Value::List(a));
            }
            (_, other) => {
                entry.insert(other.clone());
            }
        },
    }
}

pub(super) fn summarize_gremlin_value_shape(value: &Value, depth: usize) -> String {
    if depth == 0 {
        return "...".to_string();
    }
    match value {
        Value::List(items) => {
            let sample = items
                .iter()
                .take(3)
                .map(|item| summarize_gremlin_value_shape(item, depth - 1))
                .collect::<Vec<_>>()
                .join(", ");
            format!("list(len={}, sample=[{}])", items.len(), sample)
        }
        Value::Map(entries) => {
            let sample = entries
                .iter()
                .take(2)
                .map(|(k, v)| format!("{k}:{}", summarize_gremlin_value_shape(v, depth - 1)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("map(len={}, sample={{{}}})", entries.len(), sample)
        }
        Value::Node(nid) => format!("node({})", nid.0),
        Value::Edge(eid) => format!("edge({})", eid.0),
        Value::Str(s) => format!("str({s})"),
        Value::Int(i) => format!("int({i})"),
        Value::Float(f) => format!("float({f})"),
        Value::Bool(b) => format!("bool({b})"),
        Value::Null => "null".to_string(),
        other => format!("{other:?}"),
    }
}

pub(super) fn append_gremlin_debug_log(line: &str) {
    use std::io::Write;

    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/sd-gremlin-debug.log")
    {
        let _ = writeln!(file, "{line}");
    }
}

pub(super) fn eval_select_math_expr(expr: &str, current: &Value) -> Value {
    fn as_f64(v: &Value) -> Option<f64> {
        match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    let Some(current_num) = as_f64(current) else {
        return current.clone();
    };

    let compact: String = expr.chars().filter(|c| !c.is_whitespace()).collect();
    match compact.as_str() {
        "_+_" => Value::Float(current_num + current_num),
        "_-_" => Value::Float(0.0),
        "_*_" => Value::Float(current_num * current_num),
        "_/_" => Value::Float(if current_num != 0.0 { 1.0 } else { f64::NAN }),
        "_%_" => Value::Float(if current_num != 0.0 {
            current_num % current_num
        } else {
            f64::NAN
        }),
        _ => current.clone(),
    }
}

/// Evaluate a Gremlin predicate string (e.g., "P.between(26,30)") against a value.
pub(super) fn eval_gremlin_predicate(pred_str: &str, value: &Value) -> bool {
    fn parse_num(s: &str) -> Option<f64> {
        let s = s.trim();
        if let Ok(i) = s.parse::<i64>() {
            return Some(i as f64);
        }
        s.parse::<f64>().ok()
    }
    fn value_as_f64(v: &Value) -> Option<f64> {
        match v {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    let pred = pred_str.strip_prefix("P.").unwrap_or(pred_str);

    // Handle compound predicates: .or(P.xxx) and .and(P.xxx)
    // Split at top-level .or() / .and() boundaries.
    if let Some((left, right, is_or)) = split_compound_predicate(pred) {
        let left_str = if left.starts_with("P.") {
            left.to_string()
        } else {
            format!("P.{left}")
        };
        let right_str = if right.starts_with("P.") {
            right.to_string()
        } else {
            format!("P.{right}")
        };
        let left_result = eval_gremlin_predicate(&left_str, value);
        let right_result = eval_gremlin_predicate(&right_str, value);
        return if is_or {
            left_result || right_result
        } else {
            left_result && right_result
        };
    }

    if let Some(rest) = pred.strip_prefix("between(") {
        if let Some(args) = rest.strip_suffix(')') {
            let parts: Vec<&str> = args.splitn(2, ',').collect();
            if parts.len() == 2 {
                if let (Some(lo), Some(hi), Some(v)) = (
                    parse_num(parts[0]),
                    parse_num(parts[1]),
                    value_as_f64(value),
                ) {
                    return v >= lo && v < hi;
                }
            }
        }
        return false;
    }

    // Single-arg predicates: P.lt(30), P.gt(30), P.lte(30), P.gte(30), P.eq(x), P.neq(x)
    for (prefix, cmp_fn) in &[
        ("lt(", (|v: f64, t: f64| v < t) as fn(f64, f64) -> bool),
        ("lte(", |v, t| v <= t),
        ("gt(", |v, t| v > t),
        ("gte(", |v, t| v >= t),
        ("eq(", |v, t| (v - t).abs() < 1e-9),
        ("neq(", |v, t| (v - t).abs() >= 1e-9),
    ] {
        if let Some(rest) = pred.strip_prefix(prefix) {
            if let Some(arg_str) = rest.strip_suffix(')') {
                // Try numeric comparison first
                if let (Some(threshold), Some(v)) = (parse_num(arg_str), value_as_f64(value)) {
                    return cmp_fn(v, threshold);
                }
                // For eq/neq, also try string comparison
                if *prefix == "eq(" {
                    let arg = arg_str.trim().trim_matches('"');
                    return match value {
                        Value::Str(s) => s.as_ref() == arg,
                        _ => false,
                    };
                }
                if *prefix == "neq(" {
                    let arg = arg_str.trim().trim_matches('"');
                    return match value {
                        Value::Str(s) => s.as_ref() != arg,
                        _ => true,
                    };
                }
            }
            return false;
        }
    }

    // P.neq(y) with string comparison
    if let Some(rest) = pred.strip_prefix("neq(") {
        if let Some(arg_str) = rest.strip_suffix(')') {
            let arg = arg_str.trim().trim_matches('"');
            match value {
                Value::Str(s) => return s.as_ref() != arg,
                _ => return true,
            }
        }
    }

    false
}

pub(super) fn parse_gremlin_option_args(args: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut in_single = false;
    let mut in_double = false;
    for ch in args.chars() {
        match ch {
            '\'' if !in_double => {
                in_single = !in_single;
                current.push(ch);
            }
            '"' if !in_single => {
                in_double = !in_double;
                current.push(ch);
            }
            ',' if !in_single && !in_double => {
                parts.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current.trim().to_string());
    }
    parts
}

pub(super) fn trim_gremlin_option_string(s: &str) -> &str {
    s.trim().trim_matches(|c| c == '"' || c == '\'')
}

pub(super) fn parse_gremlin_option_value(raw: &str) -> Value {
    let trimmed = trim_gremlin_option_string(raw);
    if raw.trim().starts_with('"') || raw.trim().starts_with('\'') {
        return Value::Str(trimmed.into());
    }
    if let Ok(i) = trimmed.parse::<i64>() {
        return Value::Int(i);
    }
    if let Ok(f) = trimmed.parse::<f64>() {
        return Value::Float(f);
    }
    match trimmed {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ => Value::Str(trimmed.into()),
    }
}

/// Split a compound predicate like "between(26, 30).or(gt(34))" into
/// (left_pred, right_pred, is_or). Returns None if not compound.
pub(super) fn split_compound_predicate(pred: &str) -> Option<(&str, &str, bool)> {
    // Look for .or( or .and( at the top level (outside balanced parens).
    let mut depth = 0u32;
    let bytes = pred.as_bytes();
    let len = bytes.len();
    let mut i = 0;
    while i < len {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth = depth.saturating_sub(1),
            b'.' if depth == 0 && i + 4 <= len => {
                let suffix = &pred[i..];
                if suffix.starts_with(".or(") {
                    let left = &pred[..i];
                    let inner = &pred[i + 4..];
                    // Find matching closing paren for .or(
                    if let Some(close) = inner.strip_suffix(')') {
                        return Some((left, close, true));
                    }
                }
                if suffix.starts_with(".and(") {
                    let left = &pred[..i];
                    let inner = &pred[i + 5..];
                    if let Some(close) = inner.strip_suffix(')') {
                        return Some((left, close, false));
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Check whether an operator tree contains a `Distinct` node (shallow search).
/// Used by `exec_repeat` to decide whether to enable cross-iteration dedup.
pub(super) fn operator_contains_distinct(op: &Operator) -> bool {
    match op {
        Operator::Distinct { .. } => true,
        // Walk through wrappers that commonly sit between Repeat body root
        // and the actual Distinct.
        Operator::Project { child, .. }
        | Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Expand { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::Coalesce { child, .. }
        | Operator::GremlinSimplePath { child, .. }
        | Operator::GremlinCyclicPath { child, .. } => operator_contains_distinct(child),
        Operator::Apply { outer, inner } => {
            operator_contains_distinct(outer) || operator_contains_distinct(inner)
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => operator_contains_distinct(preserved) || operator_contains_distinct(optional_pattern),
        Operator::Union { left, right, .. } => {
            operator_contains_distinct(left) || operator_contains_distinct(right)
        }
        _ => false,
    }
}

/// Returns `true` if the operator tree contains a barrier step (Sort, Aggregate,
/// Distinct) that must see all input rows before producing output. When a repeat
/// body contains barrier steps, batched seed processing must be disabled because
/// each batch would run the barrier independently, producing incorrect results.
pub(super) fn operator_contains_barrier(op: &Operator) -> bool {
    match op {
        Operator::Sort { .. }
        | Operator::Aggregate { .. }
        | Operator::Distinct { .. }
        | Operator::GremlinBarrier { .. } => true,
        Operator::Project { child, .. }
        | Operator::Filter { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Expand { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::Coalesce { child, .. }
        | Operator::GremlinSimplePath { child, .. }
        | Operator::GremlinCyclicPath { child, .. }
        | Operator::GremlinGroup { child, .. }
        | Operator::GremlinAggregate { child, .. } => operator_contains_barrier(child),
        // gremlin_tail limits need all rows → treat as barrier.
        Operator::Limit { count, child } => {
            if matches!(count, Expr::FunctionCall { name, .. } if name.as_str() == "gremlin_tail") {
                true
            } else {
                operator_contains_barrier(child)
            }
        }
        Operator::Apply { outer, inner } => {
            operator_contains_barrier(outer) || operator_contains_barrier(inner)
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => operator_contains_barrier(preserved) || operator_contains_barrier(optional_pattern),
        Operator::Union { left, right, .. } => {
            operator_contains_barrier(left) || operator_contains_barrier(right)
        }
        Operator::Repeat { body, .. } => operator_contains_barrier(body),
        Operator::Choose {
            if_true, if_false, ..
        } => {
            operator_contains_barrier(if_true)
                || if_false
                    .as_ref()
                    .is_some_and(|f| operator_contains_barrier(f))
        }
        _ => false,
    }
}

/// Extract direction, rel_types, src_alias, dst_alias from a simple Expand
/// whose child is SingleRow.
pub(super) fn extract_simple_expand(
    op: &Operator,
) -> Option<(Direction, Vec<crate::ast::RelType>, Identifier, Identifier)> {
    match op {
        Operator::Expand {
            direction,
            rel_types,
            src_alias,
            dst_alias,
            child,
            ..
        } if matches!(child.as_ref(), Operator::SingleRow) => Some((
            *direction,
            rel_types.clone(),
            src_alias.clone(),
            dst_alias.clone(),
        )),
        Operator::Project { child, .. } => extract_simple_expand(child),
        _ => None,
    }
}

/// Returns true if the operator tree contains a GremlinSimplePath filter.
pub(super) fn body_contains_simple_path(op: &Operator) -> bool {
    match op {
        Operator::GremlinSimplePath { .. } => true,
        Operator::Expand { child, .. }
        | Operator::Project { child, .. }
        | Operator::Filter { child, .. }
        | Operator::GremlinCyclicPath { child, .. } => body_contains_simple_path(child),
        _ => false,
    }
}

/// Expand rows that carry `__gremlin_bulk` into individual rows.
///
/// The vertex-counting fast path collapses identical destination nodes into
/// a single row with a bulk count.  Downstream steps (tail, path, select,
/// group-by-count, etc.) expect individual traverser rows, so we expand
/// here at the fast-path boundary.
pub(super) fn expand_bulk_rows(rows: Vec<Row>) -> Vec<Row> {
    let mut out = Vec::with_capacity(rows.len());
    for mut row in rows {
        if let Some(Value::Int(bulk)) = row.remove("__gremlin_bulk") {
            let n = (bulk as usize).max(1);
            for _ in 0..n {
                out.push(row.clone());
            }
        } else {
            out.push(row);
        }
    }
    out
}
