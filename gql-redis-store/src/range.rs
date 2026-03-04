// range.rs — Range predicate evaluation and key-count scanning.

use redis::ConnectionLike;

use gql_core::error::StorageError;
use gql_core::storage::RangeQuery;
use gql_core::types::Value;

// ── Range predicate evaluation ──────────────────────────────────────────────

pub(crate) fn range_matches(val: &Value, range: &RangeQuery<'_>) -> bool {
    match range {
        RangeQuery::Eq(target) => val.cypher_eq(target) == Some(true),
        RangeQuery::Lt(target) => cmp_values(val, target) == Some(std::cmp::Ordering::Less),
        RangeQuery::Lte(target) => {
            matches!(
                cmp_values(val, target),
                Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
            )
        }
        RangeQuery::Gt(target) => cmp_values(val, target) == Some(std::cmp::Ordering::Greater),
        RangeQuery::Gte(target) => {
            matches!(
                cmp_values(val, target),
                Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
            )
        }
        RangeQuery::Between { lo, hi, inclusive } => {
            let lo_ok = if *inclusive {
                matches!(
                    cmp_values(val, lo),
                    Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
                )
            } else {
                cmp_values(val, lo) == Some(std::cmp::Ordering::Greater)
            };
            let hi_ok = if *inclusive {
                matches!(
                    cmp_values(val, hi),
                    Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
                )
            } else {
                cmp_values(val, hi) == Some(std::cmp::Ordering::Less)
            };
            lo_ok && hi_ok
        }
        RangeQuery::StartsWith(prefix) => {
            if let Value::Str(s) = val {
                s.starts_with(prefix)
            } else {
                false
            }
        }
    }
}

pub(crate) fn cmp_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(y)),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)),
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

/// Count the number of keys matching a pattern via SCAN.
pub(crate) fn scan_key_count(
    conn: &mut dyn ConnectionLike,
    pattern: &str,
) -> Result<usize, StorageError> {
    let mut cursor: u64 = 0;
    let mut count = 0usize;
    loop {
        let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(pattern)
            .arg("COUNT")
            .arg(100u64)
            .query(conn)
            .map_err(|e| StorageError::Backend { msg: e.to_string() })?;
        count += keys.len();
        cursor = next;
        if cursor == 0 {
            break;
        }
    }
    Ok(count)
}
