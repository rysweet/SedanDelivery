//! Builtin function implementations: math, string, type conversion, collection functions.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::max_list_size;

// ── Small helpers ─────────────────────────────────────────────────────────────

pub(super) fn require_arity(name: &str, expected: usize, vals: &[Value]) -> Result<(), GqlError> {
    if vals.len() != expected {
        Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: format!("{name}() expects {expected} argument(s)"),
            actual: format!("{} argument(s)", vals.len()),
        }))
    } else {
        Ok(())
    }
}

pub(super) fn type_error(name: &str, val: &Value) -> Result<Value, GqlError> {
    Err(GqlError::Execution(ExecutionError::TypeMismatch {
        expected: format!("{name}() argument"),
        actual: val.type_name().to_string(),
    }))
}

// ── Math functions ────────────────────────────────────────────────────────────

pub(super) fn builtin_rand(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("rand", 0, &vals)?;
    // Use a simple hash-based pseudo-random to avoid adding a crate dependency.
    // The TCK only checks that the result is a float in [0.0, 1.0).
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    let bits = hasher.finish();
    // Convert to f64 in [0.0, 1.0)
    let val = (bits >> 11) as f64 / ((1u64 << 53) as f64);
    Ok(Value::Float(val))
}

/// Java-compatible Linear Congruential Generator for deterministic random.
/// Matches `java.util.Random` behaviour so Gremlin SeedStrategy tests pass.
pub(super) fn builtin_java_random_next_double(vals: Vec<Value>) -> Result<Value, GqlError> {
    use std::sync::atomic::{AtomicI64, Ordering};

    // Thread-local state for the Java LCG
    thread_local! {
        static JAVA_RNG_SEED: AtomicI64 = const { AtomicI64::new(i64::MIN) };
    }

    const MULTIPLIER: i64 = 0x5DEECE66D;
    const ADDEND: i64 = 0xB;
    const MASK: i64 = (1i64 << 48) - 1;

    let initial_seed = match vals.first() {
        Some(Value::Int(s)) => *s,
        _ => 0,
    };

    JAVA_RNG_SEED.with(|state| {
        let current = state.load(Ordering::Relaxed);
        let seed = if current == i64::MIN {
            // First call: initialize with (initial_seed ^ MULTIPLIER) & MASK
            (initial_seed ^ MULTIPLIER) & MASK
        } else {
            current
        };

        // next(26)
        let seed1 = (seed.wrapping_mul(MULTIPLIER).wrapping_add(ADDEND)) & MASK;
        let bits26 = seed1 >> (48 - 26);

        // next(27)
        let seed2 = (seed1.wrapping_mul(MULTIPLIER).wrapping_add(ADDEND)) & MASK;
        let bits27 = seed2 >> (48 - 27);

        state.store(seed2, Ordering::Relaxed);

        let val = ((bits26 << 27) + bits27) as f64 / ((1i64 << 53) as f64);
        Ok(Value::Float(val))
    })
}

pub(super) fn builtin_abs(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("abs", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(n.checked_abs().ok_or_else(|| {
            GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "integer arithmetic without overflow".to_string(),
                actual: "integer overflow".to_string(),
            })
        })?)),
        Value::Float(f) => Ok(Value::Float(f.abs())),
        other => type_error("abs", other),
    }
}

pub(super) fn builtin_ceil(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("ceil", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Float(f.ceil())),
        other => type_error("ceil", other),
    }
}

pub(super) fn builtin_floor(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("floor", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Float(f.floor())),
        other => type_error("floor", other),
    }
}

pub(super) fn builtin_round(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("round", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Float(f.round())),
        other => type_error("round", other),
    }
}

pub(super) fn builtin_sqrt(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("sqrt", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).sqrt())),
        Value::Float(f) => Ok(Value::Float(f.sqrt())),
        other => type_error("sqrt", other),
    }
}

pub(super) fn builtin_sign(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("sign", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(n.signum())),
        Value::Float(f) => Ok(Value::Float(f.signum())),
        other => type_error("sign", other),
    }
}

