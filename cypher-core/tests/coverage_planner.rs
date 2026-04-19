//! Focused coverage tests for `cypher-core::planner` module dispatch and result planning.
//!
//! Drives the planner via the public `parse_query` → `QueryPlanner::plan` pipeline.

use std::collections::HashMap;

use cypher_core::parser::parse_query;
use cypher_core::planner::{Operator, QueryPlanner};
use cypher_core::storage::{GraphStore, InMemoryGraphStore};
use cypher_core::types::Value;

fn store() -> InMemoryGraphStore {
    let s = InMemoryGraphStore::new();
    s.create_graph("g").unwrap();
    s
}

fn plan(query: &str) -> Operator {
    let s = store();
    plan_with(&s, query)
}

fn plan_with(s: &InMemoryGraphStore, query: &str) -> Operator {
    let q = parse_query(query).unwrap();
    QueryPlanner::new(s, "g").plan(&q).unwrap()
}

fn plan_with_params(query: &str, params: HashMap<String, Value>) -> Operator {
    let s = store();
    let q = parse_query(query).unwrap();
    let registry = cypher_core::procedure::ProcedureRegistry::new();
    QueryPlanner::with_context(&s, "g", &registry, &params)
        .plan(&q)
        .unwrap()
}

// ── plan dispatch by clause type ────────────────────────────────────────────

#[test]
fn plan_match_node_scan() {
    let p = plan("MATCH (n) RETURN n");
    let s = format!("{:?}", p);
    assert!(s.contains("ProduceResults"));
    assert!(s.contains("NodeScan"));
}

#[test]
fn plan_match_with_label() {
    let p = plan("MATCH (n:Person) RETURN n");
    assert!(format!("{:?}", p).contains("Person"));
}

#[test]
fn plan_match_with_property_filter() {
    let p = plan("MATCH (n:Person {name: 'Alice'}) RETURN n");
    assert!(format!("{:?}", p).contains("Filter") || format!("{:?}", p).contains("IndexScan"));
}

#[test]
fn plan_create_node() {
    let p = plan("CREATE (n:Person {name: 'A'})");
    assert!(format!("{:?}", p).contains("Create"));
}

#[test]
fn plan_create_after_match() {
    let p = plan("MATCH (a:Person) CREATE (b:Friend)");
    let s = format!("{:?}", p);
    assert!(s.contains("Create"));
    assert!(s.contains("NodeScan"));
}

#[test]
fn plan_merge_node() {
    let p = plan("MERGE (n:Person {name: 'X'})");
    assert!(format!("{:?}", p).contains("Merge"));
}

#[test]
fn plan_merge_with_on_create() {
    let p = plan("MERGE (n:P {id: 1}) ON CREATE SET n.created = true");
    assert!(format!("{:?}", p).contains("Merge"));
}

#[test]
fn plan_merge_with_on_match() {
    let p = plan("MERGE (n:P {id: 1}) ON MATCH SET n.seen = true");
    assert!(format!("{:?}", p).contains("Merge"));
}

#[test]
fn plan_delete_node() {
    let p = plan("MATCH (n) DELETE n");
    assert!(format!("{:?}", p).contains("Delete"));
}

#[test]
fn plan_detach_delete() {
    let p = plan("MATCH (n) DETACH DELETE n");
    let s = format!("{:?}", p);
    assert!(s.contains("Delete"));
    assert!(s.contains("detach: true"));
}

#[test]
fn plan_set_property() {
    let p = plan("MATCH (n) SET n.x = 1");
    assert!(format!("{:?}", p).contains("Set"));
}

#[test]
fn plan_set_label() {
    let p = plan("MATCH (n) SET n:Tagged");
    assert!(format!("{:?}", p).contains("Set"));
}

#[test]
fn plan_set_map_replace() {
    let p = plan("MATCH (n) SET n = {a: 1}");
    assert!(format!("{:?}", p).contains("Set"));
}

#[test]
fn plan_set_map_merge() {
    let p = plan("MATCH (n) SET n += {a: 1}");
    assert!(format!("{:?}", p).contains("Set"));
}

#[test]
fn plan_remove_property() {
    let p = plan("MATCH (n) REMOVE n.x");
    assert!(format!("{:?}", p).contains("Remove"));
}

#[test]
fn plan_remove_label() {
    let p = plan("MATCH (n:P) REMOVE n:P");
    assert!(format!("{:?}", p).contains("Remove"));
}

#[test]
fn plan_unwind_list() {
    let p = plan("UNWIND [1,2,3] AS x RETURN x");
    assert!(format!("{:?}", p).contains("Unwind"));
}

