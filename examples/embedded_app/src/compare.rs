//! Cross-engine result-parity checks.

use sedan_cypher_embedded::{GraphStore, QueryResult, Value};

use crate::{cypher, gql, gremlin};

/// Extract the first column of a [`QueryResult`] as a sorted `Vec<String>`.
pub fn first_column_sorted(result: &QueryResult) -> Vec<String> {
    let mut out: Vec<String> = result
        .rows
        .iter()
        .map(|row| {
            let value = result
                .columns
                .first()
                .and_then(|col| row.get(col))
                .or_else(|| row.values().next())
                .cloned()
                .unwrap_or(Value::Null);
            value_to_display(&value)
        })
        .collect();
    out.sort();
    out
}

fn value_to_display(v: &Value) -> String {
    match v {
        Value::Str(s) => s.to_string(),
        other => format!("{other:?}"),
    }
}

/// Run all parity scenarios. Panics on mismatch.
pub fn run_all(store: &dyn GraphStore) {
    parity(
        store,
        "friends of Alice",
        cypher::Q_ALICE_FRIENDS,
        gql::Q_ALICE_FRIENDS,
        gremlin::Q_ALICE_FRIENDS,
    );
    parity(
        store,
        "people over 30",
        cypher::Q_OVER_30,
        gql::Q_OVER_30,
        gremlin::Q_OVER_30,
    );
}

fn parity(store: &dyn GraphStore, label: &str, cypher_q: &str, gql_q: &str, gremlin_q: &str) {
    let c = first_column_sorted(&cypher::run(store, cypher_q));
    let g = first_column_sorted(&gql::run(store, gql_q));
    let r = first_column_sorted(&gremlin::run(store, gremlin_q));

    println!("[{label}]");
    println!("  cypher : {c:?}");
    println!("  gql    : {g:?}");
    println!("  gremlin: {r:?}");

    assert_eq!(c, g, "[{label}] cypher vs gql mismatch");
    assert_eq!(c, r, "[{label}] cypher vs gremlin mismatch");
    println!("  ✓ parity OK ({} rows)\n", c.len());
}
