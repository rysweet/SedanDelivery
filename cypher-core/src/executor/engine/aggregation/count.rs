//! `count(...)` accumulator and finalizer.

use crate::types::Value;

use super::state::AggState;

pub(super) fn accumulate(state: &mut AggState, val: Value) {
    if !val.is_null() {
        state.count += 1;
    }
}

pub(super) fn finalize(state: &AggState) -> Value {
    Value::Int(state.count)
}
