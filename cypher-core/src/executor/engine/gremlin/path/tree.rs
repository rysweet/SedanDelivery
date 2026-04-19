//! Gremlin executor: `tree()` step implementation.

use indexmap::IndexMap;

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

impl<'a> QueryExecutor<'a> {
    /// Execute `GremlinTree` — build a tree structure from traversal paths.
    pub(crate) fn exec_gremlin_tree(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinTree {
            child,
            path_aliases,
            side_effect_key,
            alias,
        } = op
        else {
            debug_assert!(false, "expected GremlinTree operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinTree operator".into(),
            }));
        };

        let mut input = self.exec_operator(cx, child)?;

        let agg_col = side_effect_key
            .as_ref()
            .map(|k| format!("__gremlin_agg_{k}"));

        // Retrieve existing tree from side-effect store (accumulation in local()).
        let mut tree: IndexMap<Box<str>, Value> = if let Some(col) = &agg_col {
            let se = self.gremlin_side_effects.borrow();
            if let Some(Value::Map(existing)) = se.get(col) {
                existing.clone()
            } else {
                IndexMap::new()
            }
        } else {
            IndexMap::new()
        };

        for row in &input {
            cx.budget.check_or_err()?;
            let path: Vec<Value> = path_aliases
                .iter()
                .filter_map(|a| row.get(a.as_str()).cloned())
                .filter(|v| !matches!(v, Value::Null))
                .collect();
            Self::insert_tree_path(&mut tree, &path);
        }

        let tree_value = Value::Map(tree);

        if let Some(_key) = side_effect_key {
            // Side-effect mode: store tree and pass traversers through.
            let col = agg_col.unwrap();
            self.gremlin_side_effects
                .borrow_mut()
                .insert(col.clone(), tree_value.clone());
            for row in &mut input {
                row.insert(col.clone(), tree_value.clone());
            }
            Ok(input)
        } else {
            // Barrier mode: return single row with the tree Map.
            let mut row = Row::new();
            row.insert(alias.as_str().to_string(), tree_value);
            Ok(vec![row])
        }
    }

    /// Insert a path into a nested Map tree structure.
    fn insert_tree_path(tree: &mut IndexMap<Box<str>, Value>, path: &[Value]) {
        if path.is_empty() {
            return;
        }
        let key: Box<str> = format!("{}", path[0]).into();
        let subtree = tree
            .entry(key)
            .or_insert_with(|| Value::Map(IndexMap::new()));
        if let Value::Map(children) = subtree {
            Self::insert_tree_path(children, &path[1..]);
        }
    }
}
