use crate::packstream::decoder;
use crate::packstream::value::{BoltMap, BoltValue};
use crate::protocol::tags::*;

use super::error::MessageError;
use super::messages::ClientMessage;

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
            let query = extract_string_from_value(0, it.next().unwrap())?; // safe: field count verified as 3
            let params = extract_map_from_value(1, it.next().unwrap())?; // safe: field count verified as 3
            let extra = extract_map_from_value(2, it.next().unwrap())?; // safe: field count verified as 3
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
