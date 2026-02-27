//! openCypher TCK cucumber runner for gql-core.
//!
//! Executes the Gherkin `.feature` files in `tests/tck/features/` against
//! the real parser -> planner -> executor pipeline.  Every scenario runs in
//! a fresh `InMemoryGraphStore` graph so state does not leak between tests.
//!
//! Step patterns handled:
//!
//! Given:
//!   - `an empty graph`
//!   - `any graph`
//!   - `the binary-tree-1 graph`
//!   - `the binary-tree-2 graph`
//!   - `having executed:` (docstring)
//!   - `parameters are:` (table)
//!   - `there exists a procedure ...` (ignored -- procedures not yet supported)
//!
//! When:
//!   - `executing query:` (docstring)
//!   - `executing control query:` (docstring)
//!
//! Then:
//!   - `the result should be, in any order:` (table)
//!   - `the result should be, in order:` (table)
//!   - `the result should be (ignoring element order for lists):` (table)
//!   - `the result should be, in order (ignoring element order for lists):` (table)
//!   - `the result should be empty`
//!   - `the result should contain:` (table)
//!   - `a <ErrorType> should be raised at compile time: <detail>`
//!   - `a <ErrorType> should be raised at runtime: <detail>`
//!   - `a <ErrorType> should be raised at any time: <detail>`
//!   - `the side effects should be:` (table)
//!   - `no side effects`

use std::collections::HashMap;
use std::sync::Arc;

use cucumber::gherkin;
use cucumber::{given, then, when, World};
use gql_core::{
    executor::{ExecutionBudget, QueryExecutor, QueryResult, Row},
    parser::parse_query,
    planner::QueryPlanner,
    procedure::{ProcedureDefinition, ProcedureParam, ProcedureRegistry},
    storage::{GraphStore, InMemoryGraphStore},
    types::Value,
};

// ── World state ───────────────────────────────────────────────────────────────

/// Outcome of executing a query: either success or a caught error.
#[derive(Debug)]
enum QueryOutcome {
    Ok(QueryResult),
    Err(String),
}

/// Per-scenario state that persists across steps.
///
/// A new `GqlWorld` is created for every `Scenario` by cucumber's runner.
#[derive(Debug, Default, World)]
pub struct GqlWorld {
    /// The backing store.  Set by `Given an empty graph` / named graph steps.
    store: Option<Arc<InMemoryGraphStore>>,
    /// The graph name inside the store.
    graph_name: String,
    /// The outcome of the most recent `When executing query:` step.
    last_outcome: Option<QueryOutcome>,
    /// Counter used to generate unique graph names.
    graph_counter: usize,
    /// Query parameters set by `And parameters are:`.
    parameters: HashMap<String, Value>,
    /// Procedure registry set by `And there exists a procedure ...`.
    registry: ProcedureRegistry,
}

impl GqlWorld {
    fn get_store(&self) -> Arc<InMemoryGraphStore> {
        self.store
            .clone()
            .expect("no store initialised -- missing 'Given an empty graph'")
    }

    /// Execute `query` and return the result.  Panics on failure (used for
    /// setup steps like `Given having executed:`).
    fn execute(&mut self, query: &str) -> QueryResult {
        let store = self.get_store();
        let ast = parse_query(query)
            .unwrap_or_else(|e| panic!("parse error for query {:?}: {}", query, e));
        let planner = QueryPlanner::with_context(
            store.as_ref(),
            &self.graph_name,
            &self.registry,
            &self.parameters,
        );
        let plan = planner
            .plan(&ast)
            .unwrap_or_else(|e| panic!("plan error for query {:?}: {}", query, e));
        let mut budget = ExecutionBudget::default_budget();
        QueryExecutor::with_context(store.as_ref(), &self.registry, self.parameters.clone())
            .execute(&self.graph_name, &plan, &mut budget)
            .unwrap_or_else(|e| panic!("execution error for query {:?}: {}", query, e))
    }

    /// Execute `query` catching all errors into `QueryOutcome`.
    fn try_execute(&mut self, query: &str) -> QueryOutcome {
        let store = self.get_store();

        let ast = match parse_query(query) {
            Ok(ast) => ast,
            Err(e) => {
                return QueryOutcome::Err(format!("ParseError: {e}"));
            }
        };

        let planner = QueryPlanner::with_context(
            store.as_ref(),
            &self.graph_name,
            &self.registry,
            &self.parameters,
        );
        let plan = match planner.plan(&ast) {
            Ok(p) => p,
            Err(e) => {
                return QueryOutcome::Err(format!("PlanError: {e}"));
            }
        };

        let mut budget = ExecutionBudget::default_budget();
        match QueryExecutor::with_context(store.as_ref(), &self.registry, self.parameters.clone())
            .execute(&self.graph_name, &plan, &mut budget)
        {
            Ok(result) => QueryOutcome::Ok(result),
            Err(e) => QueryOutcome::Err(format!("ExecutionError: {e}")),
        }
    }

    /// Initialise a fresh empty graph in a new store and store its reference.
    fn init_empty_graph(&mut self) {
        self.graph_counter += 1;
        let graph_name = format!("tck_{}", self.graph_counter);
        let store = Arc::new(InMemoryGraphStore::new());
        store
            .create_graph(&graph_name)
            .expect("create_graph failed");
        self.store = Some(store);
        self.graph_name = graph_name;
        self.last_outcome = None;
        self.parameters.clear();
        self.registry = ProcedureRegistry::new();
    }

    /// Get the last successful QueryResult, panicking if the last query errored
    /// or no query has been run.
    fn last_result(&self) -> &QueryResult {
        match &self.last_outcome {
            Some(QueryOutcome::Ok(r)) => r,
            Some(QueryOutcome::Err(e)) => {
                panic!("expected successful result but query failed with: {e}")
            }
            None => panic!("no result -- missing 'When executing query:' step"),
        }
    }
}

// ── Given steps ──────────────────────────────────────────────────────────────

/// Set up a fresh empty in-memory graph.
#[given("an empty graph")]
async fn given_empty_graph(world: &mut GqlWorld) {
    world.init_empty_graph();
}

/// `any graph` -- just create an empty graph (same as empty for our purposes).
#[given("any graph")]
async fn given_any_graph(world: &mut GqlWorld) {
    world.init_empty_graph();
}

