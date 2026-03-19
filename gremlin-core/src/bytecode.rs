//! Gremlin bytecode representation and deserialization from GraphSON.

use crate::types::GremlinValue;
use serde::{Deserialize, Serialize};

/// A Gremlin predicate for comparison operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Predicate {
    Eq(GremlinValue),
    Neq(GremlinValue),
    Gt(GremlinValue),
    Gte(GremlinValue),
    Lt(GremlinValue),
    Lte(GremlinValue),
    Within(Vec<GremlinValue>),
    Without(Vec<GremlinValue>),
    Between(GremlinValue, GremlinValue),
    // Text predicates
    Containing(GremlinValue),
    NotContaining(GremlinValue),
    StartingWith(GremlinValue),
    NotStartingWith(GremlinValue),
    EndingWith(GremlinValue),
    NotEndingWith(GremlinValue),
    Regex(GremlinValue),
    NotRegex(GremlinValue),
    /// `P.typeOf(GType.STRING)` — check if value is of the specified Gremlin type
    IsType(GremlinValue),
    /// Compound predicate: all must match (P.gt(1).and(P.lt(5)))
    And(Vec<Predicate>),
    /// Compound predicate: any must match (P.gt(5).or(P.lt(1)))
    Or(Vec<Predicate>),
}

/// A single Gremlin traversal step.
#[derive(Debug, Clone, PartialEq)]
pub enum GremlinStep {
    // ── Source steps ──────────────────────────────────────────────────────────
    V(Option<Vec<GremlinValue>>),
    E(Option<Vec<GremlinValue>>),

    // ── Filter steps ─────────────────────────────────────────────────────────
    HasLabel(Vec<String>),
    Has(String, Option<GremlinValue>),
    /// `has("key", P.gt(30))` — property predicate filter
    HasPredicate(String, Predicate),
    HasId(Vec<GremlinValue>),

    // ── Traversal steps ──────────────────────────────────────────────────────
    Out(Vec<String>),
    In(Vec<String>),
    Both(Vec<String>),
    OutE(Vec<String>),
    InE(Vec<String>),
    BothE(Vec<String>),
    InV,
    OutV,
    BothV,
    OtherV,

    // ── Element steps ────────────────────────────────────────────────────────
    Values(Vec<String>),
    /// `valueMap()` — keys to project; bool indicates include tokens (id/label).
    ValueMap(Vec<String>, bool),
    Properties(Vec<String>),
    Id,
    Label,

    // ── Map/select steps ─────────────────────────────────────────────────────
    Select(Vec<String>),
    As(String),

    // ── Aggregation steps ────────────────────────────────────────────────────
    Count,
    Sum,
    Min,
    Max,
    Mean,
    SumLocal,
    MinLocal,
    MaxLocal,
    MeanLocal,
    Group(Option<String>),
    GroupCount(Option<String>),
    Fold,
    Unfold,

    // ── Ordering & paging ────────────────────────────────────────────────────
    Order,
    By(String, bool), // (key, descending)
    Limit(i64),
    Range(i64, i64),
    /// `limit(Scope.local, n)` — slice Map/List values
    LimitLocal(i64),
    /// `range(Scope.local, lo, hi)` — slice Map/List values
    RangeLocal(i64, i64),
    Dedup(Vec<String>),

    // ── Boolean steps ────────────────────────────────────────────────────────
    Where(Vec<GremlinStep>),
    Not(Vec<GremlinStep>),
    And(Vec<GremlinStep>),
    Or(Vec<GremlinStep>),
    Is(Predicate),

    // ── Mutation steps ───────────────────────────────────────────────────────
    AddV(String),
    AddE(String),
    Property(String, GremlinValue),
    From(Box<GremlinStep>),
    To(Box<GremlinStep>),
    Drop,

    // ── Path steps ───────────────────────────────────────────────────────────
    Path,

    // ── Repeat/branch steps ──────────────────────────────────────────────────
    Repeat(Vec<GremlinStep>),
    Times(i64),
    Until(Vec<GremlinStep>),
    Emit,
    Choose(Vec<GremlinStep>, Vec<GremlinStep>, Option<Vec<GremlinStep>>),
    Coalesce(Vec<Vec<GremlinStep>>),
    Union(Vec<Vec<GremlinStep>>),

    // ── Side-effect steps ────────────────────────────────────────────────────
    SideEffect(Vec<GremlinStep>),
    Aggregate(String),
    Cap(Vec<String>),

