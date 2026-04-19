//! Gremlin "value-bag" function handlers split out of `eval`:
//! `valueMap`, `elementMap`, `values()` (with and without keys),
//! `properties()` (entries form).

use crate::error::GqlError;
use crate::types::Value;

use super::super::QueryExecutor;
use super::valuemap_wrap_property;

impl<'a> QueryExecutor<'a> {
    pub(super) fn eval_func_gremlin_valuemap(
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

    pub(super) fn eval_func_gremlin_elementmap(
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

    pub(super) fn eval_func_gremlin_values_all(
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

    pub(super) fn eval_func_gremlin_values_keys(
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

    pub(super) fn eval_func_gremlin_properties_entries(
        &self,
        graph: &str,
        evaled_args: Vec<Value>,
    ) -> Result<Value, GqlError> {
        // properties(): return list of Maps with ~key, ~value, ~element.
        // First arg is the element; remaining args are optional key filters.
        let mut args_iter = evaled_args.into_iter();
        let element = args_iter.next().unwrap_or(Value::Null);
        let key_filters: Vec<String> = args_iter
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
                                                return Ok(Value::List(
                                                    entry
                                                        .iter()
                                                        .filter(|(k, _)| k.as_ref() != "value")
                                                        .map(|(k, v)| {
                                                            let pid = self.intern_property_id(
                                                                format!(
                                                                    "meta|{element:?}|{}|{target_val:?}|{}",
                                                                    prop_key.as_ref(),
                                                                    k.as_ref()
                                                                ),
                                                            );
                                                            let mut mp =
                                                                indexmap::IndexMap::new();
                                                            mp.insert(
                                                                Box::from("~id"),
                                                                Value::Int(pid),
                                                            );
                                                            mp.insert(
                                                                Box::from("~key"),
                                                                Value::Str(k.clone()),
                                                            );
                                                            mp.insert(
                                                                Box::from("~value"),
                                                                v.clone(),
                                                            );
                                                            mp.insert(
                                                                Box::from("~element"),
                                                                element.clone(),
                                                            );
                                                            Value::Map(mp)
                                                        })
                                                        .collect(),
                                                ));
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
        let entries: Vec<Value> = props
            .into_iter()
            .filter(|(k, _)| key_filters.is_empty() || key_filters.iter().any(|f| f == k))
            .flat_map(|(k, v)| {
                if let Value::List(ref items) = v {
                    if !items.is_empty()
                        && items
                            .iter()
                            .all(|item| matches!(item, Value::Map(m) if m.contains_key("value")))
                    {
                        let result: Vec<Value> = items
                            .iter()
                            .enumerate()
                            .map(|(idx, item)| {
                                let main_val = match item {
                                    Value::Map(m) => m.get("value").cloned().unwrap_or(Value::Null),
                                    other => other.clone(),
                                };
                                let pid = self.intern_property_id(format!(
                                    "value|{element:?}|{}|{idx}|{main_val:?}",
                                    k.as_str()
                                ));
                                let mut m = indexmap::IndexMap::new();
                                m.insert(Box::from("~id"), Value::Int(pid));
                                m.insert(Box::from("~key"), Value::Str(k.clone().into_boxed_str()));
                                m.insert(Box::from("~value"), main_val);
                                m.insert(Box::from("~element"), element.clone());
                                Value::Map(m)
                            })
                            .collect();
                        return result;
                    }
                }
                let pid =
                    self.intern_property_id(format!("scalar|{element:?}|{}|{v:?}", k.as_str()));
                let mut m = indexmap::IndexMap::new();
                m.insert(Box::from("~id"), Value::Int(pid));
                m.insert(Box::from("~key"), Value::Str(k.into_boxed_str()));
                m.insert(Box::from("~value"), v);
                m.insert(Box::from("~element"), element.clone());
                vec![Value::Map(m)]
            })
            .collect();
        Ok(Value::List(entries))
    }
}
