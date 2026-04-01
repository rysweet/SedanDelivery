// Benchmark safety-tax paths on the Redis-backed store.
//
// By default these benches refuse to target non-local Redis endpoints because
// they create and delete graphs repeatedly. Set STORE_SAFETY_ALLOW_REMOTE=1 if
// you intentionally want to run them against a remote environment.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use cypher_core::storage::GraphStore;
use cypher_core::types::Value;
use indexmap::IndexMap;
use sd2_redis_store::RedisGraphStore;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const INDEX_SMALL_NODE_COUNT: usize = 1_000;
const INDEX_SCAN_NODE_COUNT: usize = 12_000;
const DELETE_GRAPH_NODE_COUNT: usize = 2_000;
const BATCH_SEED_SIZE: usize = 250;

static GRAPH_COUNTER: AtomicU64 = AtomicU64::new(1);

fn local_host_only(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn redis_url_host(url: &str) -> Option<&str> {
    let without_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    if host_port.is_empty() {
        return None;
    }
    if let Some(remainder) = host_port.strip_prefix('[') {
        return remainder.split_once(']').map(|(host, _)| host);
    }
    host_port.split(':').next()
}

fn allow_remote_targets() -> bool {
    matches!(
        std::env::var("STORE_SAFETY_ALLOW_REMOTE").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

fn ensure_local_urls(source: &str, urls: &[&str]) -> Result<(), String> {
    if allow_remote_targets() {
        return Ok(());
    }
    for url in urls {
        let host = redis_url_host(url)
            .ok_or_else(|| format!("could not parse host from {source} value {url:?}"))?;
        if !local_host_only(host) {
            return Err(format!(
                "{source} points at non-local host {host:?}; refusing destructive store_safety benchmark run. \
                 Set STORE_SAFETY_ALLOW_REMOTE=1 to override deliberately."
            ));
        }
    }
    Ok(())
}

fn setup_store() -> Result<(RedisGraphStore, String), String> {
    if let Ok(cluster_nodes) = std::env::var("GQL_REDIS_CLUSTER_NODES") {
        let nodes: Vec<&str> = cluster_nodes
            .split(',')
            .map(str::trim)
            .filter(|node| !node.is_empty())
            .collect();
        if nodes.is_empty() {
            return Err(
                "GQL_REDIS_CLUSTER_NODES is set but contains no valid node URLs after parsing"
                    .to_string(),
            );
        }
        ensure_local_urls("GQL_REDIS_CLUSTER_NODES", &nodes)?;
        let store = RedisGraphStore::from_env()
            .map_err(|err| format!("failed to connect to cluster Redis target: {err:?}"))?;
        return Ok((store, "cluster".to_string()));
    }

    let url = std::env::var("GQL_REDIS_URL")
        .or_else(|_| std::env::var("REDIS_URL"))
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    ensure_local_urls("GQL_REDIS_URL/REDIS_URL", &[&url])?;
    let store = RedisGraphStore::from_env()
        .map_err(|err| format!("failed to connect to standalone Redis target: {err:?}"))?;
    Ok((store, "standalone".to_string()))
}

fn unique_graph(prefix: &str) -> String {
    let id = GRAPH_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}_{id}")
}

fn labels(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

fn node_props(i: usize) -> IndexMap<String, Value> {
    IndexMap::from([
        ("index".to_string(), Value::Int(i as i64)),
        ("name".to_string(), Value::Str(format!("node_{i}").into())),
        ("partition".to_string(), Value::Int((i % 32) as i64)),
    ])
}

fn make_batched_seed_node(i: usize) -> (Vec<String>, IndexMap<String, Value>) {
    (
        labels(&["BenchIndexed", "BenchRegistry"]),
        IndexMap::from([
            ("index".to_string(), Value::Int(i as i64)),
            ("name".to_string(), Value::Str(format!("seed_{i}").into())),
            ("bucket".to_string(), Value::Int((i % 64) as i64)),
        ]),
    )
}

fn seed_index_graph(store: &RedisGraphStore, graph: &str, count: usize) {
    let nodes: Vec<_> = (0..count).map(make_batched_seed_node).collect();
    for chunk in nodes.chunks(BATCH_SEED_SIZE) {
        store.batch_create_nodes(graph, chunk.to_vec()).unwrap();
    }
}

fn seed_delete_graph(store: &RedisGraphStore, graph: &str, count: usize) {
    let nodes: Vec<_> = (0..count).map(make_batched_seed_node).collect();
    let mut node_ids = Vec::with_capacity(nodes.len());
    for chunk in nodes.chunks(BATCH_SEED_SIZE) {
        node_ids.extend(store.batch_create_nodes(graph, chunk.to_vec()).unwrap());
    }
    store.create_index(graph, "BenchIndexed", "name").unwrap();
    for edge in node_ids.windows(2) {
        store
            .create_edge(
                graph,
                edge[0],
                edge[1],
                "LINKS_TO".to_string(),
                IndexMap::new(),
            )
            .unwrap();
    }
}

fn bench_lock_and_registry_paths(c: &mut Criterion) {
    let (store, topology) = match setup_store() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Skipping store_safety benchmarks: {err}");
            return;
        }
    };

    let mut group = c.benchmark_group("store_safety_lock");
    group.sample_size(10);
    group.throughput(Throughput::Elements(1));
    group.bench_function(BenchmarkId::new("create_graph", topology.clone()), |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::default();
            let mut created_graphs = Vec::with_capacity(iters as usize);
            for _ in 0..iters {
                let graph = unique_graph("bench_create_graph");
                let start = Instant::now();
                store.create_graph(&graph).unwrap();
                elapsed += start.elapsed();
                created_graphs.push(graph);
            }
            for graph in created_graphs {
                let _ = store.delete_graph(&graph);
            }
            elapsed
        });
    });

    group.bench_function(
        BenchmarkId::new("create_node_single_label", topology.clone()),
        |b| {
            b.iter_custom(|iters| {
                let mut elapsed = Duration::default();
                for i in 0..iters {
                    let graph = unique_graph("bench_node_single");
                    store.create_graph(&graph).unwrap();
                    let start = Instant::now();
                    store
                        .create_node(&graph, labels(&["BenchNode"]), node_props(i as usize))
                        .unwrap();
                    elapsed += start.elapsed();
                    let _ = store.delete_graph(&graph);
                }
                elapsed
            });
        },
    );

    group.bench_function(BenchmarkId::new("create_node_multi_label", topology), |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::default();
            for i in 0..iters {
                let graph = unique_graph("bench_node_multi");
                store.create_graph(&graph).unwrap();
                let start = Instant::now();
                store
                    .create_node(
                        &graph,
                        labels(&["BenchNode", "BenchHotPath", "BenchRegistry"]),
                        node_props(i as usize),
                    )
                    .unwrap();
                elapsed += start.elapsed();
                let _ = store.delete_graph(&graph);
            }
            elapsed
        });
    });
    group.finish();
}

