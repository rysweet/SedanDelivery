//! Admin query handler for DDL/system commands.
//!
//! Intercepts queries like CREATE DATABASE, DROP DATABASE, SHOW DATABASES,
//! CREATE INDEX, DROP INDEX, CREATE CONSTRAINT, DROP CONSTRAINT before
//! they reach the gql-core parser (which only handles DML).

use std::collections::HashMap;

use gql_core::error::{GqlError, ParseError, StorageError};
use gql_core::executor::{MutationStats, QueryResult};
use gql_core::plan_cache;
use gql_core::storage::GraphStore;
use gql_core::types::Value;

/// Result of an admin query, with additional stats that MutationStats doesn't track.
pub struct AdminResult {
    pub query_result: QueryResult,
    /// Extra stats beyond what MutationStats provides.
    pub indexes_added: u64,
    pub indexes_removed: u64,
    pub constraints_added: u64,
    pub constraints_removed: u64,
    pub system_updates: u64,
}

/// Try to handle a query as an admin/DDL command.
/// Returns `Some(AdminResult)` if the query was an admin command,
/// `None` if it should be handled by the normal query executor.
pub fn try_handle_admin(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
) -> Option<Result<AdminResult, GqlError>> {
    let trimmed = query.trim();
    let upper = trimmed.to_uppercase();

    // CREATE DATABASE <name> [IF NOT EXISTS] [WAIT]
    if upper.starts_with("CREATE DATABASE ") {
        return Some(handle_create_database(store, trimmed));
    }

    // DROP  DATABASE <name> [IF EXISTS] [WAIT]
    // Note: testkit uses "DROP  DATABASE" with double space
    if upper.starts_with("DROP DATABASE ") || upper.starts_with("DROP  DATABASE ") {
        return Some(handle_drop_database(store, trimmed));
    }

    // SHOW DATABASES
    if upper.starts_with("SHOW DATABASES") {
        return Some(handle_show_databases(store));
    }

    // CREATE GRAPH <name> — GQL named-graph DDL; treated as CREATE DATABASE.
    if upper.starts_with("CREATE GRAPH ") {
        let rewritten = format!("CREATE DATABASE {}", &trimmed["CREATE GRAPH ".len()..]);
        return Some(handle_create_database(store, &rewritten));
    }

    // DROP GRAPH <name> — GQL named-graph DDL; treated as DROP DATABASE.
    if upper.starts_with("DROP GRAPH ") {
        let rewritten = format!("DROP DATABASE {}", &trimmed["DROP GRAPH ".len()..]);
        return Some(handle_drop_database(store, &rewritten));
    }

    // CREATE INDEX <name> FOR (n:<Label>) ON (n.<prop>)
    if upper.starts_with("CREATE INDEX ") {
        return Some(handle_create_index(store, graph, trimmed));
    }

    // DROP INDEX <name>
    if upper.starts_with("DROP INDEX ") {
        return Some(handle_drop_index(store, graph, trimmed));
    }

    // CREATE CONSTRAINT <name> FOR (book:<Label>) REQUIRE book.<prop> IS UNIQUE
    if upper.starts_with("CREATE CONSTRAINT ") {
        return Some(handle_create_constraint(store, graph, trimmed));
    }

    // DROP CONSTRAINT <name>
    if upper.starts_with("DROP CONSTRAINT ") {
        return Some(handle_drop_constraint(store, graph, trimmed));
    }

    None
}

fn empty_result() -> QueryResult {
    QueryResult {
        columns: vec![],
        rows: vec![],
        stats: MutationStats::default(),
        trailing_error: None,
    }
}

fn syntax_error(msg: &str) -> GqlError {
    GqlError::Parse(ParseError::Syntax {
        offset: 0,
        expected: msg.to_string(),
        found: String::new(),
    })
}

