use crate::packstream::decoder;
use crate::packstream::encoder;
use crate::packstream::value::{BoltMap, BoltValue};
use crate::protocol::tags::*;

// ---- Error type -----------------------------------------------------------

/// Errors that can occur when decoding a Bolt message.
#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    /// The underlying PackStream data could not be decoded.
    #[error("packstream decode error: {0}")]
    Decode(#[from] decoder::DecodeError),

    /// The top-level PackStream value is not a Structure.
    #[error("expected Structure, got {0}")]
    NotAStructure(String),

    /// The structure tag byte does not match any known message type.
    #[error("unknown message tag: 0x{0:02X}")]
    UnknownTag(u8),

    /// The structure has the wrong number of fields for its tag.
    #[error("wrong field count for tag 0x{tag:02X}: expected {expected}, got {actual}")]
    WrongFieldCount {
        tag: u8,
        expected: usize,
        actual: usize,
    },

    /// A field has an unexpected PackStream type.
    #[error("expected {expected} in field {field}, got {actual}")]
    UnexpectedFieldType {
        field: usize,
        expected: &'static str,
        actual: String,
    },
}

// ---- Client messages ------------------------------------------------------

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

// ---- Server messages ------------------------------------------------------

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

// ---- Decoding (wire bytes -> ClientMessage) -------------------------------

/// Decode a raw PackStream-encoded message into a `ClientMessage`.
///
/// The input `bytes` must contain exactly one PackStream Structure value
/// (the full body of a single de-chunked Bolt message).
pub fn decode_client_message(bytes: &[u8]) -> Result<ClientMessage, MessageError> {
    let (value, _consumed) = decoder::decode(bytes)?;

    let (tag, fields) = match value {
        BoltValue::Structure { tag, fields } => (tag, fields),
        other => {
            return Err(MessageError::NotAStructure(format!("{:?}", other)));
        }
    };

    match tag {
        TAG_HELLO => {
            expect_field_count(tag, 1, &fields)?;
            let extra = extract_map(0, fields)?;
            Ok(ClientMessage::Hello(extra))
        }
        TAG_LOGON => {
            expect_field_count(tag, 1, &fields)?;
            let auth = extract_map(0, fields)?;
            Ok(ClientMessage::Logon(auth))
        }
        TAG_GOODBYE => {
            expect_field_count(tag, 0, &fields)?;
            Ok(ClientMessage::Goodbye)
        }
        TAG_RESET => {
            expect_field_count(tag, 0, &fields)?;
            Ok(ClientMessage::Reset)
        }
        TAG_RUN => {
            expect_field_count(tag, 3, &fields)?;
            let mut it = fields.into_iter();
            let query = extract_string_from_value(0, it.next().unwrap())?;
            let params = extract_map_from_value(1, it.next().unwrap())?;
            let extra = extract_map_from_value(2, it.next().unwrap())?;
            Ok(ClientMessage::Run {
                query,
                params,
                extra,
            })
        }
        TAG_PULL => {
            expect_field_count(tag, 1, &fields)?;
            let extra = extract_map(0, fields)?;
            Ok(ClientMessage::Pull(extra))
        }
        TAG_DISCARD => {
            expect_field_count(tag, 1, &fields)?;
            let extra = extract_map(0, fields)?;
            Ok(ClientMessage::Discard(extra))
        }
        TAG_BEGIN => {
            expect_field_count(tag, 1, &fields)?;
            let extra = extract_map(0, fields)?;
            Ok(ClientMessage::Begin(extra))
        }
        TAG_COMMIT => {
            expect_field_count(tag, 0, &fields)?;
            Ok(ClientMessage::Commit)
        }
        TAG_ROLLBACK => {
            expect_field_count(tag, 0, &fields)?;
            Ok(ClientMessage::Rollback)
        }
        _ => Err(MessageError::UnknownTag(tag)),
    }
}

// ---- Encoding (ServerMessage -> wire bytes) -------------------------------

/// Encode a `ServerMessage` into raw PackStream bytes (not chunked).
///
/// The caller is responsible for passing the result through `chunk_encode`
/// before writing to the wire.
pub fn encode_server_message(msg: &ServerMessage) -> Vec<u8> {
    let mut buf = Vec::new();
    match msg {
        ServerMessage::Success(meta) => {
            encoder::encode(
                &BoltValue::Structure {
                    tag: TAG_SUCCESS,
                    fields: vec![BoltValue::Map(meta.clone())],
                },
                &mut buf,
            );
        }
        ServerMessage::Record(data) => {
            encoder::encode(
                &BoltValue::Structure {
                    tag: TAG_RECORD,
                    fields: vec![BoltValue::List(data.clone())],
                },
                &mut buf,
            );
        }
        ServerMessage::Failure(meta) => {
            encoder::encode(
                &BoltValue::Structure {
                    tag: TAG_FAILURE,
                    fields: vec![BoltValue::Map(meta.clone())],
                },
                &mut buf,
            );
        }
        ServerMessage::Ignored => {
            encoder::encode(
                &BoltValue::Structure {
                    tag: TAG_IGNORED,
                    fields: vec![],
                },
                &mut buf,
            );
        }
    }
    buf
}

