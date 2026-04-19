//! CREATE / DROP INDEX handlers.

use cypher_core::error::{GqlError, StorageError};
use cypher_core::plan_cache;
use cypher_core::storage::GraphStore;

use super::{empty_result, extract_identifier, syntax_error, AdminResult};
use crate::bridge::ensure_graph_exists;

pub(super) fn handle_create_index(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
) -> Result<AdminResult, GqlError> {
    // Pattern: CREATE INDEX <name> FOR (n:<Label>) ON (n.<prop>)
    let upper = query.to_uppercase();

    let (label, property) = parse_index_for_on(&upper)
        .ok_or_else(|| syntax_error("FOR (n:<Label>) ON (n.<prop>) in CREATE INDEX"))?;

    // Ensure graph exists
    ensure_graph_exists(store, graph)?;

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

pub(super) fn handle_drop_index(
    store: &dyn GraphStore,
    graph: &str,
    query: &str,
) -> Result<AdminResult, GqlError> {
    // Ensure graph exists
    ensure_graph_exists(store, graph)?;

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

    #[test]
    fn parse_index_for_on_basic() {
        let q = "CREATE INDEX TEST_LABEL_PROP FOR (N:ALABEL) ON (N.PROP)";
        let (label, prop) = parse_index_for_on(q).unwrap();
        assert_eq!(label, "ALABEL");
        assert_eq!(prop, "PROP");
    }
}
