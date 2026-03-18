//! Gremlin TCK-equivalent tests: mutations, filters, branches, repeats, and
//! complex composition patterns.
//!
//! Each test mirrors a Gremlin traversal expressed as an equivalent Cypher query
//! executed through the gql-core pipeline against the "modern" graph fixture.

use std::collections::HashMap;
use std::time::Duration;

use gql_core::executor::{ExecutionBudget, QueryExecutor, QueryResult};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::storage::InMemoryGraphStore;
use gql_core::types::Value;

use crate::modern_graph::create_modern_graph;

fn unlimited() -> ExecutionBudget {
    ExecutionBudget::new(Duration::from_secs(3600), usize::MAX)
}

/// Helper: parse → plan → execute a Cypher query against a fresh modern graph.
fn run_query(query: &str) -> Vec<HashMap<String, Value>> {
    let (store, _ids) = create_modern_graph();
    run_on(&store, query).rows
}

/// Helper: parse → plan → execute a Cypher query against the given store,
/// returning the full `QueryResult` (rows + mutation stats).
fn run_on(store: &InMemoryGraphStore, query: &str) -> QueryResult {
    let ast = parse_query(query).expect("parse failed");
    let planner = QueryPlanner::new(store, "modern");
    let plan = planner.plan(&ast).expect("plan failed");
    let executor = QueryExecutor::new(store);
    let mut budget = unlimited();
    executor
        .execute("modern", &plan, &mut budget)
        .expect("execute failed")
}

// ═══════════════════════════════════════════════════════════════════════════════
// 1. Mutation Tests (addV, addE, property, drop)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.addV("animal").property("name","puppy") — creates vertex, count +1
#[test]
fn test_add_vertex_animal() {
    let (store, _ids) = create_modern_graph();

    let result = run_on(&store, "CREATE (a:animal {name: 'puppy'}) RETURN a.name AS name");
    assert_eq!(result.stats.nodes_created, 1);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0]["name"], Value::Str("puppy".into()));

    // Total vertex count is now 7.
    let r2 = run_on(&store, "MATCH (n) RETURN count(n) AS cnt");
    assert_eq!(r2.rows[0]["cnt"], Value::Int(7));
}

/// g.addV("person").property("name","stephen").property("age",26) — multi-property
#[test]
fn test_add_vertex_multi_property() {
    let (store, _ids) = create_modern_graph();

    let result = run_on(
        &store,
        "CREATE (a:person {name: 'stephen', age: 26}) RETURN a.name AS name, a.age AS age",
    );
    assert_eq!(result.stats.nodes_created, 1);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0]["name"], Value::Str("stephen".into()));
    assert_eq!(result.rows[0]["age"], Value::Int(26));
}

/// g.V().has("name","marko").property("nickname","markito") — add property to existing
#[test]
fn test_add_property_to_existing() {
    let (store, _ids) = create_modern_graph();

    let result = run_on(
        &store,
        "MATCH (n {name: 'marko'}) SET n.nickname = 'markito' RETURN n.nickname AS nick",
    );
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0]["nick"], Value::Str("markito".into()));

    // Verify the property persists in a follow-up query.
    let r2 = run_on(&store, "MATCH (n {name: 'marko'}) RETURN n.nickname AS nick");
    assert_eq!(r2.rows[0]["nick"], Value::Str("markito".into()));
}

/// g.V().has("name","marko").property("age",30) — update existing property
#[test]
fn test_update_existing_property() {
    let (store, _ids) = create_modern_graph();

    run_on(&store, "MATCH (n {name: 'marko'}) SET n.age = 30");

    let r = run_on(&store, "MATCH (n {name: 'marko'}) RETURN n.age AS age");
    assert_eq!(r.rows[0]["age"], Value::Int(30));
}

/// g.addV("animal") + g.V().has("name","marko").addE("owns").to(animal) — add edge
#[test]
fn test_add_vertex_and_edge() {
    let (store, _ids) = create_modern_graph();

    // Create the animal and an edge from marko in one query.
    let result = run_on(
        &store,
        "MATCH (m {name: 'marko'}) CREATE (a:animal {name: 'puppy'}), (m)-[:owns]->(a) RETURN a.name AS name",
    );
    assert_eq!(result.stats.nodes_created, 1);
    assert_eq!(result.stats.edges_created, 1);
    assert_eq!(result.rows[0]["name"], Value::Str("puppy".into()));

    // Verify edge traversal.
    let r2 = run_on(&store, "MATCH ({name: 'marko'})-[:owns]->(a) RETURN a.name AS name");
    assert_eq!(r2.rows.len(), 1);
    assert_eq!(r2.rows[0]["name"], Value::Str("puppy".into()));
}

