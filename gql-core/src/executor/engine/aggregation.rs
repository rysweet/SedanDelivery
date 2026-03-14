//! Aggregation operator and state for the query executor.

use std::collections::HashMap;

use crate::ast::Expr;
use crate::error::{ExecutionError, GqlError};
use crate::planner::{AggFunction, Operator};
use crate::types::Value;

use super::helpers::{cmp_values, expr_to_column_name, inject_parameters};
use super::{ExecContext, QueryExecutor, Row};

// ── Hash-based distinct dedup ────────────────────────────────────────────────

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
struct DistinctSet {
    inner: HashMap<u64, DistinctBucket>,
}

impl DistinctSet {
    #[cfg(test)]
    fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    fn len(&self) -> usize {
        self.inner
            .values()
            .map(|b| match b {
                DistinctBucket::Single(_) => 1,
                DistinctBucket::Resolved(keys) => keys.len(),
            })
            .sum()
    }

    /// Returns `true` if the value was **not** previously present (inserted).
    fn insert(&mut self, val: &Value) -> bool {
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

// ── Aggregation state ─────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub(crate) struct AggState {
    count: i64,
    sum: Option<f64>,
    min: Option<Value>,
    max: Option<Value>,
    collected: Vec<Value>,
    seen_distinct: DistinctSet,
    /// Percentile parameter for percentileDisc/percentileCont (0.0..=1.0).
    percentile: Option<f64>,
}

impl AggState {
    pub(crate) fn accumulate(
        &mut self,
        func: &AggFunction,
        val: Value,
        second_val: Option<Value>,
        distinct: bool,
    ) {
        // For distinct aggregations, skip duplicates.
        // Cap the set size to prevent OOM via hostile queries with millions
        // of distinct values — consistent with max_collect_size guard on Collect.
        if distinct {
            let max_size = crate::config::global().max_collect_size as usize;
            if self.seen_distinct.len() >= max_size {
                return; // silently drop excess — same as Collect overflow
            }
            if !self.seen_distinct.insert(&val) {
                return;
            }
        }

        match func {
            AggFunction::Count => {
                if !val.is_null() {
                    self.count += 1;
                }
            }
            AggFunction::Sum => {
                if let Some(f) = val.as_float() {
                    self.sum = Some(self.sum.unwrap_or(0.0) + f);
                }
            }
            AggFunction::Avg => {
                if let Some(f) = val.as_float() {
                    self.sum = Some(self.sum.unwrap_or(0.0) + f);
                    self.count += 1;
                }
            }
            AggFunction::Min => {
                if !val.is_null() {
                    self.min = Some(match self.min.take() {
                        None => val,
                        Some(current) => {
                            if cmp_values(&val, &current) == std::cmp::Ordering::Less {
                                val
                            } else {
                                current
                            }
                        }
                    });
                }
            }
            AggFunction::Max => {
                if !val.is_null() {
                    self.max = Some(match self.max.take() {
                        None => val,
                        Some(current) => {
                            if cmp_values(&val, &current) == std::cmp::Ordering::Greater {
                                val
                            } else {
                                current
                            }
                        }
                    });
                }
            }
            AggFunction::Collect => {
                let max_collect = crate::config::global().max_collect_size;
                if !val.is_null() && self.collected.len() < max_collect {
                    self.collected.push(val);
                }
            }
            AggFunction::StDev | AggFunction::StDevP => {
                let max_collect = crate::config::global().max_collect_size;
                if !val.is_null() && self.collected.len() < max_collect {
                    if let Some(f) = val.as_float() {
                        self.collected.push(Value::Float(f));
                    }
                }
            }
            AggFunction::PercentileDisc | AggFunction::PercentileCont => {
                let max_collect = crate::config::global().max_collect_size;
                if !val.is_null() && self.collected.len() < max_collect {
                    if let Some(f) = val.as_float() {
                        self.collected.push(Value::Float(f));
                    }
                }
                // Store the percentile parameter (constant across rows, first one wins).
                if self.percentile.is_none() {
                    if let Some(pval) = &second_val {
                        if let Some(f) = pval.as_float() {
                            self.percentile = Some(f);
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn finalize(&self, func: &AggFunction) -> Result<Value, GqlError> {
        match func {
            AggFunction::Count => Ok(Value::Int(self.count)),
            AggFunction::Sum => Ok(self.sum.map(Value::Float).unwrap_or(Value::Int(0))),
            AggFunction::Avg => {
                if self.count == 0 {
                    Ok(Value::Null)
                } else {
                    Ok(Value::Float(self.sum.unwrap_or(0.0) / self.count as f64))
                }
            }
            AggFunction::Min => Ok(self.min.clone().unwrap_or(Value::Null)),
            AggFunction::Max => Ok(self.max.clone().unwrap_or(Value::Null)),
            AggFunction::Collect => Ok(Value::List(self.collected.clone())),
            AggFunction::StDev => {
                let nums: Vec<f64> = self.collected.iter().filter_map(|v| v.as_float()).collect();
                if nums.len() <= 1 {
                    return Ok(Value::Float(0.0));
                }
                let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                let variance =
                    nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (nums.len() - 1) as f64;
                Ok(Value::Float(variance.sqrt()))
            }
            AggFunction::StDevP => {
                let nums: Vec<f64> = self.collected.iter().filter_map(|v| v.as_float()).collect();
                if nums.is_empty() {
                    return Ok(Value::Float(0.0));
                }
                let mean = nums.iter().sum::<f64>() / nums.len() as f64;
                let variance =
                    nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / nums.len() as f64;
                Ok(Value::Float(variance.sqrt()))
            }
            AggFunction::PercentileDisc => {
                let p = self.percentile.unwrap_or(0.5);
                if !(0.0..=1.0).contains(&p) {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "percentile in range [0.0, 1.0]".to_string(),
                        actual: format!("NumberOutOfRange: {p}"),
                    }));
                }
                let mut nums: Vec<f64> =
                    self.collected.iter().filter_map(|v| v.as_float()).collect();
                if nums.is_empty() {
                    return Ok(Value::Null);
                }
                nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                // Nearest rank method: index = ceil(p * n) - 1 (clamped)
                let n = nums.len();
                let idx = ((p * n as f64).ceil() as usize)
                    .saturating_sub(1)
                    .min(n - 1);
                // Return as integer if all values are whole numbers
                let v = nums[idx];
                if v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 {
                    Ok(Value::Int(v as i64))
                } else {
                    Ok(Value::Float(v))
                }
            }
            AggFunction::PercentileCont => {
                let p = self.percentile.unwrap_or(0.5);
                if !(0.0..=1.0).contains(&p) {
                    return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                        expected: "percentile in range [0.0, 1.0]".to_string(),
                        actual: format!("NumberOutOfRange: {p}"),
                    }));
                }
                let mut nums: Vec<f64> =
                    self.collected.iter().filter_map(|v| v.as_float()).collect();
                if nums.is_empty() {
                    return Ok(Value::Null);
                }
                nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                let idx = p * (nums.len() - 1) as f64;
                let lo = idx.floor() as usize;
                let hi = (idx.ceil() as usize).min(nums.len() - 1);
                if lo == hi {
                    Ok(Value::Float(nums[lo]))
                } else {
                    let frac = idx - lo as f64;
                    Ok(Value::Float(nums[lo] * (1.0 - frac) + nums[hi] * frac))
                }
            }
        }
    }
}

// ── Group key hashing helpers ────────────────────────────────────────────────

/// Bucket for the group-key hash index (mirrors `DistinctBucket` / `RowDedupSet`).
enum GroupBucket {
    /// One group whose canonical string key has NOT been computed yet.
    Single(usize),
    /// Two or more groups that share the same u64 hash — tracked by canonical
    /// string keys paired with their group index.
    Resolved(Vec<(String, usize)>),
}

/// Compute a combined u64 hash from a slice of group-key `Value`s.
fn group_key_hash(group_key: &[Value]) -> u64 {
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
fn group_key_string(group_key: &[Value]) -> String {
    group_key
        .iter()
        .map(|v| v.canonical_key())
        .collect::<Vec<_>>()
        .join("|")
}

// ── QueryExecutor impl for aggregation ────────────────────────────────────────

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_aggregate(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Aggregate {
            keys,
            aggregations,
            child,
        } = op
        else {
            debug_assert!(false, "expected Aggregate operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Aggregate operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        cx.budget.check_or_err()?;

        // Group rows by the key expression values.
        // We use a Vec-of-(key_row, agg_state) for ordered output and a
        // hash-first index for O(1) group lookup.
        //
        // Fast path (no hash collisions): only u64 hashes are stored — zero
        // String allocations.  On collision we fall back to canonical_key()
        // comparison, matching the RowDedupSet pattern.
        let mut groups: Vec<(Vec<Value>, Vec<AggState>)> = Vec::new();
        let mut group_index: HashMap<u64, GroupBucket> = HashMap::new();

        for row in &input {
            let ctx = self.build_filter_ctx(cx.graph, row)?;
            let group_key: Vec<Value> = keys
                .iter()
                .map(|expr| self.eval_with_storage(cx.graph, expr, row, &ctx))
                .collect::<Result<Vec<_>, GqlError>>()?;

            // Compute a u64 hash from the group key values via canonical_hash.
            let h = group_key_hash(&group_key);

            // Find existing group or create new — O(1) via the hash index.
            // canonical_hash() ensures equal values (e.g. -0.0/0.0, maps with
            // different insertion order, same-instant DateTimes in different
            // time zones) produce the same hash.
            let idx = match group_index.entry(h) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    // Fast path: hash never seen — zero String allocations.
                    let i = groups.len();
                    let new_states = aggregations.iter().map(|_| AggState::default()).collect();
                    groups.push((group_key, new_states));
                    e.insert(GroupBucket::Single(i));
                    i
                }
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    let key_str = group_key_string(&group_key);
                    match e.get_mut() {
                        GroupBucket::Single(first_idx) => {
                            let first_key_str = group_key_string(&groups[*first_idx].0);
                            if first_key_str == key_str {
                                // Same group
                                let i = *first_idx;
                                *e.get_mut() = GroupBucket::Resolved(vec![(first_key_str, i)]);
                                i
                            } else {
                                // True collision — different groups share the same hash
                                let i = groups.len();
                                let new_states =
                                    aggregations.iter().map(|_| AggState::default()).collect();
                                groups.push((group_key, new_states));
                                let fi = *first_idx;
                                *e.get_mut() =
                                    GroupBucket::Resolved(vec![(first_key_str, fi), (key_str, i)]);
                                i
                            }
                        }
                        GroupBucket::Resolved(entries) => {
                            if let Some(&(_, i)) = entries.iter().find(|(k, _)| *k == key_str) {
                                i
                            } else {
                                let i = groups.len();
                                let new_states =
                                    aggregations.iter().map(|_| AggState::default()).collect();
                                groups.push((group_key, new_states));
                                entries.push((key_str, i));
                                i
                            }
                        }
                    }
                }
            };

            // Accumulate into each aggregation state.
            for (agg_idx, (func, arg_expr, second_arg_expr, distinct, _alias)) in
                aggregations.iter().enumerate()
            {
                // count(*) — the `*` argument is a sentinel meaning "count rows".
                // Use a non-null placeholder so the accumulator counts every row.
                let arg_val = if matches!(arg_expr, Expr::Variable(ref id) if id.0 == "*") {
                    Value::Bool(true)
                } else {
                    self.eval_with_storage(cx.graph, arg_expr, row, &ctx)?
                };
                let second_val = second_arg_expr
                    .as_ref()
                    .map(|e| self.eval_with_storage(cx.graph, e, row, &ctx))
                    .transpose()?;
                groups[idx].1[agg_idx].accumulate(func, arg_val, second_val, *distinct);
            }
        }

        // Handle empty input with no-key aggregation (e.g. COUNT(*) on empty).
        if groups.is_empty() && keys.is_empty() {
            let new_states: Vec<AggState> =
                aggregations.iter().map(|_| AggState::default()).collect();
            groups.push((vec![], new_states));
        }

        // Build output rows.
        let mut out = Vec::with_capacity(groups.len());
        for (key_vals, agg_states) in groups {
            cx.budget.check_or_err()?;
            let mut row = Row::new();

            // Add group key columns — derive names from key expressions.
            for (expr, val) in keys.iter().zip(key_vals) {
                row.insert(expr_to_column_name(expr), val);
            }

            // Add aggregation result columns.
            for ((func, _, _, _, alias), state) in aggregations.iter().zip(&agg_states) {
                row.insert(alias.0.clone(), state.finalize(func)?);
            }
            // Preserve query parameters so downstream operators (e.g. Delete,
            // Filter) can still access $param bindings after aggregation.
            inject_parameters(&mut row, &self.parameters);
            cx.budget.record_row();
            out.push(row);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Identifier, Label};
    use crate::executor::budget::ExecutionBudget;
    use crate::planner::Operator;
    use crate::storage::{GraphStore, InMemoryGraphStore};

    fn make_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("g").unwrap();
        store
    }

