use indexmap::IndexMap;

use cypher_core::storage::GraphStore;
use cypher_core::types::Value;
use redis::Client;
use sd2_redis_store::RedisGraphStore;
use uuid::Uuid;

fn standalone_store() -> RedisGraphStore {
    standalone_redis_url();
    RedisGraphStore::from_env().expect("standalone store should initialize from env")
}

fn standalone_redis_url() -> String {
    std::env::var("GQL_REDIS_URL")
        .or_else(|_| std::env::var("REDIS_URL"))
        .expect(
            "GQL_REDIS_URL or REDIS_URL must be set; run with `cargo test -p sd2-redis-store --test metadata_audit -- --include-ignored`",
        )
}

fn prefixed(raw: String) -> String {
    let prefix = std::env::var("GQL_REDIS_KEY_PREFIX").unwrap_or_default();
    format!("{prefix}{raw}")
}

fn redis_client() -> Client {
    Client::open(standalone_redis_url().as_str()).expect("redis client should initialize")
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

fn labels_registry_key(graph: &str) -> String {
    prefixed(format!("G:{{{graph}}}:_labels"))
}

fn reltypes_registry_key(graph: &str) -> String {
    prefixed(format!("G:{{{graph}}}:_reltypes"))
}

fn propidx_manifest_key(graph: &str) -> String {
    prefixed(format!("G:{{{graph}}}:_propidx"))
}

fn deleting_key(graph: &str) -> String {
    prefixed(format!("G:{{{graph}}}:_deleting"))
}

fn graphs_registry_key() -> String {
    prefixed("__sd2_registry:graphs".to_string())
}

fn smembers_sorted(conn: &mut redis::Connection, key: &str) -> Vec<String> {
    let mut members: Vec<String> = redis::cmd("SMEMBERS")
        .arg(key)
        .query(conn)
        .expect("set members should be readable");
    members.sort_unstable();
    members
}

#[test]
#[ignore = "requires a running Redis at GQL_REDIS_URL or REDIS_URL"]
fn metadata_audit_repairs_stale_label_registry_member() {
    let store = standalone_store();
    let graph = unique_graph("metadata-audit-labels");
    let client = redis_client();
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");

    store.create_graph(&graph).expect("graph should be created");
    store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("node should be created");

    let registry_key = labels_registry_key(&graph);
    redis::cmd("SADD")
        .arg(&registry_key)
        .arg("GhostLabel")
        .query::<i64>(&mut conn)
        .expect("stale label should be injected");

    assert_eq!(
        store
            .all_label_names(&graph)
            .expect("labels should be readable"),
        vec!["Person".to_string()],
        "schema reads should already hide stale registry members before repair"
    );

    store
        .audit_graph_metadata(&graph, true)
        .expect("metadata audit repair should succeed");

    assert_eq!(
        smembers_sorted(&mut conn, &registry_key),
        vec!["Person".to_string()],
        "repair=true should prune stale _labels members with zero-cardinality backing sets"
    );

    store
        .delete_graph(&graph)
        .expect("graph cleanup should succeed");
}

#[test]
#[ignore = "requires a running Redis at GQL_REDIS_URL or REDIS_URL"]
fn metadata_audit_repairs_stale_reltype_registry_member() {
    let store = standalone_store();
    let graph = unique_graph("metadata-audit-reltypes");
    let client = redis_client();
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");

    store.create_graph(&graph).expect("graph should be created");
    let alice = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("source node should be created");
    let bob = store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Bob".into()))]),
        )
        .expect("target node should be created");
    store
        .create_edge(&graph, alice, bob, "KNOWS".to_string(), IndexMap::new())
        .expect("edge should be created");

    let registry_key = reltypes_registry_key(&graph);
    redis::cmd("SADD")
        .arg(&registry_key)
        .arg("STALE_REL")
        .query::<i64>(&mut conn)
        .expect("stale reltype should be injected");

    assert_eq!(
        store
            .graph_stats(&graph)
            .expect("graph stats should be readable")
            .rel_type_count,
        1,
        "graph stats should already ignore stale reltype registry members before repair"
    );

    store
        .audit_graph_metadata(&graph, true)
        .expect("metadata audit repair should succeed");

    assert_eq!(
        smembers_sorted(&mut conn, &registry_key),
        vec!["KNOWS".to_string()],
        "repair=true should prune stale _reltypes members with zero-cardinality backing sets"
    );

    store
        .delete_graph(&graph)
        .expect("graph cleanup should succeed");
}

