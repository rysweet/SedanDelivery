//! Star-expansion for `RETURN *` and `WITH *` clauses.

use crate::ast::{Expr, Identifier, ReturnItem};

/// If `items` contains a `RETURN *` / `WITH *` placeholder, expand it to
/// all user-visible variables discovered from the child operator tree.
/// Non-star items are returned unchanged.  Handles both pure `RETURN *`
/// and mixed forms like `RETURN *, n.name AS extra`.
pub(crate) fn expand_star_items(
    items: &[ReturnItem],
    child: &crate::planner::Operator,
) -> Vec<ReturnItem> {
    let has_star = items.iter().any(|item| {
        matches!(&item.expr, Expr::Variable(id) if id.0 == "*") && item.alias.is_none()
    });

    if !has_star {
        return items.to_vec();
    }

    let aliases = crate::planner::collect_visible_aliases(child);
    let star_items: Vec<ReturnItem> = aliases
        .into_iter()
        .map(|name| ReturnItem {
            expr: Expr::Variable(Identifier::new(name)),
            alias: None,
        })
        .collect();

    let mut result = Vec::new();
    for item in items {
        if matches!(&item.expr, Expr::Variable(id) if id.0 == "*") && item.alias.is_none() {
            result.extend(star_items.clone());
        } else {
            result.push(item.clone());
        }
    }
    result
}
