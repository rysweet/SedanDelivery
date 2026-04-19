// Helpers for emitting / discarding ChangeLogStore events after a command
// handler completes.

use std::sync::atomic::Ordering;

use redis_module::Context;

use cypher_core::changelog::changelog_stream_for_graph;

use crate::state::{CHANGELOG_ENABLED, STORE};

/// After a successful write command, drain the ChangeLogStore's pending events
/// and append each one to the __sd2:changelog Valkey stream via ctx.call.
///
/// This is a no-op when CHANGELOG_ENABLED is false — zero overhead in the
/// common case where Fabric sync is not configured.
///
/// XADD failures are logged as warnings but do NOT fail the command — the
/// Redis mutation already committed and must not be rolled back.
pub(crate) fn emit_changelog(ctx: &Context) {
    if !CHANGELOG_ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let store = match STORE.get() {
        Some(s) => s,
        None => return,
    };
    for event in store.drain_events() {
        let stream_key = changelog_stream_for_graph(event.graph_name());
        match serde_json::to_string(&event) {
            Ok(json) => {
                if let Err(e) = ctx.call("XADD", &[&stream_key, "*", "e", json.as_str()]) {
                    // Log warning but continue — changelog lag is preferable to
                    // rejecting the client's write after it already succeeded.
                    tracing::warn!(stream = %stream_key, "changelog XADD failed: {e:?}");
                }
            }
            Err(e) => {
                tracing::warn!("changelog serialize failed: {e}");
            }
        }
    }
}

/// Discard any pending events accumulated during a failed command so they
/// don't bleed into the next command's changelog batch.
pub(crate) fn discard_changelog() {
    if let Some(store) = STORE.get() {
        let _ = store.drain_events();
    }
}
