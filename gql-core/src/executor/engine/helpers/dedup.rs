//! Row deduplication for DISTINCT and UNION operations.

use indexmap::IndexMap;
use std::collections::HashMap;

use super::Row;

/// Compute a `u64` hash of a row using [`Value::canonical_hash`].
///
/// Column names are sorted so that `HashMap` iteration order does not matter.
/// This is the allocation-free core of [`RowDedupSet`].
fn row_hash(row: &Row) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    let mut keys: Vec<&str> = row.keys().map(|k| k.as_str()).collect();
    keys.sort();
    keys.len().hash(&mut hasher);
    for k in keys {
        k.hash(&mut hasher);
        row[k].canonical_hash(&mut hasher);
    }
    hasher.finish()
}

/// Generate a deterministic string key for a row.
///
/// Still used as an exact-match fallback inside [`RowDedupSet`] on hash
/// collision, and in tests for backward-compatibility checks.
pub(crate) fn row_key(row: &Row) -> String {
    let mut pairs: Vec<(&str, String)> = row
        .iter()
        .map(|(k, v)| (k.as_str(), v.canonical_key()))
        .collect();
    pairs.sort_by_key(|(k, _)| *k);
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("|")
}

/// A dedup set for rows that avoids `String` allocation in the common case.
///
/// Uses `Value::canonical_hash` (zero allocation) for the hash lookup.
/// On the first insert for a given hash the row is stored so its canonical
/// string key can be computed lazily if a collision ever occurs.  For
/// workloads where most hashes are unique (typical DISTINCT / UNION), the
/// expensive `canonical_key()` string path is never entered.
pub(crate) struct RowDedupSet {
    inner: HashMap<u64, DedupBucket>,
}

enum DedupBucket {
    /// One entry whose canonical string key has NOT been computed yet.
    Single(Row),
    /// One or more entries with their canonical string keys.
    Resolved(Vec<String>),
}

