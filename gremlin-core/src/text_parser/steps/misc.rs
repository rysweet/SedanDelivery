//! Misc steps that don't fit other categories: with/withPath/withBulk, sack,
//! profile/iterate/explain/fail (rejected), read/write, call, shortestPath,
//! pageRank, connectedComponent, peerPressure, io.

use super::super::literal::*;
use super::super::*;

pub(super) fn try_dispatch(
    name: &str,
    args_str: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
    out: &mut Vec<GremlinStep>,
) -> Result<bool, TextParseError> {
    match name {
        "sack" => {
            let args_trimmed = args_str.trim();
            if args_trimmed.is_empty() {
                out.push(GremlinStep::Sack);
            } else {
                let op_name = args_trimmed.replace("Operator.", "").trim().to_lowercase();
                out.push(GremlinStep::SackOp(op_name));
            }
        }
        "with" | "withPath" | "withBulk" => {
            let args = parse_string_arg_list(args_str)?;
            if args.iter().any(|a| a == "WithOptions.tokens") {
                let mode = if args.iter().any(|a| a == "WithOptions.labels") {
                    2u8
                } else if args.iter().any(|a| a == "WithOptions.ids") {
                    3u8
                } else {
                    1u8
                };
                for step in out.iter_mut().rev() {
                    if let GremlinStep::ValueMap(_, ref mut token_mode) = step {
                        *token_mode = mode;
                        break;
                    }
                }
            }
            if args.iter().any(|a| a == "WithOptions.indexer")
                && args.iter().any(|a| a == "WithOptions.map")
            {
                for step in out.iter_mut().rev() {
                    if matches!(step, GremlinStep::Index) {
                        *step = GremlinStep::IndexWithMap;
                        break;
                    }
                }
            }
        }
        "profile" | "iterate" | "explain" | "fail" => {
            return Err(TextParseError::UnknownStep(name.to_string()));
        }
        "read" | "write" => {
            out.push(GremlinStep::Io(String::new()));
        }
        "call" => {
            let args = parse_arg_list(args_str, resolve_param)?;
            out.push(GremlinStep::Call(args));
        }
        "shortestPath" => out.push(GremlinStep::ShortestPath),
        "pageRank" => out.push(GremlinStep::PageRank),
        "connectedComponent" => out.push(GremlinStep::ConnectedComponent),
        "peerPressure" => out.push(GremlinStep::PeerPressure),
        "io" => {
            let args = parse_string_arg_list(args_str)?;
            let file = args.into_iter().next().unwrap_or_default();
            out.push(GremlinStep::Io(file));
        }
        _ => return Ok(false),
    }
    Ok(true)
}
