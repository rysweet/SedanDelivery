//! Persistent graph store — wraps [`InMemoryGraphStore`] with WAL-based disk persistence.
//!
//! All reads delegate to the in-memory store (zero disk I/O). All writes first apply to the
//! in-memory store then append a JSON-line WAL entry. A background thread periodically
//! snapshots the full state to disk and truncates the WAL.
//!
//! # Recovery
//!
//! On [`PersistentGraphStore::open`], the store:
//! 1. Loads the snapshot file (if it exists) by replaying its entries
//! 2. Replays any WAL entries on top
//! 3. Opens the WAL for append
//!
//! # Module layout
//!
//! - [`wal`] — WAL entry types and JSON-safe value conversion
//! - [`snapshot`] — snapshot writer and background snapshot loop
//! - [`replay`] — WAL/snapshot replay logic
//! - [`store_impl`] — `impl GraphStore for PersistentGraphStore`

mod replay;
mod snapshot;
mod store_impl;
mod wal;

#[cfg(test)]
mod tests;

use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::mem::InMemoryGraphStore;
use crate::error::StorageError;

use self::wal::WalEntry;

// ── Configuration ────────────────────────────────────────────────────────────

/// Controls how the WAL is synced to disk after writes.
#[derive(Debug, Clone)]
pub enum SyncMode {
    /// `fsync` after every WAL write. Safest, slowest.
    Immediate,
    /// Flush the WAL buffer periodically (the OS decides when to sync).
    Periodic(Duration),
    /// No explicit sync — rely on OS page cache. Fastest, least durable.
    None,
}

/// Configuration for [`PersistentGraphStore`].
#[derive(Debug, Clone)]
pub struct PersistentConfig {
    /// Directory where snapshot and WAL files are stored.
    pub data_dir: PathBuf,
    /// How often the background thread writes a snapshot and truncates the WAL.
    pub snapshot_interval: Duration,
    /// WAL sync behavior.
    pub sync_mode: SyncMode,
}

// ── PersistentGraphStore ─────────────────────────────────────────────────────

/// An [`InMemoryGraphStore`] with background disk persistence via WAL and snapshots.
pub struct PersistentGraphStore {
    pub(super) inner: Arc<InMemoryGraphStore>,
    pub(super) wal: Mutex<BufWriter<File>>,
    pub(super) config: PersistentConfig,
    pub(super) shutdown: Arc<AtomicBool>,
    /// Handle to the background snapshot thread; joined on drop.
    pub(super) _snapshot_thread: Option<thread::JoinHandle<()>>,
}

impl std::fmt::Debug for PersistentGraphStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PersistentGraphStore")
            .field("data_dir", &self.config.data_dir)
            .field("inner", &self.inner)
            .finish()
    }
}

impl PersistentGraphStore {
    /// Open or create a persistent store at the configured data directory.
    ///
    /// Recovery sequence:
    /// 1. Load snapshot if present
    /// 2. Replay WAL entries on top
    /// 3. Open WAL for append
    /// 4. Start background snapshot thread
    pub fn open(config: PersistentConfig) -> Result<Self, StorageError> {
        fs::create_dir_all(&config.data_dir).map_err(|e| StorageError::Backend {
            msg: format!("failed to create data directory: {e}"),
        })?;

        let inner = Arc::new(InMemoryGraphStore::new());

        // 1. Load snapshot
        let snapshot_path = config.data_dir.join("snapshot.json");
        if snapshot_path.exists() {
            replay::replay_file(&inner, &snapshot_path)?;
        }

        // 2. Replay WAL (tolerant of a torn trailing record from crash mid-fsync)
        let wal_path = config.data_dir.join("wal.jsonl");
        if wal_path.exists() {
            replay::replay_wal_file(&inner, &wal_path)?;
        }

        // 3. Open WAL for append
        let wal_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&wal_path)
            .map_err(|e| StorageError::Backend {
                msg: format!("failed to open WAL: {e}"),
            })?;
        let wal = Mutex::new(BufWriter::new(wal_file));

        // 4. Start background snapshot thread.
        // We share `inner` with the snapshot thread via Arc::clone — no
        // unsafe, no raw pointers. The thread keeps its own strong reference
        // for as long as it runs, and Drop joins it before the parent's Arc
        // is released.
        let shutdown = Arc::new(AtomicBool::new(false));
        let snapshot_thread = {
            let shutdown = Arc::clone(&shutdown);
            let inner = Arc::clone(&inner);
            let data_dir = config.data_dir.clone();
            let interval = config.snapshot_interval;
            thread::spawn(move || {
                snapshot::snapshot_loop(&shutdown, &data_dir, &inner, interval);
            })
        };

        Ok(Self {
            inner,
            wal,
            config,
            shutdown,
            _snapshot_thread: Some(snapshot_thread),
        })
    }

    /// Append a WAL entry and optionally sync.
    fn append_wal(&self, entry: &WalEntry) -> Result<(), StorageError> {
        let line = serde_json::to_string(entry).map_err(|e| StorageError::Serialization {
            msg: format!("WAL serialize failed: {e}"),
        })?;

        let mut wal = self.wal.lock().map_err(|e| StorageError::Backend {
            msg: format!("WAL lock poisoned: {e}"),
        })?;
        writeln!(wal, "{}", line).map_err(|e| StorageError::Backend {
            msg: format!("WAL write failed: {e}"),
        })?;

        match &self.config.sync_mode {
            SyncMode::Immediate => {
                wal.flush().map_err(|e| StorageError::Backend {
                    msg: format!("WAL flush failed: {e}"),
                })?;
                wal.get_ref()
                    .sync_all()
                    .map_err(|e| StorageError::Backend {
                        msg: format!("WAL sync failed: {e}"),
                    })?;
            }
            SyncMode::Periodic(_) | SyncMode::None => {
                // Periodic flush is handled by the background thread; None skips entirely.
            }
        }

        Ok(())
    }

    /// Force a snapshot now (used in tests).
    #[cfg(test)]
    fn force_snapshot(&self) -> Result<(), StorageError> {
        snapshot::write_snapshot(&self.config.data_dir, &self.inner)?;
        snapshot::truncate_wal(&self.config.data_dir, &self.wal)?;
        Ok(())
    }
}

impl Drop for PersistentGraphStore {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(handle) = self._snapshot_thread.take() {
            let _ = handle.join();
        }
        // Final flush of the WAL buffer.
        if let Ok(mut wal) = self.wal.lock() {
            let _ = wal.flush();
        }
    }
}