impl RowDedupSet {
    pub(crate) fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Returns `true` if the row was **not** previously present (inserted).
    pub(crate) fn insert(&mut self, row: &Row) -> bool {
        use std::collections::hash_map::Entry;
        let h = row_hash(row);
        match self.inner.entry(h) {
            Entry::Vacant(e) => {
                // Fast path: hash never seen.  Zero String allocations.
                e.insert(DedupBucket::Single(row.clone()));
                true
            }
            Entry::Occupied(mut e) => {
                let key = row_key(row);
                let bucket = e.get_mut();
                match bucket {
                    DedupBucket::Single(first_row) => {
                        let first_key = row_key(first_row);
                        if first_key == key {
                            *bucket = DedupBucket::Resolved(vec![first_key]);
                            false
                        } else {
                            *bucket = DedupBucket::Resolved(vec![first_key, key]);
                            true
                        }
                    }
                    DedupBucket::Resolved(keys) => {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Value;

    #[test]
    fn row_key_deterministic() {
        let mut r1 = Row::new();
        r1.insert("a".to_string(), Value::Int(1));
        r1.insert("b".to_string(), Value::Str("h".into()));
        let mut r2 = Row::new();
        r2.insert("b".to_string(), Value::Str("h".into()));
        r2.insert("a".to_string(), Value::Int(1));
        assert_eq!(row_key(&r1), row_key(&r2));
    }

    #[test]
    fn dedup_set_basic_insert_and_duplicate() {
        let mut set = RowDedupSet::new();
        let mut r1 = Row::new();
        r1.insert("x".to_string(), Value::Int(1));
        assert!(set.insert(&r1), "first insert should succeed");
        assert!(!set.insert(&r1), "duplicate should be rejected");
    }

    #[test]
    fn dedup_set_different_rows() {
        let mut set = RowDedupSet::new();
        let mut r1 = Row::new();
        r1.insert("x".to_string(), Value::Int(1));
        let mut r2 = Row::new();
        r2.insert("x".to_string(), Value::Int(2));
        assert!(set.insert(&r1));
        assert!(set.insert(&r2), "different rows should both be accepted");
    }

    #[test]
    fn dedup_set_float_neg_zero() {
        // -0.0 and 0.0 are equal per IEEE 754, so DISTINCT should collapse them.
        let mut set = RowDedupSet::new();
        let mut r1 = Row::new();
        r1.insert("x".to_string(), Value::Float(0.0));
        let mut r2 = Row::new();
        r2.insert("x".to_string(), Value::Float(-0.0));
        assert!(set.insert(&r1));
        assert!(
            !set.insert(&r2),
            "-0.0 row should be treated as duplicate of 0.0 row"
        );
    }

    #[test]
    fn dedup_set_float_nan() {
        // NaN == NaN is false per IEEE 754, but for DISTINCT all NaN values
        // should be collapsed into one group.
        let mut set = RowDedupSet::new();
        let mut r1 = Row::new();
        r1.insert("x".to_string(), Value::Float(f64::NAN));
        let mut r2 = Row::new();
        r2.insert("x".to_string(), Value::Float(f64::NAN));
        assert!(set.insert(&r1));
        assert!(!set.insert(&r2), "NaN rows should be treated as duplicates");
    }

    #[test]
    fn dedup_set_map_insertion_order() {
        // Maps with the same keys/values in different insertion order should
        // be treated as duplicates.
        let mut set = RowDedupSet::new();
        let mut m1 = indexmap::IndexMap::new();
        m1.insert(Box::<str>::from("a"), Value::Int(1));
        m1.insert(Box::<str>::from("b"), Value::Int(2));
        let mut r1 = Row::new();
        r1.insert("m".to_string(), Value::Map(m1));

        let mut m2 = indexmap::IndexMap::new();
        m2.insert(Box::<str>::from("b"), Value::Int(2));
        m2.insert(Box::<str>::from("a"), Value::Int(1));
        let mut r2 = Row::new();
        r2.insert("m".to_string(), Value::Map(m2));

        assert!(set.insert(&r1));
        assert!(!set.insert(&r2), "maps with same k/v should be duplicates");
    }

    #[test]
    fn dedup_set_preserves_first_row() {
        // DISTINCT should keep the first occurrence and drop later duplicates.
        let mut set = RowDedupSet::new();
        let mut rows = Vec::new();
        for _ in 0..5 {
            let mut r = Row::new();
            r.insert("x".to_string(), Value::Int(42));
            rows.push(r);
        }
        let mut out = Vec::new();
        for r in &rows {
            if set.insert(r) {
                out.push(r.clone());
            }
        }
        assert_eq!(out.len(), 1, "5 identical rows should produce 1 unique");
        assert_eq!(out[0]["x"], Value::Int(42));
    }

    #[test]
    fn dedup_set_agrees_with_row_key() {
        // The RowDedupSet result must agree with the old String-based row_key
        // approach for correctness.
        let test_rows: Vec<Row> = vec![
            {
                let mut r = Row::new();
                r.insert("a".to_string(), Value::Float(0.0));
                r
            },
            {
                let mut r = Row::new();
                r.insert("a".to_string(), Value::Float(-0.0));
                r
            },
            {
                let mut r = Row::new();
                r.insert("a".to_string(), Value::Float(f64::NAN));
                r
            },
            {
                let mut r = Row::new();
                r.insert("a".to_string(), Value::Float(f64::NAN));
                r
            },
            {
                let mut r = Row::new();
                r.insert("a".to_string(), Value::Int(1));
                r
            },
            {
                let mut r = Row::new();
                r.insert("a".to_string(), Value::Str("hello".into()));
                r
            },
        ];

        // Old approach: HashSet<String>
        let mut old_seen = std::collections::HashSet::new();
        let mut old_out = Vec::new();
        for r in &test_rows {
            let key = row_key(r);
            if old_seen.insert(key) {
                old_out.push(r.clone());
            }
        }

        // New approach: RowDedupSet
        let mut new_set = RowDedupSet::new();
        let mut new_out = Vec::new();
        for r in &test_rows {
            if new_set.insert(r) {
                new_out.push(r.clone());
            }
        }

        assert_eq!(
            old_out.len(),
            new_out.len(),
            "RowDedupSet must produce the same number of unique rows as row_key"
        );
    }

    #[test]
    fn row_hash_deterministic_across_insertion_order() {
        let mut r1 = Row::new();
        r1.insert("a".to_string(), Value::Int(1));
        r1.insert("b".to_string(), Value::Str("h".into()));
        let mut r2 = Row::new();
        r2.insert("b".to_string(), Value::Str("h".into()));
        r2.insert("a".to_string(), Value::Int(1));
        assert_eq!(row_hash(&r1), row_hash(&r2));
    }
}
