//! `stDev`, `stDevP`, `percentileDisc` and `percentileCont` accumulators and
//! finalizers.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::state::AggState;

/// Push a numeric sample (used by `StDev` and `StDevP`).
pub(super) fn accumulate_numeric(state: &mut AggState, val: Value) {
    let max_collect = crate::config::global().max_collect_size;
    if !val.is_null() && state.collected.len() < max_collect {
        if let Some(f) = val.as_float() {
            state.collected.push(Value::Float(f));
        }
    }
}

/// Push a numeric sample plus capture the percentile parameter
/// (used by `PercentileDisc` and `PercentileCont`).
pub(super) fn accumulate_percentile(state: &mut AggState, val: Value, second_val: Option<Value>) {
    let max_collect = crate::config::global().max_collect_size;
    if !val.is_null() && state.collected.len() < max_collect {
        if let Some(f) = val.as_float() {
            state.collected.push(Value::Float(f));
        }
    }
    // Store the percentile parameter (constant across rows, first one wins).
    if state.percentile.is_none() {
        if let Some(pval) = &second_val {
            if let Some(f) = pval.as_float() {
                state.percentile = Some(f);
            }
        }
    }
}

pub(super) fn finalize_stddev(state: &AggState) -> Value {
    let nums: Vec<f64> = state
        .collected
        .iter()
        .filter_map(|v| v.as_float())
        .collect();
    if nums.len() <= 1 {
        return Value::Float(0.0);
    }
    let mean = nums.iter().sum::<f64>() / nums.len() as f64;
    let variance = nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (nums.len() - 1) as f64;
    Value::Float(variance.sqrt())
}

pub(super) fn finalize_stddev_p(state: &AggState) -> Value {
    let nums: Vec<f64> = state
        .collected
        .iter()
        .filter_map(|v| v.as_float())
        .collect();
    if nums.is_empty() {
        return Value::Float(0.0);
    }
    let mean = nums.iter().sum::<f64>() / nums.len() as f64;
    let variance = nums.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / nums.len() as f64;
    Value::Float(variance.sqrt())
}

pub(super) fn finalize_percentile_disc(state: &AggState) -> Result<Value, GqlError> {
    let p = state.percentile.unwrap_or(0.5);
    if !(0.0..=1.0).contains(&p) {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "percentile in range [0.0, 1.0]".to_string(),
            actual: format!("NumberOutOfRange: {p}"),
        }));
    }
    let mut nums: Vec<f64> = state
        .collected
        .iter()
        .filter_map(|v| v.as_float())
        .collect();
    if nums.is_empty() {
        return Ok(Value::Null);
    }
    nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Nearest rank method: index = ceil(p * n) - 1 (clamped)
    let n = nums.len();
    let idx = ((p * n as f64).ceil() as usize)
        .saturating_sub(1)
        .min(n - 1);
    // Return as integer if all values are whole numbers
    let v = nums[idx];
    if v.fract() == 0.0 && v >= i64::MIN as f64 && v <= i64::MAX as f64 {
        Ok(Value::Int(v as i64))
    } else {
        Ok(Value::Float(v))
    }
}

pub(super) fn finalize_percentile_cont(state: &AggState) -> Result<Value, GqlError> {
    let p = state.percentile.unwrap_or(0.5);
    if !(0.0..=1.0).contains(&p) {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "percentile in range [0.0, 1.0]".to_string(),
            actual: format!("NumberOutOfRange: {p}"),
        }));
    }
    let mut nums: Vec<f64> = state
        .collected
        .iter()
        .filter_map(|v| v.as_float())
        .collect();
    if nums.is_empty() {
        return Ok(Value::Null);
    }
    nums.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = p * (nums.len() - 1) as f64;
    let lo = idx.floor() as usize;
    let hi = (idx.ceil() as usize).min(nums.len() - 1);
    if lo == hi {
        Ok(Value::Float(nums[lo]))
    } else {
        let frac = idx - lo as f64;
        Ok(Value::Float(nums[lo] * (1.0 - frac) + nums[hi] * frac))
    }
}