    #[test]
    fn agg_count() {
        let mut s = AggState::default();
        s.accumulate(&AggFunction::Count, Value::Int(1), None, false);
        s.accumulate(&AggFunction::Count, Value::Null, None, false);
        assert_eq!(s.finalize(&AggFunction::Count).unwrap(), Value::Int(1));
    }

    #[test]
    fn agg_sum() {
        let mut s = AggState::default();
        s.accumulate(&AggFunction::Sum, Value::Int(10), None, false);
        s.accumulate(&AggFunction::Sum, Value::Int(20), None, false);
        assert_eq!(s.finalize(&AggFunction::Sum).unwrap(), Value::Float(30.0));
    }

    #[test]
    fn agg_avg() {
        let mut s = AggState::default();
        s.accumulate(&AggFunction::Avg, Value::Int(10), None, false);
        s.accumulate(&AggFunction::Avg, Value::Int(20), None, false);
        assert_eq!(s.finalize(&AggFunction::Avg).unwrap(), Value::Float(15.0));
    }

    #[test]
    fn agg_min_max() {
        let mut min_s = AggState::default();
        min_s.accumulate(&AggFunction::Min, Value::Int(5), None, false);
        min_s.accumulate(&AggFunction::Min, Value::Int(2), None, false);
        assert_eq!(min_s.finalize(&AggFunction::Min).unwrap(), Value::Int(2));

        let mut max_s = AggState::default();
        max_s.accumulate(&AggFunction::Max, Value::Int(5), None, false);
        max_s.accumulate(&AggFunction::Max, Value::Int(8), None, false);
        assert_eq!(max_s.finalize(&AggFunction::Max).unwrap(), Value::Int(8));
    }