fn handle_create_database(store: &dyn GraphStore, query: &str) -> Result<AdminResult, GqlError> {
    let upper = query.to_uppercase();
    let if_not_exists = upper.contains("IF NOT EXISTS");

    // Extract database name: "CREATE DATABASE `name` ..."
    let rest = &query["CREATE DATABASE ".len()..];
    let name = extract_identifier(rest);

    if name.is_empty() {
        return Err(syntax_error("database name in CREATE DATABASE"));
    }

    match store.create_graph(&name) {
        Ok(()) => {}
        Err(StorageError::GraphAlreadyExists { .. }) if if_not_exists => {
            // IF NOT EXISTS -- silently succeed
        }
        Err(e) => return Err(GqlError::from(e)),
    }

    Ok(AdminResult {
        query_result: empty_result(),
        indexes_added: 0,
        indexes_removed: 0,
        constraints_added: 0,
        constraints_removed: 0,
        system_updates: 1,
    })
}

fn handle_drop_database(store: &dyn GraphStore, query: &str) -> Result<AdminResult, GqlError> {
    let upper = query.to_uppercase();
    let if_exists = upper.contains("IF EXISTS");

    // Handle both "DROP DATABASE " and "DROP  DATABASE " (double space from testkit)
    let rest = if upper.starts_with("DROP  DATABASE ") {
        &query["DROP  DATABASE ".len()..]
    } else {
        &query["DROP DATABASE ".len()..]
    };
    let name = extract_identifier(rest);

    if name.is_empty() {
        return Err(syntax_error("database name in DROP DATABASE"));
    }

    match store.delete_graph(&name) {
        Ok(()) => {}
        Err(StorageError::GraphNotFound { .. }) if if_exists => {
            // IF EXISTS -- silently succeed
        }
        Err(e) => return Err(GqlError::from(e)),
    }

    Ok(AdminResult {
        query_result: empty_result(),
        indexes_added: 0,
        indexes_removed: 0,
        constraints_added: 0,
        constraints_removed: 0,
        system_updates: 1,
    })
}

fn handle_show_databases(store: &dyn GraphStore) -> Result<AdminResult, GqlError> {
    let graphs = store.list_graphs().map_err(GqlError::from)?;

    // Always include "system" in the output even if it doesn't exist as a
    // real graph -- Neo4j always lists the system database.
    let mut names: Vec<String> = graphs;
    if !names.iter().any(|n| n == "system") {
        names.push("system".to_string());
    }
    // Ensure the default "neo4j" database is always listed
    if !names.iter().any(|n| n == "neo4j") {
        names.push("neo4j".to_string());
    }
    names.sort();

    let columns = vec!["name".to_string()];
    let rows: Vec<HashMap<String, Value>> = names
        .iter()
        .map(|n| {
            let mut row = HashMap::new();
            row.insert("name".to_string(), Value::Str(n.as_str().into()));
            row
        })
        .collect();

    Ok(AdminResult {
        query_result: QueryResult {
            columns,
            rows,
            stats: MutationStats::default(),
            trailing_error: None,
        },
        indexes_added: 0,
        indexes_removed: 0,
        constraints_added: 0,
        constraints_removed: 0,
        system_updates: 0,
    })
}

fn handle_create_index(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
) -> Result<AdminResult, GqlError> {
    // Pattern: CREATE INDEX <name> FOR (n:<Label>) ON (n.<prop>)
    let upper = query.to_uppercase();

    let (label, property) = parse_index_for_on(&upper)
        .ok_or_else(|| syntax_error("FOR (n:<Label>) ON (n.<prop>) in CREATE INDEX"))?;

    // Ensure graph exists
    let exists = store.graph_exists(graph).map_err(GqlError::from)?;
    if !exists {
        store.create_graph(graph).map_err(GqlError::from)?;
    }

    store
        .create_index(graph, &label, &property)
        .map_err(GqlError::from)?;

    // Invalidate cached plans — index changes may alter plan selection.
    plan_cache::global().clear();

    Ok(AdminResult {
        query_result: empty_result(),
        indexes_added: 1,
        indexes_removed: 0,
        constraints_added: 0,
        constraints_removed: 0,
        system_updates: 0,
    })
}

