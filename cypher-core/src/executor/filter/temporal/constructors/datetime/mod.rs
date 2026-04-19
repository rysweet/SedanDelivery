//! `datetime()` and `localdatetime()` constructors.
//!
//! Decomposed by constructor variant:
//! - [`realtime`]: zero-arg "now" entry points (`builtin_temporal_realtime_*`)
//! - [`from_epoch`]: `datetime.fromEpoch` and `datetime.fromEpochMillis`
//! - [`datetime_constructor`]: full `datetime()` evaluator (string, value,
//!   map inputs with timezone resolution)
//! - [`localdatetime_constructor`]: full `localdatetime()` evaluator

mod datetime_constructor;
mod from_epoch;
mod localdatetime_constructor;
mod realtime;

pub(crate) use self::datetime_constructor::eval_datetime_constructor;
pub(crate) use self::from_epoch::{builtin_datetime_fromepoch, builtin_datetime_fromepochmillis};
pub(crate) use self::localdatetime_constructor::eval_localdatetime_constructor;
pub(crate) use self::realtime::{
    builtin_temporal_realtime_datetime, builtin_temporal_realtime_localdatetime,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Value;
    use indexmap::IndexMap;

    #[test]
    fn datetime_from_string() {
        match eval_datetime_constructor(&[Value::Str("2015-07-21T21:40:32+01:00".into())]).unwrap()
        {
            Value::DateTime(z) => assert_eq!(z.year(), 2015),
            _ => panic!("DT"),
        }
    }
    #[test]
    fn datetime_from_null() {
        assert_eq!(
            eval_datetime_constructor(&[Value::Null]).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn ldt_from_string() {
        match eval_localdatetime_constructor(&[Value::Str("2024-03-15T10:30:00".into())]).unwrap() {
            Value::LocalDateTime(dt) => assert_eq!(dt.year(), 2024),
            _ => panic!("LDT"),
        }
    }
    #[test]
    fn ldt_from_null() {
        assert_eq!(
            eval_localdatetime_constructor(&[Value::Null]).unwrap(),
            Value::Null
        );
    }
    #[test]
    fn fromepoch_zero() {
        match builtin_datetime_fromepoch(vec![Value::Int(0)]).unwrap() {
            Value::DateTime(z) => assert_eq!(z.year(), 1970),
            _ => panic!("DT"),
        }
    }
    #[test]
    fn fromepoch_bad() {
        assert!(builtin_datetime_fromepoch(vec![]).is_err());
    }
    #[test]
    fn fromepochmillis_zero() {
        match builtin_datetime_fromepochmillis(vec![Value::Int(0)]).unwrap() {
            Value::DateTime(z) => assert_eq!(z.year(), 1970),
            _ => panic!("DT"),
        }
    }
    #[test]
    fn datetime_no_args_returns_now() {
        match eval_datetime_constructor(&[]).unwrap() {
            Value::DateTime(_) => {}
            _ => panic!("DT"),
        }
    }
    #[test]
    fn datetime_too_many_args_errs() {
        assert!(eval_datetime_constructor(&[Value::Null, Value::Null]).is_err());
    }
    #[test]
    fn datetime_from_date_value() {
        let d = jiff::civil::Date::new(2024, 1, 2).unwrap();
        match eval_datetime_constructor(&[Value::Date(d)]).unwrap() {
            Value::DateTime(z) => {
                assert_eq!(z.year(), 2024);
                assert_eq!(z.month(), 1);
            }
            _ => panic!("DT"),
        }
    }
    #[test]
    fn datetime_from_localdatetime_value() {
        let dt = jiff::civil::DateTime::new(2024, 1, 2, 3, 4, 5, 0).unwrap();
        match eval_datetime_constructor(&[Value::LocalDateTime(dt)]).unwrap() {
            Value::DateTime(z) => assert_eq!(z.hour(), 3),
            _ => panic!("DT"),
        }
    }
    #[test]
    fn datetime_from_datetime_roundtrip() {
        let s = "2020-06-01T12:30:00+02:00[Europe/Berlin]";
        let z = s.parse::<jiff::Zoned>().unwrap();
        let v = eval_datetime_constructor(&[Value::DateTime(Box::new(z.clone()))]).unwrap();
        if let Value::DateTime(z2) = v {
            assert_eq!(z2.year(), 2020);
        } else {
            panic!()
        }
    }
    #[test]
    fn datetime_from_map_year_month_day() {
        let mut m = IndexMap::new();
        m.insert("year".into(), Value::Int(2030));
        m.insert("month".into(), Value::Int(5));
        m.insert("day".into(), Value::Int(6));
        m.insert("hour".into(), Value::Int(7));
        m.insert("minute".into(), Value::Int(8));
        m.insert("second".into(), Value::Int(9));
        let v = eval_datetime_constructor(&[Value::Map(m)]).unwrap();
        if let Value::DateTime(z) = v {
            assert_eq!(z.year(), 2030);
            assert_eq!(z.hour(), 7);
        } else {
            panic!()
        }
    }
    #[test]
    fn datetime_from_map_with_timezone() {
        let mut m = IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        m.insert("month".into(), Value::Int(1));
        m.insert("day".into(), Value::Int(1));
        m.insert("timezone".into(), Value::Str("UTC".into()));
        assert!(matches!(
            eval_datetime_constructor(&[Value::Map(m)]).unwrap(),
            Value::DateTime(_)
        ));
    }
    #[test]
    fn datetime_from_map_with_milli_micro_nano() {
        let mut m = IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        m.insert("month".into(), Value::Int(1));
        m.insert("day".into(), Value::Int(1));
        m.insert("millisecond".into(), Value::Int(1));
        m.insert("microsecond".into(), Value::Int(2));
        m.insert("nanosecond".into(), Value::Int(3));
        assert!(matches!(
            eval_datetime_constructor(&[Value::Map(m)]).unwrap(),
            Value::DateTime(_)
        ));
    }
    #[test]
    fn datetime_from_int_errs() {
        assert!(eval_datetime_constructor(&[Value::Int(0)]).is_err());
    }
    #[test]
    fn datetime_invalid_string_errs() {
        assert!(eval_datetime_constructor(&[Value::Str("not a date".into())]).is_err());
    }
    #[test]
    fn ldt_no_args_returns_now() {
        match eval_localdatetime_constructor(&[]).unwrap() {
            Value::LocalDateTime(_) => {}
            _ => panic!("LDT"),
        }
    }
    #[test]
    fn ldt_too_many_args_errs() {
        assert!(eval_localdatetime_constructor(&[Value::Null, Value::Null]).is_err());
    }
    #[test]
    fn ldt_from_int_errs() {
        assert!(eval_localdatetime_constructor(&[Value::Int(0)]).is_err());
    }
    #[test]
    fn ldt_from_date() {
        let d = jiff::civil::Date::new(2024, 5, 6).unwrap();
        match eval_localdatetime_constructor(&[Value::Date(d)]).unwrap() {
            Value::LocalDateTime(dt) => assert_eq!(dt.year(), 2024),
            _ => panic!(),
        }
    }
    #[test]
    fn ldt_from_datetime() {
        let z: jiff::Zoned = "2024-05-06T07:08:09+01:00[Europe/London]".parse().unwrap();
        match eval_localdatetime_constructor(&[Value::DateTime(Box::new(z))]).unwrap() {
            Value::LocalDateTime(dt) => assert_eq!(dt.year(), 2024),
            _ => panic!(),
        }
    }
    #[test]
    fn ldt_from_map() {
        let mut m = IndexMap::new();
        m.insert("year".into(), Value::Int(2024));
        m.insert("month".into(), Value::Int(2));
        m.insert("day".into(), Value::Int(3));
        m.insert("hour".into(), Value::Int(4));
        m.insert("minute".into(), Value::Int(5));
        m.insert("second".into(), Value::Int(6));
        assert!(matches!(
            eval_localdatetime_constructor(&[Value::Map(m)]).unwrap(),
            Value::LocalDateTime(_)
        ));
    }
    #[test]
    fn realtime_datetime_works() {
        assert!(matches!(
            builtin_temporal_realtime_datetime(vec![]).unwrap(),
            Value::DateTime(_)
        ));
        assert!(matches!(
            builtin_temporal_realtime_localdatetime(vec![]).unwrap(),
            Value::LocalDateTime(_)
        ));
    }
    #[test]
    fn fromepoch_with_nanos() {
        match builtin_datetime_fromepoch(vec![Value::Int(1), Value::Int(500)]).unwrap() {
            Value::DateTime(z) => assert_eq!(z.year(), 1970),
            _ => panic!(),
        }
    }
    #[test]
    fn fromepoch_null_input() {
        assert!(builtin_datetime_fromepoch(vec![Value::Null]).is_err());
        assert!(builtin_datetime_fromepochmillis(vec![Value::Null]).is_err());
    }
    #[test]
    fn fromepoch_float_input() {
        assert!(builtin_datetime_fromepoch(vec![Value::Float(1.5)]).is_err());
    }
}