    // ── Misc steps ───────────────────────────────────────────────────────────
    Constant(GremlinValue),
    Inject(Vec<GremlinValue>),
    Match(Vec<Vec<GremlinStep>>),
    Project(Vec<String>),
    Identity,
    /// `tail()` or `tail(n)` — keep only the last n elements (default 1)
    Tail(i64),
    /// `skip(n)` — drop the first n elements
    Skip(i64),
    /// `elementMap(keys...)` — like valueMap but includes id and label
    ElementMap(Vec<String>),
    /// `hasKey(keys...)` — filter elements that have all listed property keys
    HasKey(Vec<String>),
    /// `hasValue(values...)` — filter elements that have any of the listed values
    HasValue(Vec<GremlinValue>),
    /// `optional(traversal)` — execute traversal, fall back to identity if empty
    Optional(Vec<GremlinStep>),
    /// `simplePath()` — filter to paths with no repeated elements
    SimplePath,
    /// `cyclicPath()` — filter to paths with repeated elements
    CyclicPath,
    /// `coin(probability)` — random filter (0.0 to 1.0)
    Coin(f64),
    /// `map(traversal)` — apply sub-traversal to each traverser
    Map(Vec<GremlinStep>),
    /// `flatMap(traversal)` — apply sub-traversal, flatten results
    FlatMap(Vec<GremlinStep>),
    /// `filter(traversal)` — keep traversers that produce results for the sub-traversal
    FilterTraversal(Vec<GremlinStep>),
    /// `none` predicate / `none()` step — matches nothing
    None,
    /// `barrier()` — bulk synchronization step
    Barrier,
    /// `sample(n)` — random sample of n traversers
    Sample(i64),
    /// `discard()` — consume traversal without returning results
    Discard,
    /// `loops()` — returns the number of loops in a repeat context
    Loops,

    /// `sack()` — retrieve the current sack value
    Sack,
    /// `sack(operator)` — apply binary op: sack = sack OP current_value
    /// Operator string: "sum", "minus", "mult", "div", "assign"
    SackOp(String),
    /// `withSack(initValue)` — initialize the sack with a value
    WithSack(GremlinValue),
    /// `option(token, traversal)` — option step for branch/choose
    Option(GremlinValue, Vec<GremlinStep>),

    // ── String steps ────────────────────────────────────────────────────────
    /// `toLower()` — lowercase string
    ToLower,
    /// `toUpper()` — uppercase string
    ToUpper,
    /// `trim()` — trim whitespace
    Trim,
    /// `lTrim()` — trim leading whitespace
    LTrim,
    /// `rTrim()` — trim trailing whitespace
    RTrim,
    /// `length()` — string length or list size
    Length,
    /// `reverse()` — reverse string or list
    Reverse,
    /// `replace(old, new)` — string replacement
    Replace(String, String),
    /// `split(separator)` — split string into list
    Split(String),
    /// `substring(start)` or `substring(start, length)` — extract substring
    Substring(i64, Option<i64>),
    /// `substring(Scope.local, start, end)` — substring each element in a list
    SubstringLocal(i64, Option<i64>),
    /// `concat(traversals...)` — concatenate string results
    Concat(Vec<Vec<GremlinStep>>),
    /// `asString()` — convert to string
    AsString,
    /// `asString(Scope.local)` — convert elements of a list/map to strings
    AsStringLocal,
    /// `asNumber()` — convert to number (integer or double)
    AsNumber,
    /// `asBool()` — convert to boolean
    AsBool,
    /// `conjoin(separator)` — join list elements with separator
    Conjoin(String),
    /// `format(pattern)` — format string
    Format(String),
    /// `typeOf()` — return type name as string
    TypeOf,

    // ── Collection steps ────────────────────────────────────────────────────
    /// `combine(traversal)` — append two lists
    Combine(Vec<GremlinStep>),
    /// `intersect(traversal)` — set intersection
    Intersect(Vec<GremlinStep>),
    /// `difference(traversal)` — set difference
    Difference(Vec<GremlinStep>),
    /// `disjunct(traversal)` — symmetric difference
    Disjunct(Vec<GremlinStep>),
    /// `merge(traversal)` — list merge (synonym of combine for lists)
    MergeList(Vec<GremlinStep>),
    /// `product(traversal)` — cartesian product of two lists
    Product(Vec<GremlinStep>),
    /// `all(predicate)` — true if all elements satisfy predicate
    All(Predicate),
    /// `any(predicate)` — true if any element satisfies predicate
    Any(Predicate),
    /// `none(predicate)` — true if no elements satisfy predicate
    NonePred(Predicate),

    // ── Predicate wrappers (used inside Is/Where) ────────────────────────────
    Eq(GremlinValue),
    Neq(GremlinValue),
    Gt(GremlinValue),
    Gte(GremlinValue),
    Lt(GremlinValue),
    Lte(GremlinValue),
    Within(Vec<GremlinValue>),
    Without(Vec<GremlinValue>),
}