    #[test]
    fn agg_collect() {
        let mut s = AggState::default();
        s.accumulate(&AggFunction::Collect, Value::Int(1), None, false);
        s.accumulate(&AggFunction::Collect, Value::Int(2), None, false);
        s.accumulate(&AggFunction::Collect, Value::Null, None, false);
        assert_eq!(
            s.finalize(&AggFunction::Collect).unwrap(),
            Value::List(vec![Value::Int(1), Value::Int(2)])
        );
    }

    #[test]
    fn agg_distinct() {
        let mut s = AggState::default();
        s.accumulate(&AggFunction::Count, Value::Int(1), None, true);
        s.accumulate(&AggFunction::Count, Value::Int(1), None, true);
        s.accumulate(&AggFunction::Count, Value::Int(2), None, true);
        assert_eq!(s.finalize(&AggFunction::Count).unwrap(), Value::Int(2));
    }

    #[test]
    fn distinct_set_basic() {
        let mut ds = DistinctSet::new();
        assert!(ds.insert(&Value::Int(1)));
        assert!(!ds.insert(&Value::Int(1))); // duplicate
        assert!(ds.insert(&Value::Int(2)));
        assert_eq!(ds.len(), 2);
    }

    #[test]
    fn distinct_set_float_neg_zero() {
        let mut ds = DistinctSet::new();
        assert!(ds.insert(&Value::Float(0.0)));
        assert!(!ds.insert(&Value::Float(-0.0))); // -0.0 == 0.0
        assert_eq!(ds.len(), 1);
    }