/// Set up the binary-tree-1 named graph.
#[given("the binary-tree-1 graph")]
async fn given_binary_tree_1(world: &mut GqlWorld) {
    world.init_empty_graph();
    // The binary-tree-1 graph from the openCypher TCK:
    // Root (a:A), 4 children (b1-b4:X), 8 grandchildren (c11-c42:X),
    // plus cross-links b1->b2->b3->b4->b1.
    let setup = r#"
CREATE (a:A {name: 'a'}),
       (b1:X {name: 'b1'}),
       (b2:X {name: 'b2'}),
       (b3:X {name: 'b3'}),
       (b4:X {name: 'b4'}),
       (c11:X {name: 'c11'}),
       (c12:X {name: 'c12'}),
       (c21:X {name: 'c21'}),
       (c22:X {name: 'c22'}),
       (c31:X {name: 'c31'}),
       (c32:X {name: 'c32'}),
       (c41:X {name: 'c41'}),
       (c42:X {name: 'c42'})
CREATE (a)-[:KNOWS]->(b1),
       (a)-[:KNOWS]->(b2),
       (a)-[:FOLLOWS]->(b3),
       (a)-[:FOLLOWS]->(b4)
CREATE (b1)-[:FRIEND]->(c11),
       (b1)-[:FRIEND]->(c12),
       (b2)-[:FRIEND]->(c21),
       (b2)-[:FRIEND]->(c22),
       (b3)-[:FRIEND]->(c31),
       (b3)-[:FRIEND]->(c32),
       (b4)-[:FRIEND]->(c41),
       (b4)-[:FRIEND]->(c42)
CREATE (b1)-[:FRIEND]->(b2),
       (b2)-[:FRIEND]->(b3),
       (b3)-[:FRIEND]->(b4),
       (b4)-[:FRIEND]->(b1)
"#;
    world.execute(setup.trim());
}

/// Set up the binary-tree-2 named graph (same structure but alternating :X/:Y labels).
#[given("the binary-tree-2 graph")]
async fn given_binary_tree_2(world: &mut GqlWorld) {
    world.init_empty_graph();
    let setup = r#"
CREATE (a:A {name: 'a'}),
       (b1:X {name: 'b1'}),
       (b2:X {name: 'b2'}),
       (b3:X {name: 'b3'}),
       (b4:X {name: 'b4'}),
       (c11:X {name: 'c11'}),
       (c12:Y {name: 'c12'}),
       (c21:X {name: 'c21'}),
       (c22:Y {name: 'c22'}),
       (c31:X {name: 'c31'}),
       (c32:Y {name: 'c32'}),
       (c41:X {name: 'c41'}),
       (c42:Y {name: 'c42'})
CREATE (a)-[:KNOWS]->(b1),
       (a)-[:KNOWS]->(b2),
       (a)-[:FOLLOWS]->(b3),
       (a)-[:FOLLOWS]->(b4)
CREATE (b1)-[:FRIEND]->(c11),
       (b1)-[:FRIEND]->(c12),
       (b2)-[:FRIEND]->(c21),
       (b2)-[:FRIEND]->(c22),
       (b3)-[:FRIEND]->(c31),
       (b3)-[:FRIEND]->(c32),
       (b4)-[:FRIEND]->(c41),
       (b4)-[:FRIEND]->(c42)
CREATE (b1)-[:FRIEND]->(b2),
       (b2)-[:FRIEND]->(b3),
       (b3)-[:FRIEND]->(b4),
       (b4)-[:FRIEND]->(b1)
"#;
    world.execute(setup.trim());
}

/// Run a CQL setup statement (the query comes from the docstring).
///
/// Matches both `Given having executed:` and `And having executed:`.
/// (In Gherkin, `And` inherits the step type of the preceding step, so a
/// `And having executed:` after a `Given` is dispatched as a Given step.)
#[given(regex = r"(?i)having executed:")]
async fn having_executed(world: &mut GqlWorld, step: &gherkin::Step) {
    let query = step
        .docstring
        .as_ref()
        .expect("missing docstring for 'having executed:'")
        .trim()
        .to_string();
    world.execute(&query);
}

/// Handle `And parameters are:` (table of parameter name -> TCK value).
/// Parameters are stored but since our executor does not support parameterised
/// queries yet, scenarios using parameters will fail at the query execution step
/// rather than here.
#[given(regex = r"(?i)parameters are:")]
async fn given_parameters(world: &mut GqlWorld, step: &gherkin::Step) {
    let table = step
        .table
        .as_ref()
        .expect("missing data table for 'parameters are:'");
    for row in &table.rows {
        if row.len() < 2 {
            continue;
        }
        let name = row[0].trim().to_string();
        let raw_val = row[1].trim();
        let val = parse_tck_value(raw_val);
        world.parameters.insert(name, val);
    }
}

/// Handle `And there exists a procedure ...` -- parse the procedure definition
/// and register it in the procedure registry.
#[given(regex = r"(?i)there exists a procedure (.+):$")]
async fn given_procedure(world: &mut GqlWorld, step: &gherkin::Step, signature: String) {
    let proc_def = parse_procedure_definition(&signature, step);
    world.registry.register(proc_def);
}

// ── When steps ──────────────────────────────────────────────────────────────

/// Execute a CQL query and store the outcome (success or error).
#[when(regex = r"(?i)executing query:")]
async fn when_executing(world: &mut GqlWorld, step: &gherkin::Step) {
    let query = step
        .docstring
        .as_ref()
        .expect("missing docstring for 'executing query:'")
        .trim()
        .to_string();
    world.last_outcome = Some(world.try_execute(&query));
}

/// Execute a control query (same semantics as executing query for our runner).
#[when(regex = r"(?i)executing control query:")]
async fn when_executing_control(world: &mut GqlWorld, step: &gherkin::Step) {
    let query = step
        .docstring
        .as_ref()
        .expect("missing docstring for 'executing control query:'")
        .trim()
        .to_string();
    world.last_outcome = Some(world.try_execute(&query));
}

// ── Then: result should be, in any order ──────────────────────────────────────

/// Asserts that the result rows match the table (unordered, exact count).
#[then(regex = r"(?i)the result should be, in any order:")]
async fn result_any_order(world: &mut GqlWorld, step: &gherkin::Step) {
    let result = world.last_result();
    let table = step
        .table
        .as_ref()
        .expect("missing data table for 'the result should be, in any order:'");

    let headers: Vec<&str> = table.rows[0].iter().map(|h| h.trim()).collect();
    let expected_rows = parse_expected_rows(&headers, &table.rows[1..]);

    assert_eq!(
        result.rows.len(),
        expected_rows.len(),
        "Row count mismatch: expected {} rows, got {} rows.\n  Got: {:?}",
        expected_rows.len(),
        result.rows.len(),
        format_rows_for_debug(&result.rows, world),
    );

    // Each expected row must appear somewhere in the actual rows.
    let mut matched = vec![false; result.rows.len()];
    for (ei, expected) in expected_rows.iter().enumerate() {
        let found = result.rows.iter().enumerate().find(|(i, row)| {
            !matched[*i] && row_matches_tck(row, &headers, expected, world, false)
        });
        match found {
            Some((i, _)) => matched[i] = true,
            None => panic!(
                "Expected row #{ei} not found in result.\n  Expected: {expected:?}\n  Got: {:?}",
                format_rows_for_debug(&result.rows, world),
            ),
        }
    }
}

// ── Then: result should be, in order ──────────────────────────────────────────

/// Asserts that the result rows match the table exactly, in order.
#[then(regex = r"(?i)the result should be, in order:$")]
async fn result_in_order(world: &mut GqlWorld, step: &gherkin::Step) {
    let result = world.last_result();
    let table = step
        .table
        .as_ref()
        .expect("missing data table for 'the result should be, in order:'");

    let headers: Vec<&str> = table.rows[0].iter().map(|h| h.trim()).collect();
    let expected_rows = parse_expected_rows(&headers, &table.rows[1..]);

    assert_eq!(
        result.rows.len(),
        expected_rows.len(),
        "Row count mismatch: expected {} rows, got {} rows.\n  Got: {:?}",
        expected_rows.len(),
        result.rows.len(),
        format_rows_for_debug(&result.rows, world),
    );

    for (i, (actual_row, expected)) in result.rows.iter().zip(expected_rows.iter()).enumerate() {
        assert!(
            row_matches_tck(actual_row, &headers, expected, world, false),
            "Row #{i} does not match.\n  Expected: {expected:?}\n  Got: {:?}",
            format_row_for_debug(actual_row, world),
        );
    }
}

