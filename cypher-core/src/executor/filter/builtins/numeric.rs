//! Numeric / random builtins: rand, java_random_next_double, gremlin_sample_*,
//! abs, ceil, floor, round, sqrt, sign, log, log10, exp, sin, cos, tan, asin,
//! acos, atan, atan2, degrees, radians, haversin, pi, e.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::helpers::{require_arity, type_error};

struct JavaCompatibleRandom {
    seed: u64,
}

impl JavaCompatibleRandom {
    const MULTIPLIER: u64 = 0x5DEECE66D;
    const ADDEND: u64 = 0xB;
    const MASK: u64 = (1u64 << 48) - 1;

    fn new(seed: i64) -> Self {
        Self {
            seed: ((seed as u64) ^ Self::MULTIPLIER) & Self::MASK,
        }
    }

    fn next(&mut self, bits: u32) -> u32 {
        self.seed = (self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(Self::ADDEND))
            & Self::MASK;
        (self.seed >> (48 - bits)) as u32
    }

    fn next_int(&mut self, bound: usize) -> usize {
        debug_assert!(bound > 0);
        if bound.is_power_of_two() {
            return ((bound as u64 * self.next(31) as u64) >> 31) as usize;
        }
        loop {
            let bits = self.next(31) as usize;
            let val = bits % bound;
            if bits.wrapping_sub(val).wrapping_add(bound - 1) <= i32::MAX as usize {
                return val;
            }
        }
    }
}

pub(in crate::executor::filter) fn builtin_rand(vals: Vec<Value>) -> Result<Value, GqlError> {
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
pub(in crate::executor::filter) fn builtin_java_random_next_double(
    vals: Vec<Value>,
) -> Result<Value, GqlError> {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicI64, Ordering};

    // Thread-local state: (initial_seed_used, current_lcg_state)
    thread_local! {
        static JAVA_RNG_STATE: AtomicI64 = const { AtomicI64::new(i64::MIN) };
        static JAVA_RNG_INIT: Cell<i64> = const { Cell::new(i64::MIN) };
    }

    const MULTIPLIER: i64 = 0x5DEECE66D;
    const ADDEND: i64 = 0xB;
    const MASK: i64 = (1i64 << 48) - 1;

    let initial_seed = match vals.first() {
        Some(Value::Int(s)) => *s,
        _ => 0,
    };

    JAVA_RNG_STATE.with(|state| {
        JAVA_RNG_INIT.with(|init| {
            let prev_init = init.get();
            let seed = if prev_init != initial_seed {
                // New traversal with different seed: reset state
                let s = (initial_seed ^ MULTIPLIER) & MASK;
                init.set(initial_seed);
                s
            } else {
                let current = state.load(Ordering::Relaxed);
                if current == i64::MIN {
                    let s = (initial_seed ^ MULTIPLIER) & MASK;
                    init.set(initial_seed);
                    s
                } else {
                    current
                }
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
    })
}

fn sample_next_double(seed: Option<i64>) -> Result<f64, GqlError> {
    let value = match seed {
        Some(seed) => builtin_java_random_next_double(vec![Value::Int(seed)])?,
        None => builtin_rand(Vec::new())?,
    };
    match value {
        Value::Float(f) => Ok(f),
        other => type_error("sample_random", &other).and(Ok(0.0)),
    }
}

pub(in crate::executor::filter) fn builtin_gremlin_sample_weighted_key(
    vals: Vec<Value>,
) -> Result<Value, GqlError> {
    require_arity("gremlin_sample_weighted_key", 2, &vals)?;
    let seed = match &vals[0] {
        Value::Int(seed) => Some(*seed),
        Value::Null => None,
        other => return type_error("gremlin_sample_weighted_key", other),
    };
    let weight = match &vals[1] {
        Value::Int(weight) if *weight > 0 => *weight as f64,
        Value::Float(weight) if *weight > 0.0 => *weight,
        Value::Null => return Ok(Value::Null),
        Value::Int(_) | Value::Float(_) => return Ok(Value::Null),
        other => return type_error("gremlin_sample_weighted_key", other),
    };
    let u = sample_next_double(seed)?.clamp(f64::MIN_POSITIVE, 1.0 - f64::EPSILON);
    Ok(Value::Float(u.ln() / weight))
}

pub(in crate::executor::filter) fn builtin_gremlin_sample_local(
    vals: Vec<Value>,
) -> Result<Value, GqlError> {
    require_arity("gremlin_sample_local", 3, &vals)?;
    let count = match &vals[1] {
        Value::Int(count) => *count,
        other => {
            return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                expected: "gremlin_sample_local() count".into(),
                actual: other.type_name().to_string(),
            }))
        }
    };
    let seed = match &vals[2] {
        Value::Int(seed) => Some(*seed),
        Value::Null => None,
        other => return type_error("gremlin_sample_local", other),
    };

    if count <= 0 {
        return Ok(match &vals[0] {
            Value::List(_) => Value::List(Vec::new()),
            Value::Map(_) => Value::Map(IndexMap::new()),
            _ => Value::Null,
        });
    }

    match &vals[0] {
        Value::List(items) => {
            if items.len() <= count as usize {
                return Ok(Value::List(items.clone()));
            }
            let mut original = items.clone();
            let mut target = Vec::with_capacity(count as usize);
            if let Some(seed) = seed {
                let mut random = JavaCompatibleRandom::new(seed);
                while target.len() < count as usize {
                    target.push(original.remove(random.next_int(original.len())));
                }
            } else {
                while target.len() < count as usize {
                    let idx = (sample_next_double(None)? * original.len() as f64).floor() as usize;
                    target.push(original.remove(idx.min(original.len() - 1)));
                }
            }
            Ok(Value::List(target))
        }
        Value::Map(map) => {
            if map.len() <= count as usize {
                return Ok(Value::Map(map.clone()));
            }
            let mut original: Vec<(Box<str>, Value)> = map
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect();
            let mut out = IndexMap::new();
            if let Some(seed) = seed {
                let mut random = JavaCompatibleRandom::new(seed);
                while out.len() < count as usize {
                    let (key, value) = original.remove(random.next_int(original.len()));
                    out.insert(key, value);
                }
            } else {
                while out.len() < count as usize {
                    let idx = (sample_next_double(None)? * original.len() as f64).floor() as usize;
                    let (key, value) = original.remove(idx.min(original.len() - 1));
                    out.insert(key, value);
                }
            }
            Ok(Value::Map(out))
        }
        other => Ok(other.clone()),
    }
}

