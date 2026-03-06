//! Helper functions for the query planner — expression utilities, property extraction,
//! aggregation detection, and column name derivation.

use std::collections::HashMap;

use crate::ast::{
    Expr, Identifier, Label, Properties, RemoveClauseItem, ReturnItem, SetClauseItem,
};
use crate::error::{ExecutionError, GqlError};
use crate::planner::{AggFunction, RemoveItem, SetItem};
use crate::procedure::ProcedureRegistry;
use crate::types::Value;

/// Named path binding: `(path_alias, [node_aliases], [edge_aliases])`.
/// Used during CREATE planning to wrap the result with `AssemblePath`.
pub(crate) type PathBinding = (Identifier, Vec<Identifier>, Vec<Identifier>);

// ── Static defaults for backward compatibility ──────────────────────────────

static EMPTY_REGISTRY: std::sync::OnceLock<ProcedureRegistry> = std::sync::OnceLock::new();
static EMPTY_PARAMS: std::sync::OnceLock<HashMap<String, Value>> = std::sync::OnceLock::new();

pub(crate) fn empty_registry() -> &'static ProcedureRegistry {
    EMPTY_REGISTRY.get_or_init(ProcedureRegistry::new)
}

pub(crate) fn empty_params() -> &'static HashMap<String, Value> {
    EMPTY_PARAMS.get_or_init(HashMap::new)
}

// ── Free helpers ──────────────────────────────────────────────────────────────

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

    // Replace each `*` placeholder with the expanded aliases; keep non-star items.
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

/// Rewrite an ORDER BY expression when aggregation is active.
/// If the expression is an aggregate function call that matches a RETURN item,
/// replace it with a variable reference to the output alias.
/// Rewrite an ORDER BY expression in a WITH clause that uses aggregation.
/// Maps pre-projection expressions (like `a.name`) to their post-aggregation aliases
/// (like `name` if the WITH has `a.name AS name`). Also handles aggregate functions.
pub(crate) fn rewrite_with_order_expr(expr: &Expr, with_items: &[ReturnItem]) -> Expr {
    // First check if the expression exactly matches a WITH item's expression.
    for ri in with_items {
        if *expr == ri.expr {
            let alias = ri
                .alias
                .clone()
                .map(|a| a.0)
                .unwrap_or_else(|| return_item_name(ri));
            return Expr::Variable(Identifier::new(alias));
        }
    }
    // For compound expressions, try to rewrite sub-expressions.
    // E.g., `a.name + 'C'` where `a.name AS name` -> `name + 'C'`.
    match expr {
        Expr::BinaryOp { op, left, right } => {
            let new_left = rewrite_with_order_expr(left, with_items);
            let new_right = rewrite_with_order_expr(right, with_items);
            Expr::BinaryOp {
                op: *op,
                left: Box::new(new_left),
                right: Box::new(new_right),
            }
        }
        Expr::FunctionCall {
            name,
            args: _,
            distinct: _,
        } => {
            if is_agg_name(name.as_str()) {
                // Map aggregate functions to their output aliases.
                for ri in with_items {
                    if let Expr::FunctionCall { name: rn, .. } = &ri.expr {
                        if name == rn {
                            let alias = ri
                                .alias
                                .clone()
                                .map(|a| a.0)
                                .unwrap_or_else(|| return_item_name(ri));
                            return Expr::Variable(Identifier::new(alias));
                        }
                    }
                }
            }
            expr.clone()
        }
        _ => expr.clone(),
    }
}

pub(crate) fn rewrite_agg_order_expr(expr: &Expr, return_items: &[ReturnItem]) -> Expr {
    // After aggregation, ORDER BY expressions must reference output column names.
    // Try to find the matching RETURN item and rewrite to Variable(output_column_name).
    let expr_col = expr_to_col_name(expr);
    for ri in return_items {
        let ri_col = expr_to_col_name(&ri.expr);
        let ri_alias = return_item_name(ri);
        // Match if the ORDER BY expression equals the return item expression,
        // or if it matches the alias.
        if expr_col == ri_col || expr_col == ri_alias {
            return Expr::Variable(Identifier::new(ri_alias));
        }
    }
    // For compound expressions (e.g. `$age + avg(person.age) - 1000`), recursively
    // rewrite sub-expressions that match return items, replacing agg calls with aliases.
    match expr {
        Expr::BinaryOp { op, left, right } => Expr::BinaryOp {
            op: *op,
            left: Box::new(rewrite_agg_order_expr(left, return_items)),
            right: Box::new(rewrite_agg_order_expr(right, return_items)),
        },
        Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
            op: *op,
            expr: Box::new(rewrite_agg_order_expr(inner, return_items)),
        },
        _ => expr.clone(),
    }
}

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