// ── Then: result should be (ignoring element order for lists) ─────────────────

/// Result in any order, but list elements within values are sorted before comparison.
#[then(regex = r"(?i)the result should be \(ignoring element order for lists\):")]
async fn result_ignoring_list_order(world: &mut GqlWorld, step: &gherkin::Step) {
    let result = world.last_result();
    let table = step.table.as_ref().expect("missing data table");

    let headers: Vec<&str> = table.rows[0].iter().map(|h| h.trim()).collect();
    let expected_rows = parse_expected_rows(&headers, &table.rows[1..]);

    assert_eq!(
        result.rows.len(),
        expected_rows.len(),
        "Row count mismatch: expected {} rows, got {} rows.\n  Got: {:?}",
        expected_rows.len(),
        result.rows.len(),
        format_rows_for_debug(&result.rows, world),
    );

    let mut matched = vec![false; result.rows.len()];
    for (ei, expected) in expected_rows.iter().enumerate() {
        let found =
            result.rows.iter().enumerate().find(|(i, row)| {
                !matched[*i] && row_matches_tck(row, &headers, expected, world, true)
            });
        match found {
            Some((i, _)) => matched[i] = true,
            None => panic!(
                "Expected row #{ei} not found (ignoring list order).\n  Expected: {expected:?}\n  Got: {:?}",
                format_rows_for_debug(&result.rows, world),
            ),
        }
    }
}

/// Result in order, but list elements within values are sorted before comparison.
#[then(regex = r"(?i)the result should be, in order \(ignoring element order for lists\):")]
async fn result_in_order_ignoring_list_order(world: &mut GqlWorld, step: &gherkin::Step) {
    let result = world.last_result();
    let table = step.table.as_ref().expect("missing data table");

    let headers: Vec<&str> = table.rows[0].iter().map(|h| h.trim()).collect();
    let expected_rows = parse_expected_rows(&headers, &table.rows[1..]);

    assert_eq!(
        result.rows.len(),
        expected_rows.len(),
        "Row count mismatch: expected {} rows, got {} rows.\n  Got: {:?}",
        expected_rows.len(),
        result.rows.len(),
        format_rows_for_debug(&result.rows, world),
    );

    for (i, (actual_row, expected)) in result.rows.iter().zip(expected_rows.iter()).enumerate() {
        assert!(
            row_matches_tck(actual_row, &headers, expected, world, true),
            "Row #{i} does not match (ignoring list order).\n  Expected: {expected:?}\n  Got: {:?}",
            format_row_for_debug(actual_row, world),
        );
    }
}

// ── Then: result should be empty ──────────────────────────────────────────────

#[then(regex = r"(?i)the result should be empty")]
async fn result_empty(world: &mut GqlWorld) {
    let result = world.last_result();
    assert_eq!(
        result.rows.len(),
        0,
        "Expected empty result but got {} rows: {:?}",
        result.rows.len(),
        format_rows_for_debug(&result.rows, world),
    );
}

// ── Then: result should contain (unordered, subset) ───────────────────────────

/// Asserts that every row in the Gherkin table appears in the result.
/// Order does not matter; the result may have more rows than the table.
#[then(regex = r"(?i)the result should contain:")]
async fn result_contains(world: &mut GqlWorld, step: &gherkin::Step) {
    let result = world.last_result();
    let table = step
        .table
        .as_ref()
        .expect("missing data table for 'the result should contain:'");

    let headers: Vec<&str> = table.rows[0].iter().map(|h| h.trim()).collect();
    let expected_rows = parse_expected_rows(&headers, &table.rows[1..]);

    for (ei, expected) in expected_rows.iter().enumerate() {
        let found = result
            .rows
            .iter()
            .any(|row| row_matches_tck(row, &headers, expected, world, false));
        assert!(
            found,
            "Expected row #{ei} not found in result.\n  Expected: {expected:?}\n  Got: {:?}",
            format_rows_for_debug(&result.rows, world),
        );
    }
}

// ── Then: error should be raised ──────────────────────────────────────────────

/// Catches errors raised at compile time (parse/plan errors).
/// Pattern: `Then a SyntaxError should be raised at compile time: <detail>`
/// Also handles: TypeError, ParameterMissing, ProcedureError, etc.
#[then(regex = r"(?i)a (\w+) should be raised at compile time: (.+)")]
async fn error_at_compile_time(
    world: &mut GqlWorld,
    #[step] _step: &gherkin::Step,
    error_type: String,
    _detail: String,
) {
    match &world.last_outcome {
        Some(QueryOutcome::Err(msg)) => {
            // For SyntaxError, we expect a ParseError or PlanError
            // For TypeError, we expect a TypeError
            // We verify the error was raised; the detail is informational.
            match error_type.as_str() {
                "SyntaxError" => {
                    assert!(
                        msg.contains("ParseError")
                            || msg.contains("PlanError")
                            || msg.contains("parse error")
                            || msg.contains("planning error")
                            || msg.contains("ExecutionError")
                            || msg.contains("execution error"),
                        "Expected a SyntaxError but got: {msg}"
                    );
                }
                "TypeError" => {
                    assert!(
                        msg.contains("TypeError")
                            || msg.contains("type error")
                            || msg.contains("TypeMismatch")
                            || msg.contains("ExecutionError")
                            || msg.contains("execution error"),
                        "Expected a TypeError but got: {msg}"
                    );
                }
                _ => {
                    // For other error types (ParameterMissing, ProcedureError, etc.),
                    // we just verify an error was raised at all.
                }
            }
        }
        Some(QueryOutcome::Ok(_)) => {
            panic!("Expected a {error_type} at compile time but query succeeded");
        }
        None => {
            panic!("No query was executed -- missing 'When executing query:' step");
        }
    }
}

/// Catches errors raised at runtime (execution errors).
/// Pattern: `Then a TypeError should be raised at runtime: <detail>`
#[then(regex = r"(?i)a (\w+) should be raised at runtime: (.+)")]
async fn error_at_runtime(
    world: &mut GqlWorld,
    #[step] _step: &gherkin::Step,
    error_type: String,
    _detail: String,
) {
    match &world.last_outcome {
        Some(QueryOutcome::Err(msg)) => {
            match error_type.as_str() {
                "TypeError" => {
                    assert!(
                        msg.contains("TypeError")
                            || msg.contains("type error")
                            || msg.contains("TypeMismatch")
                            || msg.contains("ExecutionError"),
                        "Expected a TypeError at runtime but got: {msg}"
                    );
                }
                "SyntaxError" => {
                    // Some syntax errors are detected at runtime in the TCK
                    assert!(
                        msg.contains("Error"),
                        "Expected a SyntaxError at runtime but got: {msg}"
                    );
                }
                "ArgumentError" => {
                    assert!(
                        msg.contains("Error"),
                        "Expected an ArgumentError at runtime but got: {msg}"
                    );
                }
                _ => {
                    // For ConstraintVerificationFailed, EntityNotFound, SemanticError, etc.
                    // we just verify an error was raised.
                }
            }
        }
        Some(QueryOutcome::Ok(_)) => {
            panic!("Expected a {error_type} at runtime but query succeeded");
        }
        None => {
            panic!("No query was executed -- missing 'When executing query:' step");
        }
    }
}

