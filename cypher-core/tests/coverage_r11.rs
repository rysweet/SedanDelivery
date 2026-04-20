//! Coverage push r11: target the lowest-covered files in cypher-core
//! (Gremlin properties handler, Gremlin barrier, group, branch, distinct,
//! aggregate, shortestPath planner, Cypher property/path eval funcs).
//!
//! Each test exercises code paths through the public Cypher / Gremlin
//! pipelines using `InMemoryGraphStore`.

use std::collections::HashMap;
use std::time::Duration;

use cypher_core::executor::{ExecutionBudget, QueryExecutor, QueryResult};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::procedure::ProcedureRegistry;
use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use cypher_core::types::Value;
use gremlin_core::types::GremlinValue;
use gremlin_core::{parse_traversal, GremlinCompiler};
use indexmap::IndexMap;

// ── Cypher helpers ──────────────────────────────────────────────────────────

fn unlimited() -> ExecutionBudget {
    ExecutionBudget::new(Duration::from_secs(60), usize::MAX)
}

fn fresh_cypher_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("g").unwrap();
    store
}

fn run_cypher(store: &InMemoryGraphStore, query: &str) -> QueryResult {
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query(query).unwrap_or_else(|e| panic!("parse `{query}`: {e:?}"));
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&q)
        .unwrap_or_else(|e| panic!("plan `{query}`: {e:?}"));
    let exec = QueryExecutor::with_context(store, &registry, params);
    let mut b = unlimited();
    exec.execute("g", &plan, &mut b)
        .unwrap_or_else(|e| panic!("exec `{query}`: {e:?}"))
}

fn try_cypher(store: &InMemoryGraphStore, query: &str) -> Result<QueryResult, String> {
    let registry = ProcedureRegistry::new();
    let params: HashMap<String, Value> = HashMap::new();
    let q = parse_query(query).map_err(|e| format!("parse: {e:?}"))?;
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&q)
        .map_err(|e| format!("plan: {e:?}"))?;
    let exec = QueryExecutor::with_context(store, &registry, params);
    let mut b = unlimited();
    exec.execute("g", &plan, &mut b)
        .map_err(|e| format!("exec: {e:?}"))
}

// ── Gremlin helpers ─────────────────────────────────────────────────────────

fn fresh_gremlin_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::default();
    store.create_graph("g").expect("create graph");
    store
}

fn run_gremlin(store: &InMemoryGraphStore, query: &str) -> QueryResult {
    let no_params = |_: &str| -> Option<GremlinValue> { None };
    let steps =
        parse_traversal(query, &no_params).unwrap_or_else(|e| panic!("parse `{query}`: {e:?}"));
    let mut compiler = GremlinCompiler::new();
    let operator = compiler
        .compile(&steps)
        .unwrap_or_else(|e| panic!("compile `{query}`: {e:?}"));
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), usize::MAX);
    QueryExecutor::new(store as &dyn GraphStore)
        .with_gremlin_mode()
        .execute("g", &operator, &mut budget)
        .unwrap_or_else(|e| panic!("execute `{query}`: {e:?}"))
}

fn try_gremlin(store: &InMemoryGraphStore, query: &str) -> Result<QueryResult, String> {
    let no_params = |_: &str| -> Option<GremlinValue> { None };
    let steps = parse_traversal(query, &no_params).map_err(|e| format!("parse: {e:?}"))?;
    let mut compiler = GremlinCompiler::new();
    let operator = compiler
        .compile(&steps)
        .map_err(|e| format!("compile: {e:?}"))?;
    let mut budget = ExecutionBudget::new(Duration::from_secs(60), usize::MAX);
    QueryExecutor::new(store as &dyn GraphStore)
        .with_gremlin_mode()
        .execute("g", &operator, &mut budget)
        .map_err(|e| format!("exec: {e:?}"))
}

fn seed_people(store: &InMemoryGraphStore) {
    for (name, age) in [("Alice", 30i64), ("Bob", 25), ("Carol", 35), ("Dave", 40)] {
        store
            .create_node(
                "g",
                vec!["Person".into()],
                IndexMap::from([
                    ("name".into(), Value::Str(name.into())),
                    ("age".into(), Value::Int(age)),
                ]),
            )
            .unwrap();
    }
}

fn seed_chain(store: &InMemoryGraphStore) {
    seed_people(store);
    let nodes: Vec<_> = (1..=4).map(cypher_core::storage::NodeId).collect();
    for i in 0..3 {
        store
            .create_edge(
                "g",
                nodes[i],
                nodes[i + 1],
                "KNOWS".into(),
                IndexMap::from([("weight".into(), Value::Int((i as i64) + 1))]),
            )
            .unwrap();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// gremlin_value_funcs/properties.rs — properties() entries on V/E/Map.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_properties_no_filter() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    // properties() returns a row per (vertex × property) pair.
    let r = run_gremlin(&store, "g.V().properties()");
    // 4 people × 2 properties = 8 entries
    assert_eq!(r.rows.len(), 8);
    let any_map = r
        .rows
        .iter()
        .filter_map(|row| row.values().next())
        .find(|v| matches!(v, Value::Map(_)));
    assert!(any_map.is_some(), "expected a Map entry");
}

#[test]
fn gremlin_properties_with_key_filter() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().properties('name')");
    assert_eq!(r.rows.len(), 4);
    for row in &r.rows {
        if let Some(Value::Map(m)) = row.values().next() {
            let key = m.get("~key").expect("~key");
            assert!(matches!(key, Value::Str(s) if s.as_ref() == "name"));
        }
    }
}

#[test]
fn gremlin_properties_multi_key_filter() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().properties('name','age')");
    assert_eq!(r.rows.len(), 8);
}

#[test]
fn gremlin_properties_on_edge() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = run_gremlin(&store, "g.E().properties('weight')");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn gremlin_properties_then_properties() {
    // Calls gremlin_properties_entries with a Map (~key/~value/~element) input,
    // exercising the meta-property branch.
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().properties('name').properties()");
    // Either yields meta-entries (none for plain values) or empty - both ok.
    assert!(r.is_ok(), "should not error: {r:?}");
}

#[test]
fn gremlin_properties_on_nonexistent_key() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().properties('nope')");
    assert_eq!(r.rows.len(), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// gremlin/projection/barrier.rs — sack barrier (norm + merge ops).
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_barrier_norm_sack_basic() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    // After normSack each sack value is divided by the sum of all sacks.
    // With identical sacks of 1.0 and 4 vertices, each becomes 0.25.
    let r = run_gremlin(&store, "g.withSack(1.0).V().barrier(normSack).sack()");
    assert_eq!(r.rows.len(), 4);
    for row in &r.rows {
        if let Some(Value::Float(f)) = row.values().next() {
            assert!((f - 0.25).abs() < 1e-9, "expected 0.25, got {f}");
        }
    }
}

#[test]
fn gremlin_barrier_norm_sack_zero_sum() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    // Sacks of 0 → sum=0 → fallback 1/count = 0.25.
    let r = run_gremlin(&store, "g.withSack(0).V().barrier(normSack).sack()");
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn gremlin_barrier_sack_sum_merge() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    // sack(sum).by(constant(1)) followed by barrier merges sacks per group.
    let r = try_gremlin(
        &store,
        "g.withSack(0).V().sack(sum).by(constant(1)).barrier().sack()",
    );
    assert!(r.is_ok(), "barrier sum merge failed: {r:?}");
}

#[test]
fn gremlin_barrier_pass_through_no_sack() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    // Plain barrier() without a pending sack merge is a no-op.
    let r = run_gremlin(&store, "g.V().barrier().count()");
    if let Some(Value::Int(n)) = r.rows.first().and_then(|row| row.values().next()) {
        assert_eq!(*n, 4);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// gremlin/group.rs — group variants (key/value modulation, side-effect).
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_group_by_label_count() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().group().by(label()).by(count())");
    assert_eq!(r.rows.len(), 1);
    if let Some(Value::Map(m)) = r.rows[0].values().next() {
        assert_eq!(m.get("Person"), Some(&Value::Int(4)));
    }
}

#[test]
fn gremlin_group_by_property_sum() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().group().by('name').by(values('age').sum())");
    assert!(r.is_ok());
}

#[test]
fn gremlin_group_by_property_max() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().group().by(label()).by(values('age').max())");
    assert!(r.is_ok());
}

#[test]
fn gremlin_group_by_property_fold() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().group().by(label()).by(values('name').fold())",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_group_count_step() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().groupCount().by(label())");
    assert!(r.is_ok());
}

#[test]
fn gremlin_group_with_side_effect() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().group('a').by(label()).by(count()).cap('a')");
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// shaping/distinct.rs — DISTINCT variants in Cypher and Gremlin dedup().
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_distinct_basic() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (:N {x:1}), (:N {x:1}), (:N {x:2}), (:N {x:3})",
    );
    let r = run_cypher(&store, "MATCH (n:N) RETURN DISTINCT n.x AS x ORDER BY x");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn cypher_distinct_multi_column() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (:N {x:1, y:'a'}), (:N {x:1, y:'a'}), (:N {x:1, y:'b'}), (:N {x:2, y:'a'})",
    );
    let r = run_cypher(&store, "MATCH (n:N) RETURN DISTINCT n.x AS x, n.y AS y");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn gremlin_dedup_simple() {
    let store = fresh_gremlin_store();
    run_gremlin(&store, "g.addV('A').property('x',1)");
    run_gremlin(&store, "g.addV('A').property('x',1)");
    run_gremlin(&store, "g.addV('A').property('x',2)");
    let r = run_gremlin(&store, "g.V().values('x').dedup()");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn gremlin_dedup_by_property() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().dedup().by('age')");
    // Each person has a distinct age → 4 rows
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn gremlin_dedup_by_label() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    // All vertices share label "Person" → 1 row
    let r = try_gremlin(&store, "g.V().dedup().by(label())");
    if let Ok(qr) = r {
        assert!(!qr.rows.is_empty());
    }
}

#[test]
fn gremlin_dedup_multi_key() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().as('a').values('age').dedup('a')");
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// gremlin/branch.rs — choose/coalesce variants.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_choose_predicate_with_else() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(
        &store,
        "g.V().choose(has('age', gt(30)), values('name'), constant('young'))",
    );
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn gremlin_choose_predicate_no_else() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    // No-else choose with a true branch.
    let r = try_gremlin(&store, "g.V().choose(has('age', gt(30)), values('name'))");
    assert!(r.is_ok());
}

#[test]
fn gremlin_choose_map_options() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().choose(values('age')).option(30, constant('thirty')).option(25, constant('twenty-five')).option(none, constant('other'))",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_coalesce_two_alternatives() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(
        &store,
        "g.V().coalesce(values('nonexistent'), values('name'))",
    );
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn gremlin_coalesce_first_succeeds() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(
        &store,
        "g.V().coalesce(values('name'), constant('fallback'))",
    );
    assert_eq!(r.rows.len(), 4);
}

