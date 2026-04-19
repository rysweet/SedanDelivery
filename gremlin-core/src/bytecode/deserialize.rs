//! GraphSON bytecode deserialization: parse JSON step arrays into typed [`GremlinStep`]s.

use super::{BytecodeError, GremlinStep, Predicate};
use crate::types::GremlinValue;

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

pub(super) fn json_to_gremlin_value(v: &serde_json::Value) -> GremlinValue {
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

pub(super) fn collect_string_args(args: &[serde_json::Value]) -> Vec<String> {
    args.iter()
        .filter_map(|a| a.as_str().map(String::from))
        .collect()
}

pub(super) fn collect_value_args(args: &[serde_json::Value]) -> Vec<GremlinValue> {
    args.iter().map(json_to_gremlin_value).collect()
}

/// Parse a single nested-traversal argument (either bare `{"step":[...]}` or
/// GraphSON-wrapped `{"@type":"g:Bytecode","@value":{"step":[...]}}`).
pub(super) fn parse_inner_traversal(
    step_name: &str,
    arg: &serde_json::Value,
) -> Result<Vec<GremlinStep>, BytecodeError> {
    let inner = arg.get("@value").unwrap_or(arg);
    deserialize_bytecode(inner).map_err(|e| BytecodeError::InvalidArgument {
        step: step_name.into(),
        reason: format!("nested traversal: {e}"),
    })
}

/// Parse all args as nested traversals. Used by steps like `union`, `and`,
/// `or`, `coalesce`, `match` that accept a variadic list of sub-traversals.
pub(super) fn parse_inner_traversals(
    step_name: &str,
    args: &[serde_json::Value],
) -> Result<Vec<Vec<GremlinStep>>, BytecodeError> {
    args.iter()
        .map(|a| parse_inner_traversal(step_name, a))
        .collect()
}

/// Parse the single-traversal argument, returning MissingArgument if absent.
pub(super) fn parse_single_inner_traversal(
    step_name: &str,
    args: &[serde_json::Value],
) -> Result<Vec<GremlinStep>, BytecodeError> {
    let arg = args
        .first()
        .ok_or_else(|| BytecodeError::MissingArgument(step_name.into()))?;
    parse_inner_traversal(step_name, arg)
}

pub(super) fn parse_step(
    name: &str,
    args: &[serde_json::Value],
) -> Result<GremlinStep, BytecodeError> {
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
        "valueMap" => Ok(GremlinStep::ValueMap(collect_string_args(args), 0)),
        "properties" => Ok(GremlinStep::Properties(collect_string_args(args))),
        "id" => Ok(GremlinStep::Id),
        "label" => Ok(GremlinStep::Label),
        "select" => Ok(GremlinStep::Select(None, collect_string_args(args))),
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
        "barrier" => {
            let is_norm_sack = args.iter().any(|a| {
                a.as_str()
                    .is_some_and(|s| s.contains("normSack") || s.contains("Barrier.normSack"))
            });
            if is_norm_sack {
                Ok(GremlinStep::BarrierNormSack)
            } else {
                Ok(GremlinStep::Barrier)
            }
        }
        "sample" => {
            let n = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("sample".into()))?;
            Ok(GremlinStep::Sample(n))
        }
        "discard" => Ok(GremlinStep::Discard),
        "loops" => {
            // Optional label argument names a specific repeat scope.
            let label = args.first().and_then(|a| a.as_str()).map(String::from);
            Ok(GremlinStep::Loops(label))
        }
        "sack" => Ok(GremlinStep::Sack),
        "repeat" => {
            // Bytecode form: ["repeat", <inner-bytecode>]
            // or:            ["repeat", "<label>", <inner-bytecode>]
            // Inner is either a bare {"step":[...]} object or a wrapped
            // GraphSON value {"@type":"g:Bytecode","@value":{"step":[...]}}.
            let (label, body_json) = match args {
                [body] => (None, body),
                [maybe_label, body] => {
                    if let Some(lbl) = maybe_label.as_str() {
                        (Some(lbl.to_string()), body)
                    } else {
                        (None, maybe_label)
                    }
                }
                _ => {
                    return Err(BytecodeError::MissingArgument("repeat".into()));
                }
            };
            let inner_bytecode = body_json.get("@value").unwrap_or(body_json);
            let body = deserialize_bytecode(inner_bytecode).map_err(|e| {
                BytecodeError::InvalidArgument {
                    step: "repeat".into(),
                    reason: format!("nested traversal: {e}"),
                }
            })?;
            Ok(GremlinStep::Repeat(label, body))
        }

        // ── Nested-traversal steps that take a single sub-traversal ──────────
        "until" => Ok(GremlinStep::Until(parse_single_inner_traversal(
            "until", args,
        )?)),
        "where" => Ok(GremlinStep::Where(parse_single_inner_traversal(
            "where", args,
        )?)),
        "not" => Ok(GremlinStep::Not(parse_single_inner_traversal("not", args)?)),
        "from" => Ok(GremlinStep::From(parse_single_inner_traversal(
            "from", args,
        )?)),
        "to" => Ok(GremlinStep::To(parse_single_inner_traversal("to", args)?)),
        "optional" => Ok(GremlinStep::Optional(parse_single_inner_traversal(
            "optional", args,
        )?)),
        "map" => Ok(GremlinStep::Map(parse_single_inner_traversal("map", args)?)),
        "flatMap" => Ok(GremlinStep::FlatMap(parse_single_inner_traversal(
            "flatMap", args,
        )?)),
        "filter" => Ok(GremlinStep::FilterTraversal(parse_single_inner_traversal(
            "filter", args,
        )?)),
        "sideEffect" => Ok(GremlinStep::SideEffect(parse_single_inner_traversal(
            "sideEffect",
            args,
        )?)),

        // ── Nested-traversal steps that take a variadic list of sub-traversals ──
        "and" => Ok(GremlinStep::And(parse_inner_traversals("and", args)?)),
        "or" => Ok(GremlinStep::Or(parse_inner_traversals("or", args)?)),
        "union" => Ok(GremlinStep::Union(parse_inner_traversals("union", args)?)),
        "coalesce" => Ok(GremlinStep::Coalesce(parse_inner_traversals(
            "coalesce", args,
        )?)),
        "match" => Ok(GremlinStep::Match(parse_inner_traversals("match", args)?)),

        // ── Choose: (predicate, true_branch, optional_false_branch) ──────────
        "choose" => match args {
            [pred, t] => Ok(GremlinStep::Choose(
                parse_inner_traversal("choose", pred)?,
                parse_inner_traversal("choose", t)?,
                None,
            )),
            [pred, t, f] => Ok(GremlinStep::Choose(
                parse_inner_traversal("choose", pred)?,
                parse_inner_traversal("choose", t)?,
                Some(parse_inner_traversal("choose", f)?),
            )),
            _ => Err(BytecodeError::MissingArgument("choose".into())),
        },

        // ── Option: (token_value, sub_traversal) ─────────────────────────────
        "option" => {
            if args.len() < 2 {
                return Err(BytecodeError::MissingArgument("option".into()));
            }
            let token = json_to_gremlin_value(&args[0]);
            let body = parse_inner_traversal("option", &args[1])?;
            Ok(GremlinStep::Option(token, body))
        }

        // ── By: scalar key form, traversal form, or (key, asc/desc) form ─────
        "by" => match args {
            [] => Ok(GremlinStep::By(String::new(), false)),
            [a] => {
                if let Some(s) = a.as_str() {
                    Ok(GremlinStep::By(s.to_string(), false))
                } else {
                    Ok(GremlinStep::ByTraversal(
                        parse_inner_traversal("by", a)?,
                        false,
                    ))
                }
            }
            [a, b] => {
                let descending = b
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case("desc") || s.contains("decr"));
                if let Some(s) = a.as_str() {
                    Ok(GremlinStep::By(s.to_string(), descending))
                } else {
                    Ok(GremlinStep::ByTraversal(
                        parse_inner_traversal("by", a)?,
                        descending,
                    ))
                }
            }
            _ => Err(BytecodeError::InvalidArgument {
                step: "by".into(),
                reason: format!("unexpected by() arity {}", args.len()),
            }),
        },

        _ => Err(BytecodeError::UnknownStep(name.into())),
    }
}

pub(super) fn parse_predicate_from_args(
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
