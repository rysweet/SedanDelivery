//! Parity tests: sedan-gql-embedded vs sedan-cypher-embedded.
//!
//! Verifies that for standard Cypher queries (no GQL-only keywords),
//! SedanGql produces the same results as CypherEngine. This ensures
//! the GQL rewrite layer doesn't corrupt shared syntax.

use sedan_cypher_embedded::{CypherEngine, GraphStore as _, InMemoryGraphStore as CypherStore};
use sedan_gql_embedded::{InMemoryGraphStore as GqlStore, SedanGql};

fn gql_setup() -> (SedanGql, GqlStore) {
    let store = GqlStore::default();
    store.create_graph("g").unwrap();
    (SedanGql::new(), store)
}

fn cypher_setup() -> (CypherEngine, CypherStore) {
    let store = CypherStore::default();
    store.create_graph("g").unwrap();
    (CypherEngine::new(), store)
}

// ── RETURN literal parity ────────────────────────────────────────────────────

#[test]
fn return_integer_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let gql = ge.execute(&gs, "g", "RETURN 42 AS val").unwrap();
    let cyp = ce.execute(&cs, "g", "RETURN 42 AS val").unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.columns, cyp.columns);
    assert_eq!(gql.rows[0].get("val"), cyp.rows[0].get("val"));
}

#[test]
fn return_string_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let gql = ge.execute(&gs, "g", "RETURN 'hello' AS val").unwrap();
    let cyp = ce.execute(&cs, "g", "RETURN 'hello' AS val").unwrap();

    assert_eq!(gql.rows[0].get("val"), cyp.rows[0].get("val"));
}

#[test]
fn return_boolean_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let gql = ge.execute(&gs, "g", "RETURN true AS val").unwrap();
    let cyp = ce.execute(&cs, "g", "RETURN true AS val").unwrap();

    assert_eq!(gql.rows[0].get("val"), cyp.rows[0].get("val"));
}

#[test]
fn return_null_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let gql = ge.execute(&gs, "g", "RETURN null AS val").unwrap();
    let cyp = ce.execute(&cs, "g", "RETURN null AS val").unwrap();

    assert_eq!(gql.rows[0].get("val"), cyp.rows[0].get("val"));
}

#[test]
fn return_list_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let gql = ge.execute(&gs, "g", "RETURN [1, 2, 3] AS val").unwrap();
    let cyp = ce.execute(&cs, "g", "RETURN [1, 2, 3] AS val").unwrap();

    assert_eq!(gql.rows[0].get("val"), cyp.rows[0].get("val"));
}

// ── Arithmetic parity ────────────────────────────────────────────────────────

#[test]
fn arithmetic_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let gql = ge.execute(&gs, "g", "RETURN 2 + 3 * 4 AS val").unwrap();
    let cyp = ce.execute(&cs, "g", "RETURN 2 + 3 * 4 AS val").unwrap();

    assert_eq!(gql.rows[0].get("val"), cyp.rows[0].get("val"));
}

// ── CREATE + MATCH parity ────────────────────────────────────────────────────

#[test]
fn create_and_match_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let create = "CREATE (n:Person {name: 'Alice', age: 30}) RETURN n.name";
    let gql_created = ge.execute(&gs, "g", create).unwrap();
    let cyp_created = ce.execute(&cs, "g", create).unwrap();

    assert_eq!(gql_created.rows.len(), cyp_created.rows.len());
    assert_eq!(
        gql_created.rows[0].get("n.name"),
        cyp_created.rows[0].get("n.name")
    );

    let query = "MATCH (n:Person) RETURN n.name, n.age";
    let gql = ge.execute(&gs, "g", query).unwrap();
    let cyp = ce.execute(&cs, "g", query).unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows[0].get("n.name"), cyp.rows[0].get("n.name"));
    assert_eq!(gql.rows[0].get("n.age"), cyp.rows[0].get("n.age"));
}

// ── WHERE parity ─────────────────────────────────────────────────────────────

#[test]
fn where_clause_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let seed = |e: &dyn Fn(&str)| {
        e("CREATE (n:Person {name: 'Alice', age: 30})");
        e("CREATE (n:Person {name: 'Bob', age: 20})");
    };

    seed(&|q| {
        ge.execute(&gs, "g", q).unwrap();
    });
    seed(&|q| {
        ce.execute(&cs, "g", q).unwrap();
    });

    let query = "MATCH (n:Person) WHERE n.age > 25 RETURN n.name";
    let gql = ge.execute(&gs, "g", query).unwrap();
    let cyp = ce.execute(&cs, "g", query).unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows[0].get("n.name"), cyp.rows[0].get("n.name"));
}

