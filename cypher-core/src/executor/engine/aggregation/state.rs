//! `AggState`: shared accumulator state plus the dispatch into per-family
//! accumulator and finalizer helpers.

use crate::error::GqlError;
use crate::planner::AggFunction;
use crate::types::Value;

use super::distinct::DistinctSet;
use super::{collect, count, min_max, percentile_stddev, sum_avg};

#[derive(Debug, Default)]
pub(crate) struct AggState {
    pub(super) count: i64,
    pub(super) sum: Option<f64>,
    pub(super) min: Option<Value>,
    pub(super) max: Option<Value>,
    pub(super) collected: Vec<Value>,
    pub(super) seen_distinct: DistinctSet,
    /// Percentile parameter for percentileDisc/percentileCont (0.0..=1.0).
    pub(super) percentile: Option<f64>,
}

impl AggState {
    pub(crate) fn accumulate(
        &mut self,
        func: &AggFunction,
        val: Value,
        second_val: Option<Value>,
        distinct: bool,
    ) {
        // For distinct aggregations, skip duplicates.
        // Cap the set size to prevent OOM via hostile queries with millions
        // of distinct values — consistent with max_collect_size guard on Collect.
        if distinct {
            let max_size = crate::config::global().max_collect_size;
            if self.seen_distinct.len() >= max_size {
                return; // silently drop excess — same as Collect overflow
            }
            if !self.seen_distinct.insert(&val) {
                return;
            }
        }

        match func {
            AggFunction::Count => count::accumulate(self, val),
            AggFunction::Sum => sum_avg::accumulate_sum(self, val),
            AggFunction::Avg => sum_avg::accumulate_avg(self, val),
            AggFunction::Min => min_max::accumulate_min(self, val),
            AggFunction::Max => min_max::accumulate_max(self, val),
            AggFunction::Collect => collect::accumulate_collect(self, val),
            AggFunction::CollectAll => collect::accumulate_collect_all(self, val),
            AggFunction::StDev | AggFunction::StDevP => {
                percentile_stddev::accumulate_numeric(self, val);
            }
            AggFunction::PercentileDisc | AggFunction::PercentileCont => {
                percentile_stddev::accumulate_percentile(self, val, second_val);
            }
        }
    }

    pub(crate) fn finalize(&self, func: &AggFunction) -> Result<Value, GqlError> {
        match func {
            AggFunction::Count => Ok(count::finalize(self)),
            AggFunction::Sum => Ok(sum_avg::finalize_sum(self)),
            AggFunction::Avg => Ok(sum_avg::finalize_avg(self)),
            AggFunction::Min => Ok(min_max::finalize_min(self)),
            AggFunction::Max => Ok(min_max::finalize_max(self)),
            AggFunction::Collect | AggFunction::CollectAll => Ok(collect::finalize(self)),
            AggFunction::StDev => Ok(percentile_stddev::finalize_stddev(self)),
            AggFunction::StDevP => Ok(percentile_stddev::finalize_stddev_p(self)),
            AggFunction::PercentileDisc => percentile_stddev::finalize_percentile_disc(self),
            AggFunction::PercentileCont => percentile_stddev::finalize_percentile_cont(self),
        }
    }
}
