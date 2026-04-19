//! Buffered node/edge create requests collected before each batched flush.

use indexmap::IndexMap;

use crate::storage::NodeId;
use crate::types::Value;

pub(super) struct PendingCreateNode {
    pub(super) alias: Option<String>,
    pub(super) labels: Vec<String>,
    pub(super) non_null_prop_keys: Vec<String>,
    pub(super) props: IndexMap<String, Value>,
}

pub(super) struct PendingCreateEdge {
    pub(super) alias: Option<String>,
    pub(super) non_null_prop_keys: Vec<String>,
    pub(super) src: NodeId,
    pub(super) dst: NodeId,
    pub(super) rel_type: String,
    pub(super) props: IndexMap<String, Value>,
}
