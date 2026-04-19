//! `withSideEffect(...)` extraction and value parsing.

use super::*;
use super::{literal::*, tokenizer::*};

pub(crate) fn extract_with_side_effects(
    input: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Vec<(String, GremlinValue, Option<String>)> {
    let mut results = Vec::new();
    let mut search = input;
    while let Some(prefix) = search.find("withSideEffect(") {
        let after = &search[prefix + "withSideEffect(".len()..];
        // Find the matching close paren
        let mut depth = 1u32;
        let mut string_char: Option<char> = None;
        let mut escape = false;
        let mut end = 0;
        for (i, c) in after.char_indices() {
            if update_string_state(c, &mut string_char, &mut escape) {
                continue;
            }
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = i;
                        break;
                    }
                }
                _ => {}
            }
        }
        if end == 0 {
            break;
        }
        let arg_str = &after[..end];
        // Split into key and value (first comma at top level)
        let mut comma_depth = 0;
        let mut comma_pos = None;
        let mut string_char2: Option<char> = None;
        let mut esc = false;
        for (i, c) in arg_str.char_indices() {
            if update_string_state(c, &mut string_char2, &mut esc) {
                continue;
            }
            match c {
                '(' | '[' | '{' => comma_depth += 1,
                ')' | ']' | '}' => comma_depth -= 1,
                ',' if comma_depth == 0 => {
                    if comma_pos.is_none() {
                        comma_pos = Some(i);
                    }
                    // Don't break — we want the first comma only for key/value split
                    break;
                }
                _ => {}
            }
        }
        if let Some(cp) = comma_pos {
            let key_str = arg_str[..cp].trim().trim_matches('"');
            let rest_str = arg_str[cp + 1..].trim();
            // Check for 3-arg form: value, Operator.xxx
            // Split rest_str at the last top-level comma to separate value from operator
            let mut last_comma = None;
            let mut ld = 0i32;
            let mut string_char3: Option<char> = None;
            let mut esc2 = false;
            for (i2, c) in rest_str.char_indices() {
                if update_string_state(c, &mut string_char3, &mut esc2) {
                    continue;
                }
                match c {
                    '(' | '[' | '{' => ld += 1,
                    ')' | ']' | '}' => ld -= 1,
                    ',' if ld == 0 => {
                        last_comma = Some(i2);
                    }
                    _ => {}
                }
            }
            let (val_str, operator) = if let Some(lc) = last_comma {
                let maybe_op = rest_str[lc + 1..].trim();
                if maybe_op.starts_with("Operator.") {
                    let op_name = maybe_op.strip_prefix("Operator.").unwrap_or("sum");
                    (&rest_str[..lc], Some(op_name.to_string()))
                } else {
                    (rest_str, None)
                }
            } else {
                (rest_str, None)
            };
            // Try to parse the value — handle sets like {"alice"}, lists like ["x","y"], etc.
            if let Some(val) = parse_side_effect_value(val_str.trim(), resolve_param) {
                // If the value used set notation ({...}), mark with "set" operator
                // so the compiler tracks set semantics for this key.
                let operator = if operator.is_none()
                    && val_str.trim().starts_with('{')
                    && val_str.trim().ends_with('}')
                {
                    Some("set".to_string())
                } else {
                    operator
                };
                results.push((key_str.to_string(), val, operator));
            }
        }
        search = &after[end..];
    }
    results
}

/// Parse a withSideEffect value: strings, numbers, sets {}, lists [], etc.
pub(crate) fn parse_side_effect_value(
    s: &str,
    resolve_param: &dyn Fn(&str) -> Option<GremlinValue>,
) -> Option<GremlinValue> {
    let s = s.trim();
    // Handle Set notation: {"alice"} → List (treated as set)
    if s.starts_with('{') && s.ends_with('}') {
        let inner = &s[1..s.len() - 1];
        let items: Vec<GremlinValue> = inner
            .split(',')
            .filter_map(|item| {
                let item = item.trim().trim_matches('"');
                if item.is_empty() {
                    None
                } else {
                    Some(GremlinValue::String(item.to_string()))
                }
            })
            .collect();
        return Some(GremlinValue::List(items));
    }
    // Handle List or Map notation: ["x","y"] or [key:["val1"],key2:["val2"]]
    // Delegate to parse_single_value which handles both list and map-with-colon syntax.
    if s.starts_with('[') && s.ends_with(']') {
        return parse_single_value(s, resolve_param).ok();
    }
    // Handle "Set" keyword: skip the operator arg
    if s == "Set" || s.starts_with("Operator.") {
        return None;
    }
    parse_literal_value(s, resolve_param)
}
