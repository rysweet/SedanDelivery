//! `GremlinSelect` — project labeled step values, skipping rows
//! where any referenced label is not bound in the row.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::helpers::inject_parameters;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

use super::SelectBranch;

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinSelect` — project labeled step values, skipping rows
    /// where any referenced label is not bound in the row.
    pub(crate) fn exec_gremlin_select(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinSelect {
            keys,
            by_traversals,
            productive,
            pop,
            label_stacks,
            child,
        } = op
        else {
            debug_assert!(false, "expected GremlinSelect operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinSelect operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::new();

        for row in &input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, row)?;

            // Check that ALL referenced variables are present in the row.
            let mut all_found = true;
            let mut new_row = Row::new();

            for (idx, (src_var, output_key)) in keys.iter().enumerate() {
                let var_name = src_var.0.as_str();
                let by_traversal = by_traversals.get(idx).and_then(|entry| entry.as_ref());

                // Branch 1: Pop modulator.
                match self.select_try_pop(
                    cx,
                    row,
                    src_var,
                    output_key,
                    var_name,
                    by_traversal,
                    pop,
                    label_stacks,
                    *productive,
                )? {
                    SelectBranch::Insert(v) => {
                        new_row.insert(output_key.clone(), v);
                        continue;
                    }
                    SelectBranch::Break => {
                        all_found = false;
                        break;
                    }
                    SelectBranch::NotApplicable => {}
                }

                // Branch 2: __prop_PROPNAME__VARNAME accessor.
                match self.select_try_prop_accessor(
                    cx,
                    &ctx,
                    row,
                    src_var,
                    output_key,
                    var_name,
                    by_traversal,
                    *productive,
                )? {
                    SelectBranch::Insert(v) => {
                        new_row.insert(output_key.clone(), v);
                        continue;
                    }
                    SelectBranch::Break => {
                        all_found = false;
                        break;
                    }
                    SelectBranch::NotApplicable => {}
                }

                // Branch 3: direct row / side-effect / expression lookup.
                let val = self.select_lookup_value(&ctx, row, src_var, output_key, var_name);

                match val {
                    Some(v) => {
                        // Select-from-Map: when the resolved value is a Map
                        // containing the output key, extract the Map entry.
                        // This implements Gremlin's behavior where select(key)
                        // on a Map traverser acts as a Map key selector.
                        let v = if keys.len() == 1 {
                            if let Value::Map(ref m) = v {
                                m.get(output_key.as_str()).cloned().unwrap_or(v)
                            } else {
                                v
                            }
                        } else {
                            v
                        };
                        let v = if let Some(traversal) = by_traversal {
                            self.apply_select_by_traversal(cx, row, src_var, v, traversal)?
                        } else {
                            v
                        };
                        new_row.insert(output_key.clone(), v);
                    }
                    None => {
                        all_found = false;
                        break;
                    }
                }
            }

            if all_found {
                if keys.len() > 1 {
                    out.push(build_multi_key_row(self, cx, row, keys, &new_row));
                } else {
                    out.push(build_single_key_row(self, cx, row, keys, &new_row));
                }
            }
        }

        Ok(out)
    }
}

/// Build a Map-valued result row for multi-key select.
fn build_multi_key_row(
    exec: &QueryExecutor<'_>,
    cx: &mut ExecContext<'_>,
    row: &Row,
    keys: &[(crate::ast::Identifier, String)],
    new_row: &Row,
) -> Row {
    let mut map: indexmap::IndexMap<Box<str>, Value> = indexmap::IndexMap::new();
    for (_, output_key) in keys {
        if let Some(v) = new_row.get(output_key.as_str()) {
            map.insert(output_key.clone().into(), v.clone());
        }
    }
    let mut map_row = Row::new();
    inject_parameters(&mut map_row, &exec.parameters);
    // Carry forward Gremlin internal columns (path
    // accumulator, sack, side-effects) so downstream steps
    // can access them after multi-key select.
    for (k, v) in row.iter() {
        if k.starts_with("__gremlin_") || k == "__sack" {
            map_row.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    // Add individual key columns AND their internal alias
    // columns so downstream where/filter can reference them.
    for (src_var, output_key) in keys {
        if let Some(v) = new_row.get(output_key.as_str()) {
            map_row.insert(output_key.clone(), v.clone());
            map_row.insert(src_var.0.to_string(), v.clone());
        }
    }
    let map_alias = keys
        .iter()
        .map(|(_, k)| k.as_str())
        .collect::<Vec<_>>()
        .join("_");
    map_row.insert(map_alias, Value::Map(map));
    cx.budget.record_row();
    map_row
}

/// Build a single-key result row, carrying forward existing columns.
fn build_single_key_row(
    exec: &QueryExecutor<'_>,
    cx: &mut ExecContext<'_>,
    row: &Row,
    keys: &[(crate::ast::Identifier, String)],
    new_row: &Row,
) -> Row {
    // Single-key select: carry forward existing row columns
    // so that as() labels remain accessible to downstream steps
    // (needed for addE/from/to resolution after select/unfold).
    let mut out_row = row.clone();
    for (src_var, output_key) in keys {
        if let Some(v) = new_row.get(output_key.as_str()) {
            out_row.insert(output_key.clone(), v.clone());
            // Always update the compiler source alias so
            // downstream steps operate on the selected value.
            out_row.insert(src_var.0.to_string(), v.clone());
        }
    }
    inject_parameters(&mut out_row, &exec.parameters);
    cx.budget.record_row();
    out_row
}
