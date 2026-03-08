//! Thread-safe LRU query plan cache.
//!
//! Caches parsed AST + planned operator trees keyed by a hash of the Cypher
//! query string. On cache hit the plan is cloned (both `Query` and `Operator`
//! derive `Clone`), avoiding the cost of re-parsing and re-planning.
//!
//! # Invalidation
//!
//! Call [`PlanCache::clear`] after DDL operations that change the schema
//! (e.g. `CREATE INDEX`, `DROP INDEX`) since plans may reference index scans
//! that are no longer valid.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Mutex;

use crate::ast::Query;
use crate::planner::Operator;

/// A thread-safe LRU cache mapping query string hashes to parsed + planned results.
///
/// Uses a `Mutex<HashMap>` with a simple capacity eviction strategy: when the
/// cache exceeds `capacity`, the entire map is cleared (epoch-based eviction).
/// This is simpler and faster than a full LRU linked-list for a query cache
/// where mass eviction on schema change is already required.
pub struct PlanCache {
    cache: Mutex<HashMap<u64, (Query, Operator)>>,
    capacity: usize,
}

impl PlanCache {
    /// Create a new cache with the given maximum number of entries.
    pub fn new(capacity: usize) -> Self {
        Self {
            cache: Mutex::new(HashMap::with_capacity(capacity.min(1024))),
            capacity,
        }
    }

    /// Look up a cached plan by query string.
    ///
    /// Returns `Some((query, operator))` cloned from the cache on hit, `None` on miss.
    pub fn get(&self, query_str: &str) -> Option<(Query, Operator)> {
        let key = hash_query(query_str);
        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.get(&key).cloned()
    }

    /// Insert a parsed AST and planned operator tree into the cache.
    ///
    /// If the cache has reached capacity, it is cleared before inserting
    /// (epoch-based eviction).
    pub fn insert(&self, query_str: &str, ast: Query, plan: Operator) {
        let key = hash_query(query_str);
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if guard.len() >= self.capacity {
            guard.clear();
        }
        guard.insert(key, (ast, plan));
    }

    /// Clear all cached plans.
    ///
    /// Must be called after schema-changing DDL operations
    /// (`CREATE INDEX`, `DROP INDEX`, etc.).
    pub fn clear(&self) {
        let mut guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.clear();
    }

    /// Return the number of entries currently in the cache.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        let guard = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        guard.len()
    }

    /// Return `true` if the cache is empty.
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Hash a query string to produce a cache key.
fn hash_query(query: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    query.hash(&mut hasher);
    hasher.finish()
}

// ── Global singleton ────────────────────────────────────────────────────────

use std::sync::OnceLock;

static GLOBAL_CACHE: OnceLock<PlanCache> = OnceLock::new();

/// Return a reference to the process-wide [`PlanCache`].
///
/// On first call, the cache is created with the capacity from
/// [`crate::config::global().plan_cache_size`].
pub fn global() -> &'static PlanCache {
    GLOBAL_CACHE.get_or_init(|| PlanCache::new(crate::config::global().plan_cache_size))
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::{Clause, Query, ReturnClause, ReturnItem};
    use crate::planner::Operator;

    fn dummy_plan() -> (Query, Operator) {
        let query = Query::new(vec![Clause::Return(ReturnClause {
            distinct: false,
            items: vec![ReturnItem {
                expr: crate::ast::Expr::Integer(1),
                alias: None,
            }],
            order_by: vec![],
            skip: None,
            limit: None,
        })]);
        let op = Operator::SingleRow;
        (query, op)
    }

    #[test]
    fn get_returns_none_on_empty_cache() {
        let cache = PlanCache::new(10);
        assert!(cache.get("RETURN 1").is_none());
    }

    #[test]
    fn insert_and_get() {
        let cache = PlanCache::new(10);
        let (ast, plan) = dummy_plan();
        cache.insert("RETURN 1", ast, plan);
        assert!(cache.get("RETURN 1").is_some());
        assert!(cache.get("RETURN 2").is_none());
    }

    #[test]
    fn clear_removes_all_entries() {
        let cache = PlanCache::new(10);
        let (ast, plan) = dummy_plan();
        cache.insert("RETURN 1", ast.clone(), plan.clone());
        cache.insert("RETURN 2", ast, plan);
        assert_eq!(cache.len(), 2);
        cache.clear();
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn evicts_when_at_capacity() {
        let cache = PlanCache::new(2);
        let (ast, plan) = dummy_plan();
        cache.insert("q1", ast.clone(), plan.clone());
        cache.insert("q2", ast.clone(), plan.clone());
        assert_eq!(cache.len(), 2);
        // Third insert should trigger eviction (clear + insert)
        cache.insert("q3", ast, plan);
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn hash_query_deterministic() {
        let h1 = hash_query("MATCH (n) RETURN n");
        let h2 = hash_query("MATCH (n) RETURN n");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_query_different_for_different_queries() {
        let h1 = hash_query("MATCH (n) RETURN n");
        let h2 = hash_query("MATCH (m) RETURN m");
        assert_ne!(h1, h2);
    }
}