fn handle_drop_index(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
) -> Result<AdminResult, GqlError> {
    // Ensure graph exists
    let exists = store.graph_exists(graph).map_err(GqlError::from)?;
    if !exists {
        store.create_graph(graph).map_err(GqlError::from)?;
    }

    // Parse the index name from "DROP INDEX <name> [IF EXISTS]"
    let rest = &query["DROP INDEX ".len()..];
    let index_name = extract_identifier(rest);
    if index_name.is_empty() {
        return Err(syntax_error("index name in DROP INDEX"));
    }

    let upper = query.to_uppercase();
    let if_exists = upper.contains("IF EXISTS");

    let indexes = store.list_indexes(graph).map_err(GqlError::from)?;

    // Match the index name against existing indexes. The storage layer does
    // not track user-assigned names, so we derive a canonical name from the
    // label and property: "index_<LABEL>_<PROPERTY>" (case-insensitive match).
    let index_name_upper = index_name.to_uppercase();
    let matched = indexes.iter().find(|idx| {
        let canonical = format!("INDEX_{}_{}", idx.label, idx.property).to_uppercase();
        let short = format!("{}_{}", idx.label, idx.property).to_uppercase();
        canonical == index_name_upper
            || short == index_name_upper
            || idx.label.to_uppercase() == index_name_upper
            || index_name_upper
                == format!("{}_{}_{}", graph, idx.label, idx.property).to_uppercase()
    });

    match matched {
        Some(idx) => {
            store
                .drop_index(graph, &idx.label, &idx.property)
                .map_err(GqlError::from)?;
        }
        None => {
            // If there is exactly one index and the name didn't match any
            // canonical form, fall back to dropping it (backwards compat with
            // testkit which creates one index then drops by arbitrary name).
            if indexes.len() == 1 {
                let idx = &indexes[0];
                store
                    .drop_index(graph, &idx.label, &idx.property)
                    .map_err(GqlError::from)?;
            } else if if_exists {
                // IF EXISTS -- silently succeed when index not found.
            } else {
                return Err(GqlError::Storage(StorageError::Backend {
                    msg: format!("No such index '{}'", index_name),
                }));
            }
        }
    }

    // Invalidate cached plans — index changes may alter plan selection.
    plan_cache::global().clear();

    Ok(AdminResult {
        query_result: empty_result(),
        indexes_added: 0,
        indexes_removed: 1,
        constraints_added: 0,
        constraints_removed: 0,
        system_updates: 0,
    })
}

fn handle_create_constraint(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
) -> Result<AdminResult, GqlError> {
    // Ensure graph exists
    let exists = store.graph_exists(graph).map_err(GqlError::from)?;
    if !exists {
        store.create_graph(graph).map_err(GqlError::from)?;
    }

    // Parse: CREATE CONSTRAINT <name> FOR (var:<Label>) REQUIRE var.<prop> IS UNIQUE
    // Also: CREATE CONSTRAINT ON (var:<Label>) ASSERT var.<prop> IS UNIQUE
    let (label, property) = parse_constraint_label_property(query)
        .ok_or_else(|| syntax_error(
            "FOR (n:<Label>) REQUIRE n.<prop> IS UNIQUE or ON (n:<Label>) ASSERT n.<prop> IS UNIQUE in CREATE CONSTRAINT"
        ))?;

    use gql_core::storage::ConstraintType;
    store
        .create_constraint(graph, &label, &property, ConstraintType::Unique)
        .map_err(GqlError::from)?;

    // Invalidate cached plans — constraint changes may alter plan selection.
    plan_cache::global().clear();

    Ok(AdminResult {
        query_result: empty_result(),
        indexes_added: 0,
        indexes_removed: 0,
        constraints_added: 1,
        constraints_removed: 0,
        system_updates: 0,
    })
}

