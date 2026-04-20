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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::GremlinSession;
    use cypher_core::storage::{GraphStore, InMemoryGraphStore};
    use std::sync::Arc;
    use tokio::sync::Mutex;
    use uuid::Uuid;

    // Serialize tests that mutate GREMLIN_PASSWORD (process-global env).
    // Use tokio::sync::Mutex so the guard can be held across .await points
    // without tripping clippy's await_holding_lock lint.
    static ENV_LOCK: Mutex<()> = Mutex::const_new(());

    fn session() -> GremlinSession {
        let store: Arc<dyn GraphStore> = Arc::new(InMemoryGraphStore::new());
        GremlinSession::new(store)
    }

    fn req_json(op: &str, args: serde_json::Value) -> String {
        serde_json::json!({
            "requestId": Uuid::new_v4().to_string(),
            "op": op,
            "processor": "traversal",
            "args": args,
        })
        .to_string()
    }

    #[tokio::test]
    async fn malformed_json_returns_protocol_error() {
        let mut s = session();
        let err = handle_message("{not json}", &mut s).await.unwrap_err();
        assert!(matches!(err, ProtocolError::MalformedRequest(_)));
    }

    #[tokio::test]
    async fn close_op_returns_no_content() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("GREMLIN_PASSWORD");
        let mut s = session();
        let resp = handle_message(&req_json("close", serde_json::json!({})), &mut s)
            .await
            .unwrap();
        assert_eq!(resp.status.code, 204);
    }

    #[tokio::test]
    async fn eval_op_is_unsupported_597() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("GREMLIN_PASSWORD");
        let mut s = session();
        let resp = handle_message(
            &req_json("eval", serde_json::json!({"gremlin": "g.V()"})),
            &mut s,
        )
        .await
        .unwrap();
        assert_eq!(resp.status.code, 597);
        assert!(resp.status.message.to_lowercase().contains("text"));
    }

    #[tokio::test]
    async fn unsupported_op_returns_500() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("GREMLIN_PASSWORD");
        let mut s = session();
        let resp = handle_message(&req_json("frobnicate", serde_json::json!({})), &mut s)
            .await
            .unwrap();
        assert_eq!(resp.status.code, 500);
        assert!(resp.status.message.contains("unsupported"));
    }

    #[tokio::test]
    async fn auth_required_blocks_non_auth_ops() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("GREMLIN_PASSWORD", "secret");
        let mut s = session();
        let resp = handle_message(&req_json("close", serde_json::json!({})), &mut s)
            .await
            .unwrap();
        assert_eq!(resp.status.code, 401);
        std::env::remove_var("GREMLIN_PASSWORD");
    }

    #[tokio::test]
    async fn authentication_succeeds_with_no_password_configured() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("GREMLIN_PASSWORD");
        let mut s = session();
        let resp = handle_message(&req_json("authentication", serde_json::json!({})), &mut s)
            .await
            .unwrap();
        assert_eq!(resp.status.code, 200);
        assert!(s.authenticated);
    }

    #[tokio::test]
    async fn authentication_succeeds_with_correct_plain_password() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("GREMLIN_PASSWORD", "hunter2");
        let mut s = session();
        let resp = handle_message(
            &req_json("authentication", serde_json::json!({"password": "hunter2"})),
            &mut s,
        )
        .await
        .unwrap();
        assert_eq!(resp.status.code, 200);
        assert!(s.authenticated);
        std::env::remove_var("GREMLIN_PASSWORD");
    }

    #[tokio::test]
    async fn authentication_fails_with_wrong_password() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("GREMLIN_PASSWORD", "hunter2");
        let mut s = session();
        let resp = handle_message(
            &req_json("authentication", serde_json::json!({"password": "nope"})),
            &mut s,
        )
        .await
        .unwrap();
        assert_eq!(resp.status.code, 401);
        assert!(!s.authenticated);
        std::env::remove_var("GREMLIN_PASSWORD");
    }

    #[tokio::test]
    async fn authentication_succeeds_with_sasl_token() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("GREMLIN_PASSWORD", "p4ss");
        let mut s = session();
        // SASL PLAIN: \0authzid\0authcid\0password
        let sasl = "\u{0}user\u{0}p4ss";
        let resp = handle_message(
            &req_json("authentication", serde_json::json!({"sasl": sasl})),
            &mut s,
        )
        .await
        .unwrap();
        assert_eq!(resp.status.code, 200);
        std::env::remove_var("GREMLIN_PASSWORD");
    }

    #[tokio::test]
    async fn authentication_fails_with_empty_creds() {
        let _g = ENV_LOCK.lock().await;
        std::env::set_var("GREMLIN_PASSWORD", "p4ss");
        let mut s = session();
        // No sasl, no password — empty submitted, must fail.
        let resp = handle_message(&req_json("authentication", serde_json::json!({})), &mut s)
            .await
            .unwrap();
        assert_eq!(resp.status.code, 401);
        std::env::remove_var("GREMLIN_PASSWORD");
    }

    #[tokio::test]
    async fn bytecode_missing_gremlin_arg_returns_500() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("GREMLIN_PASSWORD");
        let mut s = session();
        let resp = handle_message(&req_json("bytecode", serde_json::json!({})), &mut s)
            .await
            .unwrap();
        assert_eq!(resp.status.code, 500);
        assert!(resp.status.message.contains("missing 'gremlin'"));
    }

    #[tokio::test]
    async fn bytecode_happy_path_returns_success() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("GREMLIN_PASSWORD");
        let mut s = session();
        let bc = serde_json::json!({
            "@type": "g:Bytecode",
            "@value": {"step": [["V"], ["count"]]}
        });
        let resp = handle_message(
            &req_json("bytecode", serde_json::json!({"gremlin": bc})),
            &mut s,
        )
        .await
        .unwrap();
        assert_eq!(resp.status.code, 200);
        // Single column count → typed Int64 with 0.
        let data = resp.result.data.as_array().unwrap();
        assert_eq!(data.len(), 1);
        assert_eq!(data[0]["@value"], 0);
    }

    #[tokio::test]
    async fn bytecode_invalid_returns_500_execution_error() {
        let _g = ENV_LOCK.lock().await;
        std::env::remove_var("GREMLIN_PASSWORD");
        let mut s = session();
        // Step array missing — execute_gremlin returns Err.
        let bc = serde_json::json!({"@type": "g:Bytecode", "@value": {}});
        let resp = handle_message(
            &req_json("bytecode", serde_json::json!({"gremlin": bc})),
            &mut s,
        )
        .await
        .unwrap();
        assert_eq!(resp.status.code, 500);
    }

    #[test]
    fn extract_sasl_password_three_segments() {
        assert_eq!(
            extract_sasl_password("\u{0}user\u{0}pw"),
            Some("pw".to_string())
        );
    }

    #[test]
    fn extract_sasl_password_too_few_segments_is_none() {
        assert_eq!(extract_sasl_password("user:pw"), None);
        assert_eq!(extract_sasl_password("\u{0}only_one"), None);
    }
}
