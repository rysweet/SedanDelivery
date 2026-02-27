use std::fmt;

use indexmap::IndexMap;
use jiff::{civil, Zoned};

use crate::storage::{EdgeId, NodeId};

/// A Cypher duration with three independent signed components.
/// Unlike jiff::Span (which requires uniform sign), each part can have
/// a different sign, as per the openCypher specification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CypherDuration {
    /// Total months (years * 12 + months). May be negative.
    pub months: i64,
    /// Total days (weeks * 7 + days). Independent sign from months.
    pub days: i64,
    /// Total time in nanoseconds. Independent sign from months and days.
    /// = hours*3_600_000_000_000 + minutes*60_000_000_000 + seconds*1_000_000_000 + sub_nanos
    pub nanos: i64,
}

impl std::hash::Hash for CypherDuration {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.months.hash(state);
        self.days.hash(state);
        self.nanos.hash(state);
    }
}

/// The universal value type for GQL/Cypher properties and expression results.
///
/// Design constraints:
/// - No `Box<dyn Any>` — all variants are statically typed
/// - `Box<str>` for strings: 16 bytes vs `String`'s 24 bytes; strings are immutable once created
/// - Temporal variants use `jiff` (2025 Rust date/time library with full ISO 8601 + DST support)
/// - Spatial `Point` stored inline as (f64, f64) + CRS tag (8+8+4 = 20 bytes)
/// - `List` and `Map` heap-allocate their contents; scalar variants are stack-only
#[derive(Debug, Clone)]
pub enum Value {
    // ── Null ──────────────────────────────────────────────────────────────────
    Null,

    // ── Scalars ───────────────────────────────────────────────────────────────
    Bool(bool),
    Int(i64),
    Float(f64),

    /// Immutable string. `Box<str>` is 16 bytes (ptr + len); `String` is 24 bytes (ptr + len + cap).
    Str(Box<str>),

    // ── Temporal (ISO 8601 via `jiff`) ────────────────────────────────────────
    /// Calendar date: year + month + day, no time zone.
    Date(civil::Date),

    /// Local time: hour + minute + second + nanosecond, no date, no time zone.
    LocalTime(civil::Time),

    /// Local date-time: date + time, no time zone.
    LocalDateTime(civil::DateTime),

    /// Zoned date-time: date + time + IANA time zone identifier.
    /// Heap-allocated because `Zoned` is 40 bytes.
    DateTime(Box<Zoned>),

    /// ISO 8601 duration (years, months, weeks, days, hours, minutes, seconds, nanoseconds).
    Duration(CypherDuration),

    // ── Spatial ───────────────────────────────────────────────────────────────
    /// WGS-84 geographic point: (longitude, latitude).
    Point(GeoPoint),

    // ── Collections ───────────────────────────────────────────────────────────
    List(Vec<Value>),

    /// Property map: preserves insertion order (Cypher spec requirement).
    Map(IndexMap<Box<str>, Value>),

    // ── Graph element references (returned in result rows) ────────────────────
    Node(NodeId),
    Edge(EdgeId),

    /// A path: alternating node IDs and edge IDs, starting and ending with a node.
    /// Invariant: nodes.len() == edges.len() + 1
    Path {
        nodes: Vec<NodeId>,
        edges: Vec<EdgeId>,
    },
}

/// A geographic point with coordinate reference system tag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeoPoint {
    pub x: f64, // longitude (WGS-84) or easting (cartesian)
    pub y: f64, // latitude  (WGS-84) or northing (cartesian)
    pub crs: Crs,
}

/// Coordinate reference system, matching Cypher's `point()` function semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crs {
    /// `srid:4326` — WGS-84 geographic (latitude/longitude)
    Wgs84,
    /// `srid:4979` — WGS-84 3D geographic (latitude/longitude/height)
    /// NOTE: GeoPoint has no z-field yet; this variant is reserved for Phase N.
    Wgs843d,
    /// `srid:7203` — Cartesian 2D
    Cartesian,
    /// `srid:9157` — Cartesian 3D
    /// NOTE: GeoPoint has no z-field yet; this variant is reserved for Phase N.
    Cartesian3d,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a == b,
            // Int/Float cross-type: coerce to f64
            (Self::Int(a), Self::Float(b)) => (*a as f64) == *b,
            (Self::Float(a), Self::Int(b)) => *a == (*b as f64),
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Date(a), Self::Date(b)) => a == b,
            (Self::LocalTime(a), Self::LocalTime(b)) => a == b,
            (Self::LocalDateTime(a), Self::LocalDateTime(b)) => a == b,
            (Self::DateTime(a), Self::DateTime(b)) => a == b,
            // CypherDuration: structural field equality
            (Self::Duration(a), Self::Duration(b)) => a == b,
            (Self::Point(a), Self::Point(b)) => a == b,
            (Self::List(a), Self::List(b)) => a == b,
            (Self::Map(a), Self::Map(b)) => a == b,
            (Self::Node(a), Self::Node(b)) => a == b,
            (Self::Edge(a), Self::Edge(b)) => a == b,
            (
                Self::Path {
                    nodes: n1,
                    edges: e1,
                },
                Self::Path {
                    nodes: n2,
                    edges: e2,
                },
            ) => n1 == n2 && e1 == e2,
            _ => false,
        }
    }
}