pub(super) fn builtin_log(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("log", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).ln())),
        Value::Float(f) => Ok(Value::Float(f.ln())),
        other => type_error("log", other),
    }
}

pub(super) fn builtin_log10(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("log10", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).log10())),
        Value::Float(f) => Ok(Value::Float(f.log10())),
        other => type_error("log10", other),
    }
}

pub(super) fn builtin_exp(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("exp", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).exp())),
        Value::Float(f) => Ok(Value::Float(f.exp())),
        other => type_error("exp", other),
    }
}

pub(super) fn builtin_sin(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("sin", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).sin())),
        Value::Float(f) => Ok(Value::Float(f.sin())),
        other => type_error("sin", other),
    }
}

pub(super) fn builtin_cos(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("cos", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).cos())),
        Value::Float(f) => Ok(Value::Float(f.cos())),
        other => type_error("cos", other),
    }
}

pub(super) fn builtin_tan(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("tan", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).tan())),
        Value::Float(f) => Ok(Value::Float(f.tan())),
        other => type_error("tan", other),
    }
}

pub(super) fn builtin_asin(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("asin", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).asin())),
        Value::Float(f) => Ok(Value::Float(f.asin())),
        other => type_error("asin", other),
    }
}

pub(super) fn builtin_acos(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("acos", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).acos())),
        Value::Float(f) => Ok(Value::Float(f.acos())),
        other => type_error("acos", other),
    }
}

pub(super) fn builtin_atan(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("atan", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).atan())),
        Value::Float(f) => Ok(Value::Float(f.atan())),
        other => type_error("atan", other),
    }
}

pub(super) fn builtin_atan2(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("atan2", 2, &vals)?;
    match (&vals[0], &vals[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Int(y), Value::Int(x)) => Ok(Value::Float((*y as f64).atan2(*x as f64))),
        (Value::Float(y), Value::Float(x)) => Ok(Value::Float(y.atan2(*x))),
        (Value::Int(y), Value::Float(x)) => Ok(Value::Float((*y as f64).atan2(*x))),
        (Value::Float(y), Value::Int(x)) => Ok(Value::Float(y.atan2(*x as f64))),
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Numeric, Numeric".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}

pub(super) fn builtin_degrees(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("degrees", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).to_degrees())),
        Value::Float(f) => Ok(Value::Float(f.to_degrees())),
        other => type_error("degrees", other),
    }
}

pub(super) fn builtin_radians(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("radians", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).to_radians())),
        Value::Float(f) => Ok(Value::Float(f.to_radians())),
        other => type_error("radians", other),
    }
}

pub(super) fn builtin_haversin(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("haversin", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => {
            let x = *n as f64;
            Ok(Value::Float((1.0 - x.cos()) / 2.0))
        }
        Value::Float(f) => Ok(Value::Float((1.0 - f.cos()) / 2.0)),
        other => type_error("haversin", other),
    }
}

pub(super) fn builtin_pi(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("pi", 0, &vals)?;
    Ok(Value::Float(std::f64::consts::PI))
}

pub(super) fn builtin_e(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("e", 0, &vals)?;
    Ok(Value::Float(std::f64::consts::E))
}

// ── Type conversion functions ─────────────────────────────────────────────────

pub(super) fn builtin_tostring(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toString", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        // A String is already a string — return it as-is (no extra quotes).
        Value::Str(s) => Ok(Value::Str(s.clone())),
        Value::Int(_) | Value::Float(_) | Value::Bool(_) => {
            Ok(Value::Str(vals[0].to_string().into_boxed_str()))
        }
        Value::Date(_)
        | Value::LocalTime(_)
        | Value::Time(_, _)
        | Value::LocalDateTime(_)
        | Value::DateTime(_)
        | Value::Duration(_) => Ok(Value::Str(vals[0].to_string().into_boxed_str())),
        other => type_error("toString", other),
    }
}

