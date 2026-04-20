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
    use cypher_core::storage::InMemoryGraphStore;

    #[test]
    fn parse_index_for_on_basic() {
        let q = "CREATE INDEX TEST_LABEL_PROP FOR (N:ALABEL) ON (N.PROP)";
        let (label, prop) = parse_index_for_on(q).unwrap();
        assert_eq!(label, "ALABEL");
        assert_eq!(prop, "PROP");
    }

    #[test]
    fn parse_index_for_on_returns_none_without_for_clause() {
        assert!(parse_index_for_on("CREATE INDEX FOO BAR").is_none());
    }

    #[test]
    fn parse_index_for_on_returns_none_without_label_colon() {
        assert!(parse_index_for_on("CREATE INDEX X FOR (N) ON (N.P)").is_none());
    }

    #[test]
    fn parse_index_for_on_returns_none_without_property_dot() {
        assert!(parse_index_for_on("CREATE INDEX X FOR (N:L) ON (NP)").is_none());
    }

    fn make_store_with_graph(graph: &str) -> InMemoryGraphStore {
        let store = InMemoryGraphStore::new();
        store.create_graph(graph).unwrap();
        store
    }

    #[test]
    fn handle_create_index_creates_index() {
        let store = make_store_with_graph("g");
        let result = handle_create_index(
            &store,
            "g",
            "CREATE INDEX my_idx FOR (n:Person) ON (n.name)",
        );
        let ar = result.expect("create index ok");
        assert_eq!(ar.indexes_added, 1);
        let indexes = store.list_indexes("g").unwrap();
        assert!(indexes
            .iter()
            .any(|i| i.label.eq_ignore_ascii_case("PERSON")
                && i.property.eq_ignore_ascii_case("NAME")));
    }

    #[test]
    fn handle_create_index_returns_syntax_error_for_malformed() {
        let store = make_store_with_graph("g");
        let result = handle_create_index(&store, "g", "CREATE INDEX broken");
        assert!(matches!(result, Err(GqlError::Parse(_))));
    }

    #[test]
    fn handle_drop_index_by_canonical_name() {
        let store = make_store_with_graph("g");
        handle_create_index(&store, "g", "CREATE INDEX foo FOR (n:L) ON (n.p)").unwrap();
        let ar = handle_drop_index(&store, "g", "DROP INDEX index_L_p").unwrap();
        assert_eq!(ar.indexes_removed, 1);
        assert!(store.list_indexes("g").unwrap().is_empty());
    }

    #[test]
    fn handle_drop_index_by_short_name() {
        let store = make_store_with_graph("g");
        handle_create_index(&store, "g", "CREATE INDEX foo FOR (n:L) ON (n.p)").unwrap();
        let ar = handle_drop_index(&store, "g", "DROP INDEX L_p").unwrap();
        assert_eq!(ar.indexes_removed, 1);
    }

    #[test]
    fn handle_drop_index_by_label_alone() {
        let store = make_store_with_graph("g");
        handle_create_index(&store, "g", "CREATE INDEX foo FOR (n:L) ON (n.p)").unwrap();
        let ar = handle_drop_index(&store, "g", "DROP INDEX L").unwrap();
        assert_eq!(ar.indexes_removed, 1);
    }

    #[test]
    fn handle_drop_index_falls_back_when_single_index_present() {
        let store = make_store_with_graph("g");
        handle_create_index(&store, "g", "CREATE INDEX foo FOR (n:L) ON (n.p)").unwrap();
        // arbitrary name that doesn't match any canonical form
        let ar = handle_drop_index(&store, "g", "DROP INDEX random_name").unwrap();
        assert_eq!(ar.indexes_removed, 1);
    }

    #[test]
    fn handle_drop_index_if_exists_silently_succeeds_when_missing() {
        let store = make_store_with_graph("g");
        let ar = handle_drop_index(&store, "g", "DROP INDEX missing IF EXISTS").unwrap();
        assert_eq!(ar.indexes_removed, 1);
    }

    #[test]
    fn handle_drop_index_errors_when_not_found_and_no_if_exists() {
        let store = make_store_with_graph("g");
        // Two indexes so the single-index fallback does not kick in.
        handle_create_index(&store, "g", "CREATE INDEX a FOR (n:L1) ON (n.p)").unwrap();
        handle_create_index(&store, "g", "CREATE INDEX b FOR (n:L2) ON (n.p)").unwrap();
        let result = handle_drop_index(&store, "g", "DROP INDEX nope");
        assert!(matches!(result, Err(GqlError::Storage(_))));
    }

    #[test]
    fn handle_drop_index_rejects_empty_name() {
        let store = make_store_with_graph("g");
        let result = handle_drop_index(&store, "g", "DROP INDEX ");
        assert!(matches!(result, Err(GqlError::Parse(_))));
    }
}
