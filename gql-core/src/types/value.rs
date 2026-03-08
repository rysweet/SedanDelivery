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
    /// Uses i128 to support very large durations (e.g. duration.inSeconds spanning billions of years).
    pub nanos: i128,
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

    /// Zoned time: time-of-day + UTC offset in seconds. No date component.
    /// Distinct from DateTime so that Bolt Time (tag 0x54) values round-trip
    /// correctly without being confused with DateTime values that happen to
    /// have an epoch date.
    Time(civil::Time, i32),

    /// Local date-time: date + time, no time zone.
    LocalDateTime(civil::DateTime),

    /// Zoned date-time: date + time + IANA time zone identifier or fixed offset.
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

    // ── Binary data ──────────────────────────────────────────────────────────
    /// Raw byte array (Bolt Bytes type). Cypher has no native bytes literal,
    /// but values may arrive via Bolt parameters.
    Bytes(Vec<u8>),

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
    pub x: f64,         // longitude (WGS-84) or easting (cartesian)
    pub y: f64,         // latitude  (WGS-84) or northing (cartesian)
    pub z: Option<f64>, // height (WGS-84 3D) or elevation (Cartesian 3D)
    pub crs: Crs,
}

/// Coordinate reference system, matching Cypher's `point()` function semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Crs {
    /// `srid:4326` — WGS-84 geographic (latitude/longitude)
    Wgs84,
    /// `srid:4979` — WGS-84 3D geographic (latitude/longitude/height)
    Wgs843d,
    /// `srid:7203` — Cartesian 2D
    Cartesian,
    /// `srid:9157` — Cartesian 3D
    Cartesian3d,
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Null, Self::Null) => true,
            (Self::Bool(a), Self::Bool(b)) => a == b,
            (Self::Int(a), Self::Int(b)) => a == b,
            (Self::Float(a), Self::Float(b)) => a == b,
            // Int/Float cross-type: use safe comparison to avoid precision
            // loss for integers beyond 2^53.
            (Self::Int(a), Self::Float(b)) => {
                let af = *a as f64;
                af as i64 == *a && af == *b
            }
            (Self::Float(a), Self::Int(b)) => {
                let bf = *b as f64;
                bf as i64 == *b && *a == bf
            }
            (Self::Str(a), Self::Str(b)) => a == b,
            (Self::Date(a), Self::Date(b)) => a == b,
            (Self::LocalTime(a), Self::LocalTime(b)) => a == b,
            (Self::Time(at, ao), Self::Time(bt, bo)) => at == bt && ao == bo,
            (Self::LocalDateTime(a), Self::LocalDateTime(b)) => a == b,
            (Self::DateTime(a), Self::DateTime(b)) => a == b,
            // CypherDuration: structural field equality
            (Self::Duration(a), Self::Duration(b)) => a == b,
            (Self::Point(a), Self::Point(b)) => a == b,
            (Self::Bytes(a), Self::Bytes(b)) => a == b,
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
            Self::Time(_, _) => "Time",
            Self::LocalDateTime(_) => "LocalDateTime",
            Self::DateTime(_) => "DateTime",
            Self::Duration(_) => "Duration",
            Self::Point(_) => "Point",
            Self::Bytes(_) => "ByteArray",
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

    /// If this is a `Time(t, offset)`, convert to a `DateTime` at epoch date
    /// (1970-01-01) so that code expecting DateTime can process it uniformly.
    /// Returns `self` unchanged for all other variants.
    pub fn time_to_datetime(self) -> Self {
        match self {
            Self::Time(t, offset_secs) => {
                let offset = match jiff::tz::Offset::from_seconds(offset_secs) {
                    Ok(o) => o,
                    Err(_) => {
                        tracing::warn!(
                            offset_secs = offset_secs,
                            "timezone offset out of range, falling back to UTC"
                        );
                        jiff::tz::Offset::UTC
                    }
                };
                let tz = jiff::tz::TimeZone::fixed(offset);
                // safe: constant epoch date 1970-01-01 is always valid
                let epoch = civil::Date::new(1970, 1, 1).expect("valid epoch");
                let dt = epoch.to_datetime(t);
                match dt.to_zoned(tz) {
                    Ok(zoned) => Self::DateTime(Box::new(zoned)),
                    Err(e) => {
                        tracing::warn!(
                            error = %e,
                            "failed to convert Time to zoned DateTime, returning Null"
                        );
                        Self::Null
                    }
                }
            }
            other => other,
        }
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

            // Int/Float cross-type coercion: use safe comparison to avoid
            // precision loss for integers beyond 2^53.
            (Self::Int(a), Self::Float(b)) => {
                let af = *a as f64;
                Some(af as i64 == *a && af == *b)
            }
            (Self::Float(a), Self::Int(b)) => {
                let bf = *b as f64;
                Some(*a == bf && bf as i64 == *b)
            }

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
                if has_null {
                    None
                } else {
                    Some(true)
                }
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
                if has_null {
                    None
                } else {
                    Some(true)
                }
            }

            // Duration: direct field comparison
            (Self::Duration(a), Self::Duration(b)) => Some(*a == *b),

            // Bytes: direct comparison
            (Self::Bytes(a), Self::Bytes(b)) => Some(a == b),

            // All other types: delegate to PartialEq
            _ => Some(self == other),
        }
    }

    /// Feed a deterministic, equality-consistent representation into a
    /// [`Hasher`](std::hash::Hasher), mirroring [`canonical_key`](Self::canonical_key)
    /// without allocating intermediate `String`s.
    ///
    /// Invariant: if `a == b` (per `PartialEq`), then `a` and `b` produce the
    /// same hash sequence.
    pub fn canonical_hash<H: std::hash::Hasher>(&self, state: &mut H) {
        use std::hash::Hash;
        std::mem::discriminant(self).hash(state);
        match self {
            Self::Null => {}
            Self::Bool(b) => b.hash(state),
            Self::Int(i) => i.hash(state),
            Self::Float(f) => {
                let bits = if *f == 0.0 {
                    0.0_f64.to_bits()
                } else if f.is_nan() {
                    f64::NAN.to_bits()
                } else {
                    f.to_bits()
                };
                bits.hash(state);
            }
            Self::Str(s) => s.hash(state),
            Self::Date(d) => {
                d.year().hash(state);
                d.month().hash(state);
                d.day().hash(state);
            }
            Self::LocalTime(t) => {
                t.hour().hash(state);
                t.minute().hash(state);
                t.second().hash(state);
                t.subsec_nanosecond().hash(state);
            }
            Self::Time(t, offset) => {
                t.hour().hash(state);
                t.minute().hash(state);
                t.second().hash(state);
                t.subsec_nanosecond().hash(state);
                offset.hash(state);
            }
            Self::LocalDateTime(dt) => {
                dt.year().hash(state);
                dt.month().hash(state);
                dt.day().hash(state);
                dt.hour().hash(state);
                dt.minute().hash(state);
                dt.second().hash(state);
                dt.subsec_nanosecond().hash(state);
            }
            Self::DateTime(z) => {
                z.timestamp().as_nanosecond().hash(state);
            }
            Self::Duration(d) => d.hash(state),
            Self::Point(p) => {
                let nx = if p.x == 0.0 { 0.0_f64 } else { p.x };
                let ny = if p.y == 0.0 { 0.0_f64 } else { p.y };
                nx.to_bits().hash(state);
                ny.to_bits().hash(state);
                match p.z {
                    Some(z) => {
                        true.hash(state);
                        let nz = if z == 0.0 { 0.0_f64 } else { z };
                        nz.to_bits().hash(state);
                    }
                    None => false.hash(state),
                }
                std::mem::discriminant(&p.crs).hash(state);
            }
            Self::Bytes(b) => b.hash(state),
            Self::List(items) => {
                items.len().hash(state);
                for v in items {
                    v.canonical_hash(state);
                }
            }
            Self::Map(m) => {
                m.len().hash(state);
                let mut keys: Vec<&str> = m.keys().map(|k| k.as_ref()).collect();
                keys.sort();
                for k in keys {
                    k.hash(state);
                    // safe: k was obtained from m.keys() so get() always succeeds
                    m.get(k).unwrap().canonical_hash(state);
                }
            }
            Self::Node(id) => id.hash(state),
            Self::Edge(id) => id.hash(state),
            Self::Path { nodes, edges } => {
                nodes.hash(state);
                edges.hash(state);
            }
        }
    }

    /// Produce a deterministic, equality-consistent string key for use in
    /// hash-based grouping (aggregation GROUP BY, DISTINCT dedup, UNION dedup).
    ///
    /// Invariant: if `a == b` (per `PartialEq`), then
    /// `a.canonical_key() == b.canonical_key()`.
    ///
    /// The default `Debug` format violates this for:
    /// - `Float`: `-0.0` vs `0.0` (equal per IEEE 754 but different Debug)
    /// - `Map`: insertion-order-dependent Debug output
    /// - `DateTime`: same instant in different time zones
    pub fn canonical_key(&self) -> String {
        match self {
            Self::Float(f) => {
                let v = if *f == 0.0 {
                    // Normalize -0.0 to 0.0 (IEEE 754: -0.0 == 0.0)
                    0.0_f64
                } else if f.is_nan() {
                    // All NaN bit patterns map to the same key
                    f64::NAN
                } else {
                    *f
                };
                format!("Float({:?})", v)
            }
            Self::Map(m) => {
                // Sort entries by key so insertion order does not affect the key.
                let mut entries: Vec<(&str, String)> = m
                    .iter()
                    .map(|(k, v)| (k.as_ref(), v.canonical_key()))
                    .collect();
                entries.sort_by_key(|(k, _)| *k);
                let inner: Vec<String> =
                    entries.iter().map(|(k, v)| format!("{k:?}: {v}")).collect();
                format!("Map({{{}}})", inner.join(", "))
            }
            Self::DateTime(z) => {
                // Compare by UTC instant (epoch nanoseconds), not by
                // civil-time-plus-timezone representation.
                let nanos = z.timestamp().as_nanosecond();
                format!("DateTime(utc_ns={})", nanos)
            }
            Self::List(items) => {
                // Recurse so nested Float/Map/DateTime values are also canonical.
                let inner: Vec<String> = items.iter().map(|v| v.canonical_key()).collect();
                format!("List([{}])", inner.join(", "))
            }
            // All other variants: Debug format is already equality-consistent.
            other => format!("{:?}", other),
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
            Self::Time(t, offset_secs) => {
                let h_off = offset_secs / 3600;
                let m_off = (offset_secs.abs() % 3600) / 60;
                let offset_str = format!("{:+03}:{:02}", h_off, m_off);
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
                    let off = dt.offset();
                    let total_secs_off = off.seconds();
                    let h_off = total_secs_off / 3600;
                    let m_off = (total_secs_off.abs() % 3600) / 60;
                    let offset_str = format!("{:+03}:{:02}", h_off, m_off);
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

    // ── canonical_key tests ──────────────────────────────────────────────────

    #[test]
    fn canonical_key_float_neg_zero_equals_pos_zero() {
        let a = Value::Float(0.0);
        let b = Value::Float(-0.0);
        assert_eq!(a, b, "PartialEq says 0.0 == -0.0");
        assert_eq!(
            a.canonical_key(),
            b.canonical_key(),
            "canonical_key must agree with PartialEq"
        );
    }

    #[test]
    fn canonical_key_float_nan_consistent() {
        let a = Value::Float(f64::NAN);
        let b = Value::Float(f64::NAN);
        // NaN != NaN per IEEE 754 / PartialEq, but for hashing we need a
        // stable key so all NaN values land in the same bucket.
        assert_eq!(a.canonical_key(), b.canonical_key());
    }

    #[test]
    fn canonical_key_float_distinct_values() {
        let a = Value::Float(1.0);
        let b = Value::Float(2.0);
        assert_ne!(a.canonical_key(), b.canonical_key());
    }

    #[test]
    fn canonical_key_map_insertion_order_independent() {
        let mut m1 = IndexMap::new();
        m1.insert(Box::<str>::from("a"), Value::Int(1));
        m1.insert(Box::<str>::from("b"), Value::Int(2));

        let mut m2 = IndexMap::new();
        m2.insert(Box::<str>::from("b"), Value::Int(2));
        m2.insert(Box::<str>::from("a"), Value::Int(1));

        let v1 = Value::Map(m1);
        let v2 = Value::Map(m2);
        assert_eq!(v1, v2, "PartialEq says maps with same k/v are equal");
        assert_eq!(
            v1.canonical_key(),
            v2.canonical_key(),
            "canonical_key must agree with PartialEq"
        );
    }

    #[test]
    fn canonical_key_map_different_values_differ() {
        let mut m1 = IndexMap::new();
        m1.insert(Box::<str>::from("a"), Value::Int(1));

        let mut m2 = IndexMap::new();
        m2.insert(Box::<str>::from("a"), Value::Int(2));

        assert_ne!(
            Value::Map(m1).canonical_key(),
            Value::Map(m2).canonical_key()
        );
    }

    #[test]
    fn canonical_key_datetime_same_instant_different_tz() {
        use jiff::tz::TimeZone;

        // 2024-06-15T12:00:00 UTC
        let utc_tz = TimeZone::UTC;
        let zdt_utc = civil::DateTime::new(2024, 6, 15, 12, 0, 0, 0)
            .expect("valid")
            .to_zoned(utc_tz)
            .expect("valid");

        // Same instant expressed as 2024-06-15T14:00:00+02:00
        let plus2 = TimeZone::fixed(jiff::tz::Offset::from_seconds(7200).unwrap());
        let zdt_plus2 = civil::DateTime::new(2024, 6, 15, 14, 0, 0, 0)
            .expect("valid")
            .to_zoned(plus2)
            .expect("valid");

        assert_eq!(zdt_utc, zdt_plus2, "jiff::Zoned compares instants");

        let v1 = Value::DateTime(Box::new(zdt_utc));
        let v2 = Value::DateTime(Box::new(zdt_plus2));
        assert_eq!(v1, v2, "Value::DateTime delegates to Zoned PartialEq");
        assert_eq!(
            v1.canonical_key(),
            v2.canonical_key(),
            "canonical_key must agree with PartialEq"
        );
    }

    #[test]
    fn canonical_key_list_with_nested_float() {
        let a = Value::List(vec![Value::Float(0.0), Value::Int(1)]);
        let b = Value::List(vec![Value::Float(-0.0), Value::Int(1)]);
        assert_eq!(a, b);
        assert_eq!(a.canonical_key(), b.canonical_key());
    }

    #[test]
    fn canonical_key_simple_types_match_debug() {
        // For non-special types, canonical_key uses Debug format
        let vals = vec![
            Value::Null,
            Value::Bool(true),
            Value::Int(42),
            Value::Str("hello".into()),
        ];
        for v in &vals {
            assert_eq!(v.canonical_key(), format!("{:?}", v));
        }
    }

    // ── canonical_hash tests ─────────────────────────────────────────────────

    fn hash_value(v: &Value) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        let mut h = DefaultHasher::new();
        v.canonical_hash(&mut h);
        h.finish()
    }

    #[test]
    fn canonical_hash_float_neg_zero() {
        assert_eq!(
            hash_value(&Value::Float(0.0)),
            hash_value(&Value::Float(-0.0))
        );
    }

    #[test]
    fn canonical_hash_float_nan() {
        assert_eq!(
            hash_value(&Value::Float(f64::NAN)),
            hash_value(&Value::Float(f64::NAN))
        );
    }

    #[test]
    fn canonical_hash_float_distinct() {
        assert_ne!(
            hash_value(&Value::Float(1.0)),
            hash_value(&Value::Float(2.0))
        );
    }

    #[test]
    fn canonical_hash_map_order_independent() {
        let mut m1 = IndexMap::new();
        m1.insert(Box::<str>::from("a"), Value::Int(1));
        m1.insert(Box::<str>::from("b"), Value::Int(2));

        let mut m2 = IndexMap::new();
        m2.insert(Box::<str>::from("b"), Value::Int(2));
        m2.insert(Box::<str>::from("a"), Value::Int(1));

        assert_eq!(hash_value(&Value::Map(m1)), hash_value(&Value::Map(m2)));
    }

    #[test]
    fn canonical_hash_datetime_same_instant() {
        use jiff::tz::TimeZone;
        let utc_tz = TimeZone::UTC;
        let zdt_utc = civil::DateTime::new(2024, 6, 15, 12, 0, 0, 0)
            .expect("valid")
            .to_zoned(utc_tz)
            .expect("valid");
        let plus2 = TimeZone::fixed(jiff::tz::Offset::from_seconds(7200).unwrap());
        let zdt_plus2 = civil::DateTime::new(2024, 6, 15, 14, 0, 0, 0)
            .expect("valid")
            .to_zoned(plus2)
            .expect("valid");
        assert_eq!(
            hash_value(&Value::DateTime(Box::new(zdt_utc))),
            hash_value(&Value::DateTime(Box::new(zdt_plus2)))
        );
    }

    #[test]
    fn canonical_hash_list_with_nested_float() {
        let a = Value::List(vec![Value::Float(0.0), Value::Int(1)]);
        let b = Value::List(vec![Value::Float(-0.0), Value::Int(1)]);
        assert_eq!(hash_value(&a), hash_value(&b));
    }

    #[test]
    fn canonical_hash_different_types_differ() {
        // Int(0) vs Bool(false) should produce different hashes
        assert_ne!(hash_value(&Value::Int(0)), hash_value(&Value::Bool(false)));
        // Int(1) vs Float(1.0) should produce different hashes (different types)
        assert_ne!(hash_value(&Value::Int(1)), hash_value(&Value::Float(1.0)));
    }
    // ── time_to_datetime: valid offset still works ─────────────────────────

    #[test]
    fn time_to_datetime_valid_offset() {
        let t = civil::Time::new(10, 30, 0, 0).expect("valid time");
        let val = Value::Time(t, 19800);
        match val.time_to_datetime() {
            Value::DateTime(zdt) => {
                assert_eq!(zdt.hour(), 10);
                assert_eq!(zdt.minute(), 30);
            }
            other => panic!("expected DateTime, got {other:?}"),
        }
    }

    // ── time_to_datetime: out-of-range offset warns and falls back to UTC ──

    #[test]
    fn time_to_datetime_invalid_offset_falls_back_to_utc() {
        let t = civil::Time::new(12, 0, 0, 0).expect("valid time");
        let val = Value::Time(t, 200_000);
        match val.time_to_datetime() {
            Value::DateTime(zdt) => {
                assert_eq!(zdt.offset().seconds(), 0, "should fall back to UTC");
            }
            other => panic!("expected DateTime with UTC fallback, got {other:?}"),
        }
    }
}
