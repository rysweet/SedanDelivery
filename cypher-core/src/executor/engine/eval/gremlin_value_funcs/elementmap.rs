//! `elementMap()` handler.

use crate::error::GqlError;
use crate::types::Value;

use super::super::super::QueryExecutor;

impl<'a> QueryExecutor<'a> {
    pub(in crate::executor::engine::eval) fn eval_func_gremlin_elementmap(
        &self,
        graph: &str,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        // elementMap(): like valueMap but unwrapped, always with t[id]/t[label].
        // For edges also includes D[OUT] and D[IN] direction maps.
        // Args: element [, key1, key2, ...]
        let mut args_iter = evaled_args.into_iter();
        let element = args_iter.next();
        let keys: Vec<String> = args_iter
            .filter_map(|v| match v {
                Value::Str(s) => Some(s.to_string()),
                Value::Null => None,
                _ => None,
            })
            .collect();
        match element {
            Some(Value::Node(node_id)) => match self.cached_get_node(graph, node_id)? {
                Some(data) => {
                    let mut m: indexmap::IndexMap<Box<str>, Value> = indexmap::IndexMap::new();
                    m.insert("t[id]".into(), Value::Int(node_id.0 as i64));
                    let label_str = data
                        .labels
                        .first()
                        .map(|l| l.to_string())
                        .unwrap_or_default();
                    m.insert("t[label]".into(), Value::Str(label_str.into()));
                    for (k, v) in data.properties.into_iter() {
                        if keys.is_empty() || keys.iter().any(|fk| fk == &k) {
                            m.insert(k.into_boxed_str(), v);
                        }
                    }
                    Ok(Value::Map(m))
                }
                None => Ok(Value::Map(indexmap::IndexMap::new())),
            },
            Some(Value::Edge(edge_id)) => match self.cached_get_edge(graph, edge_id)? {
                Some(data) => {
                    let mut m: indexmap::IndexMap<Box<str>, Value> = indexmap::IndexMap::new();
                    m.insert("t[id]".into(), Value::Int(edge_id.0 as i64));
                    m.insert("t[label]".into(), Value::Str(data.rel_type.as_str().into()));
                    for (k, v) in data.properties.into_iter() {
                        if keys.is_empty() || keys.iter().any(|fk| fk == &k) {
                            m.insert(k.into_boxed_str(), v);
                        }
                    }
                    // D[OUT] / D[IN] direction endpoint maps
                    if let Some(src_data) = self.cached_get_node(graph, data.src)? {
                        let mut out_m: indexmap::IndexMap<Box<str>, Value> =
                            indexmap::IndexMap::new();
                        out_m.insert("t[id]".into(), Value::Int(data.src.0 as i64));
                        let l = src_data
                            .labels
                            .first()
                            .map(|l| l.to_string())
                            .unwrap_or_default();
                        out_m.insert("t[label]".into(), Value::Str(l.into()));
                        m.insert("D[OUT]".into(), Value::Map(out_m));
                    }
                    if let Some(dst_data) = self.cached_get_node(graph, data.dst)? {
                        let mut in_m: indexmap::IndexMap<Box<str>, Value> =
                            indexmap::IndexMap::new();
                        in_m.insert("t[id]".into(), Value::Int(data.dst.0 as i64));
                        let l = dst_data
                            .labels
                            .first()
                            .map(|l| l.to_string())
                            .unwrap_or_default();
                        in_m.insert("t[label]".into(), Value::Str(l.into()));
                        m.insert("D[IN]".into(), Value::Map(in_m));
                    }
                    Ok(Value::Map(m))
                }
                None => Ok(Value::Map(indexmap::IndexMap::new())),
            },
            Some(Value::Map(m)) => Ok(Value::Map(m)),
            Some(other) => Ok(other),
            None => Ok(Value::Null),
        }
    }
}
