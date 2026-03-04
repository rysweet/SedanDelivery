//! Event application — translates `ChangeEvent` into `FabricGraphStore` calls.

use gql_core::changelog::{json_map_to_props, ChangeEvent};
use gql_core::storage::{EdgeId, NodeId};
use gql_core::types::Value;
use gql_fabric_store::FabricGraphStore;

/// Translate a `ChangeEvent` into the corresponding `FabricGraphStore` call.
///
/// For `CreateNode` and `CreateEdge` the event carries the ID assigned by the
/// primary store.  We inject it as `_sd2_id` / `_sd2_eid` so `FabricGraphStore`
/// uses `MERGE` semantics and preserves the original ID.
pub(crate) fn apply_event(
    fabric: &FabricGraphStore,
    event: &ChangeEvent,
) -> Result<(), gql_core::error::StorageError> {
    match event {
        // -- Graph-level ----------------------------------------------------
        ChangeEvent::CreateGraph { name } => {
            fabric.create_graph(name)?;
        }
        ChangeEvent::DeleteGraph { name } => {
            fabric.delete_graph(name)?;
        }

        // -- Node writes ----------------------------------------------------
        ChangeEvent::CreateNode {
            graph,
            id,
            labels,
            properties,
        } => {
            let mut props = json_map_to_props(properties);
            let id_i64 =
                i64::try_from(*id).map_err(|_| gql_core::error::StorageError::Backend {
                    msg: format!("node ID {id} exceeds i64 range — cannot sync to Fabric"),
                })?;
            props.insert("_sd2_id".to_string(), Value::Int(id_i64));
            fabric.create_node(graph, labels.clone(), props)?;
        }
        ChangeEvent::SetNodeProperties {
            graph,
            id,
            properties,
        } => {
            fabric.set_node_properties(graph, NodeId(*id), json_map_to_props(properties))?;
        }
        ChangeEvent::ReplaceNodeProperties {
            graph,
            id,
            properties,
        } => {
            fabric.replace_node_properties(graph, NodeId(*id), json_map_to_props(properties))?;
        }
        ChangeEvent::RemoveNodeProperty { graph, id, key } => {
            fabric.remove_node_property(graph, NodeId(*id), key)?;
        }
        ChangeEvent::AddNodeLabels { graph, id, labels } => {
            fabric.add_node_labels(graph, NodeId(*id), labels)?;
        }
        ChangeEvent::RemoveNodeLabels { graph, id, labels } => {
            fabric.remove_node_labels(graph, NodeId(*id), labels)?;
        }
        ChangeEvent::DeleteNode { graph, id, detach } => {
            fabric.delete_node(graph, NodeId(*id), *detach)?;
        }

        // -- Edge writes ----------------------------------------------------
        ChangeEvent::CreateEdge {
            graph,
            id,
            src,
            dst,
            rel_type,
            properties,
        } => {
            let mut props = json_map_to_props(properties);
            let eid_i64 =
                i64::try_from(*id).map_err(|_| gql_core::error::StorageError::Backend {
                    msg: format!("edge ID {id} exceeds i64 range — cannot sync to Fabric"),
                })?;
            props.insert("_sd2_eid".to_string(), Value::Int(eid_i64));
            fabric.create_edge(graph, NodeId(*src), NodeId(*dst), rel_type.clone(), props)?;
        }
        ChangeEvent::SetEdgeProperties {
            graph,
            id,
            properties,
        } => {
            fabric.set_edge_properties(graph, EdgeId(*id), json_map_to_props(properties))?;
        }
        ChangeEvent::ReplaceEdgeProperties {
            graph,
            id,
            properties,
        } => {
            fabric.replace_edge_properties(graph, EdgeId(*id), json_map_to_props(properties))?;
        }
        ChangeEvent::RemoveEdgeProperty { graph, id, key } => {
            fabric.remove_edge_property(graph, EdgeId(*id), key)?;
        }
        ChangeEvent::DeleteEdge { graph, id } => {
            fabric.delete_edge(graph, EdgeId(*id))?;
        }

        // -- Index management -----------------------------------------------
        ChangeEvent::CreateIndex {
            graph,
            label,
            property,
        } => {
            fabric.create_index(graph, label, property)?;
        }
        ChangeEvent::DropIndex {
            graph,
            label,
            property,
        } => {
            fabric.drop_index(graph, label, property)?;
        }
    }
    Ok(())
}
