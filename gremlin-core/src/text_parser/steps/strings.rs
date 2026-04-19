//! String / scalar conversion steps: concat, split, length, toLower, toUpper,
//! trim, lTrim, rTrim, replace, substring, format, reverse, conjoin, asString,
//! asNumber, asBool, typeOf, asDate, dateAdd, dateDiff.

use super::super::*;
use super::super::{literal::*, tokenizer::*};

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "toLower" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_tolower_local".into()));
            } else {
                out.push(GremlinStep::ToLower);
            }
        }
        "toUpper" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_toupper_local".into()));
            } else {
                out.push(GremlinStep::ToUpper);
            }
        }
        "trim" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_trim_local".into()));
            } else {
                out.push(GremlinStep::Trim);
            }
        }
        "lTrim" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_ltrim_local".into()));
            } else {
                out.push(GremlinStep::LTrim);
            }
        }
        "rTrim" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_rtrim_local".into()));
            } else {
                out.push(GremlinStep::RTrim);
            }
        }
        "length" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::StringOpLocal("gremlin_length_local".into()));
            } else {
                out.push(GremlinStep::Length);
            }
        }
        "reverse" => out.push(GremlinStep::Reverse),
        "replace" => {
            let args = parse_string_arg_list(args_str)?;
            let is_local = args
                .first()
                .map(|a| a.contains("local") || a == "Scope.local")
                .unwrap_or(false);
            if is_local {
                if args.len() >= 3 {
                    out.push(GremlinStep::StringOpLocal(format!(
                        "gremlin_replace_local\x00{}\x00{}",
                        args[1], args[2]
                    )));
                }
            } else if args.len() >= 2 {
                out.push(GremlinStep::Replace(args[0].clone(), args[1].clone()));
            } else {
                return Err(TextParseError::InvalidArgument(
                    "replace requires 2 arguments".into(),
                ));
            }
        }
        "split" => {
            let args = parse_string_arg_list(args_str)?;
            let is_local = args
                .first()
                .map(|a| a.contains("local") || a == "Scope.local")
                .unwrap_or(false);
            if is_local {
                if args.len() >= 2 {
                    out.push(GremlinStep::StringOpLocal(format!(
                        "gremlin_split_local\x00{}",
                        args[1]
                    )));
                }
            } else if !args.is_empty() {
                out.push(GremlinStep::Split(args[0].clone()));
            } else {
                out.push(GremlinStep::Split(String::new()));
            }
        }
        "substring" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let (offset, is_local) = if matches!(args.first(), Some(GremlinValue::String(s)) if s.contains("local"))
            {
                (1, true)
            } else {
                (0, false)
            };
            let start = match args.get(offset) {
                Some(GremlinValue::Integer(n)) => *n,
                _ => 0,
            };
            let len = match args.get(offset + 1) {
                Some(GremlinValue::Integer(n)) => Some(*n),
                _ => None,
            };
            if is_local {
                out.push(GremlinStep::SubstringLocal(start, len));
            } else {
                out.push(GremlinStep::Substring(start, len));
            }
        }
        "concat" => {
            let raw_args = split_top_level_args(args_str);
            let mut traversals = Vec::new();
            for arg in &raw_args {
                let s = arg.trim();
                if s.is_empty() {
                    continue;
                }
                if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
                    let inner = &s[1..s.len() - 1];
                    let unescaped = inner.replace("\\\"", "\"").replace("\\\\", "\\");
                    traversals.push(vec![GremlinStep::Constant(GremlinValue::String(unescaped))]);
                } else if s == "null" {
                    traversals.push(vec![GremlinStep::Constant(GremlinValue::Null)]);
                } else {
                    let inner = parse_inner_traversal(s, resolve_param)?;
                    traversals.push(inner);
                }
            }
            out.push(GremlinStep::Concat(traversals));
        }
        "conjoin" => {
            let args = parse_string_arg_list(args_str)?;
            let sep = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Conjoin(sep));
        }
        "format" => {
            let args = parse_string_arg_list(args_str)?;
            let pattern = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Format(pattern));
        }
        "asString" => {
            if args_str.contains("local") || args_str.contains("Scope.local") {
                out.push(GremlinStep::AsStringLocal);
            } else {
                out.push(GremlinStep::AsString);
            }
        }
        "asNumber" => {
            let trimmed = args_str.trim();
            if trimmed.is_empty() {
                out.push(GremlinStep::AsNumber);
            } else if trimmed.contains("INT") || trimmed.contains("LONG") {
                out.push(GremlinStep::AsNumberTyped("int".into()));
            } else if trimmed.contains("FLOAT")
                || trimmed.contains("DOUBLE")
                || trimmed.contains("BIG_DECIMAL")
            {
                out.push(GremlinStep::AsNumberTyped("float".into()));
            } else if trimmed.contains("BYTE") {
                out.push(GremlinStep::AsNumberTyped("byte".into()));
            } else if trimmed.contains("SHORT") {
                out.push(GremlinStep::AsNumberTyped("short".into()));
            } else if trimmed.contains("VERTEX")
                || trimmed.contains("EDGE")
                || trimmed.contains("PROPERTY")
                || trimmed.contains("PATH")
            {
                out.push(GremlinStep::AsNumberTyped("__invalid__".into()));
            } else {
                out.push(GremlinStep::AsNumber);
            }
        }
        "asBool" => out.push(GremlinStep::AsBool),
        "typeOf" => out.push(GremlinStep::TypeOf),
        "asDate" => out.push(GremlinStep::AsDate),
        "dateAdd" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let unit = match args.first() {
                Some(GremlinValue::String(s)) => s.clone(),
                _ => String::new(),
            };
            let amount = match args.get(1) {
                Some(GremlinValue::Integer(n)) => *n,
                _ => 0,
            };
            out.push(GremlinStep::DateAdd(unit, amount));
        }
        "dateDiff" => {
            let inner = parse_inner_traversal(args_str, resolve_param)?;
            out.push(GremlinStep::DateDiff(inner));
        }
        _ => return Ok(false),
    }
    Ok(true)
}
