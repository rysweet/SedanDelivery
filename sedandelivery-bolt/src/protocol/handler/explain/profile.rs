//! PROFILE plan-tree formatting (with actual rows + dbHits).

use crate::packstream::value::BoltValue;

/// Convert a cypher-core Operator tree into a Bolt plan map structure with
/// per-operator profiling statistics (rows and dbHits).
///
/// Used when PROFILE is active: each operator node includes actual execution
/// metrics in addition to estimated rows.
#[allow(clippy::only_used_in_recursion)]
pub(crate) fn operator_to_profile_plan_map(
    op: &cypher_core::planner::Operator,
    depth: usize,
    estimates: Option<&std::collections::HashMap<*const cypher_core::planner::Operator, f64>>,
    profile_stats: Option<
        &std::collections::HashMap<usize, cypher_core::executor::OperatorProfileStats>,
    >,
) -> BoltValue {
    use cypher_core::planner::Operator;

    let op_type = format!("{:?}", op)
        .split('(')
        .next()
        .unwrap_or("Unknown")
        .split('{')
        .next()
        .unwrap_or("Unknown")
        .trim()
        .to_string();
    let children_bolt: Vec<BoltValue> = op
        .children()
        .into_iter()
        .map(|child| operator_to_profile_plan_map(child, depth + 1, estimates, profile_stats))
        .collect();
    let estimated_rows = estimates
        .and_then(|m| m.get(&(op as *const Operator)))
        .copied()
        .unwrap_or(1.0);

    let op_addr = op as *const Operator as usize;
    let (actual_rows, db_hits) = profile_stats
        .and_then(|stats| stats.get(&op_addr))
        .map(|s| (s.rows as i64, s.db_hits as i64))
        .unwrap_or((0, 0));

    let entries: Vec<(String, BoltValue)> = vec![
        ("operatorType".to_string(), BoltValue::String(op_type)),
        ("identifiers".to_string(), BoltValue::List(vec![])),
        (
            "args".to_string(),
            BoltValue::Map(vec![
                (
                    "runtime".to_string(),
                    BoltValue::String("INTERPRETED".to_string()),
                ),
                (
                    "EstimatedRows".to_string(),
                    BoltValue::Float(estimated_rows),
                ),
            ]),
        ),
        ("children".to_string(), BoltValue::List(children_bolt)),
        ("rows".to_string(), BoltValue::Int(actual_rows)),
        ("dbHits".to_string(), BoltValue::Int(db_hits)),
    ];
    BoltValue::Map(entries)
}