pub(super) fn builtin_tointeger(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toInteger", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => {
            if f.is_finite() {
                Ok(Value::Int(*f as i64))
            } else {
                Ok(Value::Null)
            }
        }
        Value::Str(s) => {
            // Try integer parse first, then float parse (truncating).
            // TCK expects NULL for truly non-numeric strings.
            if let Ok(n) = s.parse::<i64>() {
                Ok(Value::Int(n))
            } else if let Ok(f) = s.parse::<f64>() {
                if f.is_finite() {
                    Ok(Value::Int(f as i64))
                } else {
                    Ok(Value::Null)
                }
            } else {
                Ok(Value::Null)
            }
        }
        other => type_error("toInteger", other),
    }
}

pub(super) fn builtin_tofloat(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toFloat", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Float(f) => Ok(Value::Float(*f)),
        Value::Int(n) => Ok(Value::Float(*n as f64)),
        Value::Str(s) => {
            // TCK expects NULL for non-numeric strings, not an error.
            Ok(s.parse::<f64>().map(Value::Float).unwrap_or(Value::Null))
        }
        other => type_error("toFloat", other),
    }
}

pub(super) fn builtin_toboolean(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toBoolean", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Bool(b) => Ok(Value::Bool(*b)),
        Value::Str(s) => match s.to_ascii_lowercase().as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            // Per Cypher spec: toBoolean() on an unrecognized string returns null,
            // not an error. This is intentional Cypher-compliant behavior.
            _ => Ok(Value::Null),
        },
        other => type_error("toBoolean", other),
    }
}

// ── String functions ──────────────────────────────────────────────────────────

pub(super) fn builtin_size(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("size", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
        Value::List(l) => Ok(Value::Int(l.len() as i64)),
        other => type_error("size", other),
    }
}

pub(super) fn builtin_length(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("length", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Int(s.chars().count() as i64)),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_length(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("length", other),
    }
}

pub(super) fn builtin_toupper(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toUpper", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Str(s.to_uppercase().into_boxed_str())),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_toupper(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("toUpper", other),
    }
}

pub(super) fn builtin_tolower(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("toLower", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Str(s.to_lowercase().into_boxed_str())),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_tolower(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("toLower", other),
    }
}

pub(super) fn builtin_trim(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("trim", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Str(s.trim().to_string().into_boxed_str())),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_trim(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("trim", other),
    }
}

pub(super) fn builtin_ltrim(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("ltrim", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Str(s.trim_start().to_string().into_boxed_str())),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_ltrim(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("ltrim", other),
    }
}