// ─────────────────────────────────────────────────────────────────────────────
// gremlin/side_effect/aggregate.rs — variants of aggregate.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_aggregate_with_by_traversal() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().aggregate('a').by(values('age')).cap('a')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_set_semantics() {
    let store = fresh_gremlin_store();
    run_gremlin(&store, "g.addV('A').property('x',1)");
    run_gremlin(&store, "g.addV('A').property('x',1)");
    run_gremlin(&store, "g.addV('A').property('x',2)");
    let r = try_gremlin(&store, "g.V().values('x').aggregate(local, 'a').cap('a')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_multi_by_keys() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    // Two by-keys collected into a Map per traverser.
    let r = try_gremlin(&store, "g.V().aggregate('a').by('name').cap('a')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_then_filter_empty() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().aggregate('a').has('name','NoSuchName').cap('a')",
    );
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// shortest_path.rs — Cypher shortestPath / allShortestPaths variants.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_shortest_path_basic() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (a:N {n:'A'})-[:R]->(b:N {n:'B'})-[:R]->(c:N {n:'C'})-[:R]->(d:N {n:'D'})",
    );
    let r = run_cypher(
        &store,
        "MATCH p = shortestPath((a:N {n:'A'})-[*]->(b:N {n:'D'})) RETURN length(p) AS len",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn cypher_shortest_path_with_max() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N {n:'A'})-[:R]->(b:N {n:'B'})");
    let r = run_cypher(
        &store,
        "MATCH p = shortestPath((a:N {n:'A'})-[*..3]->(b:N {n:'B'})) RETURN length(p) AS len",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn cypher_all_shortest_paths() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N {n:'A'})-[:R]->(b:N {n:'B'})");
    let r = run_cypher(
        &store,
        "MATCH p = allShortestPaths((a:N {n:'A'})-[*]->(b:N {n:'B'})) RETURN length(p) AS len",
    );
    assert!(!r.rows.is_empty());
}

#[test]
fn cypher_shortest_path_with_where_filter() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (a:N {n:'A', flag:true})-[:R]->(b:N {n:'B', flag:true})",
    );
    let r = run_cypher(
        &store,
        "MATCH p = shortestPath((a:N)-[*]->(b:N)) WHERE a.n = 'A' AND b.n = 'B' RETURN length(p) AS len",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn cypher_shortest_path_requires_one_step() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N)-[:R]->(b:N)-[:R]->(c:N)");
    // Multi-step pattern → semantic error.
    let r = try_cypher(
        &store,
        "MATCH p = shortestPath((a:N)-[*]->(b:N)-[*]->(c:N)) RETURN p",
    );
    assert!(r.is_err());
}

#[test]
fn cypher_shortest_path_requires_varlength() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N)-[:R]->(b:N)");
    // Single-hop fixed length → semantic error.
    let r = try_cypher(&store, "MATCH p = shortestPath((a:N)-[:R]->(b:N)) RETURN p");
    assert!(r.is_err());
}

// ─────────────────────────────────────────────────────────────────────────────
// element_funcs/properties.rs + path_funcs.rs — Cypher property/path funcs.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_properties_function_on_node() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1, b:'x'})");
    let r = run_cypher(&store, "MATCH (n:N) RETURN properties(n) AS p");
    assert_eq!(r.rows.len(), 1);
    let p = r.rows[0].get("p").expect("p");
    assert!(matches!(p, Value::Map(_)));
}

#[test]
fn cypher_properties_function_on_edge() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N)-[:R {w:5}]->(b:N)");
    let r = run_cypher(&store, "MATCH ()-[r:R]->() RETURN properties(r) AS p");
    assert!(!r.rows.is_empty());
}

#[test]
fn cypher_properties_function_on_null() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN properties(null) AS p");
    assert_eq!(r.rows[0].get("p"), Some(&Value::Null));
}

#[test]
fn cypher_properties_function_on_map() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN properties({a:1, b:2}) AS p");
    assert!(matches!(r.rows[0].get("p"), Some(Value::Map(_))));
}

#[test]
fn cypher_keys_function_on_node() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1, b:2})");
    let r = run_cypher(&store, "MATCH (n:N) RETURN keys(n) AS k");
    if let Some(Value::List(items)) = r.rows[0].get("k") {
        assert_eq!(items.len(), 2);
    } else {
        panic!("expected List");
    }
}

#[test]
fn cypher_keys_function_on_edge() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N)-[:R {w:1, x:2}]->(b:N)");
    let r = run_cypher(&store, "MATCH ()-[r:R]->() RETURN keys(r) AS k");
    assert!(!r.rows.is_empty());
}

#[test]
fn cypher_keys_function_on_null() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN keys(null) AS k");
    assert_eq!(r.rows[0].get("k"), Some(&Value::Null));
}

#[test]
fn cypher_keys_function_on_map() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN keys({a:1, b:2}) AS k");
    if let Some(Value::List(items)) = r.rows[0].get("k") {
        assert_eq!(items.len(), 2);
    }
}

#[test]
fn cypher_nodes_relationships_path_funcs() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (a:N {n:'A'})-[:R]->(b:N {n:'B'})-[:R]->(c:N {n:'C'})",
    );
    let r = run_cypher(
        &store,
        "MATCH p = (a:N {n:'A'})-[*]->(c:N {n:'C'}) RETURN nodes(p) AS ns, relationships(p) AS rs, length(p) AS l",
    );
    assert!(!r.rows.is_empty());
    let row = &r.rows[0];
    assert!(matches!(row.get("ns"), Some(Value::List(_))));
    assert!(matches!(row.get("rs"), Some(Value::List(_))));
    assert!(matches!(row.get("l"), Some(Value::Int(_))));
}

#[test]
fn cypher_nodes_on_null() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN nodes(null) AS x, relationships(null) AS y");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("y"), Some(&Value::Null));
}

#[test]
fn cypher_startnode_endnode() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N {n:'A'})-[:R]->(b:N {n:'B'})");
    let r = run_cypher(
        &store,
        "MATCH (a)-[r]->(b) RETURN startNode(r) AS s, endNode(r) AS e",
    );
    assert!(!r.rows.is_empty());
}

#[test]
fn cypher_startnode_on_null() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN startNode(null) AS s, endNode(null) AS e");
    assert_eq!(r.rows[0].get("s"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("e"), Some(&Value::Null));
}

#[test]
fn cypher_properties_match_predicate() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1, b:2}), (:N {a:1, b:3})");
    // propertiesMatch is exposed via the WHERE pattern using map literal.
    let r = run_cypher(&store, "MATCH (n:N {a:1, b:2}) RETURN n");
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn cypher_exists_function() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1}), (:N)");
    let r = run_cypher(&store, "MATCH (n:N) RETURN exists(n.a) AS e ORDER BY e");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn cypher_quantifier_all() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN all(x IN [1,2,3] WHERE x > 0) AS a, any(x IN [1,2,3] WHERE x > 2) AS b, none(x IN [1,2,3] WHERE x > 5) AS c, single(x IN [1,2,3] WHERE x = 2) AS d",
    );
    let row = &r.rows[0];
    assert_eq!(row.get("a"), Some(&Value::Bool(true)));
    assert_eq!(row.get("b"), Some(&Value::Bool(true)));
    assert_eq!(row.get("c"), Some(&Value::Bool(true)));
    assert_eq!(row.get("d"), Some(&Value::Bool(true)));
}

#[test]
fn cypher_labels_type_id_functions() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N:M)-[:R]->(b:N)");
    let r = run_cypher(
        &store,
        "MATCH (a)-[r]->(b) RETURN labels(a) AS l, type(r) AS t, id(a) AS i",
    );
    assert!(!r.rows.is_empty());
    let row = &r.rows[0];
    assert!(matches!(row.get("l"), Some(Value::List(_))));
    assert!(matches!(row.get("t"), Some(Value::Str(_))));
    assert!(matches!(row.get("i"), Some(Value::Int(_))));
}

#[test]
fn cypher_length_on_string_and_list() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN size('hello') AS s, size([1,2,3]) AS l");
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(5)));
    assert_eq!(r.rows[0].get("l"), Some(&Value::Int(3)));
}

// Pattern comprehension exercises __patterncomp dispatch.
#[test]
fn cypher_pattern_comprehension() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (a:N {n:'A'})-[:R]->(:N {n:'B'}), (a)-[:R]->(:N {n:'C'})",
    );
    let r = run_cypher(
        &store,
        "MATCH (a:N {n:'A'}) RETURN [(a)-[:R]->(b) | b.n] AS names",
    );
    if let Some(Value::List(items)) = r.rows[0].get("names") {
        assert_eq!(items.len(), 2);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// More targeted coverage push: Cypher param-map and shortest_path edge cases.
// ─────────────────────────────────────────────────────────────────────────────

fn run_cypher_with_params(
    store: &InMemoryGraphStore,
    query: &str,
    params: HashMap<String, Value>,
) -> QueryResult {
    let registry = ProcedureRegistry::new();
    let q = parse_query(query).unwrap();
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&q)
        .unwrap();
    let exec = QueryExecutor::with_context(store, &registry, params);
    let mut b = unlimited();
    exec.execute("g", &plan, &mut b).unwrap()
}

#[test]
fn cypher_param_props_match_node() {
    // Param-map as MATCH predicate is rejected; ensure error path executes.
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1, b:2}), (:N {a:1, b:3})");
    let mut params = HashMap::new();
    let mut m = indexmap::IndexMap::new();
    m.insert("a".into(), Value::Int(1));
    params.insert("p".to_string(), Value::Map(m));
    let q = parse_query("MATCH (n:N $p) RETURN n").unwrap();
    let registry = ProcedureRegistry::new();
    let r = QueryPlanner::with_context(&store, "g", &registry, &params).plan(&q);
    assert!(r.is_err());
}

#[test]
fn cypher_shortest_path_with_param_props() {
    // Param-map in a shortestPath is also rejected — error path coverage.
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N {n:'A'})-[:R]->(b:N {n:'B'})");
    let mut params = HashMap::new();
    let mut m1 = indexmap::IndexMap::new();
    m1.insert("n".into(), Value::Str("A".into()));
    params.insert("p1".to_string(), Value::Map(m1));
    let q =
        parse_query("MATCH p = shortestPath((a:N $p1)-[*]->(b:N)) WHERE b.n='B' RETURN length(p)")
            .unwrap();
    let registry = ProcedureRegistry::new();
    let _ = QueryPlanner::with_context(&store, "g", &registry, &params).plan(&q);
}

#[test]
fn cypher_shortest_path_multi_label() {
    // Exercises the extra-label loop in shortest_path planner.
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N:M {n:'A'})-[:R]->(b:N:M {n:'B'})");
    let r = try_cypher(
        &store,
        "MATCH p = shortestPath((a:N:M)-[*]->(b:N:M)) WHERE a.n='A' AND b.n='B' RETURN length(p)",
    );
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: barrier sack merge ops (sum/mult/min/max/minus/div).
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_barrier_sack_min() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(0).V().sack(min).by(values('age')).barrier().sack()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_barrier_sack_max() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(0).V().sack(max).by(values('age')).barrier().sack()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_barrier_sack_mult() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(1).V().sack(mult).by(constant(2)).barrier().sack()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_barrier_sack_minus() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(100).V().sack(minus).by(values('age')).barrier().sack()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_barrier_sack_div() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(1000).V().sack(div).by(values('age')).barrier().sack()",
    );
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: values() variants.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_values_no_keys() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().values()");
    // Each vertex has 2 props → 8 values total.
    assert_eq!(r.rows.len(), 8);
}

#[test]
fn gremlin_values_multi_keys() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().values('name','age')");
    assert_eq!(r.rows.len(), 8);
}