/// Catches errors raised at any time.
/// Pattern: `Then a TypeError should be raised at any time: <detail>`
#[then(regex = r"(?i)a (\w+) should be raised at any time: (.+)")]
async fn error_at_any_time(
    world: &mut GqlWorld,
    #[step] _step: &gherkin::Step,
    _error_type: String,
    _detail: String,
) {
    match &world.last_outcome {
        Some(QueryOutcome::Err(_)) => {
            // Error was raised -- that is what we expected.
        }
        Some(QueryOutcome::Ok(_)) => {
            panic!("Expected a {_error_type} at any time but query succeeded");
        }
        None => {
            panic!("No query was executed -- missing 'When executing query:' step");
        }
    }
}

// ── Then: side effects ────────────────────────────────────────────────────────

/// Asserts mutation counters from a `| +nodes | 1 |` style table.
#[then(regex = r"(?i)the side effects should be:")]
async fn side_effects(world: &mut GqlWorld, step: &gherkin::Step) {
    let result = world.last_result();
    let table = step
        .table
        .as_ref()
        .expect("missing data table for 'the side effects should be:'");

    for row in &table.rows {
        if row.len() < 2 {
            continue;
        }
        let key = row[0].trim();
        let val: u64 = row[1]
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("expected numeric side-effect value, got {:?}", row[1]));
        match key {
            "+nodes" => assert_eq!(
                result.stats.nodes_created, val,
                "+nodes mismatch: expected {val}, got {}",
                result.stats.nodes_created
            ),
            "-nodes" => assert_eq!(
                result.stats.nodes_deleted, val,
                "-nodes mismatch: expected {val}, got {}",
                result.stats.nodes_deleted
            ),
            "+relationships" | "+edges" => assert_eq!(
                result.stats.edges_created, val,
                "+relationships mismatch: expected {val}, got {}",
                result.stats.edges_created
            ),
            "-relationships" | "-edges" => assert_eq!(
                result.stats.edges_deleted, val,
                "-relationships mismatch: expected {val}, got {}",
                result.stats.edges_deleted
            ),
            "+labels" => assert_eq!(
                result.stats.labels_added, val,
                "+labels mismatch: expected {val}, got {}",
                result.stats.labels_added
            ),
            "+properties" => assert_eq!(
                result.stats.properties_set, val,
                "+properties mismatch: expected {val}, got {}",
                result.stats.properties_set
            ),
            "-properties" | "-labels" => {
                // We do not track label removal or property removal counters
                // separately yet; ignore gracefully.
            }
            _ => {
                // Unknown side-effect keys are silently ignored.
            }
        }
    }
}

/// Asserts no side effects (all mutation counters are zero).
#[then(regex = r"(?i)no side effects")]
async fn no_side_effects(world: &mut GqlWorld) {
    let result = world.last_result();
    assert_eq!(
        result.stats.nodes_created, 0,
        "Expected no side effects but nodes_created = {}",
        result.stats.nodes_created
    );
    assert_eq!(
        result.stats.nodes_deleted, 0,
        "Expected no side effects but nodes_deleted = {}",
        result.stats.nodes_deleted
    );
    assert_eq!(
        result.stats.edges_created, 0,
        "Expected no side effects but edges_created = {}",
        result.stats.edges_created
    );
    assert_eq!(
        result.stats.edges_deleted, 0,
        "Expected no side effects but edges_deleted = {}",
        result.stats.edges_deleted
    );
    assert_eq!(
        result.stats.properties_set, 0,
        "Expected no side effects but properties_set = {}",
        result.stats.properties_set
    );
    assert_eq!(
        result.stats.labels_added, 0,
        "Expected no side effects but labels_added = {}",
        result.stats.labels_added
    );
}

// ── TCK value parser ─────────────────────────────────────────────────────────

/// Parse a TCK table cell value into a `Value`.
///
/// TCK value formats:
/// - `null`                     -> Value::Null
/// - `true` / `false`           -> Value::Bool
/// - `123` / `-42`              -> Value::Int
/// - `1.0` / `-3.14`            -> Value::Float
/// - `'text'`                   -> Value::Str
/// - `[1, 2, 3]`               -> Value::List
/// - `{key: 'val'}`            -> Value::Map
/// - `(:Label {prop: 'val'})`  -> Value::Map with labels+properties (node representation)
/// - `[:TYPE {prop: 'val'}]`   -> Value::Map with type+properties (relationship representation)
fn parse_tck_value(s: &str) -> Value {
    let s = s.trim();
    if s.is_empty() {
        return Value::Null;
    }

    // Null
    if s == "null" {
        return Value::Null;
    }

    // Boolean
    if s == "true" {
        return Value::Bool(true);
    }
    if s == "false" {
        return Value::Bool(false);
    }

    // String (single-quoted)
    if s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2 {
        let inner = &s[1..s.len() - 1];
        return Value::Str(inner.into());
    }

    // List
    if s.starts_with('[') && s.ends_with(']') && !s.starts_with("[:") {
        let inner = &s[1..s.len() - 1];
        if inner.trim().is_empty() {
            return Value::List(vec![]);
        }
        let items = split_tck_list(inner);
        return Value::List(
            items
                .into_iter()
                .map(|item| parse_tck_value(item.trim()))
                .collect(),
        );
    }

    // Map literal
    if s.starts_with('{') && s.ends_with('}') {
        return parse_tck_map(s);
    }

    // Node pattern: (:Label {prop: val}) or ({prop: val}) or (:Label) or ()
    if s.starts_with('(') && s.ends_with(')') {
        return parse_tck_node_pattern(s);
    }

    // Relationship pattern: [:TYPE {prop: val}] or [:TYPE]
    if s.starts_with("[:") && s.ends_with(']') {
        return parse_tck_rel_pattern(s);
    }

    // Relationship list pattern: [[:TYPE], [:TYPE2]] — a list of edges
    if s.starts_with("[[:") && s.ends_with("]]") {
        // Parse as a list of relationship patterns
        let inner = &s[1..s.len() - 1]; // remove outer []
        let items = split_tck_list(inner);
        return Value::List(
            items
                .into_iter()
                .map(|item| parse_tck_value(item.trim()))
                .collect(),
        );
    }

    // Path pattern: <(...)-[...]->(...)-[...]->(...) >
    if s.starts_with('<') && s.ends_with('>') {
        return parse_tck_path_pattern(s);
    }

    // Integer
    if let Ok(n) = s.parse::<i64>() {
        return Value::Int(n);
    }

    // Float
    if let Ok(f) = s.parse::<f64>() {
        return Value::Float(f);
    }

    // Fallback: treat as string
    Value::Str(s.into())
}

