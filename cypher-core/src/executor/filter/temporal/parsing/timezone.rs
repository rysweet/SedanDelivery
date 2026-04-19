//! Timezone offset extraction from ISO 8601 datetime strings.

/// Extract a timezone offset from the tail of an ISO 8601 datetime string.
/// Looks for patterns like "+01:00", "-05:30", "+0100", "Z" at the end.
pub(crate) fn extract_offset_from_string(s: &str) -> Option<jiff::tz::TimeZone> {
    let s = s.trim();
    if s.ends_with('Z') || s.ends_with('z') {
        return Some(jiff::tz::TimeZone::UTC);
    }
    let tail = if s.len() > 6 { &s[s.len() - 6..] } else { s };
    let offset_start = tail.rfind('+').or_else(|| tail.rfind('-'));
    if let Some(pos) = offset_start {
        let offset_str = &tail[pos..];
        if let Ok(tz) = super::super::parse_timezone(offset_str) {
            return Some(tz);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_z() {
        assert_eq!(
            extract_offset_from_string("T12:00:00Z").unwrap(),
            jiff::tz::TimeZone::UTC
        );
    }
    #[test]
    fn extract_none() {
        assert!(extract_offset_from_string("T12:00:00").is_none());
    }
}