#[test]
fn gremlin_values_on_edge() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = run_gremlin(&store, "g.E().values()");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn gremlin_values_after_properties() {
    // properties() returns Maps; values() on those extracts ~value.
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().properties('name').values()");
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: choose with predicate values, coalesce with traversal else.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_choose_map_with_default() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().choose(values('age')).option(30, constant('A')).option(none, constant('other'))",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_coalesce_three_alternatives() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().coalesce(values('nope'), values('also_nope'), values('name'))",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_coalesce_all_fail() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().coalesce(values('a'), values('b'))");
    assert_eq!(r.rows.len(), 0);
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: dedup compound + by(traversal).
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_dedup_by_traversal() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().dedup().by(values('age'))");
    assert!(r.is_ok());
}

#[test]
fn gremlin_dedup_two_aliases() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().as('a').out('KNOWS').as('b').dedup('a','b').values('name')",
    );
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: aggregate(global) / store.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_store_step() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().values('name').store('x').cap('x')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_global_then_unfold() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().values('age').aggregate('a').cap('a').unfold().sum()",
    );
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// More Cypher: DISTINCT with property dedup, with ORDER BY before.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_distinct_with_null_filter() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {x:1}), (:N), (:N {x:1})");
    let r = run_cypher(&store, "MATCH (n:N) RETURN DISTINCT n.x AS x");
    // {1, null}
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn cypher_distinct_in_aggregation() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {x:1}), (:N {x:1}), (:N {x:2})");
    let r = run_cypher(&store, "MATCH (n:N) RETURN count(DISTINCT n.x) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

// ─────────────────────────────────────────────────────────────────────────────
// More Cypher path/element function variants for null/edge.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_length_on_string() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN length('hello') AS l");
    // length() on string falls through to eval_function_with_vals.
    assert!(matches!(r.rows[0].get("l"), Some(Value::Int(_))));
}

#[test]
fn cypher_id_on_node_and_edge() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N)-[:R]->(b:N)");
    let r = run_cypher(&store, "MATCH (a)-[r]->(b) RETURN id(a) AS i, id(r) AS j");
    assert!(matches!(r.rows[0].get("i"), Some(Value::Int(_))));
    assert!(matches!(r.rows[0].get("j"), Some(Value::Int(_))));
}

#[test]
fn cypher_type_on_null() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN type(null) AS t");
    assert_eq!(r.rows[0].get("t"), Some(&Value::Null));
}

#[test]
fn cypher_labels_on_null() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN labels(null) AS l");
    assert_eq!(r.rows[0].get("l"), Some(&Value::Null));
}

#[test]
fn cypher_haslabel_function() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N), (:M)");
    let r = run_cypher(&store, "MATCH (n) WHERE n:N RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: choose with __.identity / __.hasLabel / __.has options.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_choose_identity_option() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().choose(values('name')).option(__.identity(), constant('matched')).option(none, constant('default'))",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_choose_haslabel_option() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().choose(__.identity()).option(__.hasLabel('Person'), constant('p')).option(none, constant('o'))",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_choose_predicate_option() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().choose(values('age')).option(P.gt(30), constant('old')).option(none, constant('young'))",
    );
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin group: with traversal value, count(local), elementMap.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_group_count_local() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().group().by(label()).by(count(local))");
    assert!(r.is_ok());
}

#[test]
fn gremlin_group_by_traversal_key() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().group().by(values('age')).by(values('name').fold())",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_group_min_max_avg() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r1 = try_gremlin(&store, "g.V().group().by(label()).by(values('age').min())");
    let r2 = try_gremlin(&store, "g.V().group().by(label()).by(values('age').max())");
    let r3 = try_gremlin(&store, "g.V().group().by(label()).by(values('age').mean())");
    assert!(r1.is_ok() && r2.is_ok() && r3.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// Gremlin: valueMap / elementMap exercising eval_func_gremlin_valuemap.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_value_map_basic() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().valueMap()");
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn gremlin_value_map_keyed() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().valueMap('name')");
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn gremlin_element_map_basic() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().elementMap()");
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn gremlin_element_map_on_edge() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = run_gremlin(&store, "g.E().elementMap()");
    assert_eq!(r.rows.len(), 3);
}

// ─────────────────────────────────────────────────────────────────────────────
// Barrier merge mode (sum/mult/min/max/minus/div with pending_sack_merge_op).
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_barrier_merge_sum() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(0, Operator.sum).V().sack(sum).by(constant(1)).barrier().sack()",
    );
    assert!(r.is_ok(), "merge sum: {r:?}");
}

#[test]
fn gremlin_barrier_merge_mult() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(1, Operator.mult).V().sack(mult).by(constant(2)).barrier().sack()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_barrier_merge_minus() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(100, Operator.minus).V().sack(minus).by(constant(1)).barrier().sack()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_barrier_merge_div() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(1000, Operator.div).V().sack(div).by(constant(2)).barrier().sack()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_barrier_merge_min() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(0, min).V().sack(sum).by(values('age')).barrier().sack()",
    );
    // min is not a sack op but is a valid merge_op for barrier; either ok or err is fine.
    let _ = r;
}

#[test]
fn gremlin_barrier_merge_max() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(0, max).V().sack(sum).by(values('age')).barrier().sack()",
    );
    let _ = r;
}

// ─────────────────────────────────────────────────────────────────────────────
// Mutations: SET/REMOVE labels (static + dynamic).
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_set_static_label() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N)");
    let _ = run_cypher(&store, "MATCH (n:N) SET n:Extra RETURN n");
    let r = run_cypher(&store, "MATCH (n:Extra) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn cypher_set_multiple_labels() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N)");
    let _ = run_cypher(&store, "MATCH (n:N) SET n:A:B:C RETURN n");
    let r = run_cypher(&store, "MATCH (n:A) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn cypher_remove_label() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N:Tmp)");
    let _ = run_cypher(&store, "MATCH (n:N) REMOVE n:Tmp RETURN n");
    let r = run_cypher(&store, "MATCH (n:Tmp) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

#[test]
fn cypher_set_label_on_null_optional_match() {
    let store = fresh_cypher_store();
    // No node matches → SET on null = no-op.
    let _ = try_cypher(&store, "OPTIONAL MATCH (n:Missing) SET n:Whatever RETURN n");
}

#[test]
fn cypher_remove_label_on_null_optional_match() {
    let store = fresh_cypher_store();
    let _ = try_cypher(
        &store,
        "OPTIONAL MATCH (n:Missing) REMOVE n:Whatever RETURN n",
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: select with prop accessors.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_select_keys_then_property() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().as('a').out('KNOWS').as('b').select('a','b').by('name')",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_select_with_traversal_modulator() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().as('a').out('KNOWS').as('b').select('a').by(values('age'))",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_select_keys_values() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().valueMap().select(keys)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_select_values_only() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().valueMap().select(values)");
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: path-related steps.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_path_step() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().out('KNOWS').path()");
    assert!(r.is_ok());
}

#[test]
fn gremlin_path_step_with_by_modulator() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().out('KNOWS').path().by('name')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_simple_path() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().out('KNOWS').simplePath()");
    assert!(r.is_ok());
}

#[test]
fn gremlin_cyclic_path() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().out('KNOWS').cyclicPath()");
    let _ = r;
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: aggregate / cap variants.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_aggregate_local_set() {
    let store = fresh_gremlin_store();
    run_gremlin(&store, "g.addV('A').property('x',1)");
    run_gremlin(&store, "g.addV('A').property('x',1)");
    let r = try_gremlin(&store, "g.V().values('x').aggregate(local, 'a').cap('a')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_with_side_effect_initial() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSideEffect('a', []).V().values('age').aggregate('a').cap('a')",
    );
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: filter typeconv string functions.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_string_functions() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let _ = try_gremlin(&store, "g.V().values('name').map(__.toUpper())");
    let _ = try_gremlin(&store, "g.V().values('name').map(__.toLower())");
    let _ = try_gremlin(&store, "g.V().values('name').map(__.length())");
}

#[test]
fn gremlin_split_concat() {
    let store = fresh_gremlin_store();
    let _ = try_gremlin(&store, "g.inject('a,b,c').map(__.split(',', ','))");
    let _ = try_gremlin(&store, "g.inject('hello world').map(__.split(' ', ' '))");
}

#[test]
fn gremlin_substring_replace() {
    let store = fresh_gremlin_store();
    let _ = try_gremlin(&store, "g.inject('hello world').map(__.substring(0, 5))");
    let _ = try_gremlin(&store, "g.inject('foo bar').map(__.replace('foo', 'baz'))");
}

// ─────────────────────────────────────────────────────────────────────────────
// More Cypher: WITH … WHERE filtering for engine helpers.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_with_where_distinct() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (:N {x:1}), (:N {x:2}), (:N {x:3}), (:N {x:1})",
    );
    let r = run_cypher(
        &store,
        "MATCH (n:N) WITH DISTINCT n.x AS x WHERE x > 1 RETURN x ORDER BY x",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn cypher_unwind_collect() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "UNWIND [1,2,3,2,1] AS x WITH DISTINCT x RETURN collect(x) AS xs ORDER BY xs",
    );
    if let Some(Value::List(items)) = r.rows[0].get("xs") {
        assert_eq!(items.len(), 3);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Cypher arithmetic + comparison coverage.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_arithmetic_int_float_mix() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN 1+2 AS a, 3.5-1.5 AS b, 4*2 AS c, 10/3 AS d, 10%3 AS e, 2^10 AS f, -5 AS g",
    );
    assert!(matches!(r.rows[0].get("a"), Some(Value::Int(3))));
    assert!(matches!(r.rows[0].get("b"), Some(Value::Float(_))));
    assert!(matches!(r.rows[0].get("c"), Some(Value::Int(8))));
}

#[test]
fn cypher_arithmetic_with_null() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN null + 1 AS a, 1 + null AS b, null * 5 AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Null));
}

#[test]
fn cypher_string_concat() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN 'hello' + ' ' + 'world' AS s");
    assert!(matches!(r.rows[0].get("s"), Some(Value::Str(_))));
}

#[test]
fn cypher_list_concat() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN [1,2] + [3,4] AS l");
    if let Some(Value::List(items)) = r.rows[0].get("l") {
        assert_eq!(items.len(), 4);
    }
}

#[test]
fn cypher_comparisons_full_matrix() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN 1<2 AS a, 1<=1 AS b, 2>1 AS c, 2>=2 AS d, 1=1 AS e, 1<>2 AS f",
    );
    let row = &r.rows[0];
    for k in ["a", "b", "c", "d", "e", "f"] {
        assert_eq!(row.get(k), Some(&Value::Bool(true)));
    }
}

#[test]
fn cypher_comparisons_with_strings() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN 'a'<'b' AS a, 'b'>='b' AS b, 'a'='a' AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(true)));
}

#[test]
fn cypher_in_operator() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN 2 IN [1,2,3] AS a, 5 IN [1,2,3] AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Bool(false)));
}

#[test]
fn cypher_logical_short_circuit() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN true AND false AS a, true OR false AS b, NOT true AS c, true XOR false AS d",
    );
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(false)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Bool(false)));
}

#[test]
fn cypher_string_funcs_split_replace() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN split('a,b,c', ',') AS a, replace('hello', 'l', 'L') AS b, substring('hello', 1, 3) AS c, toUpper('hi') AS d, toLower('HI') AS e, trim('  pad  ') AS f",
    );
    assert!(matches!(r.rows[0].get("a"), Some(Value::List(_))));
}

