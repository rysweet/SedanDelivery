//! date() constructor.

use crate::error::{ExecutionError, GqlError};
use crate::types::Value;

use super::super::super::builtins::type_error;
use super::super::super::get_query_now;
use super::super::parsing::{large_date_value, parse_date_str, parse_large_date_str};
use super::super::temporal_parse_error;
use super::{extract_base_date, resolve_date_from_map};

pub(crate) fn builtin_temporal_realtime_date(vals: Vec<Value>) -> Result<Value, GqlError> {
    if vals.first().map(|v| v.is_null()).unwrap_or(false) {
        return Ok(Value::Null);
    }
    if !vals.is_empty() {
        return eval_date_constructor(&vals);
    }
    let today = jiff::civil::Date::from(get_query_now());
    Ok(Value::Date(today))
}

pub(crate) fn eval_date_constructor(vals: &[Value]) -> Result<Value, GqlError> {
    if vals.is_empty() {
        let today = jiff::civil::Date::from(get_query_now());
        return Ok(Value::Date(today));
    }
    if vals.len() != 1 {
        return Err(GqlError::Execution(ExecutionError::TypeMismatch {
            expected: "date() expects 0 or 1 argument".to_string(),
            actual: format!("{} arguments", vals.len()),
        }));
    }
    match &vals[0] {
        Value::Null => Ok(Value::Null),
        Value::Str(s) => {
            match parse_date_str(s) {
                Ok(d) => Ok(Value::Date(d)),
                Err(_) => {
                    if let Some((y, m, d)) = parse_large_date_str(s) {
                        Ok(large_date_value(y, m, d))
                    } else {
                        Err(temporal_parse_error(
                            "date",
                            format!("cannot parse date from '{s}'"),
                        ))
                    }
                }
            }
        }
        Value::Date(d) => Ok(Value::Date(*d)),
        Value::LocalDateTime(dt) => Ok(Value::Date(
            jiff::civil::Date::new(dt.year(), dt.month(), dt.day())
                .map_err(|e| temporal_parse_error("date", e))?,
        )),
        Value::DateTime(zdt) => {
            let dt = jiff::civil::DateTime::from(zdt.as_ref());
            Ok(Value::Date(
                jiff::civil::Date::new(dt.year(), dt.month(), dt.day())
                    .map_err(|e| temporal_parse_error("date", e))?,
            ))
        }
        Value::Map(m) => {
            let base_date = extract_base_date(m)?;
            let d = resolve_date_from_map(m, &base_date)?;
            Ok(Value::Date(d))
        }
        other => type_error("date", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_from_string() {
        match eval_date_constructor(&[Value::Str("2024-03-15".into())]).unwrap() {
            Value::Date(d) => {
                assert_eq!(d.year(), 2024);
                assert_eq!(d.day(), 15);
            }
            _ => panic!("expected Date"),
        }
    }
    #[test]
    fn date_from_null() {
        assert_eq!(eval_date_constructor(&[Value::Null]).unwrap(), Value::Null);
    }
    #[test]
    fn date_from_map() {
        let mut m = indexmap::IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        m.insert("month".into(), Value::Int(6));
        m.insert("day".into(), Value::Int(15));
        match eval_date_constructor(&[Value::Map(m)]).unwrap() {
            Value::Date(d) => assert_eq!(d.day(), 15),
            _ => panic!("Date"),
        }
    }
    #[test]
    fn date_too_many() {
        assert!(eval_date_constructor(&[Value::Int(1), Value::Int(2)]).is_err());
    }
    #[test]
    fn date_large_year() {
        assert!(matches!(
            eval_date_constructor(&[Value::Str("-999999999-01-01".into())]).unwrap(),
            Value::Map(_)
        ));
    }
}
