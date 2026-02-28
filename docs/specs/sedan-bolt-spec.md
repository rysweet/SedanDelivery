# sedan-bolt: Neo4j Bolt Protocol Server for SedanDelivery

## Complete Implementation Specification

Status: DRAFT
Date: 2026-02-28

---

## Table of Contents

1. [Crate Structure](#1-crate-structure)
2. [PackStream Encoding Specification](#2-packstream-encoding-specification)
3. [Message Specification](#3-message-specification)
4. [Server State Machine](#4-server-state-machine)
5. [Connection Handler](#5-connection-handler)
6. [Query Execution Bridge](#6-query-execution-bridge)
7. [Test Plan](#7-test-plan)
8. [Deployment](#8-deployment)
9. [Risk Register](#9-risk-register)

---

## 1. Crate Structure

### 1.1 Decision: No bolt-proto dependency

The `bolt-proto` crate (v0.12.0, by 0xSiO) has only 9.65% documentation coverage,
is designed as a client library, and does not expose server-side message handling.
We will implement our own PackStream encoder/decoder. This is roughly 800 lines of
code for the encoder and 600 for the decoder -- well within a single developer's
capacity and avoids an opaque transitive dependency.

### 1.2 File Layout

```
sedan-bolt/
  Cargo.toml
  src/
    main.rs              -- tokio TCP listener, CLI arg parsing, startup
    lib.rs               -- re-exports for integration tests
    packstream/
      mod.rs             -- pub use encoder, decoder, value
      encoder.rs         -- PackStream serialization (Value -> bytes)
      decoder.rs         -- PackStream deserialization (bytes -> Value)
      value.rs           -- BoltValue enum (protocol-level values)
    message/
      mod.rs             -- pub use types, codec
      types.rs           -- ClientMessage / ServerMessage enums
      codec.rs           -- chunk framing: chunk_encode, chunk_decode
    protocol/
      mod.rs             -- pub use handler, state
      state.rs           -- ServerState enum + transition table
      handler.rs         -- per-message dispatch logic
    connection/
      mod.rs             -- pub use listener, session
      listener.rs        -- TCP accept loop
      session.rs         -- per-connection async task (handshake + message loop)
      handshake.rs       -- Bolt handshake (magic bytes + version negotiation)
    bridge/
      mod.rs             -- pub use executor, types
      executor.rs        -- calls gql-core Engine, maps results
      types.rs           -- BoltNode, BoltRelationship, BoltPath structs
      errors.rs          -- GqlError -> FAILURE mapping
  tests/
    packstream_tests.rs  -- unit tests for encode/decode
    message_tests.rs     -- message framing tests
    integration/
      mod.rs
      handshake_test.rs  -- raw TCP handshake test
      hello_test.rs      -- HELLO auth flow
      query_test.rs      -- RUN+PULL with Python driver
      full_cypher_test.rs
      error_test.rs
```

### 1.3 Cargo.toml

```toml
[package]
name    = "sedan-bolt"
version = "0.1.0"
edition = "2021"
license = "MIT OR Apache-2.0"

[[bin]]
name = "sedan-bolt"
path = "src/main.rs"

[dependencies]
gql-core      = { path = "../gql-core" }
tokio         = { version = "1", features = ["full"] }
tracing       = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
bytes         = "1.5"
indexmap       = { workspace = true }
jiff           = { workspace = true }
thiserror      = { workspace = true }

[dev-dependencies]
tokio-test    = "0.4"
```

### 1.4 Workspace Integration

Add to `/home/azureuser/src/sd2/Cargo.toml`:

```toml
[workspace]
members = [
    # ... existing members ...
    "sedan-bolt",
]
```

### 1.5 Module Responsibilities and Public APIs

**packstream/encoder.rs**
- `pub fn encode(val: &BoltValue, buf: &mut Vec<u8>)`
- `pub fn encode_struct(tag: u8, fields: &[BoltValue], buf: &mut Vec<u8>)`
- No allocations on the encode path for scalars; Vec<u8> is caller-owned.

**packstream/decoder.rs**
- `pub fn decode(buf: &[u8]) -> Result<(BoltValue, usize), DecodeError>`
- Returns the decoded value and the number of bytes consumed.

**packstream/value.rs**
- `pub enum BoltValue { Null, Bool(bool), Int(i64), Float(f64), Bytes(Vec<u8>), String(String), List(Vec<BoltValue>), Map(Vec<(String, BoltValue)>), Structure { tag: u8, fields: Vec<BoltValue> } }`
- Deliberately flat -- structures are not pre-parsed into typed Node/Relationship.

**message/types.rs**
- `pub enum ClientMessage { Hello(BoltMap), Logon(BoltMap), Goodbye, Reset, Run { query: String, params: BoltMap, extra: BoltMap }, Pull(BoltMap), Discard(BoltMap), Begin(BoltMap), Commit, Rollback }`
- `pub enum ServerMessage { Success(BoltMap), Record(Vec<BoltValue>), Failure(BoltMap), Ignored }`

**message/codec.rs**
- `pub fn chunk_encode(msg_bytes: &[u8], out: &mut Vec<u8>)` -- splits into <=16383-byte chunks with 2-byte length headers, terminates with 0x0000.
- `pub async fn chunk_decode<R: AsyncRead>(reader: &mut R) -> Result<Vec<u8>, io::Error>` -- reads chunked message from stream.

**protocol/state.rs**
- `pub enum ServerState` -- see Section 4.
- `pub fn transition(state: &ServerState, msg: &ClientMessage) -> Result<ServerState, ProtocolError>`

**protocol/handler.rs**
- `pub async fn handle_message(session: &mut Session, msg: ClientMessage) -> Result<ServerMessage, ProtocolError>`

**connection/session.rs**
- `pub struct Session { state: ServerState, store: Arc<InMemoryGraphStore>, graph: Option<String>, ... }`
- `pub async fn run(stream: TcpStream, store: Arc<InMemoryGraphStore>)`

**bridge/executor.rs**
- `pub fn execute_query(store: &InMemoryGraphStore, graph: &str, query: &str, params: HashMap<String, Value>) -> Result<QueryResult, GqlError>`
- `pub fn result_to_records(result: &QueryResult, store: &InMemoryGraphStore, graph: &str) -> (Vec<Vec<BoltValue>>, BoltMap)`

---

## 2. PackStream Encoding Specification

### 2.1 Complete Type Mapping Table

All multi-byte integers are big-endian. Marker byte is always the first byte.

| PackStream Type | Marker(s) | Size range | Encoding |
|---|---|---|---|
| Null | `C0` | 0 | Single byte |
| Boolean false | `C2` | 0 | Single byte |
| Boolean true | `C3` | 0 | Single byte |
| TinyInt | `00`-`7F` (pos), `F0`-`FF` (neg) | 0 | Value IS the marker (-16..+127) |
| Int8 | `C8` + 1 byte | -128..-17 | Marker + signed i8 |
| Int16 | `C9` + 2 bytes | -32768..32767 | Marker + signed i16 BE |
| Int32 | `CA` + 4 bytes | -2^31..2^31-1 | Marker + signed i32 BE |
| Int64 | `CB` + 8 bytes | -2^63..2^63-1 | Marker + signed i64 BE |
| Float64 | `C1` + 8 bytes | IEEE 754 | Marker + f64 BE |
| TinyString | `80`-`8F` | 0-15 bytes | `0x80 \| len` + UTF-8 |
| String8 | `D0` + 1 byte len | 0-255 bytes | Marker + u8 len + UTF-8 |
| String16 | `D1` + 2 byte len | 0-65535 bytes | Marker + u16 BE len + UTF-8 |
| String32 | `D2` + 4 byte len | 0-2^31-1 bytes | Marker + u32 BE len + UTF-8 |
| TinyList | `90`-`9F` | 0-15 items | `0x90 \| len` + items |
| List8 | `D4` + 1 byte len | 0-255 items | Marker + u8 count + items |
| List16 | `D5` + 2 byte len | 0-65535 items | Marker + u16 BE count + items |
| List32 | `D6` + 4 byte len | 0-2^31-1 items | Marker + u32 BE count + items |
| TinyMap | `A0`-`AF` | 0-15 entries | `0xA0 \| len` + key-value pairs |
| Map8 | `D8` + 1 byte len | 0-255 entries | Marker + u8 count + pairs |
| Map16 | `D9` + 2 byte len | 0-65535 entries | Marker + u16 BE count + pairs |
| Map32 | `DA` + 4 byte len | 0-2^31-1 entries | Marker + u32 BE count + pairs |
| Bytes8 | `CC` + 1 byte len | 0-255 bytes | Marker + u8 len + raw |
| Bytes16 | `CD` + 2 byte len | 0-65535 bytes | Marker + u16 BE len + raw |
| Bytes32 | `CE` + 4 byte len | 0-2^31-1 bytes | Marker + u32 BE len + raw |
| TinyStructure | `B0`-`BF` | 0-15 fields | `0xB0 \| nfields` + tag byte + fields |

### 2.2 Value Variant -> PackStream Encoding

This maps every variant of `gql_core::types::Value` to its PackStream wire format.

**Value::Null**
- PackStream: `C0`
- Hex: `C0`

**Value::Bool(true)**
- PackStream: Boolean true
- Hex: `C3`

**Value::Bool(false)**
- PackStream: Boolean false
- Hex: `C2`

**Value::Int(i64)**
- PackStream: smallest integer encoding that fits
- Example: `Value::Int(1)` -> `01`
- Example: `Value::Int(42)` -> `2A`
- Example: `Value::Int(-7)` -> `F9`
- Example: `Value::Int(128)` -> `C9 00 80`
- Example: `Value::Int(70000)` -> `CA 00 01 11 70`
- Example: `Value::Int(-2147483649)` -> `CB FF FF FF FF 7F FF FF FF`

**Value::Float(f64)**
- PackStream: Float64
- Example: `Value::Float(1.0)` -> `C1 3F F0 00 00 00 00 00 00`
- Example: `Value::Float(6.283185)` -> `C1 40 19 21 FB 4D 12 D8 4A`

**Value::Str(Box<str>)**
- PackStream: String (smallest encoding that fits byte length)
- Example: `Value::Str("hello")` -> `85 68 65 6C 6C 6F` (TinyString, len=5)
- Example: `Value::Str("")` -> `80` (TinyString, len=0)

**Value::List(Vec<Value>)**
- PackStream: List
- Example: `Value::List(vec![Value::Int(1), Value::Int(2)])` -> `92 01 02`

**Value::Map(IndexMap<Box<str>, Value>)**
- PackStream: Map
- Example: `{name: "Alice"}` -> `A1 84 6E 61 6D 65 85 41 6C 69 63 65`

**Value::Node(NodeId)**
- Resolve to full NodeData from store, then encode as Structure with tag `0x4E`
- PackStream Structure: `B3 4E` (3-field struct, tag 0x4E)
  - Field 0: `id` (Int64 -- node ID as integer)
  - Field 1: `labels` (List of Strings)
  - Field 2: `properties` (Map)
- Example: Node #7 with labels ["Person"], props {name: "Alice"}
  -> `B3 4E 07 91 86 50 65 72 73 6F 6E A1 84 6E 61 6D 65 85 41 6C 69 63 65`

**Value::Edge(EdgeId)**
- Resolve to full EdgeData from store, then encode as Structure with tag `0x52`
- PackStream Structure: `B5 52` (5-field struct, tag 0x52)
  - Field 0: `id` (Int64 -- edge ID)
  - Field 1: `startNodeId` (Int64)
  - Field 2: `endNodeId` (Int64)
  - Field 3: `type` (String -- relationship type)
  - Field 4: `properties` (Map)
- Example: Edge #3, src=1, dst=2, type="KNOWS", props {}
  -> `B5 52 03 01 02 85 4B 4E 4F 57 53 A0`

**Value::Path { nodes, edges }**
- Encode as Structure with tag `0x50`
- PackStream Structure: `B3 50` (3-field struct, tag 0x50)
  - Field 0: `nodes` (List of Node structures -- unique nodes)
  - Field 1: `rels` (List of UnboundRelationship structures, tag `0x72`)
    - UnboundRelationship: `B3 72` (3 fields: id, type, properties)
  - Field 2: `indices` (List of Integers -- path traversal encoding)
    - For each step: +N means follow rel N-1 forward, -N means follow rel N-1 backward
    - Pattern: [rel_idx, node_idx, rel_idx, node_idx, ...]
    - Where rel_idx is 1-based signed (+forward, -backward), node_idx is 0-based

**Value::Date(civil::Date)**
- Structure tag `0x44`, 1 field
- `B1 44` + Int(days since Unix epoch 1970-01-01)
- Example: 2024-01-15 = 19,737 days -> `B1 44 C9 4D 29`

**Value::LocalTime(civil::Time)**
- Structure tag `0x74`, 1 field
- `B1 74` + Int(nanoseconds since midnight)
- Example: 10:30:00 = 37800 * 10^9 ns -> `B1 74 CB 00 00 08 CB 2E 40 00`

**Value::LocalDateTime(civil::DateTime)**
- Structure tag `0x64`, 2 fields
- `B2 64` + Int(seconds since epoch) + Int(nanoseconds adjustment)
- Example: 2024-01-15T10:30:00 -> `B2 64 CA 65 A4 A2 E8 00`

**Value::DateTime(Box<Zoned>)**
- Structure tag `0x69` (DateTimeZoneId), 3 fields
- `B3 69` + Int(seconds since epoch) + Int(nanoseconds) + String(tz_id)
- We use tag `0x69` (zone-by-name) because gql-core stores IANA zone IDs.
- Example: 2024-01-15T10:30:00+01:00[Europe/Berlin]
  -> `B3 69` + Int(epoch_secs) + Int(0) + String("Europe/Berlin")

**Value::Duration(CypherDuration)**
- Structure tag `0x45`, 4 fields
- `B4 45` + Int(months) + Int(days) + Int(seconds) + Int(nanoseconds)
- The `nanos` field from CypherDuration (i128) is decomposed:
  - seconds = nanos / 1_000_000_000
  - remaining_nanos = nanos % 1_000_000_000
- Example: P1Y2M3DT4H -> `B4 45 0E 03 CA 00 00 38 40 00`
  - months=14, days=3, seconds=14400, nanos=0

**Value::Point(GeoPoint)**
- Structure tag `0x58` (Point2D), 3 fields
- `B3 58` + Int(srid) + Float(x) + Float(y)
- SRID mapping:
  - `Crs::Wgs84` -> 4326
  - `Crs::Wgs843d` -> 4979
  - `Crs::Cartesian` -> 7203
  - `Crs::Cartesian3d` -> 9157
- Example: Point(1.5, 2.5, Wgs84) -> `B3 58 C9 10 E6 C1 3F F8 00 00 00 00 00 00 C1 40 04 00 00 00 00 00 00`

### 2.3 Encoding Rules (Encoder Contract)

1. Always use the smallest possible integer encoding.
2. String byte length determines encoding tier, NOT character count.
3. Map keys MUST be Strings.
4. Structure tag byte is always 0x00-0x7F.
5. All multi-byte integers are big-endian (network byte order).

---

## 3. Message Specification

Every Bolt message is a PackStream Structure. The tag byte identifies the message
type. Messages are transmitted using chunked transfer: each chunk has a 2-byte
big-endian length header, and the message is terminated by a zero-length chunk
(`00 00`).

### 3.1 Message Tag Bytes

| Message | Tag | Direction | Struct Fields |
|---|---|---|---|
| HELLO | `0x01` | Client->Server | 1 (extra: Map) |
| GOODBYE | `0x02` | Client->Server | 0 |
| RESET | `0x0F` | Client->Server | 0 |
| RUN | `0x10` | Client->Server | 3 (query: String, params: Map, extra: Map) |
| BEGIN | `0x11` | Client->Server | 1 (extra: Map) |
| COMMIT | `0x12` | Client->Server | 0 |
| ROLLBACK | `0x13` | Client->Server | 0 |
| DISCARD | `0x2F` | Client->Server | 1 (extra: Map) |
| PULL | `0x3F` | Client->Server | 1 (extra: Map) |
| LOGON | `0x6A` | Client->Server | 1 (auth: Map) |
| LOGOFF | `0x6B` | Client->Server | 0 |
| SUCCESS | `0x70` | Server->Client | 1 (metadata: Map) |
| RECORD | `0x71` | Server->Client | 1 (data: List) |
| IGNORED | `0x7E` | Server->Client | 0 |
| FAILURE | `0x7F` | Server->Client | 1 (metadata: Map) |

### 3.2 Chunk Framing

```
Message on wire:
  [u16 BE chunk_size_1][chunk_data_1]
  [u16 BE chunk_size_2][chunk_data_2]
  ...
  [00 00]  <-- zero-length chunk = end of message
```

- Maximum chunk size: 65535 bytes (we will use 16384 as default max chunk size)
- A message MAY be split across multiple chunks
- A zero-length chunk (0x0000) marks the end of a message
- A single message MUST have at least one data chunk before the terminator

### 3.3 Per-Message Specification

#### HELLO (0x01) - Client sends after handshake

Bolt v4.x format (auth embedded in HELLO):
```
Structure { tag: 0x01, fields: [extra] }
  extra: Map {
    "user_agent": String,     -- e.g. "neo4j-python/5.0.0 ..."
    "scheme": String,         -- "basic" or "none"
    "principal": String,      -- username (optional)
    "credentials": String,    -- password (optional)
    "routing": Map,           -- routing context (optional, we ignore)
  }
```

Bolt v5.1+ format (auth separated into LOGON):
```
Structure { tag: 0x01, fields: [extra] }
  extra: Map {
    "user_agent": String,
    // NO auth fields -- auth comes in LOGON
  }
```

Hex example (v4.x HELLO with basic auth, "neo4j"/"password"):
```
B1 01                          -- Struct(1 field), tag 0x01
A4                             -- TinyMap(4 entries)
  8A 75 73 65 72 5F 61 67 65 6E 74  -- "user_agent"
  8F 6E 65 6F 34 6A 2D 70 79 74 68 6F 6E 2F 35 2E 30  -- "neo4j-python/5.0"
  86 73 63 68 65 6D 65              -- "scheme"
  85 62 61 73 69 63                  -- "basic"
  89 70 72 69 6E 63 69 70 61 6C    -- "principal"
  85 6E 65 6F 34 6A                  -- "neo4j"
  8B 63 72 65 64 65 6E 74 69 61 6C 73  -- "credentials"
  88 70 61 73 73 77 6F 72 64        -- "password"
```

SUCCESS response to HELLO:
```
Structure { tag: 0x70, fields: [metadata] }
  metadata: Map {
    "server": String,          -- "SedanDelivery/0.1.0"
    "connection_id": String,   -- "sedan-bolt-1" (monotonic counter)
  }
```

Hex example (SUCCESS for HELLO):
```
B1 70
A2
  86 73 65 72 76 65 72                          -- "server"
  D0 14 53 65 64 61 6E 44 65 6C 69 76 65 72 79 2F 30 2E 31 2E 30  -- "SedanDelivery/0.1.0"
  8D 63 6F 6E 6E 65 63 74 69 6F 6E 5F 69 64    -- "connection_id"
  8C 73 65 64 61 6E 2D 62 6F 6C 74 2D 31        -- "sedan-bolt-1"
```

#### RUN (0x10) - Execute a query

```
Structure { tag: 0x10, fields: [query, parameters, extra] }
  query: String       -- Cypher query text
  parameters: Map     -- query parameters ($param -> value)
  extra: Map {
    "db": String,     -- database/graph name (optional, default "default")
    "mode": String,   -- "r" for read, "w" for write (optional)
  }
```

Hex example (RUN "RETURN 1 AS n" with no params):
```
B3 10                          -- Struct(3 fields), tag 0x10
  8E 52 45 54 55 52 4E 20 31 20 41 53 20 6E  -- "RETURN 1 AS n"
  A0                           -- empty Map
  A0                           -- empty Map extra
```

SUCCESS response to RUN:
```
Structure { tag: 0x70, fields: [metadata] }
  metadata: Map {
    "fields": List<String>,    -- column names from QueryResult.columns
    "t_first": Int,            -- milliseconds (we send 0)
  }
```

#### PULL (0x3F) - Pull records

```
Structure { tag: 0x3F, fields: [extra] }
  extra: Map {
    "n": Int,       -- number of records to pull (-1 = all)
    "qid": Int,     -- query ID (optional, -1 = last query)
  }
```

For each row, server sends RECORD:
```
Structure { tag: 0x71, fields: [data] }
  data: List       -- one value per column, in column order
```

After all records, server sends SUCCESS:
```
Structure { tag: 0x70, fields: [metadata] }
  metadata: Map {
    "bookmark": String,        -- "sedan:tx:1" (monotonic)
    "t_last": Int,             -- elapsed ms (we compute this)
    "type": String,            -- "r" (read), "w" (write), "rw" (read-write)
    "stats": Map,              -- mutation statistics (only if writes occurred)
    "has_more": Bool,          -- false when all records sent
  }
```

Stats map (when mutations occurred):
```
{
  "nodes-created": Int,
  "nodes-deleted": Int,
  "relationships-created": Int,
  "relationships-deleted": Int,
  "properties-set": Int,
  "labels-added": Int,
}
```

#### RESET (0x0F) - Reset connection state

```
Structure { tag: 0x0F, fields: [] }
```
Encoded: `B0 0F`

Server responds with SUCCESS (empty metadata):
```
B1 70 A0
```

#### GOODBYE (0x02) - Client disconnecting

```
Structure { tag: 0x02, fields: [] }
```
Encoded: `B0 02`

No server response. Server closes connection.

#### SUCCESS (0x70) - Generic success

```
Structure { tag: 0x70, fields: [metadata] }
```
Always exactly 1 field (a Map). The Map contents depend on context (see above).

#### RECORD (0x71) - A single result row

```
Structure { tag: 0x71, fields: [data] }
  data: List<BoltValue>   -- values in column order
```

Hex example (RECORD [1]):
```
B1 71 91 01
```

Full wire encoding with chunking:
```
00 04  B1 71 91 01  00 00
```

#### FAILURE (0x7F) - Error response

```
Structure { tag: 0x7F, fields: [metadata] }
  metadata: Map {
    "code": String,     -- Neo4j-compatible error code
    "message": String,  -- human-readable error description
  }
```

Hex example (syntax error):
```
B1 7F
A2
  84 63 6F 64 65                                        -- "code"
  D0 27 4E 65 6F 2E 43 6C 69 65 6E 74 45 72 72 6F 72  -- "Neo.ClientError
  2E 53 74 61 74 65 6D 65 6E 74 2E 53 79 6E 74 61 78  --  .Statement.Syntax
  45 72 72 6F 72                                        --  Error"
  87 6D 65 73 73 61 67 65                                -- "message"
  D0 12 73 79 6E 74 61 78 20 65 72 72 6F 72 20 61 74  -- "syntax error at
  20 62 79 74 65 20 31 30                                --  byte 10"
```

#### IGNORED (0x7E) - Server ignoring message (in FAILED/INTERRUPTED state)

```
Structure { tag: 0x7E, fields: [] }
```
Encoded: `B0 7E`

---

## 4. Server State Machine

### 4.1 States

```rust
/// Bolt server connection state.
///
/// Simplified from the full Neo4j spec: we omit LOGON/LOGOFF (v5.1+),
/// TELEMETRY (v5.4+), and ROUTE. We support Bolt 4.1 where auth is
/// embedded in HELLO.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// Connection established, awaiting handshake completion.
    /// Not a Bolt state per se -- this is pre-protocol.
    Negotiation,

    /// Handshake done, awaiting HELLO message.
    Authentication,

    /// Authenticated and idle. Accepts RUN, BEGIN, RESET, GOODBYE.
    Ready,

    /// Auto-commit query in progress (after RUN, before PULL completes).
    /// Accepts PULL, DISCARD, RESET.
    Streaming,

    /// Explicit transaction started (after BEGIN).
    /// Accepts RUN, COMMIT, ROLLBACK, RESET.
    TxReady,

    /// Streaming within an explicit transaction.
    /// Accepts PULL, DISCARD, RUN, RESET.
    TxStreaming,

    /// A recoverable error occurred. All messages except RESET and
    /// GOODBYE are answered with IGNORED.
    Failed,

    /// Connection interrupted (RESET received mid-operation).
    /// Answered with IGNORED until RESET completes.
    Interrupted,

    /// Terminal state -- connection must be closed.
    Defunct,
}
```

### 4.2 Transition Table

```
State           | Message   | Success -> | Failure ->
----------------|-----------|------------|------------
Authentication  | HELLO     | Ready      | Defunct
Ready           | RUN       | Streaming  | Failed
Ready           | BEGIN     | TxReady    | Failed
Ready           | RESET     | Ready      | Defunct
Ready           | GOODBYE   | Defunct     | Defunct
Streaming       | PULL      | Ready(*)   | Failed
Streaming       | PULL(n)   | Streaming  | Failed     (* has_more=true)
Streaming       | DISCARD   | Ready      | Failed
Streaming       | RESET     | Ready      | Defunct
TxReady         | RUN       | TxStreaming| Failed
TxReady         | COMMIT    | Ready      | Failed
TxReady         | ROLLBACK  | Ready      | Failed
TxReady         | RESET     | Ready      | Defunct
TxStreaming     | PULL      | TxReady(*) | Failed
TxStreaming     | PULL(n)   | TxStreaming | Failed     (* has_more=true)
TxStreaming     | DISCARD   | TxReady    | Failed
TxStreaming     | RUN       | TxStreaming | Failed
TxStreaming     | RESET     | Ready      | Defunct
Failed          | RESET     | Ready      | Defunct
Failed          | (any)     | Failed(IGNORED) | --
Interrupted     | RESET     | Ready      | Defunct
Interrupted     | (any)     | Interrupted(IGNORED) | --
```

### 4.3 Implementation

```rust
impl ServerState {
    /// Returns true if this state accepts the given message.
    pub fn accepts(&self, msg: &ClientMessage) -> bool {
        use ClientMessage::*;
        use ServerState::*;
        match (self, msg) {
            (Authentication, Hello(_)) => true,
            (Ready, Run { .. } | Begin(_) | Reset | Goodbye) => true,
            (Streaming, Pull(_) | Discard(_) | Reset) => true,
            (TxReady, Run { .. } | Commit | Rollback | Reset) => true,
            (TxStreaming, Pull(_) | Discard(_) | Run { .. } | Reset) => true,
            (Failed, Reset | Goodbye) => true,
            (Failed, _) => true, // accept but respond with IGNORED
            (Interrupted, _) => true, // accept but respond with IGNORED
            _ => false,
        }
    }
}
```

### 4.4 Error Handling for Invalid Transitions

When a message arrives that the current state does not accept:
1. Send FAILURE with code `Neo.ClientError.Request.Invalid` and message
   describing the invalid transition.
2. Transition to Defunct.
3. Close the TCP connection.

This is the correct behavior per the Bolt spec: an unexpected message
means the client is misbehaving and the connection is unrecoverable.

---

## 5. Connection Handler

### 5.1 TCP Listener Setup

```rust
// main.rs
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let bind_addr = std::env::var("BOLT_BIND_ADDR")
        .unwrap_or_else(|_| "0.0.0.0:7687".to_string());

    let store = Arc::new(InMemoryGraphStore::new());

    let listener = TcpListener::bind(&bind_addr).await
        .expect("failed to bind Bolt TCP listener");
    tracing::info!("sedan-bolt listening on {}", bind_addr);

    let connection_counter = Arc::new(AtomicU64::new(0));

    loop {
        let (stream, peer_addr) = listener.accept().await
            .expect("accept failed");
        let store = Arc::clone(&store);
        let conn_id = connection_counter.fetch_add(1, Ordering::Relaxed);

        tokio::spawn(async move {
            tracing::info!(%peer_addr, conn_id, "new connection");
            if let Err(e) = session::run(stream, store, conn_id).await {
                tracing::warn!(%peer_addr, conn_id, "session error: {}", e);
            }
            tracing::info!(%peer_addr, conn_id, "connection closed");
        });
    }
}
```

### 5.2 Per-Connection Async Task

```rust
// connection/session.rs
pub struct Session {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    state: ServerState,
    store: Arc<InMemoryGraphStore>,
    conn_id: u64,
    /// The currently selected graph (from "db" extra field, or "default").
    current_graph: String,
    /// Pending query result (between RUN and PULL).
    pending_result: Option<PendingQuery>,
    /// Bookmark counter for this connection.
    bookmark_counter: u64,
}

struct PendingQuery {
    columns: Vec<String>,
    records: Vec<Vec<BoltValue>>,
    stats: MutationStats,
    cursor: usize,
    query_type: String, // "r", "w", "rw"
    start_time: std::time::Instant,
}

pub async fn run(
    stream: TcpStream,
    store: Arc<InMemoryGraphStore>,
    conn_id: u64,
) -> Result<(), SessionError> {
    let (read_half, write_half) = stream.into_split();
    let mut session = Session {
        reader: BufReader::new(read_half),
        writer: write_half,
        state: ServerState::Negotiation,
        store,
        conn_id,
        current_graph: "default".to_string(),
        pending_result: None,
        bookmark_counter: 0,
    };

    // Phase 1: Handshake
    let version = handshake::perform(&mut session.reader, &mut session.writer).await?;
    tracing::info!(conn_id, ?version, "handshake complete");
    session.state = ServerState::Authentication;

    // Phase 2: Message loop
    loop {
        if session.state == ServerState::Defunct {
            break;
        }

        let msg_bytes = codec::chunk_decode(&mut session.reader).await?;
        if msg_bytes.is_empty() {
            break; // connection closed
        }

        let client_msg = types::decode_client_message(&msg_bytes)?;

        // GOODBYE -> close immediately
        if matches!(client_msg, ClientMessage::Goodbye) {
            tracing::debug!(conn_id, "received GOODBYE");
            break;
        }

        let response = handler::handle_message(&mut session, client_msg).await;
        match response {
            Ok(messages) => {
                for msg in messages {
                    let encoded = types::encode_server_message(&msg);
                    let mut wire = Vec::new();
                    codec::chunk_encode(&encoded, &mut wire);
                    session.writer.write_all(&wire).await?;
                }
            }
            Err(e) => {
                tracing::error!(conn_id, "protocol error: {}", e);
                session.state = ServerState::Defunct;
                break;
            }
        }
    }

    Ok(())
}
```

### 5.3 Handshake Implementation

```rust
// connection/handshake.rs

/// The Bolt magic preamble.
const BOLT_MAGIC: [u8; 4] = [0x60, 0x60, 0xB0, 0x17];

/// Bolt version we support: 4.1.
const SUPPORTED_VERSION: (u8, u8) = (4, 1);

pub async fn perform(
    reader: &mut BufReader<OwnedReadHalf>,
    writer: &mut OwnedWriteHalf,
) -> Result<(u8, u8), HandshakeError> {
    // Step 1: Read and verify magic preamble (4 bytes)
    let mut preamble = [0u8; 4];
    reader.read_exact(&mut preamble).await?;
    if preamble != BOLT_MAGIC {
        return Err(HandshakeError::BadMagic(preamble));
    }

    // Step 2: Read 4 version proposals (16 bytes total)
    let mut versions = [0u8; 16];
    reader.read_exact(&mut versions).await?;

    // Step 3: Find a version we support.
    // Each version is 4 bytes: [padding, range, minor, major]
    // Range means "I support major.minor down to major.(minor-range)"
    let mut agreed: Option<(u8, u8)> = None;
    for i in 0..4 {
        let offset = i * 4;
        let _padding = versions[offset];
        let range = versions[offset + 1];
        let minor = versions[offset + 2];
        let major = versions[offset + 3];

        if major == 0 && minor == 0 {
            continue; // unused slot
        }

        // Check if our supported version falls within this range
        if major == SUPPORTED_VERSION.0 {
            let min_minor = if range > minor { 0 } else { minor - range };
            if SUPPORTED_VERSION.1 >= min_minor && SUPPORTED_VERSION.1 <= minor {
                agreed = Some(SUPPORTED_VERSION);
                break;
            }
        }
    }

    // Step 4: Respond with agreed version or 0x00000000
    match agreed {
        Some((major, minor)) => {
            let response = [0u8, 0u8, minor, major];
            writer.write_all(&response).await?;
            writer.flush().await?;
            Ok((major, minor))
        }
        None => {
            writer.write_all(&[0u8; 4]).await?;
            writer.flush().await?;
            Err(HandshakeError::NoSupportedVersion)
        }
    }
}
```

Byte sequence for a successful Bolt 4.1 handshake:

```
Client -> Server:  60 60 B0 17                     (magic preamble)
Client -> Server:  00 01 01 04                     (Bolt 4.1 with range 1 = supports 4.0-4.1)
                   00 00 00 04                     (Bolt 4.0)
                   00 00 00 03                     (Bolt 3)
                   00 00 00 00                     (unused)
Server -> Client:  00 00 01 04                     (agreed: Bolt 4.1)
```

### 5.4 Authentication Handling

In Bolt 4.1, authentication is embedded in the HELLO message. Our handling:

```rust
// protocol/handler.rs (HELLO handler)
fn handle_hello(session: &mut Session, extra: &BoltMap) -> Vec<ServerMessage> {
    // Extract auth fields
    let scheme = extra.get_str("scheme").unwrap_or("none");
    let principal = extra.get_str("principal").unwrap_or("");
    let credentials = extra.get_str("credentials").unwrap_or("");

    // Check against BOLT_PASSWORD environment variable
    let expected_password = std::env::var("BOLT_PASSWORD").ok();

    let auth_ok = match expected_password {
        None => true, // no password configured = allow all
        Some(ref pw) => {
            scheme == "basic" && credentials == pw.as_str()
        }
    };

    if auth_ok {
        session.state = ServerState::Ready;
        vec![ServerMessage::Success(bolt_map! {
            "server" => BoltValue::String("SedanDelivery/0.1.0".into()),
            "connection_id" => BoltValue::String(
                format!("sedan-bolt-{}", session.conn_id)
            ),
        })]
    } else {
        session.state = ServerState::Defunct;
        vec![ServerMessage::Failure(bolt_map! {
            "code" => BoltValue::String(
                "Neo.ClientError.Security.Unauthorized".into()
            ),
            "message" => BoltValue::String(
                "The client is unauthorized due to authentication failure.".into()
            ),
        })]
    }
}
```

---

## 6. Query Execution Bridge

### 6.1 Calling gql-core's Engine

The bridge module translates between the Bolt protocol layer and gql-core.
Key insight: gql-core's `InMemoryGraphStore` implements `GraphStore + Send + Sync`,
so it can be shared across tokio tasks via `Arc<InMemoryGraphStore>`.

```rust
// bridge/executor.rs

use gql_core::executor::{ExecutionBudget, QueryExecutor, QueryResult, MutationStats, Row};
use gql_core::parser::parse_query;
use gql_core::planner::QueryPlanner;
use gql_core::storage::InMemoryGraphStore;
use gql_core::types::Value;
use std::collections::HashMap;

/// Execute a Cypher query against the store.
///
/// This is called from the Bolt protocol handler on a blocking
/// task (spawn_blocking) because gql-core is synchronous.
pub fn execute_query(
    store: &InMemoryGraphStore,
    graph: &str,
    query: &str,
    params: HashMap<String, Value>,
) -> Result<QueryResult, gql_core::error::GqlError> {
    // Ensure graph exists (auto-create "default" graph)
    if !store.graph_exists(graph).unwrap_or(false) {
        if graph == "default" {
            let _ = store.create_graph(graph);
        } else {
            return Err(gql_core::error::GqlError::Storage(
                gql_core::error::StorageError::GraphNotFound {
                    name: graph.to_string(),
                },
            ));
        }
    }

    let ast = parse_query(query)?;
    let planner = QueryPlanner::new(store, graph);
    let plan = planner.plan(&ast)?;
    let mut budget = ExecutionBudget::default_budget();
    let executor = QueryExecutor::with_context(store, &Default::default(), params);
    executor.execute(graph, &plan, &mut budget)
}
```

### 6.2 Mapping QueryResult Rows to RECORD Messages

```rust
// bridge/executor.rs (continued)

use crate::packstream::value::BoltValue;
use crate::bridge::types::{node_to_bolt, edge_to_bolt, path_to_bolt};

/// Convert a QueryResult into a list of RECORD data arrays and
/// completion metadata.
pub fn result_to_records(
    result: &QueryResult,
    store: &InMemoryGraphStore,
    graph: &str,
) -> (Vec<Vec<BoltValue>>, BoltMap) {
    let columns = &result.columns;
    let mut records = Vec::with_capacity(result.rows.len());

    for row in &result.rows {
        let mut record = Vec::with_capacity(columns.len());
        for col in columns {
            let val = row.get(col).unwrap_or(&Value::Null);
            record.push(value_to_bolt(val, store, graph));
        }
        records.push(record);
    }

    // Build completion metadata
    let query_type = determine_query_type(&result.stats);
    let stats_map = stats_to_bolt_map(&result.stats);
    let meta = (query_type, stats_map);

    (records, meta)
}

/// Convert a gql-core Value to a BoltValue.
///
/// Node, Edge, and Path values require store access to resolve
/// the full entity data (labels, properties, endpoints).
fn value_to_bolt(
    val: &Value,
    store: &InMemoryGraphStore,
    graph: &str,
) -> BoltValue {
    match val {
        Value::Null => BoltValue::Null,
        Value::Bool(b) => BoltValue::Bool(*b),
        Value::Int(i) => BoltValue::Int(*i),
        Value::Float(f) => BoltValue::Float(*f),
        Value::Str(s) => BoltValue::String(s.to_string()),
        Value::List(items) => BoltValue::List(
            items.iter().map(|v| value_to_bolt(v, store, graph)).collect()
        ),
        Value::Map(m) => BoltValue::Map(
            m.iter()
                .map(|(k, v)| (k.to_string(), value_to_bolt(v, store, graph)))
                .collect()
        ),
        Value::Node(id) => node_to_bolt(*id, store, graph),
        Value::Edge(id) => edge_to_bolt(*id, store, graph),
        Value::Path { nodes, edges } => path_to_bolt(nodes, edges, store, graph),
        Value::Date(d) => {
            let epoch = jiff::civil::Date::new(1970, 1, 1).unwrap();
            let days = epoch.until(*d).unwrap().get_days() as i64;
            BoltValue::Structure {
                tag: 0x44,
                fields: vec![BoltValue::Int(days)],
            }
        }
        Value::LocalTime(t) => {
            let nanos = t.hour() as i64 * 3_600_000_000_000
                + t.minute() as i64 * 60_000_000_000
                + t.second() as i64 * 1_000_000_000
                + t.subsec_nanosecond() as i64;
            BoltValue::Structure {
                tag: 0x74,
                fields: vec![BoltValue::Int(nanos)],
            }
        }
        Value::LocalDateTime(dt) => {
            let epoch = jiff::civil::DateTime::new(1970, 1, 1, 0, 0, 0, 0).unwrap();
            let span = epoch.until(*dt).unwrap();
            let total_secs = span.get_days() as i64 * 86400
                + span.get_hours() as i64 * 3600
                + span.get_minutes() as i64 * 60
                + span.get_seconds();
            let nanos = dt.subsec_nanosecond() as i64;
            BoltValue::Structure {
                tag: 0x64,
                fields: vec![BoltValue::Int(total_secs), BoltValue::Int(nanos)],
            }
        }
        Value::DateTime(zdt) => {
            let epoch_secs = zdt.timestamp().as_second();
            let nanos = zdt.datetime().subsec_nanosecond() as i64;
            let tz_id = zdt.time_zone().iana_name()
                .unwrap_or("UTC").to_string();
            BoltValue::Structure {
                tag: 0x69,
                fields: vec![
                    BoltValue::Int(epoch_secs),
                    BoltValue::Int(nanos),
                    BoltValue::String(tz_id),
                ],
            }
        }
        Value::Duration(d) => {
            let secs = (d.nanos / 1_000_000_000) as i64;
            let remaining_nanos = (d.nanos % 1_000_000_000) as i64;
            BoltValue::Structure {
                tag: 0x45,
                fields: vec![
                    BoltValue::Int(d.months),
                    BoltValue::Int(d.days),
                    BoltValue::Int(secs),
                    BoltValue::Int(remaining_nanos),
                ],
            }
        }
        Value::Point(p) => {
            let srid = match p.crs {
                gql_core::types::value::Crs::Wgs84 => 4326i64,
                gql_core::types::value::Crs::Wgs843d => 4979,
                gql_core::types::value::Crs::Cartesian => 7203,
                gql_core::types::value::Crs::Cartesian3d => 9157,
            };
            BoltValue::Structure {
                tag: 0x58,
                fields: vec![
                    BoltValue::Int(srid),
                    BoltValue::Float(p.x),
                    BoltValue::Float(p.y),
                ],
            }
        }
    }
}

fn determine_query_type(stats: &MutationStats) -> &'static str {
    let has_writes = stats.nodes_created > 0
        || stats.nodes_deleted > 0
        || stats.edges_created > 0
        || stats.edges_deleted > 0
        || stats.properties_set > 0
        || stats.labels_added > 0;
    if has_writes { "w" } else { "r" }
}
```

### 6.3 Node/Edge/Path to Bolt Structures

```rust
// bridge/types.rs

use gql_core::storage::{GraphStore, InMemoryGraphStore, NodeId, EdgeId, NodeData, EdgeData};
use crate::packstream::value::BoltValue;

/// Encode a resolved Node as Bolt Structure (tag 0x4E).
/// Bolt v4.x Node: 3 fields (id, labels, properties).
pub fn node_to_bolt(
    id: NodeId,
    store: &InMemoryGraphStore,
    graph: &str,
) -> BoltValue {
    match store.get_node(graph, id) {
        Ok(Some(data)) => BoltValue::Structure {
            tag: 0x4E,
            fields: vec![
                BoltValue::Int(data.id.0 as i64),
                BoltValue::List(
                    data.labels.iter()
                        .map(|l| BoltValue::String(l.clone()))
                        .collect()
                ),
                props_to_bolt_map(&data.properties),
            ],
        },
        _ => BoltValue::Null, // node not found -- deleted concurrently
    }
}

/// Encode a resolved Relationship as Bolt Structure (tag 0x52).
/// Bolt v4.x Relationship: 5 fields (id, startNodeId, endNodeId, type, properties).
pub fn edge_to_bolt(
    id: EdgeId,
    store: &InMemoryGraphStore,
    graph: &str,
) -> BoltValue {
    match store.get_edge(graph, id) {
        Ok(Some(data)) => BoltValue::Structure {
            tag: 0x52,
            fields: vec![
                BoltValue::Int(data.id.0 as i64),
                BoltValue::Int(data.src.0 as i64),
                BoltValue::Int(data.dst.0 as i64),
                BoltValue::String(data.rel_type.clone()),
                props_to_bolt_map(&data.properties),
            ],
        },
        _ => BoltValue::Null,
    }
}

/// Encode a Path as Bolt Structure (tag 0x50).
/// 3 fields: nodes (unique list), rels (unbound rels), indices.
pub fn path_to_bolt(
    node_ids: &[NodeId],
    edge_ids: &[EdgeId],
    store: &InMemoryGraphStore,
    graph: &str,
) -> BoltValue {
    // Collect unique nodes in order of first appearance
    let mut unique_nodes: Vec<NodeId> = Vec::new();
    let mut node_index_map = std::collections::HashMap::new();
    for nid in node_ids {
        if !node_index_map.contains_key(nid) {
            node_index_map.insert(*nid, unique_nodes.len());
            unique_nodes.push(*nid);
        }
    }

    // Encode unique nodes
    let bolt_nodes: Vec<BoltValue> = unique_nodes.iter()
        .map(|nid| node_to_bolt(*nid, store, graph))
        .collect();

    // Encode unbound relationships (tag 0x72, 3 fields: id, type, props)
    let bolt_rels: Vec<BoltValue> = edge_ids.iter()
        .map(|eid| {
            match store.get_edge(graph, *eid) {
                Ok(Some(data)) => BoltValue::Structure {
                    tag: 0x72,
                    fields: vec![
                        BoltValue::Int(data.id.0 as i64),
                        BoltValue::String(data.rel_type.clone()),
                        props_to_bolt_map(&data.properties),
                    ],
                },
                _ => BoltValue::Null,
            }
        })
        .collect();

    // Build index sequence.
    // For each step i (0..edges.len()):
    //   rel_index = i+1 if forward, -(i+1) if backward
    //   node_index = index of node_ids[i+1] in unique_nodes
    let mut indices: Vec<BoltValue> = Vec::new();
    for i in 0..edge_ids.len() {
        // Determine direction by checking edge endpoints
        let forward = match store.get_edge(graph, edge_ids[i]) {
            Ok(Some(data)) => data.src == node_ids[i],
            _ => true,
        };
        let rel_idx = if forward { (i + 1) as i64 } else { -((i + 1) as i64) };
        let node_idx = node_index_map[&node_ids[i + 1]] as i64;
        indices.push(BoltValue::Int(rel_idx));
        indices.push(BoltValue::Int(node_idx));
    }

    BoltValue::Structure {
        tag: 0x50,
        fields: vec![
            BoltValue::List(bolt_nodes),
            BoltValue::List(bolt_rels),
            BoltValue::List(indices),
        ],
    }
}

fn props_to_bolt_map(props: &std::collections::HashMap<String, gql_core::types::Value>) -> BoltValue {
    // Reuse value_to_bolt for each property value.
    // Properties never contain Node/Edge/Path, so store access is not needed.
    BoltValue::Map(
        props.iter()
            .map(|(k, v)| (k.clone(), scalar_value_to_bolt(v)))
            .collect()
    )
}
```

### 6.4 Error Mapping (GqlError -> FAILURE)

```rust
// bridge/errors.rs

use gql_core::error::*;

/// Map a GqlError to a Neo4j-compatible error code and message.
pub fn gql_error_to_bolt(err: &GqlError) -> (String, String) {
    match err {
        GqlError::Parse(e) => (
            "Neo.ClientError.Statement.SyntaxError".to_string(),
            e.to_string(),
        ),
        GqlError::Plan(e) => match e {
            PlanError::UndefinedVariable { .. } => (
                "Neo.ClientError.Statement.SyntaxError".to_string(),
                e.to_string(),
            ),
            PlanError::SemanticError { .. } => (
                "Neo.ClientError.Statement.SemanticError".to_string(),
                e.to_string(),
            ),
            _ => (
                "Neo.ClientError.Statement.SyntaxError".to_string(),
                e.to_string(),
            ),
        },
        GqlError::Execution(e) => match e {
            ExecutionError::TypeMismatch { .. } | ExecutionError::InvalidOperator { .. } => (
                "Neo.ClientError.Statement.TypeError".to_string(),
                e.to_string(),
            ),
            ExecutionError::Timeout { .. } => (
                "Neo.TransientError.Transaction.Timeout".to_string(),
                e.to_string(),
            ),
            ExecutionError::UnsupportedFeature { .. } => (
                "Neo.ClientError.Statement.NotSystemDatabaseError".to_string(),
                e.to_string(),
            ),
            _ => (
                "Neo.DatabaseError.Statement.ExecutionFailed".to_string(),
                e.to_string(),
            ),
        },
        GqlError::Storage(e) => match e {
            StorageError::GraphNotFound { .. } => (
                "Neo.ClientError.Database.DatabaseNotFound".to_string(),
                e.to_string(),
            ),
            StorageError::GraphAlreadyExists { .. } => (
                "Neo.ClientError.Database.ExistingDatabaseFound".to_string(),
                e.to_string(),
            ),
            StorageError::TransactionConflict => (
                "Neo.TransientError.Transaction.DeadlockDetected".to_string(),
                e.to_string(),
            ),
            _ => (
                "Neo.DatabaseError.General.UnknownError".to_string(),
                e.to_string(),
            ),
        },
        GqlError::Type(e) => (
            "Neo.ClientError.Statement.TypeError".to_string(),
            e.to_string(),
        ),
    }
}
```

### 6.5 Async Bridge (sync gql-core from async tokio)

gql-core's executor is synchronous. We MUST use `spawn_blocking` to avoid
blocking the tokio runtime:

```rust
// protocol/handler.rs (RUN handler excerpt)

async fn handle_run(
    session: &mut Session,
    query: String,
    params: BoltMap,
    extra: BoltMap,
) -> Result<Vec<ServerMessage>, ProtocolError> {
    let graph = extra.get_str("db")
        .unwrap_or(&session.current_graph)
        .to_string();
    session.current_graph = graph.clone();

    let store = Arc::clone(&session.store);
    let bolt_params = bolt_map_to_value_map(&params);

    let start_time = std::time::Instant::now();

    // Run synchronous engine on blocking thread pool
    let result = tokio::task::spawn_blocking(move || {
        bridge::executor::execute_query(&store, &graph, &query, bolt_params)
    }).await.map_err(|e| ProtocolError::Internal(e.to_string()))?;

    match result {
        Ok(qr) => {
            let columns = qr.columns.clone();
            let (records, (query_type, stats_map)) =
                bridge::executor::result_to_records(&qr, &session.store, &session.current_graph);

            session.pending_result = Some(PendingQuery {
                columns: columns.clone(),
                records,
                stats: qr.stats,
                cursor: 0,
                query_type: query_type.to_string(),
                start_time,
            });

            let fields_list: Vec<BoltValue> = columns.iter()
                .map(|c| BoltValue::String(c.clone()))
                .collect();

            session.state = ServerState::Streaming;
            Ok(vec![ServerMessage::Success(bolt_map! {
                "fields" => BoltValue::List(fields_list),
                "t_first" => BoltValue::Int(0),
            })])
        }
        Err(e) => {
            let (code, message) = bridge::errors::gql_error_to_bolt(&e);
            session.state = ServerState::Failed;
            Ok(vec![ServerMessage::Failure(bolt_map! {
                "code" => BoltValue::String(code),
                "message" => BoltValue::String(message),
            })])
        }
    }
}
```

---

## 7. Test Plan

### Phase 1: PackStream Unit Tests

```rust
// tests/packstream_tests.rs

#[cfg(test)]
mod tests {
    use sedan_bolt::packstream::encoder::encode;
    use sedan_bolt::packstream::decoder::decode;
    use sedan_bolt::packstream::value::BoltValue;

    // --- Null ---
    #[test]
    fn encode_null() {
        let mut buf = Vec::new();
        encode(&BoltValue::Null, &mut buf);
        assert_eq!(buf, vec![0xC0]);
    }

    #[test]
    fn decode_null() {
        let (val, consumed) = decode(&[0xC0]).unwrap();
        assert_eq!(val, BoltValue::Null);
        assert_eq!(consumed, 1);
    }

    // --- Boolean ---
    #[test]
    fn encode_true() {
        let mut buf = Vec::new();
        encode(&BoltValue::Bool(true), &mut buf);
        assert_eq!(buf, vec![0xC3]);
    }

    #[test]
    fn encode_false() {
        let mut buf = Vec::new();
        encode(&BoltValue::Bool(false), &mut buf);
        assert_eq!(buf, vec![0xC2]);
    }

    #[test]
    fn decode_booleans() {
        assert_eq!(decode(&[0xC3]).unwrap().0, BoltValue::Bool(true));
        assert_eq!(decode(&[0xC2]).unwrap().0, BoltValue::Bool(false));
    }

    // --- Integers ---
    #[test]
    fn encode_tiny_int_positive() {
        let mut buf = Vec::new();
        encode(&BoltValue::Int(0), &mut buf);
        assert_eq!(buf, vec![0x00]);

        buf.clear();
        encode(&BoltValue::Int(42), &mut buf);
        assert_eq!(buf, vec![0x2A]);

        buf.clear();
        encode(&BoltValue::Int(127), &mut buf);
        assert_eq!(buf, vec![0x7F]);
    }

    #[test]
    fn encode_tiny_int_negative() {
        let mut buf = Vec::new();
        encode(&BoltValue::Int(-1), &mut buf);
        assert_eq!(buf, vec![0xFF]);

        buf.clear();
        encode(&BoltValue::Int(-16), &mut buf);
        assert_eq!(buf, vec![0xF0]);
    }

    #[test]
    fn encode_int8() {
        let mut buf = Vec::new();
        encode(&BoltValue::Int(-17), &mut buf);
        assert_eq!(buf, vec![0xC8, 0xEF]);

        buf.clear();
        encode(&BoltValue::Int(-128), &mut buf);
        assert_eq!(buf, vec![0xC8, 0x80]);
    }

    #[test]
    fn encode_int16() {
        let mut buf = Vec::new();
        encode(&BoltValue::Int(128), &mut buf);
        assert_eq!(buf, vec![0xC9, 0x00, 0x80]);

        buf.clear();
        encode(&BoltValue::Int(-129), &mut buf);
        assert_eq!(buf, vec![0xC9, 0xFF, 0x7F]);

        buf.clear();
        encode(&BoltValue::Int(32767), &mut buf);
        assert_eq!(buf, vec![0xC9, 0x7F, 0xFF]);
    }

    #[test]
    fn encode_int32() {
        let mut buf = Vec::new();
        encode(&BoltValue::Int(32768), &mut buf);
        assert_eq!(buf, vec![0xCA, 0x00, 0x00, 0x80, 0x00]);

        buf.clear();
        encode(&BoltValue::Int(70000), &mut buf);
        assert_eq!(buf, vec![0xCA, 0x00, 0x01, 0x11, 0x70]);
    }

    #[test]
    fn encode_int64() {
        let mut buf = Vec::new();
        encode(&BoltValue::Int(i64::MAX), &mut buf);
        assert_eq!(buf, vec![0xCB, 0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);

        buf.clear();
        encode(&BoltValue::Int(i64::MIN), &mut buf);
        assert_eq!(buf, vec![0xCB, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn roundtrip_integers() {
        for val in [0, 1, -1, 127, -16, -17, -128, 128, 32767, -32768, 32768, 70000, i64::MAX, i64::MIN] {
            let mut buf = Vec::new();
            encode(&BoltValue::Int(val), &mut buf);
            let (decoded, _) = decode(&buf).unwrap();
            assert_eq!(decoded, BoltValue::Int(val), "roundtrip failed for {val}");
        }
    }

    // --- Float ---
    #[test]
    fn encode_float() {
        let mut buf = Vec::new();
        encode(&BoltValue::Float(1.0), &mut buf);
        assert_eq!(buf[0], 0xC1);
        assert_eq!(buf.len(), 9);
        // IEEE 754: 1.0 = 3FF0000000000000
        assert_eq!(&buf[1..], &[0x3F, 0xF0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn roundtrip_float() {
        for val in [0.0, 1.0, -1.0, 3.14159, f64::MAX, f64::MIN] {
            let mut buf = Vec::new();
            encode(&BoltValue::Float(val), &mut buf);
            let (decoded, _) = decode(&buf).unwrap();
            assert_eq!(decoded, BoltValue::Float(val));
        }
    }

    // --- String ---
    #[test]
    fn encode_tiny_string() {
        let mut buf = Vec::new();
        encode(&BoltValue::String("".into()), &mut buf);
        assert_eq!(buf, vec![0x80]);

        buf.clear();
        encode(&BoltValue::String("A".into()), &mut buf);
        assert_eq!(buf, vec![0x81, 0x41]);

        buf.clear();
        encode(&BoltValue::String("hello".into()), &mut buf);
        assert_eq!(buf, vec![0x85, 0x68, 0x65, 0x6C, 0x6C, 0x6F]);
    }

    #[test]
    fn encode_string_16_chars() {
        // 16 bytes triggers String8 encoding (D0 + 1-byte length)
        let s = "ABCDEFGHIJKLMNOP"; // exactly 16 bytes
        let mut buf = Vec::new();
        encode(&BoltValue::String(s.into()), &mut buf);
        assert_eq!(buf[0], 0xD0);
        assert_eq!(buf[1], 16);
        assert_eq!(&buf[2..], s.as_bytes());
    }

    #[test]
    fn roundtrip_strings() {
        for s in ["", "a", "hello", &"x".repeat(16), &"y".repeat(256), &"z".repeat(70000)] {
            let mut buf = Vec::new();
            encode(&BoltValue::String(s.into()), &mut buf);
            let (decoded, _) = decode(&buf).unwrap();
            assert_eq!(decoded, BoltValue::String(s.into()));
        }
    }

    // --- List ---
    #[test]
    fn encode_tiny_list() {
        let mut buf = Vec::new();
        encode(&BoltValue::List(vec![BoltValue::Int(1), BoltValue::Int(2)]), &mut buf);
        assert_eq!(buf, vec![0x92, 0x01, 0x02]);
    }

    #[test]
    fn encode_empty_list() {
        let mut buf = Vec::new();
        encode(&BoltValue::List(vec![]), &mut buf);
        assert_eq!(buf, vec![0x90]);
    }

    // --- Map ---
    #[test]
    fn encode_empty_map() {
        let mut buf = Vec::new();
        encode(&BoltValue::Map(vec![]), &mut buf);
        assert_eq!(buf, vec![0xA0]);
    }

    #[test]
    fn encode_tiny_map() {
        let mut buf = Vec::new();
        encode(&BoltValue::Map(vec![
            ("a".into(), BoltValue::Int(1)),
        ]), &mut buf);
        assert_eq!(buf, vec![0xA1, 0x81, 0x61, 0x01]);
    }

    // --- Structure ---
    #[test]
    fn encode_structure() {
        let mut buf = Vec::new();
        // Encode a Node structure: B3 4E id labels props
        let node = BoltValue::Structure {
            tag: 0x4E,
            fields: vec![
                BoltValue::Int(1),
                BoltValue::List(vec![BoltValue::String("Person".into())]),
                BoltValue::Map(vec![]),
            ],
        };
        encode(&node, &mut buf);
        assert_eq!(buf[0], 0xB3); // 3 fields
        assert_eq!(buf[1], 0x4E); // Node tag
    }

    // --- Roundtrip all types ---
    #[test]
    fn roundtrip_nested() {
        let complex = BoltValue::Map(vec![
            ("name".into(), BoltValue::String("Alice".into())),
            ("age".into(), BoltValue::Int(30)),
            ("scores".into(), BoltValue::List(vec![
                BoltValue::Float(9.5),
                BoltValue::Float(8.7),
            ])),
            ("active".into(), BoltValue::Bool(true)),
            ("meta".into(), BoltValue::Null),
        ]);
        let mut buf = Vec::new();
        encode(&complex, &mut buf);
        let (decoded, consumed) = decode(&buf).unwrap();
        assert_eq!(consumed, buf.len());
        assert_eq!(decoded, complex);
    }
}
```

### Phase 2: Message Framing Tests

```rust
// tests/message_tests.rs

#[cfg(test)]
mod tests {
    use sedan_bolt::message::codec::{chunk_encode, chunk_decode_sync};

    #[test]
    fn single_chunk_message() {
        let msg = vec![0xB1, 0x70, 0xA0]; // SUCCESS {}
        let mut wire = Vec::new();
        chunk_encode(&msg, &mut wire);
        // Expected: 00 03 B1 70 A0 00 00
        assert_eq!(wire, vec![0x00, 0x03, 0xB1, 0x70, 0xA0, 0x00, 0x00]);
    }

    #[test]
    fn empty_message_rejected() {
        // A message with zero data bytes is invalid
        let msg = vec![];
        let mut wire = Vec::new();
        chunk_encode(&msg, &mut wire);
        // Should still produce valid framing: zero-length chunk only
        assert_eq!(wire, vec![0x00, 0x00]);
    }

    #[test]
    fn multi_chunk_message() {
        // Create a message larger than our chunk size (16384)
        let msg = vec![0xAA; 20000];
        let mut wire = Vec::new();
        chunk_encode(&msg, &mut wire);

        // First chunk: 16384 bytes
        assert_eq!(u16::from_be_bytes([wire[0], wire[1]]), 16384);
        // Second chunk: 3616 bytes
        let offset = 2 + 16384;
        assert_eq!(
            u16::from_be_bytes([wire[offset], wire[offset + 1]]),
            3616
        );
        // Terminator
        let end = wire.len();
        assert_eq!(&wire[end - 2..], &[0x00, 0x00]);
    }

    #[test]
    fn chunk_decode_roundtrip() {
        let original = vec![0xB1, 0x71, 0x91, 0x01]; // RECORD [1]
        let mut wire = Vec::new();
        chunk_encode(&original, &mut wire);
        let decoded = chunk_decode_sync(&wire).unwrap();
        assert_eq!(decoded, original);
    }
}
```

### Phase 3: Handshake Integration Test (Python)

```python
# tests/integration/test_handshake.py
"""
Test raw TCP handshake with sedan-bolt.
Requires sedan-bolt running on localhost:7687.
"""
import socket
import struct
import pytest

BOLT_HOST = "localhost"
BOLT_PORT = 7687
BOLT_MAGIC = b"\x60\x60\xB0\x17"

def test_handshake_bolt_4_1():
    """Verify sedan-bolt accepts Bolt 4.1 handshake."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(5)
    sock.connect((BOLT_HOST, BOLT_PORT))

    try:
        # Send magic + 4 version proposals
        sock.sendall(BOLT_MAGIC)
        # Version 4.1 (range=1 -> supports 4.0 and 4.1)
        sock.sendall(struct.pack(">I", 0x00010104))
        # Version 4.0
        sock.sendall(struct.pack(">I", 0x00000004))
        # Version 3
        sock.sendall(struct.pack(">I", 0x00000003))
        # Unused
        sock.sendall(struct.pack(">I", 0x00000000))

        # Read server response (4 bytes)
        response = sock.recv(4)
        assert len(response) == 4

        # Should agree to 4.1
        version = struct.unpack(">I", response)[0]
        # 0x00000104 = version 4.1
        assert version == 0x00000104, f"Expected Bolt 4.1, got {version:#010x}"
    finally:
        sock.close()


def test_handshake_bad_magic():
    """Verify sedan-bolt rejects invalid magic bytes."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(5)
    sock.connect((BOLT_HOST, BOLT_PORT))

    try:
        # Send wrong magic
        sock.sendall(b"\x00\x00\x00\x00")
        sock.sendall(b"\x00" * 16)

        # Server should close connection
        response = sock.recv(4)
        assert response == b"" or response == b"\x00\x00\x00\x00"
    finally:
        sock.close()


def test_handshake_unsupported_version():
    """Verify sedan-bolt responds with 0x00000000 for unsupported versions."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(5)
    sock.connect((BOLT_HOST, BOLT_PORT))

    try:
        sock.sendall(BOLT_MAGIC)
        # Only offer v2 and v1 (not supported)
        sock.sendall(struct.pack(">I", 0x00000002))
        sock.sendall(struct.pack(">I", 0x00000001))
        sock.sendall(struct.pack(">I", 0x00000000))
        sock.sendall(struct.pack(">I", 0x00000000))

        response = sock.recv(4)
        version = struct.unpack(">I", response)[0]
        assert version == 0x00000000
    finally:
        sock.close()
```

### Phase 4: HELLO Auth Test (Python)

```python
# tests/integration/test_hello.py
"""
Test HELLO authentication with the neo4j Python driver.
Requires: pip install neo4j
Requires: sedan-bolt on localhost:7687
"""
from neo4j import GraphDatabase
import pytest

BOLT_URI = "bolt://localhost:7687"


def test_hello_no_auth():
    """Connect without authentication (when BOLT_PASSWORD is not set)."""
    driver = GraphDatabase.driver(BOLT_URI, auth=None)
    try:
        driver.verify_connectivity()
    finally:
        driver.close()


def test_hello_basic_auth():
    """Connect with basic auth (when BOLT_PASSWORD=testpass)."""
    driver = GraphDatabase.driver(BOLT_URI, auth=("neo4j", "testpass"))
    try:
        driver.verify_connectivity()
    finally:
        driver.close()


def test_hello_wrong_password():
    """Reject wrong password."""
    driver = GraphDatabase.driver(BOLT_URI, auth=("neo4j", "wrongpass"))
    with pytest.raises(Exception):
        driver.verify_connectivity()
    driver.close()


def test_server_agent():
    """Verify server returns SedanDelivery agent string."""
    driver = GraphDatabase.driver(BOLT_URI, auth=None)
    try:
        with driver.session() as session:
            result = session.run("RETURN 1 AS n")
            summary = result.consume()
            agent = summary.server.agent
            assert "SedanDelivery" in agent
    finally:
        driver.close()
```

### Phase 5: RUN+PULL Test (Python)

```python
# tests/integration/test_query.py
"""
Basic query execution tests via the neo4j Python driver.
"""
from neo4j import GraphDatabase
import pytest

BOLT_URI = "bolt://localhost:7687"


@pytest.fixture
def driver():
    d = GraphDatabase.driver(BOLT_URI, auth=None)
    yield d
    d.close()


@pytest.fixture
def session(driver):
    with driver.session(database="test") as s:
        yield s


def test_return_literal(session):
    """RETURN 1 AS n -> single record with n=1."""
    result = session.run("RETURN 1 AS n")
    records = list(result)
    assert len(records) == 1
    assert records[0]["n"] == 1


def test_return_string(session):
    """RETURN 'hello' AS greeting."""
    result = session.run("RETURN 'hello' AS greeting")
    records = list(result)
    assert records[0]["greeting"] == "hello"


def test_return_float(session):
    """RETURN 3.14 AS pi."""
    result = session.run("RETURN 3.14 AS pi")
    records = list(result)
    assert abs(records[0]["pi"] - 3.14) < 0.001


def test_return_null(session):
    """RETURN null AS x."""
    result = session.run("RETURN null AS x")
    records = list(result)
    assert records[0]["x"] is None


def test_return_boolean(session):
    """RETURN true AS t, false AS f."""
    result = session.run("RETURN true AS t, false AS f")
    records = list(result)
    assert records[0]["t"] is True
    assert records[0]["f"] is False


def test_return_list(session):
    """RETURN [1, 2, 3] AS nums."""
    result = session.run("RETURN [1, 2, 3] AS nums")
    records = list(result)
    assert records[0]["nums"] == [1, 2, 3]


def test_return_map(session):
    """RETURN {a: 1, b: 'two'} AS m."""
    result = session.run("RETURN {a: 1, b: 'two'} AS m")
    records = list(result)
    m = records[0]["m"]
    assert m["a"] == 1
    assert m["b"] == "two"


def test_parameters(session):
    """Use query parameters."""
    result = session.run("RETURN $x + $y AS sum", x=10, y=20)
    records = list(result)
    assert records[0]["sum"] == 30


def test_unwind(session):
    """UNWIND [1,2,3] AS x RETURN x."""
    result = session.run("UNWIND [1,2,3] AS x RETURN x")
    records = list(result)
    values = [r["x"] for r in records]
    assert values == [1, 2, 3]


def test_result_summary(session):
    """Verify result summary fields are present."""
    result = session.run("RETURN 1 AS n")
    summary = result.consume()
    assert summary.result_available_after is not None
    assert summary.result_consumed_after is not None
```

### Phase 6: Full Cypher Test Suite (Python)

```python
# tests/integration/test_full_cypher.py
"""
Full Cypher CRUD test suite via Bolt.
"""
from neo4j import GraphDatabase
import pytest

BOLT_URI = "bolt://localhost:7687"


@pytest.fixture
def driver():
    d = GraphDatabase.driver(BOLT_URI, auth=None)
    yield d
    d.close()


@pytest.fixture
def session(driver):
    # Use a unique graph name per test to avoid cross-contamination
    with driver.session(database="cypher_test") as s:
        yield s


def test_create_and_match_node(session):
    """CREATE a node, then MATCH it back."""
    session.run("CREATE (:Person {name: 'Alice', age: 30})")
    result = session.run("MATCH (p:Person {name: 'Alice'}) RETURN p.name AS name, p.age AS age")
    records = list(result)
    assert len(records) == 1
    assert records[0]["name"] == "Alice"
    assert records[0]["age"] == 30


def test_create_relationship(session):
    """CREATE two nodes and a relationship."""
    session.run("""
        CREATE (a:Person {name: 'Alice'})
        CREATE (b:Person {name: 'Bob'})
        CREATE (a)-[:KNOWS {since: 2020}]->(b)
    """)
    result = session.run("""
        MATCH (a:Person)-[r:KNOWS]->(b:Person)
        RETURN a.name AS from, b.name AS to, r.since AS since
    """)
    records = list(result)
    assert len(records) == 1
    assert records[0]["from"] == "Alice"
    assert records[0]["to"] == "Bob"
    assert records[0]["since"] == 2020


def test_return_node_object(session):
    """Verify that returning a node gives a proper Node object."""
    session.run("CREATE (:Animal {species: 'Cat', name: 'Whiskers'})")
    result = session.run("MATCH (a:Animal) RETURN a")
    records = list(result)
    assert len(records) == 1
    node = records[0]["a"]
    assert "Animal" in node.labels
    assert node["species"] == "Cat"
    assert node["name"] == "Whiskers"


def test_return_relationship_object(session):
    """Verify that returning a relationship gives a proper Relationship."""
    session.run("CREATE (:X)-[:R {w: 1.5}]->(:Y)")
    result = session.run("MATCH ()-[r:R]->() RETURN r")
    records = list(result)
    assert len(records) == 1
    rel = records[0]["r"]
    assert rel.type == "R"
    assert rel["w"] == 1.5


def test_aggregation(session):
    """Test count and sum aggregation."""
    session.run("UNWIND range(1, 5) AS i CREATE (:Num {val: i})")
    result = session.run("MATCH (n:Num) RETURN count(n) AS c, sum(n.val) AS s")
    records = list(result)
    assert records[0]["c"] == 5
    assert records[0]["s"] == 15


def test_mutation_stats(session):
    """Verify mutation counters in summary."""
    result = session.run("CREATE (:TestStats {x: 1})")
    summary = result.consume()
    counters = summary.counters
    assert counters.nodes_created == 1
    assert counters.properties_set >= 1


def test_delete(session):
    """DETACH DELETE removes nodes and edges."""
    session.run("CREATE (:Del)-[:R]->(:Del)")
    session.run("MATCH (n:Del) DETACH DELETE n")
    result = session.run("MATCH (n:Del) RETURN count(n) AS c")
    records = list(result)
    assert records[0]["c"] == 0
```

### Phase 7: Error Handling Tests (Python)

```python
# tests/integration/test_errors.py
"""
Error handling and RESET recovery tests.
"""
from neo4j import GraphDatabase
from neo4j.exceptions import CypherSyntaxError, DatabaseError
import pytest

BOLT_URI = "bolt://localhost:7687"


@pytest.fixture
def driver():
    d = GraphDatabase.driver(BOLT_URI, auth=None)
    yield d
    d.close()


@pytest.fixture
def session(driver):
    with driver.session(database="error_test") as s:
        yield s


def test_syntax_error(session):
    """Invalid Cypher should raise CypherSyntaxError."""
    with pytest.raises(CypherSyntaxError):
        result = session.run("THIS IS NOT CYPHER")
        result.consume()


def test_recovery_after_error(session):
    """After an error, subsequent queries should work (driver sends RESET)."""
    try:
        result = session.run("INVALID QUERY")
        result.consume()
    except Exception:
        pass

    # This should work -- the driver sends RESET automatically
    result = session.run("RETURN 42 AS answer")
    records = list(result)
    assert records[0]["answer"] == 42


def test_undefined_variable(session):
    """Reference to undefined variable should fail."""
    with pytest.raises(Exception):
        result = session.run("RETURN x")
        result.consume()


def test_division_by_zero(session):
    """Division by zero should return error, not crash."""
    with pytest.raises(Exception):
        result = session.run("RETURN 1/0 AS x")
        result.consume()
```

### Phase 8: Neo4j TestKit Conformance

The neo4j-drivers/testkit suite is a comprehensive conformance test harness.
For Phase 8, we target these specific test categories from testkit:

**Targeted testkit stubs:**
- `stub/transport/handshake` -- protocol negotiation
- `stub/transport/chunking` -- message framing
- `stub/authentication/basic` -- HELLO auth
- `stub/session/run` -- simple query execution
- `stub/session/result` -- result consumption
- `stub/session/error` -- error handling and recovery

**NOT targeted (out of scope for v1):**
- Routing and cluster tests
- Bookmark management across sessions
- Transaction retries
- Multi-database switching
- TLS/mTLS
- Streaming with partial PULL (n != -1)

We do NOT run testkit directly. Instead, we implement equivalent tests
in Python (Phases 3-7 above) that cover the same protocol interactions
that testkit validates. This avoids the complexity of testkit's Docker-based
stub server infrastructure while covering the critical paths.

---

## 8. Deployment

### 8.1 Architecture

The sedan-bolt binary runs as a SECOND process alongside Valkey in the same
container. Both share the same `InMemoryGraphStore` -- but sedan-bolt creates
its own independent instance. This means:

**Important architectural constraint:** In v1, sedan-bolt's graph data is
SEPARATE from Valkey's graph data. They are two independent InMemoryGraphStore
instances. A node created via `GQL.MUTATE` is NOT visible via Bolt, and vice versa.

**Future (v2):** Shared storage via one of:
- (a) sedan-bolt connects to Valkey via Redis protocol and uses RedisGraphStore
- (b) Both processes share a memory-mapped store
- (c) sedan-bolt replaces gql-module entirely (Bolt-only architecture)

For v1, option (c) is the recommended migration path: run sedan-bolt as the
primary interface, remove the Redis module entirely.

### 8.2 Dockerfile Changes

```dockerfile
# Multi-stage Dockerfile for SedanDelivery
# Stage 1: Build Rust binaries (module .so + sedan-bolt binary)
# Stage 2: Package in Valkey image

# -- Builder --
FROM rust:1.89-bookworm AS builder

WORKDIR /app

RUN apt-get update -q && apt-get install -y -q libclang-dev && rm -rf /var/lib/apt/lists/*

# Copy workspace manifests for layer caching
COPY Cargo.toml ./
COPY gql-core/Cargo.toml ./gql-core/
COPY gql-redis-store/Cargo.toml ./gql-redis-store/
COPY gql-module/Cargo.toml ./gql-module/
COPY gql-fabric-store/Cargo.toml ./gql-fabric-store/
COPY gql-cli/Cargo.toml ./gql-cli/
COPY gql-api/Cargo.toml ./gql-api/
COPY sedan-bolt/Cargo.toml ./sedan-bolt/

# Create stub source files
RUN mkdir -p gql-core/src gql-redis-store/src gql-fabric-store/src \
    gql-module/src gql-cli/src gql-api/src sedan-bolt/src && \
    echo "pub fn stub() {}" > gql-core/src/lib.rs && \
    echo "pub fn stub() {}" > gql-redis-store/src/lib.rs && \
    echo "pub fn stub() {}" > gql-fabric-store/src/lib.rs && \
    echo "pub fn stub() {}" > gql-module/src/lib.rs && \
    echo "fn main() {}" > gql-cli/src/main.rs && \
    echo "fn main() {}" > gql-api/src/main.rs && \
    echo "fn main() {}" > sedan-bolt/src/main.rs && \
    cargo build --release -p gql-module 2>/dev/null || true

# Copy actual source
COPY gql-core/ ./gql-core/
COPY gql-redis-store/ ./gql-redis-store/
COPY gql-fabric-store/ ./gql-fabric-store/
COPY gql-module/ ./gql-module/
COPY gql-cli/ ./gql-cli/
COPY gql-api/ ./gql-api/
COPY sedan-bolt/ ./sedan-bolt/

# Build all binaries
RUN touch gql-core/src/lib.rs gql-redis-store/src/lib.rs \
    gql-fabric-store/src/lib.rs gql-module/src/lib.rs \
    gql-api/src/main.rs sedan-bolt/src/main.rs && \
    cargo build --release -p gql-module && \
    cargo build --release -p gql-api && \
    cargo build --release -p sedan-bolt && \
    ls -lh target/release/libgql_module.so \
           target/release/gql-api \
           target/release/sedan-bolt

# -- Runtime --
FROM valkey/valkey:8.1-bookworm AS runtime

COPY --from=builder /app/target/release/libgql_module.so /usr/local/lib/libgql_module.so
COPY --from=builder /app/target/release/sedan-bolt /usr/local/bin/sedan-bolt

RUN mkdir -p /etc/valkey /data && \
    echo "loadmodule /usr/local/lib/libgql_module.so" > /etc/valkey/valkey-gql.conf && \
    echo "bind 0.0.0.0" >> /etc/valkey/valkey-gql.conf && \
    echo "protected-mode yes" >> /etc/valkey/valkey-gql.conf && \
    echo "loglevel notice" >> /etc/valkey/valkey-gql.conf && \
    echo "dir /data" >> /etc/valkey/valkey-gql.conf && \
    echo "save 60 1" >> /etc/valkey/valkey-gql.conf && \
    echo "appendonly yes" >> /etc/valkey/valkey-gql.conf && \
    echo "appendfsync everysec" >> /etc/valkey/valkey-gql.conf

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

EXPOSE 6379 7687

HEALTHCHECK --interval=30s --timeout=10s --start-period=30s --retries=3 \
    CMD (REDISCLI_AUTH="${VALKEY_PASSWORD:-}" valkey-cli ping | grep -q PONG || valkey-cli ping | grep -q PONG) && \
        (echo "" | timeout 3 nc -z localhost 7687)

RUN chown -R valkey:valkey /etc/valkey /data /entrypoint.sh /usr/local/bin/sedan-bolt
USER valkey

ENTRYPOINT ["/entrypoint.sh"]
```

### 8.3 Entrypoint Changes

Updated `/home/azureuser/src/sd2/entrypoint.sh`:

```bash
#!/bin/sh
set -e

if ! mountpoint -q /data 2>/dev/null; then
    echo "WARNING: /data is not a mounted volume" >&2
fi

if [ -n "${VALKEY_PASSWORD}" ]; then
    case "$VALKEY_PASSWORD" in
      *[[:space:]]*) echo "ERROR: VALKEY_PASSWORD contains whitespace" >&2; exit 1 ;;
    esac
    printf 'requirepass %s\n' "$VALKEY_PASSWORD" >> /etc/valkey/valkey-gql.conf
    echo "INFO: Valkey authentication enabled" >&2
else
    echo "WARNING: VALKEY_PASSWORD not set" >&2
fi

# Start sedan-bolt in the background (if enabled)
if [ "${BOLT_ENABLED:-true}" = "true" ]; then
    BOLT_BIND_ADDR="${BOLT_BIND_ADDR:-0.0.0.0:7687}"
    export BOLT_BIND_ADDR
    export BOLT_PASSWORD="${BOLT_PASSWORD:-${VALKEY_PASSWORD:-}}"
    echo "INFO: Starting sedan-bolt on ${BOLT_BIND_ADDR}" >&2
    /usr/local/bin/sedan-bolt &
    BOLT_PID=$!
    echo "INFO: sedan-bolt started (PID ${BOLT_PID})" >&2
fi

exec valkey-server /etc/valkey/valkey-gql.conf
```

### 8.4 deploy.yml Changes

In the "Create or update Container App" step, add:
- Expose port 7687 for Bolt in addition to 6379 for Redis.
- Add env vars for Bolt configuration.

Key changes to `/home/azureuser/src/sd2/.github/workflows/deploy.yml`:

```yaml
# In the 'az containerapp create' command, change:
  --target-port 6379 \
  --transport tcp \
  --exposed-port 6379 \
# To expose both ports, use additional TCP ingress.
# Azure Container Apps supports a single ingress port, so we need to either:
# Option A: Use two Container Apps (one for Redis, one for Bolt)
# Option B: Use a single Container App with TCP ingress on 7687 (primary)
#           and connect to Redis internally
# Option C: Use Azure Container Apps TCP ingress with multiple ports (preview)

# RECOMMENDED: Option B for v1 -- Bolt as primary external interface
  --target-port 7687 \
  --transport tcp \
  --exposed-port 7687 \

# Add env vars:
  ENV_VAR_ARGS="... BOLT_ENABLED=true BOLT_BIND_ADDR=0.0.0.0:7687 BOLT_PASSWORD=secretref:valkey-password"
```

**Note on Azure Container Apps limitation:** Azure Container Apps currently supports
only a single ingress port per Container App. To expose both 6379 and 7687:
- Deploy two Container Apps in the same CAE (recommended)
- OR use 7687 as the single exposed port and keep Redis internal-only
- OR use Azure Container Apps additional TCP ports (requires Premium tier)

### 8.5 Port Mapping Summary

| Service | Port | Protocol | Exposure |
|---|---|---|---|
| Valkey (Redis) | 6379 | RESP | Internal to CAE |
| sedan-bolt (Bolt) | 7687 | Bolt/TCP | External (client-facing) |
| gql-api (HTTP) | 3001 | HTTP | External (UI-facing) |

---

## 9. Risk Register

| # | Risk | Severity | Likelihood | Mitigation |
|---|---|---|---|---|
| R1 | **bolt-proto crate incompatibility**: Third-party crate does not support server-side usage | High | N/A | Already mitigated: decision to implement own PackStream codec (Section 1.1). |
| R2 | **Neo4j driver version skew**: Python driver sends Bolt v5+ messages that we do not understand | High | Medium | Handshake negotiation limits to v4.1. Drivers fall back gracefully. Test with neo4j-python-driver v5.x which supports Bolt 4.1 fallback. Pin test dependency to `neo4j>=5.0,<7.0`. |
| R3 | **Shared-nothing storage**: Bolt and Redis module have separate InMemoryGraphStore instances | High | Certain | Documented as v1 limitation (Section 8.1). Mitigation: position sedan-bolt as the primary interface and deprecate direct Redis module access. v2 adds shared store. |
| R4 | **Blocking the tokio runtime**: gql-core executor is synchronous, could block async tasks if not handled | High | High (if not mitigated) | Use `tokio::task::spawn_blocking()` for all engine calls (Section 6.5). The default blocking thread pool (512 threads) provides sufficient parallelism. |
| R5 | **Memory exhaustion from large results**: A query returning millions of rows buffers all records in memory before streaming | Medium | Low | ExecutionBudget already limits result set size (default 100K rows). PULL with `n=-1` sends all at once. Future: implement streaming PULL with cursor. |
| R6 | **Missing Bolt message types**: BEGIN/COMMIT/ROLLBACK require transaction support that gql-core lacks | Medium | High | Implement BEGIN/COMMIT/ROLLBACK as no-ops that track state but execute each RUN as auto-commit. The Python driver sends BEGIN/COMMIT in explicit transaction mode but this is semantically equivalent when there is no concurrent access. |
| R7 | **Temporal type precision loss**: CypherDuration uses i128 nanos but Bolt Duration uses i64 seconds + i32 nanos | Low | Low | Decompose: seconds = nanos / 10^9, remaining = nanos % 10^9. Values exceeding i64 seconds (~292 billion years) are truncated with a warning log. |
| R8 | **Path encoding complexity**: Bolt Path structure uses a non-obvious index-based encoding | Medium | Medium | Comprehensive unit tests for path encoding (Phase 1). Test with single-hop, multi-hop, and cyclic paths. Reference FalkorDB implementation for validation. |
| R9 | **Authentication bypass**: If BOLT_PASSWORD is empty/unset, all connections are accepted | Medium | Medium | Document clearly. Default entrypoint inherits VALKEY_PASSWORD. Log a WARNING when running without auth. |
| R10 | **Container healthcheck failure**: If sedan-bolt crashes, healthcheck may still pass if only checking Valkey | Low | Medium | Healthcheck updated to check both Valkey PING and Bolt port 7687 (Section 8.2). |
| R11 | **RESET not clearing all state**: After RESET, pending queries and transaction state must be fully cleared | Medium | Medium | RESET handler explicitly clears pending_result, resets state to Ready, and drops any in-progress computation. Test in Phase 7. |
| R12 | **PackStream integer encoding bugs**: Off-by-one in boundary values (-16/-17, 127/128, etc.) | High | Medium | Exhaustive boundary tests in Phase 1 covering every encoding tier transition. Roundtrip tests for all boundary values. |
| R13 | **TCP connection leak**: If session task panics, the TCP connection may not be cleaned up | Low | Low | Wrap session::run in catch_unwind. Tokio drops the TcpStream when the task ends, closing the connection. Add a connection timeout (5 min idle). |
| R14 | **Azure Container Apps dual-port limitation**: Cannot expose both 6379 and 7687 on a single Container App | Medium | Certain | Use sedan-bolt (port 7687) as the primary external interface. Redis stays internal. See Section 8.4 for alternatives. |
| R15 | **Neo4j driver expects element_id (v5+)**: Bolt v5 added string-based element_id fields to Node/Relationship structures | Low | Low | We negotiate Bolt 4.1 which does not require element_id. Drivers handle v4.x responses correctly. |
| R16 | **Concurrent write safety**: Multiple Bolt connections writing to the same graph simultaneously | Medium | Medium | InMemoryGraphStore uses internal RwLock synchronization. Individual operations are atomic but multi-statement transactions are not isolated. Acceptable for v1. |

---

## Appendix A: BoltMap Helper Macro

```rust
/// Convenience macro for building BoltValue::Map literals.
macro_rules! bolt_map {
    ($($key:expr => $val:expr),* $(,)?) => {
        {
            let mut entries: Vec<(String, BoltValue)> = Vec::new();
            $(entries.push(($key.to_string(), $val));)*
            entries
        }
    };
}
```

## Appendix B: SRID Reference Table

| CRS | SRID | Description |
|---|---|---|
| WGS-84 | 4326 | Geographic 2D (lon/lat) |
| WGS-84 3D | 4979 | Geographic 3D (lon/lat/height) |
| Cartesian | 7203 | Cartesian 2D |
| Cartesian 3D | 9157 | Cartesian 3D |

## Appendix C: Implementation Order

1. **Week 1**: PackStream encoder/decoder + unit tests (Phases 1-2)
2. **Week 2**: Message types + chunk framing + handshake (Phase 3)
3. **Week 3**: HELLO handler + RUN/PULL bridge + Python driver tests (Phases 4-5)
4. **Week 4**: Full Cypher + error handling + deployment (Phases 6-7-8)

## Appendix D: Source References

- Neo4j PackStream specification: https://neo4j.com/docs/bolt/current/packstream/
- Neo4j Bolt message specification: https://neo4j.com/docs/bolt/current/bolt/message/
- Neo4j Bolt handshake specification: https://neo4j.com/docs/bolt/current/bolt/handshake/
- Neo4j Bolt server state machine: https://neo4j.com/docs/bolt/current/bolt/server-state/
- Neo4j Bolt structure semantics: https://neo4j.com/docs/bolt/current/bolt/structure-semantics/
- Neo4j message exchange examples: https://neo4j.com/docs/bolt/current/appendix/message-exchange-examples/
- FalkorDB Bolt implementation: https://github.com/FalkorDB/FalkorDB/tree/master/src/bolt
- bolt-proto Rust crate: https://docs.rs/bolt-proto
- Neo4j Python driver: https://github.com/neo4j/neo4j-python-driver