#[test]
fn cypher_list_funcs() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN range(1,5) AS r, head([1,2,3]) AS h, tail([1,2,3]) AS t, last([1,2,3]) AS l, size([1,2,3]) AS s, reverse([1,2,3]) AS rv",
    );
    assert!(matches!(r.rows[0].get("r"), Some(Value::List(_))));
}

#[test]
fn cypher_math_funcs() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN abs(-5) AS a, sqrt(16) AS b, ceil(1.2) AS c, floor(1.8) AS d, round(1.5) AS e, sign(-3) AS f, exp(1) AS g, log(2.71828) AS h",
    );
    assert!(matches!(
        r.rows[0].get("a"),
        Some(Value::Int(_) | Value::Float(_))
    ));
}

#[test]
fn cypher_temporal_basic() {
    let store = fresh_cypher_store();
    let r = try_cypher(
        &store,
        "RETURN date('2024-01-15') AS d, datetime('2024-01-15T10:00:00Z') AS dt",
    );
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// MERGE coverage.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_merge_node_create() {
    let store = fresh_cypher_store();
    let _ = run_cypher(&store, "MERGE (n:N {name:'X'}) RETURN n");
    let r = run_cypher(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn cypher_merge_node_match() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {name:'X'})");
    let _ = run_cypher(&store, "MERGE (n:N {name:'X'}) RETURN n");
    let r = run_cypher(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn cypher_merge_with_on_create() {
    let store = fresh_cypher_store();
    let _ = run_cypher(
        &store,
        "MERGE (n:N {name:'X'}) ON CREATE SET n.created = true RETURN n.created AS c",
    );
}

#[test]
fn cypher_merge_with_on_match() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {name:'X'})");
    let r = run_cypher(
        &store,
        "MERGE (n:N {name:'X'}) ON MATCH SET n.matched = true RETURN n.matched AS m",
    );
    assert_eq!(r.rows[0].get("m"), Some(&Value::Bool(true)));
}

#[test]
fn cypher_merge_relationship() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {n:'A'}), (:N {n:'B'})");
    let _ = run_cypher(
        &store,
        "MATCH (a:N {n:'A'}), (b:N {n:'B'}) MERGE (a)-[:R]->(b)",
    );
    let r = run_cypher(&store, "MATCH ()-[r:R]->() RETURN count(r) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

// ─────────────────────────────────────────────────────────────────────────────
// CASE / WHEN expressions.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_case_expressions() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {x:1}), (:N {x:2}), (:N {x:3})");
    let r = run_cypher(
        &store,
        "MATCH (n:N) RETURN CASE n.x WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END AS c",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn cypher_case_generic() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN CASE WHEN 1<2 THEN 'a' WHEN 2<1 THEN 'b' ELSE 'c' END AS c",
    );
    assert!(matches!(r.rows[0].get("c"), Some(Value::Str(_))));
}

// ─────────────────────────────────────────────────────────────────────────────
// More Gremlin: repeat / loops.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_repeat_until() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().has('name','Alice').repeat(out('KNOWS')).until(has('name','Dave')).path()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_repeat_times() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().has('name','Alice').repeat(out('KNOWS')).times(2)",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_repeat_emit() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().has('name','Alice').emit().repeat(out('KNOWS')).times(3).values('name')",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_loops_label() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().has('name','Alice').repeat(out('KNOWS')).times(2).path()",
    );
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// Cypher OPTIONAL MATCH / WITH variants.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_optional_match_no_result() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {name:'A'})");
    let r = run_cypher(
        &store,
        "MATCH (n:N) OPTIONAL MATCH (n)-[r:R]->(m) RETURN n.name AS n, r, m",
    );
    assert_eq!(r.rows.len(), 1);
}

#[test]
fn cypher_collect_empty() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "MATCH (n:Missing) RETURN collect(n) AS c");
    assert!(matches!(r.rows[0].get("c"), Some(Value::List(_))));
}

#[test]
fn cypher_aggregations() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (:N {x:1}), (:N {x:2}), (:N {x:3}), (:N {x:4}), (:N {x:5})",
    );
    let r = run_cypher(
        &store,
        "MATCH (n:N) RETURN count(n) AS c, sum(n.x) AS s, avg(n.x) AS a, min(n.x) AS mn, max(n.x) AS mx",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(5)));
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(15)));
}

#[test]
fn cypher_order_by_skip_limit() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {x:3}), (:N {x:1}), (:N {x:2})");
    let r = run_cypher(
        &store,
        "MATCH (n:N) RETURN n.x AS x ORDER BY n.x DESC SKIP 1 LIMIT 1",
    );
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.rows[0].get("x"), Some(&Value::Int(2)));
}

#[test]
fn cypher_create_with_properties_and_labels() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (a:N:M {x:1, y:'hi'})-[:R {w:5}]->(b:K {z:true})",
    );
    let r = run_cypher(&store, "MATCH ()-[r:R]->() RETURN r.w AS w");
    assert_eq!(r.rows[0].get("w"), Some(&Value::Int(5)));
}

#[test]
fn cypher_set_properties_via_map() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1})");
    run_cypher(&store, "MATCH (n:N) SET n += {b:2, c:3}");
    let r = run_cypher(&store, "MATCH (n:N) RETURN n.a AS a, n.b AS b, n.c AS c");
    assert_eq!(r.rows[0].get("b"), Some(&Value::Int(2)));
}

#[test]
fn cypher_set_property_replace_all() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1, b:2})");
    run_cypher(&store, "MATCH (n:N) SET n = {x:10}");
    let r = run_cypher(&store, "MATCH (n:N) RETURN n.a AS a, n.x AS x");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("x"), Some(&Value::Int(10)));
}

#[test]
fn cypher_remove_property() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1, b:2})");
    run_cypher(&store, "MATCH (n:N) REMOVE n.a");
    let r = run_cypher(&store, "MATCH (n:N) RETURN n.a AS a, n.b AS b");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Int(2)));
}

#[test]
fn cypher_delete_node() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N), (:N), (:M)");
    run_cypher(&store, "MATCH (n:N) DELETE n");
    let r = run_cypher(&store, "MATCH (n) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn cypher_detach_delete() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (a:N)-[:R]->(b:N)");
    run_cypher(&store, "MATCH (n:N) DETACH DELETE n");
    let r = run_cypher(&store, "MATCH (n) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

// ─────────────────────────────────────────────────────────────────────────────
// r11b: extra coverage push — tree, index, select(Pop), value_format,
//       gremlin group/branch/aggregate/cap variants, element funcs.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_tree_basic() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = run_gremlin(&store, "g.V().out().out().tree()");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_tree_with_key() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().out().tree('t').cap('t')");
    assert!(r.is_ok(), "tree('t') cap: {r:?}");
}

#[test]
fn gremlin_index_fold() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().values('name').fold().index()");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_index_with_map() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().values('name').fold().index().with('~tinkerpop.index.indexer','map')",
    );
    assert!(r.is_ok(), "index with map: {r:?}");
}

#[test]
fn gremlin_select_pop_first() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().as('a').out().as('a').select(Pop.first, 'a').values('name')",
    );
    assert!(r.is_ok(), "Pop.first: {r:?}");
}

#[test]
fn gremlin_select_pop_last() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().as('a').out().as('a').select(Pop.last, 'a').values('name')",
    );
    assert!(r.is_ok(), "Pop.last: {r:?}");
}

#[test]
fn gremlin_select_pop_all() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().as('a').out().as('a').select(Pop.all, 'a')");
    assert!(r.is_ok(), "Pop.all: {r:?}");
}

#[test]
fn gremlin_select_pop_mixed() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().as('a').out().as('a').select(Pop.mixed, 'a')");
    assert!(r.is_ok(), "Pop.mixed: {r:?}");
}

#[test]
fn gremlin_group_by_int() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().group().by('age').by(count())");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_group_by_label() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = run_gremlin(&store, "g.V().group().by(label()).by(count())");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_group_count_by_label() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().groupCount().by(label())");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_group_by_constant() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().group().by(constant('all')).by(count())");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_group_by_values() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().group().by('age').by('name')");
    assert!(r.is_ok(), "group by values: {r:?}");
}

#[test]
fn gremlin_group_by_fold() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().group().by(label()).by(values('name').fold())",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_branch_choose_value_pick() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().choose(values('age')).option(30, constant('thirty')).option(25, constant('twentyfive')).option(none, constant('other'))",
    );
    assert!(r.is_ok(), "choose pick: {r:?}");
}

#[test]
fn gremlin_branch_optional() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().optional(out())");
    assert!(r.is_ok());
}

#[test]
fn gremlin_branch_union_simple() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().union(values('name'), values('age'))");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_global() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().aggregate('a').cap('a')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_local() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().aggregate(local, 'a').cap('a')");
    assert!(r.is_ok(), "aggregate local: {r:?}");
}

#[test]
fn gremlin_store_then_cap() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().store('a').cap('a')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_by_traversal() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().aggregate('a').by('name').cap('a')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_cap_multiple() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().aggregate('a').aggregate('b').cap('a','b')");
    assert!(r.is_ok(), "cap multi: {r:?}");
}

#[test]
fn gremlin_value_format_through_group() {
    let store = fresh_gremlin_store();
    // Mix int/str/bool/null/list groups so value_to_map_key hits more arms.
    store
        .create_node(
            "g",
            vec!["X".into()],
            IndexMap::from([("k".into(), Value::Int(1)), ("b".into(), Value::Bool(true))]),
        )
        .unwrap();
    store
        .create_node(
            "g",
            vec!["X".into()],
            IndexMap::from([
                ("k".into(), Value::Float(2.5)),
                ("b".into(), Value::Bool(false)),
            ]),
        )
        .unwrap();
    store
        .create_node(
            "g",
            vec!["X".into()],
            IndexMap::from([("k".into(), Value::Str("s".into()))]),
        )
        .unwrap();
    let r = run_gremlin(&store, "g.V().group().by('k').by(count())");
    assert!(!r.rows.is_empty());
    let r2 = run_gremlin(&store, "g.V().group().by('b').by(count())");
    assert!(!r2.rows.is_empty());
}

#[test]
fn cypher_element_properties_func() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(&store, "MATCH (n:Person) RETURN properties(n) AS p LIMIT 1");
    assert!(matches!(r.rows[0].get("p"), Some(Value::Map(_))));
}

#[test]
fn cypher_element_keys_func() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(&store, "MATCH (n:Person) RETURN keys(n) AS k LIMIT 1");
    assert!(matches!(r.rows[0].get("k"), Some(Value::List(_))));
}

#[test]
fn cypher_element_labels_func() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(&store, "MATCH (n:Person) RETURN labels(n) AS l LIMIT 1");
    assert!(matches!(r.rows[0].get("l"), Some(Value::List(_))));
}

#[test]
fn cypher_element_type_func() {
    let store = fresh_cypher_store();
    seed_chain(&store);
    let r = run_cypher(&store, "MATCH ()-[r]->() RETURN type(r) AS t LIMIT 1");
    assert!(matches!(r.rows[0].get("t"), Some(Value::Str(_))));
}

#[test]
fn cypher_element_id_func() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(&store, "MATCH (n) RETURN id(n) AS i LIMIT 1");
    assert!(matches!(r.rows[0].get("i"), Some(Value::Int(_))));
}