/// Return the output column name for a RETURN item.
pub(crate) fn return_item_name(item: &ReturnItem) -> String {
    if let Some(alias) = &item.alias {
        return alias.0.clone();
    }
    expr_to_col_name(&item.expr)
}

/// Returns `true` if the expression is or contains an aggregate function call.
pub(crate) fn has_agg_function(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { name, args, .. } => {
            // The function itself is an aggregate, OR one of its args contains an aggregate
            // (e.g. size(collect(a)), all(x IN collect(...) WHERE x))
            is_agg_name(name.as_str()) || args.iter().any(has_agg_function)
        }
        Expr::BinaryOp { left, right, .. } => has_agg_function(left) || has_agg_function(right),
        Expr::UnaryOp { expr, .. } => has_agg_function(expr),
        Expr::Property(obj, _) => has_agg_function(obj),
        Expr::List(items) => items.iter().any(has_agg_function),
        Expr::MapLiteral(pairs) => pairs.iter().any(|(_, v)| has_agg_function(v)),
        Expr::ListComp {
            list, filter, map, ..
        } => {
            has_agg_function(list)
                || filter.as_deref().map(has_agg_function).unwrap_or(false)
                || has_agg_function(map)
        }
        Expr::Case {
            subject,
            alternatives,
            default,
        } => {
            subject.as_deref().map(has_agg_function).unwrap_or(false)
                || alternatives
                    .iter()
                    .any(|(c, r)| has_agg_function(c) || has_agg_function(r))
                || default.as_deref().map(has_agg_function).unwrap_or(false)
        }
        Expr::IsNull(e) | Expr::IsNotNull(e) => has_agg_function(e),
        Expr::Subscript(a, b) => has_agg_function(a) || has_agg_function(b),
        Expr::Slice(a, b, c) => {
            has_agg_function(a)
                || b.as_deref().map(has_agg_function).unwrap_or(false)
                || c.as_deref().map(has_agg_function).unwrap_or(false)
        }
        _ => false,
    }
}

pub(crate) fn is_agg_name(name: &str) -> bool {
    crate::planner::is_aggregate_function(name)
}

/// Extract the aggregate function, first argument, optional second argument, and distinct flag
/// from a top-level `FunctionCall` expression.
pub(crate) fn extract_agg_function(
    expr: &Expr,
) -> Result<(AggFunction, Expr, Option<Expr>, bool), GqlError> {
    match expr {
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => {
            let func = match name.to_uppercase().as_str() {
                "COUNT" => AggFunction::Count,
                "SUM" => AggFunction::Sum,
                "AVG" => AggFunction::Avg,
                "MIN" => AggFunction::Min,
                "MAX" => AggFunction::Max,
                "COLLECT" => AggFunction::Collect,
                "STDEV" => AggFunction::StDev,
                "STDEVP" => AggFunction::StDevP,
                "PERCENTILECONT" => AggFunction::PercentileCont,
                "PERCENTILEDISC" => AggFunction::PercentileDisc,
                other => {
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "aggregate function '{other}'"
                    ))));
                }
            };
            let arg = args.first().cloned().unwrap_or(Expr::Null);
            let second_arg = args.get(1).cloned();
            Ok((func, arg, second_arg, *distinct))
        }
        _ => Err(GqlError::Execution(ExecutionError::unsupported(
            "non-function aggregate expression",
        ))),
    }
}

/// Find the innermost aggregate function call in a compound expression.
/// Returns (AggFunction, arg_expr, second_arg_expr, distinct) if found.
pub(crate) fn find_inner_aggregate(expr: &Expr) -> Option<(AggFunction, Expr, Option<Expr>, bool)> {
    match expr {
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => {
            let func_opt = match name.to_uppercase().as_str() {
                "COUNT" => Some(AggFunction::Count),
                "SUM" => Some(AggFunction::Sum),
                "AVG" => Some(AggFunction::Avg),
                "MIN" => Some(AggFunction::Min),
                "MAX" => Some(AggFunction::Max),
                "COLLECT" => Some(AggFunction::Collect),
                "STDEV" => Some(AggFunction::StDev),
                "STDEVP" => Some(AggFunction::StDevP),
                "PERCENTILECONT" => Some(AggFunction::PercentileCont),
                "PERCENTILEDISC" => Some(AggFunction::PercentileDisc),
                _ => None,
            };
            if let Some(func) = func_opt {
                let arg = args.first().cloned().unwrap_or(Expr::Null);
                let second_arg = args.get(1).cloned();
                Some((func, arg, second_arg, *distinct))
            } else {
                // Non-aggregate function: search inside args for nested aggregate
                args.iter().find_map(find_inner_aggregate)
            }
        }
        Expr::BinaryOp { left, right, .. } => {
            find_inner_aggregate(left).or_else(|| find_inner_aggregate(right))
        }
        Expr::UnaryOp { expr: inner, .. } => find_inner_aggregate(inner),
        Expr::Property(inner, _) => find_inner_aggregate(inner),
        Expr::MapLiteral(pairs) => pairs.iter().find_map(|(_, v)| find_inner_aggregate(v)),
        Expr::ListComp { list, map, .. } => {
            find_inner_aggregate(list).or_else(|| find_inner_aggregate(map))
        }
        _ => None,
    }
}

