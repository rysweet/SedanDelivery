//! Gremlin-to-Operator compiler.
//!
//! `GremlinCompiler` takes a `Vec<GremlinStep>` and builds a `cypher_core::planner::Operator`
//! tree by walking the steps left-to-right, wrapping each subsequent step around the
//! previous operator (bottom-up construction).

use std::collections::{HashMap, HashSet};

use cypher_core::ast::{Expr, Identifier};
use cypher_core::planner::{GremlinSelectTraversal, Operator};

use crate::bytecode::GremlinStep;
use crate::types::GremlinValue;

/// Errors during Gremlin → Operator compilation.
#[derive(Debug, thiserror::Error)]
pub enum CompileError {
    #[error("unsupported Gremlin step: {0}")]
    UnsupportedStep(String),
    #[error("step requires a preceding traversal but none was provided")]
    MissingChild,
    #[error("invalid step sequence: {0}")]
    InvalidSequence(String),
    #[error("The provided traversal has a mutating step and thus is not read only")]
    ReadOnlyViolation,
    #[error("The provided traversal is setting a property key to a reserved word: {0}")]
    ReservedKeyViolation(String),
    #[error("The provided traversal contains a vertex step without any specified edge label")]
    EdgeLabelRequired,
}

/// Compiles a sequence of Gremlin steps into a cypher-core `Operator` tree.
pub struct GremlinCompiler {
    alias_counter: u32,
    /// Maps Gremlin `as()` labels to their operator-level aliases.
    alias_map: HashMap<String, Identifier>,
    /// Tracks ALL aliases assigned to each label (for Pop.first/all/mixed).
    /// When the same label is used multiple times, all aliases are recorded.
    label_stacks: HashMap<String, Vec<Identifier>>,
    /// The alias that the most recent compilation left as the "current" traverser.
    last_alias: Identifier,
    /// When true, by-modulation that can't produce a value yields null
    /// instead of dropping the traverser (ProductiveByStrategy).
    productive_by: bool,
    /// When true, mutating steps (addV, addE, property, drop) must error.
    read_only: bool,
    /// When set, property keys in this set (plus "id", "label") must error.
    reserved_keys: Option<Vec<String>>,
    /// When true, vertex steps without edge labels must error.
    edge_label_verify: bool,
    /// SeedStrategy seed value for deterministic coin/sample.
    seed: Option<i64>,
    /// Reduction info from `withSideEffect(key, init, Operator.op)`.
    /// Maps side-effect key → (operator, init_value).
    reductions: HashMap<String, (String, f64)>,
    /// Compile-time literal values for side-effects, used to resolve
    /// property keys like `property(select("a"), value)` at compile time.
    side_effect_values: HashMap<String, GremlinValue>,
    /// Aliases accumulated for path() tracking. Carried through Project
    /// operations so that path() can reconstruct the traversal history.
    path_tracking: Vec<Identifier>,
    /// The source vertex alias from the most recent edge expansion step
    /// (OutE/InE/BothE). Used by OtherV to determine which endpoint is "other".
    edge_src_vertex: Option<Identifier>,
    /// Whether the current traversal uses path()/simplePath()/cyclicPath().
    /// When true, label_carry_items includes path tracking aliases.
    needs_path_tracking: bool,
    /// Whether the current traversal uses select() with Pop modifiers.
    /// When true, label_carry_items includes all label stack aliases.
    needs_label_stacks: bool,
    /// Keys registered by aggregate() steps, used by within/without to
    /// resolve side-effect references without polluting alias_map.
    aggregate_keys: HashSet<String>,
    /// Side-effect keys initialized with Set semantics (e.g. `withSideEffect("a", {})`).
    /// Aggregate steps targeting these keys deduplicate values.
    set_keys: HashSet<String>,
    /// Path aliases from the outer (parent) scope. Set when compiling inside
    /// local()/map() so that tree() can access the full traversal path.
    outer_path_aliases: Vec<Identifier>,
    /// Tracks the numeric subtype set by `AsNumberTyped("byte"/"short")`.
    /// Used by `Is(IsType(..))` to correctly resolve type checks at compile time.
    numeric_subtype: Option<String>,
    /// When true, the seed element for this sub-traversal is a single-entry Map
    /// produced by unfold() on a Map (i.e., a Map.Entry). In this context,
    /// select(Column.keys/values) should extract the key/value directly rather
    /// than returning a collection of all keys/values.
    seed_is_map_entry: bool,
}

