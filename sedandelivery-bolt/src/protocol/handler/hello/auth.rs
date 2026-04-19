//! Basic-auth credential validation and password configuration.

use std::borrow::Cow;
use std::net::IpAddr;
use std::sync::OnceLock;

use crate::message::types::ServerMessage;
use crate::packstream::value::{BoltMap, BoltMapExt, BoltValue};
use crate::protocol::state::ServerState;

use super::super::{bolt_map, constant_time_eq, Session};
use super::rate_limit;

#[cfg(test)]
tokio::task_local! {
    static REQUIRED_PASSWORD_OVERRIDE: Option<Box<str>>;
}

/// Read `BOLT_PASSWORD` from the environment for the current auth attempt.
pub(crate) fn required_password() -> Option<Cow<'static, str>> {
    #[cfg(test)]
    if let Ok(required_pw) = REQUIRED_PASSWORD_OVERRIDE.try_with(Clone::clone) {
        return required_pw.map(|pw| Cow::Owned(pw.into()));
    }

    static PW: OnceLock<Option<Box<str>>> = OnceLock::new();
    PW.get_or_init(|| {
        std::env::var("BOLT_PASSWORD")
            .ok()
            .map(String::into_boxed_str)
    })
    .as_deref()
    .map(Cow::Borrowed)
}

#[cfg(test)]
pub(crate) async fn with_required_password_override_for_tests<F, T>(
    required_password: Option<&str>,
    future: F,
) -> T
where
    F: std::future::Future<Output = T>,
{
    REQUIRED_PASSWORD_OVERRIDE
        .scope(required_password.map(Box::<str>::from), future)
        .await
}

fn auth_rate_limit_failure(session: &mut Session, ip: IpAddr) -> Vec<ServerMessage> {
    tracing::warn!(%ip, conn_id = session.conn_id, "auth rate limit exceeded");
    session.state = ServerState::Defunct;
    vec![ServerMessage::Failure(bolt_map! {
        "code" => BoltValue::String("Neo.ClientError.Security.AuthenticationRateLimit".to_string()),
        "message" => BoltValue::String("Too many authentication attempts".to_string())
    })]
}

fn unauthorized_failure(session: &mut Session, message: &str) -> Vec<ServerMessage> {
    session.state = ServerState::Defunct;
    vec![ServerMessage::Failure(bolt_map! {
        "code" => BoltValue::String("Neo.ClientError.Security.Unauthorized".to_string()),
        "message" => BoltValue::String(message.to_string())
    })]
}

async fn register_failed_auth_attempt(session: &mut Session) -> Option<Vec<ServerMessage>> {
    let ip = session.peer_ip?;
    let key = rate_limit::normalize_ip(ip);
    if rate_limit::record_failure(&key).await {
        Some(auth_rate_limit_failure(session, ip))
    } else {
        None
    }
}

pub(super) async fn validate_basic_auth(
    session: &mut Session,
    auth: &BoltMap,
    required_pw: Option<&str>,
) -> Option<Vec<ServerMessage>> {
    let required_pw = required_pw?;
    let scheme = auth.get_str("scheme").unwrap_or("");
    if scheme != "basic" {
        if let Some(rate_limited) = register_failed_auth_attempt(session).await {
            return Some(rate_limited);
        }
        return Some(unauthorized_failure(
            session,
            "Authentication scheme 'basic' required",
        ));
    }

    let credentials = auth.get_str("credentials").unwrap_or("");
    if !constant_time_eq(credentials.as_bytes(), required_pw.as_bytes()) {
        if let Some(rate_limited) = register_failed_auth_attempt(session).await {
            return Some(rate_limited);
        }
        return Some(unauthorized_failure(session, "Invalid credentials"));
    }

    if let Some(ip) = session.peer_ip {
        rate_limit::record_success(&rate_limit::normalize_ip(ip)).await;
    }

    None
}
