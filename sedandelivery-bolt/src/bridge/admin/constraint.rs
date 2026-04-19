//! CREATE / DROP CONSTRAINT handlers.

use cypher_core::error::{GqlError, StorageError};
use cypher_core::plan_cache;
use cypher_core::storage::GraphStore;

use super::{empty_result, extract_identifier, syntax_error, AdminResult};
use crate::bridge::ensure_graph_exists;

pub(super) fn handle_create_constraint(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
) -> Result<AdminResult, GqlError> {
    // Ensure graph exists
    ensure_graph_exists(store, graph)?;

    // Parse: CREATE CONSTRAINT <name> FOR (var:<Label>) REQUIRE var.<prop> IS UNIQUE
    // Also: CREATE CONSTRAINT ON (var:<Label>) ASSERT var.<prop> IS UNIQUE
    let (label, property) = parse_constraint_label_property(query)
        .ok_or_else(|| syntax_error(
            "FOR (n:<Label>) REQUIRE n.<prop> IS UNIQUE or ON (n:<Label>) ASSERT n.<prop> IS UNIQUE in CREATE CONSTRAINT"
        ))?;

    use cypher_core::storage::ConstraintType;
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

pub(super) fn handle_drop_constraint(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
) -> Result<AdminResult, GqlError> {
    // Ensure graph exists
    ensure_graph_exists(store, graph)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use cypher_core::storage::InMemoryGraphStore;
    use cypher_core::types::Value;

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
        use indexmap::IndexMap;
        let mut props = IndexMap::new();
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
}
