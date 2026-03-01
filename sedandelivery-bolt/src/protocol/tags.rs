// Named constants for Bolt protocol message and structure tag bytes.
//
// Centralizes all magic tag values used across the crate so that every
// module references the same canonical constant rather than bare hex
// literals.  The values are taken from the Bolt v4.x specification.

// ── Request message tags ────────────────────────────────────────────────────

/// HELLO (0x01) -- client authentication handshake.
pub const TAG_HELLO: u8 = 0x01;
/// GOODBYE (0x02) -- client signals session end.
pub const TAG_GOODBYE: u8 = 0x02;
/// RESET (0x0F) -- reset the session to Ready state.
pub const TAG_RESET: u8 = 0x0F;
/// RUN (0x10) -- submit a Cypher query.
pub const TAG_RUN: u8 = 0x10;
/// BEGIN (0x11) -- start an explicit transaction.
pub const TAG_BEGIN: u8 = 0x11;
/// COMMIT (0x12) -- commit the current explicit transaction.
pub const TAG_COMMIT: u8 = 0x12;
/// ROLLBACK (0x13) -- roll back the current explicit transaction.
pub const TAG_ROLLBACK: u8 = 0x13;
/// DISCARD (0x2F) -- discard pending result records.
pub const TAG_DISCARD: u8 = 0x2F;
/// PULL (0x3F) -- pull pending result records.
pub const TAG_PULL: u8 = 0x3F;
/// LOGON (0x6A) -- deferred authentication (Bolt 5.1+).
pub const TAG_LOGON: u8 = 0x6A;

// ── Response message tags ───────────────────────────────────────────────────

/// SUCCESS (0x70) -- message processed successfully.
pub const TAG_SUCCESS: u8 = 0x70;
/// RECORD (0x71) -- a single result record.
pub const TAG_RECORD: u8 = 0x71;
/// IGNORED (0x7E) -- message was ignored (session in Failed/Interrupted state).
pub const TAG_IGNORED: u8 = 0x7E;
/// FAILURE (0x7F) -- message processing failed.
pub const TAG_FAILURE: u8 = 0x7F;

// ── Structure tags for graph types ──────────────────────────────────────────

/// Node structure (0x4E, ASCII 'N').
pub const TAG_NODE: u8 = 0x4E;
/// Path structure (0x50, ASCII 'P').
pub const TAG_PATH: u8 = 0x50;
/// Relationship structure (0x52, ASCII 'R').
pub const TAG_RELATIONSHIP: u8 = 0x52;
/// Unbound relationship structure (0x72, ASCII 'r').
pub const TAG_UNBOUND_RELATIONSHIP: u8 = 0x72;

// ── Structure tags for temporal types ───────────────────────────────────────

/// Date (0x44, ASCII 'D') -- days since 1970-01-01.
pub const TAG_DATE: u8 = 0x44;
/// Duration (0x45, ASCII 'E') -- months, days, seconds, nanoseconds.
pub const TAG_DURATION: u8 = 0x45;
/// DateTimeZoneOffset, UTC mode (0x49, ASCII 'I') -- epoch_secs, nanos, offset.
pub const TAG_DATETIME_ZONE_OFFSET_UTC: u8 = 0x49;
/// DateTimeZoneOffset, wall-clock mode (0x46, ASCII 'F') -- epoch_secs, nanos, offset.
pub const TAG_DATETIME_ZONE_OFFSET_WALL: u8 = 0x46;
/// Time with UTC offset (0x54, ASCII 'T') -- nanoseconds, offset_seconds.
pub const TAG_TIME: u8 = 0x54;
/// Point2D (0x58, ASCII 'X') -- srid, x, y.
pub const TAG_POINT_2D: u8 = 0x58;
/// Point3D (0x59, ASCII 'Y') -- srid, x, y, z.
pub const TAG_POINT_3D: u8 = 0x59;
/// LocalDateTime (0x64, ASCII 'd') -- epoch_seconds, nanos.
pub const TAG_LOCAL_DATE_TIME: u8 = 0x64;
/// DateTimeZoneId, wall-clock mode (0x66, ASCII 'f') -- epoch_secs, nanos, tz_id.
pub const TAG_DATETIME_ZONE_ID_WALL: u8 = 0x66;
/// DateTimeZoneId, UTC mode (0x69, ASCII 'i') -- epoch_secs, nanos, tz_id.
pub const TAG_DATETIME_ZONE_ID_UTC: u8 = 0x69;
/// LocalTime (0x74, ASCII 't') -- nanoseconds since midnight.
pub const TAG_LOCAL_TIME: u8 = 0x74;