// ── Relationship parity ─────────────────────────────────────────────────────

#[test]
fn relationship_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let create = "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})";
    ge.execute(&gs, "g", create).unwrap();
    ce.execute(&cs, "g", create).unwrap();

    let query = "MATCH (a:Person)-[:KNOWS]->(b:Person) RETURN a.name, b.name";
    let gql = ge.execute(&gs, "g", query).unwrap();
    let cyp = ce.execute(&cs, "g", query).unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows[0].get("a.name"), cyp.rows[0].get("a.name"));
    assert_eq!(gql.rows[0].get("b.name"), cyp.rows[0].get("b.name"));
}

// ── UNWIND parity ────────────────────────────────────────────────────────────

#[test]
fn unwind_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let query = "UNWIND [1, 2, 3] AS x RETURN x";
    let gql = ge.execute(&gs, "g", query).unwrap();
    let cyp = ce.execute(&cs, "g", query).unwrap();

    assert_eq!(gql.rows, cyp.rows);
}

// ── Error parity ─────────────────────────────────────────────────────────────

#[test]
fn invalid_syntax_error_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    assert!(ge.execute(&gs, "g", "XYZZY").is_err());
    assert!(ce.execute(&cs, "g", "XYZZY").is_err());
}

#[test]
fn missing_graph_error_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    assert!(ge.execute(&gs, "nonexistent", "RETURN 1").is_err());
    assert!(ce.execute(&cs, "nonexistent", "RETURN 1").is_err());
}

// ── GQL-specific: INSERT matches Cypher CREATE ──────────────────────────────

#[test]
fn gql_insert_matches_cypher_create() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let gql = ge
        .execute(&gs, "g", "INSERT (n:Person {name: 'Alice'}) RETURN n.name")
        .unwrap();
    let cyp = ce
        .execute(&cs, "g", "CREATE (n:Person {name: 'Alice'}) RETURN n.name")
        .unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows[0].get("n.name"), cyp.rows[0].get("n.name"));
}

// ── GQL FILTER matches Cypher WHERE ──────────────────────────────────────────

#[test]
fn gql_filter_matches_cypher_where() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    ge.execute(&gs, "g", "CREATE (n:N {val: 10})").unwrap();
    ge.execute(&gs, "g", "CREATE (n:N {val: 20})").unwrap();
    ce.execute(&cs, "g", "CREATE (n:N {val: 10})").unwrap();
    ce.execute(&cs, "g", "CREATE (n:N {val: 20})").unwrap();

    let gql = ge
        .execute(&gs, "g", "MATCH (n:N) FILTER n.val > 15 RETURN n.val")
        .unwrap();
    let cyp = ce
        .execute(&cs, "g", "MATCH (n:N) WHERE n.val > 15 RETURN n.val")
        .unwrap();

    assert_eq!(gql.rows.len(), cyp.rows.len());
    assert_eq!(gql.rows[0].get("n.val"), cyp.rows[0].get("n.val"));
}

// ── GQL FOR matches Cypher UNWIND ────────────────────────────────────────────

#[test]
fn gql_for_matches_cypher_unwind() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let gql = ge
        .execute(&gs, "g", "FOR x IN [10, 20, 30] RETURN x")
        .unwrap();
    let cyp = ce
        .execute(&cs, "g", "UNWIND [10, 20, 30] AS x RETURN x")
        .unwrap();

    assert_eq!(gql.rows, cyp.rows);
}

// ── GQL-only keyword rejected by CypherEngine ──────────────────────────────

#[test]
fn cypher_engine_rejects_gql_insert() {
    let (_, cs) = cypher_setup();
    let ce = CypherEngine::new();
    assert!(
        ce.execute(&cs, "g", "INSERT (n:Person {name: 'Alice'}) RETURN n.name")
            .is_err(),
        "CypherEngine should reject GQL-only INSERT"
    );
}

// ── Multiple RETURN columns parity ──────────────────────────────────────────

#[test]
fn multiple_columns_parity() {
    let (ge, gs) = gql_setup();
    let (ce, cs) = cypher_setup();

    let query = "RETURN 1 AS a, 'two' AS b, true AS c";
    let gql = ge.execute(&gs, "g", query).unwrap();
    let cyp = ce.execute(&cs, "g", query).unwrap();

    assert_eq!(gql.rows, cyp.rows);
    assert_eq!(gql.columns, cyp.columns);
}
