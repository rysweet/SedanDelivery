// Wire-format serialization shared by GQL.QUERY and GQL.MUTATE.

use redis_module::RedisValue;

use cypher_core::serde_value::value_to_json;

/// Serialise query result rows to JSON-encoded RedisValue bulk strings.
///
/// Used by both GQL.QUERY and GQL.MUTATE to convert `QueryResult.rows`
/// into the wire format returned to clients.
pub(crate) fn serialize_rows(result: &cypher_core::executor::QueryResult) -> Vec<RedisValue> {
    result
        .rows
        .iter()
        .map(|row| {
            // Emit every declared column — missing columns become JSON null
            // (consistent with Neo4j wire protocol for optional-match results).
            let json_row: serde_json::Map<String, serde_json::Value> = result
                .columns
                .iter()
                .map(|col| {
                    let jv = match row.get(col) {
                        Some(v) => match value_to_json(v) {
                            Ok(jv) => jv,
                            Err(e) => {
                                tracing::warn!(column = %col, "value_to_json failed, substituting null: {e}");
                                serde_json::Value::Null
                            }
                        },
                        None => serde_json::Value::Null,
                    };
                    (col.clone(), jv)
                })
                .collect();
            RedisValue::BulkString(
                serde_json::to_string(&json_row)
                    .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
            )
        })
        .collect()
}
