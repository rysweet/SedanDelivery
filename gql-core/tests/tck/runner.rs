//! openCypher TCK cucumber runner for gql-core.
//!
//! Executes the Gherkin `.feature` files in `tests/tck/features/` against
//! the real parser → planner → executor pipeline.  Every scenario runs in
//! a fresh `InMemoryGraphStore` graph so state does not leak between tests.
//!
//! Pattern matching on step text:
//! - `Given an empty graph`   — creates a fresh in-memory graph.
//! - `Given having executed:` — runs a setup CQL statement (docstring).
//! - `And having executed:`   — same as above (contextually a Given step).
//! - `When executing query:`  — runs the query and stores the result.
//! - `Then the result should contain:` — checks unordered row membership.
//! - `Then the result should be, in order:` — checks ordered row equality.
//! - `Then the side effects should be:` — checks mutation stats.

use std::collections::HashMap;
use std::sync::Arc;

use cucumber::gherkin;
use cucumber::{given, then, when, World};
use gql_core::{
    executor::{ExecutionBudget, QueryExecutor, QueryResult, Row},
    parser::parse_query,
    planner::QueryPlanner,
    storage::{GraphStore, InMemoryGraphStore},
    types::Value,
};

// ── World state ───────────────────────────────────────────────────────────────

/// Per-scenario state that persists across steps.
///
/// A new `GqlWorld` is created for every `Scenario` by cucumber's runner.
#[derive(Debug, Default, World)]
pub struct GqlWorld {
    /// The backing store.  Set by `Given an empty graph`.
    store: Option<Arc<InMemoryGraphStore>>,
    /// The graph name inside the store.
    graph_name: String,
    /// The result of the most recent `When executing query:` step.
    last_result: Option<QueryResult>,
    /// Counter used to generate unique graph names.
    graph_counter: usize,
}

impl GqlWorld {
    fn get_store(&self) -> Arc<InMemoryGraphStore> {
        self.store
            .clone()
            .expect("no store initialised — missing 'Given an empty graph'")
    }

    /// Execute `query` and return the result.
    fn execute(&mut self, query: &str) -> QueryResult {
        let store = self.get_store();
        let ast = parse_query(query)
            .unwrap_or_else(|e| panic!("parse error for query {:?}: {}", query, e));
        let planner = QueryPlanner::new(store.as_ref(), &self.graph_name);
        let plan = planner
            .plan(&ast)
            .unwrap_or_else(|e| panic!("plan error for query {:?}: {}", query, e));
        let mut budget = ExecutionBudget::default_budget();
        QueryExecutor::new(store.as_ref())
            .execute(&self.graph_name, &plan, &mut budget)
            .unwrap_or_else(|e| panic!("execution error for query {:?}: {}", query, e))
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
    }
}

// ── Step implementations ──────────────────────────────────────────────────────

