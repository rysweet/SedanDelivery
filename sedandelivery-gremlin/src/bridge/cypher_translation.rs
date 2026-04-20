//! Bytecode → Cypher translation used as a fallback path when the
//! Gremlin compiler does not yet support a step.

use cypher_core::error::{ExecutionError, GqlError};

/// Shorthand for creating a GqlError from a message string.
fn gremlin_err(msg: impl Into<String>) -> GqlError {
    GqlError::Execution(ExecutionError::UnsupportedFeature {
        feature: msg.into(),
    })
}

/// Escape a user-supplied identifier for safe embedding in Cypher.
fn escape_identifier(s: &str) -> String {
    format!("`{}`", s.replace('`', "``"))
}

/// Escape a user-supplied string literal for safe embedding in Cypher.
fn escape_string_literal(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    format!("'{escaped}'")
}

/// Translate Gremlin bytecode JSON into a Cypher query string.
///
/// Bytecode format (GraphSON v3):
/// ```json
/// {"@type": "g:Bytecode", "@value": {"step": [["V"], ["has","name","marko"], ["out","knows"], ["values","name"]]}}
/// ```
pub(crate) fn bytecode_to_cypher(json: &serde_json::Value) -> Result<String, GqlError> {
    let steps = extract_steps(json)?;
    if steps.is_empty() {
        return Err(gremlin_err("empty bytecode"));
    }

    let mut ctx = TranslationContext::new();

    for step in &steps {
        let name = step
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| gremlin_err("bytecode step missing name"))?;
        let args: Vec<&serde_json::Value> = step.iter().skip(1).collect();
        translate_step(&mut ctx, name, &args)?;
    }

    Ok(ctx.to_cypher())
}

/// Extract the step array from GraphSON bytecode.
fn extract_steps(json: &serde_json::Value) -> Result<Vec<Vec<serde_json::Value>>, GqlError> {
    // Handle both {"@type":"g:Bytecode","@value":{"step":[...]}} and {"step":[...]}
    let value = json.get("@value").unwrap_or(json);

    let steps = value
        .get("step")
        .and_then(|s| s.as_array())
        .ok_or_else(|| gremlin_err("bytecode missing 'step' array"))?;

    steps
        .iter()
        .map(|s| {
            s.as_array()
                .cloned()
                .ok_or_else(|| gremlin_err("bytecode step is not an array"))
        })
        .collect()
}

/// Accumulates Cypher fragments during bytecode translation.
struct TranslationContext {
    node_var: String,
    match_clause: String,
    where_clauses: Vec<String>,
    return_expr: String,
    traversals: Vec<String>,
    limit: Option<u64>,
    started_with_v: bool,
    started_with_e: bool,
}

impl TranslationContext {
    fn new() -> Self {
        Self {
            node_var: "n".to_string(),
            match_clause: String::new(),
            where_clauses: Vec::new(),
            return_expr: String::new(),
            traversals: Vec::new(),
            limit: None,
            started_with_v: false,
            started_with_e: false,
        }
    }

    fn to_cypher(&self) -> String {
        let mut parts = Vec::new();

        if !self.match_clause.is_empty() {
            parts.push(format!("MATCH {}", self.match_clause));
        }

        for t in &self.traversals {
            parts.push(format!("MATCH {t}"));
        }

        if !self.where_clauses.is_empty() {
            parts.push(format!("WHERE {}", self.where_clauses.join(" AND ")));
        }

        let ret = if self.return_expr.is_empty() {
            self.node_var.clone()
        } else {
            self.return_expr.clone()
        };
        parts.push(format!("RETURN {ret}"));

        if let Some(lim) = self.limit {
            parts.push(format!("LIMIT {lim}"));
        }

        parts.join(" ")
    }
}

