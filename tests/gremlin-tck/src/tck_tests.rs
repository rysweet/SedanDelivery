//! Gremlin TCK-equivalent tests.
//!
//! Each test mirrors a Gremlin traversal from the TinkerPop TCK, expressed as
//! an equivalent Cypher query executed through the gql-core pipeline against
//! the "modern" graph fixture.

use std::collections::HashMap;
use std::time::Duration;

use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::types::Value;

use crate::modern_graph::create_modern_graph;

fn unlimited() -> ExecutionBudget {
    ExecutionBudget::new(Duration::from_secs(3600), usize::MAX)
}

/// Helper: parse → plan → execute a Cypher query against the modern graph.
fn run_query(query: &str) -> Vec<HashMap<String, Value>> {
    let (store, _ids) = create_modern_graph();
    let ast = parse_query(query).expect("parse failed");
    let planner = QueryPlanner::new(&store, "modern");
    let plan = planner.plan(&ast).expect("plan failed");
    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor
        .execute("modern", &plan, &mut budget)
        .expect("execute failed");
    result.rows
}

// ── Count.feature equivalents ──────────────────────────────────────────────────

/// g.V().count() → 6
#[test]
fn test_vertex_count() {
    let rows = run_query("MATCH (n) RETURN count(n) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(6));
}

/// g.E().count() → 6
#[test]
fn test_edge_count() {
    let rows = run_query("MATCH ()-[r]->() RETURN count(r) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(6));
}

// ── HasStep.feature equivalents ────────────────────────────────────────────────

/// g.V().hasLabel("person") → 4 vertices
#[test]
fn test_has_label_person() {
    let rows = run_query("MATCH (n:person) RETURN count(n) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(4));
}

/// g.V().hasLabel("software") → 2 vertices
#[test]
fn test_has_label_software() {
    let rows = run_query("MATCH (n:software) RETURN count(n) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(2));
}

/// g.V().has("name", "marko") → 1 vertex
#[test]
fn test_has_property_name_marko() {
    let rows = run_query("MATCH (n {name: 'marko'}) RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
}

/// g.V().has("person", "age", P.gt(30)) → josh (32), peter (35)
#[test]
fn test_has_property_age_gt_30() {
    let rows = run_query("MATCH (n:person) WHERE n.age > 30 RETURN n.name AS name ORDER BY n.name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("peter".into()));
}

// ── Traversal / Path equivalents ───────────────────────────────────────────────

/// g.V().has("name","marko").out("knows").values("name") → {vadas, josh}
#[test]
fn test_traversal_out_knows() {
    let rows =
        run_query("MATCH (n {name: 'marko'})-[:knows]->(m) RETURN m.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("vadas".into()));
}

/// g.V().has("name","marko").outE("knows").values("weight") → {0.5, 1.0}
#[test]
fn test_edge_properties() {
    let rows = run_query("MATCH ({name: 'marko'})-[r:knows]->() RETURN r.weight AS w ORDER BY w");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["w"], Value::Float(0.5));
    assert_eq!(rows[1]["w"], Value::Float(1.0));
}

/// g.V().has("name","josh").out("created").values("name") → {ripple, lop}
#[test]
fn test_josh_created() {
    let rows =
        run_query("MATCH ({name: 'josh'})-[:created]->(s) RETURN s.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

/// g.V().has("name","lop").in("created").values("name") → {marko, josh, peter}
#[test]
fn test_lop_creators() {
    let rows =
        run_query("MATCH (c)-[:created]->({name: 'lop'}) RETURN c.name AS name ORDER BY name");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
}

// ── GroupCount.feature equivalent ──────────────────────────────────────────────

/// g.V().groupCount().by(label) → {person: 4, software: 2}
#[test]
fn test_group_count_by_label() {
    let rows = run_query("MATCH (n) RETURN labels(n)[0] AS lbl, count(n) AS cnt ORDER BY lbl");
    assert_eq!(rows.len(), 2);

    let person_row = rows
        .iter()
        .find(|r| r.get("lbl") == Some(&Value::Str("person".into())));
    assert!(person_row.is_some(), "person label should be present");
    assert_eq!(person_row.unwrap().get("cnt"), Some(&Value::Int(4)));

    let software_row = rows
        .iter()
        .find(|r| r.get("lbl") == Some(&Value::Str("software".into())));
    assert!(software_row.is_some(), "software label should be present");
    assert_eq!(software_row.unwrap().get("cnt"), Some(&Value::Int(2)));
}

// ── Order.feature equivalents ──────────────────────────────────────────────────

/// g.V().hasLabel("person").order().by("age") → vadas(27), marko(29), josh(32), peter(35)
#[test]
fn test_order_by_age() {
    let rows = run_query("MATCH (n:person) RETURN n.name AS name, n.age AS age ORDER BY n.age");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("vadas".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("josh".into()));
    assert_eq!(rows[3]["name"], Value::Str("peter".into()));
}

/// g.V().hasLabel("person").order().by("name", desc) → vadas, peter, marko, josh
#[test]
fn test_order_by_name_desc() {
    let rows = run_query("MATCH (n:person) RETURN n.name AS name ORDER BY n.name DESC");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("vadas".into()));
    assert_eq!(rows[1]["name"], Value::Str("peter".into()));
    assert_eq!(rows[2]["name"], Value::Str("marko".into()));
    assert_eq!(rows[3]["name"], Value::Str("josh".into()));
}

// ── Select.feature equivalents ─────────────────────────────────────────────────

/// g.V().has("name","marko").as("a").out("knows").as("b").select("a","b").by("name")
/// → [{a: marko, b: vadas}, {a: marko, b: josh}]
#[test]
fn test_select_traversal_names() {
    let rows = run_query(
        "MATCH (a {name: 'marko'})-[:knows]->(b) RETURN a.name AS a_name, b.name AS b_name ORDER BY b_name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["a_name"], Value::Str("marko".into()));
    assert_eq!(rows[0]["b_name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["a_name"], Value::Str("marko".into()));
    assert_eq!(rows[1]["b_name"], Value::Str("vadas".into()));
}

// ── AddVertex.feature equivalent ───────────────────────────────────────────────

/// g.addV("animal").property("name","puppy") — verify CREATE adds a vertex
#[test]
fn test_add_vertex() {
    let (store, _ids) = create_modern_graph();

    let ast = parse_query("CREATE (a:animal {name: 'puppy'}) RETURN a.name AS name")
        .expect("parse failed");
    let planner = QueryPlanner::new(&store, "modern");
    let plan = planner.plan(&ast).expect("plan failed");
    let executor = QueryExecutor::new(&store);
    let mut budget = unlimited();
    let result = executor
        .execute("modern", &plan, &mut budget)
        .expect("execute failed");

    assert_eq!(result.stats.nodes_created, 1);
    assert_eq!(result.rows.len(), 1);
    assert_eq!(result.rows[0]["name"], Value::Str("puppy".into()));

    // Verify total vertex count is now 7.
    let ast2 = parse_query("MATCH (n) RETURN count(n) AS cnt").unwrap();
    let plan2 = QueryPlanner::new(&store, "modern").plan(&ast2).unwrap();
    let result2 = QueryExecutor::new(&store)
        .execute("modern", &plan2, &mut unlimited())
        .unwrap();
    assert_eq!(result2.rows[0]["cnt"], Value::Int(7));
}
