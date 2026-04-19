//! Mutation steps: property, drop, mergeV, mergeE.

use indexmap::IndexMap;

use super::super::literal::*;
use super::super::*;

fn attach_property_meta(base: GremlinValue, meta_pairs: &[GremlinValue]) -> GremlinValue {
    let mut map = match base {
        GremlinValue::Map(map) if map.contains_key("value") => map,
        other => {
            let mut map = IndexMap::new();
            map.insert("value".to_string(), other);
            map
        }
    };
    let mut idx = 0;
    while idx + 1 < meta_pairs.len() {
        if let GremlinValue::String(key) = &meta_pairs[idx] {
            map.insert(key.clone(), meta_pairs[idx + 1].clone());
        }
        idx += 2;
    }
    GremlinValue::Map(map)
}

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "property" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            if args.len() >= 2 {
                let is_cardinality = matches!(&args[0],
                    GremlinValue::String(s) if s.starts_with("Cardinality.") || s == "single" || s == "list" || s == "set"
                );
                if is_cardinality && matches!(&args[1], GremlinValue::Map(_)) {
                    if let GremlinValue::Map(map) = &args[1] {
                        for (k, v) in map {
                            out.push(GremlinStep::Property(k.clone(), v.clone()));
                        }
                    }
                } else if is_cardinality && args.len() >= 3 {
                    if let GremlinValue::String(key) = &args[1] {
                        let value = if args.len() > 3 {
                            attach_property_meta(args[2].clone(), &args[3..])
                        } else {
                            args[2].clone()
                        };
                        out.push(GremlinStep::Property(key.clone(), value));
                    }
                } else if let GremlinValue::String(key) = &args[0] {
                    let value = if args.len() > 2 {
                        attach_property_meta(args[1].clone(), &args[2..])
                    } else {
                        args[1].clone()
                    };
                    out.push(GremlinStep::Property(key.clone(), value));
                }
            } else if args.len() == 1 {
                if let GremlinValue::Map(map) = &args[0] {
                    for (k, v) in map {
                        out.push(GremlinStep::Property(k.clone(), v.clone()));
                    }
                }
            }
        }
        "drop" => out.push(GremlinStep::Drop),
        "mergeV" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let search = args
                .into_iter()
                .next()
                .unwrap_or(GremlinValue::Map(IndexMap::new()));
            out.push(GremlinStep::MergeV(search));
        }
        "mergeE" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            let search = args
                .into_iter()
                .next()
                .unwrap_or(GremlinValue::Map(IndexMap::new()));
            out.push(GremlinStep::MergeE(search));
        }
        _ => return Ok(false),
    }
    Ok(true)
}