fn translate_step(
    ctx: &mut TranslationContext,
    name: &str,
    args: &[&serde_json::Value],
) -> Result<(), GqlError> {
    match name {
        "V" => {
            ctx.started_with_v = true;
            if let Some(id) = args.first().and_then(|v| graphson_to_i64(v)) {
                ctx.match_clause = format!("(n) WHERE id(n) = {id}");
            } else {
                ctx.match_clause = "(n)".to_string();
            }
        }
        "E" => {
            ctx.started_with_e = true;
            ctx.match_clause = "()-[e]->()".to_string();
            ctx.node_var = "e".to_string();
        }
        "hasLabel" => {
            if let Some(label) = args.first().and_then(|v| v.as_str()) {
                let var = &ctx.node_var;
                let safe_label = escape_identifier(label);
                ctx.match_clause = if ctx.started_with_e {
                    format!("()-[{var}:{safe_label}]->()")
                } else {
                    format!("({var}:{safe_label})")
                };
            }
        }
        "has" => {
            match args.len() {
                // has(key, value) — property predicate
                2 => {
                    if let (Some(key), Some(val)) = (args[0].as_str(), graphson_to_literal(args[1]))
                    {
                        let safe_key = escape_identifier(key);
                        ctx.where_clauses
                            .push(format!("{}.{safe_key} = {val}", ctx.node_var));
                    }
                }
                // has(label, key, value) — label + property
                3 => {
                    if let Some(label) = args[0].as_str() {
                        let var = &ctx.node_var;
                        let safe_label = escape_identifier(label);
                        ctx.match_clause = format!("({var}:{safe_label})");
                    }
                    if let (Some(key), Some(val)) = (args[1].as_str(), graphson_to_literal(args[2]))
                    {
                        let safe_key = escape_identifier(key);
                        ctx.where_clauses
                            .push(format!("{}.{safe_key} = {val}", ctx.node_var));
                    }
                }
                _ => {}
            }
        }
        "out" => {
            let rel = args.first().and_then(|v| v.as_str()).unwrap_or("");
            let rel_part = if rel.is_empty() {
                String::new()
            } else {
                format!(":{}", escape_identifier(rel))
            };
            let prev = &ctx.node_var;
            let next = format!("{prev}_out");
            ctx.traversals
                .push(format!("({prev})-[{rel_part}]->({next})"));
            ctx.node_var = next;
        }
        "in" => {
            let rel = args.first().and_then(|v| v.as_str()).unwrap_or("");
            let rel_part = if rel.is_empty() {
                String::new()
            } else {
                format!(":{}", escape_identifier(rel))
            };
            let prev = &ctx.node_var;
            let next = format!("{prev}_in");
            ctx.traversals
                .push(format!("({prev})<-[{rel_part}]-({next})"));
            ctx.node_var = next;
        }
        "both" => {
            let rel = args.first().and_then(|v| v.as_str()).unwrap_or("");
            let rel_part = if rel.is_empty() {
                String::new()
            } else {
                format!(":{}", escape_identifier(rel))
            };
            let prev = &ctx.node_var;
            let next = format!("{prev}_both");
            ctx.traversals
                .push(format!("({prev})-[{rel_part}]-({next})"));
            ctx.node_var = next;
        }
        "outE" => {
            let rel = args.first().and_then(|v| v.as_str()).unwrap_or("");
            let rel_part = if rel.is_empty() {
                String::new()
            } else {
                format!(":{}", escape_identifier(rel))
            };
            let prev = &ctx.node_var;
            let edge_var = format!("{prev}_e");
            ctx.traversals
                .push(format!("({prev})-[{edge_var}{rel_part}]->()"));
            ctx.node_var = edge_var;
        }
        "inE" => {
            let rel = args.first().and_then(|v| v.as_str()).unwrap_or("");
            let rel_part = if rel.is_empty() {
                String::new()
            } else {
                format!(":{}", escape_identifier(rel))
            };
            let prev = &ctx.node_var;
            let edge_var = format!("{prev}_e");
            ctx.traversals
                .push(format!("({prev})<-[{edge_var}{rel_part}]-()"));
            ctx.node_var = edge_var;
        }
        "values" => {
            if let Some(key) = args.first().and_then(|v| v.as_str()) {
                let safe_key = escape_identifier(key);
                ctx.return_expr = format!("{}.{safe_key}", ctx.node_var);
            }
        }
        "count" => {
            let var = &ctx.node_var;
            ctx.return_expr = format!("count({var})");
        }
        "limit" => {
            if let Some(n) = args.first().and_then(|v| graphson_to_i64(v)) {
                ctx.limit = Some(n as u64);
            }
        }
        other => {
            tracing::warn!(step = other, "unsupported gremlin step, ignoring");
        }
    }

    Ok(())
}