/// Rewrite an expression by replacing the first aggregate function call with
/// a variable reference to `alias`.
pub(crate) fn rewrite_agg_to_var(expr: &Expr, alias: &Identifier) -> Expr {
    match expr {
        Expr::FunctionCall { name, .. } if is_agg_name(name) => {
            // This IS the aggregate: replace the whole call with a variable.
            Expr::Variable(alias.clone())
        }
        Expr::FunctionCall {
            name,
            args,
            distinct,
        } => {
            // Non-aggregate function: rewrite inside args if any contain an aggregate.
            if args.iter().any(has_agg_function) {
                let new_args = args
                    .iter()
                    .map(|a| {
                        if has_agg_function(a) {
                            rewrite_agg_to_var(a, alias)
                        } else {
                            a.clone()
                        }
                    })
                    .collect();
                Expr::FunctionCall {
                    name: name.clone(),
                    args: new_args,
                    distinct: *distinct,
                }
            } else {
                expr.clone()
            }
        }
        Expr::BinaryOp { op, left, right } => {
            if has_agg_function(left) {
                Expr::BinaryOp {
                    op: *op,
                    left: Box::new(rewrite_agg_to_var(left, alias)),
                    right: right.clone(),
                }
            } else if has_agg_function(right) {
                Expr::BinaryOp {
                    op: *op,
                    left: left.clone(),
                    right: Box::new(rewrite_agg_to_var(right, alias)),
                }
            } else {
                expr.clone()
            }
        }
        Expr::UnaryOp { op, expr: inner } => Expr::UnaryOp {
            op: *op,
            expr: Box::new(rewrite_agg_to_var(inner, alias)),
        },
        Expr::MapLiteral(pairs) => {
            // Rewrite inside map values
            let new_pairs = pairs
                .iter()
                .map(|(k, v)| {
                    if has_agg_function(v) {
                        (k.clone(), rewrite_agg_to_var(v, alias))
                    } else {
                        (k.clone(), v.clone())
                    }
                })
                .collect();
            Expr::MapLiteral(new_pairs)
        }
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => {
            // Rewrite inside list if it contains an aggregate
            Expr::ListComp {
                variable: variable.clone(),
                list: if has_agg_function(list) {
                    Box::new(rewrite_agg_to_var(list, alias))
                } else {
                    list.clone()
                },
                filter: filter.clone(),
                map: if has_agg_function(map) {
                    Box::new(rewrite_agg_to_var(map, alias))
                } else {
                    map.clone()
                },
            }
        }
        _ => expr.clone(),
    }
}

/// Build a filter expression that checks `alias.prop = val` for each property
/// in an inline map literal `{prop: val, ...}`.
/// Convert an `Option<&Properties>` to a `Vec<(String, Expr)>` for the planner operator layer.
///
/// For `Properties::Map`, converts inline property pairs.
/// For `Properties::Parameter`, emits a single sentinel entry with an empty key
/// and an `Expr::Parameter` value. The executor detects this sentinel and expands
/// the parameter map at runtime.
pub(crate) fn properties_to_prop_pairs(props: Option<&Properties>) -> Vec<(String, Expr)> {
    match props {
        Some(Properties::Map(map)) => map
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        Some(Properties::Parameter(name)) => {
            // Sentinel: empty key signals "expand this parameter map at runtime".
            vec![("".to_string(), Expr::Parameter(name.to_string()))]
        }
        None => Vec::new(),
    }
}

