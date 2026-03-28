use std::collections::HashMap;
use std::time::Duration;

use gql_core::executor::{ExecutionBudget, QueryExecutor};
use gql_core::parser::parse_query;
use gql_core::planner::{Operator, QueryPlanner};
use gql_core::procedure::ProcedureRegistry;
use gql_core::storage::{GraphStore, InMemoryGraphStore};
use gql_core::types::Value;
use indexmap::IndexMap;

fn make_store() -> InMemoryGraphStore {
    let store = InMemoryGraphStore::new();
    store.create_graph("test").unwrap();
    store
}

fn count_index_scans(op: &Operator) -> usize {
    match op {
        Operator::IndexScan { .. } => 1,
        Operator::Filter { child, .. }
        | Operator::Project { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::Aggregate { child, .. }
        | Operator::Delete { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::AssemblePath { child, .. }
        | Operator::ProduceResults { child, .. }
        | Operator::VarLenExpand { child, .. }
        | Operator::Expand { child, .. }
        | Operator::MergeRelationship { child, .. } => count_index_scans(child),
        Operator::Apply { outer, inner }
        | Operator::Union {
            left: outer,
            right: inner,
            ..
        } => count_index_scans(outer) + count_index_scans(inner),
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => count_index_scans(preserved) + count_index_scans(optional_pattern),
        Operator::Create { child, .. } | Operator::Call { child, .. } => {
            child.as_deref().map(count_index_scans).unwrap_or(0)
        }
        Operator::Merge { child, .. } => child.as_deref().map(count_index_scans).unwrap_or(0),
        Operator::ForEach { body, child, .. } => count_index_scans(body) + count_index_scans(child),
        Operator::Repeat {
            body,
            until,
            emit_predicate,
            ..
        } => {
            count_index_scans(body)
                + until.as_deref().map(count_index_scans).unwrap_or(0)
                + emit_predicate
                    .as_deref()
                    .map(count_index_scans)
                    .unwrap_or(0)
        }
        _ => 0,
    }
}

fn index_scan_properties(op: &Operator, properties: &mut Vec<String>) {
    match op {
        Operator::IndexScan { property, .. } => properties.push(property.clone()),
        Operator::Filter { child, .. }
        | Operator::Project { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::Aggregate { child, .. }
        | Operator::Delete { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::AssemblePath { child, .. }
        | Operator::ProduceResults { child, .. }
        | Operator::VarLenExpand { child, .. }
        | Operator::Expand { child, .. }
        | Operator::MergeRelationship { child, .. } => index_scan_properties(child, properties),
        Operator::Apply { outer, inner }
        | Operator::Union {
            left: outer,
            right: inner,
            ..
        } => {
            index_scan_properties(outer, properties);
            index_scan_properties(inner, properties);
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => {
            index_scan_properties(preserved, properties);
            index_scan_properties(optional_pattern, properties);
        }
        Operator::Create { child, .. } | Operator::Call { child, .. } => {
            if let Some(child) = child.as_deref() {
                index_scan_properties(child, properties);
            }
        }
        Operator::Merge { child, .. } => {
            if let Some(child) = child.as_deref() {
                index_scan_properties(child, properties);
            }
        }
        Operator::ForEach { body, child, .. } => {
            index_scan_properties(body, properties);
            index_scan_properties(child, properties);
        }
        Operator::Repeat {
            body,
            until,
            emit_predicate,
            ..
        } => {
            index_scan_properties(body, properties);
            if let Some(until) = until.as_deref() {
                index_scan_properties(until, properties);
            }
            if let Some(emit_predicate) = emit_predicate.as_deref() {
                index_scan_properties(emit_predicate, properties);
            }
        }
        _ => {}
    }
}

#[test]
fn correlated_match_uses_selective_lookup_without_declared_index() {
    let store = make_store();
    for i in 0..300 {
        let mut props = IndexMap::new();
        props.insert("id".to_string(), Value::Int(i));
        store
            .create_node("test", vec!["Node".to_string()], props)
            .unwrap();
    }

    let mut edge_batch = Vec::new();
    for i in 0..10 {
        let mut edge = IndexMap::new();
        edge.insert("src".into(), Value::Int(i));
        edge.insert("dst".into(), Value::Int(i + 1));
        edge_batch.push(Value::Map(edge));
    }

    let mut params = HashMap::new();
    params.insert("edges".to_string(), Value::List(edge_batch));

    let query = parse_query(
        "UNWIND $edges AS edge \
         MATCH (src:Node {id: edge.src}), (dst:Node {id: edge.dst}) \
         MERGE (src)-[:LINK]->(dst) \
         RETURN src, dst",
    )
    .unwrap();

    let registry = ProcedureRegistry::new();
    let planner = QueryPlanner::with_context(&store, "test", &registry, &params);
    let plan = planner.plan(&query).unwrap();

    assert_eq!(
        count_index_scans(&plan),
        2,
        "correlated exact-match node lookups should plan as selective IndexScan operators even without declared indexes"
    );
}

#[test]
fn unwind_match_merge_edges_stays_under_budget() {
    let store = make_store();
    for i in 0..300 {
        let mut props = IndexMap::new();
        props.insert("id".to_string(), Value::Int(i));
        store
            .create_node("test", vec!["Node".to_string()], props)
            .unwrap();
    }

    let mut edge_batch = Vec::new();
    for i in 0..10 {
        let mut edge = IndexMap::new();
        edge.insert("src".into(), Value::Int(i));
        edge.insert("dst".into(), Value::Int(i + 1));
        edge_batch.push(Value::Map(edge));
    }

    let mut params = HashMap::new();
    params.insert("edges".to_string(), Value::List(edge_batch));

    let query = parse_query(
        "UNWIND $edges AS edge \
         MATCH (src:Node {id: edge.src}), (dst:Node {id: edge.dst}) \
         MERGE (src)-[:LINK]->(dst) \
         RETURN src, dst",
    )
    .unwrap();

    let registry = ProcedureRegistry::new();
    let planner = QueryPlanner::with_context(&store, "test", &registry, &params);
    let plan = planner.plan(&query).unwrap();
    let executor = QueryExecutor::with_context(&store, &registry, params);
    let mut budget = ExecutionBudget::new(Duration::from_secs(30), 5_000);
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 10);
    assert_eq!(result.stats.edges_created, 10);
}

#[test]
fn top_level_parameterized_exact_match_binds_parameters_in_selective_lookup() {
    let store = make_store();
    let mut props = IndexMap::new();
    props.insert(
        "host".to_string(),
        Value::Str("neo4j.example.internal".into()),
    );
    store
        .create_node("test", vec!["Host".to_string()], props)
        .unwrap();

    let mut params = HashMap::new();
    params.insert(
        "host".to_string(),
        Value::Str("neo4j.example.internal".into()),
    );

    let query = parse_query("MATCH (h:Host {host: $host}) RETURN h").unwrap();
    let registry = ProcedureRegistry::new();
    let planner = QueryPlanner::with_context(&store, "test", &registry, &params);
    let plan = planner.plan(&query).unwrap();
    let executor = QueryExecutor::with_context(&store, &registry, params);
    let mut budget = ExecutionBudget::new(Duration::from_secs(30), 1_000);
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 1);
}

#[test]
fn planner_prefers_declared_index_over_first_property() {
    let store = make_store();
    store.create_index("test", "Node", "id").unwrap();

    let mut params = HashMap::new();
    params.insert("group".to_string(), Value::Int(0));
    params.insert("id".to_string(), Value::Int(7));

    let query = parse_query("MATCH (n:Node {group: $group, id: $id}) RETURN n").unwrap();
    let registry = ProcedureRegistry::new();
    let planner = QueryPlanner::with_context(&store, "test", &registry, &params);
    let plan = planner.plan(&query).unwrap();

    let mut properties = Vec::new();
    index_scan_properties(&plan, &mut properties);
    assert_eq!(properties, vec!["id".to_string()]);
}

#[test]
fn correlated_outer_node_property_lookup_binds_in_selective_lookup() {
    let store = make_store();
    for i in 0..10 {
        let mut foo_props = IndexMap::new();
        foo_props.insert("id".to_string(), Value::Int(i));
        store
            .create_node("test", vec!["Foo".to_string()], foo_props)
            .unwrap();

        let mut bar_props = IndexMap::new();
        bar_props.insert("id".to_string(), Value::Int(i));
        store
            .create_node("test", vec!["Bar".to_string()], bar_props)
            .unwrap();
    }

    let query = parse_query("MATCH (a:Foo), (b:Bar {id: a.id}) RETURN a, b").unwrap();
    let registry = ProcedureRegistry::new();
    let params = HashMap::new();
    let planner = QueryPlanner::with_context(&store, "test", &registry, &params);
    let plan = planner.plan(&query).unwrap();
    let executor = QueryExecutor::with_context(&store, &registry, params);
    let mut budget = ExecutionBudget::new(Duration::from_secs(30), 1_000);
    let result = executor.execute("test", &plan, &mut budget).unwrap();

    assert_eq!(result.rows.len(), 10);
}
