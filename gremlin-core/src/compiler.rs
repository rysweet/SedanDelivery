//! Gremlin-to-Operator compiler.
//!
//! `GremlinCompiler` takes a `Vec<GremlinStep>` and builds a `gql_core::planner::Operator`
//! tree by walking the steps left-to-right, wrapping each subsequent step around the
//! previous operator (bottom-up construction).

use std::collections::HashMap;

use gql_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType, UnaryOp};
use gql_core::planner::{AggFunction, CreateEdgeSpec, CreatePattern, Operator, SetItem};
use gql_core::types::Value;
use smol_str::SmolStr;

use crate::bytecode::{GremlinStep, Predicate};
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

/// Compiles a sequence of Gremlin steps into a gql-core `Operator` tree.
pub struct GremlinCompiler {
    alias_counter: u32,
    /// Maps Gremlin `as()` labels to their operator-level aliases.
    alias_map: HashMap<String, Identifier>,
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
            last_alias: Identifier::new("__gremlin_v"),
            productive_by: false,
            read_only: false,
            reserved_keys: None,
            edge_label_verify: false,
            seed: None,
            reductions: HashMap::new(),
            side_effect_values: HashMap::new(),
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
            last_alias: self.last_alias.clone(),
            productive_by: self.productive_by,
            read_only: self.read_only,
            reserved_keys: self.reserved_keys.clone(),
            edge_label_verify: self.edge_label_verify,
            seed: self.seed,
            reductions: self.reductions.clone(),
            side_effect_values: self.side_effect_values.clone(),
        }
    }

    /// Return extra projection items that carry forward every label set via `as()`.
    /// This preserves step labels through projection steps like `values()`.
    fn label_carry_items(&self) -> Vec<(Expr, Option<Identifier>)> {
        let mut items = Vec::new();
        for ident in self.alias_map.values() {
            items.push((Expr::Variable(ident.clone()), Some(ident.clone())));
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
        // Merge new reductions from sub-compiler
        for (k, v) in &sub.reductions {
            self.reductions
                .entry(k.clone())
                .or_insert_with(|| v.clone());
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
        let has_source = matches!(
            steps.first(),
            Some(GremlinStep::V(_) | GremlinStep::E(_) | GremlinStep::Inject(_))
        );
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

    /// Core compilation: walks steps left-to-right, wrapping each into the
    /// operator tree. `initial_op` / `initial_alias` allow callers to seed
    /// the compilation with an existing operator and alias.
    fn compile_from(
        &mut self,
        steps: &[GremlinStep],
        initial_op: Option<Operator>,
        initial_alias: Identifier,
        initial_path_aliases: Vec<Identifier>,
    ) -> Result<Operator, CompileError> {
        if steps.is_empty() {
            return Ok(initial_op.unwrap_or(Operator::SingleRow));
        }

        // Preprocess infix and()/or() with no arguments into proper branching forms.
        let preprocessed = preprocess_infix_connectives(steps);
        let steps = &preprocessed;

        let mut current: Option<Operator> = initial_op;
        let mut current_alias = initial_alias.clone();
        // Pending properties for AddV/AddE
        let mut pending_properties: Vec<(String, GremlinValue)> = Vec::new();
        // Pending By modifiers for Order
        let mut pending_order = false;
        let mut pending_order_local = false;
        // Track whether the preceding step was group/groupCount for by() modulation
        let mut pending_group = false;
        // Track how many by() the current group has consumed (0=key, 1=value)
        let mut pending_group_by_count: u32 = 0;
        // Track whether the preceding step was select for by() modulation
        let mut pending_select = false;
        // Collected by() modulations for select (applied cyclically)
        let mut select_by_mods: Vec<String> = Vec::new();
        // Track whether the preceding step was dedup for by() modulation
        let mut pending_dedup = false;
        // Track whether dedup already consumed its single by() — second by() is an error
        let mut dedup_by_consumed = false;
        // Track whether the preceding step was aggregate for by() modulation
        let mut pending_aggregate = false;
        // Track whether the pending group is specifically a groupCount (rejects 2nd by())
        let mut pending_group_is_count = false;
        // Track whether aggregate already consumed its single by() — second by() is an error
        let mut aggregate_by_consumed = false;
        // The alias BEFORE group() changed it — used by by() modulation
        let mut pre_group_alias: Identifier = initial_alias.clone();
        // Track aliases for path() reconstruction
        let mut path_aliases: Vec<Identifier> = initial_path_aliases;
        // Pending emit/until/times that appear before repeat()
        let mut pending_emit = false;
        let mut pending_emit_pred: Option<Vec<GremlinStep>> = None;
        let mut pending_until: Option<Vec<GremlinStep>> = None;
        let mut pending_times: Option<usize> = None;
        let mut pending_sack_init: Option<Expr> = None;
        // Track whether SackOp just ran, so that a following By() can rewrite the rhs
        let mut pending_sack_op = false;
        // Track whether the preceding step was project for by() modulation
        let mut pending_project = false;
        // Accumulated project keys, consumed one-by-one by by() steps
        let mut project_keys: Vec<String> = Vec::new();
        // How many by() steps have been consumed for the current project
        let mut project_by_count: usize = 0;
        // The alias of the traverser before project() ran (needed for by() sub-expressions)
        let mut pre_project_alias: Identifier = initial_alias.clone();
        // Track format() step state for by() modulation
        let mut pending_format = false;
        let mut format_pattern: String = String::new();
        let mut format_pre_alias: Identifier = initial_alias.clone();
        let mut format_by_aliases: Vec<Identifier> = Vec::new();
        // The traverser alias just before addE — used to default __src/__dst
        // when from()/to() is omitted.
        let mut adde_traverser: Option<Identifier> = None;
        // Track by() count for sample — only one allowed
        let mut pending_sample = false;
        let mut sample_by_consumed = false;
        // Track by() count for sack ops — only one allowed
        let mut pending_sack_by_consumed = false;
        // Track by() count for valueMap — only one allowed
        let mut pending_valuemap = false;
        let mut valuemap_by_consumed = false;
        // Track path() step for by() modulation
        let mut pending_path = false;
        let mut path_by_keys: Vec<String> = Vec::new();
        // Deferred output alias for ChooseMap — applied on the first non-Option step
        let mut pending_choose_output: Option<Identifier> = None;
        // Track where() label comparison for by() modulation.
        // Stores (left_var, right_var) so by("key") can rewrite to property access.
        let mut pending_where_by: Option<(Identifier, Identifier)> = None;

        for step in steps {
            // When we leave the addE modifier group (from/to/property), fill in
            // any __src/__dst placeholders that were never overridden.
            if adde_traverser.is_some()
                && !matches!(
                    step,
                    GremlinStep::From(_) | GremlinStep::To(_) | GremlinStep::Property(..)
                )
            {
                if let Some(ref traverser) = adde_traverser.take() {
                    if let Some(ref mut op) = current {
                        rewrite_create_alias(op, "__src", traverser);
                        rewrite_create_alias(op, "__dst", traverser);
                    }
                }
            }
            // Reset aggregate by-consumed flag on any non-By step
            if !matches!(step, GremlinStep::By(..) | GremlinStep::ByTraversal(..)) {
                aggregate_by_consumed = false;
                dedup_by_consumed = false;
                pending_sack_op = false;
                pending_sack_by_consumed = false;
                pending_sample = false;
                sample_by_consumed = false;
                pending_valuemap = false;
                valuemap_by_consumed = false;
                // Flush accumulated path by-keys into the GremlinPath operator
                if pending_path && !path_by_keys.is_empty() {
                    if let Some(Operator::GremlinPath {
                        ref mut by_keys, ..
                    }) = current
                    {
                        *by_keys = path_by_keys.clone();
                    }
                }
                pending_path = false;
                path_by_keys.clear();
                pending_project = false;
                pending_where_by = None;
                // Flush pending order: order() without by() → sort by identity
                if pending_order && !pending_order_local {
                    let child = current.take().unwrap_or(Operator::SingleRow);
                    current = Some(Operator::Sort {
                        items: vec![(Expr::Variable(current_alias.clone()), true)],
                        gremlin_order: true,
                        child: Box::new(child),
                    });
                    pending_order = false;
                }
                // Finalize pending format with collected by() modulations
                if pending_format && !format_by_aliases.is_empty() {
                    let child = current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("fmt");
                    // Build gremlin_format_by args: pattern, then each by-alias value
                    let mut args = vec![
                        Expr::Variable(format_pre_alias.clone()),
                        Expr::Str(format_pattern.clone().into_boxed_str()),
                    ];
                    for by_alias in &format_by_aliases {
                        args.push(Expr::Variable(by_alias.clone()));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_format_by"),
                            args,
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(project),
                    });
                    current_alias = alias;
                    format_by_aliases.clear();
                }
                pending_format = false;
                // Apply collected select by() modulations cyclically
                if pending_select && !select_by_mods.is_empty() {
                    if let Some(Operator::GremlinSelect { ref mut keys, .. }) = current {
                        for (i, (src_var, _)) in keys.iter_mut().enumerate() {
                            let mod_key = &select_by_mods[i % select_by_mods.len()];
                            if !mod_key.is_empty() {
                                *src_var =
                                    Identifier::new(format!("__prop_{}__{}", mod_key, src_var.0));
                            }
                        }
                        // After by() projection, result lives under the output
                        // key name, not the raw internal alias.
                        if keys.len() == 1 && !select_by_mods.is_empty() {
                            current_alias = Identifier::new(keys[0].1.clone());
                        }
                    }
                }
                pending_select = false;
            }
            // Flush pending choose output alias on the first non-Option step
            if !matches!(step, GremlinStep::Option(..)) {
                if let Some(alias) = pending_choose_output.take() {
                    current_alias = alias;
                }
            }
            match step {
                GremlinStep::V(ids) => {
                    let alias = self.next_alias("v");
                    path_aliases.push(alias.clone());
                    let scan = Operator::NodeScan {
                        label: None,
                        alias: alias.clone(),
                    };
                    // Flatten list parameters: V(listParam) → V(id1, id2, ...)
                    let flat_ids: Option<Vec<&GremlinValue>> = ids.as_ref().map(|id_list| {
                        let mut flat = Vec::new();
                        for id in id_list {
                            if let GremlinValue::List(inner) = id {
                                flat.extend(inner.iter());
                            } else {
                                flat.push(id);
                            }
                        }
                        flat
                    });
                    let new_op = match &flat_ids {
                        Some(id_list) if id_list.len() == 1 => {
                            let id_expr = gremlin_value_to_expr(id_list[0]);
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::Eq,
                                    left: Box::new(Expr::FunctionCall {
                                        name: SmolStr::new("id"),
                                        args: vec![Expr::Variable(alias.clone())],
                                        distinct: false,
                                    }),
                                    right: Box::new(id_expr),
                                },
                                child: Box::new(scan),
                            }
                        }
                        Some(id_list) if !id_list.is_empty() => {
                            let id_exprs: Vec<Expr> =
                                id_list.iter().map(|v| gremlin_value_to_expr(v)).collect();
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::In,
                                    left: Box::new(Expr::FunctionCall {
                                        name: SmolStr::new("id"),
                                        args: vec![Expr::Variable(alias.clone())],
                                        distinct: false,
                                    }),
                                    right: Box::new(Expr::List(id_exprs)),
                                },
                                child: Box::new(scan),
                            }
                        }
                        _ => scan,
                    };
                    // Mid-traversal V(): cross-join with existing rows
                    if let Some(child) = current.take() {
                        current = Some(Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(new_op),
                        });
                    } else {
                        current = Some(new_op);
                    }
                    current_alias = alias;
                }

                GremlinStep::E(ids) => {
                    // Edge scan: NodeScan → Expand (all types) → optional id filter
                    let src = self.next_alias("src");
                    let rel_alias = self.next_alias("e");
                    let dst = self.next_alias("dst");
                    let expand = Operator::Expand {
                        direction: Direction::Outgoing,
                        rel_types: vec![],
                        src_alias: src.clone(),
                        rel_alias: Some(rel_alias.clone()),
                        dst_alias: dst,
                        child: Box::new(Operator::NodeScan {
                            label: None,
                            alias: src,
                        }),
                    };
                    current_alias = rel_alias.clone();
                    path_aliases.push(current_alias.clone());
                    let new_op = match ids {
                        Some(id_list) if id_list.len() == 1 => {
                            let id_expr = gremlin_value_to_expr(&id_list[0]);
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::Eq,
                                    left: Box::new(Expr::FunctionCall {
                                        name: SmolStr::new("id"),
                                        args: vec![Expr::Variable(rel_alias)],
                                        distinct: false,
                                    }),
                                    right: Box::new(id_expr),
                                },
                                child: Box::new(expand),
                            }
                        }
                        Some(id_list) if !id_list.is_empty() => {
                            let id_exprs: Vec<Expr> =
                                id_list.iter().map(gremlin_value_to_expr).collect();
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::In,
                                    left: Box::new(Expr::FunctionCall {
                                        name: SmolStr::new("id"),
                                        args: vec![Expr::Variable(rel_alias)],
                                        distinct: false,
                                    }),
                                    right: Box::new(Expr::List(id_exprs)),
                                },
                                child: Box::new(expand),
                            }
                        }
                        _ => expand,
                    };
                    // Mid-traversal E(): cross-join with existing rows
                    if let Some(child) = current.take() {
                        current = Some(Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(new_op),
                        });
                    } else {
                        current = Some(new_op);
                    }
                }

                GremlinStep::HasLabel(labels) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    if labels.is_empty() {
                        // hasLabel() with no labels — no filtering needed
                        current = Some(child);
                    } else if labels.len() == 1 {
                        current = Some(Operator::Filter {
                            predicate: Expr::HasLabel(
                                Box::new(Expr::Variable(current_alias.clone())),
                                Label::new(&labels[0]),
                            ),
                            child: Box::new(child),
                        });
                    } else {
                        // OR of label checks
                        let mut checks: Vec<Expr> = labels
                            .iter()
                            .map(|l| {
                                Expr::HasLabel(
                                    Box::new(Expr::Variable(current_alias.clone())),
                                    Label::new(l.as_str()),
                                )
                            })
                            .collect();
                        let predicate = checks
                            .drain(..)
                            .reduce(|a, b| Expr::BinaryOp {
                                op: BinaryOp::Or,
                                left: Box::new(a),
                                right: Box::new(b),
                            })
                            .unwrap();
                        current = Some(Operator::Filter {
                            predicate,
                            child: Box::new(child),
                        });
                    }
                }

                GremlinStep::Has(key, value) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let prop_expr = Expr::Property(
                        Box::new(Expr::Variable(current_alias.clone())),
                        SmolStr::new(key),
                    );
                    let predicate = match value {
                        Some(val) => Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(prop_expr),
                            right: Box::new(gremlin_value_to_expr(val)),
                        },
                        None => Expr::IsNotNull(Box::new(prop_expr)),
                    };
                    current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasPredicate(key, pred) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let prop_expr = Expr::Property(
                        Box::new(Expr::Variable(current_alias.clone())),
                        SmolStr::new(key),
                    );
                    let predicate = predicate_to_prop_expr(pred, prop_expr);
                    current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasId(ids) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let id_fn = Expr::FunctionCall {
                        name: SmolStr::new("id"),
                        args: vec![Expr::Variable(current_alias.clone())],
                        distinct: false,
                    };
                    // Flatten list parameters: hasId(id1, [id2, id3]) → hasId(id1, id2, id3)
                    let mut flat_ids = Vec::new();
                    for id in ids {
                        match id {
                            GremlinValue::List(inner) => flat_ids.extend(inner.iter().cloned()),
                            other => flat_ids.push(other.clone()),
                        }
                    }
                    let predicate = if flat_ids.len() == 1 {
                        Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(id_fn),
                            right: Box::new(gremlin_value_to_expr(&flat_ids[0])),
                        }
                    } else {
                        let id_exprs: Vec<Expr> =
                            flat_ids.iter().map(gremlin_value_to_expr).collect();
                        Expr::BinaryOp {
                            op: BinaryOp::In,
                            left: Box::new(id_fn),
                            right: Box::new(Expr::List(id_exprs)),
                        }
                    };
                    current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasIdPredicate(pred) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let id_fn = Expr::FunctionCall {
                        name: SmolStr::new("id"),
                        args: vec![Expr::Variable(current_alias.clone())],
                        distinct: false,
                    };
                    let predicate = predicate_to_prop_expr(pred, id_fn);
                    current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Out(labels) | GremlinStep::In(labels) | GremlinStep::Both(labels) => {
                    if self.edge_label_verify && labels.is_empty() {
                        return Err(CompileError::EdgeLabelRequired);
                    }
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let dst = self.next_alias("v");
                    let direction = match step {
                        GremlinStep::Out(_) => Direction::Outgoing,
                        GremlinStep::In(_) => Direction::Incoming,
                        _ => Direction::Undirected,
                    };
                    let rel_types: Vec<RelType> =
                        labels.iter().map(|l| RelType::new(l.as_str())).collect();
                    current = Some(Operator::Expand {
                        direction,
                        rel_types,
                        src_alias: current_alias.clone(),
                        rel_alias: None,
                        dst_alias: dst.clone(),
                        child: Box::new(child),
                    });
                    current_alias = dst;
                    path_aliases.push(current_alias.clone());
                }

                GremlinStep::OutE(labels)
                | GremlinStep::InE(labels)
                | GremlinStep::BothE(labels) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let rel_alias = self.next_alias("e");
                    let dst = self.next_alias("v");
                    let direction = match step {
                        GremlinStep::OutE(_) => Direction::Outgoing,
                        GremlinStep::InE(_) => Direction::Incoming,
                        _ => Direction::Undirected,
                    };
                    let rel_types: Vec<RelType> =
                        labels.iter().map(|l| RelType::new(l.as_str())).collect();
                    current = Some(Operator::Expand {
                        direction,
                        rel_types,
                        src_alias: current_alias.clone(),
                        rel_alias: Some(rel_alias.clone()),
                        dst_alias: dst,
                        child: Box::new(child),
                    });
                    current_alias = rel_alias;
                    path_aliases.push(current_alias.clone());
                }

                GremlinStep::InV | GremlinStep::OutV => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let vertex_alias = self.next_alias("v");
                    let fn_name = match step {
                        GremlinStep::InV => "endNode",
                        _ => "startNode",
                    };
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new(fn_name),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(vertex_alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = vertex_alias;
                    path_aliases.push(current_alias.clone());
                }

                GremlinStep::OtherV => {
                    // otherV: return the vertex that is NOT the traversal source.
                    // We use a runtime function that picks the opposite endpoint.
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let vertex_alias = self.next_alias("v");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_other_v"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(vertex_alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = vertex_alias;
                    path_aliases.push(current_alias.clone());
                }

                GremlinStep::BothV => {
                    // bothV: return BOTH source and destination vertices.
                    // Build a list [startNode, endNode] and unwind.
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let list_alias = self.next_alias("bv");
                    let vertex_alias = self.next_alias("v");
                    let carry = self.label_carry_items();
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_both_v"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(list_alias.clone()),
                    )];
                    items.extend(carry);
                    let project = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    current = Some(Operator::Unwind {
                        list_expr: Expr::Variable(list_alias),
                        alias: vertex_alias.clone(),
                        child: Box::new(project),
                    });
                    current_alias = vertex_alias;
                    path_aliases.push(current_alias.clone());
                }

                GremlinStep::Values(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let carry = self.label_carry_items();
                    if keys.len() == 1 {
                        // Single key: project to a named alias, filter out nulls
                        // (Gremlin values() drops traversers without the property)
                        let alias = self.next_alias("val");
                        let mut items = vec![(
                            Expr::Property(
                                Box::new(Expr::Variable(current_alias.clone())),
                                SmolStr::new(&keys[0]),
                            ),
                            Some(alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(project),
                        });
                        current_alias = alias;
                    } else if keys.is_empty() {
                        // values() with no keys: unwind all property values
                        let alias = self.next_alias("val");
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_values_all"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            },
                            Some(alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        // Unwind the list of all values
                        let uf_alias = self.next_alias("uf");
                        current = Some(Operator::Unwind {
                            list_expr: Expr::Variable(alias),
                            alias: uf_alias.clone(),
                            child: Box::new(project),
                        });
                        current_alias = uf_alias;
                    } else {
                        // Multiple keys: collect values for each key into a list, then unwind
                        let list_alias = self.next_alias("vlist");
                        let mut fn_args = vec![Expr::Variable(current_alias.clone())];
                        for k in keys {
                            fn_args.push(Expr::Str(k.as_str().into()));
                        }
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_values_keys"),
                                args: fn_args,
                                distinct: false,
                            },
                            Some(list_alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        let uf_alias = self.next_alias("uf");
                        current = Some(Operator::Unwind {
                            list_expr: Expr::Variable(list_alias),
                            alias: uf_alias.clone(),
                            child: Box::new(project),
                        });
                        current_alias = uf_alias;
                    }
                }

                GremlinStep::ValueMap(keys, token_mode) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("vmap");
                    let mut args = vec![
                        Expr::Variable(current_alias.clone()),
                        Expr::Integer(i64::from(*token_mode)),
                    ];
                    for k in keys {
                        args.push(Expr::Str(k.as_str().into()));
                    }
                    let fc = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_valuemap"),
                        args,
                        distinct: false,
                    };
                    let mut items = vec![(fc, Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    current_alias = alias;
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    pending_valuemap = true;
                    valuemap_by_consumed = false;
                }

                GremlinStep::Properties(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let prop_alias = self.next_alias("prop");
                    let carry = self.label_carry_items();
                    if keys.is_empty() {
                        // properties() with no keys: unwind all properties as
                        // Map entries with ~key, ~value, ~element fields.
                        let list_alias = self.next_alias("plist");
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_properties_entries"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            },
                            Some(list_alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        current = Some(Operator::Unwind {
                            list_expr: Expr::Variable(list_alias),
                            alias: prop_alias.clone(),
                            child: Box::new(project),
                        });
                    } else if keys.len() == 1 {
                        // Single key: produce a property Map with ~key, ~value, ~element.
                        let list_alias = self.next_alias("plist");
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_properties_entries"),
                                args: vec![
                                    Expr::Variable(current_alias.clone()),
                                    Expr::Str(keys[0].as_str().into()),
                                ],
                                distinct: false,
                            },
                            Some(list_alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        current = Some(Operator::Unwind {
                            list_expr: Expr::Variable(list_alias),
                            alias: prop_alias.clone(),
                            child: Box::new(project),
                        });
                    } else {
                        // Multiple keys: build property entry list for each key and unwind
                        let list_alias = self.next_alias("plist");
                        let mut args = vec![Expr::Variable(current_alias.clone())];
                        for k in keys {
                            args.push(Expr::Str(k.as_str().into()));
                        }
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_properties_entries"),
                                args,
                                distinct: false,
                            },
                            Some(list_alias.clone()),
                        )];
                        items.extend(carry);
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        current = Some(Operator::Unwind {
                            list_expr: Expr::Variable(list_alias),
                            alias: prop_alias.clone(),
                            child: Box::new(project),
                        });
                    }
                    current_alias = prop_alias;
                }

                GremlinStep::Id => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let id_alias = self.next_alias("id");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("id"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(id_alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = id_alias;
                }

                GremlinStep::Label => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let lbl_alias = self.next_alias("lbl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_single_label"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(lbl_alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = lbl_alias;
                }

                GremlinStep::As(alias) => {
                    self.alias_map.insert(alias.clone(), current_alias.clone());
                }

                GremlinStep::Select(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;

                    // Handle Column.keys / Column.values special selects
                    if keys.len() == 1 && (keys[0] == "Column.keys" || keys[0] == "Column.values") {
                        let fn_name = if keys[0] == "Column.keys" {
                            "keys"
                        } else {
                            "values"
                        };
                        let alias = self.next_alias(fn_name);
                        current = Some(Operator::Project {
                            items: vec![(
                                Expr::FunctionCall {
                                    name: SmolStr::new(fn_name),
                                    args: vec![Expr::Variable(current_alias.clone())],
                                    distinct: false,
                                },
                                Some(alias.clone()),
                            )],
                            child: Box::new(child),
                        });
                        current_alias = alias;
                    } else {
                        let select_keys: Vec<(Identifier, String)> = keys
                            .iter()
                            .map(|k| {
                                let var = self
                                    .alias_map
                                    .get(k)
                                    .cloned()
                                    .unwrap_or_else(|| Identifier::new(k.clone()));
                                (var, k.clone())
                            })
                            .collect();
                        // Update current_alias if single key select
                        if keys.len() == 1 {
                            let resolved = self
                                .alias_map
                                .get(&keys[0])
                                .cloned()
                                .unwrap_or_else(|| Identifier::new(keys[0].clone()));
                            current_alias = resolved;
                        } else {
                            // Multi-key: set alias to the Map output column name
                            current_alias = Identifier::new(keys.join("_"));
                        }
                        current = Some(Operator::GremlinSelect {
                            keys: select_keys,
                            productive: self.productive_by,
                            child: Box::new(child),
                        });
                        pending_select = true;
                        select_by_mods.clear();
                    }
                }

                GremlinStep::Count => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let count_alias = self.next_alias("cnt");
                    current = Some(Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Count,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            count_alias.clone(),
                        )],
                        child: Box::new(child),
                    });
                    current_alias = count_alias;
                }

                GremlinStep::Sum => {
                    // Use Fold + SumLocal for Gremlin semantics (empty → empty)
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let fold_alias = self.next_alias("fold");
                    let folded = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Collect,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            fold_alias.clone(),
                        )],
                        child: Box::new(child),
                    };
                    let sum_alias = self.next_alias("sum");
                    let projected = Operator::Project {
                        items: vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_sum_local"),
                                args: vec![Expr::Variable(fold_alias)],
                                distinct: false,
                            },
                            Some(sum_alias.clone()),
                        )],
                        child: Box::new(folded),
                    };
                    // Filter out null (empty list produces null sum) unless
                    // ProductiveByStrategy is active (all-null lists → null).
                    if self.productive_by {
                        current = Some(projected);
                    } else {
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(sum_alias.clone()))),
                            child: Box::new(projected),
                        });
                    }
                    current_alias = sum_alias;
                }

                GremlinStep::Min => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("min");
                    let agg = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Min,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        current = Some(agg);
                    } else {
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(agg),
                        });
                    }
                    current_alias = alias;
                }

                GremlinStep::Max => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("max");
                    let agg = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Max,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        current = Some(agg);
                    } else {
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(agg),
                        });
                    }
                    current_alias = alias;
                }

                GremlinStep::Mean => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("avg");
                    let agg = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Avg,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        current = Some(agg);
                    } else {
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(agg),
                        });
                    }
                    current_alias = alias;
                }

                GremlinStep::SumLocal => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("suml");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_sum_local"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let proj = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        current = Some(proj);
                    } else {
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(proj),
                        });
                    }
                    current_alias = alias;
                }

                GremlinStep::MinLocal => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("minl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_min_local"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let proj = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        current = Some(proj);
                    } else {
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(proj),
                        });
                    }
                    current_alias = alias;
                }

                GremlinStep::MaxLocal => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("maxl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_max_local"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let proj = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        current = Some(proj);
                    } else {
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(proj),
                        });
                    }
                    current_alias = alias;
                }

                GremlinStep::MeanLocal => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("meanl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_mean_local"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    let proj = Operator::Project {
                        items,
                        child: Box::new(child),
                    };
                    if self.productive_by {
                        current = Some(proj);
                    } else {
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(proj),
                        });
                    }
                    current_alias = alias;
                }

                GremlinStep::Group(ref key) | GremlinStep::GroupCount(ref key) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("grp");
                    let is_count = matches!(step, GremlinStep::GroupCount(..));
                    let agg_fn = if is_count {
                        AggFunction::Count
                    } else {
                        AggFunction::Collect
                    };
                    let has_side_effect = key.is_some();
                    // Save the pre-group alias for by() modulation
                    pre_group_alias = current_alias.clone();
                    current = Some(Operator::GremlinGroup {
                        key_expr: Expr::Variable(current_alias.clone()),
                        value_expr: Expr::Variable(current_alias.clone()),
                        agg_fn,
                        alias: alias.clone(),
                        side_effect_key: key.clone(),
                        key_traversal: None,
                        value_traversal: None,
                        productive: self.productive_by,
                        explicit_fold: false,
                        child: Box::new(child),
                    });
                    // Named groups are side-effects (pass-through): keep current alias.
                    // Unnamed groups consume the stream: switch to group alias.
                    if !has_side_effect {
                        current_alias = alias;
                    }
                    pending_group = true;
                    pending_group_by_count = 0;
                    pending_group_is_count = is_count;
                }

                GremlinStep::Fold => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("fold");
                    current = Some(Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Collect,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::FoldSeed(seed, op_name) => {
                    // fold(seed, Operator) — collect then reduce with a function.
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let collect_alias = self.next_alias("fold");
                    let agg = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Collect,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            collect_alias.clone(),
                        )],
                        child: Box::new(child),
                    };
                    let alias = self.next_alias("fred");
                    let op_str = op_name
                        .strip_prefix("Operator.")
                        .unwrap_or(op_name)
                        .to_lowercase();
                    let fc = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_fold_reduce"),
                        args: vec![
                            Expr::Variable(collect_alias),
                            gremlin_value_to_expr(seed),
                            Expr::Str(op_str.into_boxed_str()),
                        ],
                        distinct: false,
                    };
                    let mut items = vec![(fc, Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(agg),
                    });
                    current_alias = alias;
                }

                GremlinStep::Unfold => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("uf");
                    current = Some(Operator::Unwind {
                        list_expr: Expr::Variable(current_alias.clone()),
                        alias: alias.clone(),
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Order => {
                    pending_order = true;
                    // Actual Sort operator created when we see By (or at the end)
                }

                GremlinStep::By(key, descending) => {
                    if pending_order && pending_order_local {
                        // by() after order(Scope.local) — sort within list by key
                        let child = current.take().ok_or(CompileError::MissingChild)?;
                        let alias = self.next_alias("srtl");
                        let by_key = if key.is_empty() { "" } else { key };
                        let desc_str = if *descending { "desc" } else { "asc" };
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_sort_local"),
                                args: vec![
                                    Expr::Variable(current_alias.clone()),
                                    Expr::Str(by_key.to_string().into_boxed_str()),
                                    Expr::Str(desc_str.to_string().into_boxed_str()),
                                ],
                                distinct: false,
                            },
                            Some(alias.clone()),
                        )];
                        items.extend(self.label_carry_items());
                        current = Some(Operator::Project {
                            items,
                            child: Box::new(child),
                        });
                        current_alias = alias;
                        pending_order = false;
                        pending_order_local = false;
                    } else if pending_order {
                        let child = current.take().ok_or(CompileError::MissingChild)?;
                        if key.is_empty() {
                            // by() with no key — sort by current value
                            current = Some(Operator::Sort {
                                items: vec![(Expr::Variable(current_alias.clone()), !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "~id" || key == "T.id" || key == "t[id]" {
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("id"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            };
                            current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "~label" || key == "T.label" || key == "t[label]" {
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("labels"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            };
                            current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "~key" || key == "T.key" || key == "t[key]" {
                            // Sort by property key name (for VertexProperty traversers)
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("gremlin_property_key"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            };
                            current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "~value" || key == "T.value" || key == "t[value]" {
                            // Sort by property value (for VertexProperty traversers)
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("gremlin_property_value"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            };
                            current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "Column.values" {
                            // Sort by map entry values (for unfold'd single-entry Maps)
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("gremlin_column_values"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            };
                            current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else if key == "Column.keys" {
                            // Sort by map entry keys (for unfold'd single-entry Maps)
                            let sort_expr = Expr::FunctionCall {
                                name: SmolStr::new("gremlin_column_keys"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            };
                            current = Some(Operator::Sort {
                                items: vec![(sort_expr, !descending)],
                                gremlin_order: true,
                                child: Box::new(child),
                            });
                        } else {
                            // Property-based sort
                            let prop_expr = Expr::Property(
                                Box::new(Expr::Variable(current_alias.clone())),
                                SmolStr::new(key),
                            );
                            if self.productive_by {
                                // ProductiveByStrategy: use coalesce to return Null
                                // for missing properties instead of filtering.
                                let safe_prop = Expr::FunctionCall {
                                    name: SmolStr::new("coalesce"),
                                    args: vec![prop_expr, Expr::Null],
                                    distinct: false,
                                };
                                current = Some(Operator::Sort {
                                    items: vec![(safe_prop, !descending)],
                                    gremlin_order: true,
                                    child: Box::new(child),
                                });
                            } else {
                                // Filter out elements missing the property
                                let filtered = Operator::Filter {
                                    predicate: Expr::IsNotNull(Box::new(prop_expr.clone())),
                                    child: Box::new(child),
                                };
                                current = Some(Operator::Sort {
                                    items: vec![(prop_expr, !descending)],
                                    gremlin_order: true,
                                    child: Box::new(filtered),
                                });
                            }
                        }
                        pending_order = false;
                    } else if !pending_order && matches!(current, Some(Operator::Sort { .. })) {
                        // Additional by() after a Sort — add secondary sort key
                        let sort_expr = if key.is_empty() {
                            Expr::Variable(current_alias.clone())
                        } else {
                            let prop = Expr::Property(
                                Box::new(Expr::Variable(current_alias.clone())),
                                SmolStr::new(key),
                            );
                            if self.productive_by {
                                Expr::FunctionCall {
                                    name: SmolStr::new("coalesce"),
                                    args: vec![prop, Expr::Null],
                                    distinct: false,
                                }
                            } else {
                                prop
                            }
                        };
                        if let Some(Operator::Sort { ref mut items, .. }) = current {
                            items.push((sort_expr, !descending));
                        }
                    } else if pending_group {
                        // by() after group()/groupCount() — modifies GremlinGroup.
                        // Use pre_group_alias (the alias before group() changed it).
                        if pending_group_by_count == 0 {
                            // First by() = group key
                            if !key.is_empty() {
                                // Try parsing __.prefix strings as sub-traversals
                                if key.starts_with("__.") && key.contains('(') {
                                    let no_param = |_: &str| -> Option<GremlinValue> { None };
                                    if let Ok(sub_steps) =
                                        crate::text_parser::parse_inner_traversal(key, &no_param)
                                    {
                                        if !sub_steps.is_empty() {
                                            let mut sub = self.sub_compiler();
                                            if let Ok(sub_op) =
                                                sub.compile_seeded(&sub_steps, &pre_group_alias)
                                            {
                                                let sub_out_alias = sub.last_alias.clone();
                                                self.absorb(&sub);
                                                if let Some(Operator::GremlinGroup {
                                                    ref mut key_traversal,
                                                    ..
                                                }) = current
                                                {
                                                    *key_traversal =
                                                        Some((Box::new(sub_op), sub_out_alias));
                                                }
                                            }
                                        }
                                    }
                                } else if let Some(Operator::GremlinGroup {
                                    ref mut key_expr,
                                    ..
                                }) = current
                                {
                                    *key_expr = make_by_expr(key, &pre_group_alias);
                                }
                            }
                            pending_group_by_count = 1;
                        } else if pending_group_is_count {
                            // groupCount() only supports one by() modulator
                            return Err(CompileError::InvalidSequence(
                                "GroupCount step can only have one by modulator".into(),
                            ));
                        } else {
                            // Second by() = value aggregation
                            if key == "count()" || key == "__.count()" {
                                if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = current
                                {
                                    *agg_fn = AggFunction::Count;
                                }
                            } else if key == "fold()" || key == "__.fold()" {
                                if let Some(Operator::GremlinGroup {
                                    ref mut agg_fn,
                                    ref mut explicit_fold,
                                    ..
                                }) = current
                                {
                                    *agg_fn = AggFunction::Collect;
                                    *explicit_fold = true;
                                }
                            } else if key == "sum()" || key == "__.sum()" {
                                if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = current
                                {
                                    *agg_fn = AggFunction::Sum;
                                }
                            } else if key.is_empty() {
                                // by() with no args — identity (collect the element)
                                if let Some(Operator::GremlinGroup { ref mut agg_fn, .. }) = current
                                {
                                    *agg_fn = AggFunction::Collect;
                                }
                            } else if key.starts_with("__.") && key.contains('(') {
                                // __.prefix value-by: parse as sub-traversal
                                let no_param = |_: &str| -> Option<GremlinValue> { None };
                                if let Ok(sub_steps) =
                                    crate::text_parser::parse_inner_traversal(key, &no_param)
                                {
                                    if !sub_steps.is_empty() {
                                        let (proj_steps, trailing_agg) =
                                            split_trailing_agg(&sub_steps);
                                        if proj_steps.is_empty() {
                                            if let Some(Operator::GremlinGroup {
                                                ref mut agg_fn,
                                                ref mut explicit_fold,
                                                ..
                                            }) = current
                                            {
                                                if let Some(agg) = trailing_agg {
                                                    if matches!(agg, AggFunction::Collect) {
                                                        *explicit_fold = true;
                                                    }
                                                    *agg_fn = agg;
                                                }
                                            }
                                        } else {
                                            let mut sub = self.sub_compiler();
                                            if let Ok(sub_op) =
                                                sub.compile_seeded(proj_steps, &pre_group_alias)
                                            {
                                                let sub_out_alias = sub.last_alias.clone();
                                                self.absorb(&sub);
                                                if let Some(Operator::GremlinGroup {
                                                    ref mut value_traversal,
                                                    ref mut agg_fn,
                                                    ref mut explicit_fold,
                                                    ..
                                                }) = current
                                                {
                                                    *value_traversal =
                                                        Some((Box::new(sub_op), sub_out_alias));
                                                    if let Some(agg) = trailing_agg {
                                                        if matches!(agg, AggFunction::Collect) {
                                                            *explicit_fold = true;
                                                        }
                                                        *agg_fn = agg;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            } else if let Some(Operator::GremlinGroup {
                                ref mut value_expr, ..
                            }) = current
                            {
                                // by("propName") — collect property values
                                *value_expr = make_by_expr(key, &pre_group_alias);
                            }
                            pending_group = false;
                        }
                    } else if pending_select {
                        // by() after select() — collect modulation
                        select_by_mods.push(key.clone());
                    } else if pending_path {
                        // by() after path() — collect by-key for path element transformation
                        path_by_keys.push(key.to_string());
                    } else if pending_dedup {
                        if dedup_by_consumed {
                            return Err(CompileError::InvalidSequence(
                                "Dedup step can only have one by modulator".into(),
                            ));
                        }
                        // by() after dedup() — dedup by specific property
                        if !key.is_empty() {
                            if let Some(Operator::Distinct { ref mut by_key, .. }) = current {
                                *by_key =
                                    Some(format!("__dedup_prop__{}__{}", key, current_alias.0));
                            }
                        }
                        dedup_by_consumed = true;
                    } else if pending_aggregate {
                        // by() after aggregate() — project a property from each traverser
                        if let Some(Operator::GremlinAggregate {
                            ref mut by_keys, ..
                        }) = current
                        {
                            if !key.is_empty() {
                                by_keys.push(key.to_string());
                            }
                        }
                        pending_aggregate = false;
                        aggregate_by_consumed = true;
                    } else if pending_format {
                        // by(key) after format() — compile by-key to a property access
                        let child = current.take().ok_or(CompileError::MissingChild)?;
                        let by_alias = self.next_alias("fby");
                        let by_expr = if key.is_empty() {
                            Expr::Variable(format_pre_alias.clone())
                        } else {
                            make_by_expr(key, &format_pre_alias)
                        };
                        let mut items = vec![(by_expr, Some(by_alias.clone()))];
                        items.extend(self.label_carry_items());
                        current = Some(Operator::Project {
                            items,
                            child: Box::new(child),
                        });
                        format_by_aliases.push(by_alias);
                    } else if pending_sack_op {
                        // by(key) after sack(op) — rewrite the sack Project to
                        // use a property/label expression instead of current_alias.
                        if !key.is_empty() {
                            let by_expr = make_by_expr(key, &current_alias);
                            if let Some(Operator::Project { ref mut items, .. }) = current {
                                // First item is the sack assignment; rewrite the rhs
                                // (current_alias reference) with the by expression.
                                if let Some((expr, _)) = items.first_mut() {
                                    rewrite_sack_by(expr, &current_alias, &by_expr);
                                }
                            }
                            // Filter out traversers where the by-key property is null
                            // (Gremlin drops traversers when by() can't resolve a value).
                            let sack_var = Identifier::new("__sack");
                            if let Some(proj) = current.take() {
                                current = Some(Operator::Filter {
                                    predicate: Expr::IsNotNull(Box::new(Expr::Variable(sack_var))),
                                    child: Box::new(proj),
                                });
                            }
                        }
                        pending_sack_op = false;
                        pending_sack_by_consumed = true;
                    } else if pending_project {
                        // by() after project() — replace the default property
                        // expression for the next project key with the by() expression.
                        if project_by_count < project_keys.len() {
                            // Check if the key looks like a sub-traversal
                            let looks_like_traversal = key.contains('(') && !key.is_empty();
                            if looks_like_traversal {
                                // Parse and compile the sub-traversal
                                let no_param = |_: &str| -> Option<GremlinValue> { None };
                                if let Ok(sub_steps) =
                                    crate::text_parser::parse_inner_traversal(key, &no_param)
                                {
                                    if !sub_steps.is_empty() {
                                        let mut sub = self.sub_compiler();
                                        if let Ok(sub_op) =
                                            sub.compile_seeded(&sub_steps, &pre_project_alias)
                                        {
                                            let sub_out_alias = sub.last_alias.clone();
                                            self.absorb(&sub);

                                            // Convert to GremlinProject
                                            match &current {
                                                Some(Operator::Project { items, child }) => {
                                                    let mut entries: Vec<(
                                                        String,
                                                        Operator,
                                                        Identifier,
                                                    )> = Vec::new();
                                                    for (i, (expr, alias_opt)) in
                                                        items.iter().enumerate()
                                                    {
                                                        if i >= project_keys.len() {
                                                            break;
                                                        }
                                                        if i == project_by_count {
                                                            entries.push((
                                                                project_keys[i].clone(),
                                                                sub_op.clone(),
                                                                sub_out_alias.clone(),
                                                            ));
                                                        } else {
                                                            let prop_op = Operator::Project {
                                                                items: vec![(
                                                                    expr.clone(),
                                                                    alias_opt.clone(),
                                                                )],
                                                                child: Box::new(
                                                                    Operator::SingleRow,
                                                                ),
                                                            };
                                                            entries.push((
                                                                project_keys[i].clone(),
                                                                prop_op,
                                                                alias_opt
                                                                    .as_ref()
                                                                    .cloned()
                                                                    .unwrap_or_else(|| {
                                                                        Identifier::new(
                                                                            &project_keys[i],
                                                                        )
                                                                    }),
                                                            ));
                                                        }
                                                    }
                                                    current = Some(Operator::GremlinProject {
                                                        entries,
                                                        child: child.clone(),
                                                        current_alias: pre_project_alias.clone(),
                                                    });
                                                }
                                                Some(Operator::GremlinProject {
                                                    entries: old_entries,
                                                    child,
                                                    current_alias: ca,
                                                }) => {
                                                    let mut entries = old_entries.clone();
                                                    if project_by_count < entries.len() {
                                                        entries[project_by_count] = (
                                                            project_keys[project_by_count].clone(),
                                                            sub_op,
                                                            sub_out_alias,
                                                        );
                                                    }
                                                    current = Some(Operator::GremlinProject {
                                                        entries,
                                                        child: child.clone(),
                                                        current_alias: ca.clone(),
                                                    });
                                                }
                                                _ => {}
                                            }
                                            project_by_count += 1;
                                            if project_by_count >= project_keys.len() {
                                                pending_project = false;
                                            }
                                            // Skip the default handling below
                                        } else {
                                            // Compilation failed — fall back to expression
                                            let by_expr = make_by_expr(key, &pre_project_alias);
                                            if let Some(Operator::Project {
                                                ref mut items, ..
                                            }) = current
                                            {
                                                if let Some((expr, _)) =
                                                    items.get_mut(project_by_count)
                                                {
                                                    *expr = by_expr;
                                                }
                                            }
                                            project_by_count += 1;
                                            if project_by_count >= project_keys.len() {
                                                pending_project = false;
                                            }
                                        }
                                    } else {
                                        // Empty steps — fall back
                                        let by_expr = make_by_expr(key, &pre_project_alias);
                                        if let Some(Operator::Project { ref mut items, .. }) =
                                            current
                                        {
                                            if let Some((expr, _)) = items.get_mut(project_by_count)
                                            {
                                                *expr = by_expr;
                                            }
                                        }
                                        project_by_count += 1;
                                        if project_by_count >= project_keys.len() {
                                            pending_project = false;
                                        }
                                    }
                                } else {
                                    // Parse failed — fall back to expression
                                    let by_expr = make_by_expr(key, &pre_project_alias);
                                    if let Some(Operator::Project { ref mut items, .. }) = current {
                                        if let Some((expr, _)) = items.get_mut(project_by_count) {
                                            *expr = by_expr;
                                        }
                                    }
                                    project_by_count += 1;
                                    if project_by_count >= project_keys.len() {
                                        pending_project = false;
                                    }
                                }
                            } else {
                                // Simple property key
                                let by_expr = if key.is_empty() {
                                    Expr::Variable(pre_project_alias.clone())
                                } else {
                                    make_by_expr(key, &pre_project_alias)
                                };
                                if let Some(Operator::Project { ref mut items, .. }) = current {
                                    if let Some((expr, _)) = items.get_mut(project_by_count) {
                                        *expr = by_expr;
                                    }
                                } else if let Some(Operator::GremlinProject {
                                    ref mut entries,
                                    ..
                                }) = current
                                {
                                    // Already converted to GremlinProject — update the entry
                                    if project_by_count < entries.len() {
                                        let by_ex = if key.is_empty() {
                                            Expr::Variable(pre_project_alias.clone())
                                        } else {
                                            make_by_expr(key, &pre_project_alias)
                                        };
                                        let prop_op = Operator::Project {
                                            items: vec![(
                                                by_ex,
                                                Some(Identifier::new(
                                                    &project_keys[project_by_count],
                                                )),
                                            )],
                                            child: Box::new(Operator::SingleRow),
                                        };
                                        entries[project_by_count] = (
                                            project_keys[project_by_count].clone(),
                                            prop_op,
                                            Identifier::new(&project_keys[project_by_count]),
                                        );
                                    }
                                }
                                project_by_count += 1;
                                if project_by_count >= project_keys.len() {
                                    pending_project = false;
                                }
                            }
                        }
                    } else if pending_where_by.is_some() {
                        // by() after where("a", P.gt("b")) — rewrite filter predicate
                        // to use property access: where("a", P.gt("b")).by("age")
                        // becomes filter(a.age > b.age) instead of filter(a > b).
                        if let Some((left_id, right_id)) = pending_where_by.take() {
                            if !key.is_empty() {
                                if let Some(Operator::Filter { ref mut predicate, .. }) = current {
                                    rewrite_where_by_predicate(predicate, &left_id, &right_id, key);
                                }
                            }
                        }
                    } else if aggregate_by_consumed {
                        // Second by() after aggregate — error per Gremlin spec
                        return Err(CompileError::InvalidSequence(
                            "Aggregate step can only have one by modulator".into(),
                        ));
                    } else if pending_sample && sample_by_consumed {
                        return Err(CompileError::InvalidSequence(
                            "Sample step can only have one by modulator".into(),
                        ));
                    } else if pending_sack_by_consumed {
                        return Err(CompileError::InvalidSequence(
                            "Sack step can only have one by modulator".into(),
                        ));
                    } else if pending_valuemap && valuemap_by_consumed {
                        return Err(CompileError::InvalidSequence(
                            "step can only have one by modulator".into(),
                        ));
                    }
                    // Track by() consumption for sample/sack/valueMap
                    if pending_sample && !sample_by_consumed {
                        sample_by_consumed = true;
                    }
                    if pending_sack_op && !pending_sack_by_consumed {
                        pending_sack_by_consumed = true;
                    }
                    if pending_valuemap && !valuemap_by_consumed {
                        valuemap_by_consumed = true;
                    }
                }

                GremlinStep::ByTraversal(sub_steps, _descending) => {
                    // by(traversal) after valueMap — unfold list values
                    if pending_valuemap && !valuemap_by_consumed {
                        let is_unfold =
                            sub_steps.len() == 1 && matches!(sub_steps[0], GremlinStep::Unfold);
                        if is_unfold {
                            // Wrap the current valueMap result to unfold lists
                            let child = current.take().ok_or(CompileError::MissingChild)?;
                            let alias = self.next_alias("vmuf");
                            let mut items = vec![(
                                Expr::FunctionCall {
                                    name: SmolStr::new("gremlin_valuemap_unfold"),
                                    args: vec![Expr::Variable(current_alias.clone())],
                                    distinct: false,
                                },
                                Some(alias.clone()),
                            )];
                            items.extend(self.label_carry_items());
                            current = Some(Operator::Project {
                                items,
                                child: Box::new(child),
                            });
                            current_alias = alias;
                        }
                        valuemap_by_consumed = true;
                    } else if pending_valuemap && valuemap_by_consumed {
                        return Err(CompileError::InvalidSequence(
                            "step can only have one by modulator".into(),
                        ));
                    } else if pending_project {
                        // by(traversal) after project() — compile the sub-traversal
                        // and convert to GremlinProject if not already done.
                        if project_by_count < project_keys.len() {
                            // Compile the sub-traversal seeded with the pre-project alias
                            let mut sub = self.sub_compiler();
                            let sub_op = sub.compile_seeded(sub_steps, &pre_project_alias)?;
                            let sub_out_alias = sub.last_alias.clone();
                            self.absorb(&sub);

                            // Convert Operator::Project → GremlinProject on first ByTraversal
                            match &current {
                                Some(Operator::Project { items, child }) => {
                                    // Build GremlinProject entries from existing items
                                    let mut entries: Vec<(String, Operator, Identifier)> =
                                        Vec::new();
                                    for (i, (_expr, alias_opt)) in items.iter().enumerate() {
                                        if i < project_keys.len() {
                                            if i == project_by_count {
                                                // This is the by(traversal) entry
                                                entries.push((
                                                    project_keys[i].clone(),
                                                    sub_op.clone(),
                                                    sub_out_alias.clone(),
                                                ));
                                            } else if i < project_by_count {
                                                // Previously filled by By() — create identity project
                                                let prop_op = Operator::Project {
                                                    items: vec![(
                                                        items[i].0.clone(),
                                                        alias_opt.clone(),
                                                    )],
                                                    child: Box::new(Operator::SingleRow),
                                                };
                                                entries.push((
                                                    project_keys[i].clone(),
                                                    prop_op,
                                                    alias_opt.as_ref().cloned().unwrap_or_else(
                                                        || Identifier::new(&project_keys[i]),
                                                    ),
                                                ));
                                            } else {
                                                // Not yet filled — default to property access
                                                let prop_op = Operator::Project {
                                                    items: vec![(
                                                        items[i].0.clone(),
                                                        alias_opt.clone(),
                                                    )],
                                                    child: Box::new(Operator::SingleRow),
                                                };
                                                entries.push((
                                                    project_keys[i].clone(),
                                                    prop_op,
                                                    alias_opt.as_ref().cloned().unwrap_or_else(
                                                        || Identifier::new(&project_keys[i]),
                                                    ),
                                                ));
                                            }
                                        }
                                    }
                                    current = Some(Operator::GremlinProject {
                                        entries,
                                        child: child.clone(),
                                        current_alias: pre_project_alias.clone(),
                                    });
                                }
                                Some(Operator::GremlinProject {
                                    entries: old_entries,
                                    child,
                                    current_alias: ca,
                                }) => {
                                    // Already a GremlinProject — update the current entry
                                    let mut entries = old_entries.clone();
                                    if project_by_count < entries.len() {
                                        entries[project_by_count] = (
                                            project_keys[project_by_count].clone(),
                                            sub_op,
                                            sub_out_alias,
                                        );
                                    }
                                    current = Some(Operator::GremlinProject {
                                        entries,
                                        child: child.clone(),
                                        current_alias: ca.clone(),
                                    });
                                }
                                _ => {}
                            }
                            project_by_count += 1;
                            if project_by_count >= project_keys.len() {
                                pending_project = false;
                            }
                        }
                    }
                    // Non-project contexts: delegate ByTraversal to the same
                    // behavior as By by reconstructing the original key string.
                    if !pending_project {
                        if pending_order {
                            // by(traversal) after order() — compile sub-traversal for sort
                            let child = current.take().ok_or(CompileError::MissingChild)?;
                            let mut sub = self.sub_compiler();
                            let sub_op = sub.compile_seeded(sub_steps, &current_alias)?;
                            let sub_out_alias = sub.last_alias.clone();
                            self.absorb(&sub);
                            let applied = Operator::Apply {
                                outer: Box::new(child),
                                inner: Box::new(sub_op),
                            };
                            current = Some(Operator::Sort {
                                items: vec![(Expr::Variable(sub_out_alias), !_descending)],
                                gremlin_order: true,
                                child: Box::new(applied),
                            });
                            pending_order = false;
                        } else if pending_select {
                            // by(traversal) after select() — treat as string key for select_by_mods
                            // (preserves existing behavior)
                            select_by_mods.push(String::new());
                        } else if pending_path {
                            // by(traversal) after path() — not yet supported as
                            // sub-traversal, treat as identity
                            path_by_keys.push(String::new());
                        } else if pending_dedup {
                            // by(traversal) after dedup() — compile sub-traversal for dedup key
                            let child = current.take().ok_or(CompileError::MissingChild)?;
                            let mut sub = self.sub_compiler();
                            let sub_op = sub.compile_seeded(sub_steps, &current_alias)?;
                            let sub_out_alias = sub.last_alias.clone();
                            self.absorb(&sub);
                            let applied = Operator::Apply {
                                outer: Box::new(child),
                                inner: Box::new(sub_op),
                            };
                            current = Some(Operator::Distinct {
                                child: Box::new(applied),
                                by_key: Some(sub_out_alias.to_string()),
                            });
                            pending_dedup = false;
                        } else if pending_group {
                            // by(traversal) after group() — compile sub-traversal for key or value
                            if pending_group_by_count == 0 {
                                // First by(traversal) = group key
                                let mut sub = self.sub_compiler();
                                let sub_op = sub.compile_seeded(sub_steps, &pre_group_alias)?;
                                let sub_out_alias = sub.last_alias.clone();
                                self.absorb(&sub);
                                if let Some(Operator::GremlinGroup {
                                    ref mut key_traversal,
                                    ..
                                }) = current
                                {
                                    *key_traversal = Some((Box::new(sub_op), sub_out_alias));
                                }
                                pending_group_by_count = 1;
                            } else {
                                // Second by(traversal) = value aggregation
                                // Check for trailing agg step (count/sum/fold)
                                let (proj_steps, trailing_agg) = split_trailing_agg(sub_steps);

                                if proj_steps.is_empty() {
                                    if let Some(Operator::GremlinGroup {
                                        ref mut agg_fn,
                                        ref mut explicit_fold,
                                        ..
                                    }) = current
                                    {
                                        if let Some(agg) = trailing_agg {
                                            if matches!(agg, AggFunction::Collect) {
                                                *explicit_fold = true;
                                            }
                                            *agg_fn = agg;
                                        }
                                    }
                                } else {
                                    let mut sub = self.sub_compiler();
                                    let sub_op =
                                        sub.compile_seeded(proj_steps, &pre_group_alias)?;
                                    let sub_out_alias = sub.last_alias.clone();
                                    self.absorb(&sub);
                                    if let Some(Operator::GremlinGroup {
                                        ref mut value_traversal,
                                        ref mut agg_fn,
                                        ref mut explicit_fold,
                                        ..
                                    }) = current
                                    {
                                        *value_traversal = Some((Box::new(sub_op), sub_out_alias));
                                        if let Some(agg) = trailing_agg {
                                            if matches!(agg, AggFunction::Collect) {
                                                *explicit_fold = true;
                                            }
                                            *agg_fn = agg;
                                        }
                                    }
                                }
                                pending_group = false;
                            }
                        } else if pending_aggregate {
                            // by(traversal) after aggregate() — compile sub-traversal
                            let mut sub = self.sub_compiler();
                            let sub_op = sub.compile_seeded(sub_steps, &current_alias)?;
                            let sub_out_alias = sub.last_alias.clone();
                            self.absorb(&sub);
                            if let Some(Operator::GremlinAggregate {
                                ref mut by_traversal,
                                ..
                            }) = current
                            {
                                *by_traversal = Some((Box::new(sub_op), sub_out_alias));
                            }
                            pending_aggregate = false;
                            aggregate_by_consumed = true;
                        } else if pending_format {
                            // by(traversal) after format() — compile sub-traversal for by-modulation
                            let child = current.take().ok_or(CompileError::MissingChild)?;
                            let mut sub = self.sub_compiler();
                            let sub_op = sub.compile_seeded(sub_steps, &format_pre_alias)?;
                            let sub_out_alias = sub.last_alias.clone();
                            self.absorb(&sub);
                            // Limit to 1 result per input row
                            let limited = Operator::Limit {
                                count: Expr::Integer(1),
                                child: Box::new(sub_op),
                            };
                            current = Some(Operator::Apply {
                                outer: Box::new(child),
                                inner: Box::new(limited),
                            });
                            format_by_aliases.push(sub_out_alias);
                        } else if pending_sack_op && !pending_sack_by_consumed {
                            // by(traversal) after sack(op) — rewrite sack assignment
                            // to use the sub-traversal result instead of current_alias.
                            let mut sub = self.sub_compiler();
                            let sub_op = sub.compile_seeded(sub_steps, &current_alias)?;
                            let sub_out_alias = sub.last_alias.clone();
                            self.absorb(&sub);
                            let by_expr = Expr::Variable(sub_out_alias);
                            // Wrap current operator in Apply so the sub-traversal runs
                            // per-row, then rewrite the sack Project's rhs.
                            if let Some(ref mut proj) = current {
                                // First, wrap child in Apply with the sub-traversal
                                let sack_var = Identifier::new("__sack");
                                if let Operator::Project {
                                    ref mut items,
                                    ref mut child,
                                } = proj
                                {
                                    **child = Operator::Apply {
                                        outer: child.clone(),
                                        inner: Box::new(sub_op),
                                    };
                                    if let Some((expr, _)) = items.first_mut() {
                                        rewrite_sack_by(expr, &current_alias, &by_expr);
                                    }
                                }
                                // Filter out rows where __sack is null
                                let proj_taken = std::mem::replace(proj, Operator::SingleRow);
                                *proj = Operator::Filter {
                                    predicate: Expr::IsNotNull(Box::new(Expr::Variable(sack_var))),
                                    child: Box::new(proj_taken),
                                };
                            }
                            pending_sack_op = false;
                            pending_sack_by_consumed = true;
                        }
                    }
                }

                GremlinStep::Limit(n) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Limit {
                        count: Expr::Integer(*n),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Range(low, high) => {
                    if *high < *low {
                        return Err(CompileError::InvalidSequence(format!(
                            "range({}, {}): high must be >= low",
                            low, high
                        )));
                    }
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let skipped = if *low > 0 {
                        Operator::Skip {
                            count: Expr::Integer(*low),
                            child: Box::new(child),
                        }
                    } else {
                        child
                    };
                    current = Some(Operator::Limit {
                        count: Expr::Integer(*high - *low),
                        child: Box::new(skipped),
                    });
                }

                GremlinStep::LimitLocal(n) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::GremlinMapSlice {
                        alias: current_alias.clone(),
                        lo: 0,
                        hi: *n as usize,
                        child: Box::new(child),
                    });
                }

                GremlinStep::RangeLocal(low, high) => {
                    // Flush pending order(Scope.local) so the sort is applied
                    // before slicing.
                    if pending_order && pending_order_local {
                        if let Some(child) = current.take() {
                            let alias = self.next_alias("srtl");
                            let mut items = vec![(
                                Expr::FunctionCall {
                                    name: SmolStr::new("gremlin_sort_local"),
                                    args: vec![Expr::Variable(current_alias.clone())],
                                    distinct: false,
                                },
                                Some(alias.clone()),
                            )];
                            items.extend(self.label_carry_items());
                            current = Some(Operator::Project {
                                items,
                                child: Box::new(child),
                            });
                            current_alias = alias;
                        }
                        pending_order = false;
                        pending_order_local = false;
                    }
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::GremlinMapSlice {
                        alias: current_alias.clone(),
                        lo: *low as usize,
                        hi: *high as usize,
                        child: Box::new(child),
                    });
                }

                GremlinStep::SkipLocal(n) => {
                    // Flush pending order(Scope.local) so the sort is applied
                    // before skipping.
                    if pending_order && pending_order_local {
                        if let Some(child) = current.take() {
                            let alias = self.next_alias("srtl");
                            let mut items = vec![(
                                Expr::FunctionCall {
                                    name: SmolStr::new("gremlin_sort_local"),
                                    args: vec![Expr::Variable(current_alias.clone())],
                                    distinct: false,
                                },
                                Some(alias.clone()),
                            )];
                            items.extend(self.label_carry_items());
                            current = Some(Operator::Project {
                                items,
                                child: Box::new(child),
                            });
                            current_alias = alias;
                        }
                        pending_order = false;
                        pending_order_local = false;
                    }
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::GremlinMapSlice {
                        alias: current_alias.clone(),
                        lo: *n as usize,
                        hi: usize::MAX,
                        child: Box::new(child),
                    });
                }

                GremlinStep::SortLocal => {
                    // Sort elements within a list/map value.
                    // Defer to by() handler; if no by() follows, resolve at end.
                    pending_order = true;
                    pending_order_local = true;
                }

                GremlinStep::Dedup(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    // dedup() — deduplicate by the current traverser value
                    // dedup("a","b") — deduplicate by values at step labels
                    let by_key = if keys.is_empty() {
                        Some(current_alias.to_string())
                    } else if keys.len() == 1 {
                        Some(keys[0].clone())
                    } else {
                        // Multi-key dedup: compose a key from all labels
                        // Handled by passing a composite key hint
                        Some(keys.join("\x1f"))
                    };
                    current = Some(Operator::Distinct {
                        child: Box::new(child),
                        by_key,
                    });
                    pending_dedup = true;
                }

                GremlinStep::Where(inner_steps) | GremlinStep::Not(inner_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let is_not = matches!(step, GremlinStep::Not(_));

                    // Use predicate mode only when ALL inner steps are simple
                    // predicate-convertible steps. Otherwise use traversal mode
                    // (sub-traversal existence check via Apply + Count + Filter).
                    let can_be_predicate = !inner_steps.is_empty()
                        && inner_steps.iter().all(|s| {
                            matches!(
                                s,
                                GremlinStep::Is(_)
                                    | GremlinStep::Has(_, _)
                                    | GremlinStep::HasPredicate(_, _)
                                    | GremlinStep::HasLabel(_)
                                    | GremlinStep::Values(_)
                                    | GremlinStep::Label
                                    | GremlinStep::Id
                                    | GremlinStep::Select(_)
                            )
                        });

                    if !can_be_predicate {
                        // Compile inner steps as a sub-traversal seeded with
                        // the current alias. Use Apply + Count + Filter to
                        // implement existence semantics.
                        // If the sub-traversal starts with as("label"), seed
                        // from that label's existing alias instead.
                        let mut sub = self.sub_compiler();
                        let (seed_alias, steps_slice) = match inner_steps.first() {
                            Some(GremlinStep::As(label)) if self.alias_map.contains_key(label) => {
                                (self.alias_map[label].clone(), &inner_steps[1..])
                            }
                            _ => (current_alias.clone(), &inner_steps[..]),
                        };
                        let inner_op = sub.compile_seeded(steps_slice, &seed_alias)?;
                        self.absorb(&sub);
                        let sub_result_alias = sub.last_alias.clone();

                        // Count the sub-traversal results
                        let cnt_alias = self.next_alias("wcnt");
                        let count_op = Operator::Aggregate {
                            keys: vec![],
                            aggregations: vec![(
                                gql_core::planner::AggFunction::Count,
                                Expr::Variable(sub_result_alias),
                                None,
                                false,
                                cnt_alias.clone(),
                            )],
                            child: Box::new(inner_op),
                        };

                        // Wrap in Apply so each input row seeds the sub-traversal
                        let apply = Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(count_op),
                        };

                        // Filter: keep rows where count > 0 (or == 0 for Not)
                        let predicate = if is_not {
                            Expr::BinaryOp {
                                op: BinaryOp::Eq,
                                left: Box::new(Expr::Variable(cnt_alias)),
                                right: Box::new(Expr::Integer(0)),
                            }
                        } else {
                            Expr::BinaryOp {
                                op: BinaryOp::Gt,
                                left: Box::new(Expr::Variable(cnt_alias)),
                                right: Box::new(Expr::Integer(0)),
                            }
                        };
                        current = Some(Operator::Filter {
                            predicate,
                            child: Box::new(apply),
                        });
                    } else {
                        let inner_pred =
                            steps_to_predicate(inner_steps, &current_alias, &self.alias_map);
                        let predicate = if is_not {
                            // Wrap in COALESCE so that Null (from cross-type
                            // comparison) becomes false before negation. This
                            // ensures not(has("name", P.gt(27))) correctly
                            // keeps traversers when the comparison is undefined.
                            let coalesced = Expr::FunctionCall {
                                name: SmolStr::new("coalesce"),
                                args: vec![inner_pred, Expr::Bool(false)],
                                distinct: false,
                            };
                            Expr::UnaryOp {
                                op: gql_core::ast::UnaryOp::Not,
                                expr: Box::new(coalesced),
                            }
                        } else {
                            inner_pred
                        };
                        // Track label pair for by() modulation on where("a", P.gt("b")).by("age")
                        if !is_not {
                            if let Some((left_id, right_id)) =
                                extract_where_label_pair(inner_steps, &self.alias_map, &current_alias)
                            {
                                pending_where_by = Some((left_id, right_id));
                            }
                        }
                        current = Some(Operator::Filter {
                            predicate,
                            child: Box::new(child),
                        });
                    }
                }

                GremlinStep::And(branches) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    if branches.is_empty() {
                        current = Some(child);
                    } else {
                        current = Some(self.compile_connective_branches(
                            branches,
                            &current_alias,
                            child,
                            BinaryOp::And,
                        )?);
                    }
                }

                GremlinStep::Or(branches) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    if branches.is_empty() {
                        current = Some(child);
                    } else {
                        current = Some(self.compile_connective_branches(
                            branches,
                            &current_alias,
                            child,
                            BinaryOp::Or,
                        )?);
                    }
                }

                GremlinStep::Is(predicate) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let pred_expr = predicate_to_expr(predicate, &current_alias);
                    current = Some(Operator::Filter {
                        predicate: pred_expr,
                        child: Box::new(child),
                    });
                }

                GremlinStep::AddV(label) => {
                    if self.read_only {
                        return Err(CompileError::ReadOnlyViolation);
                    }
                    pending_properties.clear();
                    let alias = self.next_alias("nv");
                    current_alias = alias.clone();
                    let pattern = CreatePattern {
                        nodes: vec![(
                            Some(alias),
                            vec![Label::new(label.as_str())],
                            vec![], // properties added later
                        )],
                        edges: vec![],
                    };
                    current = Some(Operator::Create {
                        patterns: vec![pattern],
                        child: current.map(Box::new),
                    });
                }

                GremlinStep::AddE(label) => {
                    if self.read_only {
                        return Err(CompileError::ReadOnlyViolation);
                    }
                    pending_properties.clear();
                    adde_traverser = Some(current_alias.clone());
                    let rel_alias = self.next_alias("ne");
                    current_alias = rel_alias.clone();
                    // Placeholder: src/dst to be set by From/To steps
                    let src = Identifier::new("__src");
                    let dst = Identifier::new("__dst");
                    let edge_spec: CreateEdgeSpec = (
                        Some(rel_alias),
                        src,
                        dst,
                        RelType::new(label.as_str()),
                        Direction::Outgoing,
                        vec![],
                    );
                    let pattern = CreatePattern {
                        nodes: vec![],
                        edges: vec![edge_spec],
                    };
                    current = Some(Operator::Create {
                        patterns: vec![pattern],
                        child: current.map(Box::new),
                    });
                }

                GremlinStep::Property(key, value) => {
                    if self.read_only {
                        return Err(CompileError::ReadOnlyViolation);
                    }
                    // Helper: extract the select key from "__.select("x")" patterns
                    let extract_select_key = |s: &str| -> Option<String> {
                        let trimmed = s.trim_start_matches("__.");
                        trimmed
                            .strip_prefix("select(")
                            .and_then(|s| s.strip_suffix(')'))
                            .map(|inner| inner.trim_matches('"').trim_matches('\'').to_string())
                    };
                    // Resolve property key: if __.select("a") and "a" is a string
                    // side-effect, use the string value as the property key name.
                    let resolved_key = if let Some(ref sel_key) = extract_select_key(key) {
                        if let Some(GremlinValue::String(s)) =
                            self.side_effect_values.get(sel_key.as_str())
                        {
                            s.clone()
                        } else {
                            key.clone()
                        }
                    } else {
                        key.clone()
                    };
                    if let Some(ref reserved) = self.reserved_keys {
                        let default_reserved = ["id", "label"];
                        if default_reserved.contains(&resolved_key.as_str())
                            || reserved.iter().any(|r| *r == resolved_key)
                        {
                            return Err(CompileError::ReservedKeyViolation(resolved_key));
                        }
                    }
                    pending_properties.push((resolved_key.clone(), value.clone()));
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    // Resolve property value: if __.select("a"), emit Variable ref
                    let val_expr = if let GremlinValue::String(s) = value {
                        if let Some(ref sel_key) = extract_select_key(s) {
                            if let Some(alias) = self.alias_map.get(sel_key.as_str()) {
                                Expr::Variable(alias.clone())
                            } else {
                                gremlin_value_to_expr(value)
                            }
                        } else {
                            gremlin_value_to_expr(value)
                        }
                    } else {
                        gremlin_value_to_expr(value)
                    };
                    current = Some(Operator::Set {
                        items: vec![SetItem::Property {
                            alias: current_alias.clone(),
                            key: resolved_key,
                            value: val_expr,
                        }],
                        child: Box::new(child),
                    });
                }

                GremlinStep::From(_) | GremlinStep::To(_) => {
                    // Edge endpoint modifiers: resolve the vertex reference
                    // and rewrite the preceding AddE's CreateEdgeSpec.
                    let inner_steps = match step {
                        GremlinStep::From(s) | GremlinStep::To(s) => s.as_slice(),
                        _ => unreachable!(),
                    };
                    let is_from = matches!(step, GremlinStep::From(_));

                    // Resolve the vertex reference from the inner steps
                    let resolved_alias = if inner_steps.len() == 1 {
                        match &inner_steps[0] {
                            GremlinStep::V(Some(ids)) if ids.len() == 1 => {
                                // from(V(id)) or to(V(id)) — compile a node lookup
                                let alias = self.next_alias("fv");
                                let scan = Operator::Filter {
                                    predicate: Expr::BinaryOp {
                                        op: BinaryOp::Eq,
                                        left: Box::new(Expr::FunctionCall {
                                            name: SmolStr::new("id"),
                                            args: vec![Expr::Variable(alias.clone())],
                                            distinct: false,
                                        }),
                                        right: Box::new(gremlin_value_to_expr(&ids[0])),
                                    },
                                    child: Box::new(Operator::NodeScan {
                                        label: None,
                                        alias: alias.clone(),
                                    }),
                                };
                                let child = current.take().ok_or(CompileError::MissingChild)?;
                                // Inject the vertex lookup as a cross join into the
                                // Create's child chain so that the fv_N alias is
                                // present in every row when Create resolves src/dst.
                                current = Some(inject_scan_into_child(child, scan));
                                alias
                            }
                            GremlinStep::Select(keys) if keys.len() == 1 => {
                                // from(select("a")) or to(select("a"))
                                self.alias_map
                                    .get(&keys[0])
                                    .cloned()
                                    .unwrap_or_else(|| Identifier::new(keys[0].clone()))
                            }
                            _ => {
                                // Single-step complex traversal
                                let mut sub = self.sub_compiler();
                                if let Ok(inner_op) =
                                    sub.compile_seeded(inner_steps, &current_alias)
                                {
                                    self.absorb(&sub);
                                    let inner_alias = sub.last_alias.clone();
                                    let child = current.take().ok_or(CompileError::MissingChild)?;
                                    current = Some(inject_scan_into_child(child, inner_op));
                                    inner_alias
                                } else {
                                    current_alias.clone()
                                }
                            }
                        }
                    } else {
                        // Multi-step traversal in from/to
                        let mut sub = self.sub_compiler();
                        if let Ok(inner_op) = sub.compile_seeded(inner_steps, &current_alias) {
                            self.absorb(&sub);
                            let inner_alias = sub.last_alias.clone();
                            let child = current.take().ok_or(CompileError::MissingChild)?;
                            current = Some(inject_scan_into_child(child, inner_op));
                            inner_alias
                        } else {
                            current_alias.clone()
                        }
                    };

                    // Rewrite the __src/__dst placeholder in the Create operator
                    let placeholder = if is_from { "__src" } else { "__dst" };
                    if let Some(ref mut op) = current {
                        rewrite_create_alias(op, placeholder, &resolved_alias);
                    }
                }

                GremlinStep::Drop => {
                    if self.read_only {
                        return Err(CompileError::ReadOnlyViolation);
                    }
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Delete {
                        exprs: vec![Expr::Variable(current_alias.clone())],
                        detach: true,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Path => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::GremlinPath {
                        child: Box::new(child),
                        path_aliases: path_aliases.clone(),
                        by_keys: vec![],
                    });
                    current_alias = Identifier::new("path");
                    pending_path = true;
                    path_by_keys.clear();
                }

                GremlinStep::Repeat(body_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let seed_alias = current_alias.clone();
                    let mut sub = self.sub_compiler();
                    let body = sub.compile_seeded(body_steps, &current_alias)?;
                    let body_out_alias = sub.last_alias.clone();
                    self.absorb(&sub);
                    // Apply any pending emit/until that appeared before the repeat
                    let emit_val = pending_emit;
                    pending_emit = false;
                    let until_val = if let Some(until_steps) = pending_until.take() {
                        let mut sub2 = self.sub_compiler();
                        let until_op = sub2.compile_seeded(&until_steps, &current_alias)?;
                        self.absorb(&sub2);
                        Some(Box::new(until_op))
                    } else {
                        None
                    };
                    // If body produces a different alias, remap it back to the seed
                    let remap = if body_out_alias.as_str() != seed_alias.as_str() {
                        Some((
                            body_out_alias.as_str().to_string(),
                            seed_alias.as_str().to_string(),
                        ))
                    } else {
                        None
                    };
                    let times_val = pending_times.take();
                    let times_before_val = times_val.is_some();
                    current = Some(Operator::Repeat {
                        body: Box::new(body),
                        times: times_val,
                        until: until_val,
                        emit: emit_val,
                        emit_before: emit_val,
                        emit_predicate: if let Some(ep_steps) = pending_emit_pred.take() {
                            let mut sub3 = self.sub_compiler();
                            let ep_op = sub3.compile_seeded(&ep_steps, &current_alias)?;
                            self.absorb(&sub3);
                            Some(Box::new(ep_op))
                        } else {
                            None
                        },
                        remap,
                        times_before: times_before_val,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Times(n) => {
                    // Modify preceding Repeat
                    if let Some(Operator::Repeat {
                        ref mut times,
                        ref mut times_before,
                        ..
                    }) = current
                    {
                        *times = Some(*n as usize);
                        *times_before = false; // times() after repeat() → do-while
                    } else {
                        // times() before repeat() — store as pending
                        pending_times = Some(*n as usize);
                    }
                }

                GremlinStep::Until(until_steps) => {
                    if let Some(Operator::Repeat { ref mut until, .. }) = current {
                        // Modify preceding Repeat
                        let mut sub = self.sub_compiler();
                        let until_op = sub.compile_seeded(until_steps, &current_alias)?;
                        self.absorb(&sub);
                        *until = Some(Box::new(until_op));
                    } else {
                        // until() before repeat() — store as pending
                        pending_until = Some(until_steps.clone());
                    }
                }

                GremlinStep::Emit => {
                    if let Some(Operator::Repeat {
                        ref mut emit,
                        ref mut emit_before,
                        ..
                    }) = current
                    {
                        *emit = true;
                        // emit() after repeat() → post-emit (don't include starting traversers)
                        *emit_before = false;
                    } else {
                        // emit() before repeat() — store as pending
                        pending_emit = true;
                    }
                }

                GremlinStep::EmitPred(pred_steps) => {
                    if let Some(Operator::Repeat {
                        ref mut emit,
                        ref mut emit_before,
                        ref mut emit_predicate,
                        ..
                    }) = current
                    {
                        *emit = true;
                        *emit_before = false;
                        let mut sub = self.sub_compiler();
                        let ep_op = sub.compile_seeded(pred_steps, &current_alias)?;
                        self.absorb(&sub);
                        *emit_predicate = Some(Box::new(ep_op));
                    } else {
                        // emit(pred) before repeat() — store as pending
                        pending_emit = true;
                        pending_emit_pred = Some(pred_steps.clone());
                    }
                }

                GremlinStep::Choose(pred_steps, true_steps, false_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    if true_steps.is_empty() && false_steps.is_none() {
                        // 1-arg choose: value-based dispatch, options come from Option steps
                        let selector = sub.compile_seeded(pred_steps, &current_alias)?;
                        let sel_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        let choose_alias = self.next_alias("chm");
                        current = Some(Operator::ChooseMap {
                            selector: Box::new(selector),
                            selector_alias: sel_alias,
                            options: Vec::new(),
                            child: Box::new(child),
                            output_alias: Some(choose_alias.clone()),
                        });
                        // Defer alias switch until after all option() steps
                        pending_choose_output = Some(choose_alias);
                    } else {
                        let pred_op = sub.compile_seeded(pred_steps, &current_alias)?;
                        let true_op = sub.compile_seeded(true_steps, &current_alias)?;
                        let t_alias = sub.last_alias.clone();
                        let false_op = match false_steps {
                            Some(steps) => {
                                Some(Box::new(sub.compile_seeded(steps, &current_alias)?))
                            }
                            None => None,
                        };
                        // For false branch: use the sub-compiler's alias if a false
                        // branch exists, otherwise use the seed alias (current_alias
                        // before choose) for pass-through rows.
                        let f_alias = if false_steps.is_some() {
                            sub.last_alias.clone()
                        } else {
                            current_alias.clone()
                        };
                        self.absorb(&sub);

                        // Create a unified output alias for choose branches.
                        let choose_alias = self.next_alias("ch");
                        current_alias = choose_alias.clone();

                        current = Some(Operator::Choose {
                            predicate: Box::new(pred_op),
                            if_true: Box::new(true_op),
                            if_false: false_op,
                            child: Box::new(child),
                            true_alias: Some(t_alias),
                            false_alias: Some(f_alias),
                            output_alias: Some(choose_alias.clone()),
                        });
                    }
                }

                GremlinStep::Coalesce(alternatives) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    // Compile each alternative and track their output aliases.
                    let mut alt_ops = Vec::new();
                    let mut alt_aliases = Vec::new();
                    for s in alternatives {
                        let mut sub = self.sub_compiler();
                        let op = sub.compile_seeded(s, &current_alias)?;
                        alt_aliases.push(sub.last_alias.clone());
                        self.absorb(&sub);
                        alt_ops.push(op);
                    }

                    // Normalize: wrap each alternative in a Project to rename
                    // its output to a common alias.
                    let coal_alias = self.next_alias("coal");
                    let normalized: Vec<Operator> = alt_ops
                        .into_iter()
                        .zip(alt_aliases.iter())
                        .map(|(op, alt_alias)| {
                            if alt_alias.as_str() == coal_alias.as_str() {
                                op
                            } else {
                                Operator::Project {
                                    items: vec![
                                        (
                                            Expr::Variable(alt_alias.clone()),
                                            Some(coal_alias.clone()),
                                        ),
                                        // Also carry forward the original alias for path()
                                        (
                                            Expr::Variable(alt_alias.clone()),
                                            Some(alt_alias.clone()),
                                        ),
                                    ],
                                    child: Box::new(op),
                                }
                            }
                        })
                        .collect();

                    current = Some(Operator::Coalesce {
                        alternatives: normalized,
                        child: Box::new(child),
                    });
                    current_alias = coal_alias;
                    path_aliases.push(current_alias.clone());
                }

                GremlinStep::Union(branches) => {
                    let child = current.take().unwrap_or(Operator::SingleRow);
                    if branches.is_empty() {
                        // union() with no branches → produce no results
                        current = Some(Operator::Filter {
                            predicate: Expr::Bool(false),
                            child: Box::new(child),
                        });
                    } else {
                        // Compile each branch and track its result alias.
                        let mut branch_ops: Vec<Operator> = Vec::new();
                        let mut branch_aliases: Vec<Identifier> = Vec::new();
                        for s in branches {
                            let mut sub = self.sub_compiler();
                            let op = sub.compile_seeded(s, &current_alias)?;
                            branch_aliases.push(sub.last_alias.clone());
                            self.absorb(&sub);
                            branch_ops.push(op);
                        }

                        // Choose a common output alias for the union.
                        let union_alias = self.next_alias("u");

                        // Wrap each branch in a Project to normalize the output alias.
                        let normalized: Vec<Operator> = branch_ops
                            .into_iter()
                            .zip(branch_aliases.iter())
                            .map(|(op, src_alias)| {
                                if src_alias == &union_alias {
                                    op
                                } else {
                                    let mut items = vec![(
                                        Expr::Variable(src_alias.clone()),
                                        Some(union_alias.clone()),
                                    )];
                                    // Carry forward all step labels
                                    items.extend(self.label_carry_items());
                                    Operator::Project {
                                        items,
                                        child: Box::new(op),
                                    }
                                }
                            })
                            .collect();

                        // Build a left-associative union chain
                        let union = normalized
                            .into_iter()
                            .reduce(|left, right| Operator::Union {
                                left: Box::new(left),
                                right: Box::new(right),
                                all: true,
                            })
                            .unwrap_or(Operator::SingleRow);
                        // Wrap union in Apply so parent variables (v_1, etc.)
                        // are available to each branch via outer_rows propagation.
                        current = Some(Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(union),
                        });
                        current_alias = union_alias;
                    }
                }

                GremlinStep::SideEffect(effect_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let effect = sub.compile_seeded(effect_steps, &current_alias)?;
                    self.absorb(&sub);
                    current = Some(Operator::GremlinSideEffect {
                        effect: Box::new(effect),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Aggregate(key) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::GremlinAggregate {
                        key: key.clone(),
                        child: Box::new(child),
                        value_alias: current_alias.clone(),
                        by_keys: vec![],
                        by_traversal: None,
                        productive: self.productive_by,
                    });
                    pending_aggregate = true;
                }

                GremlinStep::Cap(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    // Collect reduction metadata from compiler state
                    let mut reductions: Vec<(String, String, f64)> = Vec::new();
                    for k in keys {
                        if let Some((op, init)) = self.reductions.get(k) {
                            reductions.push((k.clone(), op.clone(), *init));
                        }
                    }
                    current = Some(Operator::Cap {
                        keys: keys.clone(),
                        child: Box::new(child),
                        reductions,
                    });
                    // After cap(), the result row uses cap key names as columns.
                    // Update current_alias so subsequent steps reference the right column.
                    if keys.len() == 1 {
                        current_alias = Identifier::new(keys[0].clone());
                    }
                }

                GremlinStep::Constant(val) => {
                    let child = current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("const");
                    let mut items = vec![(gremlin_value_to_expr(val), Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Inject(values) => {
                    if values.is_empty() {
                        // inject() with no args produces an empty stream
                        if current.is_none() {
                            current = Some(Operator::Filter {
                                predicate: Expr::Bool(false),
                                child: Box::new(Operator::SingleRow),
                            });
                        }
                    } else {
                        let exprs: Vec<Expr> = values.iter().map(gremlin_value_to_expr).collect();
                        let alias = self.next_alias("inj");
                        let injected = Operator::Unwind {
                            list_expr: Expr::List(exprs),
                            alias: alias.clone(),
                            child: Box::new(Operator::SingleRow),
                        };
                        if let Some(child) = current.take() {
                            // Mid-traversal: UNION child stream with injected values,
                            // then project to a common alias.
                            let prev_alias = current_alias.clone();
                            let union_alias = self.next_alias("inj");
                            let left = Operator::Project {
                                items: vec![(
                                    Expr::Variable(prev_alias),
                                    Some(union_alias.clone()),
                                )],
                                child: Box::new(child),
                            };
                            let right = Operator::Project {
                                items: vec![(Expr::Variable(alias), Some(union_alias.clone()))],
                                child: Box::new(injected),
                            };
                            current = Some(Operator::Union {
                                left: Box::new(left),
                                right: Box::new(right),
                                all: true,
                            });
                            current_alias = union_alias;
                        } else {
                            // Source step: just unwind
                            current = Some(injected);
                            current_alias = alias;
                        }
                    }
                }

                GremlinStep::Match(patterns) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut compiled_patterns = Vec::with_capacity(patterns.len());
                    let mut all_alias_pairs: Vec<(String, Identifier)> = Vec::new();
                    // Track whether the input traverser has been bound to a
                    // label. Only the first new label gets the input; subsequent
                    // new labels enumerate via V(None).
                    let mut input_bound = false;
                    for pattern_steps in patterns {
                        let mut sub = self.sub_compiler();

                        // Check if the pattern starts with As("X").
                        let first_as_label = pattern_steps.first().and_then(|s| {
                            if let GremlinStep::As(label) = s {
                                Some(label.clone())
                            } else {
                                None
                            }
                        });

                        let compiled = if let Some(ref label) = first_as_label {
                            if let Some(existing_alias) = sub.alias_map.get(label).cloned() {
                                // Label already bound — use existing alias as seed.
                                let remaining = &pattern_steps[1..];
                                sub.compile_with_seed(remaining, existing_alias)?
                            } else if !input_bound {
                                // First new label — bind to the current traverser.
                                input_bound = true;
                                sub.alias_map.insert(label.clone(), current_alias.clone());
                                sub.compile_with_seed(&pattern_steps[1..], current_alias.clone())?
                            } else {
                                // Subsequent new label — create an explicit
                                // NodeScan with a known alias so the label maps
                                // to the correct column.
                                let scan_alias = sub.next_alias("v");
                                sub.alias_map.insert(label.clone(), scan_alias.clone());
                                let scan = Operator::NodeScan {
                                    label: None,
                                    alias: scan_alias.clone(),
                                };
                                sub.compile_from(
                                    &pattern_steps[1..],
                                    Some(scan),
                                    scan_alias.clone(),
                                    vec![scan_alias],
                                )?
                            }
                        } else {
                            // No leading as() — seed from the current traverser.
                            sub.compile_with_seed(pattern_steps, current_alias.clone())?
                        };

                        for (k, v) in &sub.alias_map {
                            self.alias_map.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                        // Collect ALL alias→label pairs from each pattern
                        // (different patterns may use different aliases for the
                        // same label).
                        for (label, alias) in &sub.alias_map {
                            all_alias_pairs.push((label.clone(), alias.clone()));
                        }
                        self.absorb(&sub);
                        compiled_patterns.push(compiled);
                    }
                    // Build the label→alias mapping with ALL aliases.
                    let label_map: Vec<(String, Identifier)> = all_alias_pairs;
                    let match_result_alias = self.next_alias("match");
                    current = Some(Operator::GremlinMatch {
                        patterns: compiled_patterns,
                        label_map,
                        result_alias: match_result_alias.clone(),
                        child: Box::new(child),
                    });
                    current_alias = match_result_alias;
                }

                GremlinStep::Project(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    pre_project_alias = current_alias.clone();
                    let mut items: Vec<(Expr, Option<Identifier>)> = keys
                        .iter()
                        .map(|k| {
                            (
                                Expr::Property(
                                    Box::new(Expr::Variable(current_alias.clone())),
                                    SmolStr::new(k),
                                ),
                                Some(Identifier::new(k.clone())),
                            )
                        })
                        .collect();
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    pending_project = true;
                    project_keys = keys.clone();
                    project_by_count = 0;
                }

                GremlinStep::Identity => {
                    // No-op: identity() passes through the current traverser unchanged.
                    if current.is_none() {
                        return Err(CompileError::MissingChild);
                    }
                }

                GremlinStep::Element => {
                    // element() — extract ~element from property Map objects.
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let elem_alias = self.next_alias("elem");
                    let mut items = vec![(
                        Expr::Property(
                            Box::new(Expr::Variable(current_alias.clone())),
                            SmolStr::new("~element"),
                        ),
                        Some(elem_alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(elem_alias.clone()))),
                        child: Box::new(Operator::Project {
                            items,
                            child: Box::new(child),
                        }),
                    });
                    current_alias = elem_alias;
                }

                GremlinStep::Index | GremlinStep::IndexWithMap => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let out_alias = self.next_alias("idx");
                    let with_map = matches!(step, GremlinStep::IndexWithMap);
                    current = Some(Operator::GremlinIndex {
                        child: Box::new(child),
                        alias: current_alias.clone(),
                        out_alias: out_alias.clone(),
                        with_map,
                    });
                    current_alias = out_alias;
                }

                GremlinStep::Tail(n) => {
                    // tail(n): reverse → limit(n) → reverse
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let reversed = Operator::Sort {
                        items: vec![(Expr::Variable(current_alias.clone()), false)],
                        gremlin_order: true,
                        child: Box::new(child),
                    };
                    let limited = Operator::Limit {
                        count: Expr::Integer(*n),
                        child: Box::new(reversed),
                    };
                    current = Some(Operator::Sort {
                        items: vec![(Expr::Variable(current_alias.clone()), true)],
                        gremlin_order: true,
                        child: Box::new(limited),
                    });
                }

                GremlinStep::TailLocal(n) => {
                    // Flush pending order(Scope.local) so the sort is applied
                    // before taking the tail.
                    if pending_order && pending_order_local {
                        if let Some(child) = current.take() {
                            let alias = self.next_alias("srtl");
                            let mut items = vec![(
                                Expr::FunctionCall {
                                    name: SmolStr::new("gremlin_sort_local"),
                                    args: vec![Expr::Variable(current_alias.clone())],
                                    distinct: false,
                                },
                                Some(alias.clone()),
                            )];
                            items.extend(self.label_carry_items());
                            current = Some(Operator::Project {
                                items,
                                child: Box::new(child),
                            });
                            current_alias = alias;
                        }
                        pending_order = false;
                        pending_order_local = false;
                    }
                    // tail(Scope.local, n): take last n elements of list values
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("tail");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_tail_local"),
                            args: vec![Expr::Variable(current_alias.clone()), Expr::Integer(*n)],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Skip(n) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Skip {
                        count: Expr::Integer(*n),
                        child: Box::new(child),
                    });
                }

                GremlinStep::ElementMap(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("emap");
                    // elementMap as a function call that returns a Map value
                    let mut args = vec![Expr::Variable(current_alias.clone())];
                    for k in keys {
                        args.push(Expr::Str(k.as_str().into()));
                    }
                    let fc = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_elementmap"),
                        args,
                        distinct: false,
                    };
                    let mut items = vec![(fc, Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    current_alias = alias;
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasKey(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    // After properties(), current alias is a property Map with ~key.
                    // Check if ~key matches any of the given keys.
                    let predicates: Vec<Expr> = keys
                        .iter()
                        .map(|k| {
                            // Check both: the element has the property (node/edge context)
                            // AND the property Map ~key matches (properties() context).
                            // Use ~key field for property Map objects.
                            Expr::BinaryOp {
                                op: BinaryOp::Eq,
                                left: Box::new(Expr::Property(
                                    Box::new(Expr::Variable(current_alias.clone())),
                                    SmolStr::new("~key"),
                                )),
                                right: Box::new(Expr::Str(k.as_str().into())),
                            }
                        })
                        .collect();
                    // Any key matches (OR), or if checking on element, all must exist (AND).
                    // In Gremlin, hasKey on properties filters for any of the keys.
                    let predicate = if keys.len() == 1 {
                        // Single key: also support element context
                        Expr::BinaryOp {
                            op: BinaryOp::Or,
                            left: Box::new(predicates[0].clone()),
                            right: Box::new(Expr::IsNotNull(Box::new(Expr::Property(
                                Box::new(Expr::Variable(current_alias.clone())),
                                SmolStr::new(&keys[0]),
                            )))),
                        }
                    } else {
                        predicates
                            .into_iter()
                            .reduce(|a, b| Expr::BinaryOp {
                                op: BinaryOp::Or,
                                left: Box::new(a),
                                right: Box::new(b),
                            })
                            .unwrap_or(Expr::Bool(true))
                    };
                    current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasValue(values) => {
                    // hasValue filters by ~value (property context) or current value.
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    // Try ~value for property Map context, fall back to current alias.
                    let tilde_val = Expr::Property(
                        Box::new(Expr::Variable(current_alias.clone())),
                        SmolStr::new("~value"),
                    );
                    if values.len() == 1 {
                        // Check ~value == val OR current == val
                        let val_expr = gremlin_value_to_expr(&values[0]);
                        let pred_tilde = Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(tilde_val),
                            right: Box::new(val_expr.clone()),
                        };
                        let pred_direct = Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(Expr::Variable(current_alias.clone())),
                            right: Box::new(val_expr),
                        };
                        current = Some(Operator::Filter {
                            predicate: Expr::BinaryOp {
                                op: BinaryOp::Or,
                                left: Box::new(pred_tilde),
                                right: Box::new(pred_direct),
                            },
                            child: Box::new(child),
                        });
                    } else if !values.is_empty() {
                        // Filter null values out, then check ~value IN [non-null values]
                        // OR ~value IS NULL if null is in the list.
                        let non_null: Vec<Expr> = values
                            .iter()
                            .filter(|v| !matches!(v, GremlinValue::Null))
                            .map(gremlin_value_to_expr)
                            .collect();
                        let pred_in = if !non_null.is_empty() {
                            Some(Expr::BinaryOp {
                                op: BinaryOp::In,
                                left: Box::new(tilde_val.clone()),
                                right: Box::new(Expr::List(non_null)),
                            })
                        } else {
                            None
                        };
                        let predicate = match pred_in {
                            Some(pred) => pred,
                            None => Expr::Bool(false),
                        };
                        current = Some(Operator::Filter {
                            predicate,
                            child: Box::new(child),
                        });
                    } else {
                        current = Some(child);
                    }
                }

                GremlinStep::HasValuePredicate(pred) => {
                    // hasValue(P.gt(30)) — filter on ~value (property context) or current value.
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let tilde_val = Expr::Property(
                        Box::new(Expr::Variable(current_alias.clone())),
                        SmolStr::new("~value"),
                    );
                    let predicate = predicate_to_prop_expr(pred, tilde_val);
                    current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Optional(opt_steps) => {
                    // optional(traversal): coalesce(traversal, identity)
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let opt_op = sub.compile_seeded(opt_steps, &current_alias)?;
                    self.absorb(&sub);
                    current_alias = sub.last_alias.clone();
                    current = Some(Operator::Coalesce {
                        alternatives: vec![opt_op, Operator::SingleRow],
                        child: Box::new(child),
                    });
                }

                GremlinStep::SimplePath => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::GremlinSimplePath {
                        child: Box::new(child),
                        path_aliases: path_aliases.clone(),
                    });
                }

                GremlinStep::CyclicPath => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::GremlinCyclicPath {
                        child: Box::new(child),
                        path_aliases: path_aliases.clone(),
                    });
                }

                GremlinStep::Coin(probability) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let rand_expr = if let Some(seed) = self.seed {
                        // Seeded: use Java-compatible LCG for deterministic results
                        Expr::FunctionCall {
                            name: SmolStr::new("java_random_next_double"),
                            args: vec![Expr::Integer(seed)],
                            distinct: false,
                        }
                    } else {
                        Expr::FunctionCall {
                            name: SmolStr::new("rand"),
                            args: vec![],
                            distinct: false,
                        }
                    };
                    current = Some(Operator::Filter {
                        predicate: Expr::BinaryOp {
                            op: BinaryOp::Lt,
                            left: Box::new(rand_expr),
                            right: Box::new(Expr::Float(*probability)),
                        },
                        child: Box::new(child),
                    });
                }

                GremlinStep::Map(map_steps) => {
                    // map(traversal) — apply sub-traversal to each traverser
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let map_op = sub.compile_seeded(map_steps, &current_alias)?;
                    self.absorb(&sub);
                    current_alias = sub.last_alias.clone();
                    current = Some(Operator::Apply {
                        outer: Box::new(child),
                        inner: Box::new(map_op),
                    });
                }

                GremlinStep::FlatMap(fm_steps) => {
                    // flatMap(traversal) — apply and flatten
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let fm_op = sub.compile_seeded(fm_steps, &current_alias)?;
                    self.absorb(&sub);
                    current_alias = sub.last_alias.clone();
                    current = Some(Operator::Apply {
                        outer: Box::new(child),
                        inner: Box::new(fm_op),
                    });
                }

                GremlinStep::FilterTraversal(filter_steps) => {
                    // filter(traversal) — keep traversers where sub-traversal is true
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    // Try to extract a predicate from the inner steps
                    let pred = steps_to_predicate(filter_steps, &current_alias, &self.alias_map);
                    // If steps_to_predicate returned true (couldn't convert) and
                    // there are actual steps, fall back to sub-traversal existence
                    // check via Apply + Count + Filter (same pattern as Where).
                    if pred == Expr::Bool(true) && !filter_steps.is_empty() {
                        let mut sub = self.sub_compiler();
                        let inner_op = sub.compile_seeded(filter_steps, &current_alias)?;
                        self.absorb(&sub);
                        let sub_result_alias = sub.last_alias.clone();
                        let cnt_alias = self.next_alias("fcnt");
                        let count_op = Operator::Aggregate {
                            keys: vec![],
                            aggregations: vec![(
                                gql_core::planner::AggFunction::Count,
                                Expr::Variable(sub_result_alias),
                                None,
                                false,
                                cnt_alias.clone(),
                            )],
                            child: Box::new(inner_op),
                        };
                        let apply = Operator::Apply {
                            outer: Box::new(child),
                            inner: Box::new(count_op),
                        };
                        current = Some(Operator::Filter {
                            predicate: Expr::BinaryOp {
                                op: BinaryOp::Gt,
                                left: Box::new(Expr::Variable(cnt_alias)),
                                right: Box::new(Expr::Integer(0)),
                            },
                            child: Box::new(apply),
                        });
                    } else {
                        current = Some(Operator::Filter {
                            predicate: pred,
                            child: Box::new(child),
                        });
                    }
                }

                GremlinStep::None => {
                    // none() — produces no results (empty)
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Filter {
                        predicate: Expr::Bool(false),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Barrier => {
                    // barrier() — synchronization, no-op in our execution model
                    if current.is_none() {
                        return Err(CompileError::MissingChild);
                    }
                }

                GremlinStep::Sample(n) => {
                    // sample(n) — approximated as limit(n) for deterministic tests
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Limit {
                        count: Expr::Integer(*n),
                        child: Box::new(child),
                    });
                    pending_sample = true;
                    sample_by_consumed = false;
                }

                GremlinStep::SampleLocal(n) => {
                    // sample(Scope.local, n) — approximated as limit(local, n)
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::GremlinMapSlice {
                        alias: current_alias.clone(),
                        lo: 0,
                        hi: *n as usize,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Discard => {
                    // discard() — consume traversal, return empty
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Filter {
                        predicate: Expr::Bool(false),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Loops => {
                    // loops() — returns loop count; approximated as constant 0
                    // outside repeat context
                    let child = current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("loops");
                    let mut items = vec![(Expr::Integer(0), Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Sack => {
                    // sack() — return the sack value from the __sack column
                    let child = current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("sack");
                    let sack_var = Identifier::new("__sack");
                    let mut items = vec![(Expr::Variable(sack_var), Some(alias.clone()))];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::SackOp(op) => {
                    // sack(operator) — apply: __sack = __sack OP current_value
                    // Must preserve current_alias and all label columns.
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let sack_var = Identifier::new("__sack");
                    let bin_op = match op.as_str() {
                        "sum" | "add" => BinaryOp::Add,
                        "minus" | "sub" => BinaryOp::Sub,
                        "mult" | "multiply" => BinaryOp::Mul,
                        "div" | "divide" => BinaryOp::Div,
                        "assign" => {
                            let mut items = vec![
                                (Expr::Variable(current_alias.clone()), Some(sack_var)),
                                (
                                    Expr::Variable(current_alias.clone()),
                                    Some(current_alias.clone()),
                                ),
                            ];
                            items.extend(self.label_carry_items());
                            current = Some(Operator::Project {
                                items,
                                child: Box::new(child),
                            });
                            pending_sack_op = true;
                            continue;
                        }
                        _ => BinaryOp::Add,
                    };
                    let mut items = vec![
                        (
                            Expr::BinaryOp {
                                op: bin_op,
                                left: Box::new(Expr::Variable(sack_var.clone())),
                                right: Box::new(Expr::Variable(current_alias.clone())),
                            },
                            Some(sack_var),
                        ),
                        (
                            Expr::Variable(current_alias.clone()),
                            Some(current_alias.clone()),
                        ),
                    ];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    pending_sack_op = true;
                }

                GremlinStep::WithSack(val) => {
                    // withSack(value) — initialize __sack on all traversers
                    // This should be the first step; apply to whichever op follows
                    // We'll inject it as a Project that adds __sack to each row
                    pending_sack_init = Some(gremlin_value_to_expr(val));
                }

                GremlinStep::WithSideEffect(key, val, ref reduce_op) => {
                    // withSideEffect(key, value[, operator]) — register a named side-effect
                    let se_alias = Identifier::new(format!("__side_effect_{}", key));
                    // Collect carry items BEFORE inserting the new alias to avoid
                    // a self-referencing carry item that would try to read the
                    // column we're about to create.
                    let carry = self.label_carry_items();
                    self.alias_map.insert(key.clone(), se_alias.clone());
                    // Store the literal value for compile-time key resolution
                    self.side_effect_values.insert(key.clone(), val.clone());
                    // For reduce operators, store in compiler state for Cap to use.
                    // Still inject the initial value as a column so downstream
                    // select/where can resolve it (e.g., where(within('a'))).
                    if let Some(op) = reduce_op {
                        if op != "assign" {
                            let init_val = match val {
                                GremlinValue::Integer(n) => *n as f64,
                                GremlinValue::Float(f) => *f,
                                GremlinValue::Bool(b) => {
                                    if *b {
                                        1.0
                                    } else {
                                        0.0
                                    }
                                }
                                _ => 0.0,
                            };
                            self.reductions.insert(key.clone(), (op.clone(), init_val));
                        }
                    }
                    // Always inject the side-effect value as a column so
                    // downstream select/where/property can resolve it.
                    let child = current.take().unwrap_or(Operator::SingleRow);
                    let val_expr = gremlin_value_to_expr(val);
                    let mut items = vec![(val_expr, Some(se_alias))];
                    items.extend(carry);
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                }

                GremlinStep::ProductiveByStrategy => {
                    self.productive_by = true;
                }

                GremlinStep::SeedStrategy(seed) => {
                    self.seed = Some(*seed);
                }

                GremlinStep::ReadOnlyStrategy => {
                    self.read_only = true;
                }

                GremlinStep::ReservedKeysVerificationStrategy(keys) => {
                    self.reserved_keys = Some(keys.clone());
                }

                GremlinStep::EdgeLabelVerificationStrategy => {
                    self.edge_label_verify = true;
                }

                GremlinStep::DedupLocal(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("dedup_local");
                    let mut args = vec![Expr::Variable(current_alias.clone())];
                    for k in keys {
                        args.push(Expr::Str(k.clone().into_boxed_str()));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_dedup_local"),
                            args,
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Option(token, branch_steps) => {
                    if let Some(Operator::ChooseMap {
                        ref mut options, ..
                    }) = current
                    {
                        let mut sub = self.sub_compiler();
                        let branch_op = sub.compile_seeded(branch_steps, &current_alias)?;
                        let branch_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        let match_val = match token {
                            GremlinValue::String(s) if s == "none" => None,
                            _ => Some(gremlin_value_to_value(token)),
                        };
                        options.push((match_val, Box::new(branch_op), Some(branch_alias)));
                    } else if current.is_none() {
                        return Err(CompileError::MissingChild);
                    }
                    // If current is not ChooseMap, option is ignored (no-op).
                }

                // ── String steps ─────────────────────────────────────────────────
                GremlinStep::ToLower => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_toLower"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::ToUpper => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_toUpper"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Trim => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_trim"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::LTrim => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_lTrim"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::RTrim => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_rTrim"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Length => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("len");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_length"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Reverse => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("rev");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_reverse"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Replace(old, new) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_replace"),
                            args: vec![
                                Expr::Variable(current_alias.clone()),
                                Expr::Str(old.clone().into_boxed_str()),
                                Expr::Str(new.clone().into_boxed_str()),
                            ],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Split(sep) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("split");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_split"),
                            args: vec![
                                Expr::Variable(current_alias.clone()),
                                Expr::Str(sep.clone().into_boxed_str()),
                            ],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Substring(start, len) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("substr");
                    let mut args =
                        vec![Expr::Variable(current_alias.clone()), Expr::Integer(*start)];
                    if let Some(l) = len {
                        args.push(Expr::Integer(*l));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_substring"),
                            args,
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::SubstringLocal(start, len) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("substrl");
                    let mut args =
                        vec![Expr::Variable(current_alias.clone()), Expr::Integer(*start)];
                    if let Some(l) = len {
                        args.push(Expr::Integer(*l));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_substring_local"),
                            args,
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Concat(traversals) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("concat");
                    // Build concat args: current value + each sub-traversal result
                    let mut args = vec![Expr::Variable(current_alias.clone())];
                    let mut base = child;
                    for trav in traversals {
                        if trav.is_empty() {
                            // Empty traversal → empty string
                            args.push(Expr::Str("".into()));
                            continue;
                        }
                        // Single constant → inline
                        if trav.len() == 1 {
                            if let GremlinStep::Constant(v) = &trav[0] {
                                args.push(gremlin_value_to_expr(v));
                                continue;
                            }
                        }
                        // Sub-traversal: compile and wrap in Apply.
                        // Limit inner to 1 result per outer row (Gremlin concat
                        // uses only the first value from inner traversals).
                        let mut sub = self.sub_compiler();
                        let sub_op = sub.compile_seeded(trav, &current_alias)?;
                        let sub_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        let limited = Operator::Limit {
                            count: Expr::Integer(1),
                            child: Box::new(sub_op),
                        };
                        base = Operator::Apply {
                            outer: Box::new(base),
                            inner: Box::new(limited),
                        };
                        args.push(Expr::Variable(sub_alias));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_concat"),
                            args,
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    current_alias = alias;
                }

                GremlinStep::Conjoin(sep) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("join");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_conjoin"),
                            args: vec![
                                Expr::Variable(current_alias.clone()),
                                Expr::Str(sep.clone().into_boxed_str()),
                            ],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Format(pattern) => {
                    if pattern.contains("%{_}") {
                        // Pattern has positional placeholders — defer until by() modulations collected
                        let child = current.take().ok_or(CompileError::MissingChild)?;
                        pending_format = true;
                        format_pattern = pattern.clone();
                        format_pre_alias = current_alias.clone();
                        format_by_aliases.clear();
                        current = Some(child);
                    } else {
                        let child = current.take().ok_or(CompileError::MissingChild)?;
                        let alias = self.next_alias("fmt");
                        // Build a map of as-label values for %{label} resolution
                        let label_pairs: Vec<(SmolStr, Expr)> = self
                            .alias_map
                            .iter()
                            .map(|(label, ident)| {
                                (SmolStr::new(label), Expr::Variable(ident.clone()))
                            })
                            .collect();
                        let mut args = vec![
                            Expr::Variable(current_alias.clone()),
                            Expr::Str(pattern.clone().into_boxed_str()),
                        ];
                        if !label_pairs.is_empty() {
                            args.push(Expr::MapLiteral(label_pairs));
                        }
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("gremlin_format"),
                                args,
                                distinct: false,
                            },
                            Some(alias.clone()),
                        )];
                        items.extend(self.label_carry_items());
                        let project = Operator::Project {
                            items,
                            child: Box::new(child),
                        };
                        // Filter out traversers where format couldn't resolve all placeholders
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                            child: Box::new(project),
                        });
                        current_alias = alias;
                    }
                }

                GremlinStep::StringOpLocal(fn_name) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("sloc");
                    // Parse function name and optional extra args from \0-separated string
                    let parts: Vec<&str> = fn_name.split('\0').collect();
                    let func_name = parts[0];
                    let mut args = vec![Expr::Variable(current_alias.clone())];
                    for extra in &parts[1..] {
                        args.push(Expr::Str(extra.to_string().into_boxed_str()));
                    }
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new(func_name),
                            args,
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::AsString => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("str");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asString"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::AsStringLocal => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("strl");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asstring_local"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::AsNumber => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("num");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asnumber"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::AsNumberTyped(ref target_type) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("num");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asnumber_typed"),
                            args: vec![
                                Expr::Variable(current_alias.clone()),
                                Expr::Str(target_type.clone().into_boxed_str()),
                            ],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::AsBool => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("bool");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_asBool"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Math(expr_str) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("math");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_math"),
                            args: vec![
                                Expr::Variable(current_alias.clone()),
                                Expr::Str(expr_str.clone().into_boxed_str()),
                            ],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::TypeOf => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("type");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_typeOf"),
                            args: vec![Expr::Variable(current_alias.clone())],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                // ── Collection steps ─────────────────────────────────────────────
                GremlinStep::Combine(rhs_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_combine"),
                            args: vec![Expr::Variable(current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    current_alias = alias;
                }

                GremlinStep::Intersect(rhs_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_intersect"),
                            args: vec![Expr::Variable(current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    current_alias = alias;
                }

                GremlinStep::Difference(rhs_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_difference"),
                            args: vec![Expr::Variable(current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    current_alias = alias;
                }

                GremlinStep::Disjunct(rhs_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_disjunct"),
                            args: vec![Expr::Variable(current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    current_alias = alias;
                }

                GremlinStep::MergeList(rhs_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_merge"),
                            args: vec![Expr::Variable(current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    current_alias = alias;
                }

                GremlinStep::Product(rhs_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("coll");
                    let (rhs_expr, base) =
                        self.compile_collection_rhs(rhs_steps, &current_alias, child)?;
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_product"),
                            args: vec![Expr::Variable(current_alias.clone()), rhs_expr],
                            distinct: false,
                        },
                        Some(alias.clone()),
                    )];
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(base),
                    });
                    current_alias = alias;
                }

                GremlinStep::All(pred) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
                    let pred_fn = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_all"),
                        args: vec![
                            Expr::Variable(current_alias.clone()),
                            Expr::Str(pred_op_str),
                            pred_val_expr,
                        ],
                        distinct: false,
                    };
                    current = Some(Operator::Filter {
                        predicate: pred_fn,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Any(pred) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
                    let pred_fn = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_any"),
                        args: vec![
                            Expr::Variable(current_alias.clone()),
                            Expr::Str(pred_op_str),
                            pred_val_expr,
                        ],
                        distinct: false,
                    };
                    current = Some(Operator::Filter {
                        predicate: pred_fn,
                        child: Box::new(child),
                    });
                }

                GremlinStep::NonePred(pred) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let (pred_op_str, pred_val_expr) = predicate_to_args(pred);
                    let pred_fn = Expr::FunctionCall {
                        name: SmolStr::new("gremlin_none"),
                        args: vec![
                            Expr::Variable(current_alias.clone()),
                            Expr::Str(pred_op_str),
                            pred_val_expr,
                        ],
                        distinct: false,
                    };
                    current = Some(Operator::Filter {
                        predicate: pred_fn,
                        child: Box::new(child),
                    });
                }

                // Predicate steps used inside Where/Is — should not appear at top level
                GremlinStep::Eq(_)
                | GremlinStep::Neq(_)
                | GremlinStep::Gt(_)
                | GremlinStep::Gte(_)
                | GremlinStep::Lt(_)
                | GremlinStep::Lte(_)
                | GremlinStep::Within(_)
                | GremlinStep::Without(_) => {
                    return Err(CompileError::InvalidSequence(
                        "predicate steps should not appear at the top level".into(),
                    ));
                }
            }
        }

        // Flush deferred choose output alias
        if let Some(alias) = pending_choose_output.take() {
            current_alias = alias;
        }

        // Flush accumulated path by-keys into the GremlinPath operator
        if pending_path && !path_by_keys.is_empty() {
            if let Some(Operator::GremlinPath {
                ref mut by_keys, ..
            }) = current
            {
                *by_keys = path_by_keys;
            }
        }

        // If Order was set but no By followed, apply default sort
        if pending_order && pending_order_local {
            // order(Scope.local) with no by() — default sort
            if let Some(child) = current.take() {
                let alias = self.next_alias("srtl");
                let mut items = vec![(
                    Expr::FunctionCall {
                        name: SmolStr::new("gremlin_sort_local"),
                        args: vec![Expr::Variable(current_alias.clone())],
                        distinct: false,
                    },
                    Some(alias.clone()),
                )];
                items.extend(self.label_carry_items());
                current = Some(Operator::Project {
                    items,
                    child: Box::new(child),
                });
                current_alias = alias;
            }
        } else if pending_order {
            if let Some(child) = current.take() {
                current = Some(Operator::Sort {
                    items: vec![(Expr::Variable(current_alias.clone()), true)],
                    gremlin_order: true,
                    child: Box::new(child),
                });
            }
        }

        // If a withSack() was present, inject __sack initialization into the
        // bottom-most leaf operator of the tree.
        if let Some(init_expr) = pending_sack_init {
            if let Some(ref mut op) = current {
                inject_sack_init(op, &init_expr);
            }
        }

        // Validate: emit/until/times without repeat is an error
        if pending_emit || pending_until.is_some() || pending_times.is_some() {
            return Err(CompileError::InvalidSequence(
                "The repeat()-traversal was not defined".into(),
            ));
        }

        // Apply any remaining select by() modulations
        if pending_select && !select_by_mods.is_empty() {
            if let Some(Operator::GremlinSelect { ref mut keys, .. }) = current {
                for (i, (src_var, _)) in keys.iter_mut().enumerate() {
                    let mod_key = &select_by_mods[i % select_by_mods.len()];
                    if !mod_key.is_empty() {
                        *src_var = Identifier::new(format!("__prop_{}__{}", mod_key, src_var.0));
                    }
                }
                if keys.len() == 1 && !select_by_mods.is_empty() {
                    current_alias = Identifier::new(keys[0].1.clone());
                }
            }
        }

        // Flush any remaining addE defaults (traversal ends with addE group)
        if let Some(ref traverser) = adde_traverser {
            if let Some(ref mut op) = current {
                rewrite_create_alias(op, "__src", traverser);
                rewrite_create_alias(op, "__dst", traverser);
            }
        }

        // Finalize any remaining format with by() modulations
        if pending_format && !format_by_aliases.is_empty() {
            let child = current.take().unwrap_or(Operator::SingleRow);
            let alias = self.next_alias("fmt");
            let mut args = vec![
                Expr::Variable(format_pre_alias.clone()),
                Expr::Str(format_pattern.clone().into_boxed_str()),
            ];
            for by_alias in &format_by_aliases {
                args.push(Expr::Variable(by_alias.clone()));
            }
            let mut items = vec![(
                Expr::FunctionCall {
                    name: SmolStr::new("gremlin_format_by"),
                    args,
                    distinct: false,
                },
                Some(alias.clone()),
            )];
            items.extend(self.label_carry_items());
            let project = Operator::Project {
                items,
                child: Box::new(child),
            };
            current = Some(Operator::Filter {
                predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                child: Box::new(project),
            });
            current_alias = alias;
        }

        self.last_alias = current_alias;
        current.ok_or(CompileError::MissingChild)
    }

    /// Compile And/Or branches, using predicate mode for simple branches and
    /// traversal (Apply+Count) mode for complex branches.
    fn compile_connective_branches(
        &mut self,
        branches: &[Vec<GremlinStep>],
        current_alias: &Identifier,
        child: Operator,
        conn_op: BinaryOp,
    ) -> Result<Operator, CompileError> {
        let all_simple = branches
            .iter()
            .all(|branch| is_simple_predicate_steps(branch));

        if all_simple {
            let preds: Vec<Expr> = branches
                .iter()
                .map(|branch| steps_to_predicate(branch, current_alias, &self.alias_map))
                .collect();
            let default = if conn_op == BinaryOp::And {
                Expr::Bool(true)
            } else {
                Expr::Bool(false)
            };
            let predicate = preds
                .into_iter()
                .reduce(|a, b| Expr::BinaryOp {
                    op: conn_op,
                    left: Box::new(a),
                    right: Box::new(b),
                })
                .unwrap_or(default);
            return Ok(Operator::Filter {
                predicate,
                child: Box::new(child),
            });
        }

        // At least one branch has complex traversal steps.
        // For AND: chain filters (each branch must pass).
        // For OR: build existence count per branch and combine predicates.
        if conn_op == BinaryOp::And {
            let mut result = child;
            for branch in branches {
                if is_simple_predicate_steps(branch) {
                    let pred = steps_to_predicate(branch, current_alias, &self.alias_map);
                    result = Operator::Filter {
                        predicate: pred,
                        child: Box::new(result),
                    };
                } else {
                    result = self.compile_existence_filter(branch, current_alias, result, false)?;
                }
            }
            Ok(result)
        } else {
            // OR: need to check if ANY branch produces results.
            let mut cnt_aliases = Vec::new();
            let mut result = child;
            for branch in branches {
                if is_simple_predicate_steps(branch) {
                    let pred = steps_to_predicate(branch, current_alias, &self.alias_map);
                    let cnt_alias = self.next_alias("ocnt");
                    // Wrap in a project that produces 1 if pred is true, 0 if false.
                    cnt_aliases.push((cnt_alias, Some(pred)));
                } else {
                    let mut sub = self.sub_compiler();
                    let inner_op = sub.compile_seeded(branch, current_alias)?;
                    self.absorb(&sub);
                    let sub_alias = sub.last_alias.clone();
                    let cnt_alias = self.next_alias("ocnt");
                    let count_op = Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            gql_core::planner::AggFunction::Count,
                            Expr::Variable(sub_alias),
                            None,
                            false,
                            cnt_alias.clone(),
                        )],
                        child: Box::new(inner_op),
                    };
                    result = Operator::Apply {
                        outer: Box::new(result),
                        inner: Box::new(count_op),
                    };
                    cnt_aliases.push((cnt_alias, None));
                }
            }

            // Build OR predicate: any count > 0 or any simple pred is true
            let or_pred = cnt_aliases
                .into_iter()
                .map(|(alias, simple_pred)| {
                    if let Some(pred) = simple_pred {
                        pred
                    } else {
                        Expr::BinaryOp {
                            op: BinaryOp::Gt,
                            left: Box::new(Expr::Variable(alias)),
                            right: Box::new(Expr::Integer(0)),
                        }
                    }
                })
                .reduce(|a, b| Expr::BinaryOp {
                    op: BinaryOp::Or,
                    left: Box::new(a),
                    right: Box::new(b),
                })
                .unwrap_or(Expr::Bool(false));

            Ok(Operator::Filter {
                predicate: or_pred,
                child: Box::new(result),
            })
        }
    }

    /// Compile a sub-traversal as an existence filter (Apply + Count + Filter).
    fn compile_existence_filter(
        &mut self,
        steps: &[GremlinStep],
        seed_alias: &Identifier,
        child: Operator,
        negate: bool,
    ) -> Result<Operator, CompileError> {
        let mut sub = self.sub_compiler();
        let inner_op = sub.compile_seeded(steps, seed_alias)?;
        self.absorb(&sub);
        let sub_alias = sub.last_alias.clone();
        let cnt_alias = self.next_alias("acnt");
        let count_op = Operator::Aggregate {
            keys: vec![],
            aggregations: vec![(
                gql_core::planner::AggFunction::Count,
                Expr::Variable(sub_alias),
                None,
                false,
                cnt_alias.clone(),
            )],
            child: Box::new(inner_op),
        };
        let apply = Operator::Apply {
            outer: Box::new(child),
            inner: Box::new(count_op),
        };
        let predicate = if negate {
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                left: Box::new(Expr::Variable(cnt_alias)),
                right: Box::new(Expr::Integer(0)),
            }
        } else {
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(Expr::Variable(cnt_alias)),
                right: Box::new(Expr::Integer(0)),
            }
        };
        Ok(Operator::Filter {
            predicate,
            child: Box::new(apply),
        })
    }
}

/// Check whether all steps in a branch are simple predicate-convertible steps.
fn is_simple_predicate_steps(steps: &[GremlinStep]) -> bool {
    !steps.is_empty()
        && steps.iter().all(|s| {
            matches!(
                s,
                GremlinStep::Is(_)
                    | GremlinStep::Has(_, _)
                    | GremlinStep::HasPredicate(_, _)
                    | GremlinStep::HasLabel(_)
                    | GremlinStep::HasId(_)
                    | GremlinStep::HasIdPredicate(_)
                    | GremlinStep::Values(_)
                    | GremlinStep::Label
                    | GremlinStep::Id
                    | GremlinStep::Select(_)
            )
        })
}

/// Preprocess a step list to handle infix `and()`/`or()` with no arguments.
///
/// In Gremlin, `hasLabel("person").and().out("created")` is syntactic sugar for
/// `and(hasLabel("person"), out("created"))`. This function detects empty `And([])`
/// and `Or([])` steps and transforms the surrounding steps into proper branches.
///
/// Source steps (V, E, Inject) are preserved at the beginning, outside the
/// connective branches.  Or has lower precedence than And.
fn preprocess_infix_connectives(steps: &[GremlinStep]) -> Vec<GremlinStep> {
    let has_infix = steps.iter().any(|s| match s {
        GremlinStep::And(b) if b.is_empty() => true,
        GremlinStep::Or(b) if b.is_empty() => true,
        _ => false,
    });
    if !has_infix {
        return steps.to_vec();
    }

    // Separate leading source steps from traversal steps.
    let source_end = steps
        .iter()
        .position(|s| {
            !matches!(
                s,
                GremlinStep::V(_) | GremlinStep::E(_) | GremlinStep::Inject(_)
            )
        })
        .unwrap_or(steps.len());

    let source_steps = &steps[..source_end];
    let traversal_steps = &steps[source_end..];

    // Re-check that the infix connective is in the traversal portion.
    let has_infix_in_traversal = traversal_steps.iter().any(|s| match s {
        GremlinStep::And(b) if b.is_empty() => true,
        GremlinStep::Or(b) if b.is_empty() => true,
        _ => false,
    });
    if !has_infix_in_traversal {
        return steps.to_vec();
    }

    // Split traversal steps at Or([]) first (lower precedence).
    let or_segments = split_at_empty_connective(traversal_steps, false);

    let mut result: Vec<GremlinStep> = source_steps.to_vec();
    if or_segments.len() > 1 {
        let or_branches: Vec<Vec<GremlinStep>> = or_segments
            .into_iter()
            .map(|segment| {
                let and_segments = split_at_empty_connective(&segment, true);
                if and_segments.len() > 1 {
                    vec![GremlinStep::And(and_segments)]
                } else {
                    segment
                }
            })
            .collect();
        result.push(GremlinStep::Or(or_branches));
    } else {
        let and_segments = split_at_empty_connective(traversal_steps, true);
        if and_segments.len() > 1 {
            result.push(GremlinStep::And(and_segments));
        } else {
            result.extend(traversal_steps.iter().cloned());
        }
    }
    result
}

/// Split a step list at empty `And([])` (is_and=true) or `Or([])` (is_and=false).
fn split_at_empty_connective(steps: &[GremlinStep], is_and: bool) -> Vec<Vec<GremlinStep>> {
    let mut segments = Vec::new();
    let mut current_seg = Vec::new();
    for step in steps {
        let is_split = match step {
            GremlinStep::And(b) if b.is_empty() && is_and => true,
            GremlinStep::Or(b) if b.is_empty() && !is_and => true,
            _ => false,
        };
        if is_split {
            segments.push(std::mem::take(&mut current_seg));
        } else {
            current_seg.push(step.clone());
        }
    }
    segments.push(current_seg);
    segments
}

/// Inject a `__sack` column into the lowest-level scan/single-row operator
/// by wrapping it in an Apply that adds the initialization value.
fn inject_sack_init(op: &mut Operator, init_expr: &Expr) {
    match op {
        Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::GremlinAggregate { child, .. }
        | Operator::Cap { child, .. }
        | Operator::Repeat { child, .. }
        | Operator::Project { child, .. }
        | Operator::GremlinGroup { child, .. }
        | Operator::GremlinSelect { child, .. }
        | Operator::Choose { child, .. }
        | Operator::ChooseMap { child, .. }
        | Operator::Coalesce { child, .. }
        | Operator::Expand { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::GremlinPath { child, .. }
        | Operator::GremlinMatch { child, .. }
        | Operator::GremlinSimplePath { child, .. }
        | Operator::GremlinCyclicPath { child, .. }
        | Operator::GremlinIndex { child, .. }
        | Operator::GremlinProject { child, .. }
        | Operator::GremlinMapSlice { child, .. }
        | Operator::ForEach { child, .. }
        | Operator::VarLenExpand { child, .. }
        | Operator::Aggregate { child, .. } => {
            inject_sack_init(child, init_expr);
        }
        Operator::Apply { outer, .. }
        | Operator::LeftOuterApply {
            preserved: outer, ..
        } => {
            inject_sack_init(outer, init_expr);
        }
        Operator::Union { left, right, .. } => {
            inject_sack_init(left, init_expr);
            inject_sack_init(right, init_expr);
        }
        // Leaf operators: wrap in Apply that adds __sack
        Operator::NodeScan { .. }
        | Operator::IndexScan { .. }
        | Operator::NodeByIdSeek { .. }
        | Operator::SingleRow
        | Operator::Unwind { .. } => {
            let original = std::mem::replace(op, Operator::SingleRow);
            *op = Operator::Apply {
                outer: Box::new(original),
                inner: Box::new(Operator::Project {
                    items: vec![(init_expr.clone(), Some(Identifier::new("__sack")))],
                    child: Box::new(Operator::SingleRow),
                }),
            };
        }
        // Operators with optional child: recurse if child exists
        Operator::Create { child, .. }
        | Operator::Merge { child, .. }
        | Operator::Call { child, .. } => {
            if let Some(ch) = child {
                inject_sack_init(ch, init_expr);
            }
        }
        // Remaining operators with a required child
        Operator::MergeRelationship { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. }
        | Operator::AssemblePath { child, .. }
        | Operator::ProduceResults { child, .. } => {
            inject_sack_init(child, init_expr);
        }
        _ => {}
    }
}

/// Inject a vertex scan into the child chain of an operator (typically Create)
/// so that the scan's alias is available in every row when the operator executes.
/// Uses Apply with scan as outer and original operator as inner.
fn inject_scan_into_child(op: Operator, scan: Operator) -> Operator {
    match op {
        Operator::Create { patterns, child } => {
            let new_child = match child {
                Some(ch) => Some(Box::new(Operator::Apply {
                    outer: Box::new(scan),
                    inner: ch,
                })),
                None => Some(Box::new(scan)),
            };
            Operator::Create {
                patterns,
                child: new_child,
            }
        }
        Operator::Set { items, child } => {
            // Set wrapping Create: inject into Set's child
            Operator::Set {
                items,
                child: Box::new(inject_scan_into_child(*child, scan)),
            }
        }
        other => {
            // Fallback: Apply with scan as outer
            Operator::Apply {
                outer: Box::new(scan),
                inner: Box::new(other),
            }
        }
    }
}

/// Recursively rewrite a placeholder alias in a `Create` operator's edge specs.
fn rewrite_create_alias(op: &mut Operator, placeholder: &str, replacement: &Identifier) {
    match op {
        Operator::Create { patterns, .. } => {
            for pat in patterns {
                for edge in &mut pat.edges {
                    // edge is (alias, src, dst, rel_type, dir, props)
                    if edge.1.as_str() == placeholder {
                        edge.1 = replacement.clone();
                    }
                    if edge.2.as_str() == placeholder {
                        edge.2 = replacement.clone();
                    }
                }
            }
        }
        Operator::Apply { outer, inner } => {
            rewrite_create_alias(outer, placeholder, replacement);
            rewrite_create_alias(inner, placeholder, replacement);
        }
        Operator::Set { child, .. } => {
            rewrite_create_alias(child, placeholder, replacement);
        }
        _ => {}
    }
}

/// Convert a `GremlinValue` to an `Expr` literal.
fn gremlin_value_to_expr(gv: &GremlinValue) -> Expr {
    match gv {
        GremlinValue::Null => Expr::Null,
        GremlinValue::Bool(b) => Expr::Bool(*b),
        GremlinValue::Integer(i) => Expr::Integer(*i),
        GremlinValue::Float(f) => Expr::Float(*f),
        GremlinValue::String(s) => Expr::Str(s.clone().into_boxed_str()),
        GremlinValue::List(items) => Expr::List(items.iter().map(gremlin_value_to_expr).collect()),
        GremlinValue::Map(entries) => Expr::MapLiteral(
            entries
                .iter()
                .map(|(k, v)| (SmolStr::new(k), gremlin_value_to_expr(v)))
                .collect(),
        ),
        GremlinValue::Vertex { id, .. } => Expr::Integer(*id),
        GremlinValue::Edge { id, .. } => Expr::Integer(*id),
        GremlinValue::Path { .. } => Expr::Null,
    }
}

/// Convert a `GremlinValue` to a core `Value` for use in ChooseMap option keys.
fn gremlin_value_to_value(gv: &GremlinValue) -> Value {
    match gv {
        GremlinValue::Null => Value::Null,
        GremlinValue::Bool(b) => Value::Bool(*b),
        GremlinValue::Integer(i) => Value::Int(*i),
        GremlinValue::Float(f) => Value::Float(*f),
        GremlinValue::String(s) => Value::Str(s.clone().into_boxed_str()),
        GremlinValue::List(items) => {
            Value::List(items.iter().map(gremlin_value_to_value).collect())
        }
        GremlinValue::Map(_)
        | GremlinValue::Vertex { .. }
        | GremlinValue::Edge { .. }
        | GremlinValue::Path { .. } => Value::Null,
    }
}

/// Split a trailing aggregation step (Count, Sum, Fold) from a list of Gremlin steps.
/// Returns the remaining steps (before the agg) and the detected aggregation function.
fn split_trailing_agg(steps: &[GremlinStep]) -> (&[GremlinStep], Option<AggFunction>) {
    if let Some(last) = steps.last() {
        match last {
            GremlinStep::Count => (&steps[..steps.len() - 1], Some(AggFunction::Count)),
            GremlinStep::Sum => (&steps[..steps.len() - 1], Some(AggFunction::Sum)),
            GremlinStep::Fold => (&steps[..steps.len() - 1], Some(AggFunction::Collect)),
            _ => (steps, None),
        }
    } else {
        (steps, None)
    }
}

/// Build an `Expr` from a `by(key)` modulator key string and the current alias.
/// Handles T.id, T.label, Column.keys/values, common traversal patterns, and property names.
fn make_by_expr(key: &str, current_alias: &Identifier) -> Expr {
    if key == "~id" || key == "T.id" || key == "t[id]" {
        Expr::FunctionCall {
            name: SmolStr::new("id"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "~label"
        || key == "T.label"
        || key == "t[label]"
        || key == "__.label()"
        || key == "label()"
    {
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_single_label"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if let Some(prop) = key
        .strip_prefix("__.values(")
        .or_else(|| key.strip_prefix("values("))
        .and_then(|s| s.strip_suffix(')'))
    {
        // by(values("name")) or by(__.values("name")) → property access
        let prop = prop.trim_matches(|c| c == '\'' || c == '"');
        Expr::Property(
            Box::new(Expr::Variable(current_alias.clone())),
            SmolStr::new(prop),
        )
    } else if key == "~key" || key == "T.key" || key == "t[key]" {
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_property_key"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "~value" || key == "T.value" || key == "t[value]" {
        Expr::FunctionCall {
            name: SmolStr::new("gremlin_property_value"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else {
        Expr::Property(
            Box::new(Expr::Variable(current_alias.clone())),
            SmolStr::new(key),
        )
    }
}

/// Rewrite a sack assignment expression, replacing Expr::Variable references
/// that match `target` with `replacement`. Used by `.by(key)` after `sack(op)`.
fn rewrite_sack_by(expr: &mut Expr, target: &Identifier, replacement: &Expr) {
    match expr {
        Expr::Variable(id) if id == target => {
            *expr = replacement.clone();
        }
        Expr::BinaryOp { left, right, .. } => {
            rewrite_sack_by(left, target, replacement);
            rewrite_sack_by(right, target, replacement);
        }
        _ => {}
    }
}

/// Convert a single GremlinStep into a filter predicate Expr.
fn step_to_predicate(step: &GremlinStep, alias: &Identifier) -> Expr {
    match step {
        GremlinStep::Has(key, Some(val)) => Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(Expr::Property(
                Box::new(Expr::Variable(alias.clone())),
                SmolStr::new(key),
            )),
            right: Box::new(gremlin_value_to_expr(val)),
        },
        GremlinStep::Has(key, None) => Expr::IsNotNull(Box::new(Expr::Property(
            Box::new(Expr::Variable(alias.clone())),
            SmolStr::new(key),
        ))),
        GremlinStep::HasPredicate(key, pred) => {
            let prop_expr =
                Expr::Property(Box::new(Expr::Variable(alias.clone())), SmolStr::new(key));
            // has(key, pred) means: property exists AND satisfies predicate.
            // Wrap in AND(IsNotNull(prop), pred(prop)) to properly handle
            // missing properties (returns false instead of null).
            let pred_expr = predicate_to_prop_expr(pred, prop_expr.clone());
            Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(Expr::IsNotNull(Box::new(prop_expr))),
                right: Box::new(pred_expr),
            }
        }
        GremlinStep::HasLabel(labels) if !labels.is_empty() => labels
            .iter()
            .map(|l| {
                Expr::HasLabel(
                    Box::new(Expr::Variable(alias.clone())),
                    Label::new(l.as_str()),
                )
            })
            .reduce(|a, b| Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(a),
                right: Box::new(b),
            })
            .unwrap(),
        GremlinStep::Is(pred) => predicate_to_expr(pred, alias),
        GremlinStep::HasId(ids) if !ids.is_empty() => {
            let id_fn = Expr::FunctionCall {
                name: SmolStr::new("id"),
                args: vec![Expr::Variable(alias.clone())],
                distinct: false,
            };
            // Flatten list parameters: hasId(id1, [id2, id3]) → hasId(id1, id2, id3)
            let mut flat_ids = Vec::new();
            for id in ids {
                match id {
                    GremlinValue::List(inner) => flat_ids.extend(inner.iter().cloned()),
                    other => flat_ids.push(other.clone()),
                }
            }
            if flat_ids.len() == 1 {
                Expr::BinaryOp {
                    op: BinaryOp::Eq,
                    left: Box::new(id_fn),
                    right: Box::new(gremlin_value_to_expr(&flat_ids[0])),
                }
            } else {
                let id_exprs: Vec<Expr> = flat_ids.iter().map(gremlin_value_to_expr).collect();
                Expr::BinaryOp {
                    op: BinaryOp::In,
                    left: Box::new(id_fn),
                    right: Box::new(Expr::List(id_exprs)),
                }
            }
        }
        GremlinStep::HasIdPredicate(pred) => {
            let id_fn = Expr::FunctionCall {
                name: SmolStr::new("id"),
                args: vec![Expr::Variable(alias.clone())],
                distinct: false,
            };
            predicate_to_prop_expr(pred, id_fn)
        }
        _ => Expr::Bool(true), // fallback for complex sub-traversals
    }
}

/// Convert a list of GremlinSteps into a filter predicate.
/// Handles multi-step traversals like `values("age").is(P.gt(30))`.
fn steps_to_predicate(
    steps: &[GremlinStep],
    alias: &Identifier,
    alias_map: &HashMap<String, Identifier>,
) -> Expr {
    if steps.is_empty() {
        return Expr::Bool(true);
    }

    // Single step: delegate to step_to_predicate
    if steps.len() == 1 {
        // Handle Or/And/Not at the single-step level too
        match &steps[0] {
            GremlinStep::Or(branches) => {
                return branches
                    .iter()
                    .map(|b| steps_to_predicate(b, alias, alias_map))
                    .reduce(|a, b| Expr::BinaryOp {
                        op: BinaryOp::Or,
                        left: Box::new(a),
                        right: Box::new(b),
                    })
                    .unwrap_or(Expr::Bool(false));
            }
            GremlinStep::And(branches) => {
                return branches
                    .iter()
                    .map(|b| steps_to_predicate(b, alias, alias_map))
                    .reduce(|a, b| Expr::BinaryOp {
                        op: BinaryOp::And,
                        left: Box::new(a),
                        right: Box::new(b),
                    })
                    .unwrap_or(Expr::Bool(true));
            }
            GremlinStep::Not(inner) => {
                return Expr::UnaryOp {
                    op: UnaryOp::Not,
                    expr: Box::new(steps_to_predicate(inner, alias, alias_map)),
                };
            }
            _ => {}
        }
        // Single Is(pred) where the pred argument is a string label reference:
        // where(P.eq("b")) → compare current value against labeled value "b"
        if let GremlinStep::Is(pred) = &steps[0] {
            if let Some(label_expr) = predicate_label_ref(pred, alias_map) {
                let op = predicate_to_binary_op(pred);
                return Expr::BinaryOp {
                    op,
                    left: Box::new(Expr::Variable(alias.clone())),
                    right: Box::new(label_expr),
                };
            }
        }
        return step_to_predicate(&steps[0], alias);
    }

    // Two-step pattern: Values(["key"]) + Is(pred) → property predicate
    if steps.len() == 2 {
        if let (GremlinStep::Values(keys), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            if keys.len() == 1 {
                let prop_expr = Expr::Property(
                    Box::new(Expr::Variable(alias.clone())),
                    SmolStr::new(&keys[0]),
                );
                return predicate_to_prop_expr(pred, prop_expr);
            }
        }
        // Label + Is(Eq("person")) → hasLabel("person")
        if let (GremlinStep::Label, GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            match pred {
                Predicate::Eq(GremlinValue::String(label)) => {
                    return Expr::HasLabel(
                        Box::new(Expr::Variable(alias.clone())),
                        Label::new(label.as_str()),
                    );
                }
                _ => {
                    // For other predicates on label, use function call
                    let label_expr = Expr::FunctionCall {
                        name: SmolStr::new("labels"),
                        args: vec![Expr::Variable(alias.clone())],
                        distinct: false,
                    };
                    return predicate_to_prop_expr(pred, label_expr);
                }
            }
        }
        // Id + Is(pred) → id predicate
        if let (GremlinStep::Id, GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            let id_expr = Expr::FunctionCall {
                name: SmolStr::new("id"),
                args: vec![Expr::Variable(alias.clone())],
                distinct: false,
            };
            return predicate_to_prop_expr(pred, id_expr);
        }
        // Two-step pattern: Select(["label"]) + Is(pred) → labeled value comparison
        // This handles where("a", P.eq("b"))
        if let (GremlinStep::Select(keys), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            if keys.len() == 1 {
                let src_var = alias_map
                    .get(&keys[0])
                    .cloned()
                    .unwrap_or_else(|| Identifier::new(keys[0].clone()));
                if let Some(label_expr) = predicate_label_ref(pred, alias_map) {
                    let op = predicate_to_binary_op(pred);
                    return Expr::BinaryOp {
                        op,
                        left: Box::new(Expr::Variable(src_var)),
                        right: Box::new(label_expr),
                    };
                }
                return predicate_to_expr(pred, &src_var);
            }
        }
    }

    // Multi-step patterns: try to build AND of individual predicates
    let mut preds = Vec::new();
    for step in steps {
        // Handle Or/And/Not by recursing into steps_to_predicate
        let pred = match step {
            GremlinStep::Or(branches) => branches
                .iter()
                .map(|b| steps_to_predicate(b, alias, alias_map))
                .reduce(|a, b| Expr::BinaryOp {
                    op: BinaryOp::Or,
                    left: Box::new(a),
                    right: Box::new(b),
                })
                .unwrap_or(Expr::Bool(false)),
            GremlinStep::And(branches) => branches
                .iter()
                .map(|b| steps_to_predicate(b, alias, alias_map))
                .reduce(|a, b| Expr::BinaryOp {
                    op: BinaryOp::And,
                    left: Box::new(a),
                    right: Box::new(b),
                })
                .unwrap_or(Expr::Bool(true)),
            GremlinStep::Not(inner) => {
                let inner_pred = steps_to_predicate(inner, alias, alias_map);
                Expr::UnaryOp {
                    op: UnaryOp::Not,
                    expr: Box::new(inner_pred),
                }
            }
            _ => step_to_predicate(step, alias),
        };
        if pred != Expr::Bool(true) {
            preds.push(pred);
        }
    }

    if preds.is_empty() {
        return Expr::Bool(true);
    }

    preds
        .into_iter()
        .reduce(|a, b| Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(a),
            right: Box::new(b),
        })
        .unwrap_or(Expr::Bool(true))
}

/// Extract predicate operator name and value expression for all()/any()/none() steps.
fn predicate_to_args(pred: &Predicate) -> (Box<str>, Expr) {
    match pred {
        Predicate::Eq(v) => ("eq".into(), gremlin_value_to_expr(v)),
        Predicate::Neq(v) => ("neq".into(), gremlin_value_to_expr(v)),
        Predicate::Gt(v) => ("gt".into(), gremlin_value_to_expr(v)),
        Predicate::Gte(v) => ("gte".into(), gremlin_value_to_expr(v)),
        Predicate::Lt(v) => ("lt".into(), gremlin_value_to_expr(v)),
        Predicate::Lte(v) => ("lte".into(), gremlin_value_to_expr(v)),
        Predicate::Within(vals) => (
            "within".into(),
            Expr::List(vals.iter().map(gremlin_value_to_expr).collect()),
        ),
        Predicate::Without(vals) => (
            "without".into(),
            Expr::List(vals.iter().map(gremlin_value_to_expr).collect()),
        ),
        Predicate::Between(low, high) => (
            "within".into(),
            Expr::List(vec![
                gremlin_value_to_expr(low),
                gremlin_value_to_expr(high),
            ]),
        ),
        Predicate::Inside(low, high) => (
            "within".into(),
            Expr::List(vec![
                gremlin_value_to_expr(low),
                gremlin_value_to_expr(high),
            ]),
        ),
        _ => ("neq".into(), Expr::Null),
    }
}

/// Convert a `Predicate` to an `Expr`.
fn predicate_to_expr(pred: &Predicate, alias: &Identifier) -> Expr {
    let var = Expr::Variable(alias.clone());
    match pred {
        Predicate::Eq(v) => {
            // Gremlin: P.eq(null) means IS NULL (unlike Cypher where null=null is null)
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNull(Box::new(var));
            }
            // Gremlin: null.eq(non_null) is false, not null.
            // Use (var IS NOT NULL AND var == val) to prevent null from passing.
            Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(Expr::IsNotNull(Box::new(Expr::Variable(alias.clone())))),
                right: Box::new(Expr::BinaryOp {
                    op: BinaryOp::Eq,
                    left: Box::new(var),
                    right: Box::new(gremlin_value_to_expr(v)),
                }),
            }
        }
        Predicate::Neq(v) => {
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNotNull(Box::new(var));
            }
            // Gremlin: null != non_null is true (pass through).
            // Use (var IS NULL OR var != val) to handle null propagation.
            Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(Expr::IsNull(Box::new(Expr::Variable(alias.clone())))),
                right: Box::new(Expr::BinaryOp {
                    op: BinaryOp::Neq,
                    left: Box::new(var),
                    right: Box::new(gremlin_value_to_expr(v)),
                }),
            }
        }
        Predicate::Gt(v) => {
            if matches!(v, GremlinValue::Null) {
                return Expr::Bool(false);
            }
            Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Gte(v) => {
            // Gremlin: P.gte(null) on null value should be true
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNull(Box::new(var));
            }
            Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Lt(v) => {
            // Gremlin: P.lt(null) always false (nothing is strictly less than null)
            if matches!(v, GremlinValue::Null) {
                return Expr::Bool(false);
            }
            Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Lte(v) => {
            if matches!(v, GremlinValue::Null) {
                return Expr::IsNull(Box::new(var));
            }
            Expr::BinaryOp {
                op: BinaryOp::Lte,
                left: Box::new(var),
                right: Box::new(gremlin_value_to_expr(v)),
            }
        }
        Predicate::Within(vals) => Expr::BinaryOp {
            op: BinaryOp::In,
            left: Box::new(var),
            right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
        },
        Predicate::Without(vals) => Expr::UnaryOp {
            op: gql_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::BinaryOp {
                op: BinaryOp::In,
                left: Box::new(var),
                right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
            }),
        },
        Predicate::Between(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        // inside(low, high) → low < x < high (exclusive on both ends)
        Predicate::Inside(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(Expr::Variable(alias.clone())),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        // Text predicates reuse the same pattern as predicate_to_prop_expr
        Predicate::Containing(_)
        | Predicate::NotContaining(_)
        | Predicate::StartingWith(_)
        | Predicate::NotStartingWith(_)
        | Predicate::EndingWith(_)
        | Predicate::NotEndingWith(_)
        | Predicate::Regex(_)
        | Predicate::NotRegex(_)
        | Predicate::IsType(_)
        | Predicate::And(_)
        | Predicate::Or(_)
        | Predicate::Not(_) => predicate_to_prop_expr(pred, var),
    }
}

/// Apply a predicate to a property expression (for `has("key", P.gt(30))`).
fn predicate_to_prop_expr(pred: &Predicate, prop_expr: Expr) -> Expr {
    match pred {
        Predicate::Eq(v) => Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Neq(v) => Expr::BinaryOp {
            op: BinaryOp::Neq,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Gt(v) => Expr::BinaryOp {
            op: BinaryOp::Gt,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Gte(v) => Expr::BinaryOp {
            op: BinaryOp::Gte,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Lt(v) => Expr::BinaryOp {
            op: BinaryOp::Lt,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Lte(v) => Expr::BinaryOp {
            op: BinaryOp::Lte,
            left: Box::new(prop_expr),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Within(vals) => Expr::BinaryOp {
            op: BinaryOp::In,
            left: Box::new(prop_expr),
            right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
        },
        Predicate::Without(vals) => Expr::UnaryOp {
            op: gql_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::BinaryOp {
                op: BinaryOp::In,
                left: Box::new(prop_expr),
                right: Box::new(Expr::List(vals.iter().map(gremlin_value_to_expr).collect())),
            }),
        },
        Predicate::Between(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gte,
                left: Box::new(prop_expr.clone()),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(prop_expr),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        // inside(low, high) → low < x < high (exclusive on both ends)
        Predicate::Inside(low, high) => Expr::BinaryOp {
            op: BinaryOp::And,
            left: Box::new(Expr::BinaryOp {
                op: BinaryOp::Gt,
                left: Box::new(prop_expr.clone()),
                right: Box::new(gremlin_value_to_expr(low)),
            }),
            right: Box::new(Expr::BinaryOp {
                op: BinaryOp::Lt,
                left: Box::new(prop_expr),
                right: Box::new(gremlin_value_to_expr(high)),
            }),
        },
        Predicate::Containing(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_containing"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotContaining(v) => Expr::UnaryOp {
            op: gql_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_containing"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::StartingWith(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_startingWith"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotStartingWith(v) => Expr::UnaryOp {
            op: gql_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_startingWith"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::EndingWith(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_endingWith"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotEndingWith(v) => Expr::UnaryOp {
            op: gql_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_endingWith"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::Regex(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_regex"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::NotRegex(v) => Expr::UnaryOp {
            op: gql_core::ast::UnaryOp::Not,
            expr: Box::new(Expr::FunctionCall {
                name: SmolStr::new("gremlin_regex"),
                args: vec![prop_expr, gremlin_value_to_expr(v)],
                distinct: false,
            }),
        },
        Predicate::IsType(v) => Expr::FunctionCall {
            name: SmolStr::new("gremlin_isType"),
            args: vec![prop_expr, gremlin_value_to_expr(v)],
            distinct: false,
        },
        Predicate::And(preds) => {
            let mut iter = preds.iter();
            let first = predicate_to_prop_expr(iter.next().unwrap(), prop_expr.clone());
            iter.fold(first, |acc, p| Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(acc),
                right: Box::new(predicate_to_prop_expr(p, prop_expr.clone())),
            })
        }
        Predicate::Or(preds) => {
            let mut iter = preds.iter();
            let first = predicate_to_prop_expr(iter.next().unwrap(), prop_expr.clone());
            iter.fold(first, |acc, p| Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(acc),
                right: Box::new(predicate_to_prop_expr(p, prop_expr.clone())),
            })
        }
        Predicate::Not(inner) => Expr::UnaryOp {
            op: gql_core::ast::UnaryOp::Not,
            expr: Box::new(predicate_to_prop_expr(inner, prop_expr)),
        },
    }
}
/// in the alias map. Returns the variable expression for that label if so.
fn predicate_label_ref(pred: &Predicate, alias_map: &HashMap<String, Identifier>) -> Option<Expr> {
    let label = match pred {
        Predicate::Eq(GremlinValue::String(s))
        | Predicate::Neq(GremlinValue::String(s))
        | Predicate::Gt(GremlinValue::String(s))
        | Predicate::Gte(GremlinValue::String(s))
        | Predicate::Lt(GremlinValue::String(s))
        | Predicate::Lte(GremlinValue::String(s)) => s,
        // within("a") / without("a") with a single string argument:
        // if it resolves to a known alias/side-effect, treat it as a
        // variable reference (membership test against the stored list).
        Predicate::Within(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                s
            } else {
                return None;
            }
        }
        Predicate::Without(vals) if vals.len() == 1 => {
            if let GremlinValue::String(s) = &vals[0] {
                s
            } else {
                return None;
            }
        }
        _ => return None,
    };
    alias_map.get(label).map(|id| Expr::Variable(id.clone()))
}

/// Extract the BinaryOp from a predicate (ignoring its value).
fn predicate_to_binary_op(pred: &Predicate) -> BinaryOp {
    match pred {
        Predicate::Eq(_) => BinaryOp::Eq,
        Predicate::Neq(_) => BinaryOp::Neq,
        Predicate::Gt(_) => BinaryOp::Gt,
        Predicate::Gte(_) => BinaryOp::Gte,
        Predicate::Lt(_) => BinaryOp::Lt,
        Predicate::Lte(_) => BinaryOp::Lte,
        Predicate::Within(_) => BinaryOp::In,
        Predicate::Without(_) => BinaryOp::In,
        Predicate::Between(_, _) => BinaryOp::And,
        Predicate::Inside(_, _) => BinaryOp::And,
        // Text predicates don't map to a single BinaryOp; default to Eq
        Predicate::Containing(_)
        | Predicate::NotContaining(_)
        | Predicate::StartingWith(_)
        | Predicate::NotStartingWith(_)
        | Predicate::EndingWith(_)
        | Predicate::NotEndingWith(_)
        | Predicate::Regex(_)
        | Predicate::NotRegex(_)
        | Predicate::IsType(_)
        | Predicate::And(_)
        | Predicate::Or(_)
        | Predicate::Not(_) => BinaryOp::Eq,
    }
}

/// Extract the left/right variable identifiers from a `Where([Select(["a"]), Is(pred)])`
/// pattern where the predicate references another label. Returns `(left_var, right_var)`.
fn extract_where_label_pair(
    steps: &[GremlinStep],
    alias_map: &HashMap<String, Identifier>,
    current_alias: &Identifier,
) -> Option<(Identifier, Identifier)> {
    // Pattern 1: Select(["a"]) + Is(pred("b")) → where("a", P.gt("b"))
    if steps.len() == 2 {
        if let (GremlinStep::Select(keys), GremlinStep::Is(pred)) = (&steps[0], &steps[1]) {
            if keys.len() == 1 {
                let left = alias_map
                    .get(&keys[0])
                    .cloned()
                    .unwrap_or_else(|| Identifier::new(keys[0].clone()));
                let right_label = match pred {
                    Predicate::Eq(GremlinValue::String(s))
                    | Predicate::Neq(GremlinValue::String(s))
                    | Predicate::Gt(GremlinValue::String(s))
                    | Predicate::Gte(GremlinValue::String(s))
                    | Predicate::Lt(GremlinValue::String(s))
                    | Predicate::Lte(GremlinValue::String(s)) => Some(s.as_str()),
                    _ => None,
                };
                if let Some(rl) = right_label {
                    let right = alias_map
                        .get(rl)
                        .cloned()
                        .unwrap_or_else(|| Identifier::new(rl));
                    return Some((left, right));
                }
            }
        }
    }
    // Pattern 2: Is(pred("a")) → where(P.gt("a")) compares current vs label
    if steps.len() == 1 {
        if let GremlinStep::Is(pred) = &steps[0] {
            let label = match pred {
                Predicate::Gt(GremlinValue::String(s))
                | Predicate::Gte(GremlinValue::String(s))
                | Predicate::Lt(GremlinValue::String(s))
                | Predicate::Lte(GremlinValue::String(s))
                | Predicate::Eq(GremlinValue::String(s))
                | Predicate::Neq(GremlinValue::String(s)) => Some(s.as_str()),
                _ => None,
            };
            if let Some(l) = label {
                if alias_map.contains_key(l) {
                    let right = alias_map[l].clone();
                    return Some((current_alias.clone(), right));
                }
            }
        }
    }
    None
}

/// Rewrite a where predicate to use property access. Transforms:
/// `left_var > right_var` → `left_var.key > right_var.key`
fn rewrite_where_by_predicate(
    predicate: &mut Expr,
    left_id: &Identifier,
    right_id: &Identifier,
    key: &str,
) {
    match predicate {
        Expr::BinaryOp { left, right, .. } => {
            if let Expr::Variable(ref v) = **left {
                if v == left_id {
                    *left = Box::new(Expr::Property(
                        Box::new(Expr::Variable(left_id.clone())),
                        SmolStr::new(key),
                    ));
                }
            }
            if let Expr::Variable(ref v) = **right {
                if v == right_id {
                    *right = Box::new(Expr::Property(
                        Box::new(Expr::Variable(right_id.clone())),
                        SmolStr::new(key),
                    ));
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compile_v_scan() {
        let mut compiler = GremlinCompiler::new();
        let op = compiler.compile(&[GremlinStep::V(None)]).unwrap();
        assert!(matches!(op, Operator::NodeScan { label: None, .. }));
    }

    #[test]
    fn compile_v_with_id() {
        let mut compiler = GremlinCompiler::new();
        let op = compiler
            .compile(&[GremlinStep::V(Some(vec![GremlinValue::Integer(1)]))])
            .unwrap();
        assert!(matches!(op, Operator::Filter { .. }));
    }

    #[test]
    fn compile_has_filter() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into()))),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Filter { .. }));
    }

    #[test]
    fn compile_out_expand() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Out(vec!["knows".into()])];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(
            op,
            Operator::Expand {
                direction: Direction::Outgoing,
                ..
            }
        ));
    }

    #[test]
    fn compile_full_traversal() {
        // g.V().has("name","marko").out("knows").values("name")
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into()))),
            GremlinStep::Out(vec!["knows".into()]),
            GremlinStep::Values(vec!["name".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        // Top-level should be Filter (null filter from values) wrapping Project
        assert!(matches!(op, Operator::Filter { .. }));

        // Walk down to verify structure
        if let Operator::Filter { child: proj, .. } = &op {
            if let Operator::Project { child, items, .. } = proj.as_ref() {
                assert_eq!(items.len(), 1);
                assert!(matches!(**child, Operator::Expand { .. }));
            }
        }
    }

    #[test]
    fn compile_count() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Count];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Aggregate { .. }));
    }

    #[test]
    fn compile_order_by_limit() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Order,
            GremlinStep::By("age".into(), true),
            GremlinStep::Limit(10),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Limit { .. }));
        if let Operator::Limit { child, .. } = &op {
            assert!(matches!(**child, Operator::Sort { .. }));
        }
    }

    #[test]
    fn compile_dedup() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Dedup(vec![])];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Distinct { .. }));
    }

    #[test]
    fn compile_add_v() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::AddV("person".into()),
            GremlinStep::Property("name".into(), GremlinValue::String("alice".into())),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Set { .. }));
    }

    #[test]
    fn compile_drop() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Drop];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Delete { detach: true, .. }));
    }

    #[test]
    fn compile_has_label() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::HasLabel(vec!["person".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Filter { .. }));
    }

    #[test]
    fn compile_in_expand() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::In(vec!["created".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(
            op,
            Operator::Expand {
                direction: Direction::Incoming,
                ..
            }
        ));
    }

    #[test]
    fn compile_empty_steps_returns_single_row() {
        let mut compiler = GremlinCompiler::new();
        let op = compiler.compile(&[]).unwrap();
        assert!(matches!(op, Operator::SingleRow));
    }

    #[test]
    fn compile_range() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Range(5, 15)];
        let op = compiler.compile(&steps).unwrap();
        // Should be Limit wrapping Skip
        assert!(matches!(op, Operator::Limit { .. }));
        if let Operator::Limit { child, count } = &op {
            assert!(matches!(count, Expr::Integer(10)));
            assert!(matches!(**child, Operator::Skip { .. }));
        }
    }

    // ── New tests for expanded compiler coverage ─────────────────────────────

    #[test]
    fn compile_e_scan() {
        let mut compiler = GremlinCompiler::new();
        let op = compiler.compile(&[GremlinStep::E(None)]).unwrap();
        // E(None) → Expand (all edges) over NodeScan
        assert!(matches!(op, Operator::Expand { .. }));
        if let Operator::Expand {
            child, rel_types, ..
        } = &op
        {
            assert!(rel_types.is_empty());
            assert!(matches!(**child, Operator::NodeScan { .. }));
        }
    }

    #[test]
    fn compile_e_with_single_id() {
        let mut compiler = GremlinCompiler::new();
        let op = compiler
            .compile(&[GremlinStep::E(Some(vec![GremlinValue::Integer(42)]))])
            .unwrap();
        assert!(matches!(op, Operator::Filter { .. }));
        if let Operator::Filter { child, .. } = &op {
            assert!(matches!(**child, Operator::Expand { .. }));
        }
    }

    #[test]
    fn compile_e_with_multiple_ids() {
        let mut compiler = GremlinCompiler::new();
        let op = compiler
            .compile(&[GremlinStep::E(Some(vec![
                GremlinValue::Integer(1),
                GremlinValue::Integer(2),
            ]))])
            .unwrap();
        assert!(matches!(op, Operator::Filter { .. }));
    }

    #[test]
    fn compile_has_existence() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Has("age".into(), None)];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Filter { .. }));
        if let Operator::Filter { predicate, .. } = &op {
            assert!(matches!(predicate, Expr::IsNotNull(_)));
        }
    }

    #[test]
    fn compile_has_id_single() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::HasId(vec![GremlinValue::Integer(7)]),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Filter { .. }));
        if let Operator::Filter { predicate, .. } = &op {
            assert!(matches!(
                predicate,
                Expr::BinaryOp {
                    op: BinaryOp::Eq,
                    ..
                }
            ));
        }
    }

    #[test]
    fn compile_has_id_multiple() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::HasId(vec![
                GremlinValue::Integer(1),
                GremlinValue::Integer(2),
                GremlinValue::Integer(3),
            ]),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Filter { .. }));
        if let Operator::Filter { predicate, .. } = &op {
            assert!(matches!(
                predicate,
                Expr::BinaryOp {
                    op: BinaryOp::In,
                    ..
                }
            ));
        }
    }

    #[test]
    fn compile_has_label_multiple() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::HasLabel(vec!["person".into(), "software".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Filter { .. }));
        if let Operator::Filter { predicate, .. } = &op {
            // Multiple labels → OR of HasLabel checks
            assert!(matches!(
                predicate,
                Expr::BinaryOp {
                    op: BinaryOp::Or,
                    ..
                }
            ));
        }
    }

    #[test]
    fn compile_both_expand() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Both(vec!["knows".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(
            op,
            Operator::Expand {
                direction: Direction::Undirected,
                ..
            }
        ));
    }

    #[test]
    fn compile_out_e() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::OutE(vec!["created".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Expand {
            direction,
            rel_alias,
            ..
        } = &op
        {
            assert_eq!(*direction, Direction::Outgoing);
            assert!(rel_alias.is_some(), "outE should set rel_alias");
        } else {
            panic!("expected Expand");
        }
    }

    #[test]
    fn compile_in_e() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::InE(vec!["knows".into()])];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Expand {
            direction,
            rel_alias,
            ..
        } = &op
        {
            assert_eq!(*direction, Direction::Incoming);
            assert!(rel_alias.is_some());
        } else {
            panic!("expected Expand");
        }
    }

    #[test]
    fn compile_both_e() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::BothE(vec![])];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Expand {
            direction,
            rel_alias,
            ..
        } = &op
        {
            assert_eq!(*direction, Direction::Undirected);
            assert!(rel_alias.is_some());
        } else {
            panic!("expected Expand");
        }
    }

    #[test]
    fn compile_in_v() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::OutE(vec![]),
            GremlinStep::InV,
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Project { .. }));
    }

    #[test]
    fn compile_out_v() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::OutE(vec![]),
            GremlinStep::OutV,
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Project { .. }));
    }

    #[test]
    fn compile_values_multiple_keys() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Values(vec!["name".into(), "age".into(), "city".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        // Multiple keys: Project (with gremlin_values_keys) → Unwind
        if let Operator::Unwind { child, .. } = &op {
            assert!(matches!(**child, Operator::Project { .. }));
        } else {
            panic!("expected Unwind(Project)");
        }
    }

    #[test]
    fn compile_value_map_with_keys() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::ValueMap(vec!["name".into(), "age".into()], 0),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Project { items, .. } = &op {
            // Single gremlin_valuemap call with element + key args
            assert_eq!(items.len(), 1);
            assert!(matches!(&items[0].0, Expr::FunctionCall { .. }));
        } else {
            panic!("expected Project");
        }
    }

    #[test]
    fn compile_value_map_empty() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::ValueMap(vec![], 0)];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Project { items, .. } = &op {
            assert_eq!(items.len(), 1);
            assert!(matches!(&items[0].0, Expr::FunctionCall { .. }));
        } else {
            panic!("expected Project");
        }
    }

    #[test]
    fn compile_properties_with_keys() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Properties(vec!["name".into(), "age".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        // Multi-key properties() produces Unwind { Project { list_compact(...) } }
        assert!(matches!(&op, Operator::Unwind { .. }), "expected Unwind");
    }

    #[test]
    fn compile_id_step() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Id];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Project { items, .. } = &op {
            assert_eq!(items.len(), 1);
            assert!(matches!(
                &items[0].0,
                Expr::FunctionCall { name, .. } if name.as_str() == "id"
            ));
        } else {
            panic!("expected Project");
        }
    }

    #[test]
    fn compile_label_step() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Label];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Project { items, .. } = &op {
            assert_eq!(items.len(), 1);
            assert!(matches!(
                &items[0].0,
                Expr::FunctionCall { name, .. } if name.as_str() == "gremlin_single_label"
            ));
        } else {
            panic!("expected Project");
        }
    }

    #[test]
    fn compile_sum() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Values(vec!["age".into()]),
            GremlinStep::Sum,
        ];
        let op = compiler.compile(&steps).unwrap();
        // Sum now uses Filter { Project { gremlin_sum_local, Aggregate(Collect) } }
        assert!(matches!(op, Operator::Filter { .. }));
    }

    #[test]
    fn compile_min() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Values(vec!["age".into()]),
            GremlinStep::Min,
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Filter { child, .. } = &op {
            if let Operator::Aggregate { aggregations, .. } = child.as_ref() {
                assert!(matches!(aggregations[0].0, AggFunction::Min));
            } else {
                panic!("expected Aggregate inside Filter");
            }
        } else {
            panic!("expected Filter");
        }
    }

    #[test]
    fn compile_max() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Values(vec!["age".into()]),
            GremlinStep::Max,
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Filter { child, .. } = &op {
            if let Operator::Aggregate { aggregations, .. } = child.as_ref() {
                assert!(matches!(aggregations[0].0, AggFunction::Max));
            } else {
                panic!("expected Aggregate inside Filter");
            }
        } else {
            panic!("expected Filter");
        }
    }

    #[test]
    fn compile_mean() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Values(vec!["age".into()]),
            GremlinStep::Mean,
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Filter { child, .. } = &op {
            if let Operator::Aggregate { aggregations, .. } = child.as_ref() {
                assert!(matches!(aggregations[0].0, AggFunction::Avg));
            } else {
                panic!("expected Aggregate inside Filter");
            }
        } else {
            panic!("expected Filter");
        }
    }

    #[test]
    fn compile_group() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Group(None)];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::GremlinGroup { agg_fn, .. } = &op {
            assert!(matches!(agg_fn, AggFunction::Collect));
        } else {
            panic!("expected GremlinGroup");
        }
    }

    #[test]
    fn compile_group_count() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::GroupCount(None)];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::GremlinGroup { agg_fn, .. } = &op {
            assert!(matches!(agg_fn, AggFunction::Count));
        } else {
            panic!("expected GremlinGroup");
        }
    }

    #[test]
    fn compile_fold() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Fold];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Aggregate {
            keys, aggregations, ..
        } = &op
        {
            assert!(keys.is_empty(), "fold has no group keys");
            assert!(matches!(aggregations[0].0, AggFunction::Collect));
        } else {
            panic!("expected Aggregate");
        }
    }

    #[test]
    fn compile_unfold() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Fold, GremlinStep::Unfold];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Unwind { .. }));
    }

    #[test]
    fn compile_as_select() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::As("a".into()),
            GremlinStep::Out(vec!["knows".into()]),
            GremlinStep::As("b".into()),
            GremlinStep::Select(vec!["a".into(), "b".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::GremlinSelect { keys, .. } = &op {
            assert_eq!(keys.len(), 2);
            assert_eq!(keys[0].1, "a");
            assert_eq!(keys[1].1, "b");
        } else {
            panic!("expected GremlinSelect");
        }
    }

    #[test]
    fn compile_where_filter() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Where(vec![GremlinStep::Has(
                "age".into(),
                Some(GremlinValue::Integer(30)),
            )]),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Filter { .. }));
    }

    #[test]
    fn compile_not_filter() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Not(vec![GremlinStep::Has(
                "deleted".into(),
                Some(GremlinValue::Bool(true)),
            )]),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Filter { predicate, .. } = &op {
            assert!(matches!(
                predicate,
                Expr::UnaryOp {
                    op: gql_core::ast::UnaryOp::Not,
                    ..
                }
            ));
        } else {
            panic!("expected Filter");
        }
    }

    #[test]
    fn compile_and_filter() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::And(vec![
                vec![GremlinStep::Has(
                    "name".into(),
                    Some(GremlinValue::String("marko".into())),
                )],
                vec![GremlinStep::Has(
                    "age".into(),
                    Some(GremlinValue::Integer(29)),
                )],
            ]),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Filter { predicate, .. } = &op {
            assert!(matches!(
                predicate,
                Expr::BinaryOp {
                    op: BinaryOp::And,
                    ..
                }
            ));
        } else {
            panic!("expected Filter");
        }
    }

    #[test]
    fn compile_or_filter() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Or(vec![
                vec![GremlinStep::HasLabel(vec!["person".into()])],
                vec![GremlinStep::HasLabel(vec!["software".into()])],
            ]),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Filter { predicate, .. } = &op {
            assert!(matches!(
                predicate,
                Expr::BinaryOp {
                    op: BinaryOp::Or,
                    ..
                }
            ));
        } else {
            panic!("expected Filter");
        }
    }

    #[test]
    fn compile_is_predicate() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Values(vec!["age".into()]),
            GremlinStep::Is(Predicate::Gt(GremlinValue::Integer(25))),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Filter { predicate, .. } = &op {
            assert!(matches!(
                predicate,
                Expr::BinaryOp {
                    op: BinaryOp::Gt,
                    ..
                }
            ));
        } else {
            panic!("expected Filter");
        }
    }

    #[test]
    fn compile_constant() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::Constant(GremlinValue::Integer(42))];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Project { items, .. } = &op {
            assert_eq!(items.len(), 1);
            assert!(matches!(&items[0].0, Expr::Integer(42)));
        } else {
            panic!("expected Project");
        }
    }

    #[test]
    fn compile_inject() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::Inject(vec![
            GremlinValue::Integer(1),
            GremlinValue::Integer(2),
            GremlinValue::Integer(3),
        ])];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Unwind { .. }));
        if let Operator::Unwind { list_expr, .. } = &op {
            if let Expr::List(items) = list_expr {
                assert_eq!(items.len(), 3);
            } else {
                panic!("expected List expr");
            }
        }
    }

    #[test]
    fn compile_union() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Union(vec![
                vec![GremlinStep::V(None), GremlinStep::Out(vec!["knows".into()])],
                vec![
                    GremlinStep::V(None),
                    GremlinStep::Out(vec!["created".into()]),
                ],
            ]),
        ];
        let op = compiler.compile(&steps).unwrap();
        // Union is now wrapped in Apply for parent-variable propagation
        assert!(matches!(op, Operator::Apply { .. }));
    }

    #[test]
    fn compile_repeat_times() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Repeat(vec![
                GremlinStep::V(None),
                GremlinStep::Out(vec!["knows".into()]),
            ]),
            GremlinStep::Times(3),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Repeat { times, body, .. } = &op {
            assert_eq!(*times, Some(3));
            assert!(matches!(**body, Operator::Expand { .. }));
        } else {
            panic!("expected Repeat");
        }
    }

    #[test]
    fn compile_choose() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Choose(
                vec![
                    GremlinStep::V(None),
                    GremlinStep::Has("age".into(), Some(GremlinValue::Integer(30))),
                ],
                vec![
                    GremlinStep::V(None),
                    GremlinStep::Values(vec!["name".into()]),
                ],
                Some(vec![GremlinStep::Constant(GremlinValue::String(
                    "unknown".into(),
                ))]),
            ),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Choose {
            if_true, if_false, ..
        } = &op
        {
            // values() now wraps Project in Filter (null filtering)
            assert!(matches!(
                **if_true,
                Operator::Filter { .. } | Operator::Project { .. }
            ));
            assert!(if_false.is_some());
        } else {
            panic!("expected Choose");
        }
    }

    #[test]
    fn compile_coalesce() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Coalesce(vec![
                vec![
                    GremlinStep::V(None),
                    GremlinStep::Values(vec!["nickname".into()]),
                ],
                vec![
                    GremlinStep::V(None),
                    GremlinStep::Values(vec!["name".into()]),
                ],
            ]),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Coalesce { alternatives, .. } = &op {
            assert_eq!(alternatives.len(), 2);
        } else {
            panic!("expected Coalesce");
        }
    }

    #[test]
    fn compile_order_default_sort() {
        // Order without By → default ascending sort on current alias
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Order];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Sort { items, .. } = &op {
            assert_eq!(items.len(), 1);
            assert!(items[0].1, "default sort should be ascending");
        } else {
            panic!("expected Sort");
        }
    }

    #[test]
    fn compile_v_multiple_ids() {
        let mut compiler = GremlinCompiler::new();
        let op = compiler
            .compile(&[GremlinStep::V(Some(vec![
                GremlinValue::Integer(1),
                GremlinValue::Integer(2),
                GremlinValue::Integer(3),
            ]))])
            .unwrap();
        if let Operator::Filter { predicate, .. } = &op {
            assert!(matches!(
                predicate,
                Expr::BinaryOp {
                    op: BinaryOp::In,
                    ..
                }
            ));
        } else {
            panic!("expected Filter for multi-id V()");
        }
    }

    #[test]
    fn compile_path_produces_gremlin_path() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Path];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::GremlinPath { .. }));
    }

    #[test]
    fn compile_match_produces_gremlin_match() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Match(vec![vec![GremlinStep::Out(vec!["knows".into()])]]),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::GremlinMatch { .. }));
    }

    #[test]
    fn compile_aggregate_produces_gremlin_aggregate() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Aggregate("x".into())];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::GremlinAggregate { .. }));
    }

    #[test]
    fn compile_add_e() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::AddV("person".into()),
            GremlinStep::AddE("knows".into()),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::Create { .. }));
    }

    #[test]
    fn compile_add_e_to_label() {
        // g.V(1).as("a").out("created").addE("createdBy").to("a")
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(Some(vec![GremlinValue::Integer(1)])),
            GremlinStep::As("a".into()),
            GremlinStep::Out(vec!["created".into()]),
            GremlinStep::AddE("createdBy".into()),
            GremlinStep::To(vec![GremlinStep::Select(vec!["a".into()])]),
        ];
        let op = compiler.compile(&steps).unwrap();
        // Verify Create operator has non-placeholder src/dst
        fn check_create(op: &Operator) -> bool {
            match op {
                Operator::Create { patterns, .. } => {
                    for pat in patterns {
                        for edge in &pat.edges {
                            if edge.1.as_str() == "__src" || edge.2.as_str() == "__dst" {
                                return false;
                            }
                        }
                    }
                    true
                }
                Operator::Apply { outer, inner } => check_create(outer) && check_create(inner),
                Operator::Set { child, .. } => check_create(child),
                _ => true,
            }
        }
        assert!(
            check_create(&op),
            "Create operator still has __src or __dst placeholders"
        );
    }

    #[test]
    fn compile_property_set() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Property("status".into(), GremlinValue::String("active".into())),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Set { items, .. } = &op {
            assert_eq!(items.len(), 1);
            if let SetItem::Property { key, .. } = &items[0] {
                assert_eq!(key, "status");
            } else {
                panic!("expected SetItem::Property");
            }
        } else {
            panic!("expected Set");
        }
    }

    #[test]
    fn compile_limit() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Limit(5)];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Limit { count, .. } = &op {
            assert!(matches!(count, Expr::Integer(5)));
        } else {
            panic!("expected Limit");
        }
    }

    #[test]
    fn compile_range_zero_start() {
        // range(0, 10) → Limit only (no Skip needed)
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::Range(0, 10)];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Limit { count, child } = &op {
            assert!(matches!(count, Expr::Integer(10)));
            // No skip wrapping since low=0
            assert!(matches!(**child, Operator::NodeScan { .. }));
        } else {
            panic!("expected Limit");
        }
    }

    #[test]
    fn compile_side_effect() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::SideEffect(vec![
                GremlinStep::V(None),
                GremlinStep::Property("visited".into(), GremlinValue::Bool(true)),
            ]),
        ];
        let op = compiler.compile(&steps).unwrap();
        assert!(matches!(op, Operator::GremlinSideEffect { .. }));
    }

    #[test]
    fn compile_cap() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Cap(vec!["x".into(), "y".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Cap { keys, .. } = &op {
            assert_eq!(keys, &["x", "y"]);
        } else {
            panic!("expected Cap");
        }
    }

    #[test]
    fn compile_project_step() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Project(vec!["name".into(), "age".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Project { items, .. } = &op {
            assert_eq!(items.len(), 2);
        } else {
            panic!("expected Project");
        }
    }

    #[test]
    fn compile_predicate_at_toplevel_errors() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::Eq(GremlinValue::Integer(1)),
        ];
        let err = compiler.compile(&steps).unwrap_err();
        assert!(matches!(err, CompileError::InvalidSequence(_)));
    }

    #[test]
    fn compile_project_by_traversal() {
        let no_param = |_: &str| -> Option<GremlinValue> { None };
        let steps =
            crate::text_parser::parse_inner_traversal("__.inE().count()", &no_param).unwrap();
        eprintln!("Parsed sub-traversal steps: {steps:?}");
        assert!(!steps.is_empty(), "Should parse to non-empty steps");

        let input = "g.V().project(\"a\", \"b\").by(__.inE().count()).by(\"age\")";
        let full = crate::text_parser::parse_traversal(input, &no_param).unwrap();
        eprintln!("Full parsed steps: {full:?}");

        let mut compiler = GremlinCompiler::new();
        let op = compiler.compile(&full).unwrap();
        eprintln!("Compiled operator: {op:?}");
        // Should be GremlinProject, not just Project
        let op_str = format!("{op:?}");
        assert!(
            op_str.contains("GremlinProject"),
            "Expected GremlinProject, got: {op_str}"
        );
    }

    #[test]
    fn compile_format_by_traversal() {
        let no_param = |_: &str| -> Option<GremlinValue> { None };
        let input = r#"g.V().format("%{name} has %{_} connections").by(bothE().count())"#;
        let full = crate::text_parser::parse_traversal(input, &no_param).unwrap();
        let mut compiler = GremlinCompiler::new();
        let op = compiler.compile(&full).unwrap();
        let op_str = format!("{op:?}");
        assert!(
            op_str.contains("gremlin_format_by"),
            "Expected gremlin_format_by in compiled plan"
        );
    }
}
