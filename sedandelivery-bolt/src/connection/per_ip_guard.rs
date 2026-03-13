//! Per-IP connection count guard.
//!
//! Tracks the number of active connections from each remote IP address.
//! When a connection arrives, [`PerIpGuard::try_new`] either succeeds (creating
//! an RAII guard that decrements the count on drop) or returns `None` when the
//! per-IP limit is already reached.

use std::net::IpAddr;
use std::sync::Arc;

use dashmap::DashMap;

/// RAII guard that decrements the per-IP connection count on drop (including on panic).
pub struct PerIpGuard {
    map: Arc<DashMap<IpAddr, usize>>,
    addr: IpAddr,
}

impl PerIpGuard {
    /// Attempt to acquire a slot for `addr`.
    ///
    /// Returns `Some(guard)` when the current count is below `limit`,
    /// or `None` when the limit is already reached.
    pub fn try_new(
        map: Arc<DashMap<IpAddr, usize>>,
        addr: IpAddr,
        limit: usize,
    ) -> Option<Self> {
        {
            let mut entry = map.entry(addr).or_insert(0);
            if *entry >= limit {
                return None;
            }
            *entry += 1;
        } // entry (and its borrow of map) is dropped here
        Some(PerIpGuard { map, addr })
    }
}

impl Drop for PerIpGuard {
    fn drop(&mut self) {
        let mut entry = self.map.entry(self.addr).or_insert(0);
        *entry = entry.saturating_sub(1);
        // Remove the entry when count reaches zero to keep the map clean.
        if *entry == 0 {
            drop(entry);
            self.map.remove_if(&self.addr, |_, v| *v == 0);
        }
    }
}
