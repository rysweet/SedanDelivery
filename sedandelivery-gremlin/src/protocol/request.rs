//! Gremlin RequestMessage deserialization (GraphSON v3 framing).
//!
//! A Gremlin request looks like:
//! ```json
//! {
//!   "requestId": "cb682578-9d92-4499-9ebc-5c6aa73c5397",
//!   "op": "bytecode",
//!   "processor": "traversal",
//!   "args": { "gremlin": { ... }, "aliases": {"g": "g"} }
//! }
//! ```

use std::collections::HashMap;

use serde::Deserialize;
use uuid::Uuid;

/// Incoming Gremlin request message.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestMessage {
    pub request_id: Uuid,
    pub op: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub processor: String,
    #[serde(default)]
    pub args: HashMap<String, serde_json::Value>,
}

impl RequestMessage {
    /// Parse a JSON text frame into a `RequestMessage`.
    pub fn from_json(text: &str) -> Result<Self, super::ProtocolError> {
        serde_json::from_str(text)
            .map_err(|e| super::ProtocolError::MalformedRequest(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bytecode_request() {
        let json = r#"{
            "requestId": "cb682578-9d92-4499-9ebc-5c6aa73c5397",
            "op": "bytecode",
            "processor": "traversal",
            "args": {
                "gremlin": {"@type": "g:Bytecode", "@value": {"step": [["V"], ["count"]]}},
                "aliases": {"g": "g"}
            }
        }"#;
        let msg = RequestMessage::from_json(json).unwrap();
        assert_eq!(msg.op, "bytecode");
        assert_eq!(msg.processor, "traversal");
        assert!(msg.args.contains_key("gremlin"));
    }

    #[test]
    fn parse_eval_request() {
        let json = r#"{
            "requestId": "00000000-0000-0000-0000-000000000001",
            "op": "eval",
            "processor": "",
            "args": {"gremlin": "g.V().count()"}
        }"#;
        let msg = RequestMessage::from_json(json).unwrap();
        assert_eq!(msg.op, "eval");
    }

    #[test]
    fn missing_request_id_fails() {
        let json = r#"{"op": "bytecode"}"#;
        assert!(RequestMessage::from_json(json).is_err());
    }
}