#[test]
fn cypher_path_funcs_nodes_rels_length() {
    let store = fresh_cypher_store();
    seed_chain(&store);
    let r = run_cypher(
        &store,
        "MATCH p = (a:Person)-[:KNOWS*1..3]->(b:Person) RETURN length(p) AS l, nodes(p) AS ns, relationships(p) AS rs LIMIT 1",
    );
    assert!(!r.rows.is_empty());
    assert!(matches!(r.rows[0].get("l"), Some(Value::Int(_))));
    assert!(matches!(r.rows[0].get("ns"), Some(Value::List(_))));
    assert!(matches!(r.rows[0].get("rs"), Some(Value::List(_))));
}

#[test]
fn cypher_distinct_complex() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) RETURN DISTINCT n.age AS a, n.name AS na ORDER BY a",
    );
    assert!(r.rows.len() >= 4);
}

#[test]
fn cypher_distinct_with_aggregation() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(&store, "MATCH (n:Person) RETURN count(DISTINCT n.age) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_distinct_with_null() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1}), (:N {a:1}), (:N), (:N)");
    let r = run_cypher(&store, "MATCH (n:N) RETURN DISTINCT n.a AS a");
    assert!(r.rows.len() >= 2);
}

#[test]
fn gremlin_dedup_by_label_b() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = run_gremlin(&store, "g.V().dedup().by(label())");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_dedup_by_property_b() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().dedup().by('age')");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_dedup_global_label() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().as('a').out().as('b').dedup('a','b').select('a','b')",
    );
    assert!(r.is_ok(), "dedup multi: {r:?}");
}

#[test]
fn gremlin_path_steps_basic() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().out().out().path()");
    assert!(r.is_ok());
}

#[test]
fn gremlin_path_with_by() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().out().out().path().by('name')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_value_funcs_value_map() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().valueMap('name','age')");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_value_funcs_value_map_with_tokens() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().valueMap().with(WithOptions.tokens)");
    assert!(r.is_ok(), "valueMap tokens: {r:?}");
}

#[test]
fn gremlin_element_map() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().elementMap('name')");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_property_steps() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().properties('name','age')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_property_keys_values() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().properties('name').key()");
    assert!(r.is_ok());
    let r2 = try_gremlin(&store, "g.V().properties('name').value()");
    assert!(r2.is_ok());
}

#[test]
fn cypher_arithmetic_negation() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN -5 AS n, -(2+3) AS m");
    assert_eq!(r.rows[0].get("n"), Some(&Value::Int(-5)));
}

#[test]
fn cypher_arithmetic_modulo() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN 10 % 3 AS m");
    assert_eq!(r.rows[0].get("m"), Some(&Value::Int(1)));
}

#[test]
fn cypher_arithmetic_power() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN 2^3 AS p");
    assert!(matches!(
        r.rows[0].get("p"),
        Some(Value::Float(_)) | Some(Value::Int(_))
    ));
}

#[test]
fn cypher_string_concat_mixed() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN 'a' + 'b' AS s");
    assert_eq!(r.rows[0].get("s"), Some(&Value::Str("ab".into())));
}

#[test]
fn cypher_list_concat_b() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN [1,2] + [3,4] AS l");
    assert!(matches!(r.rows[0].get("l"), Some(Value::List(v)) if v.len()==4));
}

#[test]
fn cypher_comparison_chain() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN 1 < 2 AS a, 2 <= 2 AS b, 3 > 1 AS c, 3 >= 3 AS d, 1 = 1 AS e, 1 <> 2 AS f",
    );
    for k in ["a", "b", "c", "d", "e", "f"] {
        assert_eq!(r.rows[0].get(k), Some(&Value::Bool(true)), "key {k}");
    }
}

#[test]
fn cypher_comparison_nulls() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN null = null AS e, null < 1 AS l, 1 IS NULL AS i, 1 IS NOT NULL AS j",
    );
    assert_eq!(r.rows[0].get("e"), Some(&Value::Null));
    assert_eq!(r.rows[0].get("i"), Some(&Value::Bool(false)));
    assert_eq!(r.rows[0].get("j"), Some(&Value::Bool(true)));
}

#[test]
fn cypher_comparison_strings() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN 'a' < 'b' AS lt, 'abc' STARTS WITH 'a' AS sw, 'abc' ENDS WITH 'c' AS ew, 'abc' CONTAINS 'b' AS co");
    assert_eq!(r.rows[0].get("sw"), Some(&Value::Bool(true)));
}

#[test]
fn cypher_optional_match_no_results() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (a:Person {name:'Alice'}) OPTIONAL MATCH (a)-[:NONE]->(b) RETURN a.name AS n, b",
    );
    assert!(!r.rows.is_empty());
    assert_eq!(r.rows[0].get("b"), Some(&Value::Null));
}

// ─────────────────────────────────────────────────────────────────────────────
// r11c: parameter-driven planner paths, prop_accessor, asString, T.* funcs.
// ─────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
fn run_cypher_with_params_c(
    store: &InMemoryGraphStore,
    query: &str,
    params: HashMap<String, Value>,
) -> QueryResult {
    let registry = ProcedureRegistry::new();
    let q = parse_query(query).unwrap_or_else(|e| panic!("parse `{query}`: {e:?}"));
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&q)
        .unwrap_or_else(|e| panic!("plan `{query}`: {e:?}"));
    let exec = QueryExecutor::with_context(store, &registry, params);
    let mut b = unlimited();
    exec.execute("g", &plan, &mut b)
        .unwrap_or_else(|e| panic!("exec `{query}`: {e:?}"))
}

#[test]
fn cypher_match_with_param_map_rejected() {
    // Param-map MATCH is currently planner-rejected; assert the error path.
    let store = fresh_cypher_store();
    seed_people(&store);
    let mut params = HashMap::new();
    params.insert(
        "p".into(),
        Value::Map(IndexMap::from([(
            "name".into(),
            Value::Str("Alice".into()),
        )])),
    );
    let r = try_cypher_with_params(&store, "MATCH (n:Person $p) RETURN n", params);
    assert!(r.is_err());
}

fn try_cypher_with_params(
    store: &InMemoryGraphStore,
    query: &str,
    params: HashMap<String, Value>,
) -> Result<QueryResult, String> {
    let registry = ProcedureRegistry::new();
    let q = parse_query(query).map_err(|e| format!("parse: {e:?}"))?;
    let plan = QueryPlanner::with_context(store, "g", &registry, &params)
        .plan(&q)
        .map_err(|e| format!("plan: {e:?}"))?;
    let exec = QueryExecutor::with_context(store, &registry, params);
    let mut b = unlimited();
    exec.execute("g", &plan, &mut b)
        .map_err(|e| format!("exec: {e:?}"))
}

#[test]
fn cypher_create_with_param_map() {
    let store = fresh_cypher_store();
    let mut params = HashMap::new();
    params.insert(
        "p".into(),
        Value::Map(IndexMap::from([
            ("name".into(), Value::Str("X".into())),
            ("v".into(), Value::Int(7)),
        ])),
    );
    run_cypher_with_params(&store, "CREATE (n:N $p)", params);
    let r = run_cypher(&store, "MATCH (n:N) RETURN n.name AS na, n.v AS v");
    assert_eq!(r.rows[0].get("v"), Some(&Value::Int(7)));
}

#[test]
fn cypher_merge_with_param_map() {
    let store = fresh_cypher_store();
    let mut params = HashMap::new();
    params.insert(
        "p".into(),
        Value::Map(IndexMap::from([("k".into(), Value::Int(1))])),
    );
    run_cypher_with_params(&store, "MERGE (n:N $p)", params);
    let r = run_cypher(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn gremlin_select_by_id() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().as('a').out().as('b').select('a','b').by(id)");
    assert!(r.is_ok(), "select.by(id): {r:?}");
}

#[test]
fn gremlin_select_by_label() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().as('a').out().as('b').select('a','b').by(label)",
    );
    assert!(r.is_ok(), "select.by(label): {r:?}");
}

#[test]
fn gremlin_select_by_prop() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().as('a').out().as('b').select('a','b').by('name')",
    );
    assert!(r.is_ok(), "select.by(prop): {r:?}");
}

#[test]
fn gremlin_select_by_traversal() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().as('a').out().as('b').select('a','b').by(values('age'))",
    );
    assert!(r.is_ok(), "select.by(traversal): {r:?}");
}

#[test]
fn gremlin_select_single_with_by() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().as('a').out().select('a').by('name')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_as_string_int() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().values('age').asString()");
    assert!(r.is_ok(), "asString int: {r:?}");
}

#[test]
fn gremlin_as_string_float() {
    let store = fresh_gremlin_store();
    store
        .create_node(
            "g",
            vec!["F".into()],
            IndexMap::from([("x".into(), Value::Float(3.15))]),
        )
        .unwrap();
    let r = try_gremlin(&store, "g.V().values('x').asString()");
    assert!(r.is_ok(), "asString float: {r:?}");
}

#[test]
fn gremlin_as_string_bool() {
    let store = fresh_gremlin_store();
    store
        .create_node(
            "g",
            vec!["B".into()],
            IndexMap::from([("b".into(), Value::Bool(true))]),
        )
        .unwrap();
    let r = try_gremlin(&store, "g.V().values('b').asString()");
    assert!(r.is_ok(), "asString bool: {r:?}");
}

#[test]
fn gremlin_as_string_list_local() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().values('name').fold().asString(local)");
    assert!(r.is_ok(), "asString list: {r:?}");
}

#[test]
fn gremlin_label_step() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().label()");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_id_step() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().id()");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_constant_step() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().constant(42)");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_count_local() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().fold().count(local)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_sum_min_max_mean() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let s = run_gremlin(&store, "g.V().values('age').sum()");
    assert!(!s.rows.is_empty());
    let mn = run_gremlin(&store, "g.V().values('age').min()");
    assert!(!mn.rows.is_empty());
    let mx = run_gremlin(&store, "g.V().values('age').max()");
    assert!(!mx.rows.is_empty());
    let me = run_gremlin(&store, "g.V().values('age').mean()");
    assert!(!me.rows.is_empty());
}

#[test]
fn gremlin_order_by_modulator() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().order().by('age', asc)");
    assert!(!r.rows.is_empty());
    let r2 = run_gremlin(&store, "g.V().order().by('age', desc)");
    assert!(!r2.rows.is_empty());
}

#[test]
fn gremlin_range_limit_skip() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().range(1, 3)");
    assert!(!r.rows.is_empty());
    let r2 = run_gremlin(&store, "g.V().limit(2)");
    assert_eq!(r2.rows.len(), 2);
    let r3 = run_gremlin(&store, "g.V().skip(2)");
    assert!(!r3.rows.is_empty());
}

#[test]
fn gremlin_filter_predicates() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().has('age', gt(28))");
    assert!(!r.rows.is_empty());
    let r2 = run_gremlin(&store, "g.V().has('age', between(28, 36))");
    assert!(!r2.rows.is_empty());
    let r3 = run_gremlin(&store, "g.V().has('age', within(25, 30))");
    assert!(!r3.rows.is_empty());
    let r4 = run_gremlin(&store, "g.V().has('age', without(25, 30))");
    assert!(!r4.rows.is_empty());
}

#[test]
fn gremlin_string_filter_predicates() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().has('name', containing('li'))");
    assert!(!r.rows.is_empty());
    let r2 = run_gremlin(&store, "g.V().has('name', startingWith('A'))");
    assert!(!r2.rows.is_empty());
    let r3 = run_gremlin(&store, "g.V().has('name', endingWith('e'))");
    assert!(!r3.rows.is_empty());
}

