//! Cross-engine query runner with canonical result normalization.

use sedan_cypher_embedded::{CypherEngine, GqlError, GraphStore, QueryResult, Row, Value};
use sedan_gql_embedded::SedanGql;
use sedan_gremlin_embedded::GremlinEngine;

use crate::cases::{DivergenceMode, ParityCase};
use crate::fixture::GRAPH;

/// Outcome of running a [`ParityCase`] across all three engines.
pub struct CaseOutcome {
    pub name: &'static str,
    pub cypher: Vec<Vec<Value>>,
    pub gql: Vec<Vec<Value>>,
    pub gremlin: Vec<Vec<Value>>,
    pub mode: DivergenceMode,
}

impl CaseOutcome {
    /// True if all three engines agree on the canonical result set.
    pub fn agrees(&self) -> bool {
        self.cypher == self.gql && self.cypher == self.gremlin
    }

    /// Human-readable diagnostic for failing cases.
    pub fn diagnostic(&self) -> String {
        let mut s = format!("[{}] cross-engine divergence:\n", self.name);
        s.push_str(&format!(
            "  cypher  ({} rows): {:?}\n",
            self.cypher.len(),
            self.cypher
        ));
        s.push_str(&format!(
            "  gql     ({} rows): {:?}\n",
            self.gql.len(),
            self.gql
        ));
        s.push_str(&format!(
            "  gremlin ({} rows): {:?}\n",
            self.gremlin.len(),
            self.gremlin
        ));

        if self.cypher != self.gql {
            s.push_str("  -> cypher vs gql DIFFER\n");
        }
        if self.cypher != self.gremlin {
            s.push_str("  -> cypher vs gremlin DIFFER\n");
        }
        if self.gql != self.gremlin {
            s.push_str("  -> gql vs gremlin DIFFER\n");
        }
        s
    }
}

/// Run a parity case through all three engines and return the canonicalized
/// result sets.
///
/// Errors from any single engine are converted to a one-row sentinel so that
/// the caller can still see WHICH engine diverged on a parse/exec failure
/// instead of just "test panicked".
pub fn run_case(store: &dyn GraphStore, case: &ParityCase) -> CaseOutcome {
    let cypher = run_cypher(store, case.cypher);
    let gql = run_gql(store, case.gql);
    let gremlin = run_gremlin(store, case.gremlin);

    CaseOutcome {
        name: case.name,
        cypher: canonicalize(cypher),
        gql: canonicalize(gql),
        gremlin: canonicalize(gremlin),
        mode: case.mode,
    }
}

fn run_cypher(store: &dyn GraphStore, q: &str) -> Result<QueryResult, String> {
    CypherEngine::new()
        .execute(store, GRAPH, q)
        .map_err(|e: GqlError| e.to_string())
}

fn run_gql(store: &dyn GraphStore, q: &str) -> Result<QueryResult, String> {
    SedanGql::new()
        .execute(store, GRAPH, q)
        .map_err(|e| e.to_string())
}

fn run_gremlin(store: &dyn GraphStore, q: &str) -> Result<QueryResult, String> {
    GremlinEngine::new()
        .execute(store, GRAPH, q)
        .map_err(|e| e.to_string())
}

/// Convert an engine's [`QueryResult`] into a canonical, order-independent
/// form: a sorted `Vec<Vec<Value>>` indexed by column position.
///
/// We do NOT rely on column names matching across engines (Gremlin in
/// particular doesn't always emit named columns), only on positional value
/// ordering. Each parity case is responsible for ensuring the engines emit
/// columns in the same logical order.
fn canonicalize(result: Result<QueryResult, String>) -> Vec<Vec<Value>> {
    match result {
        Ok(r) => {
            let mut rows: Vec<Vec<Value>> = r
                .rows
                .iter()
                .map(|row| row_values(&r.columns, row))
                .collect();
            rows.sort_by(|a, b| compare_row(a, b));
            rows
        }
        Err(e) => vec![vec![Value::Str(format!("ERROR: {e}").into())]],
    }
}

fn row_values(columns: &[String], row: &Row) -> Vec<Value> {
    if columns.is_empty() {
        // Some engines emit unnamed result rows for traversals (Gremlin
        // values()). Fall back to row.values() in insertion order.
        row.values().cloned().collect()
    } else {
        columns
            .iter()
            .map(|c| row.get(c).cloned().unwrap_or(Value::Null))
            .collect()
    }
}

fn compare_row(a: &[Value], b: &[Value]) -> std::cmp::Ordering {
    let ka: Vec<String> = a.iter().map(value_key).collect();
    let kb: Vec<String> = b.iter().map(value_key).collect();
    ka.cmp(&kb)
}

/// Stable, total-order key for a [`Value`] suitable for canonical sorting.
///
/// We avoid relying on `PartialOrd` for `Value` because some variants
/// (Float, Map, ...) don't have well-defined orderings. The string
/// representation here is for sorting only — equality checks still use
/// `PartialEq` on `Value` itself.
fn value_key(v: &Value) -> String {
    match v {
        Value::Null => "0:null".to_string(),
        Value::Bool(b) => format!("1:{b}"),
        Value::Int(i) => format!("2:{i:020}"),
        Value::Float(f) => format!("3:{f:.10}"),
        Value::Str(s) => format!("4:{s}"),
        Value::List(xs) => {
            let inner: Vec<String> = xs.iter().map(value_key).collect();
            format!("5:[{}]", inner.join(","))
        }
        other => format!("9:{other:?}"),
    }
}