impl Value {
    /// Returns a static string naming the Cypher type of this value.
    #[inline]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Null => "Null",
            Self::Bool(_) => "Boolean",
            Self::Int(_) => "Integer",
            Self::Float(_) => "Float",
            Self::Str(_) => "String",
            Self::Date(_) => "Date",
            Self::LocalTime(_) => "LocalTime",
            Self::LocalDateTime(_) => "LocalDateTime",
            Self::DateTime(_) => "DateTime",
            Self::Duration(_) => "Duration",
            Self::Point(_) => "Point",
            Self::List(_) => "List",
            Self::Map(_) => "Map",
            Self::Node(_) => "Node",
            Self::Edge(_) => "Relationship",
            Self::Path { .. } => "Path",
        }
    }

    /// Returns `true` if this value is `Null`.
    #[inline]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Coerce to `f64` for arithmetic. Returns `None` for non-numeric types.
    #[inline]
    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(f) => Some(*f),
            Self::Int(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Coerce to `i64`. Returns `None` for non-integer types (does NOT truncate floats).
    #[inline]
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Returns the string slice if this is a `Str` variant.
    #[inline]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(s) => Some(s),
            _ => None,
        }
    }

    /// Returns the boolean value if this is a `Bool` variant.
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Cypher truthiness: only `false` and `Null` are falsy.
    /// Everything else (including 0, empty string, empty list) is truthy.
    #[inline]
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            _ => true,
        }
    }

    /// Compare two values for Cypher equality semantics.
    /// Returns `None` when either operand is `Null` (Cypher three-valued logic).
    ///
    /// Handles:
    /// - NULL propagation: `null = X` → `None`
    /// - Int/Float coercion: `1 = 1.0` → `Some(true)`
    /// - List/Map null propagation: `[null] = [1]` → `None`
    pub fn cypher_eq(&self, other: &Self) -> Option<bool> {
        match (self, other) {
            // NULL = anything is NULL
            (Self::Null, _) | (_, Self::Null) => None,

            // Int/Float cross-type coercion
            (Self::Int(a), Self::Float(b)) => Some(*a as f64 == *b),
            (Self::Float(a), Self::Int(b)) => Some(*a == *b as f64),

            // List equality with null propagation
            (Self::List(a), Self::List(b)) => {
                if a.len() != b.len() {
                    return Some(false);
                }
                let mut has_null = false;
                for (av, bv) in a.iter().zip(b.iter()) {
                    match av.cypher_eq(bv) {
                        Some(false) => return Some(false),
                        None => has_null = true,
                        Some(true) => {}
                    }
                }
                if has_null { None } else { Some(true) }
            }

            // Map equality with null propagation
            (Self::Map(a), Self::Map(b)) => {
                // Different key sets → false (even with nulls)
                if a.len() != b.len() {
                    return Some(false);
                }
                let mut has_null = false;
                for (k, av) in a.iter() {
                    match b.get(k.as_ref()) {
                        None => return Some(false), // key missing
                        Some(bv) => match av.cypher_eq(bv) {
                            Some(false) => return Some(false),
                            None => has_null = true,
                            Some(true) => {}
                        },
                    }
                }
                if has_null { None } else { Some(true) }
            }

            // Duration: direct field comparison
            (Self::Duration(a), Self::Duration(b)) => Some(*a == *b),

            // All other types: delegate to PartialEq
            _ => Some(self == other),
        }
    }
}

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
            Self::LocalDateTime(dt) => write!(f, "{dt}"),
            Self::DateTime(dt) => {
                let civil_dt = jiff::civil::DateTime::from(dt.as_ref());
                let is_epoch_date = civil_dt.date() == jiff::civil::Date::new(1970, 1, 1).unwrap();
                let tz = dt.time_zone();
                let has_named_tz = tz.iana_name().is_some();
                if is_epoch_date {
                    // Time-only value: display as HH:MM:SS[.nanos]+offset
                    let t = civil_dt.time();
                    let off = dt.offset();
                    let total_secs_off = off.seconds();
                    let h_off = total_secs_off / 3600;
                    let m_off = (total_secs_off.abs() % 3600) / 60;
                    let offset_str = format!("{:+03}:{:02}", h_off, m_off);
                    // Format nanoseconds
                    let ns = t.subsec_nanosecond();
                    if ns != 0 {
                        let ns_str = format!("{:09}", ns);
                        let ns_trimmed = ns_str.trim_end_matches('0');
                        write!(f, "{:02}:{:02}:{:02}.{}{}", t.hour(), t.minute(), t.second(), ns_trimmed, offset_str)
                    } else {
                        write!(f, "{:02}:{:02}:{:02}{}", t.hour(), t.minute(), t.second(), offset_str)
                    }
                } else if has_named_tz {
                    // Named timezone: display full datetime with tz name
                    write!(f, "{dt}")
                } else {
                    // Fixed offset: display datetime without [tz] bracket
                    let civil = jiff::civil::DateTime::from(dt.as_ref());
                    let off = dt.offset();
                    let total_secs_off = off.seconds();
                    let h_off = total_secs_off / 3600;
                    let m_off = (total_secs_off.abs() % 3600) / 60;
                    let offset_str = format!("{:+03}:{:02}", h_off, m_off);
                    let ns = civil.subsec_nanosecond();
                    if ns != 0 {
                        let ns_str = format!("{:09}", ns);
                        let ns_trimmed = ns_str.trim_end_matches('0');
                        write!(f, "{}-{:02}-{:02}T{:02}:{:02}:{:02}.{}{}",
                            civil.year(), civil.month(), civil.day(),
                            civil.hour(), civil.minute(), civil.second(),
                            ns_trimmed, offset_str)
                    } else {
                        write!(f, "{}-{:02}-{:02}T{:02}:{:02}:{:02}{}",
                            civil.year(), civil.month(), civil.day(),
                            civil.hour(), civil.minute(), civil.second(),
                            offset_str)
                    }
                }
            },
            Self::Duration(d) => write!(f, "{}", format_cypher_duration(d)),
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