/// Raw Gremlin bytecode as sent over the wire (GraphSON format).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bytecode {
    #[serde(default)]
    pub source_instructions: Vec<(String, Vec<GremlinValue>)>,
    #[serde(default)]
    pub step_instructions: Vec<(String, Vec<GremlinValue>)>,
}

/// Errors that can occur during bytecode deserialization.
#[derive(Debug, thiserror::Error)]
pub enum BytecodeError {
    #[error("unknown step name: {0}")]
    UnknownStep(String),
    #[error("invalid argument for step '{step}': {reason}")]
    InvalidArgument { step: String, reason: String },
    #[error("missing required argument for step '{0}'")]
    MissingArgument(String),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}

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

fn json_to_gremlin_value(v: &serde_json::Value) -> GremlinValue {
    match v {
        serde_json::Value::Null => GremlinValue::Null,
        serde_json::Value::Bool(b) => GremlinValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                GremlinValue::Integer(i)
            } else {
                GremlinValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => GremlinValue::String(s.clone()),
        serde_json::Value::Array(arr) => {
            GremlinValue::List(arr.iter().map(json_to_gremlin_value).collect())
        }
        serde_json::Value::Object(obj) => {
            let map = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_gremlin_value(v)))
                .collect();
            GremlinValue::Map(map)
        }
    }
}

fn collect_string_args(args: &[serde_json::Value]) -> Vec<String> {
    args.iter()
        .filter_map(|a| a.as_str().map(String::from))
        .collect()
}

fn collect_value_args(args: &[serde_json::Value]) -> Vec<GremlinValue> {
    args.iter().map(json_to_gremlin_value).collect()
}

fn parse_step(name: &str, args: &[serde_json::Value]) -> Result<GremlinStep, BytecodeError> {
    match name {
        "V" => {
            if args.is_empty() {
                Ok(GremlinStep::V(None))
            } else {
                Ok(GremlinStep::V(Some(collect_value_args(args))))
            }
        }
        "E" => {
            if args.is_empty() {
                Ok(GremlinStep::E(None))
            } else {
                Ok(GremlinStep::E(Some(collect_value_args(args))))
            }
        }
        "hasLabel" => Ok(GremlinStep::HasLabel(collect_string_args(args))),
        "has" => {
            let key = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("has".into()))?
                .to_string();
            let value = args.get(1).map(json_to_gremlin_value);
            Ok(GremlinStep::Has(key, value))
        }
        "hasId" => Ok(GremlinStep::HasId(collect_value_args(args))),
        "out" => Ok(GremlinStep::Out(collect_string_args(args))),
        "in" => Ok(GremlinStep::In(collect_string_args(args))),
        "both" => Ok(GremlinStep::Both(collect_string_args(args))),
        "outE" => Ok(GremlinStep::OutE(collect_string_args(args))),
        "inE" => Ok(GremlinStep::InE(collect_string_args(args))),
        "bothE" => Ok(GremlinStep::BothE(collect_string_args(args))),
        "inV" => Ok(GremlinStep::InV),
        "outV" => Ok(GremlinStep::OutV),
        "bothV" => Ok(GremlinStep::BothV),
        "otherV" => Ok(GremlinStep::OtherV),
        "values" => Ok(GremlinStep::Values(collect_string_args(args))),
        "valueMap" => Ok(GremlinStep::ValueMap(collect_string_args(args), false)),
        "properties" => Ok(GremlinStep::Properties(collect_string_args(args))),
        "id" => Ok(GremlinStep::Id),
        "label" => Ok(GremlinStep::Label),
        "select" => Ok(GremlinStep::Select(collect_string_args(args))),
        "as" => {
            let alias = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("as".into()))?
                .to_string();
            Ok(GremlinStep::As(alias))
        }
        "count" => Ok(GremlinStep::Count),
        "sum" => Ok(GremlinStep::Sum),
        "min" => Ok(GremlinStep::Min),
        "max" => Ok(GremlinStep::Max),
        "mean" => Ok(GremlinStep::Mean),
        "group" => Ok(GremlinStep::Group(None)),
        "groupCount" => Ok(GremlinStep::GroupCount(None)),
        "fold" => Ok(GremlinStep::Fold),
        "unfold" => Ok(GremlinStep::Unfold),
        "order" => Ok(GremlinStep::Order),
        "limit" => {
            let n = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("limit".into()))?;
            Ok(GremlinStep::Limit(n))
        }
        "range" => {
            let low = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("range".into()))?;
            let high = args
                .get(1)
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("range".into()))?;
            Ok(GremlinStep::Range(low, high))
        }
        "dedup" => Ok(GremlinStep::Dedup(collect_string_args(args))),
        "is" => {
            let pred = parse_predicate_from_args(args)?;
            Ok(GremlinStep::Is(pred))
        }
        "addV" => {
            let label = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("addV".into()))?
                .to_string();
            Ok(GremlinStep::AddV(label))
        }
        "addE" => {
            let label = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("addE".into()))?
                .to_string();
            Ok(GremlinStep::AddE(label))
        }
        "property" => {
            let key = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("property".into()))?
                .to_string();
            let value = args
                .get(1)
                .map(json_to_gremlin_value)
                .unwrap_or(GremlinValue::Null);
            Ok(GremlinStep::Property(key, value))
        }
        "drop" => Ok(GremlinStep::Drop),
        "path" => Ok(GremlinStep::Path),
        "times" => {
            let n = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("times".into()))?;
            Ok(GremlinStep::Times(n))
        }
        "emit" => Ok(GremlinStep::Emit),
        "aggregate" => {
            let key = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("aggregate".into()))?
                .to_string();
            Ok(GremlinStep::Aggregate(key))
        }
        "cap" => Ok(GremlinStep::Cap(collect_string_args(args))),
        "constant" => {
            let value = args
                .first()
                .map(json_to_gremlin_value)
                .unwrap_or(GremlinValue::Null);
            Ok(GremlinStep::Constant(value))
        }
        "inject" => Ok(GremlinStep::Inject(collect_value_args(args))),
        "project" => Ok(GremlinStep::Project(collect_string_args(args))),
        "identity" => Ok(GremlinStep::Identity),
        "tail" => {
            let n = args.first().and_then(|a| a.as_i64()).unwrap_or(1);
            Ok(GremlinStep::Tail(n))
        }
        "skip" => {
            let n = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("skip".into()))?;
            Ok(GremlinStep::Skip(n))
        }
        "elementMap" => Ok(GremlinStep::ElementMap(collect_string_args(args))),
        "hasKey" => Ok(GremlinStep::HasKey(collect_string_args(args))),
        "hasValue" => Ok(GremlinStep::HasValue(collect_value_args(args))),
        "simplePath" => Ok(GremlinStep::SimplePath),
        "cyclicPath" => Ok(GremlinStep::CyclicPath),
        "coin" => {
            let p = args
                .first()
                .and_then(|a| a.as_f64())
                .ok_or_else(|| BytecodeError::MissingArgument("coin".into()))?;
            Ok(GremlinStep::Coin(p))
        }
        "none" => Ok(GremlinStep::None),
        "barrier" => Ok(GremlinStep::Barrier),
        "sample" => {
            let n = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("sample".into()))?;
            Ok(GremlinStep::Sample(n))
        }
        "discard" => Ok(GremlinStep::Discard),
        "loops" => Ok(GremlinStep::Loops),
        "sack" => Ok(GremlinStep::Sack),
        _ => Err(BytecodeError::UnknownStep(name.into())),
    }
}

