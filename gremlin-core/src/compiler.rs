//! Gremlin-to-Operator compiler.
//!
//! `GremlinCompiler` takes a `Vec<GremlinStep>` and builds a `gql_core::planner::Operator`
//! tree by walking the steps left-to-right, wrapping each subsequent step around the
//! previous operator (bottom-up construction).

use std::collections::HashMap;

use gql_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType};
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
}

/// Compiles a sequence of Gremlin steps into a gql-core `Operator` tree.
pub struct GremlinCompiler {
    alias_counter: u32,
    /// Maps Gremlin `as()` labels to their operator-level aliases.
    alias_map: HashMap<String, Identifier>,
    /// The alias that the most recent compilation left as the "current" traverser.
    last_alias: Identifier,
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
    fn absorb(&mut self, sub: &Self) {
        self.alias_counter = sub.alias_counter;
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

        let mut current: Option<Operator> = initial_op;
        let mut current_alias = initial_alias.clone();
        // Pending properties for AddV/AddE
        let mut pending_properties: Vec<(String, GremlinValue)> = Vec::new();
        // Pending By modifiers for Order
        let mut pending_order = false;
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
        let mut pending_until: Option<Vec<GremlinStep>> = None;
        let mut pending_times: Option<usize> = None;
        let mut pending_sack_init: Option<Expr> = None;
        // Track whether SackOp just ran, so that a following By() can rewrite the rhs
        let mut pending_sack_op = false;

        for step in steps {
            // Reset aggregate by-consumed flag on any non-By step
            if !matches!(step, GremlinStep::By(..)) {
                aggregate_by_consumed = false;
                pending_sack_op = false;
                // Apply collected select by() modulations cyclically
                if pending_select && !select_by_mods.is_empty() {
                    if let Some(Operator::GremlinSelect { ref mut keys, .. }) = current {
                        for (i, (src_var, _)) in keys.iter_mut().enumerate() {
                            let mod_key = &select_by_mods[i % select_by_mods.len()];
                            if !mod_key.is_empty() {
                                *src_var = Identifier::new(format!(
                                    "__prop_{}__{}", mod_key, src_var.0
                                ));
                            }
                        }
                    }
                }
                pending_select = false;
            }
            match step {
                GremlinStep::V(ids) => {
                    let alias = self.next_alias("v");
                    path_aliases.push(alias.clone());
                    let scan = Operator::NodeScan {
                        label: None,
                        alias: alias.clone(),
                    };
                    let new_op = match ids {
                        Some(id_list) if id_list.len() == 1 => {
                            let id_expr = gremlin_value_to_expr(&id_list[0]);
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
                                id_list.iter().map(gremlin_value_to_expr).collect();
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
                    current = Some(match ids {
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
                    });
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
                    let predicate = if ids.len() == 1 {
                        Expr::BinaryOp {
                            op: BinaryOp::Eq,
                            left: Box::new(id_fn),
                            right: Box::new(gremlin_value_to_expr(&ids[0])),
                        }
                    } else {
                        let id_exprs: Vec<Expr> = ids.iter().map(gremlin_value_to_expr).collect();
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

                GremlinStep::Out(labels) | GremlinStep::In(labels) | GremlinStep::Both(labels) => {
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

                GremlinStep::InV | GremlinStep::OutV | GremlinStep::BothV | GremlinStep::OtherV => {
                    // These resolve to the target/source vertex of an edge traversal.
                    // Represented as a project extracting the vertex from the edge context.
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let vertex_alias = self.next_alias("v");
                    let fn_name = match step {
                        GremlinStep::InV => "endNode",
                        GremlinStep::OutV => "startNode",
                        _ => "endNode",
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
                        // Multiple keys: unwind values for each key, skip nulls
                        let _val_alias = self.next_alias("val");
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
                        items.extend(carry);
                        current = Some(Operator::Project {
                            items,
                            child: Box::new(child),
                        });
                        // For multi-value, keep current_alias (it's a multi-column result)
                    }
                }

                GremlinStep::ValueMap(keys, with_tokens) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("vmap");
                    // Build args: element, tokens flag, then optional key filters
                    let mut args = vec![
                        Expr::Variable(current_alias.clone()),
                        Expr::Bool(*with_tokens),
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
                }

                GremlinStep::Properties(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let prop_alias = self.next_alias("prop");
                    let carry = self.label_carry_items();
                    if keys.is_empty() {
                        // properties() with no keys: pass through the element
                        let mut items = vec![(
                            Expr::Variable(current_alias.clone()),
                            Some(prop_alias.clone()),
                        )];
                        items.extend(carry);
                        current = Some(Operator::Project {
                            items,
                            child: Box::new(child),
                        });
                    } else if keys.len() == 1 {
                        let mut items = vec![(
                            Expr::Property(
                                Box::new(Expr::Variable(current_alias.clone())),
                                SmolStr::new(&keys[0]),
                            ),
                            Some(prop_alias.clone()),
                        )];
                        items.extend(carry);
                        current = Some(Operator::Filter {
                            predicate: Expr::IsNotNull(Box::new(Expr::Variable(
                                prop_alias.clone(),
                            ))),
                            child: Box::new(Operator::Project {
                                items,
                                child: Box::new(child),
                            }),
                        });
                    } else {
                        // Multiple keys: build a list of values and unwind
                        let list_alias = self.next_alias("plist");
                        let list_exprs: Vec<Expr> = keys
                            .iter()
                            .map(|k| {
                                Expr::Property(
                                    Box::new(Expr::Variable(current_alias.clone())),
                                    SmolStr::new(k),
                                )
                            })
                            .collect();
                        let mut items = vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("list_compact"),
                                args: list_exprs,
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
                            name: SmolStr::new("labels"),
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
                        }
                        current = Some(Operator::GremlinSelect {
                            keys: select_keys,
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
                    // Filter out null (empty list produces null sum)
                    current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(sum_alias.clone()))),
                        child: Box::new(projected),
                    });
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
                    current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(agg),
                    });
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
                    current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(agg),
                    });
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
                    current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(agg),
                    });
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
                    current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(proj),
                    });
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
                    current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(proj),
                    });
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
                    current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(proj),
                    });
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
                    current = Some(Operator::Filter {
                        predicate: Expr::IsNotNull(Box::new(Expr::Variable(alias.clone()))),
                        child: Box::new(proj),
                    });
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
                    if pending_order {
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
                        } else {
                            // Property-based sort: filter out elements missing the property
                            let prop_expr = Expr::Property(
                                Box::new(Expr::Variable(current_alias.clone())),
                                SmolStr::new(key),
                            );
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
                        pending_order = false;
                    } else if pending_group {
                        // by() after group()/groupCount() — modifies GremlinGroup.
                        // Use pre_group_alias (the alias before group() changed it).
                        if let Some(Operator::GremlinGroup {
                            ref mut key_expr,
                            ref mut value_expr,
                            ref mut agg_fn,
                            ..
                        }) = current
                        {
                            if pending_group_by_count == 0 {
                                // First by() = group key
                                if !key.is_empty() {
                                    let new_key = make_by_expr(key, &pre_group_alias);
                                    *key_expr = new_key;
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
                                    *agg_fn = AggFunction::Count;
                                } else if key == "fold()" || key == "__.fold()" {
                                    *agg_fn = AggFunction::Collect;
                                } else if key == "sum()" || key == "__.sum()" {
                                    *agg_fn = AggFunction::Sum;
                                } else if key.is_empty() {
                                    // by() with no args — identity (collect the element)
                                    *agg_fn = AggFunction::Collect;
                                } else {
                                    // by("propName") — collect property values
                                    *value_expr = make_by_expr(key, &pre_group_alias);
                                }
                                pending_group = false;
                            }
                        } else {
                            pending_group = false;
                        }
                    } else if pending_select {
                        // by() after select() — collect modulation
                        select_by_mods.push(key.clone());
                    } else if pending_dedup {
                        // by() after dedup() — dedup by specific property
                        if !key.is_empty() {
                            if let Some(Operator::Distinct { ref mut by_key, .. }) = current {
                                *by_key =
                                    Some(format!("__dedup_prop__{}__{}", key, current_alias.0));
                            }
                        }
                        pending_dedup = false;
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
                    } else if aggregate_by_consumed {
                        // Second by() after aggregate — error per Gremlin spec
                        return Err(CompileError::InvalidSequence(
                            "Aggregate step can only have one by modulator".into(),
                        ));
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
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::GremlinMapSlice {
                        alias: current_alias.clone(),
                        lo: *low as usize,
                        hi: *high as usize,
                        child: Box::new(child),
                    });
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

                    // Check whether inner steps are traversal steps (out/in/both/etc.)
                    // that need sub-traversal existence semantics.
                    let is_traversal = inner_steps.iter().any(|s| {
                        matches!(
                            s,
                            GremlinStep::Out(_)
                                | GremlinStep::In(_)
                                | GremlinStep::Both(_)
                                | GremlinStep::OutE(_)
                                | GremlinStep::InE(_)
                                | GremlinStep::BothE(_)
                                | GremlinStep::InV
                                | GremlinStep::OutV
                                | GremlinStep::BothV
                                | GremlinStep::V(_)
                                | GremlinStep::E(_)
                                | GremlinStep::Repeat(_)
                                | GremlinStep::Fold
                                | GremlinStep::Unfold
                                | GremlinStep::Count
                                | GremlinStep::Sum
                                | GremlinStep::Mean
                                | GremlinStep::Min
                                | GremlinStep::Max
                                | GremlinStep::Path
                        )
                    });

                    if is_traversal {
                        // Compile inner steps as a sub-traversal seeded with
                        // the current alias. Use Apply + Count + Filter to
                        // implement existence semantics.
                        let mut sub = self.sub_compiler();
                        let inner_op = sub.compile_seeded(inner_steps, &current_alias)?;
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
                            Expr::UnaryOp {
                                op: gql_core::ast::UnaryOp::Not,
                                expr: Box::new(inner_pred),
                            }
                        } else {
                            inner_pred
                        };
                        current = Some(Operator::Filter {
                            predicate,
                            child: Box::new(child),
                        });
                    }
                }

                GremlinStep::And(branches) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let preds: Vec<Expr> = branches
                        .iter()
                        .map(|branch| steps_to_predicate(branch, &current_alias, &self.alias_map))
                        .collect();
                    let predicate = preds
                        .into_iter()
                        .reduce(|a, b| Expr::BinaryOp {
                            op: BinaryOp::And,
                            left: Box::new(a),
                            right: Box::new(b),
                        })
                        .unwrap_or(Expr::Bool(true));
                    current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Or(branches) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let preds: Vec<Expr> = branches
                        .iter()
                        .map(|branch| steps_to_predicate(branch, &current_alias, &self.alias_map))
                        .collect();
                    let predicate = preds
                        .into_iter()
                        .reduce(|a, b| Expr::BinaryOp {
                            op: BinaryOp::Or,
                            left: Box::new(a),
                            right: Box::new(b),
                        })
                        .unwrap_or(Expr::Bool(false));
                    current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
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
                    pending_properties.clear();
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
                    pending_properties.push((key.clone(), value.clone()));
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Set {
                        items: vec![SetItem::Property {
                            alias: current_alias.clone(),
                            key: key.clone(),
                            value: gremlin_value_to_expr(value),
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
                                current = Some(Operator::Apply {
                                    outer: Box::new(child),
                                    inner: Box::new(scan),
                                });
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
                                    current = Some(Operator::Apply {
                                        outer: Box::new(child),
                                        inner: Box::new(inner_op),
                                    });
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
                            current = Some(Operator::Apply {
                                outer: Box::new(child),
                                inner: Box::new(inner_op),
                            });
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
                    });
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
                    current = Some(Operator::Repeat {
                        body: Box::new(body),
                        times: times_val,
                        until: until_val,
                        emit: emit_val,
                        remap,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Times(n) => {
                    // Modify preceding Repeat
                    if let Some(Operator::Repeat { ref mut times, .. }) = current {
                        *times = Some(*n as usize);
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
                    if let Some(Operator::Repeat { ref mut emit, .. }) = current {
                        *emit = true;
                    } else {
                        // emit() before repeat() — store as pending
                        pending_emit = true;
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
                        current = Some(Operator::ChooseMap {
                            selector: Box::new(selector),
                            selector_alias: sel_alias,
                            options: Vec::new(),
                            child: Box::new(child),
                        });
                    } else {
                        let pred_op = sub.compile_seeded(pred_steps, &current_alias)?;
                        let true_op = sub.compile_seeded(true_steps, &current_alias)?;
                        let false_op = match false_steps {
                            Some(steps) => {
                                Some(Box::new(sub.compile_seeded(steps, &current_alias)?))
                            }
                            None => None,
                        };
                        self.absorb(&sub);
                        current = Some(Operator::Choose {
                            predicate: Box::new(pred_op),
                            if_true: Box::new(true_op),
                            if_false: false_op,
                            child: Box::new(child),
                        });
                    }
                }

                GremlinStep::Coalesce(alternatives) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let alt_ops: Result<Vec<Operator>, _> = alternatives
                        .iter()
                        .map(|s| sub.compile_seeded(s, &current_alias))
                        .collect();
                    self.absorb(&sub);
                    current = Some(Operator::Coalesce {
                        alternatives: alt_ops?,
                        child: Box::new(child),
                    });
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
                    });
                    pending_aggregate = true;
                }

                GremlinStep::Cap(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Cap {
                        keys: keys.clone(),
                        child: Box::new(child),
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
                                sub.alias_map
                                    .insert(label.clone(), current_alias.clone());
                                sub.compile_with_seed(
                                    &pattern_steps[1..],
                                    current_alias.clone(),
                                )?
                            } else {
                                // Subsequent new label — create an explicit
                                // NodeScan with a known alias so the label maps
                                // to the correct column.
                                let scan_alias = sub.next_alias("v");
                                sub.alias_map
                                    .insert(label.clone(), scan_alias.clone());
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
                            sub.compile_with_seed(
                                pattern_steps,
                                current_alias.clone(),
                            )?
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
                }

                GremlinStep::Identity => {
                    // No-op: identity() passes through the current traverser unchanged.
                    if current.is_none() {
                        return Err(CompileError::MissingChild);
                    }
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
                    // elementMap includes id, label, plus selected properties
                    let mut items: Vec<(Expr, Option<Identifier>)> = vec![
                        (
                            Expr::FunctionCall {
                                name: SmolStr::new("id"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            },
                            Some(Identifier::new("~id")),
                        ),
                        (
                            Expr::FunctionCall {
                                name: SmolStr::new("labels"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            },
                            Some(Identifier::new("~label")),
                        ),
                    ];
                    if keys.is_empty() {
                        // All properties — represented as the element itself
                        items.push((
                            Expr::Variable(current_alias.clone()),
                            Some(current_alias.clone()),
                        ));
                    } else {
                        for k in keys {
                            items.push((
                                Expr::Property(
                                    Box::new(Expr::Variable(current_alias.clone())),
                                    SmolStr::new(k),
                                ),
                                Some(Identifier::new(k.clone())),
                            ));
                        }
                    }
                    items.extend(self.label_carry_items());
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasKey(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    // hasKey(k1, k2, ...) — all listed keys must exist (IS NOT NULL)
                    let predicates: Vec<Expr> = keys
                        .iter()
                        .map(|k| {
                            Expr::IsNotNull(Box::new(Expr::Property(
                                Box::new(Expr::Variable(current_alias.clone())),
                                SmolStr::new(k),
                            )))
                        })
                        .collect();
                    let predicate = predicates
                        .into_iter()
                        .reduce(|a, b| Expr::BinaryOp {
                            op: BinaryOp::And,
                            left: Box::new(a),
                            right: Box::new(b),
                        })
                        .unwrap_or(Expr::Bool(true));
                    current = Some(Operator::Filter {
                        predicate,
                        child: Box::new(child),
                    });
                }

                GremlinStep::HasValue(values) => {
                    // hasValue filters properties by value — simplified as passthrough filter
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    if values.len() == 1 {
                        current = Some(Operator::Filter {
                            predicate: Expr::BinaryOp {
                                op: BinaryOp::Eq,
                                left: Box::new(Expr::Variable(current_alias.clone())),
                                right: Box::new(gremlin_value_to_expr(&values[0])),
                            },
                            child: Box::new(child),
                        });
                    } else if !values.is_empty() {
                        let val_exprs: Vec<Expr> =
                            values.iter().map(gremlin_value_to_expr).collect();
                        current = Some(Operator::Filter {
                            predicate: Expr::BinaryOp {
                                op: BinaryOp::In,
                                left: Box::new(Expr::Variable(current_alias.clone())),
                                right: Box::new(Expr::List(val_exprs)),
                            },
                            child: Box::new(child),
                        });
                    } else {
                        current = Some(child);
                    }
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
                    // simplePath() — filter to non-cyclic paths (no repeated vertices)
                    // Without native path tracking, approximate as pass-through.
                    // The Gremlin path executor handles dedup-in-path semantics.
                    if current.is_none() {
                        return Err(CompileError::MissingChild);
                    }
                }

                GremlinStep::CyclicPath => {
                    // cyclicPath() — filter to paths with repeated elements
                    // Without native path tracking, approximate as always-false filter.
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Filter {
                        predicate: Expr::Bool(false),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Coin(probability) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Filter {
                        predicate: Expr::BinaryOp {
                            op: BinaryOp::Lt,
                            left: Box::new(Expr::FunctionCall {
                                name: SmolStr::new("rand"),
                                args: vec![],
                                distinct: false,
                            }),
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
                    current = Some(Operator::Filter {
                        predicate: pred,
                        child: Box::new(child),
                    });
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

                GremlinStep::Option(token, branch_steps) => {
                    if let Some(Operator::ChooseMap {
                        ref mut options, ..
                    }) = current
                    {
                        let mut sub = self.sub_compiler();
                        let branch_op = sub.compile_seeded(branch_steps, &current_alias)?;
                        self.absorb(&sub);
                        let match_val = match token {
                            GremlinValue::String(s) if s == "none" => None,
                            _ => Some(gremlin_value_to_value(token)),
                        };
                        options.push((match_val, Box::new(branch_op)));
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
                        // Sub-traversal: compile and wrap in Apply
                        let mut sub = self.sub_compiler();
                        let sub_op = sub.compile_seeded(trav, &current_alias)?;
                        let sub_alias = sub.last_alias.clone();
                        self.absorb(&sub);
                        base = Operator::Apply {
                            outer: Box::new(base),
                            inner: Box::new(sub_op),
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
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("fmt");
                    let mut items = vec![(
                        Expr::FunctionCall {
                            name: SmolStr::new("gremlin_format"),
                            args: vec![
                                Expr::Variable(current_alias.clone()),
                                Expr::Str(pattern.clone().into_boxed_str()),
                            ],
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
                            name: SmolStr::new("gremlin_asNumber"),
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

        // If Order was set but no By followed, apply default sort
        if pending_order {
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
                        *src_var = Identifier::new(format!(
                            "__prop_{}__{}", mod_key, src_var.0
                        ));
                    }
                }
            }
        }

        self.last_alias = current_alias;
        current.ok_or(CompileError::MissingChild)
    }
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
        | Operator::Coalesce { child, .. } => {
            inject_sack_init(child, init_expr);
        }
        Operator::Apply { outer, .. } => {
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
        | Operator::Unwind { .. }
        | Operator::Aggregate { .. } => {
            let original = std::mem::replace(op, Operator::SingleRow);
            *op = Operator::Apply {
                outer: Box::new(original),
                inner: Box::new(Operator::Project {
                    items: vec![(init_expr.clone(), Some(Identifier::new("__sack")))],
                    child: Box::new(Operator::SingleRow),
                }),
            };
        }
        _ => {}
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
            name: SmolStr::new("labels"),
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
            predicate_to_prop_expr(pred, prop_expr)
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
            if ids.len() == 1 {
                Expr::BinaryOp {
                    op: BinaryOp::Eq,
                    left: Box::new(id_fn),
                    right: Box::new(gremlin_value_to_expr(&ids[0])),
                }
            } else {
                let id_exprs: Vec<Expr> = ids.iter().map(gremlin_value_to_expr).collect();
                Expr::BinaryOp {
                    op: BinaryOp::In,
                    left: Box::new(id_fn),
                    right: Box::new(Expr::List(id_exprs)),
                }
            }
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
        let pred = step_to_predicate(step, alias);
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
        if let Operator::Project { items, .. } = &op {
            assert_eq!(items.len(), 3);
        } else {
            panic!("expected Project");
        }
    }

    #[test]
    fn compile_value_map_with_keys() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![
            GremlinStep::V(None),
            GremlinStep::ValueMap(vec!["name".into(), "age".into()], false),
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
        let steps = vec![GremlinStep::V(None), GremlinStep::ValueMap(vec![], false)];
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
                Expr::FunctionCall { name, .. } if name.as_str() == "labels"
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
}
