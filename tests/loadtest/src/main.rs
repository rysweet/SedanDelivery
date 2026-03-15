//! Load testing tool for SedanDelivery Bolt endpoint.
//!
//! Validates the system under real scale with three scenarios:
//! - `nodes`: Bulk node creation with throughput and latency reporting
//! - `ramp`: Connection ramp from 10 to max with failure tracking
//! - `mixed`: Configurable read/write mix under sustained load

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use neo4rs::{ConfigBuilder, Graph};
use serde::Serialize;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(name = "loadtest", about = "SedanDelivery load testing tool")]
struct Cli {
    /// Bolt URI of the SedanDelivery server
    #[arg(long, default_value = "bolt://localhost:7687")]
    bolt_uri: String,

    /// Optional PEM CA certificate to trust for Bolt TLS
    #[arg(long)]
    ca_cert: Option<PathBuf>,

    /// Database user (Bolt auth)
    #[arg(long, default_value = "")]
    user: String,

    /// Database password (Bolt auth)
    #[arg(long, default_value = "")]
    password: String,

    /// Which scenario to run
    #[arg(long)]
    scenario: Scenario,

    /// Total number of items (nodes scenario: node count)
    #[arg(long, default_value_t = 100_000)]
    count: u64,

    /// Number of concurrent connections / tasks
    #[arg(long, default_value_t = 50)]
    concurrency: usize,

    /// Max connections for ramp scenario
    #[arg(long, default_value_t = 2000)]
    max_connections: usize,

    /// Duration in seconds (ramp and mixed scenarios)
    #[arg(long, default_value_t = 60)]
    duration: u64,

    /// Read percentage for mixed scenario (0-100)
    #[arg(long, default_value_t = 80)]
    ratio: u8,

    /// Graph name to use for testing
    #[arg(long, default_value = "loadtest")]
    graph: String,

    /// Output format
    #[arg(long, default_value = "json")]
    format: OutputFormat,

    /// Skip dropping the test graph after printing results.
    #[arg(long)]
    skip_cleanup: bool,

    /// Minimum required throughput in nodes/sec for the nodes scenario.
    #[arg(long)]
    min_throughput: Option<f64>,

    /// Maximum allowed p99 latency in milliseconds.
    #[arg(long)]
    max_p99_ms: Option<f64>,

    /// Maximum allowed error rate as a fraction in [0, 1].
    #[arg(long)]
    max_error_rate: Option<f64>,
}

#[derive(Debug, Clone, ValueEnum)]
enum Scenario {
    Nodes,
    Ramp,
    Mixed,
}