fn parse_predicate_from_args(args: &[serde_json::Value]) -> Result<Predicate, BytecodeError> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_simple_traversal() {
        let json: serde_json::Value = serde_json::json!({
            "step": [
                ["V"],
                ["has", "name", "marko"],
                ["out", "knows"],
                ["values", "name"]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0], GremlinStep::V(None));
        assert_eq!(
            steps[1],
            GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into())))
        );
        assert_eq!(steps[2], GremlinStep::Out(vec!["knows".into()]));
        assert_eq!(steps[3], GremlinStep::Values(vec!["name".into()]));
    }

    #[test]
    fn deserialize_v_with_ids() {
        let json: serde_json::Value = serde_json::json!({
            "step": [["V", 1, 2, 3]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::V(Some(vec![
                GremlinValue::Integer(1),
                GremlinValue::Integer(2),
                GremlinValue::Integer(3),
            ]))
        );
    }

    #[test]
    fn deserialize_mutation_steps() {
        let json: serde_json::Value = serde_json::json!({
            "step": [
                ["addV", "person"],
                ["property", "name", "alice"],
                ["property", "age", 30]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0], GremlinStep::AddV("person".into()));
        assert_eq!(
            steps[1],
            GremlinStep::Property("name".into(), GremlinValue::String("alice".into()))
        );
        assert_eq!(
            steps[2],
            GremlinStep::Property("age".into(), GremlinValue::Integer(30))
        );
    }

    #[test]
    fn unknown_step_returns_error() {
        let json: serde_json::Value = serde_json::json!({
            "step": [["nonexistent_step"]]
        });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::UnknownStep(_)));
    }

    #[test]
    fn deserialize_limit_and_dedup() {
        let json: serde_json::Value = serde_json::json!({
            "step": [
                ["V"],
                ["dedup"],
                ["limit", 10]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[1], GremlinStep::Dedup(vec![]));
        assert_eq!(steps[2], GremlinStep::Limit(10));
    }
}
