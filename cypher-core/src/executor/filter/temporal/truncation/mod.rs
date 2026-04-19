//! Temporal truncation functions (date.truncate, datetime.truncate, etc.)
//!
//! Submodules:
//! - [`extract`]: extract date/time/timezone components from temporal values.
//! - [`units`]: per-unit truncation logic (year, month, week, hour, …).
//! - [`overrides`]: apply map-shaped field overrides and value conversions.
//! - [`eval`]: public `eval_*_truncate` entry points used by the temporal builtins.

mod eval;
mod extract;
mod overrides;
mod units;

pub(crate) use eval::{
    eval_date_truncate, eval_datetime_truncate, eval_localdatetime_truncate,
    eval_localtime_truncate, eval_time_truncate,
};

#[cfg(test)]
mod tests {
    use super::eval::*;
    use super::extract::*;
    use super::overrides::*;
    use super::units::*;
    use crate::types::Value;

    fn date_val(y: i16, m: i8, d: i8) -> Value {
        Value::Date(jiff::civil::Date::new(y, m, d).unwrap())
    }
    fn ldt_val(y: i16, m: i8, d: i8, h: i8, mi: i8, s: i8) -> Value {
        Value::LocalDateTime(jiff::civil::DateTime::new(y, m, d, h, mi, s, 0).unwrap())
    }
    fn dt_val(y: i16, m: i8, d: i8, h: i8, mi: i8, s: i8) -> Value {
        Value::DateTime(Box::new(
            jiff::civil::DateTime::new(y, m, d, h, mi, s, 0)
                .unwrap()
                .to_zoned(jiff::tz::TimeZone::UTC)
                .unwrap(),
        ))
    }
    fn lt_val(h: i8, m: i8, s: i8) -> Value {
        Value::LocalTime(jiff::civil::Time::new(h, m, s, 0).unwrap())
    }
    fn time_val(h: i8, m: i8, s: i8, off: i32) -> Value {
        Value::Time(jiff::civil::Time::new(h, m, s, 0).unwrap(), off)
    }

