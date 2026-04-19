//! `GremlinMatch` — pattern join semantics.

use crate::error::{ExecutionError, GqlError};
use crate::planner::Operator;
use crate::types::Value;

use crate::executor::engine::{ExecContext, QueryExecutor, Row};

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
}
