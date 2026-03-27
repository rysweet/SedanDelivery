//! Message dispatch — routes incoming Gremlin operations to the appropriate handler.

use super::request::RequestMessage;
use super::response::{ResponseMessage, StatusCode};
use super::ProtocolError;
use crate::bridge;
use crate::connection::GremlinSession;

/// Handle a single text-frame Gremlin message and produce a response.
pub async fn handle_message(
    text: &str,
    session: &mut GremlinSession,
) -> Result<ResponseMessage, ProtocolError> {
    let req = RequestMessage::from_json(text)?;

    // Authentication gate: if password is required, only allow auth op through.
    if session.requires_auth() && req.op != "authentication" {
        return Ok(ResponseMessage::error(
            req.request_id,
            StatusCode::Unauthorized,
            "authentication required".to_string(),
        ));
    }

    match req.op.as_str() {
        "authentication" => handle_authentication(&req, session),
        "bytecode" => handle_bytecode(&req, session).await,
        "eval" => Ok(ResponseMessage::error(
            req.request_id,
            StatusCode::ScriptEvaluationError,
            "text evaluation not supported; submit bytecode instead".to_string(),
        )),
        "close" => Ok(ResponseMessage::no_content(req.request_id)),
        other => Ok(ResponseMessage::error(
            req.request_id,
            StatusCode::ServerError,
            format!("unsupported op: {other}"),
        )),
    }
}

fn handle_authentication(
    req: &RequestMessage,
    session: &mut GremlinSession,
) -> Result<ResponseMessage, ProtocolError> {
    let expected = std::env::var("GREMLIN_PASSWORD").unwrap_or_default();
    if expected.is_empty() {
        // No password configured — authentication always succeeds.
        session.authenticated = true;
        return Ok(ResponseMessage::success(
            req.request_id,
            serde_json::Value::Null,
        ));
    }

    // SASL PLAIN: args.sasl is a base64-encoded "\0username\0password" string.
    // For simplicity in Phase 1 we also accept a plain-text "password" arg.
    let submitted = req
        .args
        .get("sasl")
        .and_then(|v| v.as_str())
        .and_then(extract_sasl_password)
        .or_else(|| {
            req.args
                .get("password")
                .and_then(|v| v.as_str())
                .map(String::from)
        })
        .unwrap_or_default();

    if submitted == expected {
        session.authenticated = true;
        Ok(ResponseMessage::success(
            req.request_id,
            serde_json::Value::Null,
        ))
    } else {
        Ok(ResponseMessage::error(
            req.request_id,
            StatusCode::Unauthorized,
            "bad credentials".to_string(),
        ))
    }
}

/// Extract the password from a SASL PLAIN token ("\0user\0password").
///
/// Some clients send the token as raw UTF-8 with NUL separators; base64
/// decoding is deferred to a future phase.
fn extract_sasl_password(encoded: &str) -> Option<String> {
    // SASL PLAIN is \0authzid\0authcid\0password — we want the last segment.
    let parts: Vec<&str> = encoded.split('\0').collect();
    if parts.len() >= 3 {
        return Some(parts[2].to_string());
    }
    None
}

async fn handle_bytecode(
    req: &RequestMessage,
    session: &GremlinSession,
) -> Result<ResponseMessage, ProtocolError> {
    let bytecode_json = match req.args.get("gremlin") {
        Some(bc) => bc,
        None => {
            return Ok(ResponseMessage::error(
                req.request_id,
                StatusCode::ServerError,
                "missing 'gremlin' bytecode in args".to_string(),
            ));
        }
    };

    let store = std::sync::Arc::clone(&session.store);
    let graph = session.graph.clone();
    let bytecode = bytecode_json.clone();
    let request_id = req.request_id;

    match tokio::task::spawn_blocking(move || {
        bridge::executor::execute_gremlin(&*store, &graph, &bytecode)
    })
    .await
    .map_err(|e| ProtocolError::Internal(format!("gremlin execution task panicked: {e}")))?
    {
        Ok(data) => Ok(ResponseMessage::success(
            request_id,
            serde_json::Value::Array(data),
        )),
        Err(e) => Ok(ResponseMessage::error(
            request_id,
            StatusCode::ServerError,
            e.to_string(),
        )),
    }
}
