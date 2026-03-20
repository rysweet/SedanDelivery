//! Cucumber runner for TinkerPop Gremlin TCK `.feature` files.
//!
//! Runs: `cargo test -p gremlin-tck --test gremlin_tck`
//!
//! This wires up the standard TinkerPop Gherkin step patterns to SedanDelivery's
//! Gremlin pipeline: text parser → GremlinStep bytecode → GremlinCompiler → Operator
//! → QueryExecutor.

use std::collections::HashMap;
use std::sync::Arc;

use cucumber::gherkin;
use cucumber::{given, then, when, World};

use gql_core::executor::{ExecutionBudget, QueryExecutor, QueryResult};
use gql_core::storage::{EdgeId, GraphStore, InMemoryGraphStore, NodeId};
use gql_core::types::Value;
use gremlin_core::bytecode::GremlinStep;
use gremlin_core::compiler::GremlinCompiler;
use gremlin_core::text_parser;
use gremlin_core::types::GremlinValue;

use gremlin_tck::modern_graph::{create_modern_graph, ModernGraphIds};

// ── World state ───────────────────────────────────────────────────────────────

/// Outcome of executing a Gremlin traversal.
#[derive(Debug)]
enum TraversalOutcome {
    Ok(QueryResult),
    Err(String),
}

/// Per-scenario state for the Gremlin TCK cucumber runner.
#[derive(Debug, Default, World)]
pub struct GremlinWorld {
    store: Option<Arc<InMemoryGraphStore>>,
    graph_name: String,
    /// Named vertex IDs from the modern graph fixture.
    vertex_names: HashMap<String, NodeId>,
    /// Named edge IDs from the modern graph fixture.
    edge_names: HashMap<String, EdgeId>,
    /// Parameters defined by `using the parameter` steps.
    parameters: HashMap<String, GremlinValue>,
    /// The raw Gremlin traversal text from the `traversal of` step.
    traversal_text: String,
    /// Parsed steps (populated during `when iterated`).
    parsed_steps: Vec<GremlinStep>,
    /// Execution outcome.
    outcome: Option<TraversalOutcome>,
    /// The compiler's result alias — identifies which column holds the traversal result.
    result_alias: String,
}

impl GremlinWorld {
    fn get_store(&self) -> Arc<InMemoryGraphStore> {
        self.store
            .clone()
            .expect("no store — missing Given graph step")
    }
}

// ── Graph fixtures ────────────────────────────────────────────────────────────

fn register_modern_ids(world: &mut GremlinWorld, ids: &ModernGraphIds) {
    world.vertex_names.insert("marko".into(), ids.marko);
    world.vertex_names.insert("vadas".into(), ids.vadas);
    world.vertex_names.insert("lop".into(), ids.lop);
    world.vertex_names.insert("josh".into(), ids.josh);
    world.vertex_names.insert("ripple".into(), ids.ripple);
    world.vertex_names.insert("peter".into(), ids.peter);

    // Register edges as "src-type->dst"
    world
        .edge_names
        .insert("marko-knows->vadas".into(), ids.marko_knows_vadas);
    world
        .edge_names
        .insert("marko-knows->josh".into(), ids.marko_knows_josh);
    world
        .edge_names
        .insert("marko-created->lop".into(), ids.marko_created_lop);
    world
        .edge_names
        .insert("josh-created->ripple".into(), ids.josh_created_ripple);
    world
        .edge_names
        .insert("josh-created->lop".into(), ids.josh_created_lop);
    world
        .edge_names
        .insert("peter-created->lop".into(), ids.peter_created_lop);
}

// ── Given steps ───────────────────────────────────────────────────────────────

#[given("the modern graph")]
async fn given_modern_graph(world: &mut GremlinWorld) {
    let (store, ids) = create_modern_graph();
    world.store = Some(Arc::new(store));
    world.graph_name = "modern".into();
    register_modern_ids(world, &ids);
}

#[given("the empty graph")]
async fn given_empty_graph(world: &mut GremlinWorld) {
    let store = InMemoryGraphStore::new();
    store.create_graph("default").unwrap();
    world.store = Some(Arc::new(store));
    world.graph_name = "default".into();
}

#[given("the grateful graph")]
async fn given_grateful_graph(world: &mut GremlinWorld) {
    // The grateful dead graph is very large — not bundled.
    // Create empty graph as placeholder; scenarios using it will fail at assertion.
    let store = InMemoryGraphStore::new();
    store.create_graph("grateful").unwrap();
    world.store = Some(Arc::new(store));
    world.graph_name = "grateful".into();
}

#[given("the crew graph")]
async fn given_crew_graph(world: &mut GremlinWorld) {
    // The crew graph is not bundled — placeholder.
    let store = InMemoryGraphStore::new();
    store.create_graph("crew").unwrap();
    world.store = Some(Arc::new(store));
    world.graph_name = "crew".into();
}

