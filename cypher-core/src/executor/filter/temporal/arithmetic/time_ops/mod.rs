//! duration.between, duration.inMonths, duration.inDays, duration.inSeconds operations.
//!
//! Decomposed by operation kind:
//! - [`between`]: full-precision diff producing months + days + nanos
//! - [`in_months`]: diff truncated to whole months
//! - [`in_days`]: diff truncated to whole days
//! - [`in_seconds`]: diff producing nanos (with day-aware handling)

mod between;
mod in_days;
mod in_months;
mod in_seconds;

pub(crate) use between::eval_duration_between;
pub(crate) use in_days::eval_duration_in_days;
pub(crate) use in_months::eval_duration_in_months;
pub(crate) use in_seconds::eval_duration_in_seconds;

#[cfg(test)]
mod tests {
    use super::super::{has_date_component, is_time_only};
    use super::*;
    use crate::types::Value;

    #[test]
    fn between_dates() {
        let d1 = Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap());
        let d2 = Value::Date(jiff::civil::Date::new(2024, 3, 15).unwrap());
        match eval_duration_between(&[d1, d2]).unwrap() {
            Value::Duration(d) => {
                assert_eq!(d.months, 2);
                assert!(d.days > 0);
            }
            _ => panic!("Dur"),
        }
    }
    #[test]
    fn between_null() {
        assert_eq!(
            eval_duration_between(&[
                Value::Null,
                Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap())
            ])
            .unwrap(),
            Value::Null
        );
    }
    #[test]
    fn between_bad() {
        assert!(eval_duration_between(&[Value::Int(1)]).is_err());
    }
    #[test]
    fn in_months_dates() {
        let d1 = Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap());
        let d2 = Value::Date(jiff::civil::Date::new(2024, 7, 1).unwrap());
        match eval_duration_in_months(&[d1, d2]).unwrap() {
            Value::Duration(d) => assert_eq!(d.months, 6),
            _ => panic!("Dur"),
        }
    }
    #[test]
    fn in_months_null() {
        assert_eq!(
            eval_duration_in_months(&[
                Value::Null,
                Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap())
            ])
            .unwrap(),
            Value::Null
        );
    }
    #[test]
    fn in_days_dates() {
        let d1 = Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap());
        let d2 = Value::Date(jiff::civil::Date::new(2024, 1, 11).unwrap());
        match eval_duration_in_days(&[d1, d2]).unwrap() {
            Value::Duration(d) => assert_eq!(d.days, 10),
            _ => panic!("Dur"),
        }
    }
    #[test]
    fn in_days_null() {
        assert_eq!(
            eval_duration_in_days(&[
                Value::Null,
                Value::Date(jiff::civil::Date::new(2024, 1, 1).unwrap())
            ])
            .unwrap(),
            Value::Null
        );
    }
    #[test]
    fn in_seconds_ldts() {
        let d1 = Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 10, 0, 0, 0).unwrap());
        let d2 = Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 12, 0, 0, 0).unwrap());
        match eval_duration_in_seconds(&[d1, d2]).unwrap() {
            Value::Duration(d) => assert_eq!(d.nanos, 2 * 3_600_000_000_000i128),
            _ => panic!("Dur"),
        }
    }
    #[test]
    fn in_seconds_null() {
        assert_eq!(
            eval_duration_in_seconds(&[
                Value::Null,
                Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 0, 0, 0, 0).unwrap())
            ])
            .unwrap(),
            Value::Null
        );
    }
    #[test]
    fn is_time_only_lt() {
        assert!(is_time_only(&Value::LocalTime(
            jiff::civil::Time::new(10, 0, 0, 0).unwrap()
        )));
    }
    #[test]
    fn is_time_only_date() {
        assert!(!is_time_only(&Value::Date(
            jiff::civil::Date::new(2024, 1, 1).unwrap()
        )));
    }
    #[test]
    fn has_date_date() {
        assert!(has_date_component(&Value::Date(
            jiff::civil::Date::new(2024, 1, 1).unwrap()
        )));
    }
    #[test]
    fn between_datetimes() {
        let z1: jiff::Zoned = "2024-01-01T00:00:00+00:00[UTC]".parse().unwrap();
        let z2: jiff::Zoned = "2024-06-01T12:00:00+00:00[UTC]".parse().unwrap();
        match eval_duration_between(&[Value::DateTime(Box::new(z1)), Value::DateTime(Box::new(z2))])
            .unwrap()
        {
            Value::Duration(d) => assert_eq!(d.months, 5),
            _ => panic!(),
        }
    }
    #[test]
    fn between_localdatetimes() {
        let dt1 = jiff::civil::DateTime::new(2024, 1, 1, 0, 0, 0, 0).unwrap();
        let dt2 = jiff::civil::DateTime::new(2024, 1, 2, 0, 0, 0, 0).unwrap();
        match eval_duration_between(&[Value::LocalDateTime(dt1), Value::LocalDateTime(dt2)])
            .unwrap()
        {
            Value::Duration(d) => assert!(d.days >= 0),
            _ => panic!(),
        }
    }
    #[test]
    fn between_local_times() {
        let t1 = Value::LocalTime(jiff::civil::Time::new(10, 0, 0, 0).unwrap());
        let t2 = Value::LocalTime(jiff::civil::Time::new(12, 0, 0, 0).unwrap());
        match eval_duration_between(&[t1, t2]).unwrap() {
            Value::Duration(d) => assert!(d.nanos > 0),
            _ => panic!(),
        }
    }
    #[test]
    fn between_zoned_times() {
        let t1 = Value::Time(jiff::civil::Time::new(10, 0, 0, 0).unwrap(), 0);
        let t2 = Value::Time(jiff::civil::Time::new(12, 0, 0, 0).unwrap(), 0);
        match eval_duration_between(&[t1, t2]).unwrap() {
            Value::Duration(d) => assert!(d.nanos > 0),
            _ => panic!(),
        }
    }
    #[test]
    fn between_too_few_args_errs() {
        assert!(eval_duration_between(&[]).is_err());
    }
    #[test]
    fn in_months_too_few_args_errs() {
        assert!(eval_duration_in_months(&[]).is_err());
    }
    #[test]
    fn in_days_too_few_args_errs() {
        assert!(eval_duration_in_days(&[]).is_err());
    }
    #[test]
    fn in_seconds_too_few_args_errs() {
        assert!(eval_duration_in_seconds(&[]).is_err());
    }
    #[test]
    fn in_seconds_null_first_arg() {
        assert_eq!(
            eval_duration_in_seconds(&[
                Value::LocalDateTime(jiff::civil::DateTime::new(2024, 1, 1, 0, 0, 0, 0).unwrap()),
                Value::Null
            ])
            .unwrap(),
            Value::Null
        );
    }
    #[test]
    fn in_months_datetimes() {
        let z1: jiff::Zoned = "2024-01-01T00:00:00+00:00[UTC]".parse().unwrap();
        let z2: jiff::Zoned = "2024-04-01T00:00:00+00:00[UTC]".parse().unwrap();
        match eval_duration_in_months(&[
            Value::DateTime(Box::new(z1)),
            Value::DateTime(Box::new(z2)),
        ])
        .unwrap()
        {
            Value::Duration(d) => assert_eq!(d.months, 3),
            _ => panic!(),
        }
    }
    #[test]
    fn in_days_datetimes() {
        let z1: jiff::Zoned = "2024-01-01T00:00:00+00:00[UTC]".parse().unwrap();
        let z2: jiff::Zoned = "2024-01-08T00:00:00+00:00[UTC]".parse().unwrap();
        match eval_duration_in_days(&[Value::DateTime(Box::new(z1)), Value::DateTime(Box::new(z2))])
            .unwrap()
        {
            Value::Duration(d) => assert_eq!(d.days, 7),
            _ => panic!(),
        }
    }
    #[test]
    fn in_seconds_datetimes() {
        let z1: jiff::Zoned = "2024-01-01T00:00:00+00:00[UTC]".parse().unwrap();
        let z2: jiff::Zoned = "2024-01-01T00:01:00+00:00[UTC]".parse().unwrap();
        match eval_duration_in_seconds(&[
            Value::DateTime(Box::new(z1)),
            Value::DateTime(Box::new(z2)),
        ])
        .unwrap()
        {
            Value::Duration(d) => assert!(d.nanos > 0),
            _ => panic!(),
        }
    }
    #[test]
    fn is_time_only_zoned_time() {
        assert!(is_time_only(&Value::Time(
            jiff::civil::Time::new(0, 0, 0, 0).unwrap(),
            0
        )));
    }
    #[test]
    fn has_date_localdatetime() {
        assert!(has_date_component(&Value::LocalDateTime(
            jiff::civil::DateTime::new(2024, 1, 1, 0, 0, 0, 0).unwrap()
        )));
    }
}