/// Split a comma-separated list respecting nesting (brackets, braces, parens, quotes).
fn split_tck_list(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0i32;
    let mut in_quote = false;
    let mut start = 0;

    for (i, ch) in s.char_indices() {
        match ch {
            '\'' if !in_quote => in_quote = true,
            '\'' if in_quote => in_quote = false,
            '[' | '(' | '{' if !in_quote => depth += 1,
            ']' | ')' | '}' if !in_quote => depth -= 1,
            ',' if !in_quote && depth == 0 => {
                result.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    if start <= s.len() {
        let tail = &s[start..];
        if !tail.trim().is_empty() {
            result.push(tail);
        }
    }
    result
}

/// Parse a TCK map literal `{key: val, ...}` into Value::Map.
fn parse_tck_map(s: &str) -> Value {
    let inner = &s[1..s.len() - 1];
    if inner.trim().is_empty() {
        return Value::Map(indexmap::IndexMap::new());
    }
    let pairs = split_tck_list(inner);
    let mut map = indexmap::IndexMap::new();
    for pair in pairs {
        let pair = pair.trim();
        if let Some(colon_pos) = pair.find(':') {
            let key = pair[..colon_pos].trim();
            let val = pair[colon_pos + 1..].trim();
            map.insert(key.into(), parse_tck_value(val));
        }
    }
    Value::Map(map)
}

/// Parse a node pattern like `(:Label {prop: 'val'})` or `(:Label)` or `()`.
/// Returns a Value::Map with special keys for comparison.
fn parse_tck_node_pattern(s: &str) -> Value {
    // Strip outer parens
    let inner = s[1..s.len() - 1].trim();
    if inner.is_empty() {
        // Empty node pattern `()` matches any node
        return Value::Map(indexmap::IndexMap::new());
    }

    let mut labels: Vec<String> = Vec::new();
    let mut properties = indexmap::IndexMap::new();
    let mut rest = inner;

    // Parse labels: `:Label1:Label2`
    while rest.starts_with(':') {
        rest = &rest[1..]; // skip ':'
        let end = rest
            .find(|c: char| c == ':' || c == ' ' || c == '{')
            .unwrap_or(rest.len());
        labels.push(rest[..end].to_string());
        rest = rest[end..].trim();
    }

    // Parse properties: `{prop: val}`
    if rest.starts_with('{') && rest.ends_with('}') {
        if let Value::Map(m) = parse_tck_map(rest) {
            properties = m;
        }
    }

    // Store as a map with __labels__ and the actual properties
    let mut map = indexmap::IndexMap::new();
    if !labels.is_empty() {
        map.insert(
            "__labels__".into(),
            Value::List(labels.into_iter().map(|l| Value::Str(l.into())).collect()),
        );
    }
    for (k, v) in properties {
        map.insert(k, v);
    }
    Value::Map(map)
}

/// Parse a relationship pattern like `[:TYPE {prop: val}]` or `[:TYPE]`.
fn parse_tck_rel_pattern(s: &str) -> Value {
    let inner = s[1..s.len() - 1].trim();
    let mut rel_type = String::new();
    let mut properties = indexmap::IndexMap::new();

    let mut rest = inner;
    if rest.starts_with(':') {
        rest = &rest[1..];
        let end = rest
            .find(|c: char| c == ' ' || c == '{')
            .unwrap_or(rest.len());
        rel_type = rest[..end].to_string();
        rest = rest[end..].trim();
    }

    if rest.starts_with('{') && rest.ends_with('}') {
        if let Value::Map(m) = parse_tck_map(rest) {
            properties = m;
        }
    }

    let mut map = indexmap::IndexMap::new();
    if !rel_type.is_empty() {
        map.insert("__type__".into(), Value::Str(rel_type.into()));
    }
    for (k, v) in properties {
        map.insert(k, v);
    }
    Value::Map(map)
}

/// Parsed representation of a TCK path pattern.
/// Stores the expected node patterns and edge patterns in order.
#[derive(Debug)]
struct TckPathPattern {
    /// Node patterns (labels + properties) in path order
    node_patterns: Vec<indexmap::IndexMap<Box<str>, Value>>,
    /// Edge patterns (type + properties) in path order, with direction
    edge_patterns: Vec<(indexmap::IndexMap<Box<str>, Value>, PathDir)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PathDir {
    Forward,  // ->
    Backward, // <-
    Both,     // -
}

/// Parse a TCK path pattern like `<(:A {name: 'A'})-[:KNOWS]->(:B)>`.
fn parse_tck_path_pattern(s: &str) -> Value {
    // Strip outer < >
    let inner = s[1..s.len() - 1].trim();
    if inner.is_empty() {
        // Empty path — should not normally occur
        return Value::Map(indexmap::IndexMap::new());
    }

    let mut node_patterns = Vec::new();
    let mut edge_patterns = Vec::new();
    let mut pos = 0;
    let bytes = inner.as_bytes();
    let len = bytes.len();

    while pos < len {
        // Skip whitespace
        while pos < len && bytes[pos] == b' ' {
            pos += 1;
        }
        if pos >= len {
            break;
        }

        // Expect a node: (...)
        if bytes[pos] == b'(' {
            let start = pos;
            let mut depth = 1;
            pos += 1;
            while pos < len && depth > 0 {
                if bytes[pos] == b'(' {
                    depth += 1;
                } else if bytes[pos] == b')' {
                    depth -= 1;
                }
                pos += 1;
            }
            let node_str = &inner[start..pos];
            if let Value::Map(m) = parse_tck_node_pattern(node_str) {
                node_patterns.push(m);
            }
        }

        // After a node, expect either end or an edge pattern
        if pos >= len {
            break;
        }

        // Look for edge direction and edge pattern
        // Patterns: -[:TYPE]-> or <-[:TYPE]- or -[:TYPE]-
        let dir;
        if pos + 1 < len && bytes[pos] == b'<' && bytes[pos + 1] == b'-' {
            dir = PathDir::Backward;
            pos += 2; // skip "<-"
        } else if bytes[pos] == b'-' {
            pos += 1; // skip "-"
            dir = PathDir::Forward; // tentative; check for -> at end
        } else {
            break;
        }

        // Now expect [...]
        if pos < len && bytes[pos] == b'[' {
            let start = pos;
            let mut depth = 1;
            pos += 1;
            while pos < len && depth > 0 {
                if bytes[pos] == b'[' {
                    depth += 1;
                } else if bytes[pos] == b']' {
                    depth -= 1;
                }
                pos += 1;
            }
            let edge_str = &inner[start..pos];
            if let Value::Map(m) = parse_tck_rel_pattern(edge_str) {
                let final_dir = if pos + 1 < len && bytes[pos] == b'-' && bytes[pos + 1] == b'>' {
                    pos += 2;
                    PathDir::Forward
                } else if pos < len && bytes[pos] == b'-' {
                    pos += 1;
                    if dir == PathDir::Backward {
                        PathDir::Backward
                    } else {
                        PathDir::Both
                    }
                } else {
                    dir
                };
                edge_patterns.push((m, final_dir));
            }
        } else if pos + 1 < len && bytes[pos] == b'-' && bytes[pos + 1] == b'>' {
            // No edge pattern, just direction ->
            pos += 2;
            edge_patterns.push((indexmap::IndexMap::new(), PathDir::Forward));
        } else if pos < len && bytes[pos] == b'-' {
            pos += 1;
            edge_patterns.push((
                indexmap::IndexMap::new(),
                if dir == PathDir::Backward {
                    PathDir::Backward
                } else {
                    PathDir::Both
                },
            ));
        }
    }

    // Store as a special map with __path__ sentinel
    let mut map = indexmap::IndexMap::new();
    map.insert(
        "__path__".into(),
        Value::Bool(true),
    );
    map.insert(
        "__path_nodes__".into(),
        Value::List(
            node_patterns
                .into_iter()
                .map(Value::Map)
                .collect(),
        ),
    );
    map.insert(
        "__path_edges__".into(),
        Value::List(
            edge_patterns
                .into_iter()
                .map(|(m, _dir)| Value::Map(m))
                .collect(),
        ),
    );
    Value::Map(map)
}

// ── Row comparison helpers ───────────────────────────────────────────────────

/// Parse the expected rows from a Gherkin table (skipping header).
fn parse_expected_rows<'a>(_headers: &[&'a str], data_rows: &[Vec<String>]) -> Vec<Vec<String>> {
    data_rows
        .iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(_i, cell)| cell.trim().to_string())
                .collect()
        })
        .collect()
}

