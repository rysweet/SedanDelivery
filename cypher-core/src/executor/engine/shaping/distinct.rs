//! `Distinct` operator and its property-aware deduplication helpers.

use crate::error::GqlError;
use crate::planner::Operator;
use crate::types::Value;

use super::super::helpers::{row_key, RowDedupSet};
use super::super::{ExecContext, QueryExecutor, Row};

/// Compute a dedup key for a value, treating Gremlin property Maps specially:
/// Edge property Maps (containing `~key`/`~value`/`~element` where element is an Edge)
/// are compared by `~key` + `~value` only, ignoring `~element` (the parent edge).
/// Vertex property Maps (where element is a Node) are compared by `~key` + `~element`,
/// ignoring `~id` (which is a per-access counter) and `~value` (two visits to the same
/// property on the same vertex should dedup regardless of value representation).
pub(super) fn property_aware_dedup_key(v: &Value) -> String {
    if let Value::Map(m) = v {
        if m.contains_key("~key") && m.contains_key("~value") {
            let key = m.get("~key").map(|v| v.canonical_key()).unwrap_or_default();
            // Edge properties: compare by key+value only
            if let Some(Value::Edge(_)) = m.get("~element") {
                let val = m
                    .get("~value")
                    .map(|v| v.canonical_key())
                    .unwrap_or_default();
                return format!("EdgeProp({key}, {val})");
            }
            // Meta-properties of vertex properties expose the parent property
            // as ~element. Dedup should collapse identical meta key/value pairs
            // across different parent properties.
            if matches!(m.get("~element"), Some(Value::Map(parent)) if parent.contains_key("~key") && parent.contains_key("~value"))
            {
                let val = m
                    .get("~value")
                    .map(|v| v.canonical_key())
                    .unwrap_or_default();
                return format!("MetaProp({key}, {val})");
            }
            // Vertex properties: compare by key+element (same property on same vertex)
            if let Some(elem) = m.get("~element") {
                let elem_key = elem.canonical_key();
                return format!("VtxProp({key}, {elem_key})");
            }
        }
    }
    v.canonical_key()
}