#[test]
#[ignore = "requires a running Redis at GQL_REDIS_URL or REDIS_URL"]
fn metadata_audit_repairs_stale_propidx_manifest_after_drop_index() {
    let store = standalone_store();
    let graph = unique_graph("metadata-audit-propidx");
    let client = redis_client();
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");

    store.create_graph(&graph).expect("graph should be created");
    store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("node should be created");
    store
        .create_index(&graph, "Person", "name")
        .expect("index should be created");

    let manifest_key = propidx_manifest_key(&graph);
    let manifest_before_drop = smembers_sorted(&mut conn, &manifest_key);
    assert!(
        !manifest_before_drop.is_empty(),
        "create_index should register propidx keys in the cleanup manifest"
    );

    store
        .drop_index(&graph, "Person", "name")
        .expect("drop_index should succeed");

    let manifest_after_drop = smembers_sorted(&mut conn, &manifest_key);
    assert!(
        !manifest_after_drop.is_empty(),
        "drop_index currently leaves stale manifest members behind; this test must create repairable drift"
    );
    for member in &manifest_after_drop {
        let exists: i64 = redis::cmd("EXISTS")
            .arg(member)
            .query(&mut conn)
            .expect("manifest member existence should be readable");
        assert_eq!(
            exists, 0,
            "stale _propidx members should point to missing propidx keys after drop_index"
        );
    }

    let unrelated_key = prefixed(format!("outside-audit-namespace:{graph}"));
    redis::cmd("SET")
        .arg(&unrelated_key)
        .arg("still-present")
        .query::<()>(&mut conn)
        .expect("unrelated key should be created");
    redis::cmd("SADD")
        .arg(&manifest_key)
        .arg(&unrelated_key)
        .query::<i64>(&mut conn)
        .expect("invalid manifest member should be injected");

    store
        .audit_graph_metadata(&graph, true)
        .expect("metadata audit repair should succeed");

    assert!(
        smembers_sorted(&mut conn, &manifest_key).is_empty(),
        "repair=true should prune manifest members whose propidx keys are already gone"
    );
    let unrelated_exists: i64 = redis::cmd("EXISTS")
        .arg(&unrelated_key)
        .query(&mut conn)
        .expect("unrelated key existence should be readable");
    assert_eq!(
        unrelated_exists, 1,
        "repair must not touch unrelated Redis keys even when a poisoned manifest member is pruned"
    );
    redis::cmd("DEL")
        .arg(&unrelated_key)
        .query::<i64>(&mut conn)
        .expect("unrelated key cleanup should succeed");

    store
        .delete_graph(&graph)
        .expect("graph cleanup should succeed");
}

#[test]
#[ignore = "requires a running Redis at GQL_REDIS_URL or REDIS_URL"]
fn metadata_audit_does_not_finalize_active_deleting_marker() {
    let store = standalone_store();
    let graph = unique_graph("metadata-audit-deleting-active");
    let client = redis_client();
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");

    store.create_graph(&graph).expect("graph should be created");
    store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("node should be created");

    let deleting_key = deleting_key(&graph);
    redis::cmd("SET")
        .arg(&deleting_key)
        .arg("1")
        .query::<()>(&mut conn)
        .expect("deleting marker should be injected");

    let report = store
        .audit_graph_metadata(&graph, true)
        .expect("repair=true should report active delete state without mutating it");

    let deleting_exists: i64 = redis::cmd("EXISTS")
        .arg(&deleting_key)
        .query(&mut conn)
        .expect("deleting marker existence should be readable");
    let graph_still_registered: bool = redis::cmd("SISMEMBER")
        .arg(graphs_registry_key())
        .arg(&graph)
        .query(&mut conn)
        .expect("graph registry membership should be readable");

    assert_eq!(
        deleting_exists, 1,
        "repair=true must not resume or finalize deletion while graph-owned data still exists"
    );
    assert!(
        graph_still_registered,
        "active _deleting state must keep the graph name reserved until a human or delete retry finishes cleanup"
    );
    assert!(
        !report.deleting_orphaned,
        "active delete state must not be misclassified as orphaned"
    );
}

#[test]
#[ignore = "requires a running Redis at GQL_REDIS_URL or REDIS_URL"]
fn metadata_audit_reports_orphaned_deleting_marker_without_finalizing() {
    let store = standalone_store();
    let graph = unique_graph("metadata-audit-deleting-orphan");
    let client = redis_client();
    let mut conn = client
        .get_connection()
        .expect("redis connection should initialize");

    store.create_graph(&graph).expect("graph should be created");
    store
        .create_node(
            &graph,
            vec!["Person".to_string()],
            props(&[("name", Value::Str("Alice".into()))]),
        )
        .expect("node should be created");

    let graph_owned_keys: Vec<String> = redis::cmd("KEYS")
        .arg(prefixed(format!("G:{{{graph}}}:*")))
        .query(&mut conn)
        .expect("graph-owned keys should be discoverable in test setup");
    if !graph_owned_keys.is_empty() {
        redis::cmd("DEL")
            .arg(&graph_owned_keys)
            .query::<i64>(&mut conn)
            .expect("graph-owned keys should be removable in test setup");
    }

    let deleting_key = deleting_key(&graph);
    redis::cmd("SET")
        .arg(&deleting_key)
        .arg("1")
        .query::<()>(&mut conn)
        .expect("orphaned deleting marker should be injected");

    let report = store
        .audit_graph_metadata(&graph, true)
        .expect("repair=true should report orphaned delete state without finalizing it");

    let deleting_exists: i64 = redis::cmd("EXISTS")
        .arg(&deleting_key)
        .query(&mut conn)
        .expect("deleting marker existence should be readable");
    let graph_still_registered: bool = redis::cmd("SISMEMBER")
        .arg(graphs_registry_key())
        .arg(&graph)
        .query(&mut conn)
        .expect("graph registry membership should be readable");

    assert_eq!(
        deleting_exists, 1,
        "audit must leave _deleting in place until a separate delete retry or manual recovery path finishes cleanup"
    );
    assert!(
        graph_still_registered,
        "audit must not release graph-name reservation when it has only observed an orphan-like marker"
    );
    assert!(
        !store
            .graph_exists(&graph)
            .expect("graph_exists should be readable after orphan-like audit report"),
        "graph should remain hidden while the deleting marker exists"
    );
    assert!(
        report.deleting_orphaned,
        "audit should still report when delete-time roots are already gone"
    );
}
