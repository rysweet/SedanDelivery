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
        Value::Str(s) => match parse_date_str(s) {
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
        },
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

    fn dt_utc(y: i16, mo: i8, d: i8) -> Value {
        Value::DateTime(Box::new(
            jiff::civil::DateTime::new(y, mo, d, 0, 0, 0, 0)
                .unwrap()
                .to_zoned(jiff::tz::TimeZone::UTC)
                .unwrap(),
        ))
    }

    #[test]
    fn date_realtime() {
        assert!(matches!(
            builtin_temporal_realtime_date(vec![]).unwrap(),
            Value::Date(_)
        ));
        assert_eq!(
            builtin_temporal_realtime_date(vec![Value::Null]).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn date_no_args_is_today() {
        assert!(matches!(
            eval_date_constructor(&[]).unwrap(),
            Value::Date(_)
        ));
    }
    #[test]
    fn date_from_date_value() {
        let d = Value::Date(jiff::civil::Date::new(2020, 1, 1).unwrap());
        assert_eq!(eval_date_constructor(std::slice::from_ref(&d)).unwrap(), d);
    }
    #[test]
    fn date_from_localdatetime() {
        let v = Value::LocalDateTime(jiff::civil::DateTime::new(2020, 6, 15, 1, 2, 3, 0).unwrap());
        if let Value::Date(d) = eval_date_constructor(&[v]).unwrap() {
            assert_eq!(d.year(), 2020);
            assert_eq!(d.day(), 15);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_from_datetime() {
        if let Value::Date(d) = eval_date_constructor(&[dt_utc(2020, 6, 15)]).unwrap() {
            assert_eq!(d.day(), 15);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_from_int_errs() {
        assert!(eval_date_constructor(&[Value::Int(0)]).is_err());
    }
    #[test]
    fn date_invalid_string_errs() {
        assert!(eval_date_constructor(&[Value::Str("not-a-date".into())]).is_err());
    }
    #[test]
    fn date_from_map_with_week() {
        let mut m = indexmap::IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        m.insert("week".into(), Value::Int(2));
        m.insert("dayOfWeek".into(), Value::Int(3));
        assert!(matches!(
            eval_date_constructor(&[Value::Map(m)]).unwrap(),
            Value::Date(_)
        ));
    }
    #[test]
    fn date_from_map_with_week_no_dow() {
        let mut m = indexmap::IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        m.insert("week".into(), Value::Int(2));
        assert!(matches!(
            eval_date_constructor(&[Value::Map(m)]).unwrap(),
            Value::Date(_)
        ));
    }
    #[test]
    fn date_from_map_with_ordinal_day() {
        let mut m = indexmap::IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        m.insert("ordinalDay".into(), Value::Int(60));
        if let Value::Date(d) = eval_date_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(d.year(), 2024);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_from_map_with_quarter_and_dayofquarter() {
        let mut m = indexmap::IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        m.insert("quarter".into(), Value::Int(2));
        m.insert("dayOfQuarter".into(), Value::Int(15));
        assert!(matches!(
            eval_date_constructor(&[Value::Map(m)]).unwrap(),
            Value::Date(_)
        ));
    }
    #[test]
    fn date_from_map_with_quarter_only() {
        let mut m = indexmap::IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        m.insert("quarter".into(), Value::Int(3));
        if let Value::Date(d) = eval_date_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(d.month(), 7);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_from_map_with_base_date_and_quarter() {
        let mut m = indexmap::IndexMap::new();
        m.insert(
            "date".into(),
            Value::Date(jiff::civil::Date::new(2024, 5, 20).unwrap()),
        );
        m.insert("quarter".into(), Value::Int(3));
        // base month=5 -> month_in_quarter=1, new = (3-1)*3+1+1 = 8
        if let Value::Date(d) = eval_date_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(d.month(), 8);
            assert_eq!(d.day(), 20);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_from_map_with_base_date_inherits_year() {
        let mut m = indexmap::IndexMap::new();
        m.insert(
            "date".into(),
            Value::Date(jiff::civil::Date::new(2030, 5, 20).unwrap()),
        );
        m.insert("month".into(), Value::Int(8));
        if let Value::Date(d) = eval_date_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(d.year(), 2030);
            assert_eq!(d.month(), 8);
            assert_eq!(d.day(), 20);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_from_map_with_base_date_localdatetime() {
        let mut m = indexmap::IndexMap::new();
        m.insert(
            "date".into(),
            Value::LocalDateTime(jiff::civil::DateTime::new(2030, 5, 20, 0, 0, 0, 0).unwrap()),
        );
        m.insert("day".into(), Value::Int(10));
        if let Value::Date(d) = eval_date_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(d.day(), 10);
            assert_eq!(d.year(), 2030);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_from_map_with_base_date_datetime() {
        let mut m = indexmap::IndexMap::new();
        m.insert("date".into(), dt_utc(2030, 5, 20));
        m.insert("day".into(), Value::Int(10));
        if let Value::Date(d) = eval_date_constructor(&[Value::Map(m)]).unwrap() {
            assert_eq!(d.day(), 10);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_from_map_with_base_date_null() {
        let mut m = indexmap::IndexMap::new();
        m.insert("date".into(), Value::Null);
        m.insert("year".into(), Value::Int(2024));
        m.insert("month".into(), Value::Int(6));
        m.insert("day".into(), Value::Int(15));
        assert!(matches!(
            eval_date_constructor(&[Value::Map(m)]).unwrap(),
            Value::Date(_)
        ));
    }
    #[test]
    fn date_from_map_invalid_base_date_errs() {
        let mut m = indexmap::IndexMap::new();
        m.insert("date".into(), Value::Int(0));
        assert!(eval_date_constructor(&[Value::Map(m)]).is_err());
    }
    #[test]
    fn date_from_map_week_with_base_date() {
        // omit year — should pull from base_date.iso_week_date()
        let mut m = indexmap::IndexMap::new();
        m.insert(
            "date".into(),
            Value::Date(jiff::civil::Date::new(2024, 6, 15).unwrap()),
        );
        m.insert("week".into(), Value::Int(2));
        assert!(matches!(
            eval_date_constructor(&[Value::Map(m)]).unwrap(),
            Value::Date(_)
        ));
    }
    #[test]
    fn date_from_map_ordinalday_with_base_date() {
        let mut m = indexmap::IndexMap::new();
        m.insert(
            "date".into(),
            Value::Date(jiff::civil::Date::new(2024, 6, 15).unwrap()),
        );
        m.insert("ordinalDay".into(), Value::Int(60));
        assert!(matches!(
            eval_date_constructor(&[Value::Map(m)]).unwrap(),
            Value::Date(_)
        ));
    }
    #[test]
    fn date_from_map_with_empty_map() {
        let m = indexmap::IndexMap::new();
        assert!(matches!(
            eval_date_constructor(&[Value::Map(m)]).unwrap(),
            Value::Date(_)
        ));
    }
}