/// Build a filter expression that checks whether an entity's properties match
/// all key-value pairs from a parameter map.
///
/// Generates `propertiesMatch(alias, $param_name)` which the executor evaluates
/// by comparing all entries in the resolved parameter map against the entity's
/// properties.
pub(crate) fn param_props_filter_expr(alias: &Identifier, param_name: &smol_str::SmolStr) -> Expr {
    Expr::FunctionCall {
        name: "propertiesMatch".into(),
        args: vec![
            Expr::Variable(alias.clone()),
            Expr::Parameter(param_name.to_string()),
        ],
        distinct: false,
    }
}

pub(crate) fn props_to_filter_expr(
    alias: &Identifier,
    props: &[(smol_str::SmolStr, Expr)],
) -> Expr {
    let mut parts: Vec<Expr> = props
        .iter()
        .map(|(key, val)| Expr::BinaryOp {
            op: crate::ast::BinaryOp::Eq,
            left: Box::new(Expr::Property(
                Box::new(Expr::Variable(alias.clone())),
                key.clone(),
            )),
            right: Box::new(val.clone()),
        })
        .collect();

    if parts.len() == 1 {
        parts.remove(0)
    } else {
        parts
            .into_iter()
            .reduce(|acc, e| Expr::BinaryOp {
                op: crate::ast::BinaryOp::And,
                left: Box::new(acc),
                right: Box::new(e),
            })
            .unwrap_or(Expr::Bool(true))
    }
}

/// Build a filter expression that checks whether a node has a specific label.
///
/// In Phase 2, label membership is enforced at the `NodeScan` level.  After an
/// `Expand`, the executor does not re-check labels automatically.  We synthesise
/// a `HAS LABEL` predicate using `FunctionCall { name: "hasLabel", ... }` which
/// `eval_expr` understands (see filter.rs).
pub(crate) fn label_filter_expr(alias: &Identifier, label: &Label) -> Expr {
    // Use hasLabel() built-in which is evaluated by eval_expr in filter.rs.
    Expr::FunctionCall {
        name: "hasLabel".into(),
        args: vec![
            Expr::Variable(alias.clone()),
            Expr::Str(label.as_str().into()),
        ],
        distinct: false,
    }
}

/// Derive a column name from an expression (mirrors executor's `expr_to_column_name`).
fn binop_col_prec(op: &crate::ast::BinaryOp) -> i32 {
    use crate::ast::BinaryOp;
    match op {
        BinaryOp::Pow => 6,
        BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => 5,
        BinaryOp::Add | BinaryOp::Sub | BinaryOp::Concat => 4,
        _ => 3,
    }
}

fn binop_col_nonassoc(op: &crate::ast::BinaryOp) -> bool {
    use crate::ast::BinaryOp;
    matches!(op, BinaryOp::Sub | BinaryOp::Div | BinaryOp::Mod)
}