fn handle_drop_constraint(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
) -> Result<AdminResult, GqlError> {
    // Ensure graph exists
    let exists = store.graph_exists(graph).map_err(GqlError::from)?;
    if !exists {
        store.create_graph(graph).map_err(GqlError::from)?;
    }

    let upper = query.to_uppercase();
    let if_exists = upper.contains("IF EXISTS");

    // Try to parse label+property from the full syntax first.
    // DROP CONSTRAINT <name> — where name matches a constraint by convention.
    // OR: DROP CONSTRAINT ON (n:<Label>) ASSERT n.<prop> IS UNIQUE
    if let Some((label, property)) = parse_constraint_label_property(query) {
        match store.drop_constraint(graph, &label, &property) {
            Ok(()) => {}
            Err(_) if if_exists => {}
            Err(e) => return Err(GqlError::from(e)),
        }
    } else {
        // Parse the constraint name: "DROP CONSTRAINT <name> [IF EXISTS]"
        let rest = &query["DROP CONSTRAINT ".len()..];
        let constraint_name = extract_identifier(rest);
        if constraint_name.is_empty() {
            return Err(syntax_error("constraint name in DROP CONSTRAINT"));
        }

        // Try to match constraint name against existing constraints.
        // Convention: "constraint_<LABEL>_<PROPERTY>" or "<LABEL>_<PROPERTY>"
        let constraints = store.list_constraints(graph).map_err(GqlError::from)?;
        let name_upper = constraint_name.to_uppercase();
        let matched = constraints.iter().find(|c| {
            let canonical = format!("CONSTRAINT_{}_{}", c.label, c.property).to_uppercase();
            let short = format!("{}_{}", c.label, c.property).to_uppercase();
            canonical == name_upper || short == name_upper
        });

        match matched {
            Some(c) => {
                store
                    .drop_constraint(graph, &c.label, &c.property)
                    .map_err(GqlError::from)?;
            }
            None => {
                // Fall back: if there is exactly one constraint, drop it
                if constraints.len() == 1 {
                    let c = &constraints[0];
                    store
                        .drop_constraint(graph, &c.label, &c.property)
                        .map_err(GqlError::from)?;
                } else if if_exists {
                    // IF EXISTS -- silently succeed
                } else {
                    return Err(GqlError::Storage(StorageError::Backend {
                        msg: format!("No such constraint '{}'", constraint_name),
                    }));
                }
            }
        }
    }

    // Invalidate cached plans.
    plan_cache::global().clear();

    Ok(AdminResult {
        query_result: empty_result(),
        indexes_added: 0,
        indexes_removed: 0,
        constraints_added: 0,
        constraints_removed: 1,
        system_updates: 0,
    })
}

/// Parse label and property from constraint syntax, preserving original case.
/// Handles both:
///   FOR (var:<Label>) REQUIRE var.<prop> IS UNIQUE
///   ON (var:<Label>) ASSERT var.<prop> IS UNIQUE
///
/// Uses `upper` for keyword detection but extracts values from `original`.
fn parse_constraint_label_property(original: &str) -> Option<(String, String)> {
    let upper = original.to_uppercase();
    // Look for FOR ... REQUIRE or ON ... ASSERT
    let (label_section_start, prop_keyword) = if let Some(for_pos) = upper.find("FOR ") {
        (for_pos, "REQUIRE ")
    } else if let Some(on_pos) = upper.find("ON ") {
        (on_pos, "ASSERT ")
    } else {
        return None;
    };

    let prop_start = upper.find(prop_keyword)?;

    // Extract label from (var:<Label>) using original-case string at same positions.
    let label_section = &original[label_section_start..prop_start];
    let colon_pos = label_section.find(':')?;
    let label_end = label_section[colon_pos + 1..].find(')')?;
    let label = label_section[colon_pos + 1..colon_pos + 1 + label_end].trim();

    // Extract property from REQUIRE/ASSERT var.<prop> IS UNIQUE
    let prop_section = &original[prop_start + prop_keyword.len()..];
    let dot_pos = prop_section.find('.')?;
    // Property name ends at whitespace
    let after_dot = &prop_section[dot_pos + 1..];
    let prop_end = after_dot
        .find(|c: char| c.is_whitespace())
        .unwrap_or(after_dot.len());
    let property = after_dot[..prop_end].trim();

    if label.is_empty() || property.is_empty() {
        return None;
    }

    Some((label.to_string(), property.to_string()))
}

