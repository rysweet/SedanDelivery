//! Predicate object parsing (`g:P` GraphSON form) used by `is`/`where`.

use super::value::json_to_gremlin_value;
use crate::bytecode::{BytecodeError, Predicate};
use crate::types::GremlinValue;

pub(in crate::bytecode) fn parse_predicate_from_args(
    args: &[serde_json::Value],
) -> Result<Predicate, BytecodeError> {
    if args.is_empty() {
        return Err(BytecodeError::MissingArgument("is".into()));
    }
    // Simple scalar: is(42) → Eq
    if !args[0].is_object() {
        return Ok(Predicate::Eq(json_to_gremlin_value(&args[0])));
    }
    // Predicate object: {"@type": "g:P", "@value": {"predicate": "gt", "value": 10}}
    let obj = args[0]
        .as_object()
        .ok_or_else(|| BytecodeError::InvalidArgument {
            step: "is".into(),
            reason: "predicate must be an object".into(),
        })?;
    if let Some(pval) = obj.get("@value") {
        let pred_name = pval
            .get("predicate")
            .and_then(|p| p.as_str())
            .unwrap_or("eq");
        let value = pval
            .get("value")
            .map(json_to_gremlin_value)
            .unwrap_or(GremlinValue::Null);
        match pred_name {
            "eq" => Ok(Predicate::Eq(value)),
            "neq" => Ok(Predicate::Neq(value)),
            "gt" => Ok(Predicate::Gt(value)),
            "gte" => Ok(Predicate::Gte(value)),
            "lt" => Ok(Predicate::Lt(value)),
            "lte" => Ok(Predicate::Lte(value)),
            "within" => {
                if let GremlinValue::List(items) = value {
                    Ok(Predicate::Within(items))
                } else {
                    Ok(Predicate::Within(vec![value]))
                }
            }
            "without" => {
                if let GremlinValue::List(items) = value {
                    Ok(Predicate::Without(items))
                } else {
                    Ok(Predicate::Without(vec![value]))
                }
            }
            "between" => {
                let low = value;
                let high = pval
                    .get("other")
                    .map(json_to_gremlin_value)
                    .unwrap_or(GremlinValue::Null);
                Ok(Predicate::Between(low, high))
            }
            _ => Err(BytecodeError::UnknownStep(format!("predicate:{pred_name}"))),
        }
    } else {
        // Plain value comparison
        Ok(Predicate::Eq(json_to_gremlin_value(&args[0])))
    }
}
