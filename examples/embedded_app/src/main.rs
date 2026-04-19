//! `embedded-app` — a CLI that demonstrates embedding all three
//! SedanDelivery query engines (openCypher, GQL, Apache TinkerPop Gremlin)
//! into a single Rust application sharing one in-memory graph store.
//!
//! Run `cargo run -p embedded-app -- demo` for a guided tour.

use clap::{Parser, Subcommand};

mod compare;
mod cypher;
mod gql;
mod gremlin;
mod seed;

#[derive(Parser)]
#[command(
    name = "embedded-app",
    about = "Demo: embed cypher / GQL / gremlin engines as a Rust library"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Build the sample graph and dump basic stats.
    Seed,
    /// Run a Cypher query against the sample graph.
    QueryCypher { query: String },
    /// Run a Gremlin traversal against the sample graph.
    QueryGremlin { query: String },
    /// Run a GQL query against the sample graph.
    QueryGql { query: String },
    /// Run equivalent queries through all three engines and assert parity.
    Compare,
    /// Guided tour: seed + run equivalent queries through all 3 engines.
    Demo,
}

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Seed => cmd_seed(),
        Cmd::QueryCypher { query } => cmd_query_cypher(&query),
        Cmd::QueryGremlin { query } => cmd_query_gremlin(&query),
        Cmd::QueryGql { query } => cmd_query_gql(&query),
        Cmd::Compare => cmd_compare(),
        Cmd::Demo => cmd_demo(),
    }
}

fn cmd_seed() {
    let store = seed::build();
    let nodes = cypher::run(&store, "MATCH (n) RETURN count(n) AS n");
    let edges = cypher::run(&store, "MATCH ()-[r]->() RETURN count(r) AS n");
    println!("seeded graph '{}'", seed::GRAPH);
    println!("  nodes: {:?}", nodes.rows[0].get("n").unwrap());
    println!("  edges: {:?}", edges.rows[0].get("n").unwrap());
}

fn cmd_query_cypher(q: &str) {
    let store = seed::build();
    print_result(&cypher::run(&store, q));
}

fn cmd_query_gremlin(q: &str) {
    let store = seed::build();
    print_result(&gremlin::run(&store, q));
}

fn cmd_query_gql(q: &str) {
    let store = seed::build();
    print_result(&gql::run(&store, q));
}

fn cmd_compare() {
    let store = seed::build();
    compare::run_all(&store);
    println!("All parity checks passed ✓");
}

fn cmd_demo() {
    println!("=== embedded-app demo ===\n");

    println!("[1] Building sample social-network graph...");
    let store = seed::build();
    let stats = cypher::run(&store, "MATCH (n) RETURN count(n) AS nodes");
    println!(
        "    {} nodes\n",
        fmt_value(stats.rows[0].get("nodes").unwrap())
    );

    println!("[2] Cypher: friends of Alice");
    println!("    query:   {}", cypher::Q_ALICE_FRIENDS);
    let c = cypher::run(&store, cypher::Q_ALICE_FRIENDS);
    println!("    result:  {:?}\n", compare::first_column_sorted(&c));

    println!("[3] Gremlin: friends of Alice");
    println!("    query:   {}", gremlin::Q_ALICE_FRIENDS);
    let g = gremlin::run(&store, gremlin::Q_ALICE_FRIENDS);
    println!("    result:  {:?}\n", compare::first_column_sorted(&g));

    println!("[4] GQL: friends of Alice");
    println!("    query:   {}", gql::Q_ALICE_FRIENDS);
    let q = gql::run(&store, gql::Q_ALICE_FRIENDS);
    println!("    result:  {:?}\n", compare::first_column_sorted(&q));

    println!("[5] Cross-engine parity assertions:");
    compare::run_all(&store);

    println!("=== demo complete ✓ ===");
}

fn print_result(result: &sedan_cypher_embedded::QueryResult) {
    println!("columns: {:?}", result.columns);
    println!("{} row(s):", result.rows.len());
    for row in &result.rows {
        let formatted: Vec<String> = result
            .columns
            .iter()
            .map(|c| {
                format!(
                    "{}={}",
                    c,
                    fmt_value(row.get(c).unwrap_or(&sedan_cypher_embedded::Value::Null))
                )
            })
            .collect();
        println!("  {}", formatted.join(", "));
    }
}

fn fmt_value(v: &sedan_cypher_embedded::Value) -> String {
    use sedan_cypher_embedded::Value;
    match v {
        Value::Str(s) => format!("\"{s}\""),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parity_holds() {
        let store = seed::build();
        compare::run_all(&store);
    }

    #[test]
    fn alice_has_three_friends() {
        let store = seed::build();
        let r = cypher::run(&store, cypher::Q_ALICE_FRIENDS);
        assert_eq!(r.rows.len(), 3);
    }
}