/// Format a CypherDuration as an ISO 8601-compatible duration string.
/// Cypher allows different signs for months, days, and time parts.
pub fn format_cypher_duration(d: &CypherDuration) -> String {
    let years = d.months / 12;
    let months = d.months % 12;
    let days = d.days;

    // Decompose nanos into hours, minutes, seconds, sub-nanos (all with same sign as nanos)
    let (time_sign, abs_nanos) = if d.nanos < 0 { (-1i64, -d.nanos) } else { (1i64, d.nanos) };
    let h_abs = abs_nanos / 3_600_000_000_000i64;
    let rem = abs_nanos % 3_600_000_000_000i64;
    let m_abs = rem / 60_000_000_000i64;
    let rem = rem % 60_000_000_000i64;
    let s_abs = rem / 1_000_000_000i64;
    let ns_abs = rem % 1_000_000_000i64;

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
    use super::*;

    #[test]
    fn null_is_falsy() {
        assert!(!Value::Null.is_truthy());
    }

    #[test]
    fn false_is_falsy() {
        assert!(!Value::Bool(false).is_truthy());
    }

    #[test]
    fn zero_int_is_truthy() {
        // Cypher: 0 is truthy (unlike Python/JS)
        assert!(Value::Int(0).is_truthy());
    }

    #[test]
    fn empty_string_is_truthy() {
        // Cypher: "" is truthy (unlike Python/JS)
        assert!(Value::Str("".into()).is_truthy());
    }

    #[test]
    fn null_equality_returns_none() {
        assert_eq!(Value::Null.cypher_eq(&Value::Int(1)), None);
        assert_eq!(Value::Int(1).cypher_eq(&Value::Null), None);
        assert_eq!(Value::Null.cypher_eq(&Value::Null), None);
    }

    #[test]
    fn int_equality() {
        assert_eq!(Value::Int(42).cypher_eq(&Value::Int(42)), Some(true));
        assert_eq!(Value::Int(42).cypher_eq(&Value::Int(43)), Some(false));
    }

    #[test]
    fn type_names_correct() {
        assert_eq!(Value::Null.type_name(), "Null");
        assert_eq!(Value::Bool(true).type_name(), "Boolean");
        assert_eq!(Value::Int(1).type_name(), "Integer");
        assert_eq!(Value::Float(1.0).type_name(), "Float");
        assert_eq!(Value::Str("x".into()).type_name(), "String");
    }

    #[test]
    fn as_float_coerces_int() {
        assert_eq!(Value::Int(3).as_float(), Some(3.0));
        assert_eq!(Value::Float(1.23456).as_float(), Some(1.23456));
        assert_eq!(Value::Null.as_float(), None);
    }

    #[test]
    fn display_list() {
        let v = Value::List(vec![Value::Int(1), Value::Int(2)]);
        assert_eq!(v.to_string(), "[1, 2]");
    }

    #[test]
    fn display_null() {
        assert_eq!(Value::Null.to_string(), "null");
    }
}
