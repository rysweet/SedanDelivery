//! Bridge between the Bolt RUN message and the cypher-core executor.
//!
//! This module is split into focused submodules:
//! - [`query`] — parse, plan, and execute Cypher queries
//! - [`results`] — convert `QueryResult` rows into Bolt records
//! - [`stats`] — convert mutation stats into a Bolt summary map
//! - [`read_only`] — lightweight read-only query detection (for routing)
//! - [`params`] — convert Bolt parameters/structures into cypher-core values

mod params;
mod query;
mod read_only;
mod results;
mod stats;

pub use params::bolt_map_to_value_map;
pub use query::{execute_query, execute_query_with_profiling, plan_query};
pub use read_only::is_read_only_query;
pub use results::result_to_records;
pub use stats::stats_to_bolt_map;
