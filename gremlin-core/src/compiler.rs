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
        let mut current_alias = initial_alias;
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
        // Track whether the preceding step was dedup for by() modulation
        let mut pending_dedup = false;
        // Track whether the preceding step was aggregate for by() modulation
        let mut pending_aggregate = false;
        // Track aliases for path() reconstruction
        let mut path_aliases: Vec<Identifier> = initial_path_aliases;
        // Pending emit/until that appear before repeat()
        let mut pending_emit = false;
        let mut pending_until: Option<Vec<GremlinStep>> = None;

        for step in steps {
            match step {
                GremlinStep::V(ids) => {
                    let alias = self.next_alias("v");
                    current_alias = alias.clone();
                    path_aliases.push(current_alias.clone());
                    let scan = Operator::NodeScan {
                        label: None,
                        alias: alias.clone(),
                    };
                    current = Some(match ids {
                        Some(id_list) if id_list.len() == 1 => {
                            let id_expr = gremlin_value_to_expr(&id_list[0]);
                            Operator::Filter {
                                predicate: Expr::BinaryOp {
                                    op: BinaryOp::Eq,
                                    left: Box::new(Expr::FunctionCall {
                                        name: SmolStr::new("id"),
                                        args: vec![Expr::Variable(alias)],
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
                                        args: vec![Expr::Variable(alias)],
                                        distinct: false,
                                    }),
                                    right: Box::new(Expr::List(id_exprs)),
                                },
                                child: Box::new(scan),
                            }
                        }
                        _ => scan,
                    });
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
                    current = Some(Operator::Project {
                        items: vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new(fn_name),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            },
                            Some(vertex_alias.clone()),
                        )],
                        child: Box::new(child),
                    });
                    current_alias = vertex_alias;
                    path_aliases.push(current_alias.clone());
                }

                GremlinStep::Values(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    if keys.len() == 1 {
                        // Single key: project to a named alias
                        let alias = self.next_alias("val");
                        current = Some(Operator::Project {
                            items: vec![(
                                Expr::Property(
                                    Box::new(Expr::Variable(current_alias.clone())),
                                    SmolStr::new(&keys[0]),
                                ),
                                Some(alias.clone()),
                            )],
                            child: Box::new(child),
                        });
                        current_alias = alias;
                    } else {
                        let items: Vec<(Expr, Option<Identifier>)> = keys
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
                        current = Some(Operator::Project {
                            items,
                            child: Box::new(child),
                        });
                        // For multi-value, keep current_alias (it's a multi-column result)
                    }
                }

                GremlinStep::ValueMap(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let items: Vec<(Expr, Option<Identifier>)> = if keys.is_empty() {
                        vec![(
                            Expr::Variable(current_alias.clone()),
                            Some(current_alias.clone()),
                        )]
                    } else {
                        keys.iter()
                            .map(|k| {
                                (
                                    Expr::Property(
                                        Box::new(Expr::Variable(current_alias.clone())),
                                        SmolStr::new(k),
                                    ),
                                    Some(Identifier::new(k.clone())),
                                )
                            })
                            .collect()
                    };
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Properties(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let items: Vec<(Expr, Option<Identifier>)> = if keys.is_empty() {
                        vec![(
                            Expr::Variable(current_alias.clone()),
                            Some(current_alias.clone()),
                        )]
                    } else {
                        keys.iter()
                            .map(|k| {
                                (
                                    Expr::Property(
                                        Box::new(Expr::Variable(current_alias.clone())),
                                        SmolStr::new(k),
                                    ),
                                    Some(Identifier::new(k.clone())),
                                )
                            })
                            .collect()
                    };
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Id => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let id_alias = self.next_alias("id");
                    current = Some(Operator::Project {
                        items: vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("id"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            },
                            Some(id_alias.clone()),
                        )],
                        child: Box::new(child),
                    });
                    current_alias = id_alias;
                }

                GremlinStep::Label => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let lbl_alias = self.next_alias("lbl");
                    current = Some(Operator::Project {
                        items: vec![(
                            Expr::FunctionCall {
                                name: SmolStr::new("labels"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            },
                            Some(lbl_alias.clone()),
                        )],
                        child: Box::new(child),
                    });
                    current_alias = lbl_alias;
                }

                GremlinStep::As(alias) => {
                    self.alias_map.insert(alias.clone(), current_alias.clone());
                }

                GremlinStep::Select(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
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
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let sum_alias = self.next_alias("sum");
                    current = Some(Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Sum,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            sum_alias.clone(),
                        )],
                        child: Box::new(child),
                    });
                    current_alias = sum_alias;
                }

                GremlinStep::Min => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("min");
                    current = Some(Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Min,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Max => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("max");
                    current = Some(Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Max,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Mean => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("avg");
                    current = Some(Operator::Aggregate {
                        keys: vec![],
                        aggregations: vec![(
                            AggFunction::Avg,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Group(ref key) | GremlinStep::GroupCount(ref key) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("grp");
                    let agg_fn = if matches!(step, GremlinStep::GroupCount(..)) {
                        AggFunction::Count
                    } else {
                        AggFunction::Collect
                    };
                    current = Some(Operator::GremlinGroup {
                        key_expr: Expr::Variable(current_alias.clone()),
                        value_expr: Expr::Variable(current_alias.clone()),
                        agg_fn,
                        alias: alias.clone(),
                        side_effect_key: key.clone(),
                        child: Box::new(child),
                    });
                    current_alias = alias;
                    pending_group = true;
                    pending_group_by_count = 0;
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
                        let sort_expr = if key.is_empty() {
                            // by() with no key — sort by current value
                            Expr::Variable(current_alias.clone())
                        } else if key == "~id" || key == "T.id" || key == "t[id]" {
                            Expr::FunctionCall {
                                name: SmolStr::new("id"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            }
                        } else if key == "~label" || key == "T.label" || key == "t[label]" {
                            Expr::FunctionCall {
                                name: SmolStr::new("labels"),
                                args: vec![Expr::Variable(current_alias.clone())],
                                distinct: false,
                            }
                        } else {
                            Expr::Property(
                                Box::new(Expr::Variable(current_alias.clone())),
                                SmolStr::new(key),
                            )
                        };
                        current = Some(Operator::Sort {
                            items: vec![(sort_expr, !descending)],
                            child: Box::new(child),
                        });
                        pending_order = false;
                    } else if pending_group {
                        // by() after group()/groupCount() — modifies GremlinGroup
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
                                    let new_key = make_by_expr(key, &current_alias);
                                    *key_expr = new_key;
                                }
                                pending_group_by_count = 1;
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
                                    *value_expr = make_by_expr(key, &current_alias);
                                }
                                pending_group = false;
                            }
                        } else {
                            pending_group = false;
                        }
                    } else if pending_select {
                        // by() after select() — project selected values through property
                        if let Some(Operator::GremlinSelect { ref mut keys, .. }) = current {
                            if !key.is_empty() {
                                // Apply property projection to ALL select keys
                                // by("name") means project each selected value's "name" property
                                for (src_var, _) in keys.iter_mut() {
                                    // The src_var already points to the element; we need
                                    // to change it to a property-accessor alias. We'll
                                    // handle this by creating a synthetic alias that the
                                    // GremlinSelect executor interprets as property access.
                                    // For now, set a naming convention.
                                    *src_var =
                                        Identifier::new(format!("__prop_{}__{}", key, src_var.0));
                                }
                            }
                        }
                        pending_select = false;
                    } else if pending_dedup {
                        // by() after dedup() — dedup by specific property
                        if !key.is_empty() {
                            if let Some(Operator::Distinct { ref mut by_key, .. }) = current {
                                *by_key = Some(key.to_string());
                            }
                        }
                        pending_dedup = false;
                    } else if pending_aggregate {
                        // by() after aggregate() — project a property from each traverser
                        if let Some(Operator::GremlinAggregate {
                            ref mut by_key, ..
                        }) = current
                        {
                            if !key.is_empty() {
                                *by_key = Some(key.to_string());
                            }
                        }
                        pending_aggregate = false;
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

                GremlinStep::Dedup(_keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Distinct {
                        child: Box::new(child),
                        by_key: None,
                    });
                    pending_dedup = true;
                }

                GremlinStep::Where(inner_steps) | GremlinStep::Not(inner_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let inner_pred =
                        steps_to_predicate(inner_steps, &current_alias, &self.alias_map);
                    let predicate = if matches!(step, GremlinStep::Not(_)) {
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

                GremlinStep::And(steps_list) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let preds: Vec<Expr> = steps_list
                        .iter()
                        .map(|s| step_to_predicate(s, &current_alias))
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

                GremlinStep::Or(steps_list) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let preds: Vec<Expr> = steps_list
                        .iter()
                        .map(|s| step_to_predicate(s, &current_alias))
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
                    // Edge endpoint modifiers — in a full implementation these would
                    // modify the preceding AddE's CreateEdgeSpec. Simplified: no-op.
                    if current.is_none() {
                        return Err(CompileError::MissingChild);
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
                    current = Some(Operator::Repeat {
                        body: Box::new(body),
                        times: None,
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
                        self.absorb(&sub);
                        current = Some(Operator::ChooseMap {
                            selector: Box::new(selector),
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
                    let _child = current.take().unwrap_or(Operator::SingleRow);
                    let mut sub = self.sub_compiler();
                    let branch_ops: Result<Vec<Operator>, _> = branches
                        .iter()
                        .map(|s| sub.compile_seeded(s, &current_alias))
                        .collect();
                    self.absorb(&sub);
                    let branch_ops = branch_ops?;
                    // Build a left-associative union chain
                    let union = branch_ops
                        .into_iter()
                        .reduce(|left, right| Operator::Union {
                            left: Box::new(left),
                            right: Box::new(right),
                            all: true,
                        })
                        .unwrap_or(Operator::SingleRow);
                    current = Some(union);
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
                        by_key: None,
                    });
                    pending_aggregate = true;
                }

                GremlinStep::Cap(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Cap {
                        keys: keys.clone(),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Constant(val) => {
                    let child = current.take().unwrap_or(Operator::SingleRow);
                    let alias = self.next_alias("const");
                    current = Some(Operator::Project {
                        items: vec![(gremlin_value_to_expr(val), Some(alias.clone()))],
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Inject(values) => {
                    let exprs: Vec<Expr> = values.iter().map(gremlin_value_to_expr).collect();
                    let alias = self.next_alias("inj");
                    let unwind = Operator::Unwind {
                        list_expr: Expr::List(exprs),
                        alias: alias.clone(),
                        child: Box::new(current.take().unwrap_or(Operator::SingleRow)),
                    };
                    current = Some(unwind);
                    current_alias = alias;
                }

                GremlinStep::Match(patterns) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut compiled_patterns = Vec::with_capacity(patterns.len());
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
                            } else {
                                // New label — use V(None) to enumerate all options.
                                // Skip the As step: we'll assign __gremlin_v to this label.
                                sub.alias_map
                                    .insert(label.clone(), Identifier::new("__gremlin_v"));
                                let mut seeded = vec![GremlinStep::V(None)];
                                seeded.extend(pattern_steps[1..].iter().cloned());
                                sub.compile(&seeded)?
                            }
                        } else {
                            let mut seeded = vec![GremlinStep::V(None)];
                            seeded.extend(pattern_steps.iter().cloned());
                            sub.compile(&seeded)?
                        };

                        for (k, v) in &sub.alias_map {
                            self.alias_map.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                        self.absorb(&sub);
                        compiled_patterns.push(compiled);
                    }
                    // Build the label→alias mapping for output renaming.
                    let label_map: Vec<(String, Identifier)> = self
                        .alias_map
                        .iter()
                        .map(|(label, alias)| (label.clone(), alias.clone()))
                        .collect();
                    current = Some(Operator::GremlinMatch {
                        patterns: compiled_patterns,
                        label_map,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Project(keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let items: Vec<(Expr, Option<Identifier>)> = keys
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
                        child: Box::new(child),
                    };
                    let limited = Operator::Limit {
                        count: Expr::Integer(*n),
                        child: Box::new(reversed),
                    };
                    current = Some(Operator::Sort {
                        items: vec![(Expr::Variable(current_alias.clone()), true)],
                        child: Box::new(limited),
                    });
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
                    current = Some(Operator::Project {
                        items: vec![(Expr::Integer(0), Some(alias.clone()))],
                        child: Box::new(child),
                    });
                    current_alias = alias;
                }

                GremlinStep::Sack => {
                    // sack() with no args returns the sack value — identity passthrough
                    if current.is_none() {
                        current = Some(Operator::SingleRow);
                    }
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
                    child: Box::new(child),
                });
            }
        }

        self.last_alias = current_alias;
        current.ok_or(CompileError::MissingChild)
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
/// Handles T.id, T.label, Column.keys/values, and property names.
fn make_by_expr(key: &str, current_alias: &Identifier) -> Expr {
    if key == "~id" || key == "T.id" || key == "t[id]" {
        Expr::FunctionCall {
            name: SmolStr::new("id"),
            args: vec![Expr::Variable(current_alias.clone())],
            distinct: false,
        }
    } else if key == "~label" || key == "T.label" || key == "t[label]" {
        Expr::FunctionCall {
            name: SmolStr::new("labels"),
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

/// Convert a `Predicate` to an `Expr`.
fn predicate_to_expr(pred: &Predicate, alias: &Identifier) -> Expr {
    let var = Expr::Variable(alias.clone());
    match pred {
        Predicate::Eq(v) => Expr::BinaryOp {
            op: BinaryOp::Eq,
            left: Box::new(var),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Neq(v) => Expr::BinaryOp {
            op: BinaryOp::Neq,
            left: Box::new(var),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Gt(v) => Expr::BinaryOp {
            op: BinaryOp::Gt,
            left: Box::new(var),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Gte(v) => Expr::BinaryOp {
            op: BinaryOp::Gte,
            left: Box::new(var),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Lt(v) => Expr::BinaryOp {
            op: BinaryOp::Lt,
            left: Box::new(var),
            right: Box::new(gremlin_value_to_expr(v)),
        },
        Predicate::Lte(v) => Expr::BinaryOp {
            op: BinaryOp::Lte,
            left: Box::new(var),
            right: Box::new(gremlin_value_to_expr(v)),
        },
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
    }
}

/// Check if a predicate's argument is a string that corresponds to a step label
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
        // Top-level should be Project (values)
        assert!(matches!(op, Operator::Project { .. }));

        // Walk down to verify structure
        if let Operator::Project { child, items, .. } = &op {
            assert_eq!(items.len(), 1);
            assert!(matches!(**child, Operator::Expand { .. }));
            if let Operator::Expand { child, .. } = child.as_ref() {
                assert!(matches!(**child, Operator::Filter { .. }));
                if let Operator::Filter { child, .. } = child.as_ref() {
                    assert!(matches!(**child, Operator::NodeScan { .. }));
                }
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
            GremlinStep::ValueMap(vec!["name".into(), "age".into()]),
        ];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Project { items, .. } = &op {
            assert_eq!(items.len(), 2);
            // Each item should be a Property projection
            for (expr, _alias) in items {
                assert!(matches!(expr, Expr::Property(_, _)));
            }
        } else {
            panic!("expected Project");
        }
    }

    #[test]
    fn compile_value_map_empty() {
        let mut compiler = GremlinCompiler::new();
        let steps = vec![GremlinStep::V(None), GremlinStep::ValueMap(vec![])];
        let op = compiler.compile(&steps).unwrap();
        if let Operator::Project { items, .. } = &op {
            assert_eq!(items.len(), 1);
            assert!(matches!(&items[0].0, Expr::Variable(_)));
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
        if let Operator::Project { items, .. } = &op {
            assert_eq!(items.len(), 2);
        } else {
            panic!("expected Project");
        }
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
        if let Operator::Aggregate { aggregations, .. } = &op {
            assert_eq!(aggregations.len(), 1);
            assert!(matches!(aggregations[0].0, AggFunction::Sum));
        } else {
            panic!("expected Aggregate");
        }
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
        if let Operator::Aggregate { aggregations, .. } = &op {
            assert!(matches!(aggregations[0].0, AggFunction::Min));
        } else {
            panic!("expected Aggregate");
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
        if let Operator::Aggregate { aggregations, .. } = &op {
            assert!(matches!(aggregations[0].0, AggFunction::Max));
        } else {
            panic!("expected Aggregate");
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
        if let Operator::Aggregate { aggregations, .. } = &op {
            assert!(matches!(aggregations[0].0, AggFunction::Avg));
        } else {
            panic!("expected Aggregate");
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
                GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into()))),
                GremlinStep::Has("age".into(), Some(GremlinValue::Integer(29))),
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
                GremlinStep::HasLabel(vec!["person".into()]),
                GremlinStep::HasLabel(vec!["software".into()]),
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
        assert!(matches!(op, Operator::Union { all: true, .. }));
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
            assert!(matches!(**if_true, Operator::Project { .. }));
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
