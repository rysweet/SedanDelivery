//! Direct row / side-effect / expression fallback lookup branch.

use crate::ast::{Expr, Identifier};
use crate::executor::filter::Row as FilterContext;
use crate::types::Value;

use crate::executor::engine::{QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    /// Resolve a select key by checking the row, aggregate side-effects,
    /// nested Map values, and finally a generic expression evaluation.
    /// Returns `None` only when no fallback can produce a value.
    pub(super) fn select_lookup_value(
        &self,
        ctx: &FilterContext<'_>,
        row: &Row,
        src_var: &Identifier,
        output_key: &str,
        var_name: &str,
    ) -> Option<Value> {
        if let Some(v) = self.resolve_map_select_value(row, var_name) {
            return Some(v);
        }
        if let Some(v) = row.get(var_name) {
            return Some(v.clone());
        }
        if let Some(v) = row.get(&format!("__gremlin_agg_{}", output_key)) {
            // Fallback: aggregate sideeffect columns are stored
            // with a __gremlin_agg_ prefix.
            return Some(v.clone());
        }
        if let Some(v) = self
            .gremlin_side_effects
            .borrow()
            .get(&format!("__gremlin_agg_{}", output_key))
            .cloned()
        {
            // Fallback: check the global side-effect store.
            // Inside local(), the seed row doesn't carry the
            // aggregate column, but the store accumulates it.
            return Some(v);
        }
        // For select() after group(): the row may contain a Map value.
        // Try looking up output_key as a map key in any Map values in the row.
        for (_k, v) in row.iter() {
            if let Value::Map(m) = v {
                if let Some(map_val) = m.get(output_key) {
                    return Some(map_val.clone());
                }
            }
        }
        // Last-resort variable resolution via expression eval.
        // .ok() is intentional: a missing variable here means all
        // prior lookup paths failed, which is a valid "not found"
        // in Gremlin select() semantics — not an error to propagate.
        let expr = Expr::Variable(src_var.clone());
        crate::executor::filter::eval_expr(&expr, ctx).ok()
    }
}
