//! Tagged value decoder for the SedanDelivery wire format.

/// Recursively decode tagged strings from the SedanDelivery wire format.
///
/// Tagged string prefixes:
/// - `__str:` -> plain string (un-escapes `__`-prefixed strings)
/// - `__date:` -> string value of the date
/// - `__localtime:` -> string value
/// - `__localdatetime:` -> string value
/// - `__datetime:` -> string value
/// - `__duration:` -> string value
/// - `__point:` -> JSON object with x, y, crs
/// - `__node:` -> integer node ID
/// - `__edge:` -> integer edge ID
///
/// Path objects (`__path_nodes` + `__path_edges`) are decoded into a JSON object
/// with "nodes" and "edges" arrays.
pub fn decode_value(v: &serde_json::Value) -> serde_json::Value {
    match v {
        serde_json::Value::String(s) => decode_tagged_string(s),
        serde_json::Value::Array(arr) => {
            let decoded: Vec<serde_json::Value> = arr.iter().map(decode_value).collect();
            serde_json::Value::Array(decoded)
        }
        serde_json::Value::Object(map) => {
            // Check for path encoding first.
            if map.contains_key("__path_nodes") && map.contains_key("__path_edges") {
                let nodes = map
                    .get("__path_nodes")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|n| n.as_u64())
                            .map(serde_json::Value::from)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let edges = map
                    .get("__path_edges")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|e| e.as_u64())
                            .map(serde_json::Value::from)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let mut result = serde_json::Map::new();
                result.insert("nodes".to_string(), serde_json::Value::Array(nodes));
                result.insert("edges".to_string(), serde_json::Value::Array(edges));
                return serde_json::Value::Object(result);
            }
            let decoded: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, val)| (k.clone(), decode_value(val)))
                .collect();
            serde_json::Value::Object(decoded)
        }
        // Null, Bool, Number -- return as-is.
        other => other.clone(),
    }
}

fn decode_tagged_string(s: &str) -> serde_json::Value {
    if let Some(rest) = s.strip_prefix("__str:") {
        return serde_json::Value::String(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("__date:") {
        return serde_json::Value::String(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("__localtime:") {
        return serde_json::Value::String(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("__localdatetime:") {
        return serde_json::Value::String(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("__datetime:") {
        // Strip optional IANA tz suffix like [Europe/Berlin]
        let clean = if let Some(bracket) = rest.find('[') {
            &rest[..bracket]
        } else {
            rest
        };
        return serde_json::Value::String(clean.to_string());
    }
    if let Some(rest) = s.strip_prefix("__duration:") {
        return serde_json::Value::String(rest.to_string());
    }
    if let Some(rest) = s.strip_prefix("__point:") {
        let parts: Vec<&str> = rest.splitn(3, ',').collect();
        if parts.len() == 3 {
            if let (Ok(x), Ok(y)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) {
                let mut map = serde_json::Map::new();
                map.insert("x".to_string(), serde_json::Value::from(x));
                map.insert("y".to_string(), serde_json::Value::from(y));
                map.insert(
                    "crs".to_string(),
                    serde_json::Value::String(parts[2].to_string()),
                );
                return serde_json::Value::Object(map);
            }
        }
        // Malformed point, return as-is
        return serde_json::Value::String(s.to_string());
    }
    if let Some(rest) = s.strip_prefix("__node:") {
        if let Ok(id) = rest.parse::<u64>() {
            return serde_json::Value::from(id);
        }
    }
    if let Some(rest) = s.strip_prefix("__edge:") {
        if let Ok(id) = rest.parse::<u64>() {
            return serde_json::Value::from(id);
        }
    }
    // Not a tagged string -- return as-is.
    serde_json::Value::String(s.to_string())
}
