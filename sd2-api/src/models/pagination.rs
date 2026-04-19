//! Pagination query parameters and configurable limit helpers.

use serde::Deserialize;

/// Return the maximum number of items that may be requested in a single page.
///
/// Reads `GQL_API_MAX_PAGE_LIMIT` from the environment on first call (default `10_000`).
pub fn max_page_limit() -> usize {
    use std::sync::OnceLock;
    static VAL: OnceLock<usize> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("GQL_API_MAX_PAGE_LIMIT")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_MAX_PAGE_LIMIT: {v:?}, using default 10000"
                    );
                    None
                }
            })
            .unwrap_or(10_000)
    })
}

/// Return the default page size when the caller does not supply a `limit` parameter.
///
/// Reads `GQL_API_DEFAULT_PAGE_LIMIT` from the environment on first call (default `100`).
pub fn default_page_limit() -> usize {
    use std::sync::OnceLock;
    static VAL: OnceLock<usize> = OnceLock::new();
    *VAL.get_or_init(|| {
        std::env::var("GQL_API_DEFAULT_PAGE_LIMIT")
            .ok()
            .and_then(|v| match v.parse() {
                Ok(val) => Some(val),
                Err(_) => {
                    tracing::warn!(
                        "invalid value for GQL_API_DEFAULT_PAGE_LIMIT: {v:?}, using default 100"
                    );
                    None
                }
            })
            .unwrap_or(100)
    })
}

#[derive(Deserialize)]
pub struct PaginationParams {
    #[serde(default = "default_page_limit")]
    pub limit: usize,
    #[serde(default)]
    pub offset: usize,
}