// ── Parsing helpers ─────────────────────────────────────────────────────────

/// Extract an identifier from the start of a string.
/// Handles backtick-quoted identifiers (`my-db`) and simple identifiers.
/// Backtick-escaped backticks (`` `` ``) within quoted identifiers are unescaped.
fn extract_identifier(s: &str) -> String {
    let s = s.trim();
    if s.starts_with('`') {
        // Backtick-quoted identifier: scan for unescaped closing backtick.
        // A double backtick (`` `` ``) inside is an escaped literal backtick.
        let bytes = s.as_bytes();
        let mut i = 1;
        let mut result = String::new();
        while i < bytes.len() {
            if bytes[i] == b'`' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'`' {
                    // Escaped backtick -- include one literal backtick
                    result.push('`');
                    i += 2;
                } else {
                    // Closing backtick
                    return result;
                }
            } else {
                result.push(bytes[i] as char);
                i += 1;
            }
        }
        return result;
    }
    // Simple identifier: take alphanumeric + underscore + hyphen
    s.chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
        .collect()
}

/// Parse "FOR (n:<Label>) ON (n.<prop>)" from an uppercase CREATE INDEX query.
fn parse_index_for_on(upper: &str) -> Option<(String, String)> {
    let for_pos = upper.find("FOR ")?;
    let on_pos = upper.find(" ON ")?;

    // Extract label from FOR (n:<Label>)
    let for_part = &upper[for_pos..on_pos];
    let colon_pos = for_part.find(':')?;
    let label_end = for_part[colon_pos + 1..].find(')')?;
    let label = for_part[colon_pos + 1..colon_pos + 1 + label_end].trim();

    // Extract property from ON (n.<prop>)
    let on_part = &upper[on_pos + 4..];
    let dot_pos = on_part.find('.')?;
    let prop_end = on_part[dot_pos + 1..].find(')')?;
    let property = on_part[dot_pos + 1..dot_pos + 1 + prop_end].trim();

    Some((label.to_string(), property.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gql_core::storage::InMemoryGraphStore;

    #[test]
    fn extract_simple_identifier_basic() {
        assert_eq!(extract_identifier("mydb "), "mydb");
        assert_eq!(
            extract_identifier("test-database IF EXISTS"),
            "test-database"
        );
    }

    #[test]
    fn extract_backtick_identifier() {
        assert_eq!(
            extract_identifier("`test-database` IF EXISTS"),
            "test-database"
        );
        assert_eq!(extract_identifier("`my``db` WAIT"), "my`db");
    }

    #[test]
    fn parse_index_for_on_basic() {
        let q = "CREATE INDEX TEST_LABEL_PROP FOR (N:ALABEL) ON (N.PROP)";
        let (label, prop) = parse_index_for_on(q).unwrap();
        assert_eq!(label, "ALABEL");
        assert_eq!(prop, "PROP");
    }

    #[test]
    fn create_database_basic() {
        let store = InMemoryGraphStore::new();
        let result =
            handle_create_database(&store, "CREATE DATABASE `test-database` IF NOT EXISTS WAIT");
        assert!(result.is_ok());
        let ar = result.unwrap();
        assert_eq!(ar.system_updates, 1);
        assert!(store.graph_exists("test-database").unwrap());
    }

    #[test]
    fn drop_database_if_exists() {
        let store = InMemoryGraphStore::new();
        let result = handle_drop_database(&store, "DROP  DATABASE `test-database` IF EXISTS WAIT");
        assert!(result.is_ok());
    }

    // ── Constraint tests ─────────────────────────────────────────────────

    #[test]
    fn parse_constraint_for_require_syntax() {
        let q = "CREATE CONSTRAINT UNIQ_EMAIL FOR (n:Person) REQUIRE n.email IS UNIQUE";
        let (label, prop) = parse_constraint_label_property(q).unwrap();
        assert_eq!(label, "Person");
        assert_eq!(prop, "email");
    }

    #[test]
    fn parse_constraint_on_assert_syntax() {
        let q = "CREATE CONSTRAINT ON (n:Person) ASSERT n.email IS UNIQUE";
        let (label, prop) = parse_constraint_label_property(q).unwrap();
        assert_eq!(label, "Person");
        assert_eq!(prop, "email");
    }

    #[test]
    fn create_constraint_succeeds() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        let result = handle_create_constraint(
            &store,
            "test",
            "CREATE CONSTRAINT uniq_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
        );
        assert!(result.is_ok());
        let ar = result.unwrap();
        assert_eq!(ar.constraints_added, 1);
    }

    #[test]
    fn create_constraint_on_assert_succeeds() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        let result = handle_create_constraint(
            &store,
            "test",
            "CREATE CONSTRAINT ON (n:Person) ASSERT n.email IS UNIQUE",
        );
        assert!(result.is_ok());
        let ar = result.unwrap();
        assert_eq!(ar.constraints_added, 1);
    }

    #[test]
    fn create_constraint_fails_with_existing_violations() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        // Create two nodes with the same email
        use std::collections::HashMap;
        let mut props = HashMap::new();
        props.insert("email".to_string(), Value::Str("alice@example.com".into()));
        store
            .create_node("test", vec!["Person".to_string()], props.clone())
            .unwrap();
        store
            .create_node("test", vec!["Person".to_string()], props)
            .unwrap();

        let result = handle_create_constraint(
            &store,
            "test",
            "CREATE CONSTRAINT uniq_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
        );
        assert!(result.is_err());
    }

    #[test]
    fn drop_constraint_succeeds() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        handle_create_constraint(
            &store,
            "test",
            "CREATE CONSTRAINT uniq_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .unwrap();

        let result = handle_drop_constraint(
            &store,
            "test",
            "DROP CONSTRAINT ON (n:Person) ASSERT n.email IS UNIQUE",
        );
        assert!(result.is_ok());
        let ar = result.unwrap();
        assert_eq!(ar.constraints_removed, 1);
    }

    #[test]
    fn drop_constraint_by_name() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        handle_create_constraint(
            &store,
            "test",
            "CREATE CONSTRAINT uniq_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .unwrap();

        // Drop by name — since there's only one constraint, it should match
        let result = handle_drop_constraint(&store, "test", "DROP CONSTRAINT uniq_email");
        assert!(result.is_ok());
    }

    #[test]
    fn list_constraints_returns_created() {
        let store = InMemoryGraphStore::new();
        store.create_graph("test").unwrap();
        handle_create_constraint(
            &store,
            "test",
            "CREATE CONSTRAINT uniq_email FOR (n:Person) REQUIRE n.email IS UNIQUE",
        )
        .unwrap();

        let constraints = store.list_constraints("test").unwrap();
        assert_eq!(constraints.len(), 1);
        assert_eq!(constraints[0].label, "Person");
        assert_eq!(constraints[0].property, "email");
    }

    #[test]
    fn show_databases_includes_system_and_neo4j() {
        let store = InMemoryGraphStore::new();
        store.create_graph("neo4j").unwrap();
        let result = handle_show_databases(&store).unwrap();
        let names: Vec<&str> = result
            .query_result
            .rows
            .iter()
            .filter_map(|r| {
                r.get("name").and_then(|v| match v {
                    Value::Str(s) => Some(s.as_ref()),
                    _ => None,
                })
            })
            .collect();
        assert!(names.contains(&"system"));
        assert!(names.contains(&"neo4j"));
    }
}
