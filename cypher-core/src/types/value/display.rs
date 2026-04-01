use std::fmt;

use super::{CypherDuration, Value};

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => write!(f, "null"),
            Self::Bool(b) => write!(f, "{b}"),
            Self::Int(i) => write!(f, "{i}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Str(s) => write!(f, "{s:?}"),
            Self::Date(d) => write!(f, "{d}"),
            Self::LocalTime(t) => write!(f, "{t}"),
            Self::Time(t, offset_secs) => {
                let offset_str = format_offset_str(*offset_secs);
                let ns = t.subsec_nanosecond();
                if ns != 0 {
                    let ns_str = format!("{:09}", ns);
                    let ns_trimmed = ns_str.trim_end_matches('0');
                    write!(
                        f,
                        "{:02}:{:02}:{:02}.{}{}",
                        t.hour(),
                        t.minute(),
                        t.second(),
                        ns_trimmed,
                        offset_str
                    )
                } else {
                    write!(
                        f,
                        "{:02}:{:02}:{:02}{}",
                        t.hour(),
                        t.minute(),
                        t.second(),
                        offset_str
                    )
                }
            }
            Self::LocalDateTime(dt) => write!(f, "{dt}"),
            Self::DateTime(dt) => {
                let civil_dt = jiff::civil::DateTime::from(dt.as_ref());
                // safe: constant epoch date 1970-01-01 is always valid
                let is_epoch_date =
                    civil_dt.date() == jiff::civil::Date::new(1970, 1, 1).expect("epoch");
                let tz = dt.time_zone();
                let has_named_tz = tz.iana_name().is_some();
                if is_epoch_date {
                    // Time-only value: display as HH:MM:SS[.nanos]+offset
                    let t = civil_dt.time();
                    let total_secs_off = dt.offset().seconds();
                    let offset_str = format_offset_str(total_secs_off);
                    // Format nanoseconds
                    let ns = t.subsec_nanosecond();
                    if ns != 0 {
                        let ns_str = format!("{:09}", ns);
                        let ns_trimmed = ns_str.trim_end_matches('0');
                        write!(
                            f,
                            "{:02}:{:02}:{:02}.{}{}",
                            t.hour(),
                            t.minute(),
                            t.second(),
                            ns_trimmed,
                            offset_str
                        )
                    } else {
                        write!(
                            f,
                            "{:02}:{:02}:{:02}{}",
                            t.hour(),
                            t.minute(),
                            t.second(),
                            offset_str
                        )
                    }
                } else if has_named_tz {
                    // Named timezone: display full datetime with tz name
                    write!(f, "{dt}")
                } else {
                    // Fixed offset: display datetime without [tz] bracket
                    let civil = jiff::civil::DateTime::from(dt.as_ref());
                    let total_secs_off = dt.offset().seconds();
                    let offset_str = format_offset_str(total_secs_off);
                    let ns = civil.subsec_nanosecond();
                    if ns != 0 {
                        let ns_str = format!("{:09}", ns);
                        let ns_trimmed = ns_str.trim_end_matches('0');
                        write!(
                            f,
                            "{}-{:02}-{:02}T{:02}:{:02}:{:02}.{}{}",
                            civil.year(),
                            civil.month(),
                            civil.day(),
                            civil.hour(),
                            civil.minute(),
                            civil.second(),
                            ns_trimmed,
                            offset_str
                        )
                    } else {
                        write!(
                            f,
                            "{}-{:02}-{:02}T{:02}:{:02}:{:02}{}",
                            civil.year(),
                            civil.month(),
                            civil.day(),
                            civil.hour(),
                            civil.minute(),
                            civil.second(),
                            offset_str
                        )
                    }
                }
            }
            Self::Duration(d) => write!(f, "{}", format_cypher_duration(d)),
            Self::Bytes(b) => {
                write!(f, "0x")?;
                for byte in b {
                    write!(f, "{:02X}", byte)?;
                }
                Ok(())
            }
            Self::Point(p) => write!(f, "point({{x: {}, y: {}}})", p.x, p.y),
            Self::List(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Self::Map(m) => {
                write!(f, "{{")?;
                for (i, (k, v)) in m.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, "}}")
            }
            Self::Node(id) => write!(f, "(node#{})", id.0), // id.0 is u64
            Self::Edge(id) => write!(f, "[edge#{}]", id.0), // id.0 is u64
            Self::Path { nodes, edges } => {
                write!(f, "<path: {} nodes, {} edges>", nodes.len(), edges.len())
            }
        }
    }
}

