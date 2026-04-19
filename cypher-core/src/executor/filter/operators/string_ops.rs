//! String predicates: `STARTS WITH`, `ENDS WITH`, `CONTAINS`, `=~` (regex).

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use crate::executor::filter::REGEX_CACHE;

pub(super) fn starts_with(lv: Value, rv: Value) -> Value {
    match (lv, rv) {
        (Value::Null, _) | (_, Value::Null) => Value::Null,
        (Value::Str(s), Value::Str(prefix)) => Value::Bool(s.starts_with(prefix.as_ref())),
        // Type mismatch in string predicates returns null in Cypher
        (_, _) => Value::Null,
    }
}

pub(super) fn ends_with(lv: Value, rv: Value) -> Value {
    match (lv, rv) {
        (Value::Null, _) | (_, Value::Null) => Value::Null,
        (Value::Str(s), Value::Str(suffix)) => Value::Bool(s.ends_with(suffix.as_ref())),
        (_, _) => Value::Null,
    }
}

pub(super) fn contains(lv: Value, rv: Value) -> Value {
    match (lv, rv) {
        (Value::Null, _) | (_, Value::Null) => Value::Null,
        (Value::Str(s), Value::Str(needle)) => Value::Bool(s.contains(needle.as_ref())),
        (_, _) => Value::Null,
    }
}

pub(super) fn regex_match(lv: Value, rv: Value) -> Result<Value, GqlError> {
    match (lv, rv) {
        (Value::Null, _) | (_, Value::Null) => Ok(Value::Null),
        (Value::Str(s), Value::Str(pattern)) => {
            let max_pattern_len = crate::config::global().max_regex_pattern_len;
            if pattern.len() > max_pattern_len {
                return Err(GqlError::Execution(ExecutionError::TypeMismatch {
                    expected: format!("regex pattern <= {max_pattern_len} bytes"),
                    actual: format!("{} bytes", pattern.len()),
                }));
            }
            // Cypher's =~ operator requires the pattern to match the ENTIRE
            // string (implicit ^..$ anchoring), but regex::is_match() succeeds
            // on substring matches.  Wrap with ^(?:...)$ so the compiled regex
            // enforces a full-string match.
            let anchored = format!("^(?:{})$", pattern);
            let matched = REGEX_CACHE.with(|cache| -> Result<bool, GqlError> {
                let mut map = cache.borrow_mut();
                if !map.contains_key(anchored.as_str()) {
                    let re = regex::Regex::new(&anchored).map_err(|e| {
                        GqlError::Execution(ExecutionError::TypeMismatch {
                            expected: "valid regex pattern".to_string(),
                            actual: e.to_string(),
                        })
                    })?;
                    if map.len() >= crate::config::global().regex_cache_capacity {
                        // Evict the oldest half instead of clearing everything.
                        // This avoids a "cache cliff" where all compiled regexes
                        // are dropped simultaneously, causing a spike of recompilation.
                        let half = map.len() / 2;
                        map.drain(..half);
                    }
                    map.insert(anchored.to_string(), re);
                }
                Ok(map[anchored.as_str()].is_match(s.as_ref()))
            })?;
            Ok(Value::Bool(matched))
        }
        (l, r) => Err(GqlError::Execution(ExecutionError::InvalidOperator {
            op: "=~".to_string(),
            left: l.type_name().to_string(),
            right: r.type_name().to_string(),
        })),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_utils::{bin, lit_i, lit_null, lit_s, run};
    use crate::ast::BinaryOp;
    use crate::types::Value;

    #[test]
    fn starts_with_basic() {
        assert_eq!(
            run(&bin(BinaryOp::StartsWith, lit_s("abc"), lit_s("ab"))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::StartsWith, lit_null(), lit_s("a"))).unwrap(),
            Value::Null
        );
        assert_eq!(
            run(&bin(BinaryOp::StartsWith, lit_i(1), lit_s("a"))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn ends_with_basic() {
        assert_eq!(
            run(&bin(BinaryOp::EndsWith, lit_s("abc"), lit_s("bc"))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::EndsWith, lit_null(), lit_s("a"))).unwrap(),
            Value::Null
        );
        assert_eq!(
            run(&bin(BinaryOp::EndsWith, lit_i(1), lit_s("a"))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn contains_basic() {
        assert_eq!(
            run(&bin(BinaryOp::Contains, lit_s("hello world"), lit_s("o w"))).unwrap(),
            Value::Bool(true)
        );
        assert_eq!(
            run(&bin(BinaryOp::Contains, lit_null(), lit_s("a"))).unwrap(),
            Value::Null
        );
        assert_eq!(
            run(&bin(BinaryOp::Contains, lit_i(1), lit_s("a"))).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn regex_match() {
        assert_eq!(
            run(&bin(BinaryOp::RegexMatch, lit_s("hello"), lit_s("h.*o"))).unwrap(),
            Value::Bool(true)
        );
        // anchored: must match entire string
        assert_eq!(
            run(&bin(
                BinaryOp::RegexMatch,
                lit_s("hello world"),
                lit_s("h.*o")
            ))
            .unwrap(),
            Value::Bool(false)
        );
        assert_eq!(
            run(&bin(BinaryOp::RegexMatch, lit_null(), lit_s("a"))).unwrap(),
            Value::Null
        );
        assert!(run(&bin(BinaryOp::RegexMatch, lit_i(1), lit_s("a"))).is_err());
        // bad pattern
        assert!(run(&bin(BinaryOp::RegexMatch, lit_s("a"), lit_s("("))).is_err());
    }
}
