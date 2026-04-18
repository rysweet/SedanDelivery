//! Connective and existence-filter compilation helpers.
//!
//! Extracted from `mod.rs`: `compile_connective_branches` selects between
//! predicate-mode and traversal-mode (Apply+Count) compilation for And/Or
//! branches; `compile_existence_filter` wraps a child operator with a
//! has/hasNot existence check.

use cypher_core::ast::{BinaryOp, Expr, Identifier};
use cypher_core::planner::Operator;

use super::helpers::*;
use super::{CompileError, GremlinCompiler};
use crate::bytecode::GremlinStep;

impl GremlinCompiler {
    /// Compile And/Or branches, using predicate mode for simple branches and
    /// traversal (Apply+Count) mode for complex branches.
    pub(super) fn compile_connective_branches(
        &mut self,
        branches: &[Vec<GremlinStep>],
        current_alias: &Identifier,
        child: Operator,
        conn_op: BinaryOp,
    ) -> Result<Operator, CompileError> {
        let all_simple = branches
            .iter()
            .all(|branch| is_simple_predicate_steps(branch));

        if all_simple {
            let preds: Vec<Expr> = branches
                .iter()
                .map(|branch| {
                    steps_to_predicate(branch, current_alias, &self.alias_map, &self.aggregate_keys)
                })
                .collect();
            let default = if conn_op == BinaryOp::And {
                Expr::Bool(true)
            } else {
                Expr::Bool(false)
            };
            let predicate = preds
                .into_iter()
                .reduce(|a, b| Expr::BinaryOp {
                    op: conn_op,
                    left: Box::new(a),
                    right: Box::new(b),
                })
                .unwrap_or(default);
            return Ok(Operator::Filter {
                predicate,
                child: Box::new(child),
            });
        }

        // At least one branch has complex traversal steps.
        // For AND: chain filters (each branch must pass).
        // For OR: build existence count per branch and combine predicates.
        if conn_op == BinaryOp::And {
            let mut result = child;
            for branch in branches {
                if is_simple_predicate_steps(branch) {
                    let pred = steps_to_predicate(
                        branch,
                        current_alias,
                        &self.alias_map,
                        &self.aggregate_keys,
                    );
                    result = Operator::Filter {
                        predicate: pred,
                        child: Box::new(result),
                    };
                } else {
                    result = self.compile_existence_filter(branch, current_alias, result, false)?;
                }
            }
            Ok(result)
        } else {
            // OR: need to check if ANY branch produces results.
            let mut cnt_aliases = Vec::new();
            let mut result = child;
            for branch in branches {
                if is_simple_predicate_steps(branch) {
                    let pred = steps_to_predicate(
                        branch,
                        current_alias,
                        &self.alias_map,
                        &self.aggregate_keys,
                    );
                    let cnt_alias = self.next_alias("ocnt");
                    // Wrap in a project that produces 1 if pred is true, 0 if false.
                    cnt_aliases.push((cnt_alias, Some(pred)));
                } else {
                    let mut sub = self.sub_compiler();
                    let (actual_seed, steps_slice) = match branch.first() {
                        Some(GremlinStep::As(label)) if self.alias_map.contains_key(label) => {
                            (self.alias_map[label].clone(), &branch[1..])
                        }
                        _ => (current_alias.clone(), branch.as_slice()),
                    };
                    let inner_op = sub.compile_seeded(steps_slice, &actual_seed)?;
                    self.alias_counter = sub.alias_counter;
                    let sub_alias = sub.last_alias.clone();
                    let cnt_alias = self.next_alias("ocnt");
                    let count_op = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            cypher_core::planner::AggFunction::Count,
                            Expr::Variable(sub_alias),
                            None,
                            false,
                            cnt_alias.clone(),
                        )],
                        child: Box::new(inner_op),
                    };
                    result = Operator::Apply {
                        outer: Box::new(result),
                        inner: Box::new(count_op),
                    };
                    cnt_aliases.push((cnt_alias, None));
                }
            }

            // Build OR predicate: any count > 0 or any simple pred is true
            let or_pred = cnt_aliases
                .into_iter()
                .map(|(alias, simple_pred)| {
                    if let Some(pred) = simple_pred {
                        pred
                    } else {
                        Expr::BinaryOp {
                            op: BinaryOp::Gt,
                            left: Box::new(Expr::Variable(alias)),
                            right: Box::new(Expr::Integer(0)),
                        }
                    }
                })
                .reduce(|a, b| Expr::BinaryOp {
                    op: BinaryOp::Or,
                    left: Box::new(a),
                    right: Box::new(b),
                })
                .unwrap_or(Expr::Bool(false));

            Ok(Operator::Filter {
                predicate: or_pred,
                child: Box::new(result),
            })
        }
    }

    /// Compile a sub-traversal as an existence filter (Apply + Count + Filter).
    fn compile_existence_filter(
        &mut self,
        steps: &[GremlinStep],
        seed_alias: &Identifier,
        child: Operator,
        negate: bool,
    ) -> Result<Operator, CompileError> {
        let mut sub = self.sub_compiler();

        // If the sub-traversal starts with as("label") referencing an
        // already-bound variable, seed from that variable's alias.
        let (actual_seed, steps_slice) = match steps.first() {
            Some(GremlinStep::As(label)) if self.alias_map.contains_key(label) => {
                (self.alias_map[label].clone(), &steps[1..])
            }
            _ => (seed_alias.clone(), steps),
        };

        // If the sub-traversal ends with as("label") referencing an
        // already-bound variable, constrain the endpoint to match.
        let endpoint_target = if let Some(GremlinStep::As(label)) = steps_slice.last() {
            self.alias_map.get(label).cloned()
        } else {
            None
        };

        let inner_op = sub.compile_seeded(steps_slice, &actual_seed)?;
        self.absorb(&sub);
        let sub_alias = sub.last_alias.clone();

        if let Some(target_alias) = endpoint_target {
            // The traversal ends with as("label") where label is already bound.
            // Place the endpoint equality Filter AFTER the Apply so that both
            // the traversal result (sub_alias) and the bound value (target_alias)
            // are available in the same row.  Placing it inside the Aggregate
            // chain would lose target_alias (Aggregate creates a fresh row).
            let apply = Operator::Apply {
                outer: Box::new(child),
                inner: Box::new(inner_op),
            };
            let cmp_op = if negate { BinaryOp::Neq } else { BinaryOp::Eq };
            let predicate = Expr::BinaryOp {
                op: cmp_op,
                left: Box::new(Expr::Variable(sub_alias)),
                right: Box::new(Expr::Variable(target_alias)),
            };
            return Ok(Operator::Filter {
                predicate,
                child: Box::new(apply),
            });
        }

        let cnt_alias = self.next_alias("acnt");
        let count_op = Operator::Aggregate {
            keys: vec![],
            aggregations: vec![(
                cypher_core::planner::AggFunction::Count,
                Expr::Variable(sub_alias),
                None,
                false,
                cnt_alias.clone(),
            )],
            child: Box::new(inner_op),
        };
        let apply = Operator::Apply {
            outer: Box::new(child),
            inner: Box::new(count_op),
        };
        let predicate = if negate {
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                left: Box::new(Expr::Variable(cnt_alias)),
                right: Box::new(Expr::Integer(0)),
            }
        } else {
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(Expr::Variable(cnt_alias)),
                right: Box::new(Expr::Integer(0)),
            }
        };
        Ok(Operator::Filter {
            predicate,
            child: Box::new(apply),
        })
    }
}