/// Format a UTC offset (in total seconds) as `+HH:MM` or `-HH:MM`.
///
/// A plain `secs / 3600` truncates toward zero, so negative offsets whose
/// magnitude is less than one hour (e.g. -1800 for -00:30) would lose the
/// sign.  We decompose using the absolute value instead.
pub fn format_offset_str(total_secs: i32) -> String {
    let sign = if total_secs < 0 { '-' } else { '+' };
    let abs = total_secs.unsigned_abs();
    let h = abs / 3600;
    let m = (abs % 3600) / 60;
    format!("{sign}{h:02}:{m:02}")
}

/// Format a CypherDuration as an ISO 8601-compatible duration string.
/// Cypher allows different signs for months, days, and time parts.
pub fn format_cypher_duration(d: &CypherDuration) -> String {
    let years = d.months / 12;
    let months = d.months % 12;
    let days = d.days;

    // Decompose nanos into hours, minutes, seconds, sub-nanos (all with same sign as nanos)
    let (time_sign, abs_nanos) = if d.nanos < 0 {
        (-1i128, -d.nanos)
    } else {
        (1i128, d.nanos)
    };
    let h_abs = abs_nanos / 3_600_000_000_000i128;
    let rem = abs_nanos % 3_600_000_000_000i128;
    let m_abs = rem / 60_000_000_000i128;
    let rem = rem % 60_000_000_000i128;
    let s_abs = rem / 1_000_000_000i128;
    let ns_abs = rem % 1_000_000_000i128;

    let norm_h = time_sign * h_abs;
    let norm_m = time_sign * m_abs;
    let norm_s = time_sign * s_abs;
    let norm_ns = time_sign * ns_abs;

    let has_date = years != 0 || months != 0 || days != 0;
    let has_time = norm_h != 0 || norm_m != 0 || norm_s != 0 || norm_ns != 0;

    let mut s = String::from("P");
    if years != 0 {
        s.push_str(&format!("{years}Y"));
    }
    if months != 0 {
        s.push_str(&format!("{months}M"));
    }
    if days != 0 {
        s.push_str(&format!("{days}D"));
    }
    if has_time {
        s.push('T');
        if norm_h != 0 {
            s.push_str(&format!("{norm_h}H"));
        }
        if norm_m != 0 {
            s.push_str(&format!("{norm_m}M"));
        }
        if norm_ns != 0 {
            let ns_u = norm_ns.unsigned_abs();
            let ns_str = format!("{ns_u:09}");
            let ns_trimmed = ns_str.trim_end_matches('0');
            // Handle -0.NNNs case: norm_s=0 but time is negative
            if norm_s == 0 && time_sign < 0 {
                s.push_str(&format!("-0.{ns_trimmed}S"));
            } else {
                s.push_str(&format!("{norm_s}.{ns_trimmed}S"));
            }
        } else if norm_s != 0 {
            s.push_str(&format!("{norm_s}S"));
        }
    }
    if !has_date && !has_time {
        s.push_str("T0S");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::super::*;

    #[test]
    fn display_list() {
        let v = Value::List(vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(v.to_string(), "[1, 2]");
    }

    #[test]
    fn display_null() {
        assert_eq!(Value::Null.to_string(), "null");
    }

    // ── C7 regression: negative sub-hour offsets must keep their sign ────────

    #[test]
    fn format_offset_str_negative_sub_hour() {
        // -00:30 is -1800 seconds; the sign must be preserved even though hours == 0
        assert_eq!(super::format_offset_str(-1800), "-00:30");
    }

    #[test]
    fn format_offset_str_negative_full_hour() {
        assert_eq!(super::format_offset_str(-3600), "-01:00");
    }

    #[test]
    fn format_offset_str_positive_sub_hour() {
        assert_eq!(super::format_offset_str(1800), "+00:30");
    }

    #[test]
    fn format_offset_str_zero() {
        assert_eq!(super::format_offset_str(0), "+00:00");
    }

    #[test]
    fn format_offset_str_negative_45_min() {
        // -00:45 is -2700 seconds
        assert_eq!(super::format_offset_str(-2700), "-00:45");
    }

    #[test]
    fn format_offset_str_negative_5h30m() {
        // -05:30 is -19800 seconds
        assert_eq!(super::format_offset_str(-19800), "-05:30");
    }

    #[test]
    fn display_time_with_negative_sub_hour_offset() {
        let t = jiff::civil::Time::new(12, 0, 0, 0).unwrap();
        let v = Value::Time(t, -1800); // -00:30
        let s = v.to_string();
        assert!(s.ends_with("-00:30"), "expected -00:30 suffix, got: {s}");
    }
}