    #[test]
    fn distinct_set_mixed_types() {
        let mut ds = DistinctSet::new();
        assert!(ds.insert(&Value::Int(1)));
        assert!(ds.insert(&Value::Str("1".into())));
        assert!(ds.insert(&Value::Float(1.0)));
        assert_eq!(ds.len(), 3);
    }

    #[test]
    fn group_key_hash_deterministic() {
        let key1 = vec![Value::Int(1), Value::Str("a".into())];
        let key2 = vec![Value::Int(1), Value::Str("a".into())];
        assert_eq!(group_key_hash(&key1), group_key_hash(&key2));
    }

    #[test]
    fn group_key_hash_different_keys_differ() {
        let key1 = vec![Value::Int(1)];
        let key2 = vec![Value::Int(2)];
        assert_ne!(group_key_hash(&key1), group_key_hash(&key2));
    }

    #[test]
    fn group_key_string_deterministic() {
        let key = vec![Value::Int(1), Value::Str("hello".into())];
        let s1 = group_key_string(&key);
        let s2 = group_key_string(&key);
        assert_eq!(s1, s2);
    }

    #[test]
    fn exec_aggregate_count() {
        let store = make_store();
        for _ in 0..3 {
            store
                .create_node("g", vec!["P".to_string()], HashMap::new())
                .unwrap();
        }
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Aggregate {
            keys: vec![],
            aggregations: vec![(
                AggFunction::Count,
                Expr::Variable(Identifier::new("n")),
                None,
                false,
                Identifier::new("c"),
            )],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("P")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows[0]["c"], Value::Int(3));
    }

    #[test]
    fn exec_aggregate_empty_input() {
        let store = make_store();
        let executor = QueryExecutor::new(&store);
        let plan = Operator::Aggregate {
            keys: vec![],
            aggregations: vec![(
                AggFunction::Count,
                Expr::Variable(Identifier::new("*")),
                None,
                false,
                Identifier::new("c"),
            )],
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("X")),
                alias: Identifier::new("n"),
            }),
        };
        let mut budget = ExecutionBudget::unlimited();
        let result = executor.execute("g", &plan, &mut budget).unwrap();
        assert_eq!(result.rows[0]["c"], Value::Int(0));
    }
}