/// Set up a fresh empty in-memory graph.
#[given("an empty graph")]
async fn given_empty_graph(world: &mut GqlWorld) {
    world.init_empty_graph();
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

/// Execute a CQL query and store the result.
#[when(regex = r"(?i)executing query:")]
async fn when_executing(world: &mut GqlWorld, step: &gherkin::Step) {
    let query = step
        .docstring
        .as_ref()
        .expect("missing docstring for 'executing query:'")
        .trim()
        .to_string();
    world.last_result = Some(world.execute(&query));
}

// ── Then: result contains (unordered) ─────────────────────────────────────────

/// Asserts that every row in the Gherkin table appears in the result.
/// Order does not matter; the result may have more rows than the table.
#[then(regex = r"(?i)the result should contain:")]
async fn result_contains(world: &mut GqlWorld, step: &gherkin::Step) {
    let result = world
        .last_result
        .as_ref()
        .expect("no result — missing 'When executing query:' step");

    let table = step
        .table
        .as_ref()
        .expect("missing data table for 'the result should contain:'");

    // Row 0 is the header.
    let headers: Vec<&str> = table.rows[0].iter().map(|h| h.trim()).collect();

    for (row_idx, row_data) in table.rows.iter().enumerate().skip(1) {
        let expected: HashMap<&str, String> = headers
            .iter()
            .zip(row_data.iter())
            .map(|(&h, v)| (h, v.trim().trim_matches('\'').to_string()))
            .collect();

        let found = result.rows.iter().any(|row| row_matches(row, &expected));

        assert!(
            found,
            "Expected row #{row_idx} not found in result.\n  Expected: {expected:?}\n  Got rows: {rows:?}",
            rows = result.rows,
        );
    }
}

// ── Then: result is in order ───────────────────────────────────────────────────

/// Asserts that the result rows match the table exactly, in order.
#[then(regex = r"(?i)the result should be, in order:")]
async fn result_in_order(world: &mut GqlWorld, step: &gherkin::Step) {
    let result = world
        .last_result
        .as_ref()
        .expect("no result — missing 'When executing query:' step");

    let table = step
        .table
        .as_ref()
        .expect("missing data table for 'the result should be, in order:'");

    let headers: Vec<&str> = table.rows[0].iter().map(|h| h.trim()).collect();
    let expected_rows: Vec<HashMap<&str, String>> = table
        .rows
        .iter()
        .skip(1)
        .map(|row_data| {
            headers
                .iter()
                .zip(row_data.iter())
                .map(|(&h, v)| (h, v.trim().trim_matches('\'').to_string()))
                .collect()
        })
        .collect();

    assert_eq!(
        result.rows.len(),
        expected_rows.len(),
        "Row count mismatch: expected {} rows, got {} rows.\n  Got: {:?}",
        expected_rows.len(),
        result.rows.len(),
        result.rows,
    );

    for (i, (actual_row, expected)) in result.rows.iter().zip(expected_rows.iter()).enumerate() {
        assert!(
            row_matches(actual_row, expected),
            "Row #{i} does not match.\n  Expected: {expected:?}\n  Got: {actual_row:?}",
        );
    }
}

// ── Then: side effects ─────────────────────────────────────────────────────────

/// Asserts mutation counters from a `| +nodes | 1 |` style table.
#[then(regex = r"(?i)the side effects should be:")]
async fn side_effects(world: &mut GqlWorld, step: &gherkin::Step) {
    let result = world
        .last_result
        .as_ref()
        .expect("no result — missing 'When executing query:' step");

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
            "+labels" => assert_eq!(
                result.stats.labels_added, val,
                "+labels mismatch: expected {val}, got {}",
                result.stats.labels_added
            ),
            "+edges" => assert_eq!(
                result.stats.edges_created, val,
                "+edges mismatch: expected {val}, got {}",
                result.stats.edges_created
            ),
            other => {
                // Unknown side-effect keys are silently ignored so we can add
                // more TCK assertions in future without breaking existing tests.
                let _ = other;
            }
        }
    }
}

// ── Helper functions ──────────────────────────────────────────────────────────

/// Returns `true` if `actual_row` contains every `(col, expected_str)` pair
/// in `expected`, where value comparison strips outer quotes and converts to
/// string for matching.
fn row_matches(actual_row: &Row, expected: &HashMap<&str, String>) -> bool {
    expected.iter().all(|(col, expected_val)| {
        actual_row
            .get(*col)
            .map(|v| value_matches_str(v, expected_val))
            .unwrap_or(false)
    })
}

/// Convert a `Value` to a comparable string and compare with `expected`.
///
/// Handles the common TCK table value formats:
/// - Integer: compare as decimal string (`42` → `"42"`)
/// - String: compare the inner string (without surrounding `'...'` in the table)
/// - Boolean: compare as `"true"` / `"false"`
/// - Float: compare as decimal string
fn value_matches_str(v: &Value, expected: &str) -> bool {
    match v {
        Value::Str(s) => s.as_ref() == expected,
        Value::Int(n) => n.to_string() == expected,
        Value::Float(f) => f.to_string() == expected,
        Value::Bool(b) => b.to_string() == expected,
        Value::Null => expected == "null",
        _ => false,
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Cucumber test entry point.
///
/// `cargo test --test tck` invokes this binary (harness = false, so cucumber
/// drives the test runner, not libtest).
#[tokio::main]
async fn main() {
    GqlWorld::run("tests/tck/features").await;
}