#[derive(Debug, Clone)]
pub(super) enum SelectByModulator {
    Key(String),
    Traversal(Vec<GremlinStep>),
}

impl Default for GremlinCompiler {
    fn default() -> Self {
        Self::new()
    }
}

impl GremlinCompiler {
    pub fn new() -> Self {
        Self {
            alias_counter: 0,
            alias_map: HashMap::new(),
            label_stacks: HashMap::new(),
            last_alias: Identifier::new("__gremlin_v"),
            productive_by: false,
            read_only: false,
            reserved_keys: None,
            edge_label_verify: false,
            seed: None,
            reductions: HashMap::new(),
            side_effect_values: HashMap::new(),
            path_tracking: Vec::new(),
            edge_src_vertex: None,
            needs_path_tracking: false,
            needs_label_stacks: false,
            aggregate_keys: HashSet::new(),
            set_keys: HashSet::new(),
            outer_path_aliases: Vec::new(),
            numeric_subtype: None,
            seed_is_map_entry: false,
        }
    }

    fn steps_have_independent_source(steps: &[GremlinStep]) -> bool {
        match steps.first() {
            Some(GremlinStep::V(_) | GremlinStep::E(_) | GremlinStep::Inject(_)) => true,
            Some(GremlinStep::Union(branches)) => {
                !branches.is_empty()
                    && branches
                        .iter()
                        .all(|branch| Self::steps_have_independent_source(branch))
            }
            Some(GremlinStep::Coalesce(alternatives)) => {
                !alternatives.is_empty()
                    && alternatives
                        .iter()
                        .all(|alt| Self::steps_have_independent_source(alt))
            }
            _ => false,
        }
    }

    fn next_alias(&mut self, prefix: &str) -> Identifier {
        self.alias_counter += 1;
        Identifier::new(format!("{prefix}_{}", self.alias_counter))
    }

