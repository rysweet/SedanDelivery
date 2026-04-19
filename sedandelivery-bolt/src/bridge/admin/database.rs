//! CREATE / DROP / SHOW DATABASE handlers (also CREATE/DROP GRAPH).

use std::collections::HashMap;

use cypher_core::error::{GqlError, StorageError};
use cypher_core::executor::{MutationStats, QueryResult};
use cypher_core::storage::GraphStore;
use cypher_core::types::Value;

use super::{empty_result, extract_identifier, syntax_error, AdminResult};

pub(super) fn handle_create_database(
    store: &dyn GraphStore,
    query: &str,
) -> Result<AdminResult, GqlError> {
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

pub(super) fn handle_drop_database(
    store: &dyn GraphStore,
    query: &str,
) -> Result<AdminResult, GqlError> {
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

pub(super) fn handle_show_databases(store: &dyn GraphStore) -> Result<AdminResult, GqlError> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::storage::InMemoryGraphStore;

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
