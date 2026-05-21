//! Low-level tokenization: balanced-paren scanning, step splitting, predicate chains.

use super::*;

pub(crate) fn update_string_state(
    ch: char,
    string_char: &mut Option<char>,
    escape: &mut bool,
) -> bool {
    if *escape {
        *escape = false;
        return true;
    }
    if let Some(q) = *string_char {
        if ch == '\\' {
            *escape = true;
            return true;
        }
        if ch == q {
            *string_char = None;
        }
        return true;
    }
    if ch == '"' || ch == '\'' {
        *string_char = Some(ch);
        return true;
    }
    false
}

/// Skip past balanced parentheses.  Input starts just after the opening `(`.
/// Returns the slice after the matching `)` and any trailing `.`.
pub(crate) fn skip_balanced_parens(input: &str) -> &str {
    let mut depth = 1u32;
    let mut string_char: Option<char> = None;
    let mut escape = false;
    for (i, ch) in input.char_indices() {
        if update_string_state(ch, &mut string_char, &mut escape) {
            continue;
        }
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                return &input[i + 1..];
            }
        }
    }
    input
}

/// Split a post-`g.` traversal string into step tokens like `V()`, `has("name","marko")`, etc.
///
/// Splits on `.` while respecting parentheses, strings, and nested `__.*` anonymous traversals.
/// Trims whitespace from each token (multiline traversals have `\n` and leading spaces).
pub(crate) fn tokenize_steps(input: &str) -> Result<Vec<String>, TextParseError> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut depth = 0u32;
    let mut string_char: Option<char> = None;
    let mut escape = false;

    for ch in input.chars() {
        if update_string_state(ch, &mut string_char, &mut escape) {
            current.push(ch);
            continue;
        }
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                if depth == 0 {
                    return Err(TextParseError::InvalidArgument(
                        "unbalanced parentheses: unexpected ')'".into(),
                    ));
                }
                depth -= 1;
                current.push(ch);
            }
            '.' if depth == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    tokens.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(ch);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        tokens.push(trimmed);
    }
    if depth != 0 {
        return Err(TextParseError::InvalidArgument(
            "unbalanced parentheses: missing ')'".into(),
        ));
    }
    Ok(tokens)
}

/// Parse a single step token (e.g. `has("name","marko")`) into GremlinStep(s).
pub(crate) fn split_step_name_args(token: &str) -> (&str, &str) {
    if let Some(paren_pos) = token.find('(') {
        let name = &token[..paren_pos];
        // Strip outer parens
        let inner = &token[paren_pos + 1..];
        let inner = inner.strip_suffix(')').unwrap_or(inner);
        (name, inner)
    } else {
        (token, "")
    }
}

/// Parse the `has(...)` step which has multiple overloads:
/// - `has("key")` — property existence
/// - `has("key", value)` — property equals
/// - `has("key", P.gt(30))` — predicate
/// - `has("label", "key", value)` — label + key + value
pub(crate) fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut string_char: Option<char> = None;
    let mut escape = false;
    for (i, ch) in s.char_indices() {
        if update_string_state(ch, &mut string_char, &mut escape) {
            continue;
        }
        if ch == '(' {
            depth += 1;
        } else if ch == ')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Try to parse a predicate call like `P.gt(30)`, `P.within("a","b")`, etc.
/// Also handles bare predicates like `eq(30)`, `gt(30)`.
pub(crate) fn split_predicate_chain(s: &str) -> Vec<(&str, &str)> {
    let mut parts = Vec::new();
    let mut depth = 0u32;
    let mut last_start = 0;
    let mut last_op = "first";
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut in_string: Option<u8> = None;
    let mut escape = false;
    while i < bytes.len() {
        // Byte-level string/escape tracking (mirrors update_string_state).
        if escape {
            escape = false;
            i += 1;
            continue;
        }
        if let Some(q) = in_string {
            if bytes[i] == b'\\' {
                escape = true;
            } else if bytes[i] == q {
                in_string = None;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            in_string = Some(bytes[i]);
            i += 1;
            continue;
        }
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && i + 1 < bytes.len() {
                    // Check if followed by .and( or .or(
                    let rest = &s[i + 1..];
                    if rest.starts_with(".and(") {
                        let part = &s[last_start..=i];
                        // Non-first parts have an extra trailing ) from the
                        // .and()/.or() wrapper — strip it.
                        let part = if last_op != "first" {
                            part.strip_suffix(')').unwrap_or(part)
                        } else {
                            part
                        };
                        parts.push((last_op, part));
                        last_op = "and";
                        // skip ".and(" — 5 chars from rest start (i+1), content at i+6
                        last_start = i + 6;
                        i += 6;
                        depth = 1;
                        continue;
                    } else if rest.starts_with(".or(") {
                        let part = &s[last_start..=i];
                        let part = if last_op != "first" {
                            part.strip_suffix(')').unwrap_or(part)
                        } else {
                            part
                        };
                        parts.push((last_op, part));
                        last_op = "or";
                        // skip ".or(" — 4 chars from rest start (i+1), content at i+5
                        last_start = i + 5;
                        i += 5;
                        depth = 1;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }
    if last_start < s.len() {
        let end = s.len();
        // Remove trailing ) that was part of the .and()/.or() wrapper
        let part = &s[last_start..end];
        let part = part.strip_suffix(')').unwrap_or(part);
        parts.push((last_op, part));
    }
    // If only one part (no chaining), return empty to signal no chain
    if parts.len() <= 1 && last_op == "first" {
        return vec![];
    }
    parts
}

/// Parse a predicate from an `is(...)` argument.
pub(crate) fn split_top_level_args(s: &str) -> Vec<&str> {
    if s.trim().is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut depth = 0u32;
    let mut string_char: Option<char> = None;
    let mut escape = false;
    let mut start = 0;

    for (i, ch) in s.char_indices() {
        if update_string_state(ch, &mut string_char, &mut escape) {
            continue;
        }
        match ch {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                result.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    result.push(&s[start..]);
    result
}
