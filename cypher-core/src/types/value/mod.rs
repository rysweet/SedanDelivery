mod accessors;
mod canonical;
mod compare;
mod convert;
mod display;

pub use display::format_cypher_duration;
pub use display::format_offset_str;

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
