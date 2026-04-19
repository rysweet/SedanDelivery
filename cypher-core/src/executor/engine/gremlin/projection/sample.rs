//! `GremlinSample` — barrier sampling using weighted roulette semantics.

use crate::error::{ExecutionError, GqlError};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::planner::Operator;
use crate::types::Value;

#[allow(unused_imports)]
use super::super::helpers::*;

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinSample` — barrier sampling of the traverser stream using
    /// Gremlin's weighted roulette semantics.
    pub(crate) fn exec_gremlin_sample(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinSample {
            count,
            weight,
            seed,
            child,
        } = op
        else {
            debug_assert!(false, "expected GremlinSample operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinSample operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut weighted_rows = Vec::with_capacity(input.len());
        for row in input {
            cx.budget.check_or_err()?;
            let row_weight = if let Some(weight_expr) = weight {
                let filter_ctx = self.build_filter_ctx(cx.graph, &row)?;
                match self.eval_with_storage(cx.graph, weight_expr, &row, &filter_ctx)? {
                    Value::Int(n) => n as f64,
                    Value::Float(f) => f,
                    Value::Null => continue,
                    other => {
                        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "sample().by() numeric weight".into(),
                            actual: other.type_name().to_string(),
                        }))
                    }
                }
            } else {
                1.0
            };
            if row_weight.is_sign_negative() || !row_weight.is_finite() {
                continue;
            }
            weighted_rows.push((row, row_weight));
        }

        if weighted_rows.len() <= *count {
            return Ok(weighted_rows.into_iter().map(|(row, _)| row).collect());
        }

        let mut total_weight: f64 = weighted_rows.iter().map(|(_, weight)| *weight).sum();
        let seed = seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as i64
        });
        let mut rng = JavaCompatibleRandom::new(seed);
        let mut sampled = Vec::with_capacity(*count);

        while sampled.len() < *count && !weighted_rows.is_empty() {
            let mut running_total_weight = total_weight;
            let mut selected = None;
            for (idx, (_, row_weight)) in weighted_rows.iter().enumerate() {
                cx.budget.check_or_err()?;
                let draw = rng.next_double();
                if draw <= (*row_weight / running_total_weight.max(f64::MIN_POSITIVE)) {
                    selected = Some(idx);
                    break;
                }
                running_total_weight -= *row_weight;
            }
            if let Some(idx) = selected {
                let (row, row_weight) = weighted_rows.remove(idx);
                total_weight -= row_weight;
                sampled.push(row);
            } else {
                break;
            }
        }

        Ok(sampled)
    }
}
