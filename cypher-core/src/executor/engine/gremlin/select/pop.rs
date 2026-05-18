//! Pop modulator branch: resolve label values when `Pop` is set
//! (`first`/`last`/`all`/`mixed`) and the label has a stack of aliases.

use crate::ast::Identifier;
use crate::error::GqlError;
use crate::planner::{GremlinPop, GremlinSelectTraversal};
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

use super::SelectBranch;

impl<'a> QueryExecutor<'a> {
    /// Try to resolve a select key via the Pop-modulator branch.
    /// Returns `NotApplicable` if `pop` is unset or no stack exists.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn select_try_pop(
        &self,
        cx: &mut ExecContext<'_>,
        row: &Row,
        src_var: &Identifier,
        output_key: &str,
        var_name: &str,
        by_traversal: Option<&GremlinSelectTraversal>,
        pop: &Option<GremlinPop>,
        label_stacks: &[(String, Vec<Identifier>)],
        productive: bool,
    ) -> Result<SelectBranch, GqlError> {
        let Some(pop_mode) = pop else {
            return Ok(SelectBranch::NotApplicable);
        };

        let stack = label_stacks
            .iter()
            .find(|(k, _)| k == output_key)
            .map(|(_, v)| v.as_slice())
            .unwrap_or(&[]);
        if stack.is_empty() {
            return Ok(SelectBranch::NotApplicable);
        }

        // Repeat() uses synthetic pop_orig_* aliases to preserve
        // the original binding for Pop.first when a label is
        // rebound inside the loop body. Those shadow aliases
        // must not leak into Pop.all/last/mixed materialization.
        let mut vals: Vec<Value> = Vec::new();
        for alias in stack.iter().filter(|alias| {
            matches!(pop_mode, GremlinPop::First) || !alias.0.as_str().starts_with("pop_orig_")
        }) {
            if let Some(v) = row.get(alias.0.as_str()) {
                vals.push(v.clone());
            }
        }
        if vals.is_empty() {
            return Ok(SelectBranch::Break);
        }
        let resolved = match pop_mode {
            GremlinPop::First => vals.into_iter().next().unwrap(),
            GremlinPop::Last => vals.into_iter().last().unwrap(),
            GremlinPop::All => Value::List(vals),
            GremlinPop::Mixed => {
                if vals.len() == 1 {
                    vals.into_iter().next().unwrap()
                } else {
                    Value::List(vals)
                }
            }
        };
        // Apply by-modulation: if src_var has a __prop_ prefix,
        // extract the property from the resolved value.
        let resolved = if let Some(rest) = var_name.strip_prefix("__prop_") {
            if let Some((prop_name, _real_var)) = self.split_prop_accessor(rest) {
                if let Some(n_str) = prop_name.strip_prefix("~limit_local:") {
                    let n = n_str.parse::<usize>().unwrap_or(0);
                    match resolved {
                        Value::List(items) => Value::List(items.into_iter().take(n).collect()),
                        other => other,
                    }
                } else if let Some(n_str) = prop_name.strip_prefix("~tail_local:") {
                    let n = n_str.parse::<usize>().unwrap_or(0);
                    match resolved {
                        Value::List(items) => {
                            let skip = items.len().saturating_sub(n);
                            Value::List(items.into_iter().skip(skip).collect())
                        }
                        other => other,
                    }
                } else {
                    self.resolve_prop_on_value(cx, prop_name, &resolved)?
                }
            } else {
                resolved
            }
        } else {
            resolved
        };
        let resolved = if let Some(traversal) = by_traversal {
            self.apply_select_by_traversal(cx, row, src_var, resolved, traversal)?
        } else {
            resolved
        };
        let empty_local = matches!(&resolved, Value::List(items) if items.is_empty())
            || matches!(&resolved, Value::Map(map) if map.is_empty());
        if !productive && empty_local {
            return Ok(SelectBranch::Break);
        }
        Ok(SelectBranch::Insert(resolved))
    }
}
