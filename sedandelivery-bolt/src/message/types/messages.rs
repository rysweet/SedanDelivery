use crate::packstream::value::{BoltMap, BoltValue};

/// Messages sent by the client to the server.
#[derive(Debug, Clone)]
pub enum ClientMessage {
    /// HELLO (0x01) -- 1 field: extra map.
    Hello(BoltMap),
    /// LOGON (0x6A) -- 1 field: auth map.
    Logon(BoltMap),
    /// GOODBYE (0x02) -- 0 fields.
    Goodbye,
    /// RESET (0x0F) -- 0 fields.
    Reset,
    /// RUN (0x10) -- 3 fields: query, params, extra.
    Run {
        query: String,
        params: BoltMap,
        extra: BoltMap,
    },
    /// PULL (0x3F) -- 1 field: extra map.
    Pull(BoltMap),
    /// DISCARD (0x2F) -- 1 field: extra map.
    Discard(BoltMap),
    /// BEGIN (0x11) -- 1 field: extra map.
    Begin(BoltMap),
    /// COMMIT (0x12) -- 0 fields.
    Commit,
    /// ROLLBACK (0x13) -- 0 fields.
    Rollback,
}

/// Messages sent by the server to the client.
#[derive(Debug, Clone)]
pub enum ServerMessage {
    /// SUCCESS (0x70) -- 1 field: metadata map.
    Success(BoltMap),
    /// RECORD (0x71) -- 1 field: data list.
    Record(Vec<BoltValue>),
    /// FAILURE (0x7F) -- 1 field: metadata map.
    Failure(BoltMap),
    /// IGNORED (0x7E) -- 0 fields.
    Ignored,
}