pub(in crate::executor::filter) fn builtin_abs(vals: Vec<Value>) -> Result<Value, GqlError> {
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

pub(in crate::executor::filter) fn builtin_ceil(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("ceil", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Float(f.ceil())),
        other => type_error("ceil", other),
    }
}

pub(in crate::executor::filter) fn builtin_floor(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("floor", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Float(f.floor())),
        other => type_error("floor", other),
    }
}

pub(in crate::executor::filter) fn builtin_round(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("round", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Float(f.round())),
        other => type_error("round", other),
    }
}

pub(in crate::executor::filter) fn builtin_sqrt(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("sqrt", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).sqrt())),
        Value::Float(f) => Ok(Value::Float(f.sqrt())),
        other => type_error("sqrt", other),
    }
}

pub(in crate::executor::filter) fn builtin_sign(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("sign", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Int(n.signum())),
        Value::Float(f) => Ok(Value::Float(f.signum())),
        other => type_error("sign", other),
    }
}

pub(in crate::executor::filter) fn builtin_log(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("log", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).ln())),
        Value::Float(f) => Ok(Value::Float(f.ln())),
        other => type_error("log", other),
    }
}

pub(in crate::executor::filter) fn builtin_log10(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("log10", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).log10())),
        Value::Float(f) => Ok(Value::Float(f.log10())),
        other => type_error("log10", other),
    }
}

pub(in crate::executor::filter) fn builtin_exp(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("exp", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).exp())),
        Value::Float(f) => Ok(Value::Float(f.exp())),
        other => type_error("exp", other),
    }
}

pub(in crate::executor::filter) fn builtin_sin(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("sin", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).sin())),
        Value::Float(f) => Ok(Value::Float(f.sin())),
        other => type_error("sin", other),
    }
}

pub(in crate::executor::filter) fn builtin_cos(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("cos", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).cos())),
        Value::Float(f) => Ok(Value::Float(f.cos())),
        other => type_error("cos", other),
    }
}

pub(in crate::executor::filter) fn builtin_tan(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("tan", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).tan())),
        Value::Float(f) => Ok(Value::Float(f.tan())),
        other => type_error("tan", other),
    }
}

pub(in crate::executor::filter) fn builtin_asin(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("asin", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).asin())),
        Value::Float(f) => Ok(Value::Float(f.asin())),
        other => type_error("asin", other),
    }
}

pub(in crate::executor::filter) fn builtin_acos(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("acos", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).acos())),
        Value::Float(f) => Ok(Value::Float(f.acos())),
        other => type_error("acos", other),
    }
}

pub(in crate::executor::filter) fn builtin_atan(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("atan", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).atan())),
        Value::Float(f) => Ok(Value::Float(f.atan())),
        other => type_error("atan", other),
    }
}

pub(in crate::executor::filter) fn builtin_atan2(vals: Vec<Value>) -> Result<Value, GqlError> {
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

pub(in crate::executor::filter) fn builtin_degrees(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("degrees", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).to_degrees())),
        Value::Float(f) => Ok(Value::Float(f.to_degrees())),
        other => type_error("degrees", other),
    }
}

pub(in crate::executor::filter) fn builtin_radians(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("radians", 1, &vals)?;
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Int(n) => Ok(Value::Float((*n as f64).to_radians())),
        Value::Float(f) => Ok(Value::Float(f.to_radians())),
        other => type_error("radians", other),
    }
}

pub(in crate::executor::filter) fn builtin_haversin(vals: Vec<Value>) -> Result<Value, GqlError> {
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

pub(in crate::executor::filter) fn builtin_pi(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("pi", 0, &vals)?;
    Ok(Value::Float(std::f64::consts::PI))
}

pub(in crate::executor::filter) fn builtin_e(vals: Vec<Value>) -> Result<Value, GqlError> {
    require_arity("e", 0, &vals)?;
    Ok(Value::Float(std::f64::consts::E))
}