#[derive(Debug, Clone, ValueEnum)]
enum OutputFormat {
    Json,
    Table,
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct LatencyStats {
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    min_ms: f64,
    max_ms: f64,
    mean_ms: f64,
}

#[derive(Debug, Serialize)]
struct NodesResult {
    scenario: String,
    total_nodes: u64,
    created_nodes: u64,
    verified_count: u64,
    count_matches: bool,
    elapsed_secs: f64,
    throughput_nodes_per_sec: f64,
    concurrency: usize,
    latency: LatencyStats,
    errors: u64,
}

#[derive(Debug, Serialize)]
struct RampLevel {
    target_connections: usize,
    established: usize,
    failures: usize,
    accept_latency_mean_ms: f64,
    accept_latency_p99_ms: f64,
}

#[derive(Debug, Serialize)]
struct RampResult {
    scenario: String,
    bolt_max_connections: usize,
    levels: Vec<RampLevel>,
    zero_failures_below_max: bool,
    elapsed_secs: f64,
}

#[derive(Debug, Serialize)]
struct MixedResult {
    scenario: String,
    duration_secs: f64,
    concurrency: usize,
    read_ratio: u8,
    total_reads: u64,
    total_writes: u64,
    total_ops: u64,
    total_ops_per_sec: f64,
    reads_per_sec: f64,
    writes_per_sec: f64,
    read_errors: u64,
    write_errors: u64,
    backend_errors: u64,
    read_latency: LatencyStats,
    write_latency: LatencyStats,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn compute_latency(mut samples: Vec<f64>) -> LatencyStats {
    if samples.is_empty() {
        return LatencyStats {
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            min_ms: 0.0,
            max_ms: 0.0,
            mean_ms: 0.0,
        };
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = samples.len();
    let sum: f64 = samples.iter().sum();
    LatencyStats {
        p50_ms: samples[n * 50 / 100],
        p95_ms: samples[n * 95 / 100],
        p99_ms: samples[(n * 99 / 100).min(n - 1)],
        min_ms: samples[0],
        max_ms: samples[n - 1],
        mean_ms: sum / n as f64,
    }
}

async fn connect(
    uri: &str,
    user: &str,
    password: &str,
    ca_cert: Option<&PathBuf>,
) -> Result<Graph, neo4rs::Error> {
    let u = if user.is_empty() { "neo4j" } else { user };
    let p = if password.is_empty() {
        "neo4j"
    } else {
        password
    };
    let mut builder = ConfigBuilder::default()
        .uri(uri)
        .user(u)
        .password(p)
        .fetch_size(1)
        .max_connections(1);
    if let Some(ca_cert) = ca_cert {
        builder = builder.with_client_certificate(ca_cert);
    }
    Graph::connect(builder.build()?).await
}

async fn connect_pool(
    uri: &str,
    user: &str,
    password: &str,
    ca_cert: Option<&PathBuf>,
    pool_size: usize,
) -> Result<Graph, neo4rs::Error> {
    let u = if user.is_empty() { "neo4j" } else { user };
    let p = if password.is_empty() {
        "neo4j"
    } else {
        password
    };
    let mut builder = ConfigBuilder::default()
        .uri(uri)
        .user(u)
        .password(p)
        .fetch_size(256)
        .max_connections(pool_size);
    if let Some(ca_cert) = ca_cert {
        builder = builder.with_client_certificate(ca_cert);
    }
    Graph::connect(builder.build()?).await
}

fn quoted_identifier(identifier: &str) -> String {
    format!("`{}`", identifier.replace('`', "``"))
}

async fn recreate_graph(graph: &Graph, name: &str) -> Result<(), neo4rs::Error> {
    let _ = graph
        .run(neo4rs::query(&format!(
            "DROP GRAPH {}",
            quoted_identifier(name)
        )))
        .await;
    graph
        .run(neo4rs::query(&format!(
            "CREATE GRAPH {}",
            quoted_identifier(name)
        )))
        .await
}

async fn cleanup_graph(graph: &Graph, name: &str) {
    let _ = graph
        .run(neo4rs::query(&format!(
            "DROP GRAPH {}",
            quoted_identifier(name)
        )))
        .await;
}

async fn maybe_cleanup_graph(cli: &Cli, graph: &Graph) {
    if cli.skip_cleanup {
        tracing::info!(graph = %cli.graph, "Skipping graph cleanup");
        return;
    }

    cleanup_graph(graph, &cli.graph).await;
}

fn print_result<T: Serialize>(result: &T, format: &OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(result).unwrap());
        }
        OutputFormat::Table => {
            // Simple key-value table from JSON
            let val = serde_json::to_value(result).unwrap();
            print_table_value(&val, 0);
        }
    }
}

fn print_table_value(val: &serde_json::Value, indent: usize) {
    let pad = " ".repeat(indent);
    match val {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                match v {
                    serde_json::Value::Object(_) | serde_json::Value::Array(_) => {
                        println!("{pad}{k}:");
                        print_table_value(v, indent + 2);
                    }
                    _ => println!("{pad}{:<30} {}", k, v),
                }
            }
        }
        serde_json::Value::Array(arr) => {
            for (i, v) in arr.iter().enumerate() {
                println!("{pad}[{i}]:");
                print_table_value(v, indent + 2);
            }
        }
        other => println!("{pad}{other}"),
    }
}

