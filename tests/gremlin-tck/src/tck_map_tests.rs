//! Gremlin TCK-equivalent tests — map, aggregation, order, select, and path steps.
//!
//! Each test mirrors a Gremlin traversal from the TinkerPop TCK, expressed as
//! an equivalent Cypher query executed through the cypher-core pipeline against
//! the "modern" graph fixture.

use std::collections::HashMap;
use std::time::Duration;

use cypher_core::executor::{ExecutionBudget, QueryExecutor};
use cypher_core::parser::parse_query;
use cypher_core::planner::QueryPlanner;
use cypher_core::types::Value;

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

// ═══════════════════════════════════════════════════════════════════════════════
// §1  Values / Properties  (Gremlin .values() / .properties() map steps)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().values("name") — 6 names total (4 persons + 2 software).
#[test]
fn test_values_name_all_vertices() {
    let rows = run_query("MATCH (n) RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 6);
    let names: Vec<&Value> = rows.iter().map(|r| &r["name"]).collect();
    assert_eq!(names[0], &Value::Str("josh".into()));
    assert_eq!(names[1], &Value::Str("lop".into()));
    assert_eq!(names[2], &Value::Str("marko".into()));
    assert_eq!(names[3], &Value::Str("peter".into()));
    assert_eq!(names[4], &Value::Str("ripple".into()));
    assert_eq!(names[5], &Value::Str("vadas".into()));
}

/// g.V().has("name","marko").values("name") — ["marko"]
#[test]
fn test_values_name_marko() {
    let rows = run_query("MATCH (n {name: 'marko'}) RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
}

/// g.V().has("name","marko").values("name","age") — marko, 29
#[test]
fn test_values_name_and_age_marko() {
    let rows = run_query("MATCH (n {name: 'marko'}) RETURN n.name AS name, n.age AS age");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
    assert_eq!(rows[0]["age"], Value::Int(29));
}

/// g.V().hasLabel("person").values("age") — [29, 27, 32, 35] (sorted)
#[test]
fn test_values_age_persons() {
    let rows = run_query("MATCH (n:person) RETURN n.age AS age ORDER BY age");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["age"], Value::Int(27));
    assert_eq!(rows[1]["age"], Value::Int(29));
    assert_eq!(rows[2]["age"], Value::Int(32));
    assert_eq!(rows[3]["age"], Value::Int(35));
}

/// g.V().hasLabel("software").values("lang") — ["java", "java"]
#[test]
fn test_values_lang_software() {
    let rows = run_query("MATCH (n:software) RETURN n.lang AS lang ORDER BY lang");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["lang"], Value::Str("java".into()));
    assert_eq!(rows[1]["lang"], Value::Str("java".into()));
}