    /// Create a sub-compiler that shares alias counter and alias map.
    fn sub_compiler(&self) -> Self {
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

    /// Return extra projection items that carry forward every label set via `as()`
    /// and (when path tracking is active) every path-tracking alias. This preserves
    /// step labels and path history through projection steps like `values()`.
    fn label_carry_items(&self) -> Vec<(Expr, Option<Identifier>)> {
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

    /// Absorb counter state from a sub-compiler after it finishes.
    /// Also merge any new alias_map entries and reductions so that
    /// side-effect aliases created in sub-traversals (repeat, choose, etc.)
    /// are visible to the parent compiler for downstream steps like cap().
    fn absorb(&mut self, sub: &Self) {
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

    /// Compile a match pattern sub-traversal starting from an already-bound alias.
    /// Uses `SingleRow` as the base (the match executor seeds from current bindings)
    /// and sets `current_alias` to `seed_alias` so traversal steps expand from the
    /// correct starting point.
    fn compile_with_seed(
        &mut self,
        steps: &[GremlinStep],
        seed_alias: Identifier,
    ) -> Result<Operator, CompileError> {
        if steps.is_empty() {
            return Ok(Operator::SingleRow);
        }
        self.compile_from(
            steps,
            Some(Operator::SingleRow),
            seed_alias.clone(),
            vec![seed_alias],
        )
    }

    /// Compile a sub-traversal that expects a starting context (e.g. repeat body,
    /// choose branches, optional(out()), map(values("name")), filter(has("age"))).
    /// Uses `SingleRow` as the base so the executor's `foreach_seed` injects the
    /// parent row, and sets `current_alias` to `seed_alias` so steps like out()
    /// expand from the correct traverser.
    fn compile_seeded(
        &mut self,
        steps: &[GremlinStep],
        seed_alias: &Identifier,
    ) -> Result<Operator, CompileError> {
        if steps.is_empty() {
            return Ok(Operator::SingleRow);
        }
        // If the sub-traversal starts with its own source step, compile independently.
        let has_source = Self::steps_have_independent_source(steps);
        if has_source {
            return self.compile(steps);
        }
        // Use SingleRow as base — foreach_seed injects the parent row at runtime.
        self.compile_from(
            steps,
            Some(Operator::SingleRow),
            seed_alias.clone(),
            vec![seed_alias.clone()],
        )
    }

    /// Compile a step sequence into an Operator tree.
    pub fn compile(&mut self, steps: &[GremlinStep]) -> Result<Operator, CompileError> {
        self.compile_from(steps, None, Identifier::new("__gremlin_v"), Vec::new())
    }

    /// Extract a literal expression from collection step RHS sub-traversal steps.
    /// Returns (rhs_expr, optional_apply_operator). If the RHS is a sub-traversal,
    /// the caller must wrap their child in the Apply operator before projecting.
    fn compile_collection_rhs(
        &mut self,
        rhs_steps: &[GremlinStep],
        current_alias: &Identifier,
        child: Operator,
    ) -> Result<(Expr, Operator), CompileError> {
        // Empty steps → Null (will trigger appropriate error in execution)
        if rhs_steps.is_empty() {
            return Ok((Expr::Null, child));
        }
        // Single Inject step with one list arg → literal list
        if rhs_steps.len() == 1 {
            if let GremlinStep::Inject(values) = &rhs_steps[0] {
                if values.len() == 1 {
                    return Ok((gremlin_value_to_expr(&values[0]), child));
                }
                return Ok((
                    Expr::List(values.iter().map(gremlin_value_to_expr).collect()),
                    child,
                ));
            }
            if let GremlinStep::Constant(v) = &rhs_steps[0] {
                return Ok((gremlin_value_to_expr(v), child));
            }
        }
        // Sub-traversal: compile with Apply to evaluate within the current context
        let mut sub = self.sub_compiler();
        let rhs_op = sub.compile_seeded(rhs_steps, current_alias)?;
        let rhs_alias = sub.last_alias.clone();
        self.absorb(&sub);
        let applied = Operator::Apply {
            outer: Box::new(child),
            inner: Box::new(rhs_op),
        };
        Ok((Expr::Variable(rhs_alias), applied))
    }

    /// Returns the alias for the traversal's final result column after compilation.
    pub fn result_alias(&self) -> &str {
        self.last_alias.0.as_str()
    }

    fn apply_pending_select_mods(
        &mut self,
        current: &mut Option<Operator>,
        current_alias: &mut Identifier,
        select_by_mods: &[SelectByModulator],
    ) -> Result<(), CompileError> {
        if select_by_mods.is_empty() {
            return Ok(());
        }
        if let Some(Operator::GremlinSelect {
            keys,
            by_traversals,
            ..
        }) = current
        {
            by_traversals.clear();
            by_traversals.resize(keys.len(), None);
            for (i, (src_var, _)) in keys.iter_mut().enumerate() {
                let original_src_var = src_var.clone();
                match &select_by_mods[i % select_by_mods.len()] {
                    SelectByModulator::Key(mod_key) => {
                        if !mod_key.is_empty() {
                            *src_var = Identifier::new(format!(
                                "__prop_{}__{}",
                                mod_key, original_src_var.0
                            ));
                        }
                    }
                    SelectByModulator::Traversal(steps) => {
                        let mut sub = self.sub_compiler();
                        let sub_op = sub.compile_seeded(steps, &original_src_var)?;
                        let sub_out_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        by_traversals[i] = Some(GremlinSelectTraversal::Traversal {
                            op: Box::new(sub_op),
                            out_alias: sub_out_alias,
                        });
                    }
                }
            }
            if keys.len() == 1 {
                *current_alias = Identifier::new(keys[0].1.clone());
            }
        }
        Ok(())
    }
}

mod branches;
mod compile_from;
mod compile_from_context;
mod helpers;
mod steps;
use helpers::*;

#[cfg(test)]
mod by_tests;
#[cfg(test)]
mod filter_tests;
#[cfg(test)]
mod helpers_tests;
#[cfg(test)]
mod strings_tests;
#[cfg(test)]
mod tests;
