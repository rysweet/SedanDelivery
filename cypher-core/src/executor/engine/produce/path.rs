//! Path assembly operator: exec_assemble_path.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::storage::{EdgeId, NodeId};
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_assemble_path(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::AssemblePath {
            path_alias,
            node_aliases,
            edge_aliases,
            child,
        } = op
        else {
            debug_assert!(false, "expected AssemblePath operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected AssemblePath operator".into(),
            }));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());
        for mut row in input {
            cx.budget.check_or_err()?;

            let mut nodes: Vec<NodeId> = Vec::new();
            let mut edges: Vec<EdgeId> = Vec::new();

            let mut fixed_edges_before_varlen: usize = 0;
            let mut varlen_path_found = false;
            let mut varlen_edge_aliases: Vec<String> = Vec::new();
            for ea in edge_aliases {
                match row.get(&ea.0) {
                    Some(Value::Edge(eid)) => {
                        edges.push(*eid);
                        if !varlen_path_found {
                            fixed_edges_before_varlen += 1;
                        }
                    }
                    Some(Value::Path {
                        nodes: pnodes,
                        edges: pedges,
                    }) => {
                        if !varlen_path_found {
                            for na in node_aliases.iter().take(fixed_edges_before_varlen) {
                                if let Some(Value::Node(nid)) = row.get(&na.0) {
                                    nodes.push(*nid);
                                }
                            }
                            nodes.extend_from_slice(pnodes);
                            varlen_path_found = true;
                        } else {
                            nodes.extend_from_slice(&pnodes[1..]);
                        }
                        edges.extend_from_slice(pedges);
                        varlen_edge_aliases.push(ea.0.clone());
                    }
                    Some(Value::List(items)) => {
                        for item in items {
                            if let Value::Edge(eid) = item {
                                edges.push(*eid);
                            }
                        }
                    }
                    _ => {
                        tracing::debug!("unexpected value type in path edge list, skipping");
                    }
                }
            }

            if !varlen_path_found {
                for na in node_aliases {
                    if let Some(Value::Node(nid)) = row.get(&na.0) {
                        nodes.push(*nid);
                    }
                }
            }

            let path_val = Value::Path { nodes, edges };
            row.insert(path_alias.0.clone(), path_val);

            for ea_name in &varlen_edge_aliases {
                if let Some(Value::Path { edges: pedges, .. }) = row.get(ea_name).cloned() {
                    let edge_list: Vec<Value> = pedges.iter().map(|e| Value::Edge(*e)).collect();
                    row.insert(ea_name.clone(), Value::List(edge_list));
                }
            }

            out.push(row);
        }
        Ok(out)
    }
}
