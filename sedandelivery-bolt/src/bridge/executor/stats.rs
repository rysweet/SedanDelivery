//! Convert cypher-core mutation statistics into a Bolt summary map.

use cypher_core::executor::MutationStats;

use crate::packstream::value::{BoltMap, BoltValue};

/// Convert `MutationStats` to a `BoltMap`, including only non-zero counters.
///
/// Counter names follow the Neo4j convention:
/// - `"nodes-created"`
/// - `"nodes-deleted"`
/// - `"relationships-created"`
/// - `"relationships-deleted"`
/// - `"properties-set"`
/// - `"labels-added"`
pub fn stats_to_bolt_map(stats: &MutationStats) -> BoltMap {
    let mut map: BoltMap = Vec::new();

    if stats.nodes_created > 0 {
        map.push((
            "nodes-created".to_string(),
            BoltValue::Int(stats.nodes_created as i64),
        ));
    }
    if stats.nodes_deleted > 0 {
        map.push((
            "nodes-deleted".to_string(),
            BoltValue::Int(stats.nodes_deleted as i64),
        ));
    }
    if stats.edges_created > 0 {
        map.push((
            "relationships-created".to_string(),
            BoltValue::Int(stats.edges_created as i64),
        ));
    }
    if stats.edges_deleted > 0 {
        map.push((
            "relationships-deleted".to_string(),
            BoltValue::Int(stats.edges_deleted as i64),
        ));
    }
    if stats.properties_set > 0 {
        map.push((
            "properties-set".to_string(),
            BoltValue::Int(stats.properties_set as i64),
        ));
    }
    if stats.labels_added > 0 {
        map.push((
            "labels-added".to_string(),
            BoltValue::Int(stats.labels_added as i64),
        ));
    }
    if stats.labels_removed > 0 {
        map.push((
            "labels-removed".to_string(),
            BoltValue::Int(stats.labels_removed as i64),
        ));
    }

    map
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_to_bolt_map_empty_for_no_mutations() {
        let stats = MutationStats::default();
        let map = stats_to_bolt_map(&stats);
        assert!(map.is_empty());
    }

    #[test]
    fn stats_to_bolt_map_includes_nonzero() {
        let mut stats = MutationStats::default();
        stats.nodes_created = 3;
        stats.edges_created = 2;
        let map = stats_to_bolt_map(&stats);
        assert_eq!(map.len(), 2);

        let has_nodes_created = map
            .iter()
            .any(|(k, v)| k == "nodes-created" && *v == BoltValue::Int(3));
        assert!(has_nodes_created);

        let has_rels_created = map
            .iter()
            .any(|(k, v)| k == "relationships-created" && *v == BoltValue::Int(2));
        assert!(has_rels_created);
    }
}
