//! `collect(...)` and `collect_all(...)` accumulators and finalizer.
//!
//! `Collect` skips `NULL` values per openCypher semantics; `CollectAll` is the
//! Gremlin-style variant that preserves `NULL` entries.  Both honour the
//! `max_collect_size` config guard to bound memory use.

use crate::types::Value;

use super::state::AggState;

pub(super) fn accumulate_collect(state: &mut AggState, val: Value) {
    let max_collect = crate::config::global().max_collect_size;
    if !val.is_null() && state.collected.len() < max_collect {
        state.collected.push(val);
    }
}

pub(super) fn accumulate_collect_all(state: &mut AggState, val: Value) {
    let max_collect = crate::config::global().max_collect_size;
    if state.collected.len() < max_collect {
        state.collected.push(val);
    }
}

pub(super) fn finalize(state: &AggState) -> Value {
    Value::List(state.collected.clone())
}