/// g.V().has("name","vadas").drop() — delete vertex, verify count -1
#[test]
fn test_drop_vertex() {
    let (store, _ids) = create_modern_graph();

    let result = run_on(&store, "MATCH (n {name: 'vadas'}) DETACH DELETE n");
    assert_eq!(result.stats.nodes_deleted, 1);

    let r = run_on(&store, "MATCH (n) RETURN count(n) AS cnt");
    assert_eq!(r.rows[0]["cnt"], Value::Int(5));
}

/// g.E().hasLabel("knows").drop() — delete edges, verify count -2
#[test]
fn test_drop_edges_by_label() {
    let (store, _ids) = create_modern_graph();

    let result = run_on(&store, "MATCH ()-[r:knows]->() DELETE r");
    assert_eq!(result.stats.edges_deleted, 2);

    let r = run_on(&store, "MATCH ()-[r]->() RETURN count(r) AS cnt");
    assert_eq!(r.rows[0]["cnt"], Value::Int(4));
}

/// g.V().has("name","peter").outE().drop() — delete peter's outgoing edges
#[test]
fn test_drop_outgoing_edges() {
    let (store, _ids) = create_modern_graph();

    let result = run_on(&store, "MATCH ({name: 'peter'})-[r]->() DELETE r");
    assert_eq!(result.stats.edges_deleted, 1);

    // Peter had 1 outgoing edge (created→lop), so total is now 5.
    let r = run_on(&store, "MATCH ()-[r]->() RETURN count(r) AS cnt");
    assert_eq!(r.rows[0]["cnt"], Value::Int(5));
}

/// g.addV("test").property("x",1) then g.V().has("x",1).count() — verify create+query
#[test]
fn test_create_then_query() {
    let (store, _ids) = create_modern_graph();

    run_on(&store, "CREATE (:test {x: 1})");

    let r = run_on(&store, "MATCH (n:test {x: 1}) RETURN count(n) AS cnt");
    assert_eq!(r.rows[0]["cnt"], Value::Int(1));
}

/// g.V().count() before and after addV — verify count increases
#[test]
fn test_vertex_count_increases_after_add() {
    let (store, _ids) = create_modern_graph();

    let before = run_on(&store, "MATCH (n) RETURN count(n) AS cnt");
    assert_eq!(before.rows[0]["cnt"], Value::Int(6));

    run_on(&store, "CREATE (:ghost {name: 'casper'})");

    let after = run_on(&store, "MATCH (n) RETURN count(n) AS cnt");
    assert_eq!(after.rows[0]["cnt"], Value::Int(7));
}

/// g.E().count() before and after addE — verify count increases
#[test]
fn test_edge_count_increases_after_add() {
    let (store, _ids) = create_modern_graph();

    let before = run_on(&store, "MATCH ()-[r]->() RETURN count(r) AS cnt");
    assert_eq!(before.rows[0]["cnt"], Value::Int(6));

    run_on(
        &store,
        "MATCH (a {name: 'vadas'}), (b {name: 'lop'}) CREATE (a)-[:uses]->(b)",
    );

    let after = run_on(&store, "MATCH ()-[r]->() RETURN count(r) AS cnt");
    assert_eq!(after.rows[0]["cnt"], Value::Int(7));
}

/// g.V().has("name","peter").drop() then g.V().count() — verify count decreases
#[test]
fn test_vertex_count_decreases_after_drop() {
    let (store, _ids) = create_modern_graph();

    run_on(&store, "MATCH (n {name: 'peter'}) DETACH DELETE n");

    let r = run_on(&store, "MATCH (n) RETURN count(n) AS cnt");
    assert_eq!(r.rows[0]["cnt"], Value::Int(5));
}

// ═══════════════════════════════════════════════════════════════════════════════
// 2. Filter Tests (where, not, and, or, is)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().where(out("knows")) — vertices that have outgoing "knows" edges
#[test]
fn test_where_out_knows() {
    let rows = run_query(
        "MATCH (n) WHERE EXISTS { (n)-[:knows]->() } RETURN n.name AS name ORDER BY name",
    );
    // Only marko has outgoing "knows" edges.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
}

