//! `values()` and `values(k1, k2, ...)` handlers.

use crate::error::GqlError;
use crate::types::Value;

use super::super::super::QueryExecutor;

impl<'a> QueryExecutor<'a> {
    pub(in crate::executor::engine::eval) fn eval_func_gremlin_values_all(
        &self,
        graph: &str,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        // values() with no keys: return list of all property values
        match evaled_args.into_iter().next() {
            Some(Value::Node(node_id)) => match self.cached_get_node(graph, node_id)? {
                Some(data) => {
                    let vals: Vec<Value> = data.properties.into_values().collect();
                    Ok(Value::List(vals))
                }
                None => Ok(Value::List(vec![])),
            },
            Some(Value::Edge(edge_id)) => match self.cached_get_edge(graph, edge_id)? {
                Some(data) => {
                    let vals: Vec<Value> = data.properties.into_values().collect();
                    Ok(Value::List(vals))
                }
                None => Ok(Value::List(vec![])),
            },
            Some(Value::Map(m)) => {
                // Property Map from properties(): extract ~value
                if m.contains_key("~value") {
                    let val = m.get("~value").cloned().unwrap_or(Value::Null);
                    Ok(Value::List(vec![val]))
                } else {
                    let vals: Vec<Value> = m.into_values().collect();
                    Ok(Value::List(vals))
                }
            }
            Some(Value::Null) | None => Ok(Value::List(vec![])),
            // Scalar values (from properties().value() chain):
            // wrap in a single-element list so Unwind produces
            // one row.
            Some(other) => Ok(Value::List(vec![other])),
        }
    }

    pub(in crate::executor::engine::eval) fn eval_func_gremlin_values_keys(
        &self,
        graph: &str,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        // values("k1", "k2") — return list of values for specific keys
        let mut args_iter = evaled_args.into_iter();
        let element = args_iter.next().unwrap_or(Value::Null);
        let keys: Vec<String> = args_iter
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        let props: Vec<(String, Value)> = match &element {
            Value::Node(node_id) => match self.cached_get_node(graph, *node_id)? {
                Some(data) => data.properties.into_iter().collect(),
                None => Vec::new(),
            },
            Value::Edge(edge_id) => match self.cached_get_edge(graph, *edge_id)? {
                Some(data) => data.properties.into_iter().collect(),
                None => Vec::new(),
            },
            Value::Map(m) => {
                if m.contains_key("~key") && m.contains_key("~value") {
                    if let Some(Value::Node(nid)) = m.get("~element") {
                        if let Some(Value::Str(prop_key)) = m.get("~key") {
                            if let Some(data) = self.cached_get_node(graph, *nid)? {
                                if let Some(Value::List(items)) =
                                    data.properties.get(prop_key.as_ref())
                                {
                                    let target_val = m.get("~value");
                                    for item in items {
                                        if let Value::Map(entry) = item {
                                            if entry.get("value") == target_val {
                                                let vals: Vec<Value> = keys
                                                    .iter()
                                                    .filter_map(|key| {
                                                        entry.get(key.as_str()).cloned()
                                                    })
                                                    .collect();
                                                return Ok(Value::List(vals));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Vec::new()
                } else {
                    m.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
                }
            }
            _ => Vec::new(),
        };
        let vals: Vec<Value> = props
            .into_iter()
            .filter(|(k, _)| keys.iter().any(|fk| fk == k))
            .map(|(_, v)| v)
            .collect();
        Ok(Value::List(vals))
    }
}
