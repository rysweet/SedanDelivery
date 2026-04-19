//! Random and sampling builtins: `rand`, Java-compatible RNG,
//! and Gremlin sample/sampleByWeighted helpers.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::helpers::{require_arity, type_error};

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
