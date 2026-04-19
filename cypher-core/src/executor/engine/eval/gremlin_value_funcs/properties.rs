//! `properties()` handler — returns entries with `~key`, `~value`, `~element`.

use crate::error::GqlError;
use crate::types::Value;

use super::super::super::QueryExecutor;

impl<'a> QueryExecutor<'a> {
    pub(in crate::executor::engine::eval) fn eval_func_gremlin_properties_entries(
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
