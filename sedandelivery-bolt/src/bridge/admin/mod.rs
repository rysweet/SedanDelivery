//! Admin query handler for DDL/system commands.
//!
//! Intercepts queries like CREATE DATABASE, DROP DATABASE, SHOW DATABASES,
//! CREATE INDEX, DROP INDEX, CREATE CONSTRAINT, DROP CONSTRAINT before
//! they reach the cypher-core parser (which only handles DML).

use cypher_core::error::{GqlError, ParseError};
use cypher_core::executor::{MutationStats, QueryResult};
use cypher_core::storage::GraphStore;

mod constraint;
mod database;
mod index;

use constraint::{handle_create_constraint, handle_drop_constraint};
use database::{handle_create_database, handle_drop_database, handle_show_databases};
use index::{handle_create_index, handle_drop_index};

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

pub(super) fn empty_result() -> QueryResult {
    QueryResult {
        columns: vec![],
        rows: vec![],
        stats: MutationStats::default(),
        trailing_error: None,
    }
}

pub(super) fn syntax_error(msg: &str) -> GqlError {
    GqlError::Parse(ParseError::Syntax {
        offset: 0,
        expected: msg.to_string(),
        found: String::new(),
    })
}

/// Extract an identifier from the start of a string.
/// Handles backtick-quoted identifiers (`my-db`) and simple identifiers.
/// Backtick-escaped backticks (`` `` ``) within quoted identifiers are unescaped.
pub(super) fn extract_identifier(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
