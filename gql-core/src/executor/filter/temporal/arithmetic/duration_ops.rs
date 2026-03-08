//! Duration add/sub/mul/div operations.

use crate::error::{ExecutionError, GqlError};
use crate::types::CypherDuration;

/// Add two durations.
pub fn duration_add(a: &CypherDuration, b: &CypherDuration) -> CypherDuration {
    CypherDuration {
        months: a.months + b.months,
        days: a.days + b.days,
        nanos: a.nanos + b.nanos,
    }
}

/// Subtract two durations.
pub fn duration_sub(a: &CypherDuration, b: &CypherDuration) -> CypherDuration {
    CypherDuration {
        months: a.months - b.months,
        days: a.days - b.days,
        nanos: a.nanos - b.nanos,
    }
}

/// Multiply a duration by a float. Uses rational approximation for clean fractions (e.g. 0.5 -> /2).
pub fn duration_mul_f64(d: &CypherDuration, f: f64) -> CypherDuration {
    if f != 0.0 {
        let inv = 1.0 / f;
        let k = inv.round() as i64;
        if k != 0 && (inv - k as f64).abs() < 1e-9 {
            if let Ok(result) = duration_div(d, k) {
                return result;
            }
        }
    }
    let n = f.round() as i64;
    if (f - n as f64).abs() < 1e-9 {
        return duration_mul(d, n);
    }
    let new_months_f = d.months as f64 * f;
    let new_months = new_months_f.trunc() as i64;
    let frac_months = new_months_f.fract();
    const MONTH_NANOS_F: f64 = 2_629_746_000_000_000.0;
    const DAY_NANOS_F: f64 = 86_400_000_000_000.0;
    let remaining_nanos_f =
        frac_months * MONTH_NANOS_F + d.days as f64 * f * DAY_NANOS_F + d.nanos as f64 * f;
    let new_days = (remaining_nanos_f / DAY_NANOS_F).trunc() as i64;
    let new_nanos = (remaining_nanos_f - new_days as f64 * DAY_NANOS_F).round() as i128;
    CypherDuration {
        months: new_months,
        days: new_days,
        nanos: new_nanos,
    }
}

/// Multiply a duration by an integer.
pub fn duration_mul(d: &CypherDuration, n: i64) -> CypherDuration {
    CypherDuration {
        months: d.months * n,
        days: d.days * n,
        nanos: d.nanos * n as i128,
    }
}

/// Divide a duration by an integer.
pub fn duration_div(d: &CypherDuration, n: i64) -> Result<CypherDuration, GqlError> {
    if n == 0 {
        return Err(GqlError::Execution(ExecutionError::DivisionByZero));
    }
    let new_months = d.months / n;
    let rem_months = d.months % n;
    const MONTH_NANOS: i128 = 2_629_746_000_000_000;
    const DAY_NANOS: i128 = 86_400_000_000_000;
    let total_sub: i128 = rem_months as i128 * MONTH_NANOS + d.days as i128 * DAY_NANOS + d.nanos;
    let new_sub = total_sub / n as i128;
    let new_days = new_sub.div_euclid(DAY_NANOS) as i64;
    let new_nanos = new_sub.rem_euclid(DAY_NANOS);
    Ok(CypherDuration {
        months: new_months,
        days: new_days,
        nanos: new_nanos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dur(months: i64, days: i64, nanos: i128) -> CypherDuration {
        CypherDuration { months, days, nanos }
    }

    #[test]
    fn add_durs() {
        let r = duration_add(&dur(12, 5, 1_000_000_000), &dur(3, 2, 500_000_000));
        assert_eq!(r.months, 15);
        assert_eq!(r.days, 7);
    }
    #[test]
    fn sub_durs() {
        let r = duration_sub(&dur(12, 5, 1_000_000_000), &dur(3, 2, 500_000_000));
        assert_eq!(r.months, 9);
        assert_eq!(r.days, 3);
    }
    #[test]
    fn mul_int() {
        let r = duration_mul(&dur(2, 3, 1_000_000_000), 3);
        assert_eq!(r.months, 6);
        assert_eq!(r.days, 9);
    }
    #[test]
    fn div_int() {
        assert_eq!(duration_div(&dur(6, 0, 0), 2).unwrap().months, 3);
    }
    #[test]
    fn div_zero() {
        assert!(duration_div(&dur(6, 0, 0), 0).is_err());
    }
    #[test]
    fn mul_half() {
        assert_eq!(duration_mul_f64(&dur(12, 0, 0), 0.5).months, 6);
    }
}