/// g.V().has("name","marko").values() — all properties of marko (name + age)
#[test]
fn test_values_all_properties_marko() {
    let rows = run_query("MATCH (n {name: 'marko'}) RETURN n.name AS name, n.age AS age");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
    assert_eq!(rows[0]["age"], Value::Int(29));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §2  Count / Sum / Min / Max / Mean  (Gremlin reducing-barrier steps)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().count() → 6
#[test]
fn test_count_all_vertices() {
    let rows = run_query("MATCH (n) RETURN count(n) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(6));
}

/// g.E().count() → 6
#[test]
fn test_count_all_edges() {
    let rows = run_query("MATCH ()-[r]->() RETURN count(r) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(6));
}

/// g.V().hasLabel("person").count() → 4
#[test]
fn test_count_persons() {
    let rows = run_query("MATCH (n:person) RETURN count(n) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(4));
}

/// g.V().hasLabel("software").count() → 2
#[test]
fn test_count_software() {
    let rows = run_query("MATCH (n:software) RETURN count(n) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(2));
}

/// g.V().has("name","marko").out().count() → 3
#[test]
fn test_count_marko_out() {
    let rows = run_query("MATCH ({name: 'marko'})-->(m) RETURN count(m) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(3));
}

/// g.V().has("name","marko").out("knows").count() → 2
#[test]
fn test_count_marko_out_knows() {
    let rows = run_query("MATCH ({name: 'marko'})-[:knows]->(m) RETURN count(m) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(2));
}

/// g.V().hasLabel("person").values("age").min() → 27
#[test]
fn test_min_age_persons() {
    let rows = run_query("MATCH (n:person) RETURN min(n.age) AS min_age");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["min_age"], Value::Int(27));
}

/// g.V().hasLabel("person").values("age").max() → 35
#[test]
fn test_max_age_persons() {
    let rows = run_query("MATCH (n:person) RETURN max(n.age) AS max_age");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["max_age"], Value::Int(35));
}

/// g.V().hasLabel("person").values("age").sum() → 123 (29+27+32+35)
#[test]
fn test_sum_age_persons() {
    let rows = run_query("MATCH (n:person) RETURN sum(n.age) AS total");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["total"], Value::Int(123));
}

/// g.V().hasLabel("person").values("age").mean() → 30.75
#[test]
fn test_mean_age_persons() {
    let rows = run_query("MATCH (n:person) RETURN avg(n.age) AS mean_age");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["mean_age"], Value::Float(30.75));
}

/// g.E().values("weight").sum() → 3.5 (0.5+1.0+0.4+1.0+0.4+0.2)
#[test]
fn test_sum_edge_weights() {
    let rows = run_query("MATCH ()-[r]->() RETURN sum(r.weight) AS total");
    assert_eq!(rows.len(), 1);
    // Float comparison: 0.5 + 1.0 + 0.4 + 1.0 + 0.4 + 0.2 = 3.5
    match &rows[0]["total"] {
        Value::Float(f) => assert!((*f - 3.5).abs() < 1e-9, "expected 3.5, got {f}"),
        other => panic!("expected Float, got {other:?}"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §3  Group / GroupCount  (Gremlin group() / groupCount() side-effect steps)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().groupCount().by(label) → {person: 4, software: 2}
#[test]
fn test_group_count_by_label() {
    let rows = run_query("MATCH (n) RETURN labels(n)[0] AS lbl, count(n) AS cnt ORDER BY lbl");
    assert_eq!(rows.len(), 2);
    let person = rows
        .iter()
        .find(|r| r["lbl"] == Value::Str("person".into()));
    let software = rows
        .iter()
        .find(|r| r["lbl"] == Value::Str("software".into()));
    assert_eq!(person.unwrap()["cnt"], Value::Int(4));
    assert_eq!(software.unwrap()["cnt"], Value::Int(2));
}

/// g.V().group().by(label) — verify person group has 4 names, software has 2
#[test]
fn test_group_by_label_collect_names() {
    let rows =
        run_query("MATCH (n) RETURN labels(n)[0] AS lbl, collect(n.name) AS names ORDER BY lbl");
    assert_eq!(rows.len(), 2);
    for row in &rows {
        match &row["lbl"] {
            Value::Str(s) if &**s == "person" => {
                if let Value::List(names) = &row["names"] {
                    assert_eq!(names.len(), 4);
                } else {
                    panic!("expected List for person names");
                }
            }
            Value::Str(s) if &**s == "software" => {
                if let Value::List(names) = &row["names"] {
                    assert_eq!(names.len(), 2);
                } else {
                    panic!("expected List for software names");
                }
            }
            other => panic!("unexpected label: {other:?}"),
        }
    }
}

/// g.V().hasLabel("person").group().by("age") — each age maps to exactly one person
#[test]
fn test_group_person_by_age() {
    let rows =
        run_query("MATCH (n:person) RETURN n.age AS age, collect(n.name) AS names ORDER BY age");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["age"], Value::Int(27));
    assert_eq!(rows[1]["age"], Value::Int(29));
    assert_eq!(rows[2]["age"], Value::Int(32));
    assert_eq!(rows[3]["age"], Value::Int(35));
    // Each age group should contain exactly one person.
    for row in &rows {
        if let Value::List(names) = &row["names"] {
            assert_eq!(names.len(), 1, "each age should map to one person");
        } else {
            panic!("expected List");
        }
    }
}

/// g.E().groupCount().by(label) → {knows: 2, created: 4}
#[test]
fn test_edge_group_count_by_type() {
    let rows = run_query("MATCH ()-[r]->() RETURN type(r) AS t, count(r) AS cnt ORDER BY t");
    assert_eq!(rows.len(), 2);
    let created = rows.iter().find(|r| r["t"] == Value::Str("created".into()));
    let knows = rows.iter().find(|r| r["t"] == Value::Str("knows".into()));
    assert_eq!(created.unwrap()["cnt"], Value::Int(4));
    assert_eq!(knows.unwrap()["cnt"], Value::Int(2));
}

/// g.V().out().groupCount().by("name") — count how many times each target appears
#[test]
fn test_out_group_count_by_name() {
    let rows = run_query("MATCH ()-->(m) RETURN m.name AS name, count(m) AS cnt ORDER BY name");
    // lop is created by marko, josh, peter → 3; josh known by marko → 1;
    // vadas known by marko → 1; ripple created by josh → 1.
    let lop = rows.iter().find(|r| r["name"] == Value::Str("lop".into()));
    assert_eq!(lop.unwrap()["cnt"], Value::Int(3));
    let josh = rows.iter().find(|r| r["name"] == Value::Str("josh".into()));
    assert_eq!(josh.unwrap()["cnt"], Value::Int(1));
    let vadas = rows
        .iter()
        .find(|r| r["name"] == Value::Str("vadas".into()));
    assert_eq!(vadas.unwrap()["cnt"], Value::Int(1));
    let ripple = rows
        .iter()
        .find(|r| r["name"] == Value::Str("ripple".into()));
    assert_eq!(ripple.unwrap()["cnt"], Value::Int(1));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §4  Order / Limit / Range / Dedup
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().hasLabel("person").order().by("age") → vadas(27), marko(29), josh(32), peter(35)
#[test]
fn test_order_persons_by_age_asc() {
    let rows = run_query("MATCH (n:person) RETURN n.name AS name, n.age AS age ORDER BY n.age");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("vadas".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("josh".into()));
    assert_eq!(rows[3]["name"], Value::Str("peter".into()));
}

/// g.V().hasLabel("person").order().by("age", desc) → peter, josh, marko, vadas
#[test]
fn test_order_persons_by_age_desc() {
    let rows =
        run_query("MATCH (n:person) RETURN n.name AS name, n.age AS age ORDER BY n.age DESC");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("peter".into()));
    assert_eq!(rows[1]["name"], Value::Str("josh".into()));
    assert_eq!(rows[2]["name"], Value::Str("marko".into()));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
}

/// g.V().hasLabel("person").order().by("name") → alphabetical
#[test]
fn test_order_persons_by_name() {
    let rows = run_query("MATCH (n:person) RETURN n.name AS name ORDER BY n.name");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
}

/// g.V().order().by("name").limit(3) → first 3 alphabetically
#[test]
fn test_order_by_name_limit_3() {
    let rows = run_query("MATCH (n) RETURN n.name AS name ORDER BY n.name LIMIT 3");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("lop".into()));
    assert_eq!(rows[2]["name"], Value::Str("marko".into()));
}

/// g.V().order().by("name").range(1, 3) → skip 1, take 2
#[test]
fn test_order_by_name_skip_1_limit_2() {
    let rows = run_query("MATCH (n) RETURN n.name AS name ORDER BY n.name SKIP 1 LIMIT 2");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
}

/// g.V().hasLabel("person").values("age").dedup() — 4 unique ages
#[test]
fn test_dedup_person_ages() {
    let rows = run_query("MATCH (n:person) RETURN DISTINCT n.age AS age ORDER BY age");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["age"], Value::Int(27));
    assert_eq!(rows[1]["age"], Value::Int(29));
    assert_eq!(rows[2]["age"], Value::Int(32));
    assert_eq!(rows[3]["age"], Value::Int(35));
}

/// g.V().out().dedup() — unique outgoing neighbors
#[test]
fn test_dedup_out_neighbors() {
    let rows = run_query("MATCH ()-->(m) RETURN DISTINCT m.name AS name ORDER BY name");
    // Targets: vadas, josh, lop(×3), ripple → distinct = 4
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("lop".into()));
    assert_eq!(rows[2]["name"], Value::Str("ripple".into()));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
}

/// g.V().out().out().dedup().count() — unique 2-hop targets from all vertices
#[test]
fn test_dedup_two_hop_count() {
    // From marko: out→{vadas,josh,lop}, then from josh: out→{ripple,lop}
    // From josh: out→{ripple,lop}, then from ripple/lop: no outgoing
    // Unique 2-hop: ripple, lop → 2 distinct targets
    let rows = run_query("MATCH ()-->(m)-->(t) RETURN COUNT(DISTINCT t) AS cnt");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(2));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §5  Select / Project / Fold
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().has("name","marko").as("a").out("knows").as("b").select("a","b")
/// → marko->{vadas, josh}
#[test]
fn test_select_marko_knows() {
    let rows = run_query(
        "MATCH (a {name: 'marko'})-[:knows]->(b) \
         RETURN a.name AS a_name, b.name AS b_name ORDER BY b_name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["a_name"], Value::Str("marko".into()));
    assert_eq!(rows[0]["b_name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["a_name"], Value::Str("marko".into()));
    assert_eq!(rows[1]["b_name"], Value::Str("vadas".into()));
}

/// g.V().has("name","marko").out().values("name").fold()
/// → collected list of marko's outgoing neighbor names
#[test]
fn test_fold_marko_out_names() {
    let rows = run_query("MATCH ({name: 'marko'})-->(m) RETURN collect(m.name) AS names");
    assert_eq!(rows.len(), 1);
    if let Value::List(names) = &rows[0]["names"] {
        assert_eq!(names.len(), 3);
        let mut sorted: Vec<&Value> = names.iter().collect();
        sorted.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
        assert!(names.contains(&Value::Str("josh".into())));
        assert!(names.contains(&Value::Str("lop".into())));
        assert!(names.contains(&Value::Str("vadas".into())));
    } else {
        panic!("expected List for collected names");
    }
}

/// g.V().hasLabel("person").values("name").order() → sorted person names
#[test]
fn test_sorted_person_names() {
    let rows = run_query("MATCH (n:person) RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
}

/// Select with three variables: creator → software → language
#[test]
fn test_select_three_variables() {
    let rows = run_query(
        "MATCH (p:person)-[:created]->(s:software) \
         RETURN p.name AS person, s.name AS sw, s.lang AS lang ORDER BY person, sw",
    );
    // marko→lop, josh→ripple, josh→lop, peter→lop
    assert_eq!(rows.len(), 4);
    // Verify all rows have lang = java
    for row in &rows {
        assert_eq!(row["lang"], Value::Str("java".into()));
    }
}

/// Project a computed column: person name and whether their age is over 30
#[test]
fn test_project_age_comparison() {
    let rows =
        run_query("MATCH (n:person) RETURN n.name AS name, n.age > 30 AS over30 ORDER BY n.name");
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[0]["over30"], Value::Bool(true));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[1]["over30"], Value::Bool(false));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
    assert_eq!(rows[2]["over30"], Value::Bool(true));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
    assert_eq!(rows[3]["over30"], Value::Bool(false));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §6  Path  (Gremlin path() step — important concept)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().has("name","marko").out("knows").path()
/// → paths: marko→vadas, marko→josh (represented as source + target names)
#[test]
fn test_path_marko_knows() {
    let rows = run_query(
        "MATCH (a {name: 'marko'})-[:knows]->(b) \
         RETURN a.name AS src, b.name AS dst ORDER BY dst",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["src"], Value::Str("marko".into()));
    assert_eq!(rows[0]["dst"], Value::Str("josh".into()));
    assert_eq!(rows[1]["src"], Value::Str("marko".into()));
    assert_eq!(rows[1]["dst"], Value::Str("vadas".into()));
}

/// g.V().has("name","marko").out().out().path()
/// → 2-hop paths through josh: marko→josh→ripple, marko→josh→lop
#[test]
fn test_path_marko_two_hop() {
    let rows = run_query(
        "MATCH (a {name: 'marko'})-->(b)-->(c) \
         RETURN a.name AS hop0, b.name AS hop1, c.name AS hop2 ORDER BY hop2",
    );
    assert_eq!(rows.len(), 2);
    // Both paths go through josh
    assert_eq!(rows[0]["hop0"], Value::Str("marko".into()));
    assert_eq!(rows[0]["hop1"], Value::Str("josh".into()));
    assert_eq!(rows[0]["hop2"], Value::Str("lop".into()));
    assert_eq!(rows[1]["hop0"], Value::Str("marko".into()));
    assert_eq!(rows[1]["hop1"], Value::Str("josh".into()));
    assert_eq!(rows[1]["hop2"], Value::Str("ripple".into()));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §7  Additional coverage — mixed traversal + aggregation combos
// ═══════════════════════════════════════════════════════════════════════════════

/// Combine filter + aggregation: average age of people who created software
#[test]
fn test_avg_age_creators() {
    // Creators: marko(29), josh(32), peter(35) → avg = 32.0
    let rows = run_query(
        "MATCH (p:person)-[:created]->(:software) \
         RETURN avg(p.age) AS avg_age",
    );
    assert_eq!(rows.len(), 1);
    match &rows[0]["avg_age"] {
        Value::Float(f) => assert!((*f - 32.0).abs() < 1e-9, "expected 32.0, got {f}"),
        other => panic!("expected Float, got {other:?}"),
    }
}

/// Min weight across all edges
#[test]
fn test_min_edge_weight() {
    let rows = run_query("MATCH ()-[r]->() RETURN min(r.weight) AS min_w");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["min_w"], Value::Float(0.2));
}

/// Max weight across all edges
#[test]
fn test_max_edge_weight() {
    let rows = run_query("MATCH ()-[r]->() RETURN max(r.weight) AS max_w");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["max_w"], Value::Float(1.0));
}

/// Count edges per source vertex (out-degree)
#[test]
fn test_out_degree_per_vertex() {
    let rows =
        run_query("MATCH (n)-->(m) RETURN n.name AS name, count(m) AS out_deg ORDER BY name");
    // josh: 2 (created ripple, created lop)
    // marko: 3 (knows vadas, knows josh, created lop)
    // peter: 1 (created lop)
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[0]["out_deg"], Value::Int(2));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
    assert_eq!(rows[1]["out_deg"], Value::Int(3));
    assert_eq!(rows[2]["name"], Value::Str("peter".into()));
    assert_eq!(rows[2]["out_deg"], Value::Int(1));
}

/// Count incoming edges per vertex (in-degree)
#[test]
fn test_in_degree_per_vertex() {
    let rows = run_query("MATCH (m)-->(n) RETURN n.name AS name, count(m) AS in_deg ORDER BY name");
    // josh: 1 (known by marko)
    // lop: 3 (created by marko, josh, peter)
    // ripple: 1 (created by josh)
    // vadas: 1 (known by marko)
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[0]["in_deg"], Value::Int(1));
    assert_eq!(rows[1]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["in_deg"], Value::Int(3));
    assert_eq!(rows[2]["name"], Value::Str("ripple".into()));
    assert_eq!(rows[2]["in_deg"], Value::Int(1));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
    assert_eq!(rows[3]["in_deg"], Value::Int(1));
}

/// Collect edge weights per source vertex
#[test]
fn test_collect_weights_per_source() {
    let rows = run_query(
        "MATCH (n)-[r]->(m) \
         RETURN n.name AS name, collect(r.weight) AS weights ORDER BY name",
    );
    assert_eq!(rows.len(), 3);
    // josh: [1.0, 0.4] or [0.4, 1.0]
    if let Value::List(w) = &rows[0]["weights"] {
        assert_eq!(w.len(), 2, "josh should have 2 edges");
    }
    // marko: 3 edges
    if let Value::List(w) = &rows[1]["weights"] {
        assert_eq!(w.len(), 3, "marko should have 3 edges");
    }
    // peter: 1 edge
    if let Value::List(w) = &rows[2]["weights"] {
        assert_eq!(w.len(), 1, "peter should have 1 edge");
        assert_eq!(w[0], Value::Float(0.2));
    }
}

/// Limit with ORDER BY DESC — top 2 oldest persons
#[test]
fn test_top_2_oldest() {
    let rows = run_query(
        "MATCH (n:person) RETURN n.name AS name, n.age AS age ORDER BY n.age DESC LIMIT 2",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("peter".into()));
    assert_eq!(rows[0]["age"], Value::Int(35));
    assert_eq!(rows[1]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["age"], Value::Int(32));
}

/// Distinct edge types across the graph
#[test]
fn test_distinct_edge_types() {
    let rows = run_query("MATCH ()-[r]->() RETURN DISTINCT type(r) AS t ORDER BY t");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["t"], Value::Str("created".into()));
    assert_eq!(rows[1]["t"], Value::Str("knows".into()));
}

/// Distinct labels across all vertices
#[test]
fn test_distinct_vertex_labels() {
    let rows = run_query("MATCH (n) RETURN DISTINCT labels(n)[0] AS lbl ORDER BY lbl");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["lbl"], Value::Str("person".into()));
    assert_eq!(rows[1]["lbl"], Value::Str("software".into()));
}

/// Vertices with no outgoing edges (sinks)
#[test]
fn test_sink_vertices() {
    // vadas, lop, ripple have no outgoing edges.
    let rows = run_query("MATCH (n) WHERE NOT (n)-->() RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
    assert_eq!(rows[2]["name"], Value::Str("vadas".into()));
}

/// Vertices with no incoming edges (sources)
#[test]
fn test_source_vertices() {
    // marko and peter have no incoming edges.
    let rows = run_query("MATCH (n) WHERE NOT ()-->(n) RETURN n.name AS name ORDER BY name");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
    assert_eq!(rows[1]["name"], Value::Str("peter".into()));
}