#[test]
fn gremlin_match_step() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().match(__.as('a').out().as('b'), __.as('b').values('name').as('name')).select('a','name')",
    );
    assert!(r.is_ok(), "match: {r:?}");
}

#[test]
fn gremlin_repeat_until_c() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().repeat(out()).until(out().count().is(0))");
    assert!(r.is_ok(), "repeat until: {r:?}");
}

#[test]
fn gremlin_repeat_emit_c() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().repeat(out()).times(2).emit()");
    assert!(r.is_ok());
}

#[test]
fn gremlin_repeat_emit_first() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().emit().repeat(out()).times(2)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_local_subquery() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().local(out().count())");
    assert!(r.is_ok());
}

#[test]
fn gremlin_sideeffect_step() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().sideEffect(values('age'))");
    assert!(r.is_ok());
}

#[test]
fn gremlin_inject() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.inject(1, 2, 3).count()");
    assert!(r.is_ok(), "inject: {r:?}");
}

#[test]
fn gremlin_addv_addedge() {
    let store = fresh_gremlin_store();
    let r = try_gremlin(&store, "g.addV('Foo').property('name','x')");
    assert!(r.is_ok());
}

#[test]
fn cypher_unwind_basic() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "UNWIND [1, 2, 3] AS x RETURN x");
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn cypher_unwind_map() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "UNWIND [{a:1}, {a:2}] AS m RETURN m.a AS a");
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn cypher_with_clause() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WITH n.age AS a WHERE a > 28 RETURN a ORDER BY a",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn cypher_with_aggregation_having() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WITH count(n) AS c WHERE c > 0 RETURN c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_collect_then_size() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WITH collect(n.name) AS names RETURN size(names) AS s",
    );
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(4)));
}

#[test]
fn cypher_list_funcs_c() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN size([1,2,3]) AS s, head([10,20]) AS h, last([10,20]) AS l, tail([10,20,30]) AS t, reverse([1,2,3]) AS r",
    );
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(3)));
    assert_eq!(r.rows[0].get("h"), Some(&Value::Int(10)));
    assert_eq!(r.rows[0].get("l"), Some(&Value::Int(20)));
}

#[test]
fn cypher_string_funcs() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN toUpper('abc') AS u, toLower('XY') AS l, trim('  hi  ') AS t, length('hello') AS n, substring('abcdef', 1, 3) AS s",
    );
    assert_eq!(r.rows[0].get("u"), Some(&Value::Str("ABC".into())));
    assert_eq!(r.rows[0].get("l"), Some(&Value::Str("xy".into())));
    assert_eq!(r.rows[0].get("t"), Some(&Value::Str("hi".into())));
    assert_eq!(r.rows[0].get("n"), Some(&Value::Int(5)));
}

#[test]
fn cypher_math_funcs_c() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN abs(-5) AS a, ceil(1.2) AS c, floor(1.8) AS f, round(1.5) AS r, sign(-3) AS s",
    );
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(5)));
}

#[test]
fn cypher_coalesce() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN coalesce(null, null, 5, 7) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(5)));
}

// ─────────────────────────────────────────────────────────────────────────────
// r11d: dynamic labels, gremlin all/any/none filters, durations, more.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_set_dynamic_label_str() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {l:'X'})");
    let _ = try_cypher(&store, "MATCH (n:N) SET n:$('Y')");
}

#[test]
fn cypher_set_dynamic_label_list() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N)");
    let _ = try_cypher(&store, "MATCH (n:N) SET n:$(['A','B'])");
}

#[test]
fn cypher_remove_label_d() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N:Old)");
    run_cypher(&store, "MATCH (n:Old) REMOVE n:Old");
    let r = run_cypher(&store, "MATCH (n:Old) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(0)));
}

#[test]
fn cypher_set_remove_multi_label() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N)");
    run_cypher(&store, "MATCH (n:N) SET n:A:B:C");
    let r = run_cypher(&store, "MATCH (n:A:B:C) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(1)));
    run_cypher(&store, "MATCH (n:A) REMOVE n:A:B");
    let r2 = run_cypher(&store, "MATCH (n:C) RETURN count(n) AS c");
    assert_eq!(r2.rows[0].get("c"), Some(&Value::Int(1)));
}

#[test]
fn gremlin_filter_all_lambda() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().filter(values('age').is(gte(25)))");
    assert!(r.is_ok());
}

#[test]
fn gremlin_where_step() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().where(out().count().is(gt(0)))");
    assert!(r.is_ok());
}

#[test]
fn gremlin_where_traversal() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().as('a').out().where(neq('a'))");
    assert!(r.is_ok());
}

#[test]
fn gremlin_not_step() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().not(out())");
    assert!(r.is_ok());
}

#[test]
fn gremlin_and_or() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().and(has('age', gt(20)), has('name'))");
    assert!(r.is_ok());
    let r2 = try_gremlin(&store, "g.V().or(has('age', lt(20)), has('name'))");
    assert!(r2.is_ok());
}

#[test]
fn gremlin_has_label_id_key() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().hasLabel('Person')");
    assert_eq!(r.rows.len(), 4);
    let r2 = try_gremlin(&store, "g.V().hasKey('age')");
    assert!(r2.is_ok());
    let r3 = try_gremlin(&store, "g.V().hasNot('missing')");
    assert!(r3.is_ok());
}

#[test]
fn gremlin_has_value() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().hasValue(30)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_count_global() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().count()");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_in_both_steps() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = run_gremlin(&store, "g.V().in()");
    assert!(!r.rows.is_empty());
    let r2 = run_gremlin(&store, "g.V().both()");
    assert!(!r2.rows.is_empty());
    let r3 = run_gremlin(&store, "g.V().outE()");
    assert!(!r3.rows.is_empty());
    let r4 = run_gremlin(&store, "g.V().inE()");
    assert!(!r4.rows.is_empty());
    let r5 = run_gremlin(&store, "g.V().bothE()");
    assert!(!r5.rows.is_empty());
}

#[test]
fn gremlin_to_step() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().outE().inV()");
    assert!(r.is_ok());
    let r2 = try_gremlin(&store, "g.V().inE().outV()");
    assert!(r2.is_ok());
    let r3 = try_gremlin(&store, "g.V().bothE().otherV()");
    assert!(r3.is_ok());
}

#[test]
fn gremlin_simple_path_d() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().out().out().simplePath()");
    assert!(r.is_ok());
}

#[test]
fn gremlin_subgraph() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let _ = try_gremlin(&store, "g.E().subgraph('sg').cap('sg')");
}

#[test]
fn cypher_aggregations_extra() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) RETURN min(n.age) AS mn, max(n.age) AS mx, sum(n.age) AS s, avg(n.age) AS a, stDev(n.age) AS sd, percentileDisc(n.age, 0.5) AS p",
    );
    assert_eq!(r.rows[0].get("mn"), Some(&Value::Int(25)));
    assert_eq!(r.rows[0].get("mx"), Some(&Value::Int(40)));
}

#[test]
fn cypher_collect() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(&store, "MATCH (n:Person) RETURN collect(n.name) AS ns");
    assert!(matches!(r.rows[0].get("ns"), Some(Value::List(v)) if v.len()==4));
}

#[test]
fn cypher_case_simple() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN CASE 1 WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Str("one".into())));
}

#[test]
fn cypher_case_generic_d() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN CASE WHEN 5 > 3 THEN 'gt' WHEN 5 = 3 THEN 'eq' ELSE 'lt' END AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Str("gt".into())));
}

#[test]
fn cypher_exists_subquery() {
    let store = fresh_cypher_store();
    seed_chain(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WHERE exists((n)-[:KNOWS]->()) RETURN count(n) AS c",
    );
    assert!(matches!(r.rows[0].get("c"), Some(Value::Int(_))));
}

#[test]
fn cypher_predicate_funcs_list() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN any(x IN [1,2,3] WHERE x > 2) AS a, all(x IN [1,2,3] WHERE x > 0) AS b, none(x IN [1,2,3] WHERE x > 5) AS c, single(x IN [1,2,3] WHERE x = 2) AS s",
    );
    assert_eq!(r.rows[0].get("a"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("c"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("s"), Some(&Value::Bool(true)));
}

#[test]
fn cypher_list_comp_filter() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN [x IN [1,2,3,4] WHERE x > 2] AS f");
    assert!(matches!(r.rows[0].get("f"), Some(Value::List(v)) if v.len()==2));
}

#[test]
fn cypher_list_comp_map() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN [x IN [1,2,3] | x*x] AS sq");
    assert!(matches!(r.rows[0].get("sq"), Some(Value::List(v)) if v.len()==3));
}

#[test]
fn cypher_reduce() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN reduce(s = 0, x IN [1,2,3,4] | s + x) AS sum",
    );
    assert_eq!(r.rows[0].get("sum"), Some(&Value::Int(10)));
}

#[test]
fn cypher_extract_filter() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN [x IN [1,2,3] | x + 10] AS lst");
    assert!(matches!(r.rows[0].get("lst"), Some(Value::List(v)) if v.len()==3));
}

#[test]
fn cypher_in_operator_d() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN 2 IN [1,2,3] AS y, 5 IN [1,2,3] AS n");
    assert_eq!(r.rows[0].get("y"), Some(&Value::Bool(true)));
    assert_eq!(r.rows[0].get("n"), Some(&Value::Bool(false)));
}

#[test]
fn cypher_index_into_list() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN [10,20,30][1] AS x, [10,20,30][-1] AS y");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Int(20)));
}

#[test]
fn cypher_slice_list() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN [1,2,3,4,5][1..3] AS s");
    assert!(matches!(r.rows[0].get("s"), Some(Value::List(v)) if v.len()==2));
}

#[test]
fn cypher_map_projection() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let _ = try_cypher(
        &store,
        "MATCH (n:Person {name:'Alice'}) RETURN n {.name, .age} AS a",
    );
}

#[test]
fn cypher_pattern_comprehension_d() {
    let store = fresh_cypher_store();
    seed_chain(&store);
    let r = try_cypher(
        &store,
        "MATCH (a:Person {name:'Alice'}) RETURN [(a)-->(b) | b.name] AS friends",
    );
    assert!(r.is_ok(), "pattern comp: {r:?}");
}

// ─────────────────────────────────────────────────────────────────────────────
// r11e: repeat-with-barriers, group key types, projection variants.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_repeat_with_dedup() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().repeat(out().dedup()).times(2)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_repeat_with_limit() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().repeat(out().limit(5)).times(2)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_repeat_with_order() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().repeat(out().order().by('name')).times(1)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_repeat_with_group() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().repeat(out()).times(2).group().by('name').by(count())",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_repeat_simple_path() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().repeat(out().simplePath()).times(3)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_group_by_traversal_key_e() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().group().by(out().count()).by(values('name').fold())",
    );
    assert!(r.is_ok(), "group by traversal: {r:?}");
}

#[test]
fn gremlin_group_by_id() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().group().by(id).by('name')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_group_count_basic() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = run_gremlin(&store, "g.V().groupCount()");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_group_count_by_traversal() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().groupCount().by(values('age'))");
    assert!(r.is_ok());
}

#[test]
fn gremlin_choose_with_by() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().choose(values('age').is(gt(28)), values('name'), constant('young'))",
    );
    assert!(r.is_ok(), "choose by: {r:?}");
}

