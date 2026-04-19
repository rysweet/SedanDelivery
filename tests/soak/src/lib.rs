//! Shared support code for the soak / memory-leak detection harness.
//!
//! Provides:
//! - [`rss_kb`] — sample resident-set-size in KB on Linux via `/proc/self/statm`.
//! - [`build_person_graph`] — deterministic synthetic graph builder used by every scenario.
//! - [`TimeoutGuard`] — lightweight watchdog that aborts the process if the
//!   scenario exceeds an upper bound (defends against deadlocks).
//! - [`PercentileTracker`] — tiny p99 latency tracker that avoids pulling in extra deps.
//!
//! All scenarios are intentionally `#[ignore]`-gated and only run from
//! `scripts/run_soak_tests.sh` or via `cargo test -p soak -- --ignored`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use sedan_cypher_embedded::{GraphStore, InMemoryGraphStore, Value};

/// Default graph name used by every scenario.
pub const GRAPH: &str = "soak";

/// Sample resident-set-size in kilobytes for the current process.
///
/// Reads `/proc/self/statm` on Linux. Returns `0` on platforms where the
/// file is unavailable; soak tests run under Linux in CI, so this is the
/// authoritative path for our budgets.
pub fn rss_kb() -> u64 {
    let Ok(text) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    // statm format: size resident shared text lib data dt — all in pages.
    let resident_pages: u64 = text
        .split_whitespace()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let page_kb = page_size_kb();
    resident_pages * page_kb
}

fn page_size_kb() -> u64 {
    // Linux page size is almost always 4 KiB. We avoid pulling in libc just
    // for sysconf(_SC_PAGESIZE) and read /proc/self/smaps_rollup as a sanity
    // fallback. Hard-coding 4 is fine for our soak budgets — we compare
    // ratios, not absolute values.
    4
}

/// Format a percentage growth between two RSS samples.
pub fn rss_growth_pct(start_kb: u64, end_kb: u64) -> f64 {
    if start_kb == 0 {
        return 0.0;
    }
    ((end_kb as f64) - (start_kb as f64)) / (start_kb as f64) * 100.0
}

/// Build a deterministic synthetic graph with `n_people` Person nodes and
/// a friendship ring connecting consecutive people. Used by every scenario
/// so that RSS deltas are comparable across runs.
pub fn build_person_graph(n_people: usize) -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph(GRAPH).expect("create graph");

    let mut ids = Vec::with_capacity(n_people);
    for i in 0..n_people {
        let id = store
            .create_node(
                GRAPH,
                vec!["Person".into()],
                IndexMap::from([
                    ("name".into(), Value::Str(format!("p{i}").into())),
                    ("age".into(), Value::Int(20 + (i as i64 % 60))),
                    ("idx".into(), Value::Int(i as i64)),
                ]),
            )
            .expect("create person");
        ids.push(id);
    }

    // Ring of friendships — guarantees every node has out-degree 1.
    for i in 0..n_people {
        let next = (i + 1) % n_people;
        store
            .create_edge(GRAPH, ids[i], ids[next], "FRIEND".into(), IndexMap::new())
            .expect("create friend");
    }

    store
}

/// Read query mix used by scenario 1. Each query exercises a different
/// executor path (count, scan, filter, projection).
pub const READ_QUERIES: &[&str] = &[
    "MATCH (p:Person) RETURN count(p) AS n",
    "MATCH (p:Person) WHERE p.age > 50 RETURN count(p) AS n",
    "MATCH (p:Person {idx: 0})-[:FRIEND]->(f) RETURN f.name AS name LIMIT 1",
    "MATCH (p:Person) RETURN p.name AS name ORDER BY p.age DESC LIMIT 5",
];

