use indexmap::IndexMap;

use gql_core::planner::collect_graph_statistics;
use gql_core::storage::{GraphStore, RangeQuery};
use gql_core::types::Value;
use gql_redis_store::RedisGraphStore;
use uuid::Uuid;

fn cluster_store() -> RedisGraphStore {
    std::env::var("GQL_REDIS_CLUSTER_NODES").expect(
        "GQL_REDIS_CLUSTER_NODES must be set; run with `cargo test -p gql-redis-store --test cluster_pipeline -- --ignored --nocapture`",
    );
    RedisGraphStore::from_env().expect("cluster store should initialize from env")
}

fn unique_graph(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4().simple())
}

fn props(entries: &[(&str, Value)]) -> IndexMap<String, Value> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.clone()))
        .collect()
}

#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; run with `cargo test -p gql-redis-store --test cluster_pipeline -- --ignored --nocapture`"]
fn cluster_mode_pipelined_reads_and_index_backfill_succeed() {
    let store = cluster_store();

    let graph = unique_graph("cluster-pipeline");
    store.create_graph(&graph).expect("graph should be created");

    let alice = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("create_node should succeed in cluster mode");
    let bob = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Bob".into()))]),
        )
        .expect("second create_node should succeed in cluster mode");

    let nodes = store
        .get_nodes_batch(&graph, &[alice, bob])
        .expect("batch node read should succeed in cluster mode");
    assert_eq!(nodes.len(), 2);
    assert_eq!(
        nodes[0]
            .as_ref()
            .and_then(|node| node.properties.get("name")),
        Some(&Value::Str("Alice".into()))
    );
    assert_eq!(
        nodes[1]
            .as_ref()
            .and_then(|node| node.properties.get("name")),
        Some(&Value::Str("Bob".into()))
    );

    let knows = store
        .create_edge(&graph, alice, bob, "KNOWS".to_string(), IndexMap::new())
        .expect("create_edge should succeed in cluster mode");
    let edges = store
        .get_edges_batch(&graph, &[knows])
        .expect("batch edge read should succeed in cluster mode");
    assert_eq!(edges.len(), 1);
    let edge = edges[0].as_ref().expect("edge should exist");
    assert_eq!(edge.src, alice);
    assert_eq!(edge.dst, bob);
    assert_eq!(edge.rel_type, "KNOWS");

    store
        .create_index(&graph, "Person", "name")
        .expect("create_index should succeed in cluster mode");
    let target = Value::Str("Alice".into());
    let matches = store
        .find_nodes_by_label_and_property(&graph, "Person", "name", RangeQuery::Eq(&target))
        .expect("indexed lookup should succeed in cluster mode");
    assert_eq!(matches, vec![alice]);
}

#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; run with `cargo test -p gql-redis-store --test cluster_pipeline -- --ignored --nocapture`"]
fn cluster_mode_batch_and_cleanup_pipelines_succeed() {
    let store = cluster_store();

    let graph = unique_graph("cluster-batch");
    store.create_graph(&graph).expect("graph should be created");

    let nodes = store
        .batch_create_nodes(
            &graph,
            vec![
                (
                    vec!["Person".to_string(), "Employee".to_string()],
                    props(&[("name", Value::Str("Carol".into()))]),
                ),
                (
                    vec!["Person".to_string()],
                    props(&[("name", Value::Str("Dave".into()))]),
                ),
            ],
        )
        .expect("batch_create_nodes should succeed in cluster mode");
    assert_eq!(nodes.len(), 2);

    let edges = store
        .batch_create_edges(
            &graph,
            vec![(
                nodes[0],
                nodes[1],
                "REPORTS_TO".to_string(),
                IndexMap::new(),
            )],
        )
        .expect("batch_create_edges should succeed in cluster mode");
    assert_eq!(edges.len(), 1);

    let batch_nodes = store
        .get_nodes_batch(&graph, &nodes)
        .expect("batch node read should succeed after batch create");
    assert!(batch_nodes.iter().all(Option::is_some));

    let batch_edges = store
        .get_edges_batch(&graph, &edges)
        .expect("batch edge read should succeed after batch create");
    assert!(batch_edges.iter().all(Option::is_some));

    store
        .add_node_labels(&graph, nodes[1], &[String::from("Manager")])
        .expect("add_node_labels should succeed in cluster mode");
    let relabeled = store
        .get_node(&graph, nodes[1])
        .expect("get_node after add_node_labels should succeed")
        .expect("relabeled node should still exist");
    assert!(relabeled.labels.iter().any(|label| label == "Manager"));
    let labels = store
        .all_label_names(&graph)
        .expect("all_label_names should succeed after add_node_labels");
    assert!(labels.iter().any(|label| label == "Manager"));

    store
        .remove_node_labels(&graph, nodes[0], &[String::from("Employee")])
        .expect("remove_node_labels should succeed in cluster mode");
    let updated = store
        .get_node(&graph, nodes[0])
        .expect("get_node should succeed")
        .expect("node should still exist");
    assert_eq!(updated.labels, vec!["Person".to_string()]);

    store
        .delete_node(&graph, nodes[0], true)
        .expect("delete_node(detach=true) should succeed in cluster mode");
    assert!(store
        .get_node(&graph, nodes[0])
        .expect("get_node after delete should succeed")
        .is_none());
    assert!(store
        .get_edge(&graph, edges[0])
        .expect("get_edge after detach delete should succeed")
        .is_none());
}

