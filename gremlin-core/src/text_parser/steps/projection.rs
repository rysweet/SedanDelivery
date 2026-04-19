//! Projection / output steps: values, valueMap, properties, label, id, key, value,
//! elementMap, project, select, as, math, constant, identity, discard, element, loops.

use super::super::literal::*;
use super::super::*;

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "values" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Values(args));
        }
        "valueMap" => {
            let args = parse_string_arg_list(args_str)?;
            let (token_mode, keys) = if args.first().map(|s| s.as_str()) == Some("true") {
                (1u8, args[1..].to_vec())
            } else {
                (0u8, args)
            };
            out.push(GremlinStep::ValueMap(keys, token_mode));
        }
        "properties" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Properties(args));
        }
        "id" => out.push(GremlinStep::Id),
        "label" => out.push(GremlinStep::Label),
        "select" => {
            let args = parse_string_arg_list(args_str)?;
            let mut pop = None;
            let filtered: Vec<String> = args
                .into_iter()
                .filter(|a| match a.as_str() {
                    "Pop.first" | "first" => {
                        pop = Some(crate::bytecode::Pop::First);
                        false
                    }
                    "Pop.last" | "last" => {
                        pop = Some(crate::bytecode::Pop::Last);
                        false
                    }
                    "Pop.all" | "all" => {
                        pop = Some(crate::bytecode::Pop::All);
                        false
                    }
                    "Pop.mixed" | "mixed" => {
                        pop = Some(crate::bytecode::Pop::Mixed);
                        false
                    }
                    _ => true,
                })
                .map(|a| resolve_constant_traversal(&a))
                .collect();
            out.push(GremlinStep::Select(pop, filtered));
        }
        "as" => {
            let args = parse_string_arg_list(args_str)?;
            for alias in args {
                out.push(GremlinStep::As(alias));
            }
        }
        "elementMap" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::ElementMap(args));
        }
        "project" => {
            let args = parse_string_arg_list(args_str)?;
            out.push(GremlinStep::Project(args));
        }
        "math" => {
            let args = parse_string_arg_list(args_str)?;
            let expr_str = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Math(expr_str));
        }
        "constant" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let val = args.into_iter().next().unwrap_or(GremlinValue::Null);
            out.push(GremlinStep::Constant(val));
        }
        "identity" => out.push(GremlinStep::Identity),
        "discard" => out.push(GremlinStep::Discard),
        "element" => out.push(GremlinStep::Element),
        "value" => {
            // value() — extract the value from a property.
            out.push(GremlinStep::Values(vec![]));
        }
        "key" => {
            // key() — extract the key from a property.
            out.push(GremlinStep::Label);
        }
        "loops" => {
            let trimmed = args_str.trim();
            if trimmed.starts_with('"') || trimmed.starts_with('\'') {
                let quote = trimmed.as_bytes()[0] as char;
                if let Some(end_quote) = trimmed[1..].find(quote) {
                    let label = &trimmed[1..1 + end_quote];
                    out.push(GremlinStep::Loops(Some(label.into())));
                } else {
                    out.push(GremlinStep::Loops(None));
                }
            } else {
                out.push(GremlinStep::Loops(None));
            }
        }
        "index" => out.push(GremlinStep::Index),
        _ => return Ok(false),
    }
    Ok(true)
}
