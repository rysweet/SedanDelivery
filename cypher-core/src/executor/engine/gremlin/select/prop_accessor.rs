//! Property-accessor branch: resolve `__prop_PROPNAME__VARNAME` keys
//! produced by `select(...).by(prop)` modulators.

use crate::ast::{Expr, Identifier};
use crate::error::GqlError;
use crate::executor::filter::Row as FilterContext;
use crate::planner::GremlinSelectTraversal;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

use super::SelectBranch;

impl<'a> QueryExecutor<'a> {
    /// Try to resolve a select key via the `__prop_` accessor branch.
    /// Returns `NotApplicable` if `var_name` is not a `__prop_` key or
    /// the suffix cannot be split into `(prop_name, real_var)`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn select_try_prop_accessor(
        &self,
        cx: &mut ExecContext<'_>,
        ctx: &FilterContext<'_>,
        row: &Row,
        src_var: &Identifier,
        _output_key: &str,
        var_name: &str,
        by_traversal: Option<&GremlinSelectTraversal>,
        productive: bool,
    ) -> Result<SelectBranch, GqlError> {
        let Some(rest) = var_name.strip_prefix("__prop_") else {
            return Ok(SelectBranch::NotApplicable);
        };
        let Some((prop_name, real_var)) = self.split_prop_accessor(rest) else {
            return Ok(SelectBranch::NotApplicable);
        };

        let val = row.get(real_var);
        let selected = if let Some(v) = self.resolve_map_select_value(row, real_var) {
            Some(v)
        } else {
            val.cloned()
        };
        let Some(node_val) = selected else {
            return Ok(SelectBranch::Break);
        };

        // Handle special property names (~id, T.id, ~label, T.label)
        let resolved = if prop_name == "~id" || prop_name == "T.id" || prop_name == "t[id]" {
            match &node_val {
                Value::Node(nid) => Some(Value::Int(nid.0 as i64)),
                Value::Edge(eid) => Some(Value::Int(eid.0 as i64)),
                _ => None,
            }
        } else if prop_name == "~label" || prop_name == "T.label" || prop_name == "t[label]" {
            match &node_val {
                Value::Node(nid) => {
                    if let Ok(Some(node)) = self.store.get_node(cx.graph, *nid) {
                        node.labels
                            .into_iter()
                            .next()
                            .map(|l| Value::Str(l.into_boxed_str()))
                    } else {
                        None
                    }
                }
                Value::Edge(eid) => {
                    if let Ok(Some(edge)) = self.store.get_edge(cx.graph, *eid) {
                        Some(Value::Str(edge.rel_type.into_boxed_str()))
                    } else {
                        None
                    }
                }
                _ => None,
            }
        } else {
            None
        };

        if let Some(v) = resolved {
            let v = if let Some(traversal) = by_traversal {
                self.apply_select_by_traversal(cx, row, src_var, v, traversal)?
            } else {
                v
            };
            return Ok(SelectBranch::Insert(v));
        }

        // Handle tilde-prefixed transformation keys
        // (e.g., ~unfold_values_fold:name, ~toupper:prop)
        // via resolve_prop_on_value instead of raw property access.
        if prop_name.starts_with('~') {
            let r = self.resolve_prop_on_value(cx, prop_name, &node_val);
            let r = if let Some(traversal) = by_traversal {
                self.apply_select_by_traversal(cx, row, src_var, r, traversal)?
            } else {
                r
            };
            if !productive && matches!(r, Value::Null) {
                return Ok(SelectBranch::Break);
            }
            return Ok(SelectBranch::Insert(r));
        }

        // Resolve property from the node/edge.
        let prop_expr = Expr::Property(
            Box::new(Expr::Variable(Identifier::new(real_var))),
            smol_str::SmolStr::new(prop_name),
        );
        match self.eval_with_storage(cx.graph, &prop_expr, row, ctx) {
            Ok(v) => {
                let v = if let Some(traversal) = by_traversal {
                    self.apply_select_by_traversal(cx, row, src_var, v, traversal)?
                } else {
                    v
                };
                if !productive && matches!(v, Value::Null) {
                    return Ok(SelectBranch::Break);
                }
                Ok(SelectBranch::Insert(v))
            }
            Err(_) => {
                // Property doesn't exist
                if !productive {
                    // Non-productive: drop the traverser
                    if let Value::Map(m) = &node_val {
                        match m.get(prop_name).cloned() {
                            Some(val) => {
                                let val = if let Some(traversal) = by_traversal {
                                    self.apply_select_by_traversal(
                                        cx, row, src_var, val, traversal,
                                    )?
                                } else {
                                    val
                                };
                                Ok(SelectBranch::Insert(val))
                            }
                            None => Ok(SelectBranch::Break),
                        }
                    } else {
                        Ok(SelectBranch::Break)
                    }
                } else {
                    // Productive: use Null
                    if let Value::Map(m) = &node_val {
                        let v = m.get(prop_name).cloned().unwrap_or(Value::Null);
                        Ok(SelectBranch::Insert(v))
                    } else {
                        Ok(SelectBranch::Insert(Value::Null))
                    }
                }
            }
        }
    }
}
