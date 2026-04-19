//! Origin allowlist policy for WebSocket upgrade requests.

use std::sync::OnceLock;

use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;

#[derive(Clone, Debug)]
pub(super) enum OriginPolicy {
    RejectPresentedOrigins,
    AllowAll,
    Allowlist(Vec<Box<str>>),
}

impl OriginPolicy {
    fn from_env() -> Self {
        match std::env::var("BOLT_WS_ALLOWED_ORIGINS") {
            Ok(value) => {
                let trimmed = value.trim();
                if trimmed == "*" {
                    Self::AllowAll
                } else {
                    let allowlist = trimmed
                        .split(',')
                        .map(str::trim)
                        .filter(|origin| !origin.is_empty())
                        .map(Box::<str>::from)
                        .collect::<Vec<_>>();
                    if allowlist.is_empty() {
                        tracing::warn!(
                            "BOLT_WS_ALLOWED_ORIGINS is empty; rejecting WebSocket upgrades with Origin headers"
                        );
                        Self::RejectPresentedOrigins
                    } else {
                        Self::Allowlist(allowlist)
                    }
                }
            }
            Err(_) => Self::RejectPresentedOrigins,
        }
    }

    fn allows(&self, origin: &str) -> bool {
        match self {
            Self::RejectPresentedOrigins => false,
            Self::AllowAll => true,
            Self::Allowlist(allowlist) => {
                allowlist.iter().any(|allowed| allowed.as_ref() == origin)
            }
        }
    }
}

pub(super) fn origin_policy_from_env() -> &'static OriginPolicy {
    static POLICY: OnceLock<OriginPolicy> = OnceLock::new();
    POLICY.get_or_init(OriginPolicy::from_env)
}

fn forbidden_origin(message: &str) -> ErrorResponse {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(Some(message.to_string()))
        .expect("forbidden websocket response should be buildable")
}

// `ErrorResponse` is `Response<Option<String>>` from tungstenite, which is
// larger than Clippy's default threshold for `result_large_err`.  The large
// type is unavoidable here: it is the exact error type required by the
// `accept_hdr_async` callback signature in `tokio-tungstenite`.  We cannot
// use a smaller wrapper because the callback must return `Result<_, ErrorResponse>`
// — there is no conversion path that avoids materialising an `ErrorResponse`
// at the call site.  Heap-boxing the error would add an allocation on every
// rejected WebSocket handshake for no correctness benefit.
#[allow(clippy::result_large_err)]
pub(super) fn validate_origin(
    request: &Request,
    policy: &OriginPolicy,
) -> Result<(), ErrorResponse> {
    let Some(origin) = request.headers().get("Origin") else {
        return match policy {
            OriginPolicy::RejectPresentedOrigins | OriginPolicy::AllowAll => Ok(()),
            OriginPolicy::Allowlist(_) => Err(forbidden_origin(
                "WebSocket Origin header is required when BOLT_WS_ALLOWED_ORIGINS is configured",
            )),
        };
    };

    let origin = origin
        .to_str()
        .map_err(|_| forbidden_origin("WebSocket Origin must be valid UTF-8"))?;

    if policy.allows(origin) {
        Ok(())
    } else {
        Err(forbidden_origin(
            "WebSocket Origin is not allowed; configure BOLT_WS_ALLOWED_ORIGINS",
        ))
    }
}