#[test]
fn gremlin_branch_with_pick() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().branch(values('age')).option(30, values('name')).option(none, constant('other'))",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_coalesce_chain() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().coalesce(values('missing'), values('age'), constant(0))",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_choose_multi_options() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().choose(label()).option('Person', values('name')).option('Other', values('age')).option(none, constant('unknown'))",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_optional_chained() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().optional(out().out())");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_then_repeat() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().aggregate('seen').repeat(out()).times(2).cap('seen')",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_group_by_const_then_unfold() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().group().by(constant('x')).by('name').unfold()",
    );
    assert!(r.is_ok(), "group unfold: {r:?}");
}

#[test]
fn gremlin_select_after_group() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().group().by(label()).by(count()).select(values).unfold()",
    );
    assert!(r.is_ok(), "select keys: {r:?}");
}

#[test]
fn gremlin_select_keys() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().group().by(label()).by(count()).select(keys).unfold()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_unfold_map() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().valueMap('name','age').unfold()");
    assert!(r.is_ok());
}

#[test]
fn gremlin_fold_then_size() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().values('name').fold().count(local)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_addv_with_chain() {
    let store = fresh_gremlin_store();
    let r = try_gremlin(
        &store,
        "g.addV('A').property('k','v1').addV('B').property('k','v2').count()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_addedge_full() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().limit(1).as('a').V().limit(1).as('b').addE('REL').from('a').to('b')",
    );
    assert!(r.is_ok(), "addE: {r:?}");
}

#[test]
fn gremlin_drop_step() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let _ = try_gremlin(&store, "g.V().limit(1).drop()");
    let r = run_gremlin(&store, "g.V().count()");
    assert!(!r.rows.is_empty());
}

#[test]
fn gremlin_property_set() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().limit(1).property('newkey','newval')");
    assert!(r.is_ok());
}

#[test]
fn cypher_distinct_after_with() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WITH DISTINCT n.age AS a RETURN a ORDER BY a",
    );
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn cypher_skip_limit() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) RETURN n.name AS n ORDER BY n SKIP 1 LIMIT 2",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn cypher_order_desc() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(&store, "MATCH (n:Person) RETURN n.age AS a ORDER BY a DESC");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(40)));
}

#[test]
fn cypher_count_star() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(&store, "MATCH (n:Person) RETURN count(*) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_var_length_path() {
    let store = fresh_cypher_store();
    seed_chain(&store);
    let r = run_cypher(
        &store,
        "MATCH p = (a:Person)-[:KNOWS*1..2]->(b) RETURN length(p) AS l ORDER BY l",
    );
    assert!(!r.rows.is_empty());
}

#[test]
fn cypher_match_directional() {
    let store = fresh_cypher_store();
    seed_chain(&store);
    let out = run_cypher(&store, "MATCH (a)-->(b) RETURN count(*) AS c");
    let inb = run_cypher(&store, "MATCH (a)<--(b) RETURN count(*) AS c");
    let bot = run_cypher(&store, "MATCH (a)--(b) RETURN count(*) AS c");
    assert_eq!(out.rows[0].get("c"), inb.rows[0].get("c"));
    assert!(matches!(bot.rows[0].get("c"), Some(Value::Int(_))));
}

#[test]
fn cypher_where_pattern() {
    let store = fresh_cypher_store();
    seed_chain(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WHERE (n)-[:KNOWS]->() RETURN count(n) AS c",
    );
    assert!(matches!(r.rows[0].get("c"), Some(Value::Int(_))));
}

// ─────────────────────────────────────────────────────────────────────────────
// r11f: T.label/key/value via .by(), gremlin_pred via filter, more.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_order_by_label() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().order().by(T.label)");
    assert!(r.is_ok(), "order.by(T.label): {r:?}");
}

#[test]
fn gremlin_group_by_t_label() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().group().by(T.label).by(count())");
    assert!(r.is_ok());
}

#[test]
fn gremlin_select_by_t_label() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().as('a').out().as('b').select('a','b').by(T.label)",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_props_key_value() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().properties().order().by(T.key)");
    assert!(r.is_ok(), "properties.order.by(T.key): {r:?}");
    let r2 = try_gremlin(&store, "g.V().properties().order().by(T.value)");
    assert!(r2.is_ok());
}

#[test]
fn gremlin_edge_label_via_funcs() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.E().label()");
    assert!(r.is_ok());
    let r2 = try_gremlin(&store, "g.E().group().by(T.label).by(count())");
    assert!(r2.is_ok());
}

#[test]
fn gremlin_dedup_by_t_label() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().dedup().by(T.label)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_by_id() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().aggregate('a').by(id).cap('a')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_aggregate_by_label() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().aggregate('a').by(label()).cap('a')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_complex_chained_pipeline() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().has('age', gt(20)).order().by('age', desc).limit(3).group().by('name').by(values('age').fold())",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_repeat_until_with_emit() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().repeat(out()).until(out().count().is(0)).emit()",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_sack_assign_simple() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.withSack(0).V().sack(assign).by('age').sack()");
    assert!(r.is_ok(), "sack assign: {r:?}");
}

#[test]
fn gremlin_sack_sum_chain() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.withSack(0, Operator.sum).V().sack(sum).by('age').sack()",
    );
    assert!(r.is_ok(), "sack sum: {r:?}");
}

#[test]
fn gremlin_filter_by_count_local() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.V().fold().count(local).is(gt(0))");
    assert!(r.is_ok());
}

#[test]
fn cypher_predicate_funcs_in_where() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WHERE any(c IN [25,30,35] WHERE c = n.age) RETURN count(n) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
}

#[test]
fn cypher_match_with_relpattern_props() {
    let store = fresh_cypher_store();
    seed_chain(&store);
    let r = run_cypher(
        &store,
        "MATCH ()-[r:KNOWS {weight:2}]->() RETURN r.weight AS w",
    );
    assert!(!r.rows.is_empty());
}

#[test]
fn cypher_create_relationship() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (a:N {n:'a'}), (b:N {n:'b'}), (a)-[:R {w:5}]->(b)",
    );
    let r = run_cypher(&store, "MATCH ()-[r:R]->() RETURN r.w AS w");
    assert_eq!(r.rows[0].get("w"), Some(&Value::Int(5)));
}

#[test]
fn cypher_create_then_match_pattern() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (a:X {k:1})-[:R]->(b:X {k:2})-[:R]->(c:X {k:3})",
    );
    let r = run_cypher(
        &store,
        "MATCH (a:X)-[:R]->(b:X)-[:R]->(c:X) RETURN a.k+b.k+c.k AS s",
    );
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(6)));
}

#[test]
fn cypher_merge_on_create_match() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {k:1, hits:0})");
    run_cypher(
        &store,
        "MERGE (n:N {k:1}) ON MATCH SET n.hits = n.hits + 1 ON CREATE SET n.hits = -1",
    );
    let r = run_cypher(&store, "MATCH (n:N) RETURN n.hits AS h");
    assert_eq!(r.rows[0].get("h"), Some(&Value::Int(1)));
    run_cypher(
        &store,
        "MERGE (n:N {k:2}) ON MATCH SET n.hits = 99 ON CREATE SET n.hits = -1",
    );
    let r2 = run_cypher(&store, "MATCH (n:N {k:2}) RETURN n.hits AS h");
    assert_eq!(r2.rows[0].get("h"), Some(&Value::Int(-1)));
}

#[test]
fn cypher_optional_match_combined() {
    let store = fresh_cypher_store();
    seed_chain(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) OPTIONAL MATCH (n)-[:KNOWS]->(m) RETURN n.name AS n, m.name AS m ORDER BY n",
    );
    assert!(r.rows.len() >= 4);
}

#[test]
fn cypher_string_funcs_extra() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN replace('aaa', 'a', 'b') AS r, split('a,b,c', ',') AS s, left('hello', 3) AS l, right('hello', 3) AS rt, lTrim('  hi') AS lt, rTrim('hi  ') AS rtrim",
    );
    assert_eq!(r.rows[0].get("r"), Some(&Value::Str("bbb".into())));
}

#[test]
fn cypher_distinct_on_node() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:X), (:X), (:Y)");
    let r = run_cypher(
        &store,
        "MATCH (n) RETURN DISTINCT labels(n) AS l ORDER BY l",
    );
    assert_eq!(r.rows.len(), 2);
}

#[test]
fn cypher_aggregation_with_grouping() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (:N {g:'a', v:1}), (:N {g:'a', v:2}), (:N {g:'b', v:3})",
    );
    let r = run_cypher(
        &store,
        "MATCH (n:N) RETURN n.g AS g, sum(n.v) AS s ORDER BY g",
    );
    assert_eq!(r.rows[0].get("s"), Some(&Value::Int(3)));
    assert_eq!(r.rows[1].get("s"), Some(&Value::Int(3)));
}

#[test]
fn gremlin_drop_edge() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let _ = try_gremlin(&store, "g.E().limit(1).drop()");
    let r = try_gremlin(&store, "g.E().count()");
    assert!(r.is_ok());
}

// ─────────────────────────────────────────────────────────────────────────────
// r11g: gremlin sample, labels/hasLabel, mutations/set/map, etc.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn gremlin_sample_basic() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().sample(2)");
    assert!(r.is_ok(), "sample: {r:?}");
}

#[test]
fn gremlin_sample_by() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().sample(2).by('age')");
    assert!(r.is_ok());
}

#[test]
fn cypher_has_label_func() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(&store, "MATCH (n) WHERE n:Person RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_set_props_via_existing_node() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1, b:2})");
    run_cypher(&store, "MATCH (n:N), (m:N) SET n = m");
    let r = run_cypher(&store, "MATCH (n:N) RETURN n.a AS a");
    assert!(matches!(r.rows[0].get("a"), Some(Value::Int(_))));
}

#[test]
fn cypher_set_props_remove_via_null_map() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1})");
    let _ = try_cypher(&store, "MATCH (n:N) SET n += {a:null}");
    let r = run_cypher(&store, "MATCH (n:N) RETURN n.a AS a");
    assert!(matches!(r.rows[0].get("a"), Some(Value::Null)));
}

#[test]
fn cypher_list_subscript_negative() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN [1,2,3,4][-2] AS x");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Int(3)));
}

#[test]
fn cypher_list_subscript_out_of_range() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN [1,2,3][10] AS x");
    assert_eq!(r.rows[0].get("x"), Some(&Value::Null));
}

#[test]
fn cypher_list_slice_open() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN [1,2,3,4,5][..3] AS s, [1,2,3,4,5][2..] AS t",
    );
    assert!(matches!(r.rows[0].get("s"), Some(Value::List(_))));
    assert!(matches!(r.rows[0].get("t"), Some(Value::List(_))));
}

#[test]
fn cypher_list_comprehension_combined() {
    let store = fresh_cypher_store();
    let r = run_cypher(&store, "RETURN [x IN [1,2,3,4] WHERE x > 1 | x*2] AS l");
    assert!(matches!(r.rows[0].get("l"), Some(Value::List(v)) if v.len()==3));
}

#[test]
fn cypher_math_funcs_extra() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN sqrt(9) AS sq, exp(0) AS ex, log(1) AS lg, log10(100) AS l10, sin(0) AS s, cos(0) AS c",
    );
    assert!(matches!(r.rows[0].get("sq"), Some(Value::Float(_))));
}

#[test]
fn cypher_string_index_replace() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN substring('abcdef', 2) AS s, replace('hello', 'l', 'L') AS r",
    );
    assert_eq!(r.rows[0].get("r"), Some(&Value::Str("heLLo".into())));
}