/// g.V().where(out("created")) — marko, josh, peter
#[test]
fn test_where_out_created() {
    let rows = run_query(
        "MATCH (n) WHERE EXISTS { (n)-[:created]->() } RETURN n.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
}

/// g.V().not(hasLabel("person")) — software vertices only
#[test]
fn test_not_has_label_person() {
    let rows = run_query("MATCH (n) WHERE NOT n:person RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

/// g.V().not(has("age")) — vertices without age (lop, ripple)
#[test]
fn test_not_has_age() {
    let rows = run_query("MATCH (n) WHERE n.age IS NULL RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

/// g.V().has("age").and().has("name") — all persons (have both)
#[test]
fn test_has_age_and_name() {
    let rows = run_query(
        "MATCH (n) WHERE n.age IS NOT NULL AND n.name IS NOT NULL RETURN n.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
}

/// g.V().or(has("age",27), has("age",35)) — vadas + peter
#[test]
fn test_or_age_filter() {
    let rows =
        run_query("MATCH (n) WHERE n.age = 27 OR n.age = 35 RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("peter".into()));
    assert_eq!(rows[1]["name"], Value::Str("vadas".into()));
}

/// g.V().hasLabel("person").has("age", gt(30)).has("age", lt(35)) — josh only (32)
#[test]
fn test_age_range_filter() {
    let rows = run_query(
        "MATCH (n:person) WHERE n.age > 30 AND n.age < 35 RETURN n.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
}

/// g.V().has("age", within(27, 29, 35)) — vadas + marko + peter
#[test]
fn test_age_within() {
    let rows = run_query(
        "MATCH (n) WHERE n.age IN [27, 29, 35] RETURN n.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
    assert_eq!(rows[1]["name"], Value::Str("peter".into()));
    assert_eq!(rows[2]["name"], Value::Str("vadas".into()));
}

/// g.V().has("age", without(27, 29)) — josh + peter (ages not 27 or 29)
#[test]
fn test_age_without() {
    let rows = run_query(
        "MATCH (n) WHERE n.age IS NOT NULL AND NOT n.age IN [27, 29] RETURN n.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("peter".into()));
}

/// g.V().has("name", containing("a")) — names containing "a"
#[test]
fn test_name_containing() {
    let rows = run_query(
        "MATCH (n) WHERE n.name CONTAINS 'a' RETURN n.name AS name ORDER BY name",
    );
    // marko and vadas contain "a".
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
    assert_eq!(rows[1]["name"], Value::Str("vadas".into()));
}

/// g.V().where(outE().count().is(gt(1))) — vertices with >1 outgoing edge
#[test]
fn test_where_out_degree_gt_1() {
    let rows = run_query(
        "MATCH (n) \
         WITH n, size([(n)-[r]->() | r]) AS deg \
         WHERE deg > 1 \
         RETURN n.name AS name ORDER BY name",
    );
    // marko has 3 outgoing, josh has 2.
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
}

// ═══════════════════════════════════════════════════════════════════════════════
// 3. Branch Tests (union, choose, coalesce)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().has("name","marko").union(out("knows"), out("created")) — vadas+josh+lop
#[test]
fn test_union_traversals() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-[:knows]->(m) RETURN m.name AS name \
         UNION ALL \
         MATCH ({name: 'marko'})-[:created]->(m) RETURN m.name AS name",
    );
    let mut names: Vec<String> = rows
        .iter()
        .map(|r| match &r["name"] {
            Value::Str(s) => s.to_string(),
            other => panic!("expected Str, got {other:?}"),
        })
        .collect();
    names.sort();
    assert_eq!(names, vec!["josh", "lop", "vadas"]);
}

/// g.V().has("name","marko").union(out("knows").count(), out("created").count()) — [2, 1]
#[test]
fn test_union_counts() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-[:knows]->(m) RETURN count(m) AS cnt \
         UNION ALL \
         MATCH ({name: 'marko'})-[:created]->(m) RETURN count(m) AS cnt",
    );
    let mut counts: Vec<i64> = rows
        .iter()
        .map(|r| match r["cnt"] {
            Value::Int(v) => v,
            _ => panic!("expected Int"),
        })
        .collect();
    counts.sort();
    assert_eq!(counts, vec![1, 2]);
}

/// g.V().choose(hasLabel("person"), values("age"), values("lang"))
#[test]
fn test_choose_by_label() {
    let rows = run_query(
        "MATCH (n) \
         RETURN n.name AS name, \
                CASE WHEN n:person THEN n.age ELSE n.lang END AS val \
         ORDER BY name",
    );
    assert_eq!(rows.len(), 6);
    // josh → 32, lop → java, marko → 29, peter → 35, ripple → java, vadas → 27
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[0]["val"], Value::Int(32));
    assert_eq!(rows[1]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["val"], Value::Str("java".into()));
    assert_eq!(rows[2]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["val"], Value::Int(29));
    assert_eq!(rows[3]["name"], Value::Str("peter".into()));
    assert_eq!(rows[3]["val"], Value::Int(35));
    assert_eq!(rows[4]["name"], Value::Str("ripple".into()));
    assert_eq!(rows[4]["val"], Value::Str("java".into()));
    assert_eq!(rows[5]["name"], Value::Str("vadas".into()));
    assert_eq!(rows[5]["val"], Value::Int(27));
}

/// g.V().coalesce(has("age").values("age"), constant(-1)) — age or -1 for software
#[test]
fn test_coalesce_age_or_default() {
    let rows = run_query(
        "MATCH (n) RETURN n.name AS name, coalesce(n.age, -1) AS val ORDER BY name",
    );
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0]["val"], Value::Int(32));  // josh
    assert_eq!(rows[1]["val"], Value::Int(-1));  // lop
    assert_eq!(rows[2]["val"], Value::Int(29));  // marko
    assert_eq!(rows[3]["val"], Value::Int(35));  // peter
    assert_eq!(rows[4]["val"], Value::Int(-1));  // ripple
    assert_eq!(rows[5]["val"], Value::Int(27));  // vadas
}

// ═══════════════════════════════════════════════════════════════════════════════
// 4. Repeat Tests (variable-length paths)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().has("name","marko").repeat(out()).times(1) — 1-hop: vadas, josh, lop
#[test]
fn test_repeat_out_times_1() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-[*1..1]->(m) RETURN m.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("lop".into()));
    assert_eq!(rows[2]["name"], Value::Str("vadas".into()));
}

/// g.V().has("name","marko").repeat(out()).times(2) — 2-hop: ripple, lop (through josh)
#[test]
fn test_repeat_out_times_2() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-[*2..2]->(m) RETURN m.name AS name ORDER BY name",
    );
    // marko→josh→ripple, marko→josh→lop
    // (vadas and lop have no outgoing edges, so no 2-hop through them)
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

