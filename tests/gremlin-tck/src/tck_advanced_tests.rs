//! Advanced Gremlin TCK-equivalent tests covering patterns from the full
//! TinkerPop feature file corpus (159 features, 2097 scenarios).
//!
//! These tests exercise map, filter, sideEffect, branch, and semantics
//! categories that are not covered by the existing tck_tests, tck_map_tests,
//! or tck_mutation_tests modules.

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

fn run_query(query: &str) -> Vec<HashMap<String, Value>> {
    let (store, _ids) = create_modern_graph();
    run_on(&store, query).rows
}

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
// §1  Filter — Has variants (Has.feature, HasLabel.feature, HasKey.feature,
//     HasNot.feature, HasValue.feature, HasId.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().has("name","vadas").has("age",27) — combined property filter
#[test]
fn test_filter_has_name_and_age_combined() {
    let rows = run_query("MATCH (n {name: 'vadas', age: 27}) RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("vadas".into()));
}

/// g.V().hasLabel("person").has("age", gt(28)).count() — chained label + predicate
#[test]
fn test_filter_has_label_person_age_gt_28() {
    let rows = run_query(
        "MATCH (n:person) WHERE n.age > 28 RETURN count(n) AS cnt",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(3)); // marko(29), josh(32), peter(35)
}

/// g.V().has("age", neq(29)) — inequality filter
#[test]
fn test_filter_has_age_neq_29() {
    let rows = run_query(
        "MATCH (n:person) WHERE n.age <> 29 RETURN n.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("peter".into()));
    assert_eq!(rows[2]["name"], Value::Str("vadas".into()));
}

/// g.V().hasLabel("person","software") — multi-label HasLabel
#[test]
fn test_filter_has_label_person_or_software() {
    let rows = run_query(
        "MATCH (n) WHERE n:person OR n:software RETURN count(n) AS cnt",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(6));
}

/// HasNot: g.V().hasNot("lang") — vertices without the lang property (persons)
#[test]
fn test_filter_has_not_lang() {
    let rows = run_query(
        "MATCH (n) WHERE n.lang IS NULL RETURN count(n) AS cnt",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(4)); // 4 persons have no lang
}

// ═══════════════════════════════════════════════════════════════════════════════
// §2  Filter — Is, And, Or, Not, Where (Is.feature, And.feature, Or.feature,
//     Not.feature, Where.feature, SimplePath.feature, CyclicPath.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// And: g.V().has("age",gt(27)).has("age",lt(35)) — both conditions
#[test]
fn test_filter_and_age_range() {
    let rows = run_query(
        "MATCH (n:person) WHERE n.age > 27 AND n.age < 35 RETURN n.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
}

/// Or: g.V().or(has("age",29),has("age",32))
#[test]
fn test_filter_or_specific_ages() {
    let rows = run_query(
        "MATCH (n:person) WHERE n.age = 29 OR n.age = 32 RETURN n.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("marko".into()));
}

/// Not: g.V().not(hasLabel("software")) — only persons
#[test]
fn test_filter_not_software() {
    let rows = run_query(
        "MATCH (n) WHERE NOT n:software RETURN count(n) AS cnt",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(4));
}

/// Where: g.V().where(out("created").count().is(gt(1))) — josh has 2 created
#[test]
fn test_filter_where_created_count_gt_1() {
    let rows = run_query(
        "MATCH (n)-[:created]->(s) \
         WITH n, count(s) AS cnt \
         WHERE cnt > 1 \
         RETURN n.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
}

/// SimplePath: g.V().has("name","marko").out().out() — unique vertices only
#[test]
fn test_filter_simple_path_two_hops() {
    let rows = run_query(
        "MATCH (a {name: 'marko'})-[]->(b)-[]->(c) \
         RETURN DISTINCT c.name AS name ORDER BY name",
    );
    // marko->josh->ripple, marko->josh->lop
    let names: Vec<&Value> = rows.iter().map(|r| &r["name"]).collect();
    assert!(names.contains(&&Value::Str("lop".into())));
    assert!(names.contains(&&Value::Str("ripple".into())));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §3  Filter — Dedup, Range, Tail (Dedup.feature, Range.feature, Tail.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// Dedup: g.V().values("lang").dedup() — just "java"
#[test]
fn test_filter_dedup_lang_values() {
    let rows = run_query(
        "MATCH (n:software) RETURN DISTINCT n.lang AS lang",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["lang"], Value::Str("java".into()));
}

/// Range: g.V().hasLabel("person").order().by("age").range(1,3)
#[test]
fn test_filter_range_skip_1_limit_2() {
    let rows = run_query(
        "MATCH (n:person) RETURN n.name AS name ORDER BY n.age SKIP 1 LIMIT 2",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("marko".into())); // age 29
    assert_eq!(rows[1]["name"], Value::Str("josh".into()));  // age 32
}

/// Tail: g.V().order().by("name").tail(2) — last two by name
#[test]
fn test_filter_tail_last_two_by_name() {
    let rows = run_query(
        "MATCH (n) RETURN n.name AS name ORDER BY n.name DESC LIMIT 2",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("vadas".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §4  Map — Constant, Fold, Unfold, Id, Label
//     (Constant.feature, Fold.feature, Unfold.feature, Vertex.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// Constant: g.V().constant(1) — returns constant value per vertex
#[test]
fn test_map_constant_value() {
    let rows = run_query("MATCH (n) RETURN 1 AS one");
    assert_eq!(rows.len(), 6);
    for row in &rows {
        assert_eq!(row["one"], Value::Int(1));
    }
}

/// Fold: g.V().values("name").fold() — collect all names into one list
#[test]
fn test_map_fold_names() {
    let rows = run_query(
        "MATCH (n) RETURN collect(n.name) AS names",
    );
    assert_eq!(rows.len(), 1);
    if let Value::List(ref names) = rows[0]["names"] {
        assert_eq!(names.len(), 6);
    } else {
        panic!("Expected List, got {:?}", rows[0]["names"]);
    }
}

/// Label: g.V().label() — label of each vertex
#[test]
fn test_map_labels() {
    let rows = run_query(
        "MATCH (n) RETURN labels(n) AS lbls",
    );
    assert_eq!(rows.len(), 6);
}

/// Id: vertex IDs are non-null
#[test]
fn test_map_vertex_id_non_null() {
    let rows = run_query("MATCH (n) RETURN id(n) AS nid");
    assert_eq!(rows.len(), 6);
    for row in &rows {
        assert_ne!(row["nid"], Value::Null);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §5  Map — Max, Min, Mean, Sum  (Max.feature, Min.feature, Mean.feature,
//     Sum.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// Max: g.V().values("age").max() — 35 (peter)
#[test]
fn test_map_max_age() {
    let rows = run_query(
        "MATCH (n:person) RETURN max(n.age) AS oldest",
    );
    assert_eq!(rows[0]["oldest"], Value::Int(35));
}

/// Min: g.V().values("age").min() — 27 (vadas)
#[test]
fn test_map_min_age() {
    let rows = run_query(
        "MATCH (n:person) RETURN min(n.age) AS youngest",
    );
    assert_eq!(rows[0]["youngest"], Value::Int(27));
}

/// Sum: g.V().values("age").sum() — 27+29+32+35 = 123
#[test]
fn test_map_sum_age() {
    let rows = run_query(
        "MATCH (n:person) RETURN sum(n.age) AS total",
    );
    assert_eq!(rows[0]["total"], Value::Int(123));
}

/// Mean: g.V().values("age").mean() — 123/4 = 30.75
#[test]
fn test_map_mean_age() {
    let rows = run_query(
        "MATCH (n:person) RETURN avg(n.age) AS average",
    );
    if let Value::Float(avg) = rows[0]["average"] {
        assert!((avg - 30.75).abs() < 0.01);
    } else {
        panic!("Expected Float, got {:?}", rows[0]["average"]);
    }
}

/// Sum of edge weights: g.E().values("weight").sum()
#[test]
fn test_map_sum_edge_weights() {
    let rows = run_query(
        "MATCH ()-[r]->() RETURN sum(r.weight) AS total",
    );
    if let Value::Float(total) = rows[0]["total"] {
        // 0.5 + 1.0 + 0.4 + 1.0 + 0.4 + 0.2 = 3.5
        assert!((total - 3.5).abs() < 0.01);
    } else {
        panic!("Expected Float, got {:?}", rows[0]["total"]);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §6  Map — Count variants (Count.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().hasLabel("person").count() — 4
#[test]
fn test_map_count_persons() {
    let rows = run_query("MATCH (n:person) RETURN count(n) AS cnt");
    assert_eq!(rows[0]["cnt"], Value::Int(4));
}

/// g.V().has("name","marko").out().count() — 3
#[test]
fn test_map_count_marko_out() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-->(t) RETURN count(t) AS cnt",
    );
    assert_eq!(rows[0]["cnt"], Value::Int(3));
}

/// g.E().hasLabel("created").count() — 4
#[test]
fn test_map_count_created_edges() {
    let rows = run_query(
        "MATCH ()-[r:created]->() RETURN count(r) AS cnt",
    );
    assert_eq!(rows[0]["cnt"], Value::Int(4));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §7  Map — Order variants (Order.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// g.V().hasLabel("person").order().by("name",asc) — alphabetical
#[test]
fn test_map_order_person_names_asc() {
    let rows = run_query(
        "MATCH (n:person) RETURN n.name AS name ORDER BY n.name ASC",
    );
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
}

/// g.V().values("age").order().by(asc).limit(1) — youngest
#[test]
fn test_map_order_age_asc_limit_1() {
    let rows = run_query(
        "MATCH (n:person) RETURN n.name AS name ORDER BY n.age ASC LIMIT 1",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("vadas".into()));
}

/// g.E().order().by("weight",desc).limit(1) — heaviest edge
#[test]
fn test_map_order_edge_weight_desc() {
    let rows = run_query(
        "MATCH ()-[r]->() RETURN r.weight AS w ORDER BY r.weight DESC LIMIT 1",
    );
    assert_eq!(rows.len(), 1);
    if let Value::Float(w) = rows[0]["w"] {
        assert!((w - 1.0).abs() < 0.001);
    } else {
        panic!("Expected Float, got {:?}", rows[0]["w"]);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §8  Map — Select, Project (Select.feature, Project.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// Select: g.V().as("a").out("created").as("b").select("a","b").by("name")
#[test]
fn test_map_select_creator_and_creation() {
    let rows = run_query(
        "MATCH (a)-[:created]->(b) RETURN a.name AS creator, b.name AS creation ORDER BY creator, creation",
    );
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["creator"], Value::Str("josh".into()));
    assert_eq!(rows[0]["creation"], Value::Str("lop".into()));
}

/// Project: g.V().hasLabel("person").project("n","a").by("name").by("age")
#[test]
fn test_map_project_name_age() {
    let rows = run_query(
        "MATCH (n:person) RETURN n.name AS n, n.age AS a ORDER BY n.name",
    );
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["n"], Value::Str("josh".into()));
    assert_eq!(rows[0]["a"], Value::Int(32));
}

/// Select three variables in a path
#[test]
fn test_map_select_three_hops() {
    let rows = run_query(
        "MATCH (a {name: 'marko'})-[:knows]->(b)-[:created]->(c) \
         RETURN a.name AS a, b.name AS b, c.name AS c ORDER BY c",
    );
    // marko->josh->lop, marko->josh->ripple
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["a"], Value::Str("marko".into()));
    assert_eq!(rows[0]["b"], Value::Str("josh".into()));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §9  Map — Path (Path.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// Path: g.V().has("name","marko").out("knows").path() — 2 paths
#[test]
fn test_map_path_marko_knows_count() {
    let rows = run_query(
        "MATCH (a {name: 'marko'})-[:knows]->(b) RETURN b.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[1]["name"], Value::Str("vadas".into()));
}

/// Multi-hop path: marko->josh->ripple
#[test]
fn test_map_path_marko_josh_ripple() {
    let rows = run_query(
        "MATCH (a {name: 'marko'})-[:knows]->(b {name: 'josh'})-[:created]->(c {name: 'ripple'}) \
         RETURN c.name AS name",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("ripple".into()));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §10  Map — Match (Match.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// Match: find all person-created-software pairs
#[test]
fn test_map_match_person_created_software() {
    let rows = run_query(
        "MATCH (p:person)-[:created]->(s:software) \
         RETURN p.name AS person, s.name AS software ORDER BY person, software",
    );
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["person"], Value::Str("josh".into()));
    assert_eq!(rows[0]["software"], Value::Str("lop".into()));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §11  SideEffect — Group, GroupCount (Group.feature, GroupCount.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// GroupCount: g.V().groupCount().by(label) — person:4, software:2
#[test]
fn test_sideeffect_group_count_by_label() {
    let rows = run_query(
        "MATCH (n) RETURN labels(n) AS lbl, count(n) AS cnt ORDER BY lbl",
    );
    assert_eq!(rows.len(), 2);
}

/// GroupCount: g.E().groupCount().by(label) — knows:2, created:4
#[test]
fn test_sideeffect_group_count_edges_by_type() {
    let rows = run_query(
        "MATCH ()-[r]->() RETURN type(r) AS t, count(r) AS cnt ORDER BY t",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["t"], Value::Str("created".into()));
    assert_eq!(rows[0]["cnt"], Value::Int(4));
    assert_eq!(rows[1]["t"], Value::Str("knows".into()));
    assert_eq!(rows[1]["cnt"], Value::Int(2));
}

/// Group: g.V().hasLabel("person").group().by("age") — group persons by age
#[test]
fn test_sideeffect_group_persons_by_age() {
    let rows = run_query(
        "MATCH (n:person) RETURN n.age AS age, collect(n.name) AS names ORDER BY age",
    );
    assert_eq!(rows.len(), 4); // four distinct ages
}

/// Group: software creators grouped by software name
#[test]
fn test_sideeffect_group_creators_by_software() {
    let rows = run_query(
        "MATCH (p:person)-[:created]->(s:software) \
         RETURN s.name AS software, collect(p.name) AS creators ORDER BY software",
    );
    assert_eq!(rows.len(), 2); // lop, ripple
    assert_eq!(rows[0]["software"], Value::Str("lop".into()));
    if let Value::List(ref creators) = rows[0]["creators"] {
        assert_eq!(creators.len(), 3); // josh, marko, peter
    } else {
        panic!("Expected List");
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// §12  SideEffect — Aggregate, Inject (Aggregate.feature, Inject.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// Inject-like: collect all names then count
#[test]
fn test_sideeffect_collect_names_count() {
    let rows = run_query(
        "MATCH (n) WITH collect(n.name) AS names RETURN size(names) AS cnt",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(6));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §13  Branch — Choose, Optional, Repeat, Union (Choose.feature,
//      Optional.feature, Repeat.feature, Union.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// Union: g.V().union(out("knows"),out("created")).count()
#[test]
fn test_branch_union_knows_and_created() {
    let rows = run_query(
        "MATCH (n)-[:knows|created]->(t) RETURN count(t) AS cnt",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(6));
}

/// Optional/Coalesce pattern: return age if person, else lang
#[test]
fn test_branch_coalesce_age_or_lang() {
    let rows = run_query(
        "MATCH (n) \
         RETURN n.name AS name, \
                CASE WHEN n.age IS NOT NULL THEN n.age ELSE n.lang END AS val \
         ORDER BY name",
    );
    assert_eq!(rows.len(), 6);
    // josh → 32, lop → "java", etc.
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[0]["val"], Value::Int(32));
    assert_eq!(rows[1]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["val"], Value::Str("java".into()));
}

/// Choose pattern: label-based CASE
#[test]
fn test_branch_choose_by_label() {
    let rows = run_query(
        "MATCH (n) \
         RETURN n.name AS name, \
                CASE WHEN n:person THEN 'human' ELSE 'code' END AS kind \
         ORDER BY name",
    );
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[0]["kind"], Value::Str("human".into())); // josh
    assert_eq!(rows[1]["kind"], Value::Str("code".into()));  // lop
}

/// Repeat out 1 hop: g.V("marko").repeat(out()).times(1)
#[test]
fn test_branch_repeat_out_1() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-->(t) RETURN t.name AS name ORDER BY name",
    );
    assert_eq!(rows.len(), 3); // vadas, josh, lop
}

/// Repeat out 2 hops: g.V("marko").repeat(out()).times(2)
#[test]
fn test_branch_repeat_out_2() {
    let rows = run_query(
        "MATCH ({name: 'marko'})-->()-->(t) RETURN DISTINCT t.name AS name ORDER BY name",
    );
    // marko->josh->lop, marko->josh->ripple
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("lop".into()));
    assert_eq!(rows[1]["name"], Value::Str("ripple".into()));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §14  Map — AddEdge, AddVertex (AddEdge.feature, AddVertex.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// AddVertex: create and verify new vertex
#[test]
fn test_map_add_vertex_with_properties() {
    let (store, _ids) = create_modern_graph();
    let result = run_on(
        &store,
        "CREATE (n:language {name: 'rust', year: 2010}) RETURN n.name AS name, n.year AS year",
    );
    assert_eq!(result.stats.nodes_created, 1);
    assert_eq!(result.rows[0]["name"], Value::Str("rust".into()));
    assert_eq!(result.rows[0]["year"], Value::Int(2010));
}

/// AddEdge: create edge between existing vertices
#[test]
fn test_map_add_edge_between_existing() {
    let (store, _ids) = create_modern_graph();
    let result = run_on(
        &store,
        "MATCH (a {name: 'peter'}), (b {name: 'josh'}) \
         CREATE (a)-[r:knows {weight: 0.7}]->(b) \
         RETURN type(r) AS t",
    );
    assert_eq!(result.stats.edges_created, 1);
    assert_eq!(result.rows[0]["t"], Value::Str("knows".into()));
}

/// AddVertex + AddEdge: create a full path
#[test]
fn test_map_add_vertex_and_edge_chain() {
    let (store, _ids) = create_modern_graph();
    let result = run_on(
        &store,
        "CREATE (a:person {name: 'alice', age: 25})-[:knows {weight: 0.9}]->(b:person {name: 'bob', age: 30}) \
         RETURN a.name AS a, b.name AS b",
    );
    assert_eq!(result.stats.nodes_created, 2);
    assert_eq!(result.stats.edges_created, 1);
    assert_eq!(result.rows[0]["a"], Value::Str("alice".into()));
    assert_eq!(result.rows[0]["b"], Value::Str("bob".into()));

    let count = run_on(&store, "MATCH (n) RETURN count(n) AS cnt");
    assert_eq!(count.rows[0]["cnt"], Value::Int(8));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §15  Filter — Drop (Drop.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// Drop: g.V().has("name","peter").drop() — remove peter, count decreases
#[test]
fn test_filter_drop_vertex_peter() {
    let (store, _ids) = create_modern_graph();
    let result = run_on(
        &store,
        "MATCH (n {name: 'peter'}) DETACH DELETE n",
    );
    assert_eq!(result.stats.nodes_deleted, 1);

    let count = run_on(&store, "MATCH (n) RETURN count(n) AS cnt");
    assert_eq!(count.rows[0]["cnt"], Value::Int(5));
}

/// Drop edges only: g.V().has("name","marko").outE("created").drop()
#[test]
fn test_filter_drop_marko_created_edges() {
    let (store, _ids) = create_modern_graph();
    let result = run_on(
        &store,
        "MATCH ({name: 'marko'})-[r:created]->() DELETE r",
    );
    assert_eq!(result.stats.edges_deleted, 1);

    let count = run_on(&store, "MATCH ()-[r:created]->() RETURN count(r) AS cnt");
    assert_eq!(count.rows[0]["cnt"], Value::Int(3));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §16  Semantics — Comparability, Equality (Comparability.feature,
//      Equality.feature)
// ═══════════════════════════════════════════════════════════════════════════════

/// Equality: integer comparison in WHERE
#[test]
fn test_semantics_equality_integer() {
    let rows = run_query("MATCH (n:person) WHERE n.age = 32 RETURN n.name AS name");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
}

/// String equality
#[test]
fn test_semantics_equality_string() {
    let rows = run_query("MATCH (n) WHERE n.name = 'ripple' RETURN n.lang AS lang");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["lang"], Value::Str("java".into()));
}

/// Comparability: ordering integers
#[test]
fn test_semantics_comparability_int_ordering() {
    let rows = run_query(
        "MATCH (n:person) WHERE n.age >= 29 AND n.age <= 32 \
         RETURN n.name AS name ORDER BY n.age",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
    assert_eq!(rows[1]["name"], Value::Str("josh".into()));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §17  Complex composition patterns — multi-step traversals
// ═══════════════════════════════════════════════════════════════════════════════

/// Triangle detection: marko->josh->lop<-marko
#[test]
fn test_composition_triangle_detection() {
    let rows = run_query(
        "MATCH (a {name: 'marko'})-[:created]->(s)<-[:created]-(b) \
         WHERE a <> b \
         RETURN a.name AS a, b.name AS b, s.name AS s ORDER BY b",
    );
    // josh and peter also created lop
    assert!(rows.len() >= 2);
}

/// Vertex with most outgoing edges
#[test]
fn test_composition_max_out_degree() {
    let rows = run_query(
        "MATCH (n)-[r]->() \
         WITH n, count(r) AS degree \
         RETURN n.name AS name, degree \
         ORDER BY degree DESC LIMIT 1",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], Value::Str("marko".into()));
    assert_eq!(rows[0]["degree"], Value::Int(3));
}

/// Isolated: vertices with no incoming edges (source vertices)
#[test]
fn test_composition_source_vertices_via_antijoin() {
    let rows = run_query(
        "MATCH (n) \
         WHERE NOT EXISTS { MATCH (n)<-[]-() } \
         RETURN n.name AS name ORDER BY name",
    );
    let names: Vec<&Value> = rows.iter().map(|r| &r["name"]).collect();
    assert!(names.contains(&&Value::Str("marko".into())));
    assert!(names.contains(&&Value::Str("peter".into())));
}

/// Mutual friends: persons known by marko who also created something
#[test]
fn test_composition_friends_who_create() {
    let rows = run_query(
        "MATCH (m {name: 'marko'})-[:knows]->(f)-[:created]->(s) \
         RETURN f.name AS friend, s.name AS creation ORDER BY friend, creation",
    );
    // josh->lop, josh->ripple
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["friend"], Value::Str("josh".into()));
}

/// Co-creators: persons who created the same software
#[test]
fn test_composition_co_creators() {
    let rows = run_query(
        "MATCH (a:person)-[:created]->(s:software)<-[:created]-(b:person) \
         WHERE a.name < b.name \
         RETURN a.name AS a, b.name AS b, s.name AS s ORDER BY s, a",
    );
    // lop has josh, marko, peter as creators → 3 pairs
    assert_eq!(rows.len(), 3);
}

/// Average age of creators per software
#[test]
fn test_composition_avg_creator_age_per_software() {
    let rows = run_query(
        "MATCH (p:person)-[:created]->(s:software) \
         RETURN s.name AS software, avg(p.age) AS avg_age ORDER BY software",
    );
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["software"], Value::Str("lop".into()));
    // lop creators: josh(32), marko(29), peter(35) → avg 32.0
    if let Value::Float(avg) = rows[0]["avg_age"] {
        assert!((avg - 32.0).abs() < 0.01);
    } else {
        panic!("Expected Float, got {:?}", rows[0]["avg_age"]);
    }
}

/// Multi-aggregation: count, sum, avg in one query
#[test]
fn test_composition_multi_aggregation() {
    let rows = run_query(
        "MATCH (n:person) \
         RETURN count(n) AS cnt, sum(n.age) AS total, avg(n.age) AS average",
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["cnt"], Value::Int(4));
    assert_eq!(rows[0]["total"], Value::Int(123));
}

/// Subquery-like: for each person, how many software did they create?
#[test]
fn test_composition_creation_count_per_person() {
    let rows = run_query(
        "MATCH (p:person) \
         OPTIONAL MATCH (p)-[:created]->(s) \
         RETURN p.name AS name, count(s) AS created ORDER BY name",
    );
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["name"], Value::Str("josh".into()));
    assert_eq!(rows[0]["created"], Value::Int(2));
    assert_eq!(rows[3]["name"], Value::Str("vadas".into()));
    assert_eq!(rows[3]["created"], Value::Int(0));
}

// ═══════════════════════════════════════════════════════════════════════════════
// §18  Write operations — multi-step mutations
// ═══════════════════════════════════════════════════════════════════════════════

/// Set property: update marko's age
#[test]
fn test_write_set_property() {
    let (store, _ids) = create_modern_graph();
    let result = run_on(
        &store,
        "MATCH (n {name: 'marko'}) SET n.age = 30 RETURN n.age AS age",
    );
    assert_eq!(result.rows[0]["age"], Value::Int(30));

    let verify = run_on(&store, "MATCH (n {name: 'marko'}) RETURN n.age AS age");
    assert_eq!(verify.rows[0]["age"], Value::Int(30));
}

/// Multiple creates in sequence
#[test]
fn test_write_multiple_creates() {
    let (store, _ids) = create_modern_graph();
    run_on(
        &store,
        "CREATE (:animal {name: 'cat'})",
    );
    run_on(
        &store,
        "CREATE (:animal {name: 'dog'})",
    );
    let count = run_on(&store, "MATCH (n:animal) RETURN count(n) AS cnt");
    assert_eq!(count.rows[0]["cnt"], Value::Int(2));
}

/// Delete all edges of a type then verify
#[test]
fn test_write_delete_all_knows_edges() {
    let (store, _ids) = create_modern_graph();
    let result = run_on(&store, "MATCH ()-[r:knows]->() DELETE r");
    assert_eq!(result.stats.edges_deleted, 2);

    let count = run_on(&store, "MATCH ()-[r:knows]->() RETURN count(r) AS cnt");
    assert_eq!(count.rows[0]["cnt"], Value::Int(0));

    // created edges still intact
    let created = run_on(&store, "MATCH ()-[r:created]->() RETURN count(r) AS cnt");
    assert_eq!(created.rows[0]["cnt"], Value::Int(4));
}
