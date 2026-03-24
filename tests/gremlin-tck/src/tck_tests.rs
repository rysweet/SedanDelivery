//! Gremlin TCK-equivalent tests.
//!
//! Each test mirrors a Gremlin traversal from the TinkerPop TCK, expressed as
//! an equivalent Cypher query executed through the gql-core pipeline against
//! the "modern" graph fixture.

use indexmap::IndexMap;
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
fn run_query(query: &str) -> Vec<IndexMap<String, Value>> {
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

// ── Vertex Step Tests (g.V variants) ───────────────────────────────────────────

/// g.V() — all vertices → 6
#[test]
fn test_g_v_all_vertices() {
    let rows = run_query("MATCH (n) RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 6);
}

/// g.V(marko_id) — single vertex by property lookup
#[test]
fn test_g_v_single_vertex_marko() {
    let rows = run_query("MATCH (n {name: 'marko'}) RETURN n.name AS name, n.age AS age");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
    assert_eq!(rows[0]["age"], Value::Int(29));
}

/// g.V().hasLabel("person") → 4 persons
#[test]
fn test_g_v_has_label_person_count() {
    let rows = run_query("MATCH (n:person) RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
}

/// g.V().hasLabel("software") → 2 software
#[test]
fn test_g_v_has_label_software_names() {
    let rows = run_query("MATCH (n:software) RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

/// g.V().has("name") — all 6 vertices have the name property
#[test]
fn test_g_v_has_name_property() {
    let rows = run_query("MATCH (n) WHERE n.name IS NOT NULL RETURN n.name AS name");
    assert_eq!(rows.len(), 6);
}

/// g.V().has("age") — 4 persons have the age property
#[test]
fn test_g_v_has_age_property() {
    let rows = run_query("MATCH (n) WHERE n.age IS NOT NULL RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
}

/// g.V().has("lang") — 2 software vertices have the lang property
#[test]
fn test_g_v_has_lang_property() {
    let rows = run_query("MATCH (n) WHERE n.lang IS NOT NULL RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

/// g.V().has("name", "marko") → 1
#[test]
fn test_g_v_has_name_eq_marko() {
    let rows = run_query("MATCH (n) WHERE n.name = 'marko' RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
}

/// g.V().has("person", "name", "marko") → 1 (label + property)
#[test]
fn test_g_v_has_label_person_name_marko() {
    let rows =
        run_query("MATCH (n:person) WHERE n.name = 'marko' RETURN n.name AS name, n.age AS age");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
    assert_eq!(rows[0]["age"], Value::Int(29));
}

/// g.V().has("person", "age", 29) → 1 (marko)
#[test]
fn test_g_v_has_person_age_29() {
    let rows = run_query("MATCH (n:person) WHERE n.age = 29 RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
}

/// g.V().hasNot("age") → 2 (lop, ripple)
#[test]
fn test_g_v_has_not_age() {
    let rows = run_query("MATCH (n) WHERE n.age IS NULL RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

/// g.V().has("age", gt(30)) → josh(32) + peter(35)
#[test]
fn test_g_v_has_age_gt_30() {
    let rows = run_query("MATCH (n) WHERE n.age > 30 RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("peter".into()));
}

/// g.V().has("age", gte(32)) → josh(32) + peter(35)
#[test]
fn test_g_v_has_age_gte_32() {
    let rows = run_query("MATCH (n) WHERE n.age >= 32 RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("peter".into()));
}

/// g.V().has("age", lt(29)) → vadas(27)
#[test]
fn test_g_v_has_age_lt_29() {
    let rows = run_query("MATCH (n) WHERE n.age < 29 RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("vadas".into()));
}

/// g.V().has("age", lte(27)) → vadas(27)
#[test]
fn test_g_v_has_age_lte_27() {
    let rows = run_query("MATCH (n) WHERE n.age <= 27 RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("vadas".into()));
}

/// g.V().has("age", between(28, 33)) → marko(29) + josh(32)
#[test]
fn test_g_v_has_age_between_28_33() {
    let rows =
        run_query("MATCH (n) WHERE n.age >= 28 AND n.age < 33 RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
}

/// g.V().has("age", within(27, 32)) → vadas + josh
#[test]
fn test_g_v_has_age_within_27_32() {
    let rows = run_query("MATCH (n) WHERE n.age IN [27, 32] RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("vadas".into()));
}

/// g.V().count() → 6
#[test]
fn test_g_v_count() {
    let rows = run_query("MATCH (n) RETURN count(n) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(6));
}

/// g.V().id() → 6 IDs
#[test]
fn test_g_v_id() {
    let rows = run_query("MATCH (n) RETURN id(n) AS nid");
    assert_eq!(rows.len(), 6);
    for row in &rows {
        assert!(row.contains_key("nid"), "each row should have an id");
    }
}

/// g.V().label() → person/software labels for all 6 vertices
#[test]
fn test_g_v_label() {
    let rows = run_query("MATCH (n) RETURN labels(n)[0] AS lbl ORDER BY lbl");
    assert_eq!(rows.len(), 6);
    let person_count = rows
        .iter()
        .filter(|r| r.get("lbl") == Some(&Value::Str("person".into())))
        .count();
    let software_count = rows
        .iter()
        .filter(|r| r.get("lbl") == Some(&Value::Str("software".into())))
        .count();
    assert_eq!(person_count, 4);
    assert_eq!(software_count, 2);
}

// ── Edge Step Tests (g.E variants) ─────────────────────────────────────────────

/// g.E() — all 6 edges
#[test]
fn test_g_e_all_edges() {
    let rows = run_query("MATCH ()-[r]->() RETURN r.weight AS w");
    assert_eq!(rows.len(), 6);
}

/// g.E().count() → 6
#[test]
fn test_g_e_count() {
    let rows = run_query("MATCH ()-[r]->() RETURN count(r) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(6));
}

/// g.E().hasLabel("knows") → 2
#[test]
fn test_g_e_has_label_knows() {
    let rows = run_query("MATCH ()-[r:knows]->() RETURN r.weight AS w ORDER BY w");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["w"], Value::Float(0.5));
    assert_eq!(rows[1]["w"], Value::Float(1.0));
}

/// g.E().hasLabel("created") → 4
#[test]
fn test_g_e_has_label_created() {
    let rows = run_query("MATCH ()-[r:created]->() RETURN r.weight AS w ORDER BY w");
    assert_eq!(rows.len(), 4);
}

/// g.E().has("weight", 1.0) → 2 edges (marko→josh knows, josh→ripple created)
#[test]
fn test_g_e_has_weight_1_0() {
    let rows = run_query(
        "MATCH (a)-[r]->(b) WHERE r.weight = 1.0 RETURN a.name AS src, b.name AS dst ORDER BY src",
    );
    assert_eq!(rows.len(), 2);
}

/// g.E().has("weight", gt(0.5)) → edges with weight > 0.5
#[test]
fn test_g_e_has_weight_gt_0_5() {
    let rows = run_query("MATCH ()-[r]->() WHERE r.weight > 0.5 RETURN r.weight AS w ORDER BY w");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["w"], Value::Float(1.0));
    assert_eq!(rows[1]["w"], Value::Float(1.0));
}

// ── Traversal Tests (out/in/both) ──────────────────────────────────────────────

/// g.V().has("name","marko").out() → 3 (vadas, josh, lop)
#[test]
fn test_g_v_has_name_marko_out() {
    let rows = run_query("MATCH ({name: 'marko'})-->(m) RETURN m.name AS name ORDER BY name");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("lop".into()));
    assert_eq!(rows[2]["name"], Value::Str("vadas".into()));
}

/// g.V().has("name","marko").out("knows") → 2 (vadas, josh)
#[test]
fn test_g_v_has_name_marko_out_knows() {
    let rows =
        run_query("MATCH ({name: 'marko'})-[:knows]->(m) RETURN m.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("vadas".into()));
}

/// g.V().has("name","marko").out("created") → 1 (lop)
#[test]
fn test_g_v_has_name_marko_out_created() {
    let rows = run_query("MATCH ({name: 'marko'})-[:created]->(m) RETURN m.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
}

/// g.V().has("name","lop").in() → 3 (marko, josh, peter)
#[test]
fn test_g_v_has_name_lop_in() {
    let rows = run_query("MATCH (c)-->({name: 'lop'}) RETURN c.name AS name ORDER BY name");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
}

/// g.V().has("name","lop").in("created") → 3
#[test]
fn test_g_v_has_name_lop_in_created() {
    let rows =
        run_query("MATCH (c)-[:created]->({name: 'lop'}) RETURN c.name AS name ORDER BY name");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
}

/// g.V().has("name","josh").both() → marko + lop + ripple (3 unique via undirected)
#[test]
fn test_g_v_has_name_josh_both() {
    let rows =
        run_query("MATCH ({name: 'josh'})--(m) RETURN DISTINCT m.name AS name ORDER BY name");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("ripple".into()));
}

/// g.V().has("name","marko").out().out() → 2-hop: ripple, lop (from josh)
#[test]
fn test_g_v_marko_out_out() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-->(mid)-->(dest) RETURN dest.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

/// g.V().has("name","marko").outE() → 3 edges
#[test]
fn test_g_v_has_name_marko_out_e() {
    let rows = run_query("MATCH ({name: 'marko'})-[r]->() RETURN r.weight AS w ORDER BY w");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["w"], Value::Float(0.4));
    assert_eq!(rows[1]["w"], Value::Float(0.5));
    assert_eq!(rows[2]["w"], Value::Float(1.0));
}

/// g.V().has("name","marko").outE("knows") → 2 edges
#[test]
fn test_g_v_has_name_marko_out_e_knows() {
    let rows = run_query("MATCH ({name: 'marko'})-[r:knows]->() RETURN r.weight AS w ORDER BY w");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["w"], Value::Float(0.5));
    assert_eq!(rows[1]["w"], Value::Float(1.0));
}

/// g.V().has("name","marko").outE().inV() → 3 vertices (same as out)
#[test]
fn test_g_v_has_name_marko_out_e_in_v() {
    let rows = run_query("MATCH ({name: 'marko'})-[r]->(m) RETURN m.name AS name ORDER BY name");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("lop".into()));
    assert_eq!(rows[2]["name"], Value::Str("vadas".into()));
}

/// g.V().has("name","marko").out().in() — Cypher enforces edge uniqueness, so
/// the first-hop edge (marko→mid) cannot be reused for the second hop (back→mid).
/// Only mid=lop yields results: josh→lop and peter→lop are distinct from marko→lop.
#[test]
fn test_g_v_has_name_marko_out_in() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-->(mid)<--(back) RETURN back.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("peter".into()));
}

/// g.V().out().out().count() → 2 (only marko→josh→{ripple,lop})
#[test]
fn test_g_v_out_out_count() {
    let rows = run_query("MATCH ()-->(mid)-->(dest) RETURN count(dest) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(2));
}