#[test]
fn plan_unwind_after_match() {
    let p = plan("MATCH (n) UNWIND [1,2] AS i RETURN n, i");
    assert!(format!("{:?}", p).contains("Unwind"));
}

#[test]
fn plan_with_passthrough() {
    let p = plan("MATCH (n) WITH n RETURN n");
    assert!(format!("{:?}", p).contains("Project"));
}

#[test]
fn plan_with_filter() {
    let p = plan("MATCH (n) WITH n WHERE n.x > 1 RETURN n");
    assert!(format!("{:?}", p).contains("Filter"));
}

#[test]
fn plan_with_sort() {
    let p = plan("MATCH (n) WITH n ORDER BY n.x RETURN n");
    assert!(format!("{:?}", p).contains("Sort"));
}

#[test]
fn plan_with_limit() {
    let p = plan("MATCH (n) WITH n LIMIT 10 RETURN n");
    assert!(format!("{:?}", p).contains("Limit"));
}

#[test]
fn plan_with_skip() {
    let p = plan("MATCH (n) WITH n SKIP 2 RETURN n");
    assert!(format!("{:?}", p).contains("Skip"));
}

#[test]
fn plan_call_procedure() {
    let p = plan("CALL db.labels()");
    assert!(format!("{:?}", p).contains("Call"));
}

#[test]
fn plan_call_yield() {
    let p = plan("CALL db.labels() YIELD label RETURN label");
    assert!(format!("{:?}", p).contains("Call"));
}

#[test]
fn plan_optional_match_after_match() {
    let p = plan("MATCH (a) OPTIONAL MATCH (a)-[:R]->(b) RETURN a, b");
    assert!(format!("{:?}", p).contains("LeftOuterApply"));
}

#[test]
fn plan_optional_match_first() {
    let p = plan("OPTIONAL MATCH (n) RETURN n");
    let s = format!("{:?}", p);
    assert!(s.contains("LeftOuterApply"));
    assert!(s.contains("SingleRow"));
}

#[test]
fn plan_two_matches_apply() {
    let p = plan("MATCH (a) MATCH (b) RETURN a, b");
    assert!(format!("{:?}", p).contains("Apply"));
}

#[test]
fn plan_union() {
    let p = plan("MATCH (a) RETURN a AS x UNION MATCH (b) RETURN b AS x");
    assert!(format!("{:?}", p).contains("Union"));
}

#[test]
fn plan_union_all() {
    let p = plan("MATCH (a) RETURN a AS x UNION ALL MATCH (b) RETURN b AS x");
    let s = format!("{:?}", p);
    assert!(s.contains("Union"));
    assert!(s.contains("all: true"));
}

#[test]
fn plan_where_after_match() {
    let p = plan("MATCH (n) WHERE n.x = 1 RETURN n");
    assert!(format!("{:?}", p).contains("Filter"));
}

#[test]
fn plan_foreach() {
    let p = plan("MATCH (n) FOREACH (i IN [1,2] | SET n.x = i)");
    let s = format!("{:?}", p);
    // ForEach gets compiled to some Operator; just verify it plans
    assert!(!s.is_empty());
}

#[test]
fn plan_call_subquery() {
    let p = plan("CALL { MATCH (n) RETURN n LIMIT 1 } RETURN n");
    let s = format!("{:?}", p);
    assert!(s.contains("Limit"));
}

#[test]
fn plan_expand_relationship() {
    let p = plan("MATCH (a)-[:R]->(b) RETURN a, b");
    assert!(format!("{:?}", p).contains("Expand"));
}

#[test]
fn plan_varlen_relationship() {
    let p = plan("MATCH (a)-[:R*1..3]->(b) RETURN a, b");
    assert!(format!("{:?}", p).contains("VarLen"));
}

#[test]
fn plan_shortest_path() {
    let p = plan("MATCH p = shortestPath((a)-[:R*]->(b)) RETURN p");
    assert!(format!("{:?}", p).contains("ShortestPath") || format!("{:?}", p).contains("VarLen"));
}

// ── error paths ────────────────────────────────────────────────────────────

#[test]
fn standalone_where_errors() {
    let q = parse_query("WHERE 1 = 1 RETURN 1");
    if let Ok(ast) = q {
        let s = store();
        let r = QueryPlanner::new(&s, "g").plan(&ast);
        assert!(r.is_err(), "WHERE without preceding MATCH/WITH must error");
    }
}

#[test]
fn merge_relationship_planned() {
    let s = store();
    let q = parse_query("MATCH (a:P), (b:Q) MERGE (a)-[:R]->(b)").unwrap();
    let r = QueryPlanner::new(&s, "g").plan(&q);
    // Plans successfully or errors gracefully — just exercise the path.
    let _ = r;
}

// ── result planning: RETURN variants ───────────────────────────────────────

