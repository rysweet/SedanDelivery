//! Compiler state propagation: sub-compiler creation, absorption of state from
//! finished sub-compilers, and computation of label/path carry-forward
//! projection items used to preserve `as()` labels and path history through
//! projection steps.

use cypher_core::ast::{Expr, Identifier};

use super::GremlinCompiler;

impl GremlinCompiler {
    /// Create a sub-compiler that shares alias counter and alias map.
    pub(super) fn sub_compiler(&self) -> Self {
        Self {
            alias_counter: self.alias_counter,
            alias_map: self.alias_map.clone(),
            label_stacks: self.label_stacks.clone(),
            last_alias: self.last_alias.clone(),
            productive_by: self.productive_by,
            read_only: self.read_only,
            reserved_keys: self.reserved_keys.clone(),
            edge_label_verify: self.edge_label_verify,
            seed: self.seed,
            reductions: self.reductions.clone(),
            side_effect_values: self.side_effect_values.clone(),
            // Sub-compilers should NOT carry parent path tracking — those
            // aliases belong to the outer scope and would pollute inner
            // sub-traversal projections.
            path_tracking: Vec::new(),
            edge_src_vertex: self.edge_src_vertex.clone(),
            // Propagate path-tracking need so sub-traversals (repeat body,
            // optional body, etc.) carry __gremlin_path_acc through Projects.
            needs_path_tracking: self.needs_path_tracking,
            needs_label_stacks: self.needs_label_stacks,
            aggregate_keys: self.aggregate_keys.clone(),
            set_keys: self.set_keys.clone(),
            outer_path_aliases: Vec::new(),
            numeric_subtype: None,
            seed_is_map_entry: false,
        }
    }

    /// Absorb counter state from a sub-compiler after it finishes.
    /// Also merge any new alias_map entries and reductions so that
    /// side-effect aliases created in sub-traversals (repeat, choose, etc.)
    /// are visible to the parent compiler for downstream steps like cap().
    pub(super) fn absorb(&mut self, sub: &Self) {
        self.alias_counter = sub.alias_counter;
        // Merge new aliases from sub-compiler (e.g., groupCount("a") in repeat body)
        for (k, v) in &sub.alias_map {
            self.alias_map.entry(k.clone()).or_insert_with(|| v.clone());
        }
        // Merge label stacks from sub-compiler (extend, dedup by alias name)
        for (k, v) in &sub.label_stacks {
            let parent_stack = self.label_stacks.entry(k.clone()).or_default();
            for alias in v {
                if !parent_stack.iter().any(|a| a.as_str() == alias.as_str()) {
                    parent_stack.push(alias.clone());
                }
            }
        }
        // Merge new reductions from sub-compiler
        for (k, v) in &sub.reductions {
            self.reductions
                .entry(k.clone())
                .or_insert_with(|| v.clone());
        }
        // Merge aggregate keys from sub-compiler
        for k in &sub.aggregate_keys {
            self.aggregate_keys.insert(k.clone());
        }
    }

    /// Return extra projection items that carry forward every label set via `as()`
    /// and (when path tracking is active) every path-tracking alias. This preserves
    /// step labels and path history through projection steps like `values()`.
    pub(super) fn label_carry_items(&self) -> Vec<(Expr, Option<Identifier>)> {
        let mut items = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for ident in self.alias_map.values() {
            if seen.insert(ident.0.as_str().to_owned()) {
                items.push((Expr::Variable(ident.clone()), Some(ident.clone())));
            }
        }
        // Carry all label-stack aliases only when Pop select is present
        if self.needs_label_stacks {
            for aliases in self.label_stacks.values() {
                for ident in aliases {
                    if seen.insert(ident.0.as_str().to_owned()) {
                        items.push((Expr::Variable(ident.clone()), Some(ident.clone())));
                    }
                }
            }
        }
        // Carry path-tracking aliases only when a path step is present
        if self.needs_path_tracking {
            for ident in &self.path_tracking {
                if seen.insert(ident.0.as_str().to_owned()) {
                    items.push((Expr::Variable(ident.clone()), Some(ident.clone())));
                }
            }
            // Carry __gremlin_path_acc through Projects so the repeat-body
            // execution preserves the path accumulator.
            let acc_key = "__gremlin_path_acc";
            if seen.insert(acc_key.to_owned()) {
                let acc_ident = Identifier::new(acc_key);
                items.push((Expr::Variable(acc_ident.clone()), Some(acc_ident)));
            }
        }
        items
    }
}

// (impl block ends above)
