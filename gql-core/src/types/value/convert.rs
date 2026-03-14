use super::Value;

impl Value {
    /// If this is a `Time(t, offset)`, convert to a `DateTime` at epoch date
    /// (1970-01-01) so that code expecting DateTime can process it uniformly.
    /// Returns `self` unchanged for all other variants.
    ///
    /// Returns an error if the offset is out of range or the zoned conversion fails.
    pub fn time_to_datetime(self) -> Result<Self, String> {
        match self {
            Self::Time(t, offset_secs) => {
                let offset = jiff::tz::Offset::from_seconds(offset_secs)
                    .map_err(|e| format!("timezone offset {offset_secs}s out of range: {e}"))?;
                let tz = jiff::tz::TimeZone::fixed(offset);
                // safe: constant epoch date 1970-01-01 is always valid
                let epoch = jiff::civil::Date::new(1970, 1, 1).expect("valid epoch");
                let dt = epoch.to_datetime(t);
                let zoned = dt
                    .to_zoned(tz)
                    .map_err(|e| format!("failed to convert Time to zoned DateTime: {e}"))?;
                Ok(Self::DateTime(Box::new(zoned)))
            }
            other => Ok(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil;

    use super::super::*;

    // ── time_to_datetime: valid offset still works ─────────────────────────

    #[test]
    fn time_to_datetime_valid_offset() {
        let t = civil::Time::new(10, 30, 0, 0).expect("valid time");
        let val = Value::Time(t, 19800);
        match val.time_to_datetime().expect("should succeed") {
            Value::DateTime(zdt) => {
                assert_eq!(zdt.hour(), 10);
                assert_eq!(zdt.minute(), 30);
            }
            other => panic!("expected DateTime, got {other:?}"),
        }
    }

    // ── time_to_datetime: out-of-range offset returns error ─────────────────

    #[test]
    fn time_to_datetime_invalid_offset_returns_error() {
        let t = civil::Time::new(12, 0, 0, 0).expect("valid time");
        let val = Value::Time(t, 200_000);
        let result = val.time_to_datetime();
        assert!(
            result.is_err(),
            "out-of-range offset should return an error"
        );
        let err_msg = result.unwrap_err();
        assert!(
            err_msg.contains("out of range"),
            "error should mention out of range, got: {err_msg}"
        );
    }
}