/// Unwrap a GraphSON typed value to i64, handling both `{"@type":"g:Int64","@value":N}` and plain numbers.
fn graphson_to_i64(v: &serde_json::Value) -> Option<i64> {
    if let Some(n) = v.as_i64() {
        return Some(n);
    }
    v.get("@value").and_then(|inner| inner.as_i64())
}

/// Convert a GraphSON value to a Cypher literal string.
fn graphson_to_literal(v: &serde_json::Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(escape_string_literal(s));
    }
    if let Some(n) = graphson_to_i64(v) {
        return Some(n.to_string());
    }
    if let Some(f) = v.as_f64() {
        return Some(f.to_string());
    }
    if let Some(b) = v.as_bool() {
        return Some(b.to_string());
    }
    // Typed GraphSON: unwrap @value and recurse.
    if let Some(inner) = v.get("@value") {
        return graphson_to_literal(inner);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bc(steps: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "@type": "g:Bytecode",
            "@value": { "step": steps }
        })
    }

    #[test]
    fn v_count() {
        let json = bc(serde_json::json!([["V"], ["count"]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("MATCH (n)"));
        assert!(cypher.contains("count(n)"));
    }

    #[test]
    fn v_has_out_values() {
        let json = bc(serde_json::json!([
            ["V"],
            ["has", "name", "marko"],
            ["out", "knows"],
            ["values", "name"]
        ]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("MATCH (n)"));
        assert!(cypher.contains("n.`name` = 'marko'"));
        assert!(cypher.contains("`knows`"));
    }

    #[test]
    fn v_with_id() {
        let json = bc(serde_json::json!([["V", 1]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("id(n) = 1"));
    }

    #[test]
    fn v_limit() {
        let json = bc(serde_json::json!([["V"], ["limit", 10]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("LIMIT 10"));
    }

    #[test]
    fn empty_bytecode_errors() {
        let json = bc(serde_json::json!([]));
        assert!(bytecode_to_cypher(&json).is_err());
    }

    #[test]
    fn missing_step_array_errors() {
        let json = serde_json::json!({"@type": "g:Bytecode", "@value": {}});
        assert!(bytecode_to_cypher(&json).is_err());
    }

    #[test]
    fn malformed_step_not_array_errors() {
        let json = serde_json::json!({
            "@type": "g:Bytecode",
            "@value": { "step": ["not_an_array"] }
        });
        assert!(bytecode_to_cypher(&json).is_err());
    }

    #[test]
    fn step_missing_name_errors() {
        let json = bc(serde_json::json!([[]]));
        assert!(bytecode_to_cypher(&json).is_err());
    }

    #[test]
    fn e_traversal_starts_with_edge() {
        let json = bc(serde_json::json!([["E"]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("()-[e]->()"));
        assert!(cypher.contains("RETURN e"));
    }

    #[test]
    fn has_label_on_vertex() {
        let json = bc(serde_json::json!([["V"], ["hasLabel", "Person"]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("(n:`Person`)"));
    }

    #[test]
    fn has_label_on_edge() {
        let json = bc(serde_json::json!([["E"], ["hasLabel", "knows"]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("()-[e:`knows`]->()"));
    }

    #[test]
    fn has_three_arg_label_key_value() {
        let json = bc(serde_json::json!([
            ["V"],
            ["has", "Person", "name", "marko"]
        ]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("(n:`Person`)"));
        assert!(cypher.contains("n.`name` = 'marko'"));
    }

    #[test]
    fn in_traversal() {
        let json = bc(serde_json::json!([["V"], ["in", "knows"]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("(n)<-[:`knows`]-(n_in)"));
    }

    #[test]
    fn both_traversal() {
        let json = bc(serde_json::json!([["V"], ["both", "knows"]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("(n)-[:`knows`]-(n_both)"));
    }

    #[test]
    fn out_with_no_label() {
        let json = bc(serde_json::json!([["V"], ["out"]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        // Empty rel label produces []
        assert!(cypher.contains("(n)-[]->(n_out)"));
    }

    #[test]
    fn out_e_and_in_e_traversals() {
        let cypher_oute =
            bytecode_to_cypher(&bc(serde_json::json!([["V"], ["outE", "rel"]]))).unwrap();
        assert!(cypher_oute.contains("(n)-[n_e:`rel`]->()"));

        let cypher_ine = bytecode_to_cypher(&bc(serde_json::json!([["V"], ["inE"]]))).unwrap();
        assert!(cypher_ine.contains("(n)<-[n_e]-()"));
    }

    #[test]
    fn unsupported_step_is_ignored() {
        // "wibble" is unknown — it should be silently dropped, not error.
        let json = bc(serde_json::json!([["V"], ["wibble"], ["count"]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("count(n)"));
    }

    #[test]
    fn graphson_int64_typed_value_for_v_id_and_limit() {
        let json = bc(serde_json::json!([
            ["V", {"@type": "g:Int64", "@value": 42}],
            ["limit", {"@type": "g:Int64", "@value": 5}]
        ]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("id(n) = 42"));
        assert!(cypher.contains("LIMIT 5"));
    }

    #[test]
    fn has_with_typed_int_value() {
        let json = bc(serde_json::json!([
            ["V"],
            ["has", "age", {"@type": "g:Int64", "@value": 29}]
        ]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("n.`age` = 29"));
    }

    #[test]
    fn has_with_float_and_bool() {
        let cy_float =
            bytecode_to_cypher(&bc(serde_json::json!([["V"], ["has", "score", 1.5]]))).unwrap();
        assert!(cy_float.contains("n.`score` = 1.5"));

        let cy_bool =
            bytecode_to_cypher(&bc(serde_json::json!([["V"], ["has", "active", true]]))).unwrap();
        assert!(cy_bool.contains("n.`active` = true"));
    }

    #[test]
    fn escape_string_literal_handles_quotes_and_escapes() {
        // Run through the public translation surface: a value containing
        // single quotes, backslashes, newlines must be escaped.
        let json = bc(serde_json::json!([
            ["V"],
            ["has", "note", "it's a \\ \n test"]
        ]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains(r"\'"), "single quote not escaped: {cypher}");
        assert!(cypher.contains(r"\\"), "backslash not escaped: {cypher}");
        assert!(cypher.contains(r"\n"), "newline not escaped: {cypher}");
    }

    #[test]
    fn escape_identifier_handles_backticks() {
        // Label containing a backtick must be doubled.
        let json = bc(serde_json::json!([["V"], ["hasLabel", "weird`label"]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(
            cypher.contains("`weird``label`"),
            "backtick not doubled: {cypher}"
        );
    }

    #[test]
    fn graphson_to_i64_on_plain_int_via_v() {
        // Plain integer (not wrapped in @type/@value) must still parse.
        let json = bc(serde_json::json!([["V", 99]]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("id(n) = 99"));
    }

    #[test]
    fn has_two_arg_unrecognized_value_is_dropped() {
        // An object value that doesn't unwrap to a literal should be skipped
        // (no WHERE clause appended), still producing a valid query.
        let json = bc(serde_json::json!([
            ["V"],
            ["has", "weird", {"unknown": "shape"}]
        ]));
        let cypher = bytecode_to_cypher(&json).unwrap();
        assert!(cypher.contains("MATCH (n)"));
        assert!(!cypher.contains("WHERE"));
    }
}
