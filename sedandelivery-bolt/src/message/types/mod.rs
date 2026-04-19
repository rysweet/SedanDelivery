//! Bolt message types: error, client/server message enums, and codec functions.
//!
//! This module preserves the original flat API at `message::types::*` while
//! splitting the implementation into focused submodules:
//! - `error`: [`MessageError`]
//! - `messages`: [`ClientMessage`] and [`ServerMessage`] enums
//! - `decode`: [`decode_client_message`]
//! - `encode`: [`encode_server_message`]

mod decode;
mod encode;
mod error;
mod messages;

pub use decode::decode_client_message;
pub use encode::encode_server_message;
pub use error::MessageError;
pub use messages::{ClientMessage, ServerMessage};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packstream::decoder;
    use crate::packstream::encoder;
    use crate::packstream::value::{BoltMap, BoltValue};
    use crate::protocol::tags::*;

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