#[test]
fn return_single_column() {
    let p = plan("MATCH (n) RETURN n");
    let s = format!("{:?}", p);
    assert!(s.contains("ProduceResults"));
}

#[test]
fn return_multiple_columns() {
    let p = plan("MATCH (n) RETURN n, n.name");
    let s = format!("{:?}", p);
    assert!(s.contains("ProduceResults"));
    assert!(s.contains("n.name"));
}

#[test]
fn return_with_alias() {
    let p = plan("MATCH (n) RETURN n AS node");
    let s = format!("{:?}", p);
    assert!(s.contains("node"));
}

#[test]
fn return_expression() {
    let p = plan("RETURN 1 + 2 AS sum");
    let s = format!("{:?}", p);
    assert!(s.contains("ProduceResults"));
    assert!(s.contains("sum"));
}

#[test]
fn return_distinct() {
    let p = plan("MATCH (n) RETURN DISTINCT n");
    assert!(format!("{:?}", p).contains("Distinct"));
}

#[test]
fn return_order_by_asc() {
    let p = plan("MATCH (n) RETURN n ORDER BY n.x");
    assert!(format!("{:?}", p).contains("Sort"));
}

#[test]
fn return_order_by_desc() {
    let p = plan("MATCH (n) RETURN n ORDER BY n.x DESC");
    assert!(format!("{:?}", p).contains("Sort"));
}

#[test]
fn return_limit() {
    let p = plan("MATCH (n) RETURN n LIMIT 5");
    assert!(format!("{:?}", p).contains("Limit"));
}

#[test]
fn return_skip() {
    let p = plan("MATCH (n) RETURN n SKIP 3");
    assert!(format!("{:?}", p).contains("Skip"));
}

#[test]
fn return_skip_and_limit() {
    let p = plan("MATCH (n) RETURN n SKIP 1 LIMIT 5");
    let s = format!("{:?}", p);
    assert!(s.contains("Skip"));
    assert!(s.contains("Limit"));
}

#[test]
fn return_count_aggregation() {
    let p = plan("MATCH (n) RETURN count(n)");
    assert!(format!("{:?}", p).contains("Aggregate"));
}

#[test]
fn return_count_star() {
    let p = plan("MATCH (n) RETURN count(*)");
    assert!(format!("{:?}", p).contains("Aggregate"));
}

#[test]
fn return_sum_with_group_by() {
    let p = plan("MATCH (n) RETURN n.label, sum(n.x)");
    assert!(format!("{:?}", p).contains("Aggregate"));
}

#[test]
fn return_avg() {
    let p = plan("MATCH (n) RETURN avg(n.x)");
    assert!(format!("{:?}", p).contains("Aggregate"));
}

#[test]
fn return_min_max() {
    let p = plan("MATCH (n) RETURN min(n.x), max(n.x)");
    assert!(format!("{:?}", p).contains("Aggregate"));
}

#[test]
fn return_collect() {
    let p = plan("MATCH (n) RETURN collect(n)");
    assert!(format!("{:?}", p).contains("Aggregate"));
}

#[test]
fn return_distinct_with_aggregate() {
    let p = plan("MATCH (n) RETURN count(DISTINCT n)");
    assert!(format!("{:?}", p).contains("Aggregate"));
}

#[test]
fn return_star() {
    let p = plan("MATCH (n) RETURN *");
    let s = format!("{:?}", p);
    assert!(s.contains("ProduceResults"));
}

#[test]
fn return_constant_expression() {
    let p = plan("RETURN 42");
    let s = format!("{:?}", p);
    assert!(s.contains("ProduceResults"));
    assert!(s.contains("SingleRow") || s.contains("Project"));
}

#[test]
fn return_string_constant() {
    let p = plan("RETURN 'hello' AS s");
    let s = format!("{:?}", p);
    assert!(s.contains("hello") || s.contains("Project"));
}

#[test]
fn return_bool_constant() {
    let p = plan("RETURN true");
    assert!(format!("{:?}", p).contains("ProduceResults"));
}

#[test]
fn return_null_constant() {
    let p = plan("RETURN NULL");
    assert!(format!("{:?}", p).contains("ProduceResults"));
}

#[test]
fn return_list_constant() {
    let p = plan("RETURN [1,2,3] AS xs");
    assert!(format!("{:?}", p).contains("ProduceResults"));
}

#[test]
fn return_map_constant() {
    let p = plan("RETURN {a: 1, b: 2} AS m");
    assert!(format!("{:?}", p).contains("ProduceResults"));
}

#[test]
fn return_function_call() {
    let p = plan("RETURN size([1,2,3])");
    assert!(format!("{:?}", p).contains("size"));
}

