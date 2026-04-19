//! Gremlin executor: `select` step implementations.

use crate::ast::{Expr, Identifier};
use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::helpers::inject_parameters;
use crate::executor::engine::{ExecContext, QueryExecutor, Row};

#[allow(unused_imports)]
use super::helpers::*;

impl<'a> QueryExecutor<'a> {
    /// For each input row, iterate through patterns. Each pattern is a
    /// sub-traversal that binds variables. We progressively join results
    /// by requiring consistent bindings across patterns.
    pub(crate) fn exec_gremlin_match(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinMatch {
            patterns,
            pattern_labels,
            label_map,
            result_alias,
            child,
            ..
        } = op
        else {
            debug_assert!(false, "expected GremlinMatch operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinMatch operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;

        if patterns.is_empty() {
            return Ok(input);
        }

        // Build reverse map: internal_alias → label_name.
        // Use or_insert to keep the FIRST label for each alias. When
        // Repeat remap causes two labels to share an alias, the earlier
        // binding is correct for initial row normalization.
        let mut reverse_map: std::collections::HashMap<&str, &str> =
            std::collections::HashMap::new();
        for (label, alias) in label_map {
            reverse_map
                .entry(alias.0.as_str())
                .or_insert(label.as_str());
        }

        // Normalize a row: replace internal aliases with label names.
        // This ensures consistent keys across patterns for merging.
        let normalize = |row: &Row| -> Row {
            let mut out = Row::new();
            for (k, v) in row {
                let key = if let Some(label) = reverse_map.get(k.as_str()) {
                    (*label).to_string()
                } else {
                    k.clone()
                };
                out.insert(key, v.clone());
            }
            out
        };

        // For each input row, execute all patterns and join on shared bindings.
        let mut out = Vec::new();
        for row in &input {
            cx.budget.check_or_err()?;

            // Normalize input row so label names are used as keys.
            let mut current_bindings = vec![normalize(row)];

            for (pattern_idx, pattern) in patterns.iter().enumerate() {
                let mut next_bindings = Vec::new();

                // Build per-pattern reverse map from produced labels only.
                // This avoids extracting the seed label's value from the
                // pattern output when the seed was overwritten internally
                // (e.g. by a Repeat step).
                // Multiple labels can share the same alias (e.g. repeat().as("b")
                // maps "b" to the same alias as "a"), so we store Vec<label>.
                let p_labels = pattern_labels.get(pattern_idx);
                let p_reverse: Option<std::collections::HashMap<&str, Vec<&str>>> =
                    p_labels.map(|pl| {
                        let mut m: std::collections::HashMap<&str, Vec<&str>> =
                            std::collections::HashMap::new();
                        for (label, alias) in pl {
                            m.entry(alias.0.as_str()).or_default().push(label.as_str());
                        }
                        m
                    });

                for binding in &current_bindings {
                    cx.budget.check_or_err()?;
                    // Seed: the pattern expects internal alias names, so we
                    // need to provide the binding with both label and internal
                    // alias keys. Build a denormalized seed row.
                    // Avoid overwriting an alias already set for a different
                    // label (e.g. Repeat remap makes "a" and "b" share v_0;
                    // we keep the first label's value).
                    let mut seed = binding.clone();
                    let mut alias_owner: std::collections::HashMap<&str, &str> =
                        std::collections::HashMap::new();
                    for (label, alias) in label_map {
                        if let Some(v) = binding.get(label.as_str()) {
                            if let Some(prev_label) = alias_owner.get(alias.0.as_str()) {
                                if *prev_label != label.as_str() {
                                    continue;
                                }
                            }
                            alias_owner.insert(alias.0.as_str(), label.as_str());
                            seed.insert(alias.0.to_string(), v.clone());
                        }
                    }

                    self.foreach_seed.borrow_mut().replace(vec![seed]);
                    let pattern_rows = self.exec_operator(cx, pattern)?;
                    self.foreach_seed.borrow_mut().take();

                    // Normalize pattern results using per-pattern produced
                    // labels, then merge on shared label keys for consistency.
                    for p_row in pattern_rows {
                        let norm = if let Some(ref pr) = p_reverse {
                            let mut n = Row::new();
                            for (k, v) in &p_row {
                                if let Some(labels) = pr.get(k.as_str()) {
                                    // Emit all labels sharing this alias (e.g.
                                    // repeat().as("b") causes "a" and "b" to
                                    // share the same column alias).
                                    for label in labels {
                                        n.insert((*label).to_string(), v.clone());
                                    }
                                }
                            }
                            n
                        } else {
                            normalize(&p_row)
                        };
                        let mut merged = binding.clone();
                        let mut consistent = true;
                        for (k, v) in &norm {
                            if let Some(existing) = merged.get(k) {
                                if existing != v {
                                    consistent = false;
                                    break;
                                }
                            } else {
                                merged.insert(k.clone(), v.clone());
                            }
                        }
                        if consistent {
                            next_bindings.push(merged);
                        }
                    }
                }

                current_bindings = next_bindings;
                if current_bindings.is_empty() {
                    break;
                }
            }

            // Build output rows with label columns and a Map result.
            for binding in current_bindings {
                let mut result_row = Row::new();
                let mut map_entries = indexmap::IndexMap::new();
                // Insert label columns from binding.
                for (k, v) in &binding {
                    result_row.insert(k.clone(), v.clone());
                    map_entries
                        .entry(Box::from(k.as_str()))
                        .or_insert_with(|| v.clone());
                }
                // Set alias columns deterministically: iterate label_map
                // in order (pattern-1 entries first, then pattern-2, etc.)
                // so the FIRST label claiming an alias wins.  This avoids
                // non-deterministic HashMap iteration reordering the labels
                // when two labels share an alias (Repeat remap).
                let mut alias_set = std::collections::HashSet::new();
                for (label, alias) in label_map {
                    if alias_set.contains(alias.0.as_str()) {
                        continue;
                    }
                    if let Some(v) = binding.get(label.as_str()) {
                        alias_set.insert(alias.0.as_str());
                        result_row.insert(alias.0.to_string(), v.clone());
                    }
                }
                result_row.insert(result_alias.0.to_string(), Value::Map(map_entries));
                out.push(result_row);
            }
        }

        Ok(out)
    }

    /// Execute `GremlinSelect` — project labeled step values, skipping rows
    /// where any referenced label is not bound in the row.
    pub(crate) fn exec_gremlin_select(
        &self,
        cx: &mut ExecContext<'_>,
        op: &Operator,
    ) -> Result<Vec<Row>, GqlError> {
        let Operator::GremlinSelect {
            keys,
            by_traversals,
            productive,
            pop,
            label_stacks,
            child,
        } = op
        else {
            debug_assert!(false, "expected GremlinSelect operator");
            return Err(GqlError::Execution(ExecutionError::UnsupportedFeature {
                feature: "internal: expected GremlinSelect operator".into(),
            }));
        };

        let input = self.exec_operator(cx, child)?;
        let mut out = Vec::new();

        for row in &input {
            cx.budget.check_or_err()?;
            let ctx = self.build_filter_ctx(cx.graph, row)?;

            // Check that ALL referenced variables are present in the row.
            let mut all_found = true;
            let mut new_row = Row::new();

            for (idx, (src_var, output_key)) in keys.iter().enumerate() {
                let var_name = src_var.0.as_str();
                let by_traversal = by_traversals.get(idx).and_then(|entry| entry.as_ref());

                // Pop modifier handling: when Pop is set and label_stacks has
                // multiple aliases for this key, resolve values from all aliases.
                if let Some(pop_mode) = pop {
                    use crate::planner::GremlinPop;
                    let stack = label_stacks
                        .iter()
                        .find(|(k, _)| k == output_key)
                        .map(|(_, v)| v.as_slice())
                        .unwrap_or(&[]);
                    if !stack.is_empty() {
                        // Repeat() uses synthetic pop_orig_* aliases to preserve
                        // the original binding for Pop.first when a label is
                        // rebound inside the loop body. Those shadow aliases
                        // must not leak into Pop.all/last/mixed materialization.
                        let mut vals: Vec<Value> = Vec::new();
                        for alias in stack.iter().filter(|alias| {
                            matches!(pop_mode, GremlinPop::First)
                                || !alias.0.as_str().starts_with("pop_orig_")
                        }) {
                            if let Some(v) = row.get(alias.0.as_str()) {
                                vals.push(v.clone());
                            }
                        }
                        if vals.is_empty() {
                            all_found = false;
                            break;
                        }
                        let resolved = match pop_mode {
                            GremlinPop::First => vals.into_iter().next().unwrap(),
                            GremlinPop::Last => vals.into_iter().last().unwrap(),
                            GremlinPop::All => Value::List(vals),
                            GremlinPop::Mixed => {
                                if vals.len() == 1 {
                                    vals.into_iter().next().unwrap()
                                } else {
                                    Value::List(vals)
                                }
                            }
                        };
                        // Apply by-modulation: if src_var has a __prop_ prefix,
                        // extract the property from the resolved value.
                        let resolved = if let Some(rest) = var_name.strip_prefix("__prop_") {
                            if let Some((prop_name, _real_var)) = self.split_prop_accessor(rest) {
                                if let Some(n_str) = prop_name.strip_prefix("~limit_local:") {
                                    let n = n_str.parse::<usize>().unwrap_or(0);
                                    match resolved {
                                        Value::List(items) => {
                                            Value::List(items.into_iter().take(n).collect())
                                        }
                                        other => other,
                                    }
                                } else if let Some(n_str) = prop_name.strip_prefix("~tail_local:") {
                                    let n = n_str.parse::<usize>().unwrap_or(0);
                                    match resolved {
                                        Value::List(items) => {
                                            let skip = items.len().saturating_sub(n);
                                            Value::List(items.into_iter().skip(skip).collect())
                                        }
                                        other => other,
                                    }
                                } else {
                                    self.resolve_prop_on_value(cx, prop_name, &resolved)
                                }
                            } else {
                                resolved
                            }
                        } else {
                            resolved
                        };
                        let resolved = if let Some(traversal) = by_traversal {
                            self.apply_select_by_traversal(cx, row, src_var, resolved, traversal)?
                        } else {
                            resolved
                        };
                        let empty_local = matches!(&resolved, Value::List(items) if items.is_empty())
                            || matches!(&resolved, Value::Map(map) if map.is_empty());
                        if !*productive && empty_local {
                            all_found = false;
                            break;
                        }
                        new_row.insert(output_key.clone(), resolved);
                        continue;
                    }
                }

                // Handle property-accessor pattern: __prop_PROPNAME__VARNAME
                if let Some(rest) = var_name.strip_prefix("__prop_") {
                    if let Some((prop_name, real_var)) = self.split_prop_accessor(rest) {
                        let val = row.get(real_var);
                        let selected = if let Some(v) = self.resolve_map_select_value(row, real_var)
                        {
                            Some(v)
                        } else {
                            val.cloned()
                        };
                        match selected.as_ref() {
                            Some(node_val) => {
                                // Handle special property names (~id, T.id, ~label, T.label)
                                let resolved = if prop_name == "~id"
                                    || prop_name == "T.id"
                                    || prop_name == "t[id]"
                                {
                                    match node_val {
                                        Value::Node(nid) => Some(Value::Int(nid.0 as i64)),
                                        Value::Edge(eid) => Some(Value::Int(eid.0 as i64)),
                                        _ => None,
                                    }
                                } else if prop_name == "~label"
                                    || prop_name == "T.label"
                                    || prop_name == "t[label]"
                                {
                                    match node_val {
                                        Value::Node(nid) => {
                                            if let Ok(Some(node)) =
                                                self.store.get_node(cx.graph, *nid)
                                            {
                                                node.labels
                                                    .into_iter()
                                                    .next()
                                                    .map(|l| Value::Str(l.into_boxed_str()))
                                            } else {
                                                None
                                            }
                                        }
                                        Value::Edge(eid) => {
                                            if let Ok(Some(edge)) =
                                                self.store.get_edge(cx.graph, *eid)
                                            {
                                                Some(Value::Str(edge.rel_type.into_boxed_str()))
                                            } else {
                                                None
                                            }
                                        }
                                        _ => None,
                                    }
                                } else {
                                    None
                                };

                                if let Some(v) = resolved {
                                    let v = if let Some(traversal) = by_traversal {
                                        self.apply_select_by_traversal(
                                            cx, row, src_var, v, traversal,
                                        )?
                                    } else {
                                        v
                                    };
                                    new_row.insert(output_key.clone(), v);
                                    continue;
                                }

                                // Handle tilde-prefixed transformation keys
                                // (e.g., ~unfold_values_fold:name, ~toupper:prop)
                                // via resolve_prop_on_value instead of raw
                                // property access.
                                if prop_name.starts_with('~') {
                                    let r = self.resolve_prop_on_value(cx, prop_name, node_val);
                                    let r = if let Some(traversal) = by_traversal {
                                        self.apply_select_by_traversal(
                                            cx, row, src_var, r, traversal,
                                        )?
                                    } else {
                                        r
                                    };
                                    if !*productive && matches!(r, Value::Null) {
                                        all_found = false;
                                        break;
                                    }
                                    new_row.insert(output_key.clone(), r);
                                    continue;
                                }

                                // Resolve property from the node/edge
                                let prop_expr = Expr::Property(
                                    Box::new(Expr::Variable(Identifier::new(real_var))),
                                    smol_str::SmolStr::new(prop_name),
                                );
                                match self.eval_with_storage(cx.graph, &prop_expr, row, &ctx) {
                                    Ok(v) => {
                                        let v = if let Some(traversal) = by_traversal {
                                            self.apply_select_by_traversal(
                                                cx, row, src_var, v, traversal,
                                            )?
                                        } else {
                                            v
                                        };
                                        if !*productive && matches!(v, Value::Null) {
                                            all_found = false;
                                            break;
                                        }
                                        new_row.insert(output_key.clone(), v);
                                    }
                                    Err(_) => {
                                        // Property doesn't exist
                                        if !*productive {
                                            // Non-productive: drop the traverser
                                            if let Value::Map(m) = node_val {
                                                let v = m.get(prop_name).cloned();
                                                match v {
                                                    Some(val) => {
                                                        let val = if let Some(traversal) =
                                                            by_traversal
                                                        {
                                                            self.apply_select_by_traversal(
                                                                cx, row, src_var, val, traversal,
                                                            )?
                                                        } else {
                                                            val
                                                        };
                                                        new_row.insert(output_key.clone(), val);
                                                    }
                                                    None => {
                                                        all_found = false;
                                                        break;
                                                    }
                                                }
                                            } else {
                                                all_found = false;
                                                break;
                                            }
                                        } else {
                                            // Productive: use Null
                                            if let Value::Map(m) = node_val {
                                                let v = m
                                                    .get(prop_name)
                                                    .cloned()
                                                    .unwrap_or(Value::Null);
                                                new_row.insert(output_key.clone(), v);
                                            } else {
                                                new_row.insert(output_key.clone(), Value::Null);
                                            }
                                        }
                                    }
                                }
                            }
                            None => {
                                all_found = false;
                                break;
                            }
                        }
                        continue;
                    }
                }

                // Try looking up in the row directly.
                let val = if let Some(v) = self.resolve_map_select_value(row, var_name) {
                    Some(v)
                } else if let Some(v) = row.get(var_name) {
                    Some(v.clone())
                } else if let Some(v) = row.get(&format!("__gremlin_agg_{}", output_key)) {
                    // Fallback: aggregate sideeffect columns are stored
                    // with a __gremlin_agg_ prefix.
                    Some(v.clone())
                } else if let Some(v) = self
                    .gremlin_side_effects
                    .borrow()
                    .get(&format!("__gremlin_agg_{}", output_key))
                    .cloned()
                {
                    // Fallback: check the global side-effect store.
                    // Inside local(), the seed row doesn't carry the
                    // aggregate column, but the store accumulates it.
                    Some(v)
                } else {
                    // For select() after group(): the row may contain a Map value.
                    // Try looking up output_key as a map key in any Map values in the row.
                    let mut found = None;
                    for (_k, v) in row.iter() {
                        if let Value::Map(m) = v {
                            if let Some(map_val) = m.get(output_key.as_str()) {
                                found = Some(map_val.clone());
                                break;
                            }
                        }
                    }
                    if found.is_none() {
                        // Last-resort variable resolution via expression eval.
                        // .ok() is intentional: a missing variable here means all
                        // prior lookup paths failed, which is a valid "not found"
                        // in Gremlin select() semantics — not an error to propagate.
                        let expr = Expr::Variable(src_var.clone());
                        crate::executor::filter::eval_expr(&expr, &ctx).ok()
                    } else {
                        found
                    }
                };

                match val {
                    Some(v) => {
                        // Select-from-Map: when the resolved value is a Map
                        // containing the output key, extract the Map entry.
                        // This implements Gremlin's behavior where select(key)
                        // on a Map traverser acts as a Map key selector.
                        let v = if keys.len() == 1 {
                            if let Value::Map(ref m) = v {
                                m.get(output_key.as_str()).cloned().unwrap_or(v)
                            } else {
                                v
                            }
                        } else {
                            v
                        };
                        let v = if let Some(traversal) = by_traversal {
                            self.apply_select_by_traversal(cx, row, src_var, v, traversal)?
                        } else {
                            v
                        };
                        new_row.insert(output_key.clone(), v);
                    }
                    None => {
                        all_found = false;
                        break;
                    }
                }
            }

            if all_found {
                // If multiple keys, produce a Map value (Gremlin select semantics)
                if keys.len() > 1 {
                    let mut map: indexmap::IndexMap<Box<str>, Value> = indexmap::IndexMap::new();
                    for (_, output_key) in keys {
                        if let Some(v) = new_row.get(output_key.as_str()) {
                            map.insert(output_key.clone().into(), v.clone());
                        }
                    }
                    let mut map_row = Row::new();
                    inject_parameters(&mut map_row, &self.parameters);
                    // Carry forward Gremlin internal columns (path
                    // accumulator, sack, side-effects) so downstream steps
                    // can access them after multi-key select.
                    for (k, v) in row.iter() {
                        if k.starts_with("__gremlin_") || k == "__sack" {
                            map_row.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                    // Add individual key columns AND their internal alias
                    // columns so downstream where/filter can reference them.
                    for (src_var, output_key) in keys {
                        if let Some(v) = new_row.get(output_key.as_str()) {
                            map_row.insert(output_key.clone(), v.clone());
                            map_row.insert(src_var.0.to_string(), v.clone());
                        }
                    }
                    let map_alias = keys
                        .iter()
                        .map(|(_, k)| k.as_str())
                        .collect::<Vec<_>>()
                        .join("_");
                    map_row.insert(map_alias, Value::Map(map));
                    cx.budget.record_row();
                    out.push(map_row);
                } else {
                    // Single-key select: carry forward existing row columns
                    // so that as() labels remain accessible to downstream steps
                    // (needed for addE/from/to resolution after select/unfold).
                    let mut out_row = row.clone();
                    for (src_var, output_key) in keys {
                        if let Some(v) = new_row.get(output_key.as_str()) {
                            out_row.insert(output_key.clone(), v.clone());
                            // Always update the compiler source alias so
                            // downstream steps operate on the selected value.
                            out_row.insert(src_var.0.to_string(), v.clone());
                        }
                    }
                    inject_parameters(&mut out_row, &self.parameters);
                    cx.budget.record_row();
                    out.push(out_row);
                }
            }
        }

        Ok(out)
    }
}