pub(super) fn builtin_rtrim(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("rtrim", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => Ok(Value::Str(s.trim_end().to_string().into_boxed_str())),
        Value::List(items) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_rtrim(vec![v.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        other => type_error("rtrim", other),
    }
}

pub(super) fn builtin_left(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("left", 2, &vals)?;
    match (&vals[0], &vals[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Int(n)) => {
            let n = (*n).max(0) as usize;
            Ok(Value::Str(
                s.chars().take(n).collect::<String>().into_boxed_str(),
            ))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String, Integer".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}

pub(super) fn builtin_right(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("right", 2, &vals)?;
    match (&vals[0], &vals[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Int(n)) => {
            let n = (*n).max(0) as usize;
            let chars: Vec<char> = s.chars().collect();
            let start = chars.len().saturating_sub(n);
            Ok(Value::Str(
                chars[start..].iter().collect::<String>().into_boxed_str(),
            ))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String, Integer".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}

pub(super) fn builtin_substring(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "substring(String, Integer[, Integer])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match (&vals[0], &vals[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Int(start)) => {
            let chars: Vec<char> = s.chars().collect();
            let len = chars.len();
            let start = (*start).max(0) as usize;
            let end = if vals.len() == 3 {
                match &vals[2] {
                    Value::Int(n) => (start + (*n).max(0) as usize).min(len),
                    Value::Null => len,
                    other => return type_error("substring", other),
                }
            } else {
                len
            };
            Ok(Value::Str(
                chars[start.min(len)..end.min(len)]
                    .iter()
                    .collect::<String>()
                    .into_boxed_str(),
            ))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String, Integer".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}

pub(super) fn builtin_replace(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("replace", 3, &vals)?;
    match (&vals[0], &vals[1], &vals[2]) {
        (Value::Null, _, _) | (_, Value::Null, _) | (_, _, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Str(from), Value::Str(to)) => Ok(Value::Str(
            s.replace(from.as_ref(), to.as_ref()).into_boxed_str(),
        )),
        (Value::List(items), from, to) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_replace(vec![v.clone(), from.clone(), to.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String, String, String".to_string(),
            actual: format!(
                "{}, {}, {}",
                vals[0].type_name(),
                vals[1].type_name(),
                vals[2].type_name()
            ),
        })),
    }
}

pub(super) fn builtin_split(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("split", 2, &vals)?;
    match (&vals[0], &vals[1]) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Str(delim)) => {
            let parts: Vec<Value> = s
                .split(delim.as_ref())
                .take(max_list_size())
                .map(|p| Value::Str(p.to_string().into_boxed_str()))
                .collect();
            Ok(Value::List(parts))
        }
        (Value::List(items), delim) => {
            let results: Result<Vec<Value>, _> = items
                .iter()
                .map(|v| builtin_split(vec![v.clone(), delim.clone()]))
                .collect();
            Ok(Value::List(results?))
        }
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "String, String".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}

// ── List / collection functions ───────────────────────────────────────────────

pub(super) fn builtin_head(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("head", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::List(l) => Ok(l.first().cloned().unwrap_or(Value::Null)),
        other => type_error("head", other),
    }
}

pub(super) fn builtin_last(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("last", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::List(l) => Ok(l.last().cloned().unwrap_or(Value::Null)),
        other => type_error("last", other),
    }
}

pub(super) fn builtin_tail(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("tail", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::List(l) => Ok(Value::List(l[1.min(l.len())..].to_vec())),
        other => type_error("tail", other),
    }
}

pub(super) fn builtin_reverse(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("reverse", 1, &vals)?;
    // safe: require_arity(1) above guarantees at least one element
    match vals.into_iter().next().expect("arity checked") {
        Value::Null => Ok(Value::Null),
        Value::List(mut l) => {
            l.reverse();
            Ok(Value::List(l))
        }
        Value::Str(s) => Ok(Value::Str(
            s.chars().rev().collect::<String>().into_boxed_str(),
        )),
        other => type_error("reverse", &other),
    }
}

pub(super) fn builtin_coalesce(vals: Vec<Value>) -> Result<Value, GqlError> {
    // Return first non-null argument.
    for v in vals {
        if !v.is_null() {
            return Ok(v);
        }
    }
    Ok(Value::Null)
}

pub(super) fn builtin_isnull(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("isNull", 1, &vals)?;
    Ok(Value::Bool(vals[0].is_null()))
}

pub(super) fn builtin_keys(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("keys", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Map(m) => Ok(Value::List(
            m.iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, _)| Value::Str(k.clone()))
                .collect(),
        )),
        other => type_error("keys", other),
    }
}

pub(super) fn builtin_values(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("values", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Map(m) => Ok(Value::List(m.values().cloned().collect())),
        other => type_error("values", other),
    }
}

pub(super) fn builtin_properties(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("properties", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Map(m) => Ok(Value::Map(m.clone())),
        other => type_error("properties", other),
    }
}

pub(super) fn builtin_range(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.len() < 2 || vals.len() > 3 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "range(Integer, Integer[, Integer])".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match (&vals[0], &vals[1]) {
        (Value::Int(start), Value::Int(end)) => {
            let step = if vals.len() == 3 {
                match &vals[2] {
                    Value::Int(s) => *s,
                    other => return type_error("range", other),
                }
            } else {
                1
            };
            if step == 0 {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: "non-zero step for range()".to_string(),
                    actual: "0".to_string(),
                }));
            }
            let list_limit = max_list_size();
            let mut result = Vec::new();
            let mut i = *start;
            if step > 0 {
                while i <= *end {
                    if result.len() >= list_limit {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("range() result <= {list_limit} elements"),
                            actual: format!("would exceed {list_limit} elements"),
                        }));
                    }
                    result.push(Value::Int(i));
                    i = i.checked_add(step).ok_or_else(|| {
                        GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "range() step addition within i64 bounds".to_string(),
                            actual: "integer overflow".to_string(),
                        })
                    })?;
                }
            } else {
                while i >= *end {
                    if result.len() >= list_limit {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: format!("range() result <= {list_limit} elements"),
                            actual: format!("would exceed {list_limit} elements"),
                        }));
                    }
                    result.push(Value::Int(i));
                    i = i.checked_add(step).ok_or_else(|| {
                        GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "range() step addition within i64 bounds".to_string(),
                            actual: "integer overflow".to_string(),
                        })
                    })?;
                }
            }
            Ok(Value::List(result))
        }
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        _ => Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "Integer, Integer".to_string(),
            actual: format!("{}, {}", vals[0].type_name(), vals[1].type_name()),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tointeger_nan_returns_null() {
        let result = builtin_tointeger(vec![Value::Float(f64::NAN)]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn tointeger_positive_infinity_returns_null() {
        let result = builtin_tointeger(vec![Value::Float(f64::INFINITY)]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn tointeger_negative_infinity_returns_null() {
        let result = builtin_tointeger(vec![Value::Float(f64::NEG_INFINITY)]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn tointeger_finite_float_returns_int() {
        let result = builtin_tointeger(vec![Value::Float(3.7)]).unwrap();
        assert_eq!(result, Value::Int(3));
    }

    #[test]
    fn tointeger_string_nan_returns_null() {
        let result = builtin_tointeger(vec![Value::Str("NaN".into())]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn tointeger_string_infinity_returns_null() {
        let result = builtin_tointeger(vec![Value::Str("Infinity".into())]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn tointeger_string_neg_infinity_returns_null() {
        let result = builtin_tointeger(vec![Value::Str("-Infinity".into())]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn tointeger_null_returns_null() {
        let result = builtin_tointeger(vec![Value::Null]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn tointeger_int_returns_same() {
        let result = builtin_tointeger(vec![Value::Int(42)]).unwrap();
        assert_eq!(result, Value::Int(42));
    }

    // ── Math function tests ─────────────────────────────────────────────────

    #[test]
    fn log_of_1_returns_0() {
        let result = builtin_log(vec![Value::Int(1)]).unwrap();
        assert_eq!(result, Value::Float(0.0));
    }

    #[test]
    fn log_of_e_returns_1() {
        let result = builtin_log(vec![Value::Float(std::f64::consts::E)]).unwrap();
        match result {
            Value::Float(f) => assert!((f - 1.0).abs() < 1e-10),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn log_null_returns_null() {
        let result = builtin_log(vec![Value::Null]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn log10_of_100_returns_2() {
        let result = builtin_log10(vec![Value::Int(100)]).unwrap();
        match result {
            Value::Float(f) => assert!((f - 2.0).abs() < 1e-10),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn exp_of_0_returns_1() {
        let result = builtin_exp(vec![Value::Int(0)]).unwrap();
        assert_eq!(result, Value::Float(1.0));
    }

    #[test]
    fn exp_of_1_returns_e() {
        let result = builtin_exp(vec![Value::Int(1)]).unwrap();
        match result {
            Value::Float(f) => assert!((f - std::f64::consts::E).abs() < 1e-10),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn sin_of_0_returns_0() {
        let result = builtin_sin(vec![Value::Int(0)]).unwrap();
        assert_eq!(result, Value::Float(0.0));
    }

    #[test]
    fn cos_of_0_returns_1() {
        let result = builtin_cos(vec![Value::Int(0)]).unwrap();
        assert_eq!(result, Value::Float(1.0));
    }

    #[test]
    fn tan_of_0_returns_0() {
        let result = builtin_tan(vec![Value::Int(0)]).unwrap();
        assert_eq!(result, Value::Float(0.0));
    }

    #[test]
    fn asin_of_0_returns_0() {
        let result = builtin_asin(vec![Value::Int(0)]).unwrap();
        assert_eq!(result, Value::Float(0.0));
    }

    #[test]
    fn acos_of_1_returns_0() {
        let result = builtin_acos(vec![Value::Int(1)]).unwrap();
        assert_eq!(result, Value::Float(0.0));
    }

    #[test]
    fn atan_of_0_returns_0() {
        let result = builtin_atan(vec![Value::Int(0)]).unwrap();
        assert_eq!(result, Value::Float(0.0));
    }

    #[test]
    fn atan2_of_0_1_returns_0() {
        let result = builtin_atan2(vec![Value::Int(0), Value::Int(1)]).unwrap();
        assert_eq!(result, Value::Float(0.0));
    }

    #[test]
    fn atan2_of_1_0_returns_pi_over_2() {
        let result = builtin_atan2(vec![Value::Float(1.0), Value::Float(0.0)]).unwrap();
        match result {
            Value::Float(f) => assert!((f - std::f64::consts::FRAC_PI_2).abs() < 1e-10),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn atan2_mixed_types() {
        let result = builtin_atan2(vec![Value::Int(1), Value::Float(0.0)]).unwrap();
        match result {
            Value::Float(f) => assert!((f - std::f64::consts::FRAC_PI_2).abs() < 1e-10),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn atan2_null_returns_null() {
        let result = builtin_atan2(vec![Value::Null, Value::Int(1)]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn pi_returns_pi() {
        let result = builtin_pi(vec![]).unwrap();
        assert_eq!(result, Value::Float(std::f64::consts::PI));
    }

    #[test]
    fn e_returns_e() {
        let result = builtin_e(vec![]).unwrap();
        assert_eq!(result, Value::Float(std::f64::consts::E));
    }

    #[test]
    fn degrees_of_pi_returns_180() {
        let result = builtin_degrees(vec![Value::Float(std::f64::consts::PI)]).unwrap();
        match result {
            Value::Float(f) => assert!((f - 180.0).abs() < 1e-10),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn radians_of_180_returns_pi() {
        let result = builtin_radians(vec![Value::Int(180)]).unwrap();
        match result {
            Value::Float(f) => assert!((f - std::f64::consts::PI).abs() < 1e-10),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn haversin_of_0_returns_0() {
        let result = builtin_haversin(vec![Value::Int(0)]).unwrap();
        assert_eq!(result, Value::Float(0.0));
    }

    #[test]
    fn haversin_of_pi_returns_1() {
        let result = builtin_haversin(vec![Value::Float(std::f64::consts::PI)]).unwrap();
        match result {
            Value::Float(f) => assert!((f - 1.0).abs() < 1e-10),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn haversin_null_returns_null() {
        let result = builtin_haversin(vec![Value::Null]).unwrap();
        assert_eq!(result, Value::Null);
    }

    #[test]
    fn math_functions_reject_string() {
        assert!(builtin_log(vec![Value::Str("x".into())]).is_err());
        assert!(builtin_exp(vec![Value::Str("x".into())]).is_err());
        assert!(builtin_sin(vec![Value::Str("x".into())]).is_err());
        assert!(builtin_degrees(vec![Value::Str("x".into())]).is_err());
        assert!(builtin_radians(vec![Value::Str("x".into())]).is_err());
        assert!(builtin_haversin(vec![Value::Str("x".into())]).is_err());
    }

    #[test]
    fn pi_rejects_args() {
        assert!(builtin_pi(vec![Value::Int(1)]).is_err());
    }

    #[test]
    fn e_rejects_args() {
        assert!(builtin_e(vec![Value::Int(1)]).is_err());
    }
}
