//! Time-string helpers: compact-time expansion and timezone offset stripping.

/// Strip a trailing timezone designator (Z or +/-HH[:MM]) from a time string.
pub(crate) fn strip_tz_from_time_str(s: &str) -> &str {
    if s.ends_with('Z') || s.ends_with('z') {
        return &s[..s.len() - 1];
    }
    let bytes = s.as_bytes();
    for i in (1..s.len()).rev() {
        let b = bytes[i];
        if b == b'+' || b == b'-' {
            let after = &s[i..];
            let rest = &after[1..];
            let rest_stripped = rest.replace(':', "");
            if (rest_stripped.len() == 2 || rest_stripped.len() == 4)
                && rest_stripped.chars().all(|c| c.is_ascii_digit())
            {
                return &s[..i];
            }
        }
    }
    s
}

/// Expand compact time notation `HHMMSS` to `HH:MM:SS`
pub(crate) fn expand_compact_time(s: &str) -> String {
    if s.len() >= 6 && !s.contains(':') {
        let hh = &s[0..2];
        let mm = &s[2..4];
        let ss = &s[4..];
        if let Some(dot_pos) = ss.find('.') {
            let (secs, frac) = ss.split_at(dot_pos);
            format!("{hh}:{mm}:{secs}{frac}")
        } else {
            format!("{hh}:{mm}:{ss}")
        }
    } else if s.len() == 4 && !s.contains(':') {
        let hh = &s[0..2];
        let mm = &s[2..4];
        format!("{hh}:{mm}:00")
    } else {
        s.to_string()
    }
}

/// Strip timezone offset from a time string like "21:40:32+01:00" → "21:40:32"
pub(crate) fn strip_tz_offset_from_time(s: &str) -> &str {
    if s.ends_with('Z') || s.ends_with('z') {
        return &s[..s.len() - 1];
    }
    if let Some(pos) = s.rfind('+') {
        if pos > 0 {
            return &s[..pos];
        }
    }
    if let Some(pos) = s.rfind('-') {
        if pos > 2 {
            return &s[..pos];
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_hhmmss() {
        assert_eq!(expand_compact_time("214032"), "21:40:32");
    }
    #[test]
    fn expand_hhmm() {
        assert_eq!(expand_compact_time("2140"), "21:40:00");
    }
    #[test]
    fn expand_frac() {
        assert_eq!(expand_compact_time("214032.142"), "21:40:32.142");
    }
    #[test]
    fn strip_tz_z() {
        assert_eq!(strip_tz_from_time_str("21:40:32Z"), "21:40:32");
    }
    #[test]
    fn strip_offset_z() {
        assert_eq!(strip_tz_offset_from_time("21:40:32Z"), "21:40:32");
    }
    #[test]
    fn strip_offset_plus() {
        assert_eq!(strip_tz_offset_from_time("21:40:32+01:00"), "21:40:32");
    }
}
