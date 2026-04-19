//! `valueMap()` handler.

use crate::error::GqlError;
use crate::types::Value;

use super::super::super::QueryExecutor;
use super::super::valuemap_wrap_property;

impl<'a> QueryExecutor<'a> {
    pub(in crate::executor::engine::eval) fn eval_func_gremlin_valuemap(
        &self,
        graph: &str,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        // valueMap(): extract properties and wrap each in a list.
        // Args: element, token_mode_int [, key1, key2, ...]
        // token_mode: 0=none, 1=all, 2=labels, 3=ids
        let mut args_iter = evaled_args.into_iter();
        let element = args_iter.next();
        let token_mode = match args_iter.next() {
            Some(Value::Int(n)) => n as u8,
            Some(Value::Bool(true)) => 1u8, // backwards compat
            _ => 0u8,
        };
        let include_id = token_mode == 1 || token_mode == 3;
        let include_label = token_mode == 1 || token_mode == 2;
        let keys: Vec<String> = args_iter
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        match element {
            Some(Value::Node(node_id)) => match self.cached_get_node(graph, node_id)? {
                Some(data) => {
                    let mut wrapped: indexmap::IndexMap<Box<str>, Value> =
                        indexmap::IndexMap::new();
                    if include_id {
                        wrapped.insert("t[id]".into(), Value::Int(node_id.0 as i64));
                    }
                    if include_label {
                        let label_str = data
                            .labels
                            .first()
                            .map(|l| l.to_string())
                            .unwrap_or_default();
                        wrapped.insert("t[label]".into(), Value::Str(label_str.into()));
                    }
                    for (k, v) in data.properties.into_iter() {
                        if keys.is_empty() || keys.iter().any(|fk| fk == &k) {
                            wrapped.insert(k.into_boxed_str(), valuemap_wrap_property(v));
                        }
                    }
                    Ok(Value::Map(wrapped))
                }
                None => Ok(Value::Map(indexmap::IndexMap::new())),
            },
            Some(Value::Edge(edge_id)) => match self.cached_get_edge(graph, edge_id)? {
                Some(data) => {
                    let mut wrapped: indexmap::IndexMap<Box<str>, Value> =
                        indexmap::IndexMap::new();
                    if include_id {
                        wrapped.insert("t[id]".into(), Value::Int(edge_id.0 as i64));
                    }
                    if include_label {
                        wrapped
                            .insert("t[label]".into(), Value::Str(data.rel_type.as_str().into()));
                    }
                    for (k, v) in data.properties.into_iter() {
                        if keys.is_empty() || keys.iter().any(|fk| fk == &k) {
                            wrapped.insert(k.into_boxed_str(), valuemap_wrap_property(v));
                        }
                    }
                    Ok(Value::Map(wrapped))
                }
                None => Ok(Value::Map(indexmap::IndexMap::new())),
            },
            Some(Value::Map(m)) => {
                let wrapped: indexmap::IndexMap<Box<str>, Value> = m
                    .into_iter()
                    .filter(|(k, _)| keys.is_empty() || keys.iter().any(|fk| fk.as_str() == &**k))
                    .map(|(k, v)| (k, valuemap_wrap_property(v)))
                    .collect();
                Ok(Value::Map(wrapped))
            }
            Some(other) => Ok(other),
            None => Ok(Value::Null),
        }
    }
}