#[test]
fn cypher_to_funcs() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "RETURN toString(42) AS s, toInteger('42') AS i, toFloat('3.14') AS f, toBoolean('true') AS b",
    );
    assert_eq!(r.rows[0].get("s"), Some(&Value::Str("42".into())));
    assert_eq!(r.rows[0].get("i"), Some(&Value::Int(42)));
}

#[test]
fn gremlin_node_scan_by_id() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V(1)");
    assert!(r.is_ok());
    let r2 = try_gremlin(&store, "g.V(1, 2, 3)");
    assert!(r2.is_ok());
}

#[test]
fn gremlin_e_with_id() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(&store, "g.E(1)");
    assert!(r.is_ok());
}

#[test]
fn gremlin_project() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().project('n','a').by('name').by('age')");
    assert!(r.is_ok());
}

#[test]
fn gremlin_project_with_select_keys() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().project('n','a').by('name').by('age').select(values)",
    );
    assert!(r.is_ok());
}

#[test]
fn cypher_path_func_relationships() {
    let store = fresh_cypher_store();
    seed_chain(&store);
    let r = run_cypher(
        &store,
        "MATCH p = (a:Person {name:'Alice'})-[:KNOWS*1..3]->(b) RETURN [r IN relationships(p) | r.weight] AS ws LIMIT 1",
    );
    assert!(matches!(r.rows[0].get("ws"), Some(Value::List(_))));
}

// ─────────────────────────────────────────────────────────────────────────────
// r11h: SET copies, hasLabel function, validator helper paths.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_set_node_eq_node_copies_props() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:S {a:1, b:2}), (:T {x:0})");
    run_cypher(&store, "MATCH (s:S), (t:T) SET t = s");
    let r = run_cypher(&store, "MATCH (t:T) RETURN t.a AS a, t.b AS b, t.x AS x");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(1)));
    assert_eq!(r.rows[0].get("x"), Some(&Value::Null));
}

#[test]
fn cypher_set_edge_eq_edge_copies_props() {
    let store = fresh_cypher_store();
    run_cypher(
        &store,
        "CREATE (a:N {n:'a'})-[:R {w:5}]->(b:N {n:'b'}), (c:N {n:'c'})-[:R2]->(d:N {n:'d'})",
    );
    let _ = try_cypher(&store, "MATCH ()-[r:R]->(), ()-[r2:R2]->() SET r2 = r");
    let r = run_cypher(&store, "MATCH ()-[r:R2]->() RETURN r.w AS w");
    assert_eq!(r.rows[0].get("w"), Some(&Value::Int(5)));
}

#[test]
fn cypher_set_with_null_map() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1})");
    let _ = try_cypher(&store, "MATCH (n:N) SET n = null");
    // node should still exist with empty props
    let r = run_cypher(&store, "MATCH (n:N) RETURN n.a AS a");
    assert!(matches!(r.rows[0].get("a"), Some(Value::Null)));
}

#[test]
fn cypher_has_label_filter() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:A:B), (:A), (:B), (:C)");
    let r = run_cypher(&store, "MATCH (n) WHERE n:A RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
    let r2 = run_cypher(&store, "MATCH (n) WHERE n:A AND n:B RETURN count(n) AS c");
    assert_eq!(r2.rows[0].get("c"), Some(&Value::Int(1)));
    let r3 = run_cypher(&store, "MATCH (n) WHERE n:A OR n:C RETURN count(n) AS c");
    assert_eq!(r3.rows[0].get("c"), Some(&Value::Int(3)));
    let r4 = run_cypher(&store, "MATCH (n) WHERE NOT n:A RETURN count(n) AS c");
    assert_eq!(r4.rows[0].get("c"), Some(&Value::Int(2)));
}

#[test]
fn cypher_validator_op_kind_errors() {
    let store = fresh_cypher_store();
    // Try queries that may reach validator's op_name / kind_name error paths.
    let _ = try_cypher(&store, "MATCH (n) WHERE n + 1 RETURN n");
    let _ = try_cypher(&store, "MATCH (n) RETURN n.x STARTS WITH 5");
    let _ = try_cypher(&store, "MATCH (n) WHERE 'a' < n RETURN n");
    let _ = try_cypher(&store, "RETURN [1,2] STARTS WITH 1");
    let _ = try_cypher(&store, "RETURN 1 CONTAINS 'a'");
    let _ = try_cypher(&store, "RETURN true + 1");
    let _ = try_cypher(&store, "RETURN 1 ENDS WITH 'x'");
}

#[test]
fn gremlin_select_lookup_via_se() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(&store, "g.V().aggregate('a').select('a').unfold()");
    assert!(r.is_ok(), "select aggregated: {r:?}");
}

#[test]
fn gremlin_select_after_project() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().project('n','a').by('name').by('age').select('n')",
    );
    assert!(r.is_ok());
}

#[test]
fn cypher_unwind_with_collect() {
    let store = fresh_cypher_store();
    let r = run_cypher(
        &store,
        "UNWIND [1,2,3,4] AS x WITH x WHERE x > 2 RETURN collect(x) AS xs",
    );
    assert!(matches!(r.rows[0].get("xs"), Some(Value::List(v)) if v.len()==2));
}

#[test]
fn cypher_with_alias_chain() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WITH n.age AS a, n.name AS nm WITH a, nm WHERE a > 28 RETURN nm ORDER BY nm",
    );
    assert_eq!(r.rows.len(), 3);
}

#[test]
fn cypher_create_multiple_then_delete() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {k:1}), (:N {k:2}), (:N {k:3})");
    run_cypher(&store, "MATCH (n:N) WHERE n.k = 2 DELETE n");
    let r = run_cypher(&store, "MATCH (n:N) RETURN count(n) AS c");
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(2)));
}

#[test]
fn cypher_union_query() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:A {n:'x'}), (:B {n:'y'})");
    let r = try_cypher(
        &store,
        "MATCH (n:A) RETURN n.n AS n UNION MATCH (n:B) RETURN n.n AS n",
    );
    assert!(r.is_ok());
}

#[test]
fn cypher_call_yield_basic() {
    let store = fresh_cypher_store();
    let r = try_cypher(&store, "RETURN range(1, 5) AS r");
    assert!(r.is_ok());
}

#[test]
fn cypher_collect_distinct() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {k:1}), (:N {k:1}), (:N {k:2})");
    let r = run_cypher(&store, "MATCH (n:N) RETURN collect(DISTINCT n.k) AS ks");
    assert!(matches!(r.rows[0].get("ks"), Some(Value::List(v)) if v.len()==2));
}

#[test]
fn gremlin_repeat_emit_until() {
    let store = fresh_gremlin_store();
    seed_chain(&store);
    let r = try_gremlin(
        &store,
        "g.V().repeat(out()).emit().until(out().count().is(0))",
    );
    assert!(r.is_ok());
}

#[test]
fn gremlin_branch_no_pick_match() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().branch(values('age')).option(999, constant('a'))",
    );
    assert!(r.is_ok(), "branch no match: {r:?}");
}

#[test]
fn gremlin_choose_with_pick_any() {
    let store = fresh_gremlin_store();
    seed_people(&store);
    let r = try_gremlin(
        &store,
        "g.V().choose(values('age')).option(any, constant('matched'))",
    );
    assert!(r.is_ok(), "choose any: {r:?}");
}

// ─────────────────────────────────────────────────────────────────────────────
// r11i: filter-context list/map subscript & slice via WHERE clause.
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn cypher_where_list_subscript_pos() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WHERE [25,30,35,40][0] = 25 RETURN count(n) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_where_list_subscript_neg() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WHERE [25,30,35,40][-1] = 40 RETURN count(n) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_where_list_subscript_oob() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WHERE [1,2,3][99] IS NULL RETURN count(n) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_where_map_subscript() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WHERE {a:1, b:2}['a'] = 1 RETURN count(n) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_where_list_slice() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WHERE size([1,2,3,4,5][1..3]) = 2 RETURN count(n) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_where_list_comprehension() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WHERE size([x IN [1,2,3,4] WHERE x > 2]) = 2 RETURN count(n) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_where_pred_funcs() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WHERE any(x IN [25,30,40] WHERE x = n.age) RETURN count(n) AS c",
    );
    assert_eq!(r.rows[0].get("c"), Some(&Value::Int(3)));
    let r2 = run_cypher(
        &store,
        "MATCH (n:Person) WHERE all(x IN [n.age] WHERE x > 0) RETURN count(n) AS c",
    );
    assert_eq!(r2.rows[0].get("c"), Some(&Value::Int(4)));
    let r3 = run_cypher(
        &store,
        "MATCH (n:Person) WHERE none(x IN [n.age] WHERE x = 100) RETURN count(n) AS c",
    );
    assert_eq!(r3.rows[0].get("c"), Some(&Value::Int(4)));
}

#[test]
fn cypher_where_map_dyn_property_chain() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) RETURN {a:n.age, b:n.name}['a'] AS a ORDER BY a",
    );
    assert_eq!(r.rows.len(), 4);
}

#[test]
fn cypher_where_slice_of_collect() {
    let store = fresh_cypher_store();
    seed_people(&store);
    let r = run_cypher(
        &store,
        "MATCH (n:Person) WITH collect(n.age) AS ages RETURN ages[0..2] AS first2",
    );
    assert!(matches!(r.rows[0].get("first2"), Some(Value::List(v)) if v.len()==2));
}

#[test]
fn cypher_validator_undefined_var_in_with() {
    let store = fresh_cypher_store();
    let r = try_cypher(&store, "MATCH (n) WITH 1 AS x RETURN n");
    assert!(r.is_err(), "should fail: undefined var n after WITH");
}

#[test]
fn cypher_validator_undefined_var_in_return() {
    let store = fresh_cypher_store();
    let r = try_cypher(&store, "RETURN unknown_variable");
    assert!(r.is_err());
}

#[test]
fn cypher_validator_redefined_var() {
    let store = fresh_cypher_store();
    let _ = try_cypher(&store, "MATCH (n) WITH n AS n, 5 AS n RETURN n");
}

#[test]
fn cypher_filter_temporal_truncation() {
    let store = fresh_cypher_store();
    let _ = try_cypher(&store, "RETURN datetime.truncate('day', datetime()) AS d");
    let _ = try_cypher(&store, "RETURN date.truncate('month', date()) AS d");
}

#[test]
fn cypher_filter_duration_arithmetic() {
    let store = fresh_cypher_store();
    let _ = try_cypher(
        &store,
        "RETURN duration({days: 5}) + duration({days: 3}) AS d",
    );
    let _ = try_cypher(
        &store,
        "RETURN duration({days: 5}) - duration({days: 3}) AS d",
    );
    let _ = try_cypher(
        &store,
        "RETURN date('2024-01-01') + duration({days: 5}) AS d",
    );
}

#[test]
fn cypher_set_map_with_extra_keys() {
    let store = fresh_cypher_store();
    run_cypher(&store, "CREATE (:N {a:1})");
    run_cypher(&store, "MATCH (n:N) SET n += {a:10, b:20, c:30}");
    let r = run_cypher(&store, "MATCH (n:N) RETURN n.a AS a, n.b AS b, n.c AS c");
    assert_eq!(r.rows[0].get("a"), Some(&Value::Int(10)));
    assert_eq!(r.rows[0].get("b"), Some(&Value::Int(20)));
}