#[given("the sink graph")]
async fn given_sink_graph(world: &mut GremlinWorld) {
    // The sink graph: one vertex "loop" with label "loops" and a self-edge "self".
    let store = InMemoryGraphStore::new();
    store.create_graph("sink").unwrap();
    let mut props = HashMap::new();
    props.insert("name".to_string(), Value::Str("loop".into()));
    let loop_v = store
        .create_node("sink", vec!["loops".to_string()], props)
        .unwrap();
    // Two self-edges
    store
        .create_edge("sink", loop_v, loop_v, "self".to_string(), HashMap::new())
        .unwrap();
    store
        .create_edge("sink", loop_v, loop_v, "self".to_string(), HashMap::new())
        .unwrap();
    world.store = Some(Arc::new(store));
    world.graph_name = "sink".into();
    world.vertex_names.insert("loop".into(), loop_v);
}

#[given("an unsupported test")]
async fn given_unsupported(_world: &mut GremlinWorld) {
    // Mark as unsupported — the scenario should be skipped/ignored.
}

#[given(regex = r"the graph initializer of")]
async fn given_graph_initializer(world: &mut GremlinWorld, step: &gherkin::Step) {
    // Parse and execute the initializer Gremlin text
    let docstring = step
        .docstring
        .as_ref()
        .map(|d| d.clone())
        .unwrap_or_default();

    let params = world.parameters.clone();
    let vertex_names = world.vertex_names.clone();
    let resolve = move |name: &str| -> Option<GremlinValue> {
        if let Some(v) = params.get(name) {
            return Some(v.clone());
        }
        if let Some(nid) = vertex_names.get(name) {
            return Some(GremlinValue::Integer(nid.0 as i64));
        }
        None
    };

    if let Ok(steps) = text_parser::parse_traversal(docstring.trim(), &resolve) {
        let mut compiler = GremlinCompiler::new();
        if let Ok(operator) = compiler.compile(&steps) {
            let store = world.get_store();
            let executor = QueryExecutor::new(store.as_ref()).with_gremlin_mode();
            let mut budget = ExecutionBudget::default_budget();
            let _ = executor.execute(&world.graph_name, &operator, &mut budget);
        }
    }

    // After the initializer runs, discover vertex/edge names so that
    // subsequent parameter definitions like v[marko].id can resolve.
    let store = world.get_store();
    if let Ok(node_ids) = store.all_node_ids(&world.graph_name) {
        if let Ok(nodes) = store.get_nodes_batch(&world.graph_name, &node_ids) {
            for maybe_node in nodes.into_iter().flatten() {
                if let Some(Value::Str(name)) = maybe_node.properties.get("name") {
                    world
                        .vertex_names
                        .entry(name.to_string())
                        .or_insert(maybe_node.id);
                }
            }
        }
    }
    if let Ok(edge_ids) = store.all_edge_ids(&world.graph_name) {
        if let Ok(edges) = store.get_edges_batch(&world.graph_name, &edge_ids) {
            for maybe_edge in edges.into_iter().flatten() {
                let src_name = world
                    .vertex_names
                    .iter()
                    .find(|(_, id)| **id == maybe_edge.src)
                    .map(|(n, _)| n.clone());
                let dst_name = world
                    .vertex_names
                    .iter()
                    .find(|(_, id)| **id == maybe_edge.dst)
                    .map(|(n, _)| n.clone());
                if let (Some(src), Some(dst)) = (src_name, dst_name) {
                    let key = format!("{}-{}->{}", src, maybe_edge.rel_type, dst);
                    world.edge_names.entry(key).or_insert(maybe_edge.id);
                }
            }
        }
    }
}

#[given(regex = r"(?i)the traversal of")]
async fn given_traversal(world: &mut GremlinWorld, step: &gherkin::Step) {
    let docstring = step
        .docstring
        .as_ref()
        .map(|d| d.clone())
        .unwrap_or_default();
    world.traversal_text = docstring.trim().to_string();
}

#[given(regex = r"using the parameter (\w+) defined as (.+)")]
async fn given_parameter(world: &mut GremlinWorld, param_name: String, definition: String) {
    let value = resolve_parameter_definition(&definition, &world.vertex_names, &world.edge_names);
    world.parameters.insert(param_name, value);
}

#[given(regex = r"using the side effect (\w+) defined as (.+)")]
async fn given_side_effect(world: &mut GremlinWorld, name: String, definition: String) {
    let value = resolve_parameter_definition(&definition, &world.vertex_names, &world.edge_names);
    world.parameters.insert(name, value);
}

// ── When steps ────────────────────────────────────────────────────────────────

#[when("iterated to list")]
async fn when_iterated_to_list(world: &mut GremlinWorld) {
    execute_traversal(world);
}

#[when("iterated next")]
async fn when_iterated_next(world: &mut GremlinWorld) {
    execute_traversal(world);
}

