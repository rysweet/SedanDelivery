//! `GremlinIndex` — pair each element with its positional index.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::executor::engine::{ExecContext, QueryExecutor, Row};
use crate::planner::Operator;
use crate::types::Value;

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinIndex` — pair each element with its positional index.
    pub(crate) fn exec_gremlin_index(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinIndex {
            child,
            alias,
            out_alias,
            with_map,
        } = op
        else {
            debug_assert!(false, "expected GremlinIndex operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinIndex operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::with_capacity(input.len());

        // Check if input is a single row containing a List — index its elements
        if input.len() == 1 {
            if let Some(Value::List(items)) = input[0].get(alias.as_str()) {
                if *with_map {
                    // Produce a single Map {0: elem0, 1: elem1, ...}
                    let mut map = IndexMap::new();
                    for (i, item) in items.iter().enumerate() {
                        map.insert(format!("{i}").into_boxed_str(), item.clone());
                    }
                    let mut row = Row::new();
                    row.insert(out_alias.0.clone(), Value::Map(map));
                    out.push(row);
                } else {
                    // Produce a single row containing the list of [element, index] pairs.
                    // TinkerPop index() on a folded list returns a list of pairs so that
                    // subsequent unfold() yields each pair as a List value rather than
                    // unfolding the pairs themselves.
                    let pairs: Vec<Value> = items
                        .iter()
                        .enumerate()
                        .map(|(i, item)| Value::List(vec![item.clone(), Value::Int(i as i64)]))
                        .collect();
                    let mut row = input[0].clone();
                    row.insert(out_alias.0.clone(), Value::List(pairs));
                    out.push(row);
                }
                return Ok(out);
            }
        }

        // Stream mode: each individual traverser gets index 0 (per TinkerPop semantics).
        if *with_map {
            // with(map) mode: each individual traverser → {0: element}
            for row in input.iter() {
                if let Some(val) = row.get(alias.as_str()) {
                    let mut map = IndexMap::new();
                    map.insert("0".to_string().into_boxed_str(), val.clone());
                    let mut new_row = Row::new();
                    new_row.insert(out_alias.0.clone(), Value::Map(map));
                    out.push(new_row);
                }
            }
        } else {
            // Collect all [element, 0] pairs into a single list for unfold().
            let mut pairs = Vec::with_capacity(input.len());
            for row in input.iter() {
                cx.budget.check_or_err()?;
                let val = row.get(alias.as_str()).cloned().unwrap_or(Value::Null);
                pairs.push(Value::List(vec![val, Value::Int(0)]));
            }
            let mut row = Row::new();
            row.insert(out_alias.0.clone(), Value::List(pairs));
            out.push(row);
        }
        Ok(out)
    }
}
