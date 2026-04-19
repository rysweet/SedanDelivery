//! Query-level "now" timestamp + RAII guard.
//!
//! Initialized once per query so temporal no-arg functions (date(), time(), …)
//! return the same value throughout. Behavior preserved verbatim from the
//! original `filter` module.

use std::cell::RefCell;

use indexmap::IndexMap;

thread_local! {
    /// The current query's "now" timestamp. Initialized once per query execution so that
    /// temporal no-arg functions (date(), time(), etc.) return the same value throughout.
    static QUERY_NOW: RefCell<Option<jiff::Zoned>> = const { RefCell::new(None) };

    /// Per-thread cache of compiled regexes to avoid recompilation on repeated pattern use.
    /// Uses `IndexMap` to preserve insertion order so we can evict the oldest half
    /// instead of clearing the entire cache when the capacity limit is reached.
    pub(crate) static REGEX_CACHE: RefCell<IndexMap<String, regex::Regex>> = RefCell::new(IndexMap::new());
}

/// Initialize the query-level "now". Call once at the start of each query execution.
pub fn init_query_now() {
    QUERY_NOW.with(|c| *c.borrow_mut() = Some(jiff::Zoned::now()));
}

/// Clear the query-level "now". Call at the end of each query execution.
pub fn clear_query_now() {
    QUERY_NOW.with(|c| *c.borrow_mut() = None);
}

/// RAII guard that initializes the query-level "now" on construction and clears
/// it on drop, ensuring `clear_query_now()` is always called even on error paths.
pub struct QueryTimeGuard;

impl QueryTimeGuard {
    /// Initialize the query "now" and return a guard that will clear it on drop.
    pub fn new() -> Self {
        init_query_now();
        QueryTimeGuard
    }
}

impl Default for QueryTimeGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for QueryTimeGuard {
    fn drop(&mut self) {
        clear_query_now();
    }
}

/// Get the query-level "now", falling back to the system time if not initialized.
pub(crate) fn get_query_now() -> jiff::Zoned {
    QUERY_NOW.with(|c| c.borrow().clone().unwrap_or_else(jiff::Zoned::now))
}
