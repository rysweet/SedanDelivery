use crate::packstream::encoder;
use crate::packstream::value::BoltValue;
use crate::protocol::tags::*;

use super::messages::ServerMessage;

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
