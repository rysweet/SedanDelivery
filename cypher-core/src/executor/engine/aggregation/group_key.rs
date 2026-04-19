//! Group-key hashing helpers used by `exec_aggregate`.
//!
//! Mirrors the `RowDedupSet` / `DistinctSet` design: we keep a `u64` hash
//! index for O(1) group lookup and only materialise canonical string keys on
//! a hash collision.

use crate::types::Value;

/// Bucket for the group-key hash index (mirrors `DistinctBucket` /
/// `RowDedupSet`).
pub(super) enum GroupBucket {
    /// One group whose canonical string key has NOT been computed yet.
    Single(usize),
    /// Two or more groups that share the same u64 hash — tracked by canonical
    /// string keys paired with their group index.
    Resolved(Vec<(String, usize)>),
}

/// Compute a combined u64 hash from a slice of group-key `Value`s.
pub(super) fn group_key_hash(group_key: &[Value]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    group_key.len().hash(&mut hasher);
    for v in group_key {
        v.canonical_hash(&mut hasher);
    }
    hasher.finish()
}

/// Build a canonical string key from a slice of group-key `Value`s.
///
/// Only called on hash collision (rare).  The result is deterministic and
/// consistent with `canonical_key()` — equal values produce equal strings.
pub(super) fn group_key_string(group_key: &[Value]) -> String {
    group_key
        .iter()
        .map(|v| v.canonical_key())
        .collect::<Vec<_>>()
        .join("|")
}