#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; run with `cargo test -p gql-redis-store --test cluster_pipeline -- --ignored --nocapture`"]
fn cluster_mode_batch_create_edges_honors_injected_edge_ids() {
    let store = cluster_store();

    let graph = unique_graph("cluster-batch-injected-edge-id");
    store.create_graph(&graph).expect("graph should be created");

    let nodes = store
        .batch_create_nodes(
            &graph,
            vec![
                (
                    vec!["Person".to_string()],
                    props(&[("name", Value::Str("A".into()))]),
                ),
                (
                    vec!["Person".to_string()],
                    props(&[("name", Value::Str("B".into()))]),
                ),
            ],
        )
        .expect("batch_create_nodes should succeed");

    let edges = store
        .batch_create_edges(
            &graph,
            vec![(
                nodes[0],
                nodes[1],
                "REPORTS_TO".to_string(),
                props(&[("_sd2_eid", Value::Int(4242))]),
            )],
        )
        .expect("batch_create_edges should honor injected edge IDs");
    assert_eq!(edges, vec![gql_core::storage::EdgeId(4242)]);

    let edge = store
        .get_edge(&graph, edges[0])
        .expect("get_edge should succeed")
        .expect("edge should exist");
    assert_eq!(edge.src, nodes[0]);
    assert_eq!(edge.dst, nodes[1]);
    assert_eq!(edge.rel_type, "REPORTS_TO");
}

#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; run with `cargo test -p gql-redis-store --test cluster_pipeline -- --ignored --nocapture`"]
fn cluster_mode_graph_metadata_reads_are_cluster_safe() {
    let store = cluster_store();

    let graph = unique_graph("cluster-metadata");
    store.create_graph(&graph).expect("graph should be created");

    let alice = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("create_node should succeed");
    let bob = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Bob".into()))]),
        )
        .expect("second create_node should succeed");
    store
        .create_edge(&graph, alice, bob, "KNOWS".to_string(), IndexMap::new())
        .expect("create_edge should succeed");

    let stats = store
        .graph_stats(&graph)
        .expect("graph_stats should succeed");
    assert_eq!(stats.node_count, 2);
    assert_eq!(stats.edge_count, 1);
    assert_eq!(stats.label_count, 1);
    assert_eq!(stats.rel_type_count, 1);

    let labels = store
        .all_label_names(&graph)
        .expect("all_label_names should succeed");
    assert_eq!(labels, vec!["Person".to_string()]);

    let planner_stats =
        collect_graph_statistics(&store, &graph).expect("planner stats should succeed");
    assert_eq!(planner_stats.total_nodes, 2);
    assert_eq!(planner_stats.total_edges, 1);
    assert_eq!(planner_stats.nodes_per_label.get("Person"), Some(&2));
}

#[test]
#[ignore = "requires GQL_REDIS_CLUSTER_NODES; run with `cargo test -p gql-redis-store --test cluster_pipeline -- --ignored --nocapture`"]
fn cluster_mode_drop_index_and_delete_graph_are_cluster_safe() {
    let store = cluster_store();

    let graph = unique_graph("cluster-ddl");
    store.create_graph(&graph).expect("graph should be created");
    store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("create_node should succeed");
    store
        .create_index(&graph, "Person", "name")
        .expect("create_index should succeed");
    store
        .drop_index(&graph, "Person", "name")
        .expect("drop_index should succeed in cluster mode");
    store
        .delete_graph(&graph)
        .expect("delete_graph should succeed in cluster mode");
    assert!(!store
        .graph_exists(&graph)
        .expect("graph_exists should succeed"));
}
