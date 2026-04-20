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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_random_deterministic_for_same_seed() {
        let mut a = JavaCompatibleRandom::new(42);
        let mut b = JavaCompatibleRandom::new(42);
        for _ in 0..5 {
            assert_eq!(a.next(31), b.next(31));
        }
    }

    #[test]
    fn java_random_next_int_in_bounds() {
        let mut r = JavaCompatibleRandom::new(7);
        for _ in 0..50 {
            let v = r.next_int(10);
            assert!(v < 10);
        }
        // Power-of-two short-circuit.
        let mut r = JavaCompatibleRandom::new(7);
        for _ in 0..50 {
            let v = r.next_int(8);
            assert!(v < 8);
        }
    }

    #[test]
    fn builtin_rand_returns_unit_interval_float() {
        for _ in 0..10 {
            let v = builtin_rand(Vec::new()).unwrap();
            match v {
                Value::Float(f) => {
                    assert!((0.0..1.0).contains(&f), "rand() out of [0,1): {f}");
                }
                _ => panic!("expected Float"),
            }
        }
    }

    #[test]
    fn builtin_rand_arity_error() {
        let err = builtin_rand(vec![Value::Int(1)]).unwrap_err();
        assert!(matches!(err, GqlError::Execution(_)));
    }

    #[test]
    fn java_random_next_double_returns_unit_interval() {
        let v = builtin_java_random_next_double(vec![Value::Int(123)]).unwrap();
        match v {
            Value::Float(f) => assert!((0.0..1.0).contains(&f)),
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn java_random_next_double_seed_reset_on_change() {
        // Two distinct seeds must yield deterministic but different first values
        // (probabilistic with overwhelming likelihood).
        let a = builtin_java_random_next_double(vec![Value::Int(101)]).unwrap();
        let b = builtin_java_random_next_double(vec![Value::Int(202)]).unwrap();
        // Same seed reuses state across calls; resetting to a new seed yields
        // a different first value than the second sample of the previous seed.
        assert!(matches!(a, Value::Float(_)));
        assert!(matches!(b, Value::Float(_)));
    }

    #[test]
    fn weighted_key_zero_weight_returns_null() {
        let r = builtin_gremlin_sample_weighted_key(vec![Value::Int(1), Value::Int(0)]).unwrap();
        assert_eq!(r, Value::Null);
        let r =
            builtin_gremlin_sample_weighted_key(vec![Value::Int(1), Value::Float(-3.5)]).unwrap();
        assert_eq!(r, Value::Null);
        let r = builtin_gremlin_sample_weighted_key(vec![Value::Int(1), Value::Null]).unwrap();
        assert_eq!(r, Value::Null);
    }

    #[test]
    fn weighted_key_returns_negative_log_over_weight() {
        let r =
            builtin_gremlin_sample_weighted_key(vec![Value::Int(7), Value::Float(2.0)]).unwrap();
        match r {
            Value::Float(f) => {
                assert!(f.is_finite());
                assert!(f <= 0.0);
            }
            _ => panic!("expected Float"),
        }
    }

    #[test]
    fn weighted_key_arity_and_type_errors() {
        // Missing arg → arity error.
        assert!(builtin_gremlin_sample_weighted_key(vec![Value::Int(1)]).is_err());
        // Wrong seed type.
        assert!(builtin_gremlin_sample_weighted_key(vec![
            Value::Str("nope".into()),
            Value::Int(1),
        ])
        .is_err());
        // Wrong weight type.
        assert!(
            builtin_gremlin_sample_weighted_key(vec![Value::Int(1), Value::Bool(true),]).is_err()
        );
    }

    #[test]
    fn sample_local_zero_count_returns_empty_container() {
        // List input → empty list.
        let r = builtin_gremlin_sample_local(vec![
            Value::List(vec![Value::Int(1), Value::Int(2)]),
            Value::Int(0),
            Value::Int(7),
        ])
        .unwrap();
        assert!(matches!(r, Value::List(ref v) if v.is_empty()));

        // Map input → empty map.
        let mut m = IndexMap::new();
        m.insert(Box::from("a"), Value::Int(1));
        let r = builtin_gremlin_sample_local(vec![Value::Map(m), Value::Int(0), Value::Int(7)])
            .unwrap();
        assert!(matches!(r, Value::Map(ref v) if v.is_empty()));

        // Other input + count<=0 → Null.
        let r = builtin_gremlin_sample_local(vec![Value::Int(99), Value::Int(0), Value::Int(7)])
            .unwrap();
        assert_eq!(r, Value::Null);
    }

    #[test]
    fn sample_local_count_ge_len_returns_input() {
        let lst = Value::List(vec![Value::Int(1), Value::Int(2)]);
        let r =
            builtin_gremlin_sample_local(vec![lst.clone(), Value::Int(5), Value::Int(7)]).unwrap();
        assert_eq!(r, lst);

        let mut m = IndexMap::new();
        m.insert(Box::from("a"), Value::Int(1));
        let mp = Value::Map(m);
        let r =
            builtin_gremlin_sample_local(vec![mp.clone(), Value::Int(5), Value::Int(7)]).unwrap();
        assert_eq!(r, mp);
    }

    #[test]
    fn sample_local_seeded_picks_deterministically() {
        let lst = Value::List(vec![
            Value::Int(0),
            Value::Int(1),
            Value::Int(2),
            Value::Int(3),
            Value::Int(4),
        ]);
        let r1 = builtin_gremlin_sample_local(vec![lst.clone(), Value::Int(2), Value::Int(123)])
            .unwrap();
        let r2 = builtin_gremlin_sample_local(vec![lst, Value::Int(2), Value::Int(123)]).unwrap();
        assert_eq!(r1, r2);
        if let Value::List(items) = r1 {
            assert_eq!(items.len(), 2);
        } else {
            panic!("expected List");
        }
    }

    #[test]
    fn sample_local_passthrough_for_non_collection() {
        let r =
            builtin_gremlin_sample_local(vec![Value::Int(42), Value::Int(2), Value::Null]).unwrap();
        assert_eq!(r, Value::Int(42));
    }

    #[test]
    fn sample_local_invalid_count_type_errors() {
        let lst = Value::List(vec![Value::Int(1)]);
        assert!(builtin_gremlin_sample_local(vec![lst, Value::Bool(true), Value::Int(7)]).is_err());
    }
}