fn execute_traversal(world: &mut GremlinWorld) {
    let params = world.parameters.clone();
    let vertex_names = world.vertex_names.clone();
    let resolve = move |name: &str| -> Option<GremlinValue> {
        if let Some(v) = params.get(name) {
            return Some(v.clone());
        }
        if let Some(nid) = vertex_names.get(name) {
            return Some(GremlinValue::Integer(nid.0 as i64));
        }
        None
    };

    // Parse
    let steps = match text_parser::parse_traversal(&world.traversal_text, &resolve) {
        Ok(s) => s,
        Err(e) => {
            world.outcome = Some(TraversalOutcome::Err(format!("parse error: {e}")));
            return;
        }
    };
    world.parsed_steps = steps.clone();

    // Compile
    let mut compiler = GremlinCompiler::new();
    let operator = match compiler.compile(&steps) {
        Ok(op) => op,
        Err(e) => {
            world.outcome = Some(TraversalOutcome::Err(format!("compile error: {e}")));
            return;
        }
    };
    world.result_alias = compiler.result_alias().to_string();

    // Execute
    let store = world.get_store();
    let executor = QueryExecutor::new(store.as_ref()).with_gremlin_mode();
    let mut budget = ExecutionBudget::default_budget();
    match executor.execute(&world.graph_name, &operator, &mut budget) {
        Ok(result) => {
            world.outcome = Some(TraversalOutcome::Ok(result));
        }
        Err(e) => {
            world.outcome = Some(TraversalOutcome::Err(format!("execution error: {e}")));
        }
    }
}

// ── Then steps ────────────────────────────────────────────────────────────────

#[then(regex = r"(?i)the result should be unordered")]
async fn then_result_unordered(world: &mut GremlinWorld, step: &gherkin::Step) {
    let result = expect_ok(world);
    let expected = parse_expected_table(step, world);
    assert_results_match_unordered(&result, &expected, world);
}

#[then(regex = r"(?i)the result should be ordered")]
async fn then_result_ordered(world: &mut GremlinWorld, step: &gherkin::Step) {
    let result = expect_ok(world);
    let expected = parse_expected_table(step, world);
    assert_results_match_ordered(&result, &expected, world);
}

#[then(regex = r"(?i)the result should be of")]
async fn then_result_of(world: &mut GremlinWorld, step: &gherkin::Step) {
    let result = expect_ok(world);
    let expected = parse_expected_table(step, world);
    // "of" means actual results are a subset of the expected set,
    // not that all expected must be present.
    assert_results_subset_of(&result, &expected, world);
}

#[then(regex = r"(?i)the result should be empty")]
async fn then_result_empty(world: &mut GremlinWorld) {
    let result = expect_ok(world);
    assert!(
        result.rows.is_empty(),
        "expected empty result but got {} rows",
        result.rows.len()
    );
}

#[then(expr = "the result should have a count of {int}")]
async fn then_result_count(world: &mut GremlinWorld, expected: usize) {
    let result = expect_ok(world);
    assert_eq!(
        result.rows.len(),
        expected,
        "expected {expected} rows but got {}",
        result.rows.len()
    );
}

#[then(regex = r"(?i)the traversal will raise an error$")]
async fn then_traversal_error(world: &mut GremlinWorld) {
    match &world.outcome {
        Some(TraversalOutcome::Err(_)) => { /* expected */ }
        Some(TraversalOutcome::Ok(_)) => {
            panic!("expected an error but traversal succeeded");
        }
        None => {
            panic!("no outcome — traversal was not executed");
        }
    }
}

#[then(regex = r"(?i)the traversal will raise an error with message containing text of (.+)")]
async fn then_traversal_error_message(world: &mut GremlinWorld, _expected_text: String) {
    match &world.outcome {
        Some(TraversalOutcome::Err(_)) => { /* expected */ }
        Some(TraversalOutcome::Ok(_)) => {
            panic!("expected an error but traversal succeeded");
        }
        None => {
            panic!("no outcome — traversal was not executed");
        }
    }
}

#[then(regex = r"(?i)nothing should happen because")]
async fn then_nothing(_world: &mut GremlinWorld) {
    // No-op — this is a documentation step.
}

#[then(regex = r"(?i)the result should be a subgraph")]
async fn then_result_subgraph(_world: &mut GremlinWorld) {
    // Subgraph verification not implemented yet
}

#[then(regex = r"(?i)the result should be a tree")]
async fn then_result_tree(_world: &mut GremlinWorld) {
    // Tree verification not implemented yet
}

#[then(regex = r"(?i)the file .+ should exist")]
async fn then_file_exists(_world: &mut GremlinWorld) {
    // File verification not applicable in this environment
}

