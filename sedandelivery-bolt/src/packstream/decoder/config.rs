/// Maximum nesting depth for recursive structures (lists, maps, structures).
/// Prevents stack overflow from maliciously crafted deeply-nested messages.
/// Configurable via `BOLT_MAX_DECODE_DEPTH` env var (default 128).
pub(super) fn max_decode_depth() -> usize {
    static DEPTH: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *DEPTH.get_or_init(|| {
        std::env::var("BOLT_MAX_DECODE_DEPTH")
            .ok()
            .and_then(|v| match v.parse::<usize>() {
                Ok(val) if val > 0 => Some(val),
                _ => {
                    tracing::warn!("invalid BOLT_MAX_DECODE_DEPTH value, using default 128");
                    None
                }
            })
            .unwrap_or(128)
    })
}

/// Maximum pre-allocation for list/map capacity from untrusted wire data,
/// measured in **elements** (not bytes).
///
/// The actual element count is still honoured; this only caps the initial
/// `Vec::with_capacity` call to prevent a single malicious List32/Map32 header
/// from triggering a multi-GB allocation (CVE-class OOM DoS).
///
/// 4096 elements is a reasonable upper bound for typical Bolt messages (result
/// sets, parameter maps) while being small enough that the worst-case
/// allocation (4096 * size_of::<BoltValue>()) stays well under 1 MiB.
pub(super) const MAX_PREALLOC: usize = 4096;
