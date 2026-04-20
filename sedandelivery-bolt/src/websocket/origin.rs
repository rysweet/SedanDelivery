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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::handshake::server::Request;

    fn req_with_origin(origin: Option<&str>) -> Request {
        let mut builder = Request::builder().uri("/").method("GET");
        if let Some(o) = origin {
            builder = builder.header("Origin", o);
        }
        builder.body(()).unwrap()
    }

    #[test]
    fn allowlist_from_env_parses_csv() {
        let policy = OriginPolicy::Allowlist(vec![
            Box::from("https://a.example"),
            Box::from("https://b.example"),
        ]);
        assert!(policy.allows("https://a.example"));
        assert!(policy.allows("https://b.example"));
        assert!(!policy.allows("https://c.example"));
    }

    #[test]
    fn allow_all_allows_anything() {
        assert!(OriginPolicy::AllowAll.allows("anything"));
        assert!(OriginPolicy::AllowAll.allows(""));
    }

    #[test]
    fn reject_presented_origins_blocks_all() {
        let p = OriginPolicy::RejectPresentedOrigins;
        assert!(!p.allows("https://x"));
    }

    #[test]
    fn missing_origin_with_reject_or_allow_all_passes() {
        for p in [OriginPolicy::RejectPresentedOrigins, OriginPolicy::AllowAll] {
            assert!(validate_origin(&req_with_origin(None), &p).is_ok());
        }
    }

    #[test]
    fn missing_origin_with_allowlist_is_forbidden() {
        let p = OriginPolicy::Allowlist(vec![Box::from("https://x")]);
        let err = validate_origin(&req_with_origin(None), &p).unwrap_err();
        assert_eq!(
            err.status(),
            tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn allowed_origin_passes() {
        let p = OriginPolicy::Allowlist(vec![Box::from("https://ok.example")]);
        assert!(validate_origin(&req_with_origin(Some("https://ok.example")), &p).is_ok());
    }

    #[test]
    fn unlisted_origin_is_forbidden() {
        let p = OriginPolicy::Allowlist(vec![Box::from("https://ok.example")]);
        assert!(validate_origin(&req_with_origin(Some("https://nope.example")), &p).is_err());
    }

    #[test]
    fn allow_all_passes_with_origin() {
        assert!(validate_origin(
            &req_with_origin(Some("https://anything")),
            &OriginPolicy::AllowAll
        )
        .is_ok());
    }

    #[test]
    fn reject_presented_origins_blocks_when_origin_present() {
        assert!(validate_origin(
            &req_with_origin(Some("https://x")),
            &OriginPolicy::RejectPresentedOrigins
        )
        .is_err());
    }

    #[test]
    fn origin_policy_from_env_parses_wildcard() {
        // OriginPolicy::from_env is private to the module; exercise via env var
        // for the wildcard branch.
        let prev = std::env::var("BOLT_WS_ALLOWED_ORIGINS").ok();
        std::env::set_var("BOLT_WS_ALLOWED_ORIGINS", "*");
        assert!(matches!(OriginPolicy::from_env(), OriginPolicy::AllowAll));

        std::env::set_var("BOLT_WS_ALLOWED_ORIGINS", "https://a, https://b ,, ");
        match OriginPolicy::from_env() {
            OriginPolicy::Allowlist(list) => {
                assert_eq!(list.len(), 2);
                assert!(list.iter().any(|s| s.as_ref() == "https://a"));
                assert!(list.iter().any(|s| s.as_ref() == "https://b"));
            }
            other => panic!("expected Allowlist, got {other:?}"),
        }

        std::env::set_var("BOLT_WS_ALLOWED_ORIGINS", " , ,");
        assert!(matches!(
            OriginPolicy::from_env(),
            OriginPolicy::RejectPresentedOrigins
        ));

        std::env::remove_var("BOLT_WS_ALLOWED_ORIGINS");
        assert!(matches!(
            OriginPolicy::from_env(),
            OriginPolicy::RejectPresentedOrigins
        ));

        match prev {
            Some(v) => std::env::set_var("BOLT_WS_ALLOWED_ORIGINS", v),
            None => std::env::remove_var("BOLT_WS_ALLOWED_ORIGINS"),
        }
    }
}