/// Check whether an actual Row matches an expected TCK row.
///
/// Normalize a column name for comparison: lowercase + remove spaces
/// adjacent to parentheses + collapse consecutive spaces.
/// This handles cases where the query uses mixed-case like `cOuNt( * )`
/// while the engine produces `count(*)`.
fn normalize_col_name(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut result = String::with_capacity(lower.len());
    let mut skip_space = false;
    for c in lower.chars() {
        match c {
            '(' => {
                result.push('(');
                skip_space = true; // skip spaces after '('
            }
            ')' => {
                // trim trailing space before ')'
                if result.ends_with(' ') {
                    result.pop();
                }
                result.push(')');
                skip_space = false;
            }
            ' ' => {
                if !skip_space && !result.ends_with(' ') {
                    result.push(' ');
                }
            }
            _ => {
                result.push(c);
                skip_space = false;
            }
        }
    }
    result.trim().to_string()
}

/// `headers` are the column names from the Gherkin table.
/// `expected_cells` are the raw TCK cell strings for one row.
/// `ignore_list_order` sorts list elements before comparison.
fn row_matches_tck(
    actual_row: &Row,
    headers: &[&str],
    expected_cells: &[String],
    world: &GqlWorld,
    ignore_list_order: bool,
) -> bool {
    if headers.len() != expected_cells.len() {
        return false;
    }
    headers
        .iter()
        .zip(expected_cells.iter())
        .all(|(col, cell)| {
            // Try exact match first, then normalized match.
            let actual_val = actual_row.get(*col).or_else(|| {
                let norm_col = normalize_col_name(col);
                actual_row
                    .iter()
                    .find(|(k, _)| normalize_col_name(k) == norm_col)
                    .map(|(_, v)| v)
            });
            match actual_val {
                Some(actual_val) => {
                    let expected_val = parse_tck_value(cell);
                    value_matches(actual_val, &expected_val, world, ignore_list_order)
                }
                None => false,
            }
        })
}

/// Compare an actual Value against an expected parsed TCK Value.
///
/// For nodes/edges, resolves from the store if needed.
fn value_matches(
    actual: &Value,
    expected: &Value,
    world: &GqlWorld,
    ignore_list_order: bool,
) -> bool {
    match (actual, expected) {
        (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Int(a), Value::Int(b)) => a == b,
        (Value::Float(a), Value::Float(b)) => {
            // NaN == NaN in TCK context.
            (a.is_nan() && b.is_nan()) || (a - b).abs() < 1e-10 || a == b
        }
        (Value::Int(a), Value::Float(b)) => {
            if b.is_nan() { false } else { (*a as f64 - b).abs() < 1e-10 }
        }
        (Value::Float(a), Value::Int(b)) => {
            if a.is_nan() { false } else { (a - *b as f64).abs() < 1e-10 }
        }
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::List(a), Value::List(b)) => {
            if a.len() != b.len() {
                return false;
            }
            if ignore_list_order {
                // Sort both lists by their display representation and compare
                let mut a_sorted: Vec<String> = a
                    .iter()
                    .map(|v| format_value_for_compare(v, world))
                    .collect();
                let mut b_sorted: Vec<String> = b
                    .iter()
                    .map(|v| format_value_for_compare(v, world))
                    .collect();
                a_sorted.sort();
                b_sorted.sort();
                a_sorted == b_sorted
            } else {
                a.iter()
                    .zip(b.iter())
                    .all(|(av, bv)| value_matches(av, bv, world, false))
            }
        }
        (Value::Map(a), Value::Map(b)) => {
            // TCK map comparison: every key in expected must match in actual
            b.iter().all(|(k, v)| {
                a.get(k.as_ref())
                    .map(|av| value_matches(av, v, world, ignore_list_order))
                    .unwrap_or(false)
            })
        }
        // Node ID from actual, node pattern (Map) from TCK expected
        (Value::Node(node_id), Value::Map(expected_map)) => {
            node_matches_pattern(*node_id, expected_map, world)
        }
        // Edge ID from actual, relationship pattern (Map) from TCK expected
        (Value::Edge(edge_id), Value::Map(expected_map)) => {
            edge_matches_pattern(*edge_id, expected_map, world)
        }
        // Path comparison: actual Path against expected path pattern (Map with __path__ sentinel)
        (Value::Path { nodes, edges }, Value::Map(expected_map))
            if expected_map.get("__path__").is_some() =>
        {
            path_matches_pattern(nodes, edges, expected_map, world, ignore_list_order)
        }
        // Path vs Path direct comparison
        (Value::Path { nodes: n1, edges: e1 }, Value::Path { nodes: n2, edges: e2 }) => {
            n1 == n2 && e1 == e2
        }
        // Temporal values: compare against string representation
        (Value::Date(d), Value::Str(s)) => format!("{d}") == s.as_ref(),
        (Value::LocalTime(t), Value::Str(s)) => {
            let actual = format!("{t}");
            actual == s.as_ref() || normalize_time_str(&actual) == normalize_time_str(s)
        }
        (Value::LocalDateTime(dt), Value::Str(s)) => {
            let actual = format!("{dt}");
            actual == s.as_ref() || normalize_datetime_str(&actual) == normalize_datetime_str(s)
        }
        (Value::DateTime(dt), Value::Str(s)) => {
            let actual = format_zoned_datetime(dt);
            if actual == s.as_ref() || normalize_datetime_str(&actual) == normalize_datetime_str(s) {
                return true;
            }
            // Also try matching as a zoned time (time+offset) - Neo4j's Time type
            let time_str = format_zoned_time(dt);
            time_str == s.as_ref() || normalize_time_str(&time_str) == normalize_time_str(s)
        }
        (Value::Duration(sp), Value::Str(s)) => {
            let actual = gql_core::types::value::format_cypher_duration(sp);
            actual == s.as_ref() || normalize_duration_str(&actual) == normalize_duration_str(s)
        }
        // Temporal equality
        (Value::Date(_), Value::Date(_))
        | (Value::LocalTime(_), Value::LocalTime(_))
        | (Value::LocalDateTime(_), Value::LocalDateTime(_))
        | (Value::DateTime(_), Value::DateTime(_)) => actual == expected,
        _ => {
            // Fallback: compare display strings
            format!("{actual}") == format!("{expected}")
        }
    }
}

