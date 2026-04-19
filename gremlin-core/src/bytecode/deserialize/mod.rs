//! GraphSON bytecode deserialization: parse JSON step arrays into typed
//! [`GremlinStep`]s.
//!
//! The implementation is split across focused submodules:
//!
//! * [`value`]    – raw JSON → [`GremlinValue`] conversion plus arg collectors
//! * [`inner`]    – nested sub-traversal parsing helpers
//! * [`step`]     – the [`parse_step`] dispatch table mapping step names to
//!   [`GremlinStep`] variants
//! * [`predicate`] – [`Predicate`] parsing for `is`/`where` style steps

mod inner;
mod predicate;
mod step;
mod value;

// Re-exported so existing callers (notably `bytecode::tests`) can keep
// referring to `super::deserialize::json_to_gremlin_value`.
#[cfg(test)]
pub(super) use value::json_to_gremlin_value;

use super::{BytecodeError, GremlinStep};
use step::parse_step;

/// Deserialize GraphSON bytecode JSON into a sequence of typed `GremlinStep`s.
///
/// Expected JSON structure:
/// ```json
/// {
///   "step": [
///     ["V"],
///     ["has", "name", "marko"],
///     ["out", "knows"],
///     ["values", "name"]
///   ]
/// }
/// ```
pub fn deserialize_bytecode(json: &serde_json::Value) -> Result<Vec<GremlinStep>, BytecodeError> {
    let steps_array = json
        .get("step")
        .or_else(|| json.get("step_instructions"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| BytecodeError::InvalidArgument {
            step: "root".into(),
            reason: "expected 'step' array".into(),
        })?;

    let mut result = Vec::with_capacity(steps_array.len());
    for step_entry in steps_array {
        let arr = step_entry
            .as_array()
            .ok_or_else(|| BytecodeError::InvalidArgument {
                step: "entry".into(),
                reason: "expected array".into(),
            })?;
        if arr.is_empty() {
            continue;
        }
        let name = arr[0]
            .as_str()
            .ok_or_else(|| BytecodeError::InvalidArgument {
                step: "entry".into(),
                reason: "step name must be a string".into(),
            })?;
        let args = &arr[1..];
        let step = parse_step(name, args)?;
        result.push(step);
    }
    Ok(result)
}