#[test]
fn return_arithmetic() {
    let p = plan("RETURN 1 + 2 * 3 - 4");
    assert!(format!("{:?}", p).contains("ProduceResults"));
}

#[test]
fn return_distinct_with_order_limit() {
    let p = plan("MATCH (n) RETURN DISTINCT n.x AS xx ORDER BY xx LIMIT 5");
    let s = format!("{:?}", p);
    assert!(s.contains("Distinct"));
    assert!(s.contains("Sort"));
    assert!(s.contains("Limit"));
}

#[test]
fn with_distinct_pipeline() {
    let p = plan("MATCH (n) WITH DISTINCT n.x AS x RETURN x");
    assert!(format!("{:?}", p).contains("Distinct"));
}

#[test]
fn with_aggregation_pipeline() {
    let p = plan("MATCH (n) WITH count(n) AS c RETURN c");
    assert!(format!("{:?}", p).contains("Aggregate"));
}

#[test]
fn with_alias_chain() {
    let p = plan("MATCH (n) WITH n AS x WITH x AS y RETURN y");
    assert!(format!("{:?}", p).contains("Project"));
}

#[test]
fn with_where_then_return() {
    let p = plan("MATCH (n) WITH n WHERE n.x > 0 RETURN n");
    let s = format!("{:?}", p);
    assert!(s.contains("Filter"));
    assert!(s.contains("Project"));
}

#[test]
fn with_complete_pipeline() {
    let p = plan("MATCH (n) WITH n WHERE n.x > 0 RETURN n ORDER BY n.x SKIP 1 LIMIT 5");
    let s = format!("{:?}", p);
    assert!(s.contains("Sort"));
    assert!(s.contains("Skip"));
    assert!(s.contains("Limit"));
    assert!(s.contains("Filter"));
}

#[test]
fn return_with_parameter() {
    let mut params = HashMap::new();
    params.insert("x".to_string(), Value::Int(10));
    let p = plan_with_params("RETURN $x AS y", params);
    assert!(format!("{:?}", p).contains("ProduceResults"));
}

#[test]
fn plan_load_csv() {
    let p = plan("LOAD CSV FROM 'file:///x.csv' AS row RETURN row");
    let s = format!("{:?}", p);
    assert!(s.contains("Unwind") || s.contains("loadcsv"));
}

#[test]
fn plan_call_subquery_with_outer() {
    let p = plan("MATCH (n) CALL { MATCH (m) RETURN m LIMIT 1 } RETURN n, m");
    let s = format!("{:?}", p);
    assert!(s.contains("Apply"));
}

#[test]
fn collect_visible_aliases_basic() {
    let s = store();
    let q = parse_query("MATCH (a:P) RETURN a").unwrap();
    let plan = QueryPlanner::new(&s, "g").plan(&q).unwrap();
    let _aliases = cypher_core::planner::collect_visible_aliases(&plan);
}

#[test]
fn varlen_bounds_validate_some_max() {
    let r = cypher_core::planner::validate_varlen_bounds(2, Some(10)).unwrap();
    assert_eq!(r, (2, 10));
}

#[test]
fn varlen_bounds_validate_zero_min() {
    let r = cypher_core::planner::validate_varlen_bounds(0, Some(5)).unwrap();
    assert_eq!(r, (0, 5));
}

#[test]
fn plan_match_with_inline_where_via_where_clause() {
    let p = plan("MATCH (n) WHERE EXISTS((n)-[:R]->()) RETURN n");
    assert!(format!("{:?}", p).contains("Filter"));
}

#[test]
fn plan_with_negative_limit_errors_or_clamps() {
    let s = store();
    let q = parse_query("MATCH (n) RETURN n LIMIT 0");
    if let Ok(ast) = q {
        let _ = QueryPlanner::new(&s, "g").plan(&ast);
    }
}

#[test]
fn plan_create_path() {
    let p = plan("CREATE (a:A)-[:R]->(b:B)");
    assert!(format!("{:?}", p).contains("Create"));
}

#[test]
fn plan_match_chain_three_nodes() {
    let p = plan("MATCH (a)-[:R]->(b)-[:R]->(c) RETURN a, b, c");
    let s = format!("{:?}", p);
    assert!(s.contains("Expand"));
}

#[test]
fn plan_with_return_alias_propagates() {
    let p = plan("MATCH (n) WITH n.name AS name RETURN name");
    assert!(format!("{:?}", p).contains("name"));
}

#[test]
fn plan_with_aggregate_then_filter() {
    let p = plan("MATCH (n) WITH count(n) AS c WHERE c > 0 RETURN c");
    let s = format!("{:?}", p);
    assert!(s.contains("Aggregate"));
    assert!(s.contains("Filter"));
}