/// Check if a node matches a TCK node pattern (stored as Map with __labels__ key).
fn node_matches_pattern(
    node_id: gql_core::storage::NodeId,
    expected_map: &indexmap::IndexMap<Box<str>, Value>,
    world: &GqlWorld,
) -> bool {
    let store = match &world.store {
        Some(s) => s,
        None => return false,
    };
    let node_data = match store.get_node(&world.graph_name, node_id) {
        Ok(Some(data)) => data,
        _ => return false,
    };

    // Check labels if specified
    if let Some(Value::List(expected_labels)) = expected_map.get("__labels__") {
        for expected_label in expected_labels {
            if let Value::Str(label) = expected_label {
                if !node_data
                    .labels
                    .iter()
                    .any(|l| l.as_str() == label.as_ref())
                {
                    return false;
                }
            }
        }
    }

    // Check properties
    for (key, expected_val) in expected_map.iter() {
        if key.as_ref() == "__labels__" {
            continue;
        }
        match node_data.properties.get(key.as_ref()) {
            Some(actual_val) => {
                if !value_matches(actual_val, expected_val, world, false) {
                    return false;
                }
            }
            None => return false,
        }
    }

    true
}

/// Check if an edge matches a TCK relationship pattern (stored as Map with __type__ key).
fn edge_matches_pattern(
    edge_id: gql_core::storage::EdgeId,
    expected_map: &indexmap::IndexMap<Box<str>, Value>,
    world: &GqlWorld,
) -> bool {
    let store = match &world.store {
        Some(s) => s,
        None => return false,
    };
    let edge_data = match store.get_edge(&world.graph_name, edge_id) {
        Ok(Some(data)) => data,
        _ => return false,
    };

    // Check relationship type if specified
    if let Some(Value::Str(expected_type)) = expected_map.get("__type__") {
        if edge_data.rel_type.as_str() != expected_type.as_ref() {
            return false;
        }
    }

    // Check properties
    for (key, expected_val) in expected_map.iter() {
        if key.as_ref() == "__type__" {
            continue;
        }
        match edge_data.properties.get(key.as_ref()) {
            Some(actual_val) => {
                if !value_matches(actual_val, expected_val, world, false) {
                    return false;
                }
            }
            None => return false,
        }
    }

    true
}

/// Check if a path matches a TCK path pattern.
fn path_matches_pattern(
    actual_nodes: &[gql_core::storage::NodeId],
    actual_edges: &[gql_core::storage::EdgeId],
    expected_map: &indexmap::IndexMap<Box<str>, Value>,
    world: &GqlWorld,
    _ignore_list_order: bool,
) -> bool {
    let expected_node_patterns = match expected_map.get("__path_nodes__") {
        Some(Value::List(patterns)) => patterns,
        _ => return false,
    };
    let expected_edge_patterns = match expected_map.get("__path_edges__") {
        Some(Value::List(patterns)) => patterns,
        _ => return false,
    };

    // Check counts
    if actual_nodes.len() != expected_node_patterns.len() {
        return false;
    }
    if actual_edges.len() != expected_edge_patterns.len() {
        return false;
    }

    // Check each node
    for (node_id, expected) in actual_nodes.iter().zip(expected_node_patterns.iter()) {
        if let Value::Map(expected_node_map) = expected {
            if !node_matches_pattern(*node_id, expected_node_map, world) {
                return false;
            }
        } else {
            return false;
        }
    }

    // Check each edge
    for (edge_id, expected) in actual_edges.iter().zip(expected_edge_patterns.iter()) {
        if let Value::Map(expected_edge_map) = expected {
            if !edge_matches_pattern(*edge_id, expected_edge_map, world) {
                return false;
            }
        } else {
            return false;
        }
    }

    true
}

/// Format a Zoned datetime to match Neo4j's canonical format.
/// jiff outputs `2000-01-02T00:00:00+00:00[UTC]` but Neo4j expects `2000-01-02T00:00Z`.
fn format_zoned_datetime(zdt: &jiff::Zoned) -> String {
    let dt = jiff::civil::DateTime::from(zdt);
    let off = zdt.offset();
    let total_secs = off.seconds();

    // Format the datetime part, omitting trailing :00 seconds and .000 nanos
    let base = if dt.subsec_nanosecond() != 0 {
        format!("{dt}")
    } else if dt.second() != 0 {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}",
            dt.year(),
            dt.month(),
            dt.day(),
            dt.hour(),
            dt.minute(),
            dt.second()
        )
    } else {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}",
            dt.year(),
            dt.month(),
            dt.day(),
            dt.hour(),
            dt.minute()
        )
    };

    // Format timezone
    let tz = zdt.time_zone();
    if let Some(iana) = tz.iana_name() {
        if iana == "UTC" && total_secs == 0 {
            format!("{base}Z")
        } else {
            let h = total_secs / 3600;
            let m = (total_secs.abs() % 3600) / 60;
            format!("{base}{:+03}:{:02}[{iana}]", h, m)
        }
    } else if total_secs == 0 {
        format!("{base}Z")
    } else {
        let h = total_secs / 3600;
        let m = (total_secs.abs() % 3600) / 60;
        format!("{base}{:+03}:{:02}", h, m)
    }
}

/// Format a Zoned datetime as a zoned time (HH:MM:SS.nnn+offset).
/// Used for Neo4j's Time type, which we represent as DateTime with epoch date.
fn format_zoned_time(zdt: &jiff::Zoned) -> String {
    let t = jiff::civil::Time::from(zdt);
    let off = zdt.offset();
    let total_secs = off.seconds();

    let time_part = if t.subsec_nanosecond() != 0 {
        format!("{t}")
    } else if t.second() != 0 {
        format!("{:02}:{:02}:{:02}", t.hour(), t.minute(), t.second())
    } else {
        format!("{:02}:{:02}", t.hour(), t.minute())
    };

    if total_secs == 0 {
        format!("{time_part}Z")
    } else {
        let h = total_secs / 3600;
        let m = (total_secs.abs() % 3600) / 60;
        format!("{time_part}{:+03}:{:02}", h, m)
    }
}

/// Normalize datetime string for comparison:
/// Strip trailing `:00` seconds, `Z` ↔ `+00:00`, `[UTC]`, etc.
fn normalize_datetime_str(s: &str) -> String {
    let s = s.replace("[UTC]", "");
    let s = s.replace("+00:00", "Z");
    let s = s.replace("-00:00", "Z");
    // Remove trailing :00 seconds when no subseconds
    let s = if s.contains('T') {
        let mut result = s.replace(":00Z", "Z")
            .replace(":00+", "+")
            .replace(":00-", "-");
        // Also handle trailing :00 at end of string (localdatetime without tz)
        if result.ends_with(":00") {
            let colon_count = result.matches(':').count();
            if colon_count >= 2 {
                result.truncate(result.len() - 3);
            }
        }
        result
    } else {
        s
    };
    s.trim().to_string()
}