pub(crate) fn expr_to_col_name(expr: &Expr) -> String {
    match expr {
        Expr::Variable(id) => id.0.clone(),
        Expr::Property(obj, key) => {
            let obj_str = match obj.as_ref() {
                Expr::Subscript(_, _) => format!("({})", expr_to_col_name(obj)),
                _ => expr_to_col_name(obj),
            };
            format!("{}.{}", obj_str, key)
        }
        Expr::FunctionCall {
            name,
            args,
            distinct,
            ..
        } => {
            let arg_strs: Vec<String> = args.iter().map(expr_to_col_name).collect();
            if *distinct {
                format!("{}(DISTINCT {})", name, arg_strs.join(", "))
            } else {
                format!("{}({})", name, arg_strs.join(", "))
            }
        }
        Expr::Integer(n) => n.to_string(),
        Expr::Float(f) => f.to_string(),
        Expr::Str(s) => format!("'{s}'"),
        Expr::BinaryOp { op, left, right } => {
            let op_str = match op {
                crate::ast::BinaryOp::Add => "+",
                crate::ast::BinaryOp::Sub => "-",
                crate::ast::BinaryOp::Mul => "*",
                crate::ast::BinaryOp::Div => "/",
                crate::ast::BinaryOp::Mod => "%",
                crate::ast::BinaryOp::Eq => "=",
                crate::ast::BinaryOp::Neq => "<>",
                crate::ast::BinaryOp::Lt => "<",
                crate::ast::BinaryOp::Lte => "<=",
                crate::ast::BinaryOp::Gt => ">",
                crate::ast::BinaryOp::Gte => ">=",
                crate::ast::BinaryOp::And => "AND",
                crate::ast::BinaryOp::Or => "OR",
                crate::ast::BinaryOp::Xor => "XOR",
                crate::ast::BinaryOp::In => "IN",
                crate::ast::BinaryOp::Contains => "CONTAINS",
                crate::ast::BinaryOp::StartsWith => "STARTS WITH",
                crate::ast::BinaryOp::EndsWith => "ENDS WITH",
                crate::ast::BinaryOp::Pow => "^",
                crate::ast::BinaryOp::RegexMatch => "=~",
                crate::ast::BinaryOp::Concat => "++",
            };
            let parent_prec = binop_col_prec(op);
            let left_str = match left.as_ref() {
                Expr::BinaryOp { op: child_op, .. } if binop_col_prec(child_op) < parent_prec => {
                    format!("({})", expr_to_col_name(left))
                }
                _ => expr_to_col_name(left),
            };
            let right_str = match right.as_ref() {
                Expr::BinaryOp { op: child_op, .. }
                    if binop_col_prec(child_op) < parent_prec
                        || (binop_col_prec(child_op) == parent_prec && binop_col_nonassoc(op)) =>
                {
                    format!("({})", expr_to_col_name(right))
                }
                _ => expr_to_col_name(right),
            };
            format!("{} {} {}", left_str, op_str, right_str)
        }
        Expr::Bool(b) => b.to_string(),
        Expr::Null => "null".to_string(),
        Expr::HasLabel(e, label) => format!("{}:{}", expr_to_col_name(e), label.0),
        Expr::UnaryOp { op, expr: e } => match op {
            crate::ast::UnaryOp::Not => format!("NOT {}", expr_to_col_name(e)),
            crate::ast::UnaryOp::Neg => format!("-{}", expr_to_col_name(e)),
        },
        Expr::IsNull(e) => format!("{} IS NULL", expr_to_col_name(e)),
        Expr::IsNotNull(e) => format!("{} IS NOT NULL", expr_to_col_name(e)),
        Expr::MapLiteral(pairs) => {
            let pairs_str: Vec<String> = pairs
                .iter()
                .map(|(k, v)| format!("{}: {}", k, expr_to_col_name_quoted(v)))
                .collect();
            format!("{{{}}}", pairs_str.join(", "))
        }
        Expr::List(items) => {
            let items_str: Vec<String> = items.iter().map(expr_to_col_name).collect();
            format!("[{}]", items_str.join(", "))
        }
        Expr::Subscript(list, idx) => {
            format!("{}[{}]", expr_to_col_name(list), expr_to_col_name(idx))
        }
        Expr::Parameter(name) => format!("${name}"),
        Expr::ListComp {
            variable,
            list,
            filter,
            map,
        } => {
            let list_str = expr_to_col_name(list);
            let var_str = &variable.0;
            if let Some(f) = filter {
                let filter_str = expr_to_col_name(f);
                if **map != Expr::Variable(variable.clone()) {
                    format!(
                        "[{} IN {} WHERE {} | {}]",
                        var_str,
                        list_str,
                        filter_str,
                        expr_to_col_name(map)
                    )
                } else {
                    format!("[{} IN {} WHERE {}]", var_str, list_str, filter_str)
                }
            } else {
                format!("[{} IN {}]", var_str, list_str)
            }
        }
        _ => format!("{:?}", expr),
    }
}

/// Like `expr_to_col_name` but quotes string literals (for map literal values).
fn expr_to_col_name_quoted(expr: &Expr) -> String {
    match expr {
        Expr::Str(s) => format!("'{s}'"),
        _ => expr_to_col_name(expr),
    }
}

// ── Helper functions ─────────────────────────────────────────────────────────

/// Convert a literal expression to a Value for type checking.
pub(crate) fn expr_to_literal_value(expr: &Expr) -> Option<Value> {
    match expr {
        Expr::Integer(n) => Some(Value::Int(*n)),
        Expr::Float(f) => Some(Value::Float(*f)),
        Expr::Str(s) => Some(Value::Str(s.clone())),
        Expr::Bool(b) => Some(Value::Bool(*b)),
        Expr::Null => Some(Value::Null),
        _ => None, // Non-literal expressions can't be type-checked at plan time
    }
}

/// Check if an expression contains an aggregation function.
pub(crate) fn contains_aggregation(expr: &Expr) -> bool {
    match expr {
        Expr::FunctionCall { name, args, .. } => {
            if crate::planner::is_aggregate_function(name.as_str()) {
                return true;
            }
            args.iter().any(contains_aggregation)
        }
        Expr::BinaryOp { left, right, .. } => {
            contains_aggregation(left) || contains_aggregation(right)
        }
        Expr::UnaryOp { expr: inner, .. } => contains_aggregation(inner),
        _ => false,
    }
}
