//! EXPLAIN/PROFILE plan formatting and cartesian product detection.
//!
//! Extracted from `run.rs` to keep the RUN handler focused on query
//! dispatch while this module owns operator-tree introspection.
//!
//! Submodules:
//! - [`plan`]: EXPLAIN plan-tree formatting (estimated rows only).
//! - [`profile`]: PROFILE plan-tree formatting (with actual rows + dbHits).
//! - [`notifications`]: cartesian product detection and Bolt notifications.

mod notifications;
mod plan;
mod profile;

pub(crate) use notifications::detect_notifications;
pub(crate) use plan::operator_to_plan_map;
pub(crate) use profile::operator_to_profile_plan_map;