    #[test]
    fn trunc_year() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "year").unwrap(),
            jiff::civil::Date::new(2024, 1, 1).unwrap()
        );
    }
    #[test]
    fn trunc_quarter() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 5, 15).unwrap(), "quarter").unwrap(),
            jiff::civil::Date::new(2024, 4, 1).unwrap()
        );
    }
    #[test]
    fn trunc_month() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "month").unwrap(),
            jiff::civil::Date::new(2024, 6, 1).unwrap()
        );
    }
    #[test]
    fn trunc_week() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "week").unwrap(),
            jiff::civil::Date::new(2024, 6, 10).unwrap()
        );
    }
    #[test]
    fn trunc_millennium() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "millennium")
                .unwrap()
                .year(),
            2000
        );
    }
    #[test]
    fn trunc_unknown() {
        assert!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "unknown").is_err()
        );
    }
    #[test]
    fn trunc_time_day() {
        assert_eq!(
            truncate_time_fields(
                jiff::civil::Time::new(14, 30, 45, 123456789).unwrap(),
                "day"
            )
            .unwrap(),
            jiff::civil::Time::midnight()
        );
    }
    #[test]
    fn trunc_time_hour() {
        let r =
            truncate_time_fields(jiff::civil::Time::new(14, 30, 45, 0).unwrap(), "hour").unwrap();
        assert_eq!(r.hour(), 14);
        assert_eq!(r.minute(), 0);
    }
    #[test]
    fn trunc_time_ms() {
        assert_eq!(
            truncate_time_fields(
                jiff::civil::Time::new(14, 30, 45, 123456789).unwrap(),
                "millisecond"
            )
            .unwrap()
            .subsec_nanosecond(),
            123_000_000
        );
    }
    #[test]
    fn trunc_time_unknown() {
        assert!(
            truncate_time_fields(jiff::civil::Time::new(14, 30, 0, 0).unwrap(), "unknown").is_err()
        );
    }
    #[test]
    fn eval_date_year() {
        assert_eq!(
            eval_date_truncate(&[Value::Str("year".into()), date_val(2024, 6, 15)]).unwrap(),
            date_val(2024, 1, 1)
        );
    }
    #[test]
    fn eval_date_bad() {
        assert!(eval_date_truncate(&[Value::Str("year".into())]).is_err());
    }
    #[test]
    fn eval_lt_hour() {
        match eval_localtime_truncate(&[Value::Str("hour".into()), lt_val(14, 30, 45)]).unwrap() {
            Value::LocalTime(t) => {
                assert_eq!(t.hour(), 14);
                assert_eq!(t.minute(), 0);
            }
            _ => panic!("LT"),
        }
    }
    #[test]
    fn eval_ldt_year() {
        match eval_localdatetime_truncate(&[
            Value::Str("year".into()),
            ldt_val(2024, 6, 15, 14, 30, 45),
        ])
        .unwrap()
        {
            Value::LocalDateTime(dt) => {
                assert_eq!(dt.year(), 2024);
                assert_eq!(dt.month(), 1);
                assert_eq!(dt.hour(), 0);
            }
            _ => panic!("LDT"),
        }
    }
    #[test]
    fn eval_dt_year() {
        match eval_datetime_truncate(&[Value::Str("year".into()), dt_val(2024, 6, 15, 14, 30, 45)])
            .unwrap()
        {
            Value::DateTime(z) => {
                assert_eq!(z.year(), 2024);
                assert_eq!(z.month(), 1);
            }
            _ => panic!("DT"),
        }
    }
    #[test]
    fn eval_time_hour() {
        match eval_time_truncate(&[Value::Str("hour".into()), time_val(14, 30, 45, 3600)]).unwrap()
        {
            Value::Time(t, off) => {
                assert_eq!(t.hour(), 14);
                assert_eq!(t.minute(), 0);
                assert_eq!(off, 3600);
            }
            _ => panic!("T"),
        }
    }
    #[test]
    fn day_override() {
        let mut m = indexmap::IndexMap::new();
        m.insert("day".into(), Value::Int(15));
        assert_eq!(
            apply_date_overrides(jiff::civil::Date::new(2024, 6, 1).unwrap(), &m)
                .unwrap()
                .day(),
            15
        );
    }
    #[test]
    fn hour_override() {
        let mut m = indexmap::IndexMap::new();
        m.insert("hour".into(), Value::Int(10));
        assert_eq!(
            apply_time_overrides(jiff::civil::Time::new(0, 0, 0, 0).unwrap(), &m)
                .unwrap()
                .hour(),
            10
        );
    }
    #[test]
    fn extract_date_ok() {
        assert!(extract_date(&date_val(2024, 6, 15)).is_some());
    }
    #[test]
    fn extract_date_fail() {
        assert!(extract_date(&Value::Int(42)).is_none());
    }
    #[test]
    fn extract_tz_ok() {
        assert!(extract_tz(&time_val(10, 0, 0, 3600)).is_some());
    }
    #[test]
    fn extract_tz_fail() {
        assert!(extract_tz(&Value::Int(42)).is_none());
    }

    // ── eval_*_truncate public function tests ─────────────────────────────

    fn s(x: &str) -> Value {
        Value::Str(x.into())
    }

    #[test]
    fn date_truncate_year() {
        let r = eval_date_truncate(&[s("year"), date_val(2024, 6, 15)]).unwrap();
        assert_eq!(r, date_val(2024, 1, 1));
    }
    #[test]
    fn date_truncate_century() {
        let r = eval_date_truncate(&[s("century"), date_val(2024, 6, 15)]).unwrap();
        if let Value::Date(d) = r {
            assert_eq!(d.year(), 2000);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_truncate_decade() {
        let r = eval_date_truncate(&[s("decade"), date_val(2024, 6, 15)]).unwrap();
        if let Value::Date(d) = r {
            assert_eq!(d.year(), 2020);
        } else {
            panic!()
        }
    }
    #[test]
    fn date_truncate_with_overrides() {
        let mut m = indexmap::IndexMap::new();
        m.insert("day".into(), Value::Int(15));
        let r = eval_date_truncate(&[s("month"), date_val(2024, 6, 1), Value::Map(m)]).unwrap();
        assert_eq!(r, date_val(2024, 6, 15));
    }
    #[test]
    fn date_truncate_arity_errs() {
        assert!(eval_date_truncate(&[]).is_err());
        assert!(eval_date_truncate(&[s("year")]).is_err());
        assert!(
            eval_date_truncate(&[s("year"), date_val(2024, 1, 1), Value::Null, Value::Null])
                .is_err()
        );
    }
    #[test]
    fn date_truncate_unit_not_string_errs() {
        assert!(eval_date_truncate(&[Value::Int(0), date_val(2024, 1, 1)]).is_err());
    }
    #[test]
    fn date_truncate_no_date_errs() {
        assert!(eval_date_truncate(&[s("year"), Value::Int(0)]).is_err());
    }
    #[test]
    fn date_truncate_overrides_not_map_errs() {
        assert!(eval_date_truncate(&[s("year"), date_val(2024, 1, 1), Value::Int(0)]).is_err());
    }

    #[test]
    fn datetime_truncate_year_from_localdatetime() {
        let r = eval_datetime_truncate(&[s("year"), ldt_val(2024, 6, 15, 12, 30, 45)]).unwrap();
        assert!(matches!(r, Value::DateTime(_)));
    }
    #[test]
    fn datetime_truncate_with_tz_override() {
        let mut m = indexmap::IndexMap::new();
        m.insert("timezone".into(), Value::Str("UTC".into()));
        let r = eval_datetime_truncate(&[s("day"), dt_val(2024, 6, 15, 12, 0, 0), Value::Map(m)])
            .unwrap();
        assert!(matches!(r, Value::DateTime(_)));
    }
    #[test]
    fn datetime_truncate_keeps_input_tz() {
        let r = eval_datetime_truncate(&[s("month"), dt_val(2024, 6, 15, 12, 0, 0)]).unwrap();
        assert!(matches!(r, Value::DateTime(_)));
    }
    #[test]
    fn datetime_truncate_arity_errs() {
        assert!(eval_datetime_truncate(&[]).is_err());
    }
    #[test]
    fn datetime_truncate_unit_errs() {
        assert!(eval_datetime_truncate(&[Value::Int(0), dt_val(2024, 6, 15, 0, 0, 0)]).is_err());
    }
    #[test]
    fn datetime_truncate_no_date_errs() {
        assert!(eval_datetime_truncate(&[s("year"), Value::Int(0)]).is_err());
    }
    #[test]
    fn datetime_truncate_invalid_overrides_errs() {
        assert!(
            eval_datetime_truncate(&[s("year"), dt_val(2024, 6, 15, 0, 0, 0), Value::Int(0)])
                .is_err()
        );
    }

    #[test]
    fn localdatetime_truncate_works() {
        let r = eval_localdatetime_truncate(&[s("year"), ldt_val(2024, 6, 15, 12, 30, 0)]).unwrap();
        assert!(matches!(r, Value::LocalDateTime(_)));
    }
    #[test]
    fn localdatetime_truncate_with_overrides() {
        let mut m = indexmap::IndexMap::new();
        m.insert("hour".into(), Value::Int(5));
        let r = eval_localdatetime_truncate(&[
            s("day"),
            ldt_val(2024, 6, 15, 12, 30, 0),
            Value::Map(m),
        ])
        .unwrap();
        if let Value::LocalDateTime(dt) = r {
            assert_eq!(dt.hour(), 5);
        } else {
            panic!()
        }
    }
    #[test]
    fn localdatetime_truncate_arity_errs() {
        assert!(eval_localdatetime_truncate(&[]).is_err());
    }
    #[test]
    fn localdatetime_truncate_no_date_errs() {
        assert!(eval_localdatetime_truncate(&[s("year"), Value::Int(0)]).is_err());
    }
    #[test]
    fn localdatetime_truncate_unit_errs() {
        assert!(
            eval_localdatetime_truncate(&[Value::Int(0), ldt_val(2024, 6, 15, 0, 0, 0)]).is_err()
        );
    }
    #[test]
    fn localdatetime_truncate_overrides_errs() {
        assert!(eval_localdatetime_truncate(&[
            s("year"),
            ldt_val(2024, 6, 15, 0, 0, 0),
            Value::Int(0)
        ])
        .is_err());
    }

    #[test]
    fn localtime_truncate_works() {
        let r = eval_localtime_truncate(&[s("hour"), lt_val(14, 30, 45)]).unwrap();
        if let Value::LocalTime(t) = r {
            assert_eq!(t.hour(), 14);
            assert_eq!(t.minute(), 0);
        } else {
            panic!()
        }
    }
    #[test]
    fn localtime_truncate_arity_errs() {
        assert!(eval_localtime_truncate(&[]).is_err());
    }
    #[test]
    fn localtime_truncate_unit_errs() {
        assert!(eval_localtime_truncate(&[Value::Int(0), lt_val(1, 0, 0)]).is_err());
    }
    #[test]
    fn localtime_truncate_no_time_errs() {
        assert!(eval_localtime_truncate(&[s("hour"), Value::Int(0)]).is_err());
    }
    #[test]
    fn localtime_truncate_overrides_errs() {
        assert!(eval_localtime_truncate(&[s("hour"), lt_val(1, 0, 0), Value::Int(0)]).is_err());
    }

    #[test]
    fn time_truncate_works() {
        let r = eval_time_truncate(&[s("minute"), time_val(14, 30, 45, 3600)]).unwrap();
        if let Value::Time(t, off) = r {
            assert_eq!(t.minute(), 30);
            assert_eq!(t.second(), 0);
            assert_eq!(off, 3600);
        } else {
            panic!()
        }
    }
    #[test]
    fn time_truncate_with_tz_override() {
        let mut m = indexmap::IndexMap::new();
        m.insert("timezone".into(), Value::Str("UTC".into()));
        let r = eval_time_truncate(&[s("hour"), lt_val(14, 30, 45), Value::Map(m)]).unwrap();
        assert!(matches!(r, Value::Time(_, _)));
    }
    #[test]
    fn time_truncate_arity_errs() {
        assert!(eval_time_truncate(&[]).is_err());
    }
    #[test]
    fn time_truncate_unit_errs() {
        assert!(eval_time_truncate(&[Value::Int(0), lt_val(1, 0, 0)]).is_err());
    }
    #[test]
    fn time_truncate_no_time_errs() {
        assert!(eval_time_truncate(&[s("hour"), Value::Int(0)]).is_err());
    }
    #[test]
    fn time_truncate_overrides_errs() {
        assert!(eval_time_truncate(&[s("hour"), lt_val(1, 0, 0), Value::Int(0)]).is_err());
    }

    #[test]
    fn trunc_weekyear_works() {
        let r =
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "weekyear").unwrap();
        assert_eq!(r.year(), 2024);
    }
    #[test]
    fn trunc_decade_century() {
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "decade")
                .unwrap()
                .year(),
            2020
        );
        assert_eq!(
            truncate_date_fields(jiff::civil::Date::new(2024, 6, 15).unwrap(), "century")
                .unwrap()
                .year(),
            2000
        );
    }
    #[test]
    fn trunc_time_minute_second_milli_micro() {
        let t = jiff::civil::Time::new(14, 30, 45, 123_456_789).unwrap();
        assert_eq!(truncate_time_fields(t, "minute").unwrap().second(), 0);
        assert_eq!(
            truncate_time_fields(t, "second")
                .unwrap()
                .subsec_nanosecond(),
            0
        );
        assert_eq!(
            truncate_time_fields(t, "millisecond")
                .unwrap()
                .subsec_nanosecond(),
            123_000_000
        );
        assert_eq!(
            truncate_time_fields(t, "microsecond")
                .unwrap()
                .subsec_nanosecond(),
            123_456_000
        );
    }
    #[test]
    fn trunc_time_unknown_errs() {
        let t = jiff::civil::Time::new(0, 0, 0, 0).unwrap();
        assert!(truncate_time_fields(t, "unknown").is_err());
    }
    #[test]
    fn apply_overrides_dayofweek() {
        let mut m = indexmap::IndexMap::new();
        m.insert("dayOfWeek".into(), Value::Int(1));
        // 2024-06-15 is a Saturday → Monday is 2024-06-10
        let r = apply_date_overrides(jiff::civil::Date::new(2024, 6, 15).unwrap(), &m).unwrap();
        assert_eq!(r, jiff::civil::Date::new(2024, 6, 10).unwrap());
    }
    #[test]
    fn apply_overrides_minute_second_ms_us_ns() {
        let mut m = indexmap::IndexMap::new();
        m.insert("minute".into(), Value::Int(45));
        m.insert("second".into(), Value::Int(30));
        let r = apply_time_overrides(jiff::civil::Time::new(10, 0, 0, 0).unwrap(), &m).unwrap();
        assert_eq!(r.minute(), 45);
        assert_eq!(r.second(), 30);

        let mut m2 = indexmap::IndexMap::new();
        m2.insert("microsecond".into(), Value::Int(5));
        let r2 = apply_time_overrides(jiff::civil::Time::new(0, 0, 0, 0).unwrap(), &m2).unwrap();
        assert_eq!(r2.subsec_nanosecond(), 5_000);

        let mut m3 = indexmap::IndexMap::new();
        m3.insert("nanosecond".into(), Value::Int(7));
        let r3 = apply_time_overrides(jiff::civil::Time::new(0, 0, 0, 0).unwrap(), &m3).unwrap();
        assert_eq!(r3.subsec_nanosecond(), 7);
    }
    #[test]
    fn extract_time_variants() {
        assert!(extract_time(&lt_val(1, 0, 0)).is_some());
        assert!(extract_time(&time_val(1, 0, 0, 0)).is_some());
        assert!(extract_time(&ldt_val(2024, 1, 1, 1, 0, 0)).is_some());
        assert!(extract_time(&dt_val(2024, 1, 1, 1, 0, 0)).is_some());
        assert!(extract_time(&Value::Int(0)).is_none());
    }
    #[test]
    fn extract_date_from_dt_ldt() {
        assert!(extract_date(&ldt_val(2024, 6, 15, 0, 0, 0)).is_some());
        assert!(extract_date(&dt_val(2024, 6, 15, 0, 0, 0)).is_some());
    }
}
