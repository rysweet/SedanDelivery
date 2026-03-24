/// gql-cli: Interactive demonstration of the gql-core engine.
///
/// Exercises the full Phase 1 pipeline against InMemoryGraphStore:
///   Storage API → Lexer → Parser → Planner → Executor
///
/// Run with: cargo run -p gql-cli
use gql_core::{
    executor::{ExecutionBudget, QueryExecutor},
    parser::{lex_query, parse_query, tokenize},
    planner::QueryPlanner,
    storage::{validate_graph_name, GraphStore, InMemoryGraphStore},
    types::Value,
};
use indexmap::IndexMap;

fn run_query(
    store: &InMemoryGraphStore,
    graph: &str,
    cypher: &str,
) -> Result<gql_core::executor::QueryResult, gql_core::error::GqlError> {
    let ast = parse_query(cypher)?;
    let planner = QueryPlanner::new(store, graph);
    let plan = planner.plan(&ast)?;
    let mut budget = ExecutionBudget::default_budget();
    QueryExecutor::new(store).execute(graph, &plan, &mut budget)
}

fn main() {
    println!("=== SedanDelivery GQL Engine — Interactive Demo ===\n");

    // ── 1. Demonstrate the storage layer ─────────────────────────────────────

    println!("[ Storage Layer Demo ]");

    let store = InMemoryGraphStore::default();
    let graph = "demo";

    // Validate graph name
    match validate_graph_name(graph) {
        Ok(()) => println!("  ✓ Graph name '{graph}' is valid"),
        Err(e) => {
            eprintln!("  ✗ Invalid graph name: {e}");
            std::process::exit(1);
        }
    }

    store.create_graph(graph).expect("create_graph failed");
    println!("  ✓ Created graph '{graph}'");

    // Create nodes
    let alice_id = store
        .create_node(
            graph,
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str("Alice".into())),
                ("age".into(), Value::Int(30)),
            ]),
        )
        .expect("create Alice failed");
    println!("  ✓ Created node Alice (id={})", alice_id.0);

    let bob_id = store
        .create_node(
            graph,
            vec!["Person".into()],
            IndexMap::from([
                ("name".into(), Value::Str("Bob".into())),
                ("age".into(), Value::Int(25)),
            ]),
        )
        .expect("create Bob failed");
    println!("  ✓ Created node Bob (id={})", bob_id.0);

    let corp_id = store
        .create_node(
            graph,
            vec!["Organization".into()],
            IndexMap::from([("name".into(), Value::Str("Acme Corp".into()))]),
        )
        .expect("create Corp failed");
    println!("  ✓ Created node Acme Corp (id={})", corp_id.0);

    // Create edges
    let knows_id = store
        .create_edge(
            graph,
            alice_id,
            bob_id,
            "KNOWS".into(),
            IndexMap::from([("since".into(), Value::Int(2020))]),
        )
        .expect("create KNOWS failed");
    println!("  ✓ Created edge Alice-[:KNOWS]->Bob (id={})", knows_id.0);

    let works_at_id = store
        .create_edge(
            graph,
            alice_id,
            corp_id,
            "WORKS_AT".into(),
            IndexMap::from([("role".into(), Value::Str("Engineer".into()))]),
        )
        .expect("create WORKS_AT failed");
    println!(
        "  ✓ Created edge Alice-[:WORKS_AT]->AcmeCorp (id={})",
        works_at_id.0
    );

    // Query the graph
    let people = store
        .find_nodes_by_label(graph, "Person")
        .expect("label scan failed");
    println!("\n  Label scan Person: {} nodes found", people.len());
    for id in &people {
        let node = store
            .get_node(graph, *id)
            .expect("get_node failed")
            .expect("node not found");
        println!("    - id={}: labels={:?}", id.0, node.labels);
        for (k, v) in &node.properties {
            println!("      {k}: {v}");
        }
    }

    // Adjacency traversal
    let alice_out = store.out_edges(graph, alice_id).expect("out_edges failed");
    println!("\n  Alice's outgoing edges ({} total):", alice_out.len());
    for adj in &alice_out {
        println!("    -[:{}]->(id={})", adj.rel_type, adj.neighbor_id.0);
    }

    // Batch node fetch
    let batch = store
        .get_nodes_batch(graph, &[alice_id, bob_id, corp_id])
        .expect("batch fetch failed");
    println!(
        "\n  Batch fetch: {} nodes retrieved",
        batch.iter().filter(|n| n.is_some()).count()
    );

    // Graph stats
    let stats = store.graph_stats(graph).expect("graph_stats failed");
    println!("\n  Graph stats:");
    println!("    nodes: {}", stats.node_count);
    println!("    edges: {}", stats.edge_count);
    println!("    labels: {}", stats.label_count);
    println!("    rel types: {}", stats.rel_type_count);

    // ── 2. Demonstrate the lexer ──────────────────────────────────────────────

    println!("\n[ Lexer Demo ]");

    let queries = [
        "MATCH (n:Person) RETURN n.name",
        "MATCH (a:Person {name: 'Alice'})-[:KNOWS]->(b) RETURN b.name",
        "MATCH (n:Person)-[r:KNOWS|WORKS_AT*1..3]->(m) WHERE n.age > 25 RETURN n, r, m",
        "CREATE (n:Person {name: $name, age: $age})",
        "MATCH (n) WHERE NOT (n:Person OR n:Organization) RETURN count(n)",
    ];

    for query in &queries {
        println!("\n  Query: {query}");
        match lex_query(query) {
            Ok(tokens) => {
                let kinds: Vec<String> = tokens.iter().map(|t| format!("{:?}", t.kind)).collect();
                // Show first 8 tokens
                let preview: Vec<&String> = kinds.iter().take(8).collect();
                println!(
                    "  Tokens ({} total): {}{}",
                    tokens.len(),
                    preview
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    if tokens.len() > 8 { ", ..." } else { "" }
                );
            }
            Err(e) => println!("  Lex error: {e}"),
        }
    }

    // Demonstrate lenient tokenizer handles errors gracefully
    let malformed = "MATCH (n:Person RETURN n.name"; // missing )
    println!("\n  Malformed query (missing closing paren): {malformed}");
    let tokens = tokenize(malformed);
    println!("  Tokenized {} tokens (lenient mode)", tokens.len());

    // ── 3. Validate graph name security ──────────────────────────────────────

    println!("\n[ Security: Graph Name Validation ]");
    let long_name = "a".repeat(65);
    let test_names = [
        ("valid-graph", true),
        ("my_graph.v2", true),
        ("", false),
        ("graph:attack", false),
        ("{cluster}slot", false),
        (long_name.as_str(), false),
    ];
    for (name, should_pass) in test_names {
        let result = validate_graph_name(name);
        let status = if result.is_ok() == should_pass {
            "✓"
        } else {
            "✗ UNEXPECTED"
        };
        println!(
            "  {status} '{name}': {}",
            if result.is_ok() {
                "accepted"
            } else {
                "rejected"
            }
        );
    }

    // ── 4. Phase 1: Parser → Planner → Executor pipeline ─────────────────────

    println!("\n[ Phase 1: Full Query Pipeline ]");

    // Re-use the demo graph for query execution
    // Use the same "demo" graph we built above
    let queries_to_run: &[(&str, &str)] = &[
        ("demo", "MATCH (p:Person) RETURN p.name"),
        ("demo", "MATCH (p:Person) WHERE p.age > 25 RETURN p.name"),
        (
            "demo",
            "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name",
        ),
    ];

    for (graph_name, cypher) in queries_to_run {
        println!("\n  Query: {cypher}");
        match run_query(&store, graph_name, cypher) {
            Ok(result) => {
                println!("  Columns: {:?}", result.columns);
                println!("  Rows ({}):", result.rows.len());
                for row in &result.rows {
                    let vals: Vec<String> = result
                        .columns
                        .iter()
                        .map(|c| {
                            format!(
                                "{} = {}",
                                c,
                                row.get(c)
                                    .map(|v| v.to_string())
                                    .unwrap_or_else(|| "null".into())
                            )
                        })
                        .collect();
                    println!("    {{ {} }}", vals.join(", "));
                }
            }
            Err(e) => println!("  Error: {e}"),
        }
    }

    // Parse a complex query and show the AST structure
    let complex = "MATCH (n:Person)-[:KNOWS*1..3]->(m:Person) WHERE n.age > 20 RETURN n.name, count(m) ORDER BY n.name LIMIT 10";
    println!("\n  Parse complex query:");
    println!("  {complex}");
    match parse_query(complex) {
        Ok(ast) => println!("  ✓ Parsed successfully: {} clause(s)", ast.clauses.len()),
        Err(e) => println!("  ✗ Parse error: {e}"),
    }

    println!("\n=== Demo complete ===");
    println!("Test suite: 237 tests (227 unit + 10 end-to-end integration)");
    println!("See docs/TUTORIAL.md for usage guide.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_store() -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").expect("create graph");
        store
    }

    #[test]
    fn create_and_match_round_trip() {
        let store = test_store();
        // CREATE a node, then MATCH it back
        let create_result = run_query(
            &store,
            "test",
            "CREATE (n:Person {name: 'Alice'}) RETURN n.name",
        )
        .expect("CREATE+RETURN should succeed");
        assert_eq!(create_result.columns, vec!["n.name"]);
        assert_eq!(create_result.rows.len(), 1);

        let match_result = run_query(&store, "test", "MATCH (n:Person) RETURN n.name")
            .expect("MATCH should succeed");
        assert_eq!(match_result.rows.len(), 1);
        let name = match_result.rows[0].get("n.name").expect("column exists");
        assert_eq!(*name, Value::Str("Alice".into()));
    }

    #[test]
    fn invalid_query_returns_error() {
        let store = test_store();
        let result = run_query(&store, "test", "NOT VALID CYPHER !!!");
        assert!(result.is_err(), "malformed Cypher should return an error");
    }
}