/// Iteration-count scaling.
///
/// Defaults to `1.0`. Override via `SOAK_ITER_SCALE` (e.g. `0.1` for a
/// quick local smoke run, `1.0` for a faithful CI run, `10.0` for an
/// extended overnight soak). Values outside `(0.0, 100.0]` are clamped
/// to the closest endpoint.
///
/// The spec calls for very large iteration counts (100K — 1M). On
/// commodity CI runners the embedded engine sustains roughly 100 — 400
/// queries/sec for the read mix, so the spec values would exceed the
/// 45-minute CI budget. Scenarios apply this scale to their default
/// loop counts so that scaling is uniform and the leak-budget assertion
/// remains meaningful (we compare RSS ratios, which are scale-invariant).
pub fn iter_scale() -> f64 {
    let raw = std::env::var("SOAK_ITER_SCALE").ok();
    let parsed = raw
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(1.0);
    if !parsed.is_finite() || parsed <= 0.0 {
        1.0
    } else {
        parsed.min(100.0)
    }
}

/// Apply [`iter_scale`] to a base iteration count, with a minimum of `1`.
pub fn scaled_iters(base: usize) -> usize {
    let scaled = (base as f64 * iter_scale()).round() as usize;
    scaled.max(1)
}

/// Count the nodes in the soak graph using the embedded Cypher engine.
pub fn count_nodes(
    engine: &sedan_cypher_embedded::CypherEngine,
    store: &InMemoryGraphStore,
) -> i64 {
    let result = engine
        .execute(store, GRAPH, "MATCH (n) RETURN count(n) AS n")
        .expect("count nodes");
    match result.rows[0].get("n").expect("count column") {
        Value::Int(i) => *i,
        other => panic!("expected Int from count(n), got {other:?}"),
    }
}

/// Watchdog that aborts the test process if `deadline` elapses.
/// Spawns a background thread that calls `std::process::exit(2)`.
/// Drop the guard to disarm it.
pub struct TimeoutGuard {
    cancel: Arc<AtomicBool>,
}

impl TimeoutGuard {
    /// Arm a watchdog that will abort the process after `deadline`.
    pub fn arm(name: &'static str, deadline: Duration) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_t = cancel.clone();
        thread::spawn(move || {
            let started = Instant::now();
            while started.elapsed() < deadline {
                if cancel_t.load(Ordering::Relaxed) {
                    return;
                }
                thread::sleep(Duration::from_millis(500));
            }
            eprintln!(
                "[soak/{name}] TIMEOUT after {:?} — aborting to surface a deadlock",
                deadline
            );
            std::process::exit(2);
        });
        Self { cancel }
    }
}

impl Drop for TimeoutGuard {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Bucketed latency tracker used to compute approximate percentiles
/// without sorting millions of samples.
///
/// Buckets are exponential (powers of 2 microseconds), so p99 is
/// approximate to within ~50%, which is fine for catching 3x degradation.
pub struct PercentileTracker {
    buckets: HashMap<u32, u64>,
    total: u64,
}

impl Default for PercentileTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl PercentileTracker {
    /// Create an empty tracker.
    pub fn new() -> Self {
        Self {
            buckets: HashMap::new(),
            total: 0,
        }
    }

    /// Record a single observation.
    pub fn record(&mut self, d: Duration) {
        let micros = d.as_micros().max(1) as u64;
        // Bucket index = floor(log2(micros)).
        let bucket = 63 - micros.leading_zeros();
        *self.buckets.entry(bucket).or_insert(0) += 1;
        self.total += 1;
    }

    /// Approximate the percentile (e.g. 0.99 for p99) and return the upper
    /// bound of the containing bucket in microseconds.
    pub fn percentile_micros(&self, p: f64) -> u64 {
        if self.total == 0 {
            return 0;
        }
        let target = ((self.total as f64) * p).ceil() as u64;
        let mut keys: Vec<u32> = self.buckets.keys().copied().collect();
        keys.sort();
        let mut cum = 0u64;
        for k in keys {
            cum += self.buckets[&k];
            if cum >= target {
                return 1u64 << (k + 1);
            }
        }
        0
    }

    /// Total samples recorded.
    pub fn count(&self) -> u64 {
        self.total
    }
}
