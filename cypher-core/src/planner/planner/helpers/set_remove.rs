//! Mapping AST mutation clause items to planner operator items.

use crate::ast::{RemoveClauseItem, SetClauseItem};
use crate::planner::{RemoveItem, SetItem};

/// Map an AST `SetClauseItem` to a planner `SetItem`.
pub(crate) fn map_set_clause_item(item: &SetClauseItem) -> SetItem {
    match item {
        SetClauseItem::SetProperty { alias, prop, value } => SetItem::Property {
            alias: alias.clone(),
            key: prop.to_string(),
            value: value.clone(),
        },
        SetClauseItem::SetMap { alias, map, merge } => SetItem::Map {
            alias: alias.clone(),
            map: map.clone(),
            merge: *merge,
        },
        SetClauseItem::SetLabels { alias, labels } => SetItem::Labels {
            alias: alias.clone(),
            labels: labels.clone(),
        },
    }
}

/// Map an AST `RemoveClauseItem` to a planner `RemoveItem`.
pub(crate) fn map_remove_clause_item(item: &RemoveClauseItem) -> RemoveItem {
    match item {
        RemoveClauseItem::RemoveProperty { alias, prop } => RemoveItem::Property {
            alias: alias.clone(),
            key: prop.to_string(),
        },
        RemoveClauseItem::RemoveLabels { alias, labels } => RemoveItem::Labels {
            alias: alias.clone(),
            labels: labels.clone(),
        },
    }
}
