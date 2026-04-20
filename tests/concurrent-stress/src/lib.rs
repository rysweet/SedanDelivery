//! Concurrent stress harness for the SedanDelivery embedded engines.
//!
//! This crate exposes a small set of helpers that the per-engine integration
//! tests under `tests/` use to drive readers and writers against a single
//! shared `InMemoryGraphStore`. The goals are:
//!
//! * **No panics** — all worker threads must join cleanly.
//! * **No deadlocks** — every scenario terminates within its budget.
//! * **No data corruption** — for write-heavy scenarios the post-run node
//!   count equals the writer-emitted count.
//!
//! All write data is generated from a deterministic per-thread seed so
//! failures are reproducible.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use cypher_core::storage::{GraphStore, InMemoryGraphStore};

/// Graph name used by every scenario.
pub const GRAPH: &str = "g";

/// Default per-scenario duration. Four scenarios × three engines × this
/// budget keeps the default `cargo test` run comfortably under 60 s.
pub const SCENARIO_DURATION: Duration = Duration::from_secs(5);

/// Soak duration used by the `--ignored` nightly variant.
pub const SOAK_DURATION: Duration = Duration::from_secs(60);

/// Burst-test thread count.
pub const BURST_THREADS: usize = 32;

/// Build a freshly-seeded shared store with `nodes` `:Person` nodes.
///
/// Each node carries a deterministic `name` and `age` derived from its
/// index, so reads can assert validity without coordinating with writers.
pub fn seeded_store(nodes: usize) -> Arc<InMemoryGraphStore> {
    let store = InMemoryGraphStore::default();
    store.create_graph(GRAPH).expect("create_graph");
    for i in 0..nodes {
        let mut props = indexmap::IndexMap::new();
        props.insert(
            "name".into(),
            cypher_core::types::Value::Str(format!("seed_{i}").into()),
        );
        props.insert(
            "age".into(),
            cypher_core::types::Value::Int((i % 100) as i64),
        );
        store
            .create_node(GRAPH, vec!["Person".into()], props)
            .expect("seed create_node");
    }
    Arc::new(store)
}

/// Counters shared between all worker threads in a scenario.
#[derive(Default)]
pub struct StressCounters {
    pub reads: AtomicU64,
    pub writes: AtomicU64,
    pub read_errors: AtomicU64,
    pub write_errors: AtomicU64,
}

impl StressCounters {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }
}

/// Result of one scenario run: ops/sec and absolute counts.
#[derive(Debug)]
pub struct ScenarioReport {
    pub name: &'static str,
    pub elapsed: Duration,
    pub reads: u64,
    pub writes: u64,
    pub read_errors: u64,
    pub write_errors: u64,
}

impl ScenarioReport {
    pub fn read_ops_per_sec(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64().max(1e-9);
        self.reads as f64 / secs
    }
    pub fn write_ops_per_sec(&self) -> f64 {
        let secs = self.elapsed.as_secs_f64().max(1e-9);
        self.writes as f64 / secs
    }
}

/// Spawn `n_readers` reader threads and `n_writers` writer threads, run for
/// `duration`, then join. Returns the aggregated counters.
///
/// `read_op` and `write_op` are stateless closures invoked in a tight loop
/// while the stop flag is unset. They receive the shared store and the
/// thread's deterministic seed.
pub fn run_scenario<R, W>(
    name: &'static str,
    store: Arc<InMemoryGraphStore>,
    n_readers: usize,
    n_writers: usize,
    duration: Duration,
    read_op: R,
    write_op: W,
) -> ScenarioReport
where
    R: Fn(&InMemoryGraphStore, u64) -> Result<(), ()> + Send + Sync + 'static,
    W: Fn(&InMemoryGraphStore, u64, u64) -> Result<(), ()> + Send + Sync + 'static,
{
    let counters = StressCounters::new();
    let stop = Arc::new(AtomicBool::new(false));
    let read_op = Arc::new(read_op);
    let write_op = Arc::new(write_op);

    let started = Instant::now();
    let mut handles: Vec<JoinHandle<()>> = Vec::with_capacity(n_readers + n_writers);

    for tid in 0..n_readers {
        let store = Arc::clone(&store);
        let counters = Arc::clone(&counters);
        let stop = Arc::clone(&stop);
        let op = Arc::clone(&read_op);
        let seed = (tid as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        handles.push(
            thread::Builder::new()
                .name(format!("{name}-reader-{tid}"))
                .spawn(move || {
                    let mut local = seed;
                    while !stop.load(Ordering::Relaxed) {
                        match op(&store, local) {
                            Ok(()) => {
                                counters.reads.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(()) => {
                                counters.read_errors.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        local = local.wrapping_add(1);
                    }
                })
                .expect("spawn reader"),
        );
    }

    for tid in 0..n_writers {
        let store = Arc::clone(&store);
        let counters = Arc::clone(&counters);
        let stop = Arc::clone(&stop);
        let op = Arc::clone(&write_op);
        let seed = (tid as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        handles.push(
            thread::Builder::new()
                .name(format!("{name}-writer-{tid}"))
                .spawn(move || {
                    let mut counter: u64 = 0;
                    while !stop.load(Ordering::Relaxed) {
                        match op(&store, seed, counter) {
                            Ok(()) => {
                                counters.writes.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(()) => {
                                counters.write_errors.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        counter = counter.wrapping_add(1);
                    }
                })
                .expect("spawn writer"),
        );
    }

    thread::sleep(duration);
    stop.store(true, Ordering::Relaxed);

    for h in handles {
        // If a worker panicked, propagate so the test fails loudly.
        h.join().expect("worker thread panicked");
    }

    let elapsed = started.elapsed();
    ScenarioReport {
        name,
        elapsed,
        reads: counters.reads.load(Ordering::Relaxed),
        writes: counters.writes.load(Ordering::Relaxed),
        read_errors: counters.read_errors.load(Ordering::Relaxed),
        write_errors: counters.write_errors.load(Ordering::Relaxed),
    }
}

/// Burst scenario: spawn `n` threads that all execute exactly one query
/// (via `op`) and join. Asserts no panics.
pub fn run_burst<F>(name: &'static str, store: Arc<InMemoryGraphStore>, n: usize, op: F)
where
    F: Fn(&InMemoryGraphStore, u64) + Send + Sync + 'static,
{
    let op = Arc::new(op);
    let mut handles = Vec::with_capacity(n);
    for tid in 0..n {
        let store = Arc::clone(&store);
        let op = Arc::clone(&op);
        handles.push(
            thread::Builder::new()
                .name(format!("{name}-burst-{tid}"))
                .spawn(move || op(&store, tid as u64))
                .expect("spawn burst"),
        );
    }
    for h in handles {
        h.join().expect("burst thread panicked");
    }
}

/// Count `:Person` nodes in `graph` directly via the storage API. Used to
/// assert post-run consistency for write-heavy scenarios.
pub fn person_count(store: &InMemoryGraphStore) -> u64 {
    store
        .find_nodes_by_label(GRAPH, "Person")
        .expect("find_nodes_by_label")
        .len() as u64
}

/// Pretty-print a report line for the test stdout.
pub fn print_report(report: &ScenarioReport) {
    println!(
        "[stress] {:<24} elapsed={:>5.2}s reads={:>8} writes={:>8} \
         read_err={:>4} write_err={:>4} read_ops/s={:>10.1} write_ops/s={:>10.1}",
        report.name,
        report.elapsed.as_secs_f64(),
        report.reads,
        report.writes,
        report.read_errors,
        report.write_errors,
        report.read_ops_per_sec(),
        report.write_ops_per_sec(),
    );
}
