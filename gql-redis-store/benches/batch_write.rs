// Benchmark: single-write vs batched node creation on Redis.
//
// Requires a running Redis instance. Set `GQL_REDIS_URL` (default redis://127.0.0.1:6379).
// Run with: cargo bench -p gql-redis-store --bench batch_write

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use gql_core::storage::GraphStore;
use gql_core::types::Value;
use gql_redis_store::RedisGraphStore;
use std::collections::HashMap;

const NODE_COUNT: usize = 1_000;

fn make_node(i: usize) -> (Vec<String>, HashMap<String, Value>) {
    let labels = vec!["BenchNode".to_string()];
    let props = HashMap::from([
        ("index".to_string(), Value::Int(i as i64)),
        ("name".to_string(), Value::Str(format!("node_{i}").into())),
    ]);
    (labels, props)
}

fn setup_store() -> Option<RedisGraphStore> {
    let url = std::env::var("GQL_REDIS_URL")
        .or_else(|_| std::env::var("REDIS_URL"))
        .unwrap_or_else(|_| "redis://127.0.0.1:6379".into());
    RedisGraphStore::from_url(&url).ok()
}

fn ensure_graph(store: &RedisGraphStore, name: &str) {
    // Delete if exists, then create fresh.
    let _ = store.delete_graph(name);
    store.create_graph(name).unwrap();
}

fn bench_single_vs_batch(c: &mut Criterion) {
    let store = match setup_store() {
        Some(s) => s,
        None => {
            eprintln!("Skipping Redis benchmarks: cannot connect to Redis");
            return;
        }
    };

    let mut group = c.benchmark_group("node_creation");
    group.sample_size(10);

    // Single-write: create 1000 nodes one at a time.
    group.bench_with_input(
        BenchmarkId::new("single", NODE_COUNT),
        &NODE_COUNT,
        |b, &n| {
            b.iter(|| {
                let graph = "bench_single";
                ensure_graph(&store, graph);
                for i in 0..n {
                    let (labels, props) = make_node(i);
                    store.create_node(graph, labels, props).unwrap();
                }
                let _ = store.delete_graph(graph);
            });
        },
    );

    // Batched: create 1000 nodes using pipeline batching (batch size 100).
    group.bench_with_input(
        BenchmarkId::new("batched", NODE_COUNT),
        &NODE_COUNT,
        |b, &n| {
            b.iter(|| {
                let graph = "bench_batch";
                ensure_graph(&store, graph);
                let batch_size = 100;
                let nodes: Vec<_> = (0..n).map(make_node).collect();
                for chunk in nodes.chunks(batch_size) {
                    store.batch_create_nodes(graph, chunk.to_vec()).unwrap();
                }
                let _ = store.delete_graph(graph);
            });
        },
    );

    group.finish();
}

criterion_group!(benches, bench_single_vs_batch);
criterion_main!(benches);
