//! Gremlin-to-Operator compiler.
//!
//! `GremlinCompiler` takes a `Vec<GremlinStep>` and builds a `gql_core::planner::Operator`
//! tree by walking the steps left-to-right, wrapping each subsequent step around the
//! previous operator (bottom-up construction).

use std::collections::HashMap;

use gql_core::ast::{BinaryOp, Direction, Expr, Identifier, Label, RelType};
use gql_core::planner::{AggFunction, CreateEdgeSpec, CreatePattern, Operator, SetItem};
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
        }
    }

    /// Absorb counter state from a sub-compiler after it finishes.
    fn absorb(&mut self, sub: &Self) {
        self.alias_counter = sub.alias_counter;
    }

    /// Compile a step sequence into an Operator tree.
    pub fn compile(&mut self, steps: &[GremlinStep]) -> Result<Operator, CompileError> {
        if steps.is_empty() {
            return Ok(Operator::SingleRow);
        }

        let mut current: Option<Operator> = None;
        let mut current_alias = Identifier::new("__gremlin_v");
        // Pending properties for AddV/AddE
        let mut pending_properties: Vec<(String, GremlinValue)> = Vec::new();
        // Pending By modifiers for Order
        let mut pending_order = false;

        for step in steps {
            match step {
                GremlinStep::V(ids) => {
                    let alias = self.next_alias("v");
                    current_alias = alias.clone();
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
                }

                GremlinStep::Values(keys) => {
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
                    let items: Vec<(Expr, Option<Identifier>)> = keys
                        .iter()
                        .map(|k| {
                            let var = self
                                .alias_map
                                .get(k)
                                .cloned()
                                .unwrap_or_else(|| Identifier::new(k.clone()));
                            (Expr::Variable(var), Some(Identifier::new(k.clone())))
                        })
                        .collect();
                    current = Some(Operator::Project {
                        items,
                        child: Box::new(child),
                    });
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

                GremlinStep::Group | GremlinStep::GroupCount => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let alias = self.next_alias("grp");
                    let agg_fn = if matches!(step, GremlinStep::GroupCount) {
                        AggFunction::Count
                    } else {
                        AggFunction::Collect
                    };
                    current = Some(Operator::Aggregate {
                        keys: vec![Expr::Variable(current_alias.clone())],
                        aggregations: vec![(
                            agg_fn,
                            Expr::Variable(current_alias.clone()),
                            None,
                            false,
                            alias.clone(),
                        )],
                        child: Box::new(child),
                    });
                    current_alias = alias;
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
                        let sort_expr = Expr::Property(
                            Box::new(Expr::Variable(current_alias.clone())),
                            SmolStr::new(key),
                        );
                        current = Some(Operator::Sort {
                            items: vec![(sort_expr, !descending)], // Sort uses ascending=true
                            child: Box::new(child),
                        });
                        pending_order = false;
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

                GremlinStep::Dedup(_keys) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    current = Some(Operator::Distinct {
                        child: Box::new(child),
                    });
                }

                GremlinStep::Where(inner) | GremlinStep::Not(inner) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let inner_pred = step_to_predicate(inner, &current_alias);
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
                    return Err(CompileError::UnsupportedStep("path".into()));
                }

                GremlinStep::Repeat(body_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let body = sub.compile(body_steps)?;
                    self.absorb(&sub);
                    current = Some(Operator::Repeat {
                        body: Box::new(body),
                        times: None,
                        until: None,
                        emit: false,
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
                    // Modify preceding Repeat
                    if let Some(Operator::Repeat { ref mut until, .. }) = current {
                        let mut sub = self.sub_compiler();
                        let until_op = sub.compile(until_steps)?;
                        self.absorb(&sub);
                        *until = Some(Box::new(until_op));
                    }
                }

                GremlinStep::Emit => {
                    if let Some(Operator::Repeat { ref mut emit, .. }) = current {
                        *emit = true;
                    }
                }

                GremlinStep::Choose(pred_steps, true_steps, false_steps) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let pred_op = sub.compile(pred_steps)?;
                    let true_op = sub.compile(true_steps)?;
                    let false_op = match false_steps {
                        Some(steps) => Some(Box::new(sub.compile(steps)?)),
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

                GremlinStep::Coalesce(alternatives) => {
                    let child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let alt_ops: Result<Vec<Operator>, _> =
                        alternatives.iter().map(|s| sub.compile(s)).collect();
                    self.absorb(&sub);
                    current = Some(Operator::Coalesce {
                        alternatives: alt_ops?,
                        child: Box::new(child),
                    });
                }

                GremlinStep::Union(branches) => {
                    let _child = current.take().ok_or(CompileError::MissingChild)?;
                    let mut sub = self.sub_compiler();
                    let branch_ops: Result<Vec<Operator>, _> =
                        branches.iter().map(|s| sub.compile(s)).collect();
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
                    let effect = sub.compile(effect_steps)?;
                    self.absorb(&sub);
                    current = Some(Operator::GremlinSideEffect {
                        effect: Box::new(effect),
                        child: Box::new(child),
                    });
                }

                GremlinStep::Aggregate(_key) => {
                    return Err(CompileError::UnsupportedStep(
                        "aggregate (side-effect)".into(),
                    ));
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

                GremlinStep::Match(_patterns) => {
                    return Err(CompileError::UnsupportedStep("match".into()));
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
                    items: vec![(Expr::Variable(current_alias), true)],
                    child: Box::new(child),
                });
            }
        }

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
        _ => Expr::Bool(true), // fallback for complex sub-traversals
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::GremlinStep;
    use crate::types::GremlinValue;

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
}