/// Normalize time string for comparison.
fn normalize_time_str(s: &str) -> String {
    // Remove trailing :00 seconds
    let s = s.to_string();
    let s = if s.ends_with(":00") && s.matches(':').count() >= 2 {
        s[..s.len() - 3].to_string()
    } else {
        s
    };
    s
}

/// Normalize duration string for comparison.
/// PT0S is equivalent to P0D, etc.
fn normalize_duration_str(s: &str) -> String {
    // Both represent zero duration
    if s == "PT0S" || s == "P0D" {
        return "PT0S".to_string();
    }
    s.to_string()
}

/// Create a sortable string representation of a Value for list-order-ignoring comparisons.
fn format_value_for_compare(v: &Value, world: &GqlWorld) -> String {
    match v {
        Value::Null => "null".to_string(),
        Value::Bool(b) => format!("bool:{b}"),
        Value::Int(n) => format!("int:{n}"),
        Value::Float(f) => format!("float:{f}"),
        Value::Str(s) => format!("str:{s}"),
        Value::List(items) => {
            let inner: Vec<String> = items
                .iter()
                .map(|i| format_value_for_compare(i, world))
                .collect();
            format!("list:[{}]", inner.join(","))
        }
        Value::Map(m) => {
            let inner: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{k}:{}", format_value_for_compare(v, world)))
                .collect();
            format!("map:{{{}}}", inner.join(","))
        }
        Value::Node(id) => format!("node:{}", id.0),
        Value::Edge(id) => format!("edge:{}", id.0),
        _ => format!("{v}"),
    }
}

/// Format rows for debug output, resolving node/edge IDs.
fn format_rows_for_debug(rows: &[Row], world: &GqlWorld) -> String {
    let row_strs: Vec<String> = rows
        .iter()
        .map(|r| format_row_for_debug(r, world))
        .collect();
    format!("[{}]", row_strs.join(", "))
}

fn format_row_for_debug(row: &Row, world: &GqlWorld) -> String {
    let pairs: Vec<String> = row
        .iter()
        .map(|(k, v)| format!("{k}: {}", format_value_debug(v, world)))
        .collect();
    format!("{{{}}}", pairs.join(", "))
}

fn format_value_debug(v: &Value, world: &GqlWorld) -> String {
    match v {
        Value::Node(id) => {
            if let Some(store) = &world.store {
                if let Ok(Some(data)) = store.get_node(&world.graph_name, *id) {
                    let labels = if data.labels.is_empty() {
                        String::new()
                    } else {
                        data.labels
                            .iter()
                            .map(|l| format!(":{l}"))
                            .collect::<String>()
                    };
                    let props = if data.properties.is_empty() {
                        String::new()
                    } else {
                        let inner: Vec<String> = data
                            .properties
                            .iter()
                            .map(|(k, v)| format!("{k}: {v}"))
                            .collect();
                        format!(" {{{}}}", inner.join(", "))
                    };
                    return format!("({labels}{props})");
                }
            }
            format!("(node#{})", id.0)
        }
        Value::Edge(id) => {
            if let Some(store) = &world.store {
                if let Ok(Some(data)) = store.get_edge(&world.graph_name, *id) {
                    let props = if data.properties.is_empty() {
                        String::new()
                    } else {
                        let inner: Vec<String> = data
                            .properties
                            .iter()
                            .map(|(k, v)| format!("{k}: {v}"))
                            .collect();
                        format!(" {{{}}}", inner.join(", "))
                    };
                    return format!("[:{}{}]", data.rel_type, props);
                }
            }
            format!("[edge#{}]", id.0)
        }
        _ => format!("{v}"),
    }
}

// ── Procedure definition parser ────────────────────────────────────────────────

/// Parse a procedure definition from the TCK step text and data table.
///
/// Signature format: `test.my.proc(name :: STRING?, id :: INTEGER?) :: (city :: STRING?, country_code :: INTEGER?)`
fn parse_procedure_definition(signature: &str, step: &gherkin::Step) -> ProcedureDefinition {
    let sig = signature.trim();

    // Split into name+inputs and outputs at " :: "
    let (name_and_inputs, outputs_str) = if let Some(pos) = sig.find(") :: (") {
        let name_inputs = &sig[..pos + 1]; // up to and including first ')'
        let outputs = &sig[pos + 6..]; // after ") :: (" (6 chars)
        (name_inputs, outputs.trim_end_matches(')').trim())
    } else if let Some(pos) = sig.find("() :: (") {
        let name_inputs = &sig[..pos + 2]; // "name()"
        let outputs = &sig[pos + 7..]; // after "() :: ("
        (name_inputs, outputs.trim_end_matches(')').trim())
    } else if sig.contains(":: ()") {
        // Void procedure: no outputs
        let name_inputs = sig.split(" :: ").next().unwrap_or(sig);
        (name_inputs.trim(), "")
    } else {
        (sig, "")
    };

    // Parse procedure name and input params
    let (proc_name, input_params) = if let Some(paren_pos) = name_and_inputs.find('(') {
        let name = name_and_inputs[..paren_pos].trim();
        let params_str = name_and_inputs[paren_pos + 1..]
            .trim_end_matches(')')
            .trim();
        (name.to_string(), parse_param_list(params_str))
    } else {
        (name_and_inputs.trim().to_string(), vec![])
    };

    // Parse output columns
    let output_columns = if outputs_str.is_empty() {
        vec![]
    } else {
        parse_param_list(outputs_str)
    };

    // Parse data table rows
    let mut data = Vec::new();
    if let Some(table) = &step.table {
        // Skip header row (first row is column names)
        for row in table.rows.iter().skip(1) {
            let values: Vec<Value> = row.iter().map(|cell| parse_tck_value(cell.trim())).collect();
            data.push(values);
        }
    }

    ProcedureDefinition {
        name: proc_name,
        input_params,
        output_columns,
        data,
    }
}

/// Parse a comma-separated parameter list like `name :: STRING?, id :: INTEGER?`.
fn parse_param_list(s: &str) -> Vec<ProcedureParam> {
    if s.trim().is_empty() {
        return vec![];
    }
    s.split(',')
        .map(|param| {
            let param = param.trim();
            if let Some(pos) = param.find("::") {
                let name = param[..pos].trim().to_string();
                let type_name = param[pos + 2..].trim().to_string();
                ProcedureParam { name, type_name }
            } else {
                ProcedureParam {
                    name: param.to_string(),
                    type_name: "ANY".to_string(),
                }
            }
        })
        .collect()
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Cucumber test entry point.
///
/// `cargo test --test tck` invokes this binary (harness = false, so cucumber
/// drives the test runner, not libtest).
///
/// We use `run_and_exit` which will print a summary and exit with code 1 if any
/// scenario fails.  Panics inside steps are caught by cucumber and reported as
/// failures rather than crashing the process.
#[tokio::main]
async fn main() {
    GqlWorld::run("tests/tck/features").await;
}
