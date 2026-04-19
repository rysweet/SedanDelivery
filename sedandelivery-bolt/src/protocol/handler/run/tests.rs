use super::*;

use indexmap::IndexMap;
use std::any::Any;
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicUsize, Ordering};

use cypher_core::executor::{MutationStats, QueryResult};
use cypher_core::storage::{
    AdjEntry, EdgeData, EdgeId, GraphStats, GraphStore, InMemoryGraphStore, IndexDescriptor,
    NodeData, NodeId, RangeQuery, ReadConsistency,
};
use cypher_core::types::Value;

struct TrackingStore {
    inner: Arc<InMemoryGraphStore>,
    applied_consistency: Option<ReadConsistency>,
    get_node_calls: AtomicUsize,
    get_edge_calls: AtomicUsize,
    get_nodes_batch_calls: AtomicUsize,
    get_edges_batch_calls: AtomicUsize,
}

impl TrackingStore {
    fn new() -> Self {
        Self {
            inner: Arc::new(InMemoryGraphStore::new()),
            applied_consistency: None,
            get_node_calls: AtomicUsize::new(0),
            get_edge_calls: AtomicUsize::new(0),
            get_nodes_batch_calls: AtomicUsize::new(0),
            get_edges_batch_calls: AtomicUsize::new(0),
        }
    }
}

macro_rules! delegate {
    ($name:ident(&self) -> $ret:ty) => {
        fn $name(&self) -> $ret {
            self.inner.$name()
        }
    };
    ($name:ident(&self, $($arg:ident : $ty:ty),+) -> $ret:ty) => {
        fn $name(&self, $($arg: $ty),+) -> $ret {
            self.inner.$name($($arg),+)
        }
    };
}

impl GraphStore for TrackingStore {
    fn get_node(
        &self,
        graph: &str,
        id: NodeId,
    ) -> Result<Option<NodeData>, cypher_core::error::StorageError> {
        self.get_node_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.get_node(graph, id)
    }

    fn get_edge(
        &self,
        graph: &str,
        id: EdgeId,
    ) -> Result<Option<EdgeData>, cypher_core::error::StorageError> {
        self.get_edge_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.get_edge(graph, id)
    }

    fn get_nodes_batch(
        &self,
        graph: &str,
        ids: &[NodeId],
    ) -> Result<Vec<Option<NodeData>>, cypher_core::error::StorageError> {
        self.get_nodes_batch_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.get_nodes_batch(graph, ids)
    }

    fn get_edges_batch(
        &self,
        graph: &str,
        ids: &[EdgeId],
    ) -> Result<Vec<Option<EdgeData>>, cypher_core::error::StorageError> {
        self.get_edges_batch_calls.fetch_add(1, Ordering::Relaxed);
        self.inner.get_edges_batch(graph, ids)
    }

    delegate!(find_nodes_by_label(&self, graph: &str, label: &str) -> Result<Vec<NodeId>, cypher_core::error::StorageError>);

    fn find_nodes_by_label_and_property(
        &self,
        graph: &str,
        label: &str,
        property: &str,
        range: RangeQuery<'_>,
    ) -> Result<Vec<NodeId>, cypher_core::error::StorageError> {
        self.inner
            .find_nodes_by_label_and_property(graph, label, property, range)
    }

    delegate!(find_edges_by_type(&self, graph: &str, rel_type: &str) -> Result<Vec<EdgeId>, cypher_core::error::StorageError>);
    delegate!(out_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, cypher_core::error::StorageError>);
    delegate!(in_edges(&self, graph: &str, node_id: NodeId) -> Result<Vec<AdjEntry>, cypher_core::error::StorageError>);

    fn out_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, cypher_core::error::StorageError> {
        self.inner.out_edges_by_type(graph, node_id, rel_types)
    }

    fn in_edges_by_type(
        &self,
        graph: &str,
        node_id: NodeId,
        rel_types: &[&str],
    ) -> Result<Vec<AdjEntry>, cypher_core::error::StorageError> {
        self.inner.in_edges_by_type(graph, node_id, rel_types)
    }

    delegate!(out_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, cypher_core::error::StorageError>);
    delegate!(in_edge_ids(&self, graph: &str, node_id: NodeId) -> Result<Vec<EdgeId>, cypher_core::error::StorageError>);
    delegate!(all_node_ids(&self, graph: &str) -> Result<Vec<NodeId>, cypher_core::error::StorageError>);
    delegate!(all_edge_ids(&self, graph: &str) -> Result<Vec<EdgeId>, cypher_core::error::StorageError>);