impl<'a> QueryExecutor<'a> {
    pub(crate) fn exec_distinct(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::Distinct {
            child,
            by_key,
            productive,
        } = op
        else {
            debug_assert!(false, "expected Distinct operator");
            return Err(GqlError::Execution(
                crate::error::ExecutionError::UnsupportedFeature {
                    feature: "internal: expected Distinct operator".into(),
                },
            ));
        };
        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        if let Some(key) = by_key {
            use std::collections::HashSet;
            // Use shared repeat-dedup state when inside a repeat loop, so that
            // values seen in previous iterations are still filtered out.
            let has_shared = self.repeat_dedup_state.borrow().is_some();
            let mut local_seen: HashSet<String> = HashSet::new();

            // Multi-key dedup (keys separated by \x1f)
            if key.contains('\x1f') {
                let labels: Vec<&str> = key.split('\x1f').collect();
                let any_has_prop = labels.iter().any(|l| l.starts_with("__dedup_prop__"));
                for row in input {
                    cx.budget.check_or_err()?;
                    let mut drop_traverser = false;
                    let composite: String = labels
                        .iter()
                        .map(|l| {
                            if let Some(rest) = l.strip_prefix("__dedup_prop__") {
                                let parts: Vec<&str> = rest.splitn(2, "__").collect();
                                let prop_name = parts.first().copied().unwrap_or("");
                                let alias = parts.get(1).copied().unwrap_or("");
                                if let Some(val) = row.get(alias) {
                                    let extracted =
                                        self.extract_property_from_value(cx.graph, val, prop_name);
                                    let is_element = matches!(val, Value::Node(_) | Value::Edge(_));
                                    if extracted.is_none() && is_element && !*productive {
                                        drop_traverser = true;
                                    }
                                    extracted.map(|v| v.canonical_key()).unwrap_or_default()
                                } else {
                                    String::new()
                                }
                            } else {
                                row.get(*l).map(|v| v.canonical_key()).unwrap_or_default()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\x1f");
                    if drop_traverser && any_has_prop {
                        continue;
                    }
                    let is_new = if has_shared {
                        self.repeat_dedup_state
                            .borrow_mut()
                            .as_mut()
                            .unwrap()
                            .insert(composite)
                    } else {
                        local_seen.insert(composite)
                    };
                    if is_new {
                        out.push(row);
                    }
                }
            } else if let Some(rest) = key.strip_prefix("__dedup_prop__") {
                // Property-based dedup: extract property from the element.
                let parts: Vec<&str> = rest.splitn(2, "__").collect();
                let prop_name = parts.first().copied().unwrap_or("");
                let alias = parts.get(1).copied().unwrap_or("");
                for row in input {
                    cx.budget.check_or_err()?;
                    if let Some(val) = row.get(alias) {
                        let extracted = self.extract_property_from_value(cx.graph, val, prop_name);
                        let is_element = matches!(val, Value::Node(_) | Value::Edge(_));
                        if extracted.is_none() && is_element && !*productive {
                            // Property missing on node/edge → drop traverser
                            // (unless ProductiveByStrategy is active)
                            continue;
                        }
                        let dedup_val = extracted.map(|v| v.canonical_key()).unwrap_or_default();
                        let is_new = if has_shared {
                            self.repeat_dedup_state
                                .borrow_mut()
                                .as_mut()
                                .unwrap()
                                .insert(dedup_val)
                        } else {
                            local_seen.insert(dedup_val)
                        };
                        if is_new {
                            out.push(row);
                        }
                    }
                }
            } else {
                // Simple column-based dedup
                for row in input {
                    cx.budget.check_or_err()?;
                    let dedup_val = row
                        .get(key.as_str())
                        .map(property_aware_dedup_key)
                        .unwrap_or_default();
                    let is_new = if has_shared {
                        self.repeat_dedup_state
                            .borrow_mut()
                            .as_mut()
                            .unwrap()
                            .insert(dedup_val)
                    } else {
                        local_seen.insert(dedup_val)
                    };
                    if is_new {
                        out.push(row);
                    }
                }
            }
        } else {
            // Row-level dedup (no by_key)
            let has_shared = self.repeat_dedup_state.borrow().is_some();
            if has_shared {
                // Use shared state with row canonical keys
                for row in input {
                    cx.budget.check_or_err()?;
                    let rk = row_key(&row);
                    let is_new = self
                        .repeat_dedup_state
                        .borrow_mut()
                        .as_mut()
                        .unwrap()
                        .insert(rk);
                    if is_new {
                        out.push(row);
                    }
                }
            } else {
                let mut seen = RowDedupSet::new();
                for row in input {
                    cx.budget.check_or_err()?;
                    if seen.insert(&row) {
                        out.push(row);
                    }
                }
            }
        }
        Ok(out)
    }

    /// Extract a property from a Value (node, edge, or map).
    fn extract_property_from_value(&self, graph: &str, val: &Value, prop: &str) -> Option<Value> {
        match val {
            Value::Node(nid) => {
                // Handle special Gremlin tokens
                if prop == "~label" {
                    return self.cached_get_node(graph, *nid).ok().flatten().map(|n| {
                        let label = n.labels.first().map(|l| l.as_str()).unwrap_or("");
                        Value::Str(Box::from(label))
                    });
                }
                if prop == "~id" {
                    return Some(Value::Int(nid.0 as i64));
                }
                self.cached_get_node(graph, *nid)
                    .ok()
                    .flatten()
                    .and_then(|n| n.properties.get(prop).cloned())
            }
            Value::Edge(eid) => {
                if prop == "~label" {
                    return self
                        .cached_get_edge(graph, *eid)
                        .ok()
                        .flatten()
                        .map(|e| Value::Str(Box::from(e.rel_type.as_str())));
                }
                if prop == "~id" {
                    return Some(Value::Int(eid.0 as i64));
                }
                self.cached_get_edge(graph, *eid)
                    .ok()
                    .flatten()
                    .and_then(|e| e.properties.get(prop).cloned())
            }
            Value::Map(m) => m.get(prop).cloned(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::property_aware_dedup_key;
    use crate::storage::{EdgeId, NodeId};
    use crate::types::Value;
    use indexmap::IndexMap;

    fn vprop_map(key: &str, value: Value, element: Value) -> Value {
        let mut m = IndexMap::new();
        m.insert(Box::from("~key"), Value::Str(key.into()));
        m.insert(Box::from("~value"), value);
        m.insert(Box::from("~element"), element);
        Value::Map(m)
    }

    #[test]
    fn falls_back_to_canonical_for_scalars_and_plain_maps() {
        // Scalars use canonical_key directly.
        assert_eq!(
            property_aware_dedup_key(&Value::Int(7)),
            Value::Int(7).canonical_key()
        );
        assert_eq!(
            property_aware_dedup_key(&Value::Str("hi".into())),
            Value::Str("hi".into()).canonical_key(),
        );
        // Plain map without ~key/~value → canonical key.
        let mut m = IndexMap::new();
        m.insert(Box::from("name"), Value::Str("Alice".into()));
        let v = Value::Map(m);
        assert_eq!(property_aware_dedup_key(&v), v.canonical_key());
    }

    #[test]
    fn edge_property_dedups_by_key_and_value_only() {
        let e1 = vprop_map("weight", Value::Int(5), Value::Edge(EdgeId(1)));
        let e2 = vprop_map("weight", Value::Int(5), Value::Edge(EdgeId(99))); // different edge
                                                                              // Edge property maps with same key+value collapse, even across edges.
        assert_eq!(property_aware_dedup_key(&e1), property_aware_dedup_key(&e2));
        // Different value → different key.
        let e3 = vprop_map("weight", Value::Int(6), Value::Edge(EdgeId(1)));
        assert_ne!(property_aware_dedup_key(&e1), property_aware_dedup_key(&e3));
        // Different prop name → different key.
        let e4 = vprop_map("color", Value::Int(5), Value::Edge(EdgeId(1)));
        assert_ne!(property_aware_dedup_key(&e1), property_aware_dedup_key(&e4));
    }

    #[test]
    fn vertex_property_dedups_by_key_and_element() {
        // Same prop on same vertex but different ~value (should still dedup).
        let v1 = vprop_map("name", Value::Str("A".into()), Value::Node(NodeId(1)));
        let v2 = vprop_map("name", Value::Str("Z".into()), Value::Node(NodeId(1)));
        assert_eq!(property_aware_dedup_key(&v1), property_aware_dedup_key(&v2));
        // Different vertex → different key.
        let v3 = vprop_map("name", Value::Str("A".into()), Value::Node(NodeId(2)));
        assert_ne!(property_aware_dedup_key(&v1), property_aware_dedup_key(&v3));
        // Different prop → different key.
        let v4 = vprop_map("age", Value::Str("A".into()), Value::Node(NodeId(1)));
        assert_ne!(property_aware_dedup_key(&v1), property_aware_dedup_key(&v4));
    }

    #[test]
    fn meta_property_dedups_when_parent_is_a_property_map() {
        // Parent is itself a vertex-property map (~key/~value present).
        let parent = vprop_map("name", Value::Str("A".into()), Value::Node(NodeId(1)));
        let parent2 = vprop_map("name", Value::Str("A".into()), Value::Node(NodeId(2)));
        let meta1 = vprop_map("lang", Value::Str("en".into()), parent);
        let meta2 = vprop_map("lang", Value::Str("en".into()), parent2);
        // Meta-property dedup ignores the parent property's element identity.
        assert_eq!(
            property_aware_dedup_key(&meta1),
            property_aware_dedup_key(&meta2),
        );
    }

    #[test]
    fn map_with_only_key_falls_through_to_canonical() {
        // No ~value present → falls through to canonical key.
        let mut m = IndexMap::new();
        m.insert(Box::from("~key"), Value::Str("foo".into()));
        let v = Value::Map(m);
        assert_eq!(property_aware_dedup_key(&v), v.canonical_key());
    }
}
