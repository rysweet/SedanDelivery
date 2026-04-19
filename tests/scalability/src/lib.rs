//! Shared seeding and expectation helpers for the scalability suite.
//!
//! Datasets are built directly through the [`GraphStore`] trait (no query
//! parsing) so seed time is dominated by storage and not by the engine
//! under test. Generators are deterministic — every test that uses the
//! same generator computes its own expected count from the same RNG
//! stream, so assertions are exact rather than approximate.

use std::time::{Duration, Instant};

use indexmap::IndexMap;
use sedan_cypher_embedded::{GraphStore, InMemoryGraphStore, Value};

// ── Dataset sizes (kept as constants so all three engines agree) ─────────
pub const LINEAR_NODES: usize = 100_000;
pub const LINEAR_EDGES: usize = LINEAR_NODES - 1;
pub const LINEAR_PATH_LEN: usize = 5;
pub const LINEAR_EXPECTED_PATHS: usize = LINEAR_NODES - LINEAR_PATH_LEN;

pub const DENSE_NODES: usize = 10_000;
pub const DENSE_EDGES: usize = 500_000;
pub const DENSE_RNG_SEED: u64 = 0x5EDA_4DE1_1A57_C0DE;

pub const PROP_NODES: usize = 100_000;
/// Predicate: `weight > 50_000 AND parity = true`. Even ints in 50_002..=99_998.
pub const PROP_EXPECTED_HITS: usize = 24_999;

/// Soft wall-clock budget per scenario. Crosses this and the test fails so
/// regressions surface loudly in CI rather than silently slowing down.
pub const BUDGET: Duration = Duration::from_secs(60);

/// xorshift64* — small, deterministic PRNG with no external dependency.
#[derive(Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // Avoid the all-zero state which would render xorshift inert.
        Self(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn gen_range(&mut self, end: u64) -> u64 {
        self.next_u64() % end
    }
}

/// Create a fresh in-memory store with a single graph named `g`.
pub fn fresh_store() -> InMemoryGraphStore {
    let s = InMemoryGraphStore::default();
    s.create_graph("g").expect("create_graph");
    s
}

/// Wrap a closure with timing, asserting the soft wall-clock budget.
pub fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    let elapsed = start.elapsed();
    eprintln!("[scalability] {label}: {elapsed:?}");
    assert!(
        elapsed < BUDGET,
        "{label} exceeded soft budget {BUDGET:?}: took {elapsed:?}"
    );
    out
}

// ── Linear chain ─────────────────────────────────────────────────────────

/// Seed `LINEAR_NODES` nodes labelled `:Node` connected by `:NEXT` edges
/// in a single linear chain `0 -> 1 -> ... -> N-1`.
pub fn seed_linear(store: &InMemoryGraphStore) {
    let nodes: Vec<_> = (0..LINEAR_NODES)
        .map(|i| {
            let mut p = IndexMap::new();
            p.insert("idx".to_string(), Value::Int(i as i64));
            (vec!["Node".to_string()], p)
        })
        .collect();
    let ids = store
        .batch_create_nodes("g", nodes)
        .expect("batch_create_nodes");

    let edges: Vec<_> = (0..LINEAR_EDGES)
        .map(|i| (ids[i], ids[i + 1], "NEXT".to_string(), IndexMap::new()))
        .collect();
    store
        .batch_create_edges("g", edges)
        .expect("batch_create_edges");
}

// ── Dense random graph ───────────────────────────────────────────────────

/// Seed a dense random multi-digraph: `DENSE_NODES` nodes, `DENSE_EDGES`
/// directed edges chosen uniformly at random with `DENSE_RNG_SEED`. Node 0
/// also receives the `:Seed` label.
///
/// Returns the exact count of distinct nodes reachable in exactly two hops
/// from node 0 — the expected answer for the dense correctness query.
pub fn seed_dense(store: &InMemoryGraphStore) -> usize {
    let mut rng = Rng::new(DENSE_RNG_SEED);
    let nodes: Vec<_> = (0..DENSE_NODES)
        .map(|i| {
            let labels = if i == 0 {
                vec!["Node".to_string(), "Seed".to_string()]
            } else {
                vec!["Node".to_string()]
            };
            let mut p = IndexMap::new();
            p.insert("idx".to_string(), Value::Int(i as i64));
            (labels, p)
        })
        .collect();
    let ids = store.batch_create_nodes("g", nodes).expect("nodes");

    let mut adj_out: Vec<Vec<u32>> = vec![Vec::new(); DENSE_NODES];
    let mut edges = Vec::with_capacity(DENSE_EDGES);
    for _ in 0..DENSE_EDGES {
        let src = rng.gen_range(DENSE_NODES as u64) as usize;
        let dst = rng.gen_range(DENSE_NODES as u64) as usize;
        adj_out[src].push(dst as u32);
        edges.push((ids[src], ids[dst], "REL".to_string(), IndexMap::new()));
    }
    store.batch_create_edges("g", edges).expect("edges");

    // Compute distinct two-hop reachable set from node 0.
    let mut reach = vec![false; DENSE_NODES];
    for &mid in &adj_out[0] {
        for &dst in &adj_out[mid as usize] {
            reach[dst as usize] = true;
        }
    }
    reach.iter().filter(|b| **b).count()
}

// ── Property scan ────────────────────────────────────────────────────────

/// Seed `PROP_NODES` nodes labelled `:Node` with mixed property types:
/// `idx: Int`, `weight: Int (= idx)`, `parity: Bool (= idx % 2 == 0)`,
/// `name: Str ("user_<idx>")`, `score: Float (= idx as f64 / 7.0)`.
///
/// NB: the integer property is named `weight` rather than `value` to dodge
/// a Gremlin embedded parser collision — see the bug filed alongside this
/// suite. Cypher and GQL accept either name; keeping a single seed across
/// engines lets us re-use the same expected-count constant.
pub fn seed_props(store: &InMemoryGraphStore) {
    let nodes: Vec<_> = (0..PROP_NODES)
        .map(|i| {
            let mut p = IndexMap::new();
            p.insert("idx".to_string(), Value::Int(i as i64));
            p.insert("weight".to_string(), Value::Int(i as i64));
            p.insert("parity".to_string(), Value::Bool(i % 2 == 0));
            p.insert(
                "name".to_string(),
                Value::Str(format!("user_{i}").into_boxed_str()),
            );
            p.insert("score".to_string(), Value::Float(i as f64 / 7.0));
            (vec!["Node".to_string()], p)
        })
        .collect();
    store.batch_create_nodes("g", nodes).expect("nodes");
}

/// Pull an integer count out of a single-row, single-column query result.
/// Accepts either a column called `n` (Cypher/GQL) or the first column
/// regardless of name (Gremlin's `count()` emits a synthetic column).
pub fn extract_count(columns: &[String], rows: &[std::collections::HashMap<String, Value>]) -> i64 {
    assert_eq!(rows.len(), 1, "expected exactly 1 row, got {}", rows.len());
    let row = &rows[0];
    let key = if row.contains_key("n") {
        "n".to_string()
    } else {
        columns
            .first()
            .cloned()
            .unwrap_or_else(|| row.keys().next().cloned().expect("no columns"))
    };
    match row.get(&key) {
        Some(Value::Int(i)) => *i,
        other => panic!("expected Int count for column {key:?}, got {other:?}"),
    }
}
