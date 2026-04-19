//! `min(...)` and `max(...)` accumulators and finalizers.

use crate::types::Value;

use super::super::helpers::cmp_values;
use super::state::AggState;

pub(super) fn accumulate_min(state: &mut AggState, val: Value) {
    if !val.is_null() {
        state.min = Some(match state.min.take() {
            None => val,
            Some(current) => {
                if cmp_values(&val, &current) == std::cmp::Ordering::Less {
                    val
                } else {
                    current
                }
            }
        });
    }
}

pub(super) fn accumulate_max(state: &mut AggState, val: Value) {
    if !val.is_null() {
        state.max = Some(match state.max.take() {
            None => val,
            Some(current) => {
                if cmp_values(&val, &current) == std::cmp::Ordering::Greater {
                    val
                } else {
                    current
                }
            }
        });
    }
}

pub(super) fn finalize_min(state: &AggState) -> Value {
    state.min.clone().unwrap_or(Value::Null)
}

pub(super) fn finalize_max(state: &AggState) -> Value {
    state.max.clone().unwrap_or(Value::Null)
}