/// g.V().has("name","marko").repeat(out("knows")).times(1) — josh, vadas
#[test]
fn test_repeat_knows_times_1() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-[:knows*1..1]->(m) RETURN m.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("vadas".into()));
}

/// g.V().hasLabel("person").repeat(out()).times(0) — just the starting vertices
#[test]
fn test_repeat_times_0() {
    // Zero-hop traversal ≡ returning the starting vertices themselves.
    let rows = run_query("MATCH (n:person) RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
}

// ═══════════════════════════════════════════════════════════════════════════════
// 5. Complex Composition Tests
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().has("name","marko").out("created").in("created").has("name",neq("marko")).values("name")
/// — co-creators: josh, peter
#[test]
fn test_co_creators() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-[:created]->(s)<-[:created]-(other) \
         WHERE other.name <> 'marko' \
         RETURN DISTINCT other.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("peter".into()));
}

/// g.V().has("name","marko").out("knows").out("created").values("name")
/// — what marko's friends created: ripple, lop
#[test]
fn test_friends_created() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-[:knows]->(f)-[:created]->(s) \
         RETURN s.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

/// g.V().has("name","marko").out("knows").order().by("age").values("name")
/// — vadas(27), josh(32)
#[test]
fn test_knows_ordered_by_age() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-[:knows]->(m) RETURN m.name AS name ORDER BY m.age",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("vadas".into()));
    assert_eq!(rows[1]["name"], Value::Str("josh".into()));
}

/// g.V().hasLabel("person").order().by("age",desc).limit(1).values("name") — "peter"
#[test]
fn test_oldest_person() {
    let rows = run_query(
        "MATCH (n:person) RETURN n.name AS name ORDER BY n.age DESC LIMIT 1",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("peter".into()));
}

/// g.V().hasLabel("person").where(out("created").has("name","lop")).values("name")
/// — marko, josh, peter (lop creators)
#[test]
fn test_lop_creators_via_where() {
    let rows = run_query(
        "MATCH (n:person)-[:created]->({name: 'lop'}) RETURN n.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
}

/// g.V().as("a").out("created").as("b").select("a","b").by("name")
/// — creator-software pairs
#[test]
fn test_creator_software_pairs() {
    let rows = run_query(
        "MATCH (a)-[:created]->(b) \
         RETURN a.name AS creator, b.name AS software \
         ORDER BY creator, software",
    );
    assert_eq!(rows.len(), 4);
    // josh→lop, josh→ripple, marko→lop, peter→lop
    assert_eq!(rows[0]["creator"], Value::Str("josh".into()));
    assert_eq!(rows[0]["software"], Value::Str("lop".into()));
    assert_eq!(rows[1]["creator"], Value::Str("josh".into()));
    assert_eq!(rows[1]["software"], Value::Str("ripple".into()));
    assert_eq!(rows[2]["creator"], Value::Str("marko".into()));
    assert_eq!(rows[2]["software"], Value::Str("lop".into()));
    assert_eq!(rows[3]["creator"], Value::Str("peter".into()));
    assert_eq!(rows[3]["software"], Value::Str("lop".into()));
}