// ---- Helpers --------------------------------------------------------------

fn expect_field_count(tag: u8, expected: usize, fields: &[BoltValue]) -> Result<(), MessageError> {
    if fields.len() != expected {
        return Err(MessageError::WrongFieldCount {
            tag,
            expected,
            actual: fields.len(),
        });
    }
    Ok(())
}

/// Extract a `BoltMap` from the first element of a fields vec.
/// Consumes the vec.
fn extract_map(field_index: usize, mut fields: Vec<BoltValue>) -> Result<BoltMap, MessageError> {
    let val = fields.swap_remove(field_index);
    extract_map_from_value(field_index, val)
}

/// Extract a `BoltMap` from a `BoltValue`.
fn extract_map_from_value(field_index: usize, val: BoltValue) -> Result<BoltMap, MessageError> {
    match val {
        BoltValue::Map(entries) => Ok(entries),
        other => Err(MessageError::UnexpectedFieldType {
            field: field_index,
            expected: "Map",
            actual: format!("{:?}", other),
        }),
    }
}

/// Extract a `String` from a `BoltValue`.
fn extract_string_from_value(field_index: usize, val: BoltValue) -> Result<String, MessageError> {
    match val {
        BoltValue::String(s) => Ok(s),
        other => Err(MessageError::UnexpectedFieldType {
            field: field_index,
            expected: "String",
            actual: format!("{:?}", other),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Helper: manually encode a PackStream structure for testing ----------

    /// Encode a tiny structure (up to 15 fields) with a tag byte.
    /// Format: 0xB0|nfields, tag, ...fields
    fn encode_struct(tag: u8, fields: &[BoltValue]) -> Vec<u8> {
        let mut buf = Vec::new();
        encoder::encode(
            &BoltValue::Structure {
                tag,
                fields: fields.to_vec(),
            },
            &mut buf,
        );
        buf
    }

    #[test]
    fn decode_hello() {
        let extra: BoltMap = vec![
            ("user_agent".into(), BoltValue::String("test/1.0".into())),
            ("scheme".into(), BoltValue::String("none".into())),
        ];
        let bytes = encode_struct(TAG_HELLO, &[BoltValue::Map(extra.clone())]);
        let msg = decode_client_message(&bytes).unwrap();
        match msg {
            ClientMessage::Hello(map) => {
                assert_eq!(map.len(), 2);
                assert_eq!(map[0].0, "user_agent");
            }
            other => panic!("expected Hello, got {:?}", other),
        }
    }

    #[test]
    fn decode_goodbye() {
        let bytes = encode_struct(TAG_GOODBYE, &[]);
        let msg = decode_client_message(&bytes).unwrap();
        assert!(matches!(msg, ClientMessage::Goodbye));
    }

    #[test]
    fn decode_reset() {
        let bytes = encode_struct(TAG_RESET, &[]);
        let msg = decode_client_message(&bytes).unwrap();
        assert!(matches!(msg, ClientMessage::Reset));
    }

    #[test]
    fn decode_run() {
        let query = BoltValue::String("RETURN 1 AS n".into());
        let params = BoltValue::Map(vec![]);
        let extra = BoltValue::Map(vec![]);
        let bytes = encode_struct(TAG_RUN, &[query, params, extra]);
        let msg = decode_client_message(&bytes).unwrap();
        match msg {
            ClientMessage::Run {
                query,
                params,
                extra,
            } => {
                assert_eq!(query, "RETURN 1 AS n");
                assert!(params.is_empty());
                assert!(extra.is_empty());
            }
            other => panic!("expected Run, got {:?}", other),
        }
    }

    #[test]
    fn decode_pull() {
        let extra: BoltMap = vec![("n".into(), BoltValue::Int(-1))];
        let bytes = encode_struct(TAG_PULL, &[BoltValue::Map(extra)]);
        let msg = decode_client_message(&bytes).unwrap();
        match msg {
            ClientMessage::Pull(map) => {
                assert_eq!(map.len(), 1);
                assert_eq!(map[0].0, "n");
            }
            other => panic!("expected Pull, got {:?}", other),
        }
    }

    #[test]
    fn decode_discard() {
        let extra: BoltMap = vec![("n".into(), BoltValue::Int(-1))];
        let bytes = encode_struct(TAG_DISCARD, &[BoltValue::Map(extra)]);
        let msg = decode_client_message(&bytes).unwrap();
        assert!(matches!(msg, ClientMessage::Discard(_)));
    }

    #[test]
    fn decode_begin() {
        let extra: BoltMap = vec![];
        let bytes = encode_struct(TAG_BEGIN, &[BoltValue::Map(extra)]);
        let msg = decode_client_message(&bytes).unwrap();
        assert!(matches!(msg, ClientMessage::Begin(_)));
    }

    #[test]
    fn decode_commit() {
        let bytes = encode_struct(TAG_COMMIT, &[]);
        let msg = decode_client_message(&bytes).unwrap();
        assert!(matches!(msg, ClientMessage::Commit));
    }

    #[test]
    fn decode_rollback() {
        let bytes = encode_struct(TAG_ROLLBACK, &[]);
        let msg = decode_client_message(&bytes).unwrap();
        assert!(matches!(msg, ClientMessage::Rollback));
    }

    #[test]
    fn decode_logon() {
        let auth: BoltMap = vec![
            ("scheme".into(), BoltValue::String("basic".into())),
            ("principal".into(), BoltValue::String("neo4j".into())),
            ("credentials".into(), BoltValue::String("pass".into())),
        ];
        let bytes = encode_struct(TAG_LOGON, &[BoltValue::Map(auth)]);
        let msg = decode_client_message(&bytes).unwrap();
        assert!(matches!(msg, ClientMessage::Logon(_)));
    }

    #[test]
    fn decode_unknown_tag() {
        let bytes = encode_struct(0xFF, &[]);
        let err = decode_client_message(&bytes).unwrap_err();
        assert!(matches!(err, MessageError::UnknownTag(0xFF)));
    }

    #[test]
    fn decode_wrong_field_count() {
        // HELLO expects 1 field, send 0
        let bytes = encode_struct(TAG_HELLO, &[]);
        let err = decode_client_message(&bytes).unwrap_err();
        assert!(matches!(
            err,
            MessageError::WrongFieldCount {
                tag: TAG_HELLO,
                expected: 1,
                actual: 0,
            }
        ));
    }

    #[test]
    fn decode_wrong_field_type() {
        // HELLO expects a Map, send an Int
        let bytes = encode_struct(TAG_HELLO, &[BoltValue::Int(42)]);
        let err = decode_client_message(&bytes).unwrap_err();
        assert!(matches!(err, MessageError::UnexpectedFieldType { .. }));
    }

    #[test]
    fn encode_success() {
        let meta: BoltMap = vec![("server".into(), BoltValue::String("test".into()))];
        let msg = ServerMessage::Success(meta);
        let bytes = encode_server_message(&msg);
        // Decode it back and verify structure
        let (val, _) = decoder::decode(&bytes).unwrap();
        match val {
            BoltValue::Structure { tag, fields } => {
                assert_eq!(tag, TAG_SUCCESS);
                assert_eq!(fields.len(), 1);
                assert!(matches!(&fields[0], BoltValue::Map(_)));
            }
            other => panic!("expected Structure, got {:?}", other),
        }
    }

    #[test]
    fn encode_record() {
        let data = vec![BoltValue::Int(1), BoltValue::String("hello".into())];
        let msg = ServerMessage::Record(data);
        let bytes = encode_server_message(&msg);
        let (val, _) = decoder::decode(&bytes).unwrap();
        match val {
            BoltValue::Structure { tag, fields } => {
                assert_eq!(tag, TAG_RECORD);
                assert_eq!(fields.len(), 1);
                match &fields[0] {
                    BoltValue::List(items) => assert_eq!(items.len(), 2),
                    other => panic!("expected List, got {:?}", other),
                }
            }
            other => panic!("expected Structure, got {:?}", other),
        }
    }

    #[test]
    fn encode_failure() {
        let meta: BoltMap = vec![
            (
                "code".into(),
                BoltValue::String("Neo.ClientError.Statement.SyntaxError".into()),
            ),
            ("message".into(), BoltValue::String("bad query".into())),
        ];
        let msg = ServerMessage::Failure(meta);
        let bytes = encode_server_message(&msg);
        let (val, _) = decoder::decode(&bytes).unwrap();
        match val {
            BoltValue::Structure { tag, fields } => {
                assert_eq!(tag, TAG_FAILURE);
                assert_eq!(fields.len(), 1);
            }
            other => panic!("expected Structure, got {:?}", other),
        }
    }

    #[test]
    fn encode_ignored() {
        let msg = ServerMessage::Ignored;
        let bytes = encode_server_message(&msg);
        let (val, _) = decoder::decode(&bytes).unwrap();
        match val {
            BoltValue::Structure { tag, fields } => {
                assert_eq!(tag, TAG_IGNORED);
                assert_eq!(fields.len(), 0);
            }
            other => panic!("expected Structure, got {:?}", other),
        }
    }

    #[test]
    fn roundtrip_run_decode_encode() {
        // Encode a RUN as if the server were echoing back the structure
        let query = "CREATE (n:Person {name: $name}) RETURN n";
        let params: BoltMap = vec![("name".into(), BoltValue::String("Alice".into()))];
        let extra: BoltMap = vec![("db".into(), BoltValue::String("default".into()))];

        let bytes = encode_struct(
            TAG_RUN,
            &[
                BoltValue::String(query.into()),
                BoltValue::Map(params.clone()),
                BoltValue::Map(extra.clone()),
            ],
        );

        let msg = decode_client_message(&bytes).unwrap();
        match msg {
            ClientMessage::Run {
                query: q,
                params: p,
                extra: e,
            } => {
                assert_eq!(q, query);
                assert_eq!(p, params);
                assert_eq!(e, extra);
            }
            other => panic!("expected Run, got {:?}", other),
        }
    }
}