// ---------------------------------------------------------------------------
// Scenario 1: Node Creation Scale
// ---------------------------------------------------------------------------

async fn run_nodes(cli: &Cli) -> Result<NodesResult, Box<dyn std::error::Error>> {
    tracing::info!(
        count = cli.count,
        concurrency = cli.concurrency,
        "Starting node creation scenario"
    );
    if cli.concurrency > 1 {
        tracing::warn!(
            concurrency = cli.concurrency,
            "nodes scenario targets one graph, so writes serialize on the graph write lock; higher concurrency measures contention rather than steady-state throughput"
        );
    }

    let graph = connect_pool(
        &cli.bolt_uri,
        &cli.user,
        &cli.password,
        cli.ca_cert.as_ref(),
        cli.concurrency,
    )
    .await?;
    recreate_graph(&graph, &cli.graph).await?;

    let total = cli.count;
    let sem = Arc::new(Semaphore::new(cli.concurrency));
    let errors = Arc::new(AtomicU64::new(0));
    let completed = Arc::new(AtomicU64::new(0));

    // Collect latencies via a channel
    let (lat_tx, mut lat_rx) = tokio::sync::mpsc::unbounded_channel::<f64>();

    let start = Instant::now();
    let mut set = JoinSet::new();

    // Batch nodes to reduce per-query overhead: 100 nodes per UNWIND
    let batch_size: u64 = 100;
    let num_batches = total.div_ceil(batch_size);

    for batch_idx in 0..num_batches {
        let graph = graph.clone();
        let sem = sem.clone();
        let errors = errors.clone();
        let completed = completed.clone();
        let lat_tx = lat_tx.clone();
        let graph_name = cli.graph.clone();
        let nodes_in_batch = std::cmp::min(batch_size, total - batch_idx * batch_size);

        set.spawn(async move {
            let _permit = sem.acquire().await.unwrap();
            let q = format!(
                "UNWIND range(1, {}) AS i CREATE (:LoadTest {{batch: {}, idx: i}})",
                nodes_in_batch, batch_idx
            );
            let t0 = Instant::now();
            match graph.run_on(&graph_name, neo4rs::query(&q)).await {
                Ok(_) => {
                    let lat = t0.elapsed().as_secs_f64() * 1000.0;
                    let _ = lat_tx.send(lat);
                    completed.fetch_add(nodes_in_batch, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::warn!(batch = batch_idx, error = %e, "Batch failed");
                    errors.fetch_add(nodes_in_batch, Ordering::Relaxed);
                }
            }
        });
    }

    // Drop sender so receiver finishes
    drop(lat_tx);

    // Collect latencies while tasks run
    let lat_handle = tokio::spawn(async move {
        let mut samples = Vec::new();
        while let Some(lat) = lat_rx.recv().await {
            samples.push(lat);
        }
        samples
    });

    // Wait for all tasks
    while set.join_next().await.is_some() {}

    let elapsed = start.elapsed();
    let latencies = lat_handle.await?;

    // Verify count
    let mut result_stream = graph
        .execute_on(
            &cli.graph,
            neo4rs::query("MATCH (n:LoadTest) RETURN count(n) AS cnt"),
        )
        .await?;
    let verified_count = if let Some(row) = result_stream.next().await? {
        row.get::<i64>("cnt").unwrap_or(0) as u64
    } else {
        0
    };

    let created = completed.load(Ordering::Relaxed);
    let err_count = errors.load(Ordering::Relaxed);

    let result = NodesResult {
        scenario: "nodes".to_string(),
        total_nodes: total,
        created_nodes: created,
        verified_count,
        count_matches: verified_count == created,
        elapsed_secs: elapsed.as_secs_f64(),
        throughput_nodes_per_sec: created as f64 / elapsed.as_secs_f64(),
        concurrency: cli.concurrency,
        latency: compute_latency(latencies),
        errors: err_count,
    };

    print_result(&result, &cli.format);

    maybe_cleanup_graph(cli, &graph).await;

    Ok(result)
}

// ---------------------------------------------------------------------------
// Scenario 2: Connection Ramp
// ---------------------------------------------------------------------------

async fn run_ramp(cli: &Cli) -> Result<RampResult, Box<dyn std::error::Error>> {
    tracing::info!(
        max_connections = cli.max_connections,
        duration = cli.duration,
        "Starting connection ramp scenario"
    );

    // Generate ramp levels: 10, 50, 100, 200, 400, 600, 800, 1024, 1200, 1500, 2000
    let mut targets: Vec<usize> = vec![10, 50, 100, 200, 400, 600, 800];
    // Add BOLT_MAX_CONNECTIONS boundary (assumed 1024 unless overridden)
    let bolt_max: usize = 1024;
    if !targets.contains(&bolt_max) {
        targets.push(bolt_max);
    }
    // Add levels above bolt_max up to max_connections
    let above = [1200, 1500, 2000, 3000, 4000, 5000];
    for &lvl in &above {
        if lvl <= cli.max_connections && lvl > bolt_max {
            targets.push(lvl);
        }
    }
    if !targets.contains(&cli.max_connections)
        && cli.max_connections > *targets.last().unwrap_or(&0)
    {
        targets.push(cli.max_connections);
    }
    targets.sort();
    targets.dedup();

    let total_levels = targets.len();
    let step_duration = Duration::from_secs(cli.duration) / total_levels as u32;

    let start = Instant::now();
    let mut levels = Vec::new();

    for &target in &targets {
        let level_start = Instant::now();
        let mut established = 0usize;
        let mut failures = 0usize;
        let mut accept_latencies = Vec::new();

        // Try to open `target` connections concurrently
        let mut set = JoinSet::new();
        for _ in 0..target {
            let uri = cli.bolt_uri.clone();
            let user = cli.user.clone();
            let password = cli.password.clone();
            let ca_cert = cli.ca_cert.clone();
            set.spawn(async move {
                let t0 = Instant::now();
                let result = connect(&uri, &user, &password, ca_cert.as_ref()).await;
                let lat = t0.elapsed().as_secs_f64() * 1000.0;
                match result {
                    Ok(g) => {
                        // Execute a simple query to confirm the connection works
                        let qr = g.run(neo4rs::query("RETURN 1")).await;
                        match qr {
                            Ok(_) => Ok(lat),
                            Err(_) => Err(lat),
                        }
                    }
                    Err(_) => Err(lat),
                }
            });
        }

        while let Some(join_result) = set.join_next().await {
            match join_result {
                Ok(Ok(lat)) => {
                    established += 1;
                    accept_latencies.push(lat);
                }
                Ok(Err(_lat)) => {
                    failures += 1;
                }
                Err(_) => {
                    failures += 1;
                }
            }
        }

        let avg_latency = if accept_latencies.is_empty() {
            0.0
        } else {
            accept_latencies.iter().sum::<f64>() / accept_latencies.len() as f64
        };
        let accept_latency = compute_latency(accept_latencies);

        tracing::info!(
            target,
            established,
            failures,
            avg_latency_ms = format!("{:.1}", avg_latency),
            p99_latency_ms = format!("{:.1}", accept_latency.p99_ms),
            "Ramp level complete"
        );

        levels.push(RampLevel {
            target_connections: target,
            established,
            failures,
            accept_latency_mean_ms: avg_latency,
            accept_latency_p99_ms: accept_latency.p99_ms,
        });

        // Pace to match the requested duration spread
        let level_elapsed = level_start.elapsed();
        if level_elapsed < step_duration {
            tokio::time::sleep(step_duration - level_elapsed).await;
        }
    }

    let elapsed = start.elapsed();

    // Check: zero failures for all levels <= bolt_max
    let zero_failures_below_max = levels
        .iter()
        .filter(|l| l.target_connections <= bolt_max)
        .all(|l| l.failures == 0);

    let result = RampResult {
        scenario: "ramp".to_string(),
        bolt_max_connections: bolt_max,
        levels,
        zero_failures_below_max,
        elapsed_secs: elapsed.as_secs_f64(),
    };

    print_result(&result, &cli.format);

    Ok(result)
}

// ---------------------------------------------------------------------------
// Scenario 3: Read/Write Mix
// ---------------------------------------------------------------------------

async fn run_mixed(cli: &Cli) -> Result<MixedResult, Box<dyn std::error::Error>> {
    tracing::info!(
        ratio = cli.ratio,
        concurrency = cli.concurrency,
        duration = cli.duration,
        "Starting mixed workload scenario"
    );

    let graph = connect_pool(
        &cli.bolt_uri,
        &cli.user,
        &cli.password,
        cli.ca_cert.as_ref(),
        cli.concurrency,
    )
    .await?;
    recreate_graph(&graph, &cli.graph).await?;

    // Seed some data for reads
    graph
        .run_on(
            &cli.graph,
            neo4rs::query("UNWIND range(1, 1000) AS i CREATE (:MixSeed {id: i})"),
        )
        .await?;

    let duration = Duration::from_secs(cli.duration);
    let read_ratio = cli.ratio;

    let total_reads = Arc::new(AtomicU64::new(0));
    let total_writes = Arc::new(AtomicU64::new(0));
    let read_errors = Arc::new(AtomicU64::new(0));
    let write_errors = Arc::new(AtomicU64::new(0));
    let backend_errors = Arc::new(AtomicU64::new(0));
    let global_op_counter = Arc::new(AtomicU64::new(0));

    let (read_lat_tx, mut read_lat_rx) = tokio::sync::mpsc::unbounded_channel::<f64>();
    let (write_lat_tx, mut write_lat_rx) = tokio::sync::mpsc::unbounded_channel::<f64>();

    let start = Instant::now();
    let sem = Arc::new(Semaphore::new(cli.concurrency));
    let mut set = JoinSet::new();

    // Spawn worker tasks
    let num_workers = cli.concurrency;
    for worker_id in 0..num_workers {
        let graph = graph.clone();
        let sem = sem.clone();
        let total_reads = total_reads.clone();
        let total_writes = total_writes.clone();
        let read_errors = read_errors.clone();
        let write_errors = write_errors.clone();
        let backend_errors = backend_errors.clone();
        let global_op_counter = global_op_counter.clone();
        let read_lat_tx = read_lat_tx.clone();
        let write_lat_tx = write_lat_tx.clone();
        let graph_name = cli.graph.clone();

        set.spawn(async move {
            while start.elapsed() < duration {
                let _permit = sem.acquire().await.unwrap();
                let op_id = global_op_counter.fetch_add(1, Ordering::Relaxed);
                let is_read = (op_id % 100) < read_ratio as u64;

                let t0 = Instant::now();
                if is_read {
                    let id = (op_id % 1000) + 1;
                    let q = format!("MATCH (n:MixSeed {{id: {id}}}) RETURN n.id");
                    match graph.run_on(&graph_name, neo4rs::query(&q)).await {
                        Ok(_) => {
                            total_reads.fetch_add(1, Ordering::Relaxed);
                            let _ = read_lat_tx.send(t0.elapsed().as_secs_f64() * 1000.0);
                        }
                        Err(e) => {
                            read_errors.fetch_add(1, Ordering::Relaxed);
                            if is_backend_error(&e) {
                                backend_errors.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                } else {
                    let q = format!(
                        "CREATE (:MixWrite {{worker: {}, seq: {}}})",
                        worker_id, op_id
                    );
                    match graph.run_on(&graph_name, neo4rs::query(&q)).await {
                        Ok(_) => {
                            total_writes.fetch_add(1, Ordering::Relaxed);
                            let _ = write_lat_tx.send(t0.elapsed().as_secs_f64() * 1000.0);
                        }
                        Err(e) => {
                            write_errors.fetch_add(1, Ordering::Relaxed);
                            if is_backend_error(&e) {
                                backend_errors.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    }
                }
            }
        });
    }

    drop(read_lat_tx);
    drop(write_lat_tx);

    let read_lat_handle = tokio::spawn(async move {
        let mut samples = Vec::new();
        while let Some(lat) = read_lat_rx.recv().await {
            samples.push(lat);
        }
        samples
    });
    let write_lat_handle = tokio::spawn(async move {
        let mut samples = Vec::new();
        while let Some(lat) = write_lat_rx.recv().await {
            samples.push(lat);
        }
        samples
    });

    while set.join_next().await.is_some() {}

    let read_latencies = read_lat_handle.await?;
    let write_latencies = write_lat_handle.await?;

    let duration_secs = start.elapsed().as_secs_f64();
    let total_reads = total_reads.load(Ordering::Relaxed);
    let total_writes = total_writes.load(Ordering::Relaxed);
    let total_ops = total_reads + total_writes;
    let result = MixedResult {
        scenario: "mixed".to_string(),
        duration_secs,
        concurrency: cli.concurrency,
        read_ratio: cli.ratio,
        total_reads,
        total_writes,
        total_ops,
        total_ops_per_sec: total_ops as f64 / duration_secs,
        reads_per_sec: total_reads as f64 / duration_secs,
        writes_per_sec: total_writes as f64 / duration_secs,
        read_errors: read_errors.load(Ordering::Relaxed),
        write_errors: write_errors.load(Ordering::Relaxed),
        backend_errors: backend_errors.load(Ordering::Relaxed),
        read_latency: compute_latency(read_latencies),
        write_latency: compute_latency(write_latencies),
    };

    print_result(&result, &cli.format);

    maybe_cleanup_graph(cli, &graph).await;

    Ok(result)
}

/// Heuristic: check if a neo4rs error looks like a StorageError::Backend.
/// neo4rs surfaces server errors as string messages, so we pattern-match on
/// common backend-error indicators.
fn is_backend_error(e: &neo4rs::Error) -> bool {
    let msg = format!("{:?}", e);
    msg.contains("Backend")
        || msg.contains("StorageError")
        || msg.contains("storage backend")
        || msg.contains("Redis")
}

struct ThresholdViolation {
    metric: String,
    limit: f64,
    actual: f64,
}

impl ThresholdViolation {
    fn new(metric: &str, limit: f64, actual: f64) -> Self {
        Self {
            metric: metric.to_string(),
            limit,
            actual,
        }
    }
}

fn check_nodes_thresholds(result: &NodesResult, cli: &Cli) -> Vec<ThresholdViolation> {
    let mut violations = Vec::new();

    if let Some(min_throughput) = cli.min_throughput {
        if result.throughput_nodes_per_sec < min_throughput {
            violations.push(ThresholdViolation::new(
                "throughput_nodes_per_sec",
                min_throughput,
                result.throughput_nodes_per_sec,
            ));
        }
    }

    if let Some(max_p99_ms) = cli.max_p99_ms {
        if result.latency.p99_ms > max_p99_ms {
            violations.push(ThresholdViolation::new(
                "latency.p99_ms",
                max_p99_ms,
                result.latency.p99_ms,
            ));
        }
    }

    if let Some(max_error_rate) = cli.max_error_rate {
        if result.total_nodes > 0 {
            let error_rate = result.errors as f64 / result.total_nodes as f64;
            if error_rate > max_error_rate {
                violations.push(ThresholdViolation::new(
                    "error_rate",
                    max_error_rate,
                    error_rate,
                ));
            }
        }
    }

    if !result.count_matches {
        violations.push(ThresholdViolation::new(
            "count_integrity",
            result.created_nodes as f64,
            result.verified_count as f64,
        ));
    }

    violations
}

fn check_ramp_thresholds(result: &RampResult, cli: &Cli) -> Vec<ThresholdViolation> {
    let mut violations = Vec::new();

    if !result.zero_failures_below_max {
        let failures_below_max: usize = result
            .levels
            .iter()
            .filter(|level| level.target_connections <= result.bolt_max_connections)
            .map(|level| level.failures)
            .sum();
        violations.push(ThresholdViolation::new(
            "zero_failures_below_bolt_max",
            0.0,
            failures_below_max as f64,
        ));
    }

    if let Some(max_p99_ms) = cli.max_p99_ms {
        for level in result
            .levels
            .iter()
            .filter(|level| level.target_connections <= result.bolt_max_connections)
        {
            if level.accept_latency_p99_ms > max_p99_ms {
                violations.push(ThresholdViolation::new(
                    &format!("ramp.accept_latency_p99_ms[{}]", level.target_connections),
                    max_p99_ms,
                    level.accept_latency_p99_ms,
                ));
            }
        }
    }

    violations
}

fn check_mixed_thresholds(result: &MixedResult, cli: &Cli) -> Vec<ThresholdViolation> {
    let mut violations = Vec::new();

    if let Some(max_p99_ms) = cli.max_p99_ms {
        if result.read_latency.p99_ms > max_p99_ms {
            violations.push(ThresholdViolation::new(
                "read_latency.p99_ms",
                max_p99_ms,
                result.read_latency.p99_ms,
            ));
        }
        if result.write_latency.p99_ms > max_p99_ms {
            violations.push(ThresholdViolation::new(
                "write_latency.p99_ms",
                max_p99_ms,
                result.write_latency.p99_ms,
            ));
        }
    }

    if let Some(max_error_rate) = cli.max_error_rate {
        let total_ops =
            result.total_reads + result.total_writes + result.read_errors + result.write_errors;
        if total_ops > 0 {
            let backend_error_rate = result.backend_errors as f64 / total_ops as f64;
            if backend_error_rate > max_error_rate {
                violations.push(ThresholdViolation::new(
                    "backend_error_rate",
                    max_error_rate,
                    backend_error_rate,
                ));
            }
        }
    }

    violations
}

fn report_violations(violations: &[ThresholdViolation]) -> bool {
    if violations.is_empty() {
        tracing::info!("All threshold assertions passed");
        return true;
    }

    for violation in violations {
        tracing::error!(
            metric = %violation.metric,
            limit = violation.limit,
            actual = violation.actual,
            "THRESHOLD VIOLATION: {} exceeded limit {:.4} (actual: {:.4})",
            violation.metric,
            violation.limit,
            violation.actual
        );
        eprintln!(
            "THRESHOLD VIOLATION: {} limit={:.4} actual={:.4}",
            violation.metric, violation.limit, violation.actual
        );
    }

    false
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    tracing::info!(
        bolt_uri = %cli.bolt_uri,
        scenario = ?cli.scenario,
        "SedanDelivery Load Test starting"
    );

    let result = match cli.scenario {
        Scenario::Nodes => run_nodes_with_assertions(&cli).await,
        Scenario::Ramp => run_ramp_with_assertions(&cli).await,
        Scenario::Mixed => run_mixed_with_assertions(&cli).await,
    };

    if let Err(e) = result {
        tracing::error!(error = %e, "Scenario failed");
        std::process::exit(1);
    }
}

async fn run_nodes_with_assertions(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let result = run_nodes(cli).await?;
    if !report_violations(&check_nodes_thresholds(&result, cli)) {
        std::process::exit(2);
    }
    Ok(())
}

async fn run_ramp_with_assertions(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let result = run_ramp(cli).await?;
    if !report_violations(&check_ramp_thresholds(&result, cli)) {
        std::process::exit(2);
    }
    Ok(())
}

async fn run_mixed_with_assertions(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let result = run_mixed(cli).await?;
    if !report_violations(&check_mixed_thresholds(&result, cli)) {
        std::process::exit(2);
    }
    Ok(())
}