    fn create_node(
        &self,
        graph: &str,
        labels: Vec<String>,
        properties: IndexMap<String, Value>,
    ) -> Result<NodeId, cypher_core::error::StorageError> {
        self.inner.create_node(graph, labels, properties)
    }

    fn create_edge(
        &self,
        graph: &str,
        src: NodeId,
        dst: NodeId,
        rel_type: String,
        properties: IndexMap<String, Value>,
    ) -> Result<EdgeId, cypher_core::error::StorageError> {
        self.inner
            .create_edge(graph, src, dst, rel_type, properties)
    }

    fn set_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), cypher_core::error::StorageError> {
        self.inner.set_node_properties(graph, id, properties)
    }

    fn replace_node_properties(
        &self,
        graph: &str,
        id: NodeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), cypher_core::error::StorageError> {
        self.inner.replace_node_properties(graph, id, properties)
    }

    fn set_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), cypher_core::error::StorageError> {
        self.inner.set_edge_properties(graph, id, properties)
    }

    fn replace_edge_properties(
        &self,
        graph: &str,
        id: EdgeId,
        properties: IndexMap<String, Value>,
    ) -> Result<(), cypher_core::error::StorageError> {
        self.inner.replace_edge_properties(graph, id, properties)
    }

    delegate!(remove_node_property(&self, graph: &str, id: NodeId, key: &str) -> Result<(), cypher_core::error::StorageError>);
    delegate!(remove_edge_property(&self, graph: &str, id: EdgeId, key: &str) -> Result<(), cypher_core::error::StorageError>);

    fn add_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), cypher_core::error::StorageError> {
        self.inner.add_node_labels(graph, id, labels)
    }

    fn remove_node_labels(
        &self,
        graph: &str,
        id: NodeId,
        labels: &[String],
    ) -> Result<(), cypher_core::error::StorageError> {
        self.inner.remove_node_labels(graph, id, labels)
    }

    delegate!(delete_node(&self, graph: &str, id: NodeId, detach: bool) -> Result<(), cypher_core::error::StorageError>);
    delegate!(delete_edge(&self, graph: &str, id: EdgeId) -> Result<(), cypher_core::error::StorageError>);
    delegate!(list_indexes(&self, graph: &str) -> Result<Vec<IndexDescriptor>, cypher_core::error::StorageError>);

    fn create_index(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), cypher_core::error::StorageError> {
        self.inner.create_index(graph, label, property)
    }

    fn drop_index(
        &self,
        graph: &str,
        label: &str,
        property: &str,
    ) -> Result<(), cypher_core::error::StorageError> {
        self.inner.drop_index(graph, label, property)
    }

    delegate!(create_graph(&self, name: &str) -> Result<(), cypher_core::error::StorageError>);
    delegate!(delete_graph(&self, name: &str) -> Result<(), cypher_core::error::StorageError>);
    delegate!(graph_exists(&self, name: &str) -> Result<bool, cypher_core::error::StorageError>);
    delegate!(list_graphs(&self) -> Result<Vec<String>, cypher_core::error::StorageError>);
    delegate!(graph_stats(&self, graph: &str) -> Result<GraphStats, cypher_core::error::StorageError>);
    delegate!(all_label_names(&self, graph: &str) -> Result<Vec<String>, cypher_core::error::StorageError>);

    fn with_read_consistency(&self, consistency: ReadConsistency) -> Option<Arc<dyn GraphStore>> {
        Some(Arc::new(Self {
            inner: Arc::clone(&self.inner),
            applied_consistency: Some(consistency),
            get_node_calls: AtomicUsize::new(0),
            get_edge_calls: AtomicUsize::new(0),
            get_nodes_batch_calls: AtomicUsize::new(0),
            get_edges_batch_calls: AtomicUsize::new(0),
        }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn ready_session_with_store(store: Arc<dyn GraphStore>) -> Session {
    ready_session_with_store_and_bookmark(
        store,
        Arc::new(crate::bookmark::BookmarkCounter::InMemory(AtomicU64::new(
            0,
        ))),
    )
}

fn ready_session_with_store_and_bookmark(
    store: Arc<dyn GraphStore>,
    global_bookmark: Arc<crate::bookmark::BookmarkCounter>,
) -> Session {
    Session {
        state: ServerState::Ready,
        store,
        conn_id: 1,
        current_graph: "neo4j".to_string(),
        pending_results: HashMap::new(),
        next_qid: 0,
        global_bookmark,
        tx_metadata: Vec::new(),
        tx_timeout: None,
        tx_store: None,
        write_semaphore: Arc::new(tokio::sync::Semaphore::new(128)),
        write_permit: None,
        saved_graph: None,
        tx_had_write: false,
        peer_ip: None,
        last_write_at: None,
    }
}

#[test]
fn result_to_records_prefetches_graph_entities_once() {
    let store = TrackingStore::new();
    store.inner.create_graph("neo4j").unwrap();
    let a = store
        .inner
        .create_node("neo4j", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let b = store
        .inner
        .create_node("neo4j", vec!["N".to_string()], IndexMap::new())
        .unwrap();
    let e = store
        .inner
        .create_edge("neo4j", a, b, "R".to_string(), IndexMap::new())
        .unwrap();

    let result = QueryResult {
        columns: vec!["n".to_string(), "r".to_string(), "p".to_string()],
        rows: vec![
            HashMap::from([
                ("n".to_string(), Value::Node(a)),
                ("r".to_string(), Value::Edge(e)),
                (
                    "p".to_string(),
                    Value::Path {
                        nodes: vec![a, b],
                        edges: vec![e],
                    },
                ),
            ]),
            HashMap::from([
                ("n".to_string(), Value::Node(b)),
                ("r".to_string(), Value::Edge(e)),
                (
                    "p".to_string(),
                    Value::Path {
                        nodes: vec![a, b],
                        edges: vec![e],
                    },
                ),
            ]),
        ],
        stats: MutationStats::default(),
        trailing_error: None,
    };

    let (records, query_type, stats_map, trailing_error) =
        bridge::executor::result_to_records(&result, &store, "neo4j");

    assert_eq!(records.len(), 2);
    assert_eq!(query_type, "r");
    assert!(stats_map.is_empty());
    assert!(trailing_error.is_none());
    assert_eq!(store.get_node_calls.load(Ordering::Relaxed), 0);
    assert_eq!(store.get_edge_calls.load(Ordering::Relaxed), 0);
    assert_eq!(store.get_nodes_batch_calls.load(Ordering::Relaxed), 1);
    assert_eq!(store.get_edges_batch_calls.load(Ordering::Relaxed), 1);
}

#[test]
fn resolve_effective_store_uses_topology_default_view_without_recent_write() {
    let store: Arc<dyn GraphStore> = Arc::new(TrackingStore::new());
    let session = ready_session_with_store(store);

    let effective_store = resolve_effective_store(&session);
    let tracking_store = effective_store
        .as_any()
        .downcast_ref::<TrackingStore>()
        .expect("tracking store view should be returned");
    assert_eq!(
        tracking_store.applied_consistency,
        Some(ReadConsistency::TopologyDefault)
    );
}

#[test]
fn resolve_effective_store_uses_session_fresh_primary_view_with_recent_write() {
    let store: Arc<dyn GraphStore> = Arc::new(TrackingStore::new());
    let mut session = ready_session_with_store(store);
    session.last_write_at = Some(std::time::Instant::now() - std::time::Duration::from_secs(1));

    let effective_store = resolve_effective_store(&session);
    let tracking_store = effective_store
        .as_any()
        .downcast_ref::<TrackingStore>()
        .expect("tracking store view should be returned");
    assert_eq!(
        tracking_store.applied_consistency,
        Some(ReadConsistency::SessionFreshPrimary)
    );
}

#[tokio::test]
async fn handle_run_rehydrates_primary_pin_from_recent_bookmark() {
    let store: Arc<dyn GraphStore> = Arc::new(TrackingStore::new());
    let global_bookmark = Arc::new(crate::bookmark::BookmarkCounter::InMemory(AtomicU64::new(
        0,
    )));
    let bookmark = crate::protocol::handler::next_bookmark(&global_bookmark).await;
    crate::protocol::handler::record_write_bookmark(&global_bookmark, &bookmark).await;
    let mut session = ready_session_with_store_and_bookmark(store, global_bookmark);

    let _ = handle_run(
        &mut session,
        "RETURN 1 AS n".to_string(),
        vec![],
        bolt_map! {
            "bookmarks" => BoltValue::List(vec![BoltValue::String(bookmark)])
        },
    )
    .await
    .expect("RUN should succeed");

    assert_eq!(
        session.read_consistency(),
        ReadConsistency::SessionFreshPrimary
    );
}

#[test]
fn resolve_effective_store_falls_back_for_stores_without_read_views() {
    let store: Arc<dyn GraphStore> = Arc::new(InMemoryGraphStore::new());
    let session = ready_session_with_store(Arc::clone(&store));

    let effective_store = resolve_effective_store(&session);
    assert!(
        Arc::ptr_eq(&store, &effective_store),
        "stores without read-view support should be reused directly"
    );
}

// ── strip_cypher_prefix ─────────────────────────────────────────────

#[test]
fn strip_cypher_prefix_no_prefix() {
    assert_eq!(
        strip_cypher_prefix("MATCH (n) RETURN n"),
        "MATCH (n) RETURN n"
    );
}

#[test]
fn strip_cypher_prefix_with_runtime_directive() {
    let q = "CYPHER runtime=interpreted MATCH (n) RETURN n";
    assert_eq!(strip_cypher_prefix(q), "MATCH (n) RETURN n");
}

#[test]
fn strip_cypher_prefix_case_insensitive() {
    let q = "cypher planner=cost RETURN 1";
    assert_eq!(strip_cypher_prefix(q), "RETURN 1");
}

#[test]
fn strip_cypher_prefix_mixed_case() {
    let q = "Cypher runtime=slotted CREATE (n:Foo)";
    assert_eq!(strip_cypher_prefix(q), "CREATE (n:Foo)");
}

#[test]
fn strip_cypher_prefix_with_explain() {
    let q = "CYPHER version=3.5 EXPLAIN MATCH (n) RETURN n";
    assert_eq!(strip_cypher_prefix(q), "EXPLAIN MATCH (n) RETURN n");
}

#[test]
fn strip_cypher_prefix_with_profile() {
    let q = "CYPHER runtime=compiled PROFILE MATCH (n) RETURN n";
    assert_eq!(strip_cypher_prefix(q), "PROFILE MATCH (n) RETURN n");
}

#[test]
fn strip_cypher_prefix_trims_whitespace() {
    assert_eq!(strip_cypher_prefix("  RETURN 1  "), "RETURN 1");
}

#[test]
fn strip_cypher_prefix_short_string() {
    assert_eq!(strip_cypher_prefix("RET"), "RET");
}

#[test]
fn strip_cypher_prefix_exactly_cypher_space() {
    let q = "CYPHER  ";
    let result = strip_cypher_prefix(q);
    assert!(!result.is_empty() || q.trim().is_empty());
}

// ── detect_explain_profile ──────────────────────────────────────────

#[test]
fn detect_explain_prefix() {
    let (is_explain, is_profile, actual) = detect_explain_profile("EXPLAIN MATCH (n) RETURN n");
    assert!(is_explain);
    assert!(!is_profile);
    assert_eq!(actual, "MATCH (n) RETURN n");
}

#[test]
fn detect_profile_prefix() {
    let (is_explain, is_profile, actual) = detect_explain_profile("PROFILE MATCH (n) RETURN n");
    assert!(!is_explain);
    assert!(is_profile);
    assert_eq!(actual, "MATCH (n) RETURN n");
}

#[test]
fn detect_explain_case_insensitive() {
    let (is_explain, _, actual) = detect_explain_profile("explain RETURN 1");
    assert!(is_explain);
    assert_eq!(actual, "RETURN 1");
}

#[test]
fn detect_profile_case_insensitive() {
    let (_, is_profile, actual) = detect_explain_profile("profile RETURN 1");
    assert!(is_profile);
    assert_eq!(actual, "RETURN 1");
}

#[test]
fn detect_normal_query_no_prefix() {
    let (is_explain, is_profile, actual) = detect_explain_profile("MATCH (n) RETURN n");
    assert!(!is_explain);
    assert!(!is_profile);
    assert_eq!(actual, "MATCH (n) RETURN n");
}

#[test]
fn detect_explain_profile_short_query() {
    let (is_explain, is_profile, actual) = detect_explain_profile("RET");
    assert!(!is_explain);
    assert!(!is_profile);
    assert_eq!(actual, "RET");
}

#[test]
fn detect_explain_profile_empty() {
    let (is_explain, is_profile, actual) = detect_explain_profile("");
    assert!(!is_explain);
    assert!(!is_profile);
    assert_eq!(actual, "");
}

#[test]
fn detect_explain_not_profile() {
    let (is_explain, is_profile, _) = detect_explain_profile("EXPLAIN PROFILE MATCH (n) RETURN n");
    assert!(is_explain);
    assert!(!is_profile);
}

// ── parse_use_database ──────────────────────────────────────────────

#[test]
fn parse_use_simple_database() {
    assert_eq!(parse_use_database("USE mydb"), Some("mydb".to_string()));
}

#[test]
fn parse_use_database_case_insensitive() {
    assert_eq!(parse_use_database("use neo4j"), Some("neo4j".to_string()));
}

#[test]
fn parse_use_database_backtick_quoted() {
    assert_eq!(
        parse_use_database("USE `my-database`"),
        Some("my-database".to_string())
    );
}

#[test]
fn parse_use_database_with_trailing_whitespace() {
    assert_eq!(
        parse_use_database("USE testdb  "),
        Some("testdb".to_string())
    );
}

#[test]
fn parse_use_database_returns_none_for_non_use() {
    assert_eq!(parse_use_database("MATCH (n) RETURN n"), None);
}

#[test]
fn parse_use_database_returns_none_for_empty_name() {
    assert_eq!(parse_use_database("USE "), None);
}

#[test]
fn parse_use_database_with_hyphen() {
    assert_eq!(
        parse_use_database("USE test-db"),
        Some("test-db".to_string())
    );
}

#[tokio::test]
async fn handle_run_merge_converts_results_in_blocking_context() {
    let store = Arc::new(InMemoryGraphStore::new());
    store.create_graph("neo4j").unwrap();
    let store: Arc<dyn GraphStore> = store;
    let mut session = ready_session_with_store(Arc::clone(&store));

    let responses = handle_run(
        &mut session,
        "MERGE (n:Person {name: 'Alice'}) RETURN n".to_string(),
        vec![],
        vec![],
    )
    .await
    .expect("MERGE should succeed");

    assert_eq!(responses.len(), 1);
    match &responses[0] {
        ServerMessage::Success(meta) => {
            let fields = meta
                .iter()
                .find(|(k, _)| k == "fields")
                .map(|(_, v)| v)
                .expect("SUCCESS should contain fields");
            match fields {
                BoltValue::List(f) => {
                    assert_eq!(f.len(), 1);
                    assert_eq!(f[0], BoltValue::String("n".to_string()));
                }
                other => panic!("expected List, got {other:?}"),
            }
        }
        other => panic!("expected SUCCESS, got {other:?}"),
    }

    assert_eq!(session.pending_results.len(), 1);
    let pending = session.pending_results.values().next().unwrap();
    assert_eq!(pending.records.len(), 1, "MERGE should produce one row");
    let node_val = &pending.records[0][0];
    match node_val {
        BoltValue::Structure { tag, fields } => {
            assert_eq!(*tag, crate::protocol::tags::TAG_NODE);
            assert_eq!(fields.len(), 3);
        }
        other => panic!("expected Node structure, got {other:?}"),
    }
}

#[tokio::test]
async fn handle_run_create_edge_converts_both_nodes_and_edge() {
    let store = Arc::new(InMemoryGraphStore::new());
    store.create_graph("neo4j").unwrap();
    let store: Arc<dyn GraphStore> = store;
    let mut session = ready_session_with_store(Arc::clone(&store));

    let responses = handle_run(
        &mut session,
        "CREATE (a:Person {name:'A'})-[r:KNOWS]->(b:Person {name:'B'}) RETURN a, r, b".to_string(),
        vec![],
        vec![],
    )
    .await
    .expect("CREATE with relationship should succeed");

    assert_eq!(responses.len(), 1);
    let pending = session.pending_results.values().next().unwrap();
    assert_eq!(pending.records.len(), 1);
    let row = &pending.records[0];
    assert_eq!(row.len(), 3);

    match &row[0] {
        BoltValue::Structure { tag, .. } => {
            assert_eq!(*tag, crate::protocol::tags::TAG_NODE, "a should be a Node");
        }
        other => panic!("expected Node for 'a', got {other:?}"),
    }
    match &row[1] {
        BoltValue::Structure { tag, .. } => {
            assert_eq!(
                *tag,
                crate::protocol::tags::TAG_RELATIONSHIP,
                "r should be a Relationship"
            );
        }
        other => panic!("expected Relationship for 'r', got {other:?}"),
    }
    match &row[2] {
        BoltValue::Structure { tag, .. } => {
            assert_eq!(*tag, crate::protocol::tags::TAG_NODE, "b should be a Node");
        }
        other => panic!("expected Node for 'b', got {other:?}"),
    }
}
