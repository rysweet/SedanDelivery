// merge/props.rs -- JSON property parsing/merging helpers for SD2.MERGE_EDGES.

use valkey_module::{Context, ValkeyString};

use super::vs;

/// Parse a JSON string into a non-empty Map, or None.
pub(super) fn parse_props_obj(s: &str) -> Option<serde_json::Map<String, serde_json::Value>> {
    if s.is_empty() || s == "{}" {
        return None;
    }
    match serde_json::from_str::<serde_json::Value>(s) {
        Ok(serde_json::Value::Object(m)) if !m.is_empty() => Some(m),
        _ => None,
    }
}

/// Read the "props" field from an edge hash and parse as JSON object.
pub(super) fn read_edge_props(
    ctx: &Context,
    edge_key: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let key_vs: ValkeyString = vs(ctx, edge_key);
    let rkey = ctx.open_key(&key_vs);
    match rkey.hash_get("props") {
        Ok(Some(val)) => {
            let s = val.to_string_lossy();
            match serde_json::from_str::<serde_json::Value>(&s) {
                Ok(serde_json::Value::Object(m)) => Some(m),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Merge edge_props (MERGE match properties) and set_props into a single JSON object.
/// set_props override edge_props on key collision.
pub(super) fn merge_props_json(edge_props: &str, set_props: &str) -> String {
    let mut merged = serde_json::Map::new();

    if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(edge_props)
    {
        for (k, v) in m {
            merged.insert(k, v);
        }
    }

    if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(set_props) {
        for (k, v) in m {
            merged.insert(k, v);
        }
    }

    if merged.is_empty() {
        "{}".to_string()
    } else {
        serde_json::to_string(&serde_json::Value::Object(merged))
            .unwrap_or_else(|_| "{}".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_props_both_empty() {
        assert_eq!(merge_props_json("", ""), "{}");
        assert_eq!(merge_props_json("{}", "{}"), "{}");
    }

    #[test]
    fn merge_props_only_edge() {
        let result = merge_props_json(r#"{"a":1}"#, "");
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["a"], 1);
    }

    #[test]
    fn merge_props_only_set() {
        let result = merge_props_json("", r#"{"b":2}"#);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["b"], 2);
    }

    #[test]
    fn merge_props_set_overrides_edge() {
        let result = merge_props_json(r#"{"x":1,"y":2}"#, r#"{"x":99,"z":3}"#);
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["x"], 99);
        assert_eq!(parsed["y"], 2);
        assert_eq!(parsed["z"], 3);
    }

    #[test]
    fn merge_props_invalid_json_treated_as_empty() {
        assert_eq!(merge_props_json("not json", "also not json"), "{}");
    }

    #[test]
    fn parse_props_obj_empty() {
        assert!(parse_props_obj("").is_none());
        assert!(parse_props_obj("{}").is_none());
    }

    #[test]
    fn parse_props_obj_valid() {
        let m = parse_props_obj(r#"{"key":"val"}"#).unwrap();
        assert_eq!(m.get("key").unwrap(), "val");
    }

    #[test]
    fn parse_props_obj_invalid_json() {
        assert!(parse_props_obj("not json").is_none());
    }

    #[test]
    fn parse_props_obj_array_returns_none() {
        assert!(parse_props_obj("[1,2,3]").is_none());
    }
}