fn bench_index_paths(c: &mut Criterion) {
    let (store, topology) = match setup_store() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Skipping store_safety benchmarks: {err}");
            return;
        }
    };

    let mut group = c.benchmark_group("store_safety_index");
    group.sample_size(10);
    for &node_count in &[INDEX_SMALL_NODE_COUNT, INDEX_SCAN_NODE_COUNT] {
        group.throughput(Throughput::Elements(node_count as u64));
        group.bench_with_input(
            BenchmarkId::new(topology.clone(), node_count),
            &node_count,
            |b, &node_count| {
                b.iter_custom(|iters| {
                    let mut elapsed = Duration::default();
                    for _ in 0..iters {
                        let graph = unique_graph("bench_create_index");
                        store.create_graph(&graph).unwrap();
                        seed_index_graph(&store, &graph, node_count);
                        let start = Instant::now();
                        store.create_index(&graph, "BenchIndexed", "name").unwrap();
                        elapsed += start.elapsed();
                        let _ = store.delete_graph(&graph);
                    }
                    elapsed
                });
            },
        );
    }
    group.finish();
}

fn bench_delete_graph_paths(c: &mut Criterion) {
    let (store, topology) = match setup_store() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("Skipping store_safety benchmarks: {err}");
            return;
        }
    };

    let mut group = c.benchmark_group("store_safety_delete");
    group.sample_size(10);
    group.throughput(Throughput::Elements(DELETE_GRAPH_NODE_COUNT as u64));
    group.bench_function(BenchmarkId::new("delete_graph", topology), |b| {
        b.iter_custom(|iters| {
            let mut elapsed = Duration::default();
            for _ in 0..iters {
                let graph = unique_graph("bench_delete_graph");
                store.create_graph(&graph).unwrap();
                seed_delete_graph(&store, &graph, DELETE_GRAPH_NODE_COUNT);
                let start = Instant::now();
                store.delete_graph(&graph).unwrap();
                elapsed += start.elapsed();
            }
            elapsed
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_lock_and_registry_paths,
    bench_index_paths,
    bench_delete_graph_paths,
);
criterion_main!(benches);
