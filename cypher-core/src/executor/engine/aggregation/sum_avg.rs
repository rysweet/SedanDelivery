//! `sum(...)` and `avg(...)` accumulators and finalizers.

use crate::types::Value;

use super::state::AggState;

pub(super) fn accumulate_sum(state: &mut AggState, val: Value) {
    if let Some(f) = val.as_float() {
        state.sum = Some(state.sum.unwrap_or(0.0) + f);
    }
}

pub(super) fn accumulate_avg(state: &mut AggState, val: Value) {
    if let Some(f) = val.as_float() {
        state.sum = Some(state.sum.unwrap_or(0.0) + f);
        state.count += 1;
    }
}

pub(super) fn finalize_sum(state: &AggState) -> Value {
    state.sum.map(Value::Float).unwrap_or(Value::Int(0))
}

pub(super) fn finalize_avg(state: &AggState) -> Value {
    if state.count == 0 {
        Value::Null
    } else {
        Value::Float(state.sum.unwrap_or(0.0) / state.count as f64)
    }
}