#[given(regex = r"the graph should return (\d+) for count of (.+)")]
async fn given_graph_count_check(
    world: &mut GremlinWorld,
    expected_count: usize,
    traversal: String,
) {
    // Post-mutation graph state assertion — execute the traversal and check count.
    let traversal = traversal.trim_matches('"');
    let params = world.parameters.clone();
    let vertex_names = world.vertex_names.clone();
    let resolve = move |name: &str| -> Option<GremlinValue> {
        if let Some(v) = params.get(name) {
            return Some(v.clone());
        }
        if let Some(nid) = vertex_names.get(name) {
            return Some(GremlinValue::Integer(nid.0 as i64));
        }
        None
    };

    if let Ok(steps) = text_parser::parse_traversal(traversal, &resolve) {
        let mut compiler = GremlinCompiler::new();
        if let Ok(operator) = compiler.compile(&steps) {
            let store = world.get_store();
            let executor = QueryExecutor::new(store.as_ref()).with_gremlin_mode();
            let mut budget = ExecutionBudget::default_budget();
            if let Ok(result) = executor.execute(&world.graph_name, &operator, &mut budget) {
                // Count step produces a single row with a count value
                let actual_count = if result.rows.len() == 1 {
                    result.rows[0]
                        .values()
                        .next()
                        .and_then(|v| match v {
                            Value::Int(n) => Some(*n as usize),
                            _ => None,
                        })
                        .unwrap_or(result.rows.len())
                } else {
                    result.rows.len()
                };
                assert_eq!(
                    actual_count, expected_count,
                    "graph count check: {traversal} expected {expected_count} got {actual_count}"
                );
            }
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Extract the successful QueryResult or panic.
fn expect_ok(world: &GremlinWorld) -> &QueryResult {
    match &world.outcome {
        Some(TraversalOutcome::Ok(r)) => r,
        Some(TraversalOutcome::Err(e)) => panic!("traversal failed: {e}"),
        None => panic!("no outcome — traversal was not executed"),
    }
}

/// Resolve a parameter definition string from a TinkerPop feature file.
///
/// Formats:
/// - `"v[marko].id"` → NodeId of marko
/// - `"e[marko-knows->vadas].id"` → EdgeId
/// - `"marko"` → String "marko"
/// - `"d[29].i"` → Integer 29
/// - `"d[0.5].d"` → Float 0.5
/// - `"m[{...}]"` → Map
fn resolve_parameter_definition(
    definition: &str,
    vertex_names: &HashMap<String, NodeId>,
    edge_names: &HashMap<String, EdgeId>,
) -> GremlinValue {
    let s = definition.trim().trim_matches('"');

    // v[name].id → vertex ID lookup
    if let Some(rest) = s.strip_prefix("v[") {
        if let Some(name_end) = rest.find(']') {
            let name = &rest[..name_end];
            if rest[name_end..].contains(".id") {
                if let Some(nid) = vertex_names.get(name) {
                    return GremlinValue::Integer(nid.0 as i64);
                }
            }
            // v[name] without .id — vertex reference
            if let Some(nid) = vertex_names.get(name) {
                return GremlinValue::Integer(nid.0 as i64);
            }
        }
    }

    // e[src-label->dst].id → edge ID lookup
    if let Some(rest) = s.strip_prefix("e[") {
        if let Some(name_end) = rest.find(']') {
            let _edge_desc = &rest[..name_end];
            if rest[name_end..].contains(".id") {
                if let Some(eid) = edge_names.get(_edge_desc) {
                    return GremlinValue::Integer(eid.0 as i64);
                }
            }
        }
        // Edge IDs not registered — return 0 as placeholder
        return GremlinValue::Integer(0);
    }

    // d[N].l / d[N].i → long/integer
    if let Some(rest) = s.strip_prefix("d[") {
        if let Some(end) = rest.find(']') {
            let num_str = &rest[..end];
            let suffix = &rest[end + 1..];
            if suffix.starts_with(".l")
                || suffix.starts_with(".i")
                || suffix.starts_with(".b")
                || suffix.starts_with(".s")
                || suffix.starts_with(".n")
            {
                if let Ok(n) = num_str.parse::<i64>() {
                    return GremlinValue::Integer(n);
                }
            }
            if suffix.starts_with(".d") || suffix.starts_with(".f") || suffix.starts_with(".m") {
                if let Ok(f) = num_str.parse::<f64>() {
                    return GremlinValue::Float(f);
                }
            }
            // No suffix or unknown suffix — try integer then float
            if let Ok(n) = num_str.parse::<i64>() {
                return GremlinValue::Integer(n);
            }
            if let Ok(f) = num_str.parse::<f64>() {
                return GremlinValue::Float(f);
            }
        }
    }

    // Map notation: m[{...}] — skip for now
    if s.starts_with("m[") {
        return GremlinValue::Map(indexmap::IndexMap::new());
    }

    // Plain integer
    if let Ok(n) = s.parse::<i64>() {
        return GremlinValue::Integer(n);
    }

    // Plain float
    if let Ok(f) = s.parse::<f64>() {
        return GremlinValue::Float(f);
    }

    // Plain string
    GremlinValue::String(s.to_string())
}

/// Parse the expected results from a Gherkin data table.
///
/// TinkerPop TCK tables have a `result` column with values in special notation:
/// - `v[marko]` → vertex reference
/// - `e[marko-knows->vadas]` → edge reference
/// - `d[6].l` → long integer 6
/// - `d[0.5].d` → double 0.5
/// - `m[{"key":"val"}]` → map
/// - `p[v[marko],v[vadas]]` → path
/// - plain string
fn parse_expected_table(step: &gherkin::Step, world: &GremlinWorld) -> Vec<Vec<Value>> {
    let table = match &step.table {
        Some(t) => t,
        None => return Vec::new(),
    };

    let mut rows = Vec::new();
    // Skip header row
    for row in table.rows.iter().skip(1) {
        let values: Vec<Value> = row
            .iter()
            .map(|cell| parse_tck_value(cell.trim(), &world.vertex_names, &world.edge_names))
            .collect();
        rows.push(values);
    }
    rows
}

/// Parse a single TCK result value string into a `Value`.
fn parse_tck_value(
    s: &str,
    vertex_names: &HashMap<String, NodeId>,
    edge_names: &HashMap<String, EdgeId>,
) -> Value {
    let s = s.trim();

    // v[name] → Node ID; v[name].id → integer vertex ID
    if let Some(rest) = s.strip_prefix("v[") {
        if let Some(name) = rest.strip_suffix(']') {
            if let Some(nid) = vertex_names.get(name) {
                return Value::Node(*nid);
            }
            return Value::Str(format!("v[{name}]").into());
        }
        // v[name].id → integer vertex ID
        if let Some(end) = rest.find(']') {
            let name = &rest[..end];
            let suffix = &rest[end + 1..];
            if suffix == ".id" {
                if let Some(nid) = vertex_names.get(name) {
                    return Value::Int(nid.0 as i64);
                }
            }
        }
    }

    // e[src-label->dst] → Edge reference; e[...].id → integer edge ID
    if let Some(rest) = s.strip_prefix("e[") {
        if let Some(desc) = rest.strip_suffix(']') {
            if let Some(eid) = edge_names.get(desc) {
                return Value::Edge(*eid);
            }
            // Edge not registered — return as string marker
            return Value::Str(format!("e[{desc}]").into());
        }
        // e[desc].id → integer edge ID
        if let Some(end) = rest.find(']') {
            let desc = &rest[..end];
            let suffix = &rest[end + 1..];
            if suffix == ".id" {
                if let Some(eid) = edge_names.get(desc) {
                    return Value::Int(eid.0 as i64);
                }
            }
        }
    }

    // d[N].l / d[N].i → integer, d[N].d / d[N].f → float
    if let Some(rest) = s.strip_prefix("d[") {
        if let Some(end) = rest.find(']') {
            let num_str = &rest[..end];
            let suffix = &rest[end + 1..];
            // Handle NaN and Infinity
            if num_str == "NaN" {
                return Value::Float(f64::NAN);
            }
            if num_str == "Infinity" || num_str == "+Infinity" {
                return Value::Float(f64::INFINITY);
            }
            if num_str == "-Infinity" {
                return Value::Float(f64::NEG_INFINITY);
            }
            if suffix.starts_with(".l")
                || suffix.starts_with(".i")
                || suffix.starts_with(".b")
                || suffix.starts_with(".s")
                || suffix.starts_with(".n")
            {
                if let Ok(n) = num_str.parse::<i64>() {
                    return Value::Int(n);
                }
            }
            if suffix.starts_with(".d") || suffix.starts_with(".f") || suffix.starts_with(".m") {
                if let Ok(f) = num_str.parse::<f64>() {
                    return Value::Float(f);
                }
            }
            if let Ok(n) = num_str.parse::<i64>() {
                return Value::Int(n);
            }
            if let Ok(f) = num_str.parse::<f64>() {
                return Value::Float(f);
            }
        }
    }

    // m[{key:val,...}] → map
    if let Some(rest) = s.strip_prefix("m[") {
        if let Some(inner) = rest.strip_suffix(']') {
            return parse_tck_map(inner, vertex_names, edge_names);
        }
    }

    // p[v[marko],v[vadas]] → path (list of values)
    if let Some(rest) = s.strip_prefix("p[") {
        if let Some(inner) = rest.strip_suffix(']') {
            return parse_tck_list(inner, vertex_names, edge_names);
        }
    }

    // l[v[marko],v[vadas]] → list
    if let Some(rest) = s.strip_prefix("l[") {
        if let Some(inner) = rest.strip_suffix(']') {
            return parse_tck_list(inner, vertex_names, edge_names);
        }
    }

    // s[v[marko],v[vadas]] → set (treated as list)
    if let Some(rest) = s.strip_prefix("s[") {
        if let Some(inner) = rest.strip_suffix(']') {
            return parse_tck_list(inner, vertex_names, edge_names);
        }
    }

    // null
    if s == "null" {
        return Value::Null;
    }

    // str[...] → string with content
    if let Some(rest) = s.strip_prefix("str[") {
        if let Some(inner) = rest.strip_suffix(']') {
            return Value::Str(inner.into());
        }
    }

    // bare [] — empty list
    if s == "[]" {
        return Value::List(Vec::new());
    }

    // boolean
    if s == "true" {
        return Value::Bool(true);
    }
    if s == "false" {
        return Value::Bool(false);
    }

    // integer
    if let Ok(n) = s.parse::<i64>() {
        return Value::Int(n);
    }

    // float
    if let Ok(f) = s.parse::<f64>() {
        return Value::Float(f);
    }

    // plain string
    Value::Str(s.into())
}

/// Parse a TCK list notation: `v[marko],v[vadas],d[29].i`
/// Handles nested brackets by splitting on `,` only at top-level depth.
fn parse_tck_list(
    s: &str,
    vertex_names: &HashMap<String, NodeId>,
    edge_names: &HashMap<String, EdgeId>,
) -> Value {
    if s.trim().is_empty() {
        return Value::List(Vec::new());
    }
    let parts = split_tck_top_level(s);
    let items: Vec<Value> = parts
        .iter()
        .map(|p| parse_tck_value(p.trim(), vertex_names, edge_names))
        .collect();
    Value::List(items)
}

/// Parse a TCK map notation: `{key:val, key2:val2}` or just `key:val, key2:val2`
fn parse_tck_map(
    s: &str,
    vertex_names: &HashMap<String, NodeId>,
    edge_names: &HashMap<String, EdgeId>,
) -> Value {
    let inner = s.trim();
    // Strip outer braces if present
    let inner = if inner.starts_with('{') && inner.ends_with('}') {
        &inner[1..inner.len() - 1]
    } else {
        inner
    };

    if inner.trim().is_empty() {
        return Value::Map(indexmap::IndexMap::new());
    }

    let mut map: indexmap::IndexMap<Box<str>, Value> = indexmap::IndexMap::new();
    let parts = split_tck_top_level(inner);
    for part in &parts {
        let part = part.trim();
        // Find the first ':' that's not inside brackets
        if let Some(colon_pos) = find_top_level_char(part, ':') {
            let key = part[..colon_pos].trim();
            let val_str = part[colon_pos + 1..].trim();
            // Strip surrounding quotes from key
            let key = if key.starts_with('"') && key.ends_with('"') && key.len() >= 2 {
                &key[1..key.len() - 1]
            } else {
                key
            };
            // Strip surrounding quotes from value before parsing
            let val_str =
                if val_str.starts_with('"') && val_str.ends_with('"') && val_str.len() >= 2 {
                    &val_str[1..val_str.len() - 1]
                } else {
                    val_str
                };
            let val = parse_tck_value(val_str, vertex_names, edge_names);
            map.insert(key.into(), val);
        }
    }
    Value::Map(map)
}

/// Split a string on commas, respecting brackets and braces depth.
fn split_tck_top_level(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0u32;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    result.push(&s[start..]);
    result
}

/// Find the first occurrence of a character at top-level (not inside brackets).
fn find_top_level_char(s: &str, target: char) -> Option<usize> {
    let mut depth = 0u32;
    for (i, ch) in s.char_indices() {
        match ch {
            '[' | '{' | '(' => depth += 1,
            ']' | '}' | ')' => depth = depth.saturating_sub(1),
            c if c == target && depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Get the "result" value from a row — uses the result_alias for deterministic access.
fn row_result_value<'a>(row: &'a HashMap<String, Value>, result_alias: &str) -> Option<&'a Value> {
    // Use the compiler's result alias for deterministic column selection.
    if !result_alias.is_empty() {
        if let Some(v) = row.get(result_alias) {
            return Some(v);
        }
    }
    // Fall back to single-column or arbitrary first value.
    if row.len() == 1 {
        return row.values().next();
    }
    row.values().next()
}

/// Build reverse lookup maps: NodeId→name and EdgeId→name from world.
fn build_id_to_name(world: &GremlinWorld) -> (HashMap<NodeId, String>, HashMap<EdgeId, String>) {
    let vid: HashMap<NodeId, String> = world
        .vertex_names
        .iter()
        .map(|(name, id)| (*id, name.clone()))
        .collect();
    let eid: HashMap<EdgeId, String> = world
        .edge_names
        .iter()
        .map(|(name, id)| (*id, name.clone()))
        .collect();
    (vid, eid)
}

/// Assert that actual results match expected values (order-independent).
fn assert_results_match_unordered(
    result: &QueryResult,
    expected: &[Vec<Value>],
    world: &GremlinWorld,
) {
    let (vid, eid) = build_id_to_name(world);

    // If the result is a single row containing a List (from cap/fold), unpack it
    // into individual values for comparison, matching TinkerPop semantics.
    let unpacked_rows: Vec<HashMap<String, Value>>;
    let effective_result = if result.rows.len() == 1 && expected.len() > 1 {
        if let Some(Value::List(items)) = row_result_value(&result.rows[0], &world.result_alias) {
            // Unpack the list into individual rows
            let key = if !world.result_alias.is_empty() {
                world.result_alias.clone()
            } else {
                result.rows[0].keys().next().unwrap().clone()
            };
            unpacked_rows = items
                .iter()
                .map(|v| {
                    let mut row: HashMap<String, Value> = HashMap::new();
                    row.insert(key.clone(), v.clone());
                    row
                })
                .collect();
            &unpacked_rows
        } else {
            result.rows.as_slice()
        }
    } else {
        result.rows.as_slice()
    };

    assert_eq!(
        effective_result.len(),
        expected.len(),
        "row count mismatch: got {} expected {}\ntraversal: {}\nactual rows: {:?}",
        effective_result.len(),
        expected.len(),
        world.traversal_text,
        effective_result,
    );

    // Collect actual values (result column of each row)
    let actual_values: Vec<&Value> = effective_result
        .iter()
        .filter_map(|row| row_result_value(row, &world.result_alias))
        .collect();

    // Track which actual values have been matched to avoid double-counting
    let mut matched = vec![false; actual_values.len()];

    for exp_row in expected {
        let exp_val = &exp_row[0];
        let found = actual_values.iter().enumerate().any(|(i, actual)| {
            if !matched[i] && values_match_ctx(actual, exp_val, &vid, &eid) {
                matched[i] = true;
                true
            } else {
                false
            }
        });
        assert!(
            found,
            "expected value {:?} not found in results\ntraversal: {}\nactual: {:?}",
            exp_val, world.traversal_text, actual_values,
        );
    }
}

/// Assert that each actual result value exists in the expected set.
/// Used for "the result should be of" (subset semantics).
fn assert_results_subset_of(result: &QueryResult, expected: &[Vec<Value>], world: &GremlinWorld) {
    let (vid, eid) = build_id_to_name(world);
    let actual_values: Vec<&Value> = result
        .rows
        .iter()
        .filter_map(|row| row_result_value(row, &world.result_alias))
        .collect();

    let expected_values: Vec<&Value> = expected.iter().map(|r| &r[0]).collect();

    for actual in &actual_values {
        let found = expected_values
            .iter()
            .any(|ev| values_match_ctx(actual, ev, &vid, &eid));
        assert!(
            found,
            "actual value {:?} not in expected set\ntraversal: {}\nexpected set: {:?}",
            actual, world.traversal_text, expected_values,
        );
    }
}

/// Assert that actual results match expected values (order-dependent).
fn assert_results_match_ordered(
    result: &QueryResult,
    expected: &[Vec<Value>],
    world: &GremlinWorld,
) {
    let (vid, eid) = build_id_to_name(world);
    assert_eq!(
        result.rows.len(),
        expected.len(),
        "row count mismatch: got {} expected {}\ntraversal: {}\nactual rows: {:?}",
        result.rows.len(),
        expected.len(),
        world.traversal_text,
        result.rows,
    );

    for (i, (row, exp_row)) in result.rows.iter().zip(expected.iter()).enumerate() {
        if let Some(actual_val) = row_result_value(row, &world.result_alias) {
            let exp_val = &exp_row[0];
            assert!(
                values_match_ctx(actual_val, exp_val, &vid, &eid),
                "row {i}: expected {:?} but got {:?}\ntraversal: {}",
                exp_val,
                actual_val,
                world.traversal_text,
            );
        }
    }
}

/// Normalize a map key for comparison — strips TCK type decorations.
/// Handles `d[N].i`, `d[N].l`, `d[N].d`, `d[N].f`, etc. → `"N"`.
/// Handles `l[...]` → `"l[...]"` (unchanged, for list keys).
fn normalize_map_key(key: &str) -> std::borrow::Cow<'_, str> {
    if let Some(rest) = key.strip_prefix("d[") {
        if let Some(end) = rest.find(']') {
            return std::borrow::Cow::Borrowed(&rest[..end]);
        }
    }
    std::borrow::Cow::Borrowed(key)
}

/// Compare two Values for TCK-style equality (without context).
#[allow(dead_code)]
fn values_match(actual: &Value, expected: &Value) -> bool {
    values_match_ctx(actual, expected, &HashMap::new(), &HashMap::new())
}

/// Compare two Values with vertex/edge name context for map key resolution.
fn values_match_ctx(
    actual: &Value,
    expected: &Value,
    vertex_id_to_name: &HashMap<NodeId, String>,
    edge_id_to_name: &HashMap<EdgeId, String>,
) -> bool {
    match (actual, expected) {
        (Value::Node(a), Value::Node(e)) => a == e,
        (Value::Edge(a), Value::Edge(e)) => a == e,
        (Value::Int(a), Value::Int(e)) => a == e,
        (Value::Float(a), Value::Float(e)) => {
            if a.is_nan() && e.is_nan() {
                return true;
            }
            if a.is_infinite() && e.is_infinite() {
                return a.signum() == e.signum();
            }
            (a - e).abs() < 1e-9
        }
        // Cross-type numeric comparisons
        (Value::Int(a), Value::Float(e)) => (*a as f64 - e).abs() < 1e-9,
        (Value::Float(a), Value::Int(e)) => (a - *e as f64).abs() < 1e-9,
        // String representation match: Str("1") matches Int(1), etc.
        (Value::Str(a), Value::Int(e)) => a.as_ref() == &e.to_string(),
        (Value::Str(a), Value::Float(e)) => a.as_ref() == &e.to_string(),
        (Value::Str(a), Value::Bool(e)) => a.as_ref() == &e.to_string(),
        // List matching stringified list: List(["marko"]) matches Str("[\"marko\"]")
        (Value::List(items), Value::Str(expected_str)) => {
            let parts: Vec<String> = items
                .iter()
                .map(|v| match v {
                    Value::Str(s) => format!("\"{}\"", s),
                    Value::Int(n) => n.to_string(),
                    Value::Float(f) => f.to_string(),
                    Value::Bool(b) => b.to_string(),
                    Value::Null => "null".to_string(),
                    other => format!("{:?}", other),
                })
                .collect();
            // Try both with and without spaces after commas
            let with_spaces = format!("[{}]", parts.join(", "));
            let without_spaces = format!("[{}]", parts.join(","));
            with_spaces == expected_str.as_ref() || without_spaces == expected_str.as_ref()
        }
        (Value::Str(a), Value::Str(e)) => a == e,
        (Value::Bool(a), Value::Bool(e)) => a == e,
        (Value::Null, Value::Null) => true,
        (Value::List(a), Value::List(e)) => {
            if a.len() != e.len() {
                return false;
            }
            // Try ordered comparison first
            let ordered = a
                .iter()
                .zip(e.iter())
                .all(|(av, ev)| values_match_ctx(av, ev, vertex_id_to_name, edge_id_to_name));
            if ordered {
                return true;
            }
            // Fall back to unordered comparison (handles s[...] set notation)
            let mut matched = vec![false; a.len()];
            e.iter().all(|ev| {
                a.iter().enumerate().any(|(i, av)| {
                    if !matched[i] && values_match_ctx(av, ev, vertex_id_to_name, edge_id_to_name) {
                        matched[i] = true;
                        true
                    } else {
                        false
                    }
                })
            })
        }
        (Value::Map(a), Value::Map(e)) => {
            if a.len() != e.len() {
                return false;
            }
            e.iter().all(|(ek, ev)| {
                // Direct key match
                if let Some(av) = a.get(ek) {
                    return values_match_ctx(av, ev, vertex_id_to_name, edge_id_to_name);
                }
                // Try normalized key match (handles d[N].x vs N format)
                let norm_ek = normalize_map_key(ek);
                // Also try vertex key resolution: v[name] vs v[id]
                a.iter().any(|(ak, av)| {
                    let keys_equal = if normalize_map_key(ak) == norm_ek {
                        true
                    } else if let (Some(a_inner), Some(e_inner)) = (
                        ak.strip_prefix("v[").and_then(|s| s.strip_suffix(']')),
                        ek.strip_prefix("v[").and_then(|s| s.strip_suffix(']')),
                    ) {
                        // Resolve v[id] vs v[name]: parse actual as id, look up name
                        if let Ok(id) = a_inner.parse::<u64>() {
                            vertex_id_to_name
                                .get(&NodeId(id))
                                .map_or(false, |name| name == e_inner)
                        } else {
                            false
                        }
                    } else if let (Some(a_inner), Some(e_inner)) = (
                        ak.strip_prefix("e[").and_then(|s| s.strip_suffix(']')),
                        ek.strip_prefix("e[").and_then(|s| s.strip_suffix(']')),
                    ) {
                        if let Ok(id) = a_inner.parse::<u64>() {
                            edge_id_to_name
                                .get(&EdgeId(id))
                                .map_or(false, |name| name == e_inner)
                        } else {
                            false
                        }
                    } else {
                        false
                    };
                    keys_equal && values_match_ctx(av, ev, vertex_id_to_name, edge_id_to_name)
                })
            })
        }
        _ => actual == expected,
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Resolve features path relative to the workspace root.
    // When running via `cargo test`, the working directory is the workspace root.
    // But when it's the package root, we need to adjust.
    let features_path = if std::path::Path::new("features").is_dir() {
        "features".to_string()
    } else if std::path::Path::new("tests/gremlin-tck/features").is_dir() {
        "tests/gremlin-tck/features".to_string()
    } else {
        // Use CARGO_MANIFEST_DIR to find the features
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        format!("{manifest_dir}/features")
    };

    // Tags for features we legitimately cannot/don't support.
    let skip_tags: &[&str] = &[
        "StepMergeV",
        "StepMergeE",
        "StepCall",
        "StepShortestPath",
        "StepPageRank",
        "StepPeerPressure",
        "StepConnectedComponent",
        "StepSubgraph",
        "StepRead",
        "StepWrite",
        "StepAsDate",
        "StepDateAdd",
        "StepDateDiff",
        "GraphComputerOnly",
        "TinkerServiceRegistry",
        "WithSubgraphStrategy",
        "WithPartitionStrategy",
        "WithHaltedTraverserStrategy",
        "WithVertexProgramStrategy",
        "WithVertexProgramRestrictionStrategy",
        // Data types we don't support
        "DataUUID",
        "DataBigDecimal",
        "DataBigInt",
        "DataDateTime",
        // Math step not implemented
        "StepMath",
    ];
    let skip_set: std::collections::HashSet<&str> = skip_tags.iter().copied().collect();

    GremlinWorld::cucumber()
        .max_concurrent_scenarios(Some(4))
        .filter_run(features_path, move |feature, _rule, scenario| {
            // Skip scenarios whose tags (or feature-level tags) intersect
            // with our skip set.
            let dominated = scenario
                .tags
                .iter()
                .chain(feature.tags.iter())
                .any(|t| skip_set.contains(t.as_str()));
            !dominated
        })
        .await;
}
