//! Hash-first dedup set used by `COUNT(DISTINCT ...)` and other distinct
//! aggregations.

use std::collections::HashMap;

use crate::types::Value;

/// Dedup bucket for hash-first distinct tracking.
///
/// On the fast path (no hash collision), we store a single `Value` and never
/// allocate a `String`.  Only when two different values hash to the same `u64`
/// do we fall back to `canonical_key()` string comparison.
#[derive(Debug)]
enum DistinctBucket {
    /// One entry whose canonical string key has NOT been computed yet.
    Single(Value),
    /// Two or more entries that collided — tracked by canonical string keys.
    Resolved(Vec<String>),
}

/// Hash-first dedup set for `COUNT(DISTINCT ...)` values.
///
/// Mirrors the `RowDedupSet` design: the fast path (no collision) avoids any
/// `String` allocation.  Only hash-bucket collisions trigger `canonical_key()`.
#[derive(Debug, Default)]
pub(super) struct DistinctSet {
    inner: HashMap<u64, DistinctBucket>,
}

impl DistinctSet {
    #[cfg(test)]
    pub(super) fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.inner
            .values()
            .map(|b| match b {
                DistinctBucket::Single(_) => 1,
                DistinctBucket::Resolved(keys) => keys.len(),
            })
            .sum()
    }

    /// Returns `true` if the value was **not** previously present (inserted).
    pub(super) fn insert(&mut self, val: &Value) -> bool {
        use std::collections::hash_map::Entry;

        let h = value_hash(val);
        match self.inner.entry(h) {
            Entry::Vacant(e) => {
                // Fast path: hash never seen.  Zero String allocations.
                e.insert(DistinctBucket::Single(val.clone()));
                true
            }
            Entry::Occupied(mut e) => {
                let key = val.canonical_key();
                let bucket = e.get_mut();
                match bucket {
                    DistinctBucket::Single(first_val) => {
                        let first_key = first_val.canonical_key();
                        if first_key == key {
                            // Duplicate
                            *bucket = DistinctBucket::Resolved(vec![first_key]);
                            false
                        } else {
                            // True collision — different values, same hash
                            *bucket = DistinctBucket::Resolved(vec![first_key, key]);
                            true
                        }
                    }
                    DistinctBucket::Resolved(keys) => {
                        if keys.contains(&key) {
                            false
                        } else {
                            keys.push(key);
                            true
                        }
                    }
                }
            }
        }
    }
}

/// Compute a `u64` hash for a single `Value` via `canonical_hash`.
fn value_hash(val: &Value) -> u64 {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    val.canonical_hash(&mut hasher);
    hasher.finish()
}
