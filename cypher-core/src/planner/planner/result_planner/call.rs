//! CALL clause planning.

use crate::ast::{CallClause, Clause, Expr, Identifier, Query};
use crate::error::{ExecutionError, GqlError, PlanError};
use crate::planner::Operator;

use super::super::helpers::{contains_aggregation, expr_to_literal_value};
use super::super::QueryPlanner;

impl<'s> QueryPlanner<'s> {
    // ── CALL planning ─────────────────────────────────────────────────────────

    pub(crate) fn plan_call(
        &self,
        cc: &CallClause,
        child: Option<Operator>,
        clause_idx: usize,
        query: &Query,
    ) -> Result<Operator, GqlError> {
        let has_return = query
            .clauses
            .iter()
            .skip(clause_idx + 1)
            .any(|c| matches!(c, Clause::Return(_)));
        let is_in_query = has_return || child.is_some();

        // YIELD * in in-query mode is an error.
        if cc.yield_star && is_in_query {
            return Err(GqlError::Execution(ExecutionError::unsupported(
                "YIELD * in in-query CALL".to_string(),
            )));
        }

        // Check if this is a built-in procedure handled dynamically by the executor.
        let is_builtin = crate::procedure::is_builtin_procedure(&cc.procedure);

        // Look up the procedure (skip registry lookup for built-ins).
        let proc_def;
        let builtin_def;
        let proc_ref: Option<&crate::procedure::ProcedureDefinition> = if is_builtin {
            // Built-in procedures have no input params; create a temporary definition
            // for validation purposes only. The executor handles them dynamically.
            let output_columns =
                crate::procedure::builtin_output_columns(&cc.procedure).unwrap_or_default();
            builtin_def = crate::procedure::ProcedureDefinition {
                name: cc.procedure.clone(),
                input_params: vec![],
                output_columns,
                data: vec![],
            };
            Some(&builtin_def)
        } else {
            match self.registry.get(&cc.procedure) {
                Some(p) => {
                    proc_def = p;
                    Some(proc_def)
                }
                None => {
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "ProcedureNotFound: {}",
                        cc.procedure
                    ))));
                }
            }
        };
        let proc_ref = proc_ref.unwrap();

        // Implicit arguments (no parens) are not allowed in in-query CALL with YIELD.
        if !cc.explicit_args && !cc.yield_items.is_empty() {
            return Err(GqlError::Execution(ExecutionError::unsupported(
                "InvalidArgumentPassingMode: implicit arguments not allowed in in-query CALL"
                    .to_string(),
            )));
        }

        // Resolve arguments.
        let args = if cc.explicit_args {
            // Explicit arguments: validate count.
            if cc.args.len() != proc_ref.input_params.len() {
                return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                    "InvalidNumberOfArguments: expected {}, got {}",
                    proc_ref.input_params.len(),
                    cc.args.len()
                ))));
            }
            // Basic type checking for literal arguments.
            for (i, arg) in cc.args.iter().enumerate() {
                if let Some(val) = expr_to_literal_value(arg) {
                    if !proc_ref.check_arg_type(i, &val) {
                        return Err(GqlError::Execution(ExecutionError::unsupported(
                            "InvalidArgumentType".to_string(),
                        )));
                    }
                }
            }
            cc.args.clone()
        } else {
            // Implicit arguments from query parameters.
            let mut args = Vec::new();
            for param in &proc_ref.input_params {
                if self.parameters.contains_key(&param.name) {
                    args.push(Expr::Parameter(param.name.clone()));
                } else {
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "MissingParameter: {}",
                        param.name
                    ))));
                }
            }
            args
        };

        // In-query CALL without YIELD: output columns are not exposed.
        // Check if subsequent RETURN references procedure output columns -> UndefinedVariable.
        if is_in_query && cc.yield_items.is_empty() && !cc.yield_star && cc.explicit_args {
            let output_names: std::collections::HashSet<&str> = proc_ref
                .output_columns
                .iter()
                .map(|c| c.name.as_str())
                .collect();
            if !output_names.is_empty() {
                for future_clause in query.clauses.iter().skip(clause_idx + 1) {
                    if let Clause::Return(rc) = future_clause {
                        for item in &rc.items {
                            if let Expr::Variable(Identifier(name)) = &item.expr {
                                if output_names.contains(name.as_str()) {
                                    return Err(GqlError::Plan(PlanError::UndefinedVariable {
                                        name: name.clone(),
                                        offset: 0,
                                    }));
                                }
                            }
                        }
                    }
                }
            }
        }

        // Check YIELD variable shadowing.
        if !cc.yield_items.is_empty() {
            let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
            // Collect variables already in scope from the pipeline.
            if let Some(ref ch) = child {
                for alias in crate::planner::collect_visible_aliases(ch) {
                    seen.insert(alias);
                }
            }
            for (name, alias) in &cc.yield_items {
                let effective_name = alias.as_ref().unwrap_or(name).0.clone();
                if !seen.insert(effective_name.clone()) {
                    return Err(GqlError::Execution(ExecutionError::unsupported(format!(
                        "VariableAlreadyBound: {}",
                        effective_name
                    ))));
                }
            }
        }

        // Check for aggregation in arguments (not allowed).
        for arg in &args {
            if contains_aggregation(arg) {
                return Err(GqlError::Execution(ExecutionError::unsupported(
                    "InvalidAggregation: cannot use aggregation in CALL arguments".to_string(),
                )));
            }
        }

        Ok(Operator::Call {
            procedure: cc.procedure.clone(),
            args,
            yield_items: cc.yield_items.clone(),
            yield_star: cc.yield_star,
            standalone: !has_return,
            child: child.map(Box::new),
        })
    }

    /// Derive ProduceResults columns for standalone CALL (no RETURN).
    pub(crate) fn derive_standalone_call_columns(&self, op: &Operator) -> Vec<(String, Expr)> {
        match op {
            Operator::Call {
                procedure,
                yield_items,
                yield_star: _,
                ..
            } => {
                if !yield_items.is_empty() {
                    // YIELD specific columns.
                    yield_items
                        .iter()
                        .map(|(name, alias)| {
                            let col = alias.as_ref().unwrap_or(name);
                            (col.0.clone(), Expr::Variable(col.clone()))
                        })
                        .collect()
                } else if let Some(proc_def) = self.registry.get(procedure) {
                    // Standalone or YIELD * -> output all columns from registry.
                    proc_def
                        .output_columns
                        .iter()
                        .map(|col| {
                            (
                                col.name.clone(),
                                Expr::Variable(Identifier::new(col.name.clone())),
                            )
                        })
                        .collect()
                } else if let Some(cols) = crate::procedure::builtin_output_columns(procedure) {
                    // Built-in procedure: derive columns from static metadata.
                    cols.iter()
                        .map(|col| {
                            (
                                col.name.clone(),
                                Expr::Variable(Identifier::new(col.name.clone())),
                            )
                        })
                        .collect()
                } else {
                    vec![]
                }
            }
            _ => vec![], // Non-CALL: empty columns (same as before for CREATE/DELETE/etc.)
        }
    }
}
