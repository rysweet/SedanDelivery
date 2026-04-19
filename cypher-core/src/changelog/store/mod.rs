//! [`ChangeLogStore`]: a [`GraphStore`] decorator that records every
//! successful write as a [`ChangeEvent`] in an in-process pending queue.

use std::sync::Mutex;

use super::event::ChangeEvent;

mod graph_store_impl;

/// Wraps any [`GraphStore`] and records every successful write as a
/// [`ChangeEvent`] in an in-process pending queue.
///
/// The caller is responsible for draining the queue (via [`drain_events`])
/// and forwarding the events to a durable sink (e.g. a Valkey stream).
///
/// [`GraphStore`]: crate::storage::GraphStore
/// [`drain_events`]: ChangeLogStore::drain_events
pub struct ChangeLogStore<S> {
    inner: S,
    pending: Mutex<Vec<ChangeEvent>>,
}

impl<S> ChangeLogStore<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            pending: Mutex::new(Vec::new()),
        }
    }

    /// Push an event onto the pending queue.
    ///
    /// A poisoned mutex is ignored rather than panicked — the module must not
    /// crash a Redis/Valkey process because of a changelog accounting error.
    fn push(&self, event: ChangeEvent) {
        match self.pending.lock() {
            Ok(mut v) => v.push(event),
            Err(_) => {
                tracing::error!("changelog: pending mutex poisoned — event dropped");
            }
        }
    }

    /// Drain all accumulated events.
    ///
    /// Call this after a successful command to retrieve events for forwarding
    /// to the changelog stream.  On failure / panic, call this to discard any
    /// partial events so they don't leak into the next command's batch.
    pub fn drain_events(&self) -> Vec<ChangeEvent> {
        match self.pending.lock() {
            Ok(mut v) => std::mem::take(&mut *v),
            Err(_) => {
                tracing::error!("changelog: pending mutex poisoned — returning empty event list");
                Vec::new()
            }
        }
    }
}
