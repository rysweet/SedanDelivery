//! Query planner — operator-type definitions and the QueryPlanner implementation.
//!
//! This module defines the complete physical plan operator set (33 operators)
//! and the QueryPlanner that translates a parsed Query AST into an Operator tree.

use crate::ast::{Direction, Expr, Identifier, Label, RelType};
use crate::config;
use crate::error::GqlError;

// ── Physical Plan Operators ───────────────────────────────────────────────────

/// Pop modifier for Gremlin select() — controls which labeled value to
/// return when a label has been assigned multiple times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GremlinPop {
    First,
    Last,
    All,
    Mixed,
}

#[derive(Debug, Clone)]
pub enum Operator {
    /// Produces exactly one empty row. Used as a leaf for bare `RETURN` (no MATCH).
    SingleRow,
    NodeScan {
        label: Option<Label>,
        alias: Identifier,
    },
    IndexScan {
        label: Label,
        property: String,
        /// When `Some`, the executor uses `RangeQuery::Eq(value)` for a precise
        /// index lookup. When `None`, all nodes with the property are returned
        /// (existence scan) and a downstream Filter handles the predicate.
        value: Option<Expr>,
        alias: Identifier,
    },
    NodeByIdSeek {
        id_expr: Expr,
        alias: Identifier,
    },
    Filter {
        predicate: Expr,
        child: Box<Operator>,
    },
    Expand {
        direction: Direction,
        rel_types: Vec<RelType>,
        src_alias: Identifier,
        rel_alias: Option<Identifier>,
        dst_alias: Identifier,
        child: Box<Operator>,
    },
    /// Invariant: min <= max; max <= DEFAULT_VARLEN_MAX_DEPTH when query had no upper bound.
    VarLenExpand {
        direction: Direction,
        rel_types: Vec<RelType>,
        min: u32,
        max: u32,
        src_alias: Identifier,
        path_alias: Option<Identifier>,
        /// When true, `path_alias` stores `Value::Path` (for AssemblePath named paths).
        /// When false, `path_alias` stores `Value::List` of edges (for user rel variables).
        path_mode: bool,
        dst_alias: Identifier,
        child: Box<Operator>,
        /// Optional inline property filter applied to each traversed edge during BFS.
        /// For `[:TYPE* {prop: val}]`, every edge in the path must satisfy this predicate.
        /// Uses an alias `__varlen_edge__` for the edge being tested.
        rel_props: Option<crate::ast::Expr>,
    },
    /// OPTIONAL MATCH: preserves left side rows even when right side matches nothing.
    LeftOuterApply {
        preserved: Box<Operator>,
        optional_pattern: Box<Operator>,
    },
    Project {
        items: Vec<(Expr, Option<Identifier>)>,
        child: Box<Operator>,
    },
    Aggregate {
        keys: Vec<Expr>,
        // (function, argument_expr, second_argument_expr, distinct, output_alias)
        aggregations: Vec<(AggFunction, Expr, Option<Expr>, bool, Identifier)>,
        child: Box<Operator>,
    },
    Distinct {
        child: Box<Operator>,
        /// When set, dedup by property value instead of full row (Gremlin `dedup().by(key)`).
        by_key: Option<String>,
        /// ProductiveByStrategy: keep traversers with missing dedup property
        /// instead of dropping them.
        productive: bool,
    },
    Sort {
        items: Vec<(Expr, bool)>, // (expr, ascending)
        /// Use Gremlin orderability (null < bool < number < string < ...)
        /// instead of openCypher ordering.
        gremlin_order: bool,
        child: Box<Operator>,
    },
    Limit {
        count: Expr,
        child: Box<Operator>,
    },
    Skip {
        count: Expr,
        child: Box<Operator>,
    },
    Unwind {
        list_expr: Expr,
        alias: Identifier,
        child: Box<Operator>,
    },
    Union {
        left: Box<Operator>,
        right: Box<Operator>,
        all: bool,
    },
    ShortestPath {
        src_alias: Identifier,
        dst_alias: Identifier,
        rel_types: Vec<RelType>,
        direction: Direction,
        min: u32,
        max: u32,
        path_alias: Identifier,
        all_paths: bool,
    },
    Apply {
        outer: Box<Operator>,
        inner: Box<Operator>,
    },
    Create {
        patterns: Vec<CreatePattern>,
        child: Option<Box<Operator>>,
    },
    Merge {
        node_alias: Option<Identifier>,
        labels: Vec<Label>,
        /// Inline property constraints from the MERGE pattern (e.g. `{name: 'Alice'}`).
        properties: Vec<(String, Expr)>,
        on_create: Vec<SetItem>,
        on_match: Vec<SetItem>,
        child: Option<Box<Operator>>,
    },
    /// MERGE (a)-[r:TYPE {props}]->(b) — relationship merge.
    MergeRelationship {
        src_alias: Identifier,
        dst_alias: Identifier,
        rel_alias: Option<Identifier>,
        rel_type: RelType,
        direction: Direction,
        properties: Vec<(String, Expr)>,
        on_create: Vec<SetItem>,
        on_match: Vec<SetItem>,
        child: Box<Operator>,
    },
    Set {
        items: Vec<SetItem>,
        child: Box<Operator>,
    },
    Remove {
        items: Vec<RemoveItem>,
        child: Box<Operator>,
    },
    Delete {
        exprs: Vec<Expr>,
        detach: bool,
        child: Box<Operator>,
    },
    /// Assembles a named path from node and edge aliases already in the row.
    AssemblePath {
        path_alias: Identifier,
        node_aliases: Vec<Identifier>,
        edge_aliases: Vec<Identifier>,
        child: Box<Operator>,
    },
    /// CALL procedure.
    Call {
        procedure: String,
        args: Vec<Expr>,
        /// YIELD items: `(output_name, optional_alias)`.
        yield_items: Vec<(Identifier, Option<Identifier>)>,
        /// Whether `YIELD *` was used.
        yield_star: bool,
        /// Whether this is a standalone CALL (no subsequent RETURN).
        standalone: bool,
        child: Option<Box<Operator>>,
    },
    /// FOREACH (variable IN list | body_clauses) — iterate a list and execute
    /// mutation clauses for each element. The loop variable is scoped: it is NOT
    /// visible after the FOREACH block.
    ForEach {
        variable: Identifier,
        list_expr: Expr,
        /// Sub-plan built from the body clauses (mutations only).
        body: Box<Operator>,
        child: Box<Operator>,
    },
    /// Root of every plan tree. Explicit result boundary.
    ProduceResults {
        columns: Vec<(String, Expr)>,
        child: Box<Operator>,
    },

    // ── Gremlin traversal operators ──────────────────────────────────────────
    /// Gremlin repeat() — execute body traversal repeatedly.
    Repeat {
        body: Box<Operator>,
        times: Option<usize>,
        until: Option<Box<Operator>>,
        emit: bool,
        /// When true, emit traversers BEFORE body execution (includes starting
        /// traversers).  When false and `emit` is true, emit AFTER body execution
        /// (excludes starting traversers).
        emit_before: bool,
        /// Optional predicate to filter which traversers are emitted.
        emit_predicate: Option<Box<Operator>>,
        /// After each body iteration, rename `body_out_alias` → `seed_alias`
        /// so the next iteration receives traversers under the expected key.
        remap: Option<(String, String)>,
        /// When true, `times` was specified BEFORE `repeat()` (while-loop
        /// semantics: check count before executing body).  When false,
        /// `times` was specified AFTER `repeat()` (do-while semantics:
        /// always execute the body at least once).
        times_before: bool,
        /// Optional label for named loop scoping (e.g., `repeat("a", body)`).
        /// When set, the loop counter is stored under `__gremlin_loop_count_<label>`
        /// so nested repeats can maintain independent counters.
        loop_label: Option<Box<str>>,
        child: Box<Operator>,
    },
    /// Gremlin choose() — conditional branching.
    Choose {
        predicate: Box<Operator>,
        if_true: Box<Operator>,
        if_false: Option<Box<Operator>>,
        child: Box<Operator>,
        /// Alias names for the true/false branch results plus unified output.
        /// The executor copies each branch's result column to `output_alias`
        /// so subsequent steps use a consistent column name.
        true_alias: Option<crate::ast::Identifier>,
        false_alias: Option<crate::ast::Identifier>,
        output_alias: Option<crate::ast::Identifier>,
    },
    /// Gremlin choose(selector).option(val, branch) — value-based dispatch.
    ChooseMap {
        /// Traversal that produces a dispatch value for each input row.
        selector: Box<Operator>,
        /// Alias of the selector's result column.
        selector_alias: crate::ast::Identifier,
        /// (match_value, branch_operator, branch_output_alias) triples.
        /// `None` match_value = default branch.
        options: Vec<(
            Option<crate::types::Value>,
            Box<Operator>,
            Option<crate::ast::Identifier>,
        )>,
        child: Box<Operator>,
        /// Unified output alias: branch results are copied here so downstream
        /// steps use a consistent column name.
        output_alias: Option<crate::ast::Identifier>,
    },
    /// Gremlin coalesce() — first non-empty alternative.
    Coalesce {
        alternatives: Vec<Operator>,
        child: Box<Operator>,
    },
    /// Gremlin sideEffect() — execute for side effects, pass input through.
    GremlinSideEffect {
        effect: Box<Operator>,
        child: Box<Operator>,
    },
    /// Gremlin cap() — emit accumulated side-effect value.
    Cap {
        keys: Vec<String>,
        child: Box<Operator>,
        /// Optional per-key reduction: (key, operator, init_value) triples.
        /// Populated from `withSideEffect(key, init, Operator.op)`.
        reductions: Vec<(String, String, f64)>,
    },
    /// Gremlin aggregate() — collect traverser values into a named side-effect.
    GremlinAggregate {
        key: String,
        child: Box<Operator>,
        /// The alias whose value is collected into the aggregate bucket.
        value_alias: Identifier,
        /// When set via `.by("prop")`, collect these properties instead of the raw traverser.
        /// A single key projects that property; two keys project a map of both.
        by_keys: Vec<String>,
        /// When set via `.by(traversal)`, run this sub-traversal per traverser to compute
        /// the collected value. Takes precedence over `by_keys`.
        by_traversal: Option<(Box<Operator>, Identifier)>,
        /// When true (ProductiveByStrategy), include null values for missing properties.
        productive: bool,
    },
    /// Gremlin path() — extract path from traverser aliases.
    GremlinPath {
        child: Box<Operator>,
        /// Ordered list of aliases that form the path elements.
        path_aliases: Vec<Identifier>,
        /// Cycling by-modulator keys for path element transformation.
        by_keys: Vec<String>,
        /// When true (ProductiveByStrategy), preserve Null by-key
        /// results instead of dropping the traverser.
        productive: bool,
        /// Optional label for `from()` — path starts at this label.
        from_label: Option<String>,
        /// Optional label for `to()` — path ends at this label.
        to_label: Option<String>,
    },
    /// Gremlin match() — declarative pattern matching with named bindings.
    GremlinMatch {
        /// Each pattern is a compiled sub-traversal that binds variables.
        patterns: Vec<Operator>,
        /// Maps step labels (e.g. "a", "b") to internal aliases (e.g. v_1).
        /// Used to rename output rows from internal aliases to label names.
        label_map: Vec<(String, Identifier)>,
        /// The output alias for the Map result value.
        result_alias: Identifier,
        child: Box<Operator>,
    },
    /// Gremlin select() — project labeled step values, skipping rows where
    /// any referenced label is not bound (instead of erroring).
    GremlinSelect {
        /// (source_variable, output_key) pairs.
        keys: Vec<(Identifier, String)>,
        /// When true, missing properties produce Null. When false (default),
        /// missing properties cause the traverser to be dropped.
        productive: bool,
        /// Pop modifier: controls which value to return when a label has
        /// been assigned multiple times (first, last, all, mixed).
        pop: Option<GremlinPop>,
        /// For each output key, the full list of variable aliases from
        /// all as() assignments (ordered oldest-to-newest). Used by Pop.
        label_stacks: Vec<(String, Vec<Identifier>)>,
        child: Box<Operator>,
    },
    /// Gremlin group()/groupCount() — collect traversers into a single Map.
    GremlinGroup {
        /// Expression to compute the group key for each traverser.
        key_expr: Expr,
        /// Expression to access the value to aggregate (before aggregation).
        value_expr: Expr,
        /// Aggregation function (Count for groupCount, Collect for group).
        agg_fn: AggFunction,
        /// Alias for the resulting Map in the output row.
        alias: Identifier,
        /// When set (e.g. `groupCount("a")`), the group result is stored as a
        /// side-effect in `__gremlin_agg_<key>` on every pass-through row
        /// instead of collapsing the stream into a single Map row.
        side_effect_key: Option<String>,
        /// Optional compiled sub-traversal for the group key (from by(traversal)).
        /// (Operator, output_alias) — seeded per-row to compute the key.
        key_traversal: Option<(Box<Operator>, Identifier)>,
        /// Optional compiled sub-traversal for the group value (from by(traversal)).
        /// (Operator, output_alias) — seeded per-row to compute the value.
        value_traversal: Option<(Box<Operator>, Identifier)>,
        /// When true (ProductiveByStrategy), include null-keyed entries for missing properties.
        productive: bool,
        /// When true, the Collect agg was set explicitly by fold() (not default).
        /// Forces List result even for single-member groups.
        explicit_fold: bool,
        child: Box<Operator>,
    },
    /// Gremlin `range(Scope.local, lo, hi)` / `limit(Scope.local, n)` —
    /// slice Map/List values in each row rather than slicing the stream.
    GremlinMapSlice {
        /// Column whose Map/List value should be sliced.
        alias: Identifier,
        /// Start index (inclusive).
        lo: usize,
        /// End index (exclusive).
        hi: usize,
        child: Box<Operator>,
    },
    /// Gremlin simplePath() — filter to traversers with no repeated objects in path.
    GremlinSimplePath {
        child: Box<Operator>,
        /// Ordered list of aliases that form the path elements.
        path_aliases: Vec<Identifier>,
        /// Optional label for `from()` — path checking starts at this label.
        from_label: Option<String>,
        /// Optional label for `to()` — path checking ends at this label.
        to_label: Option<String>,
    },
    /// Gremlin cyclicPath() — filter to traversers with repeated objects in path.
    GremlinCyclicPath {
        child: Box<Operator>,
        /// Ordered list of aliases that form the path elements.
        path_aliases: Vec<Identifier>,
        /// Optional label for `from()` — path checking starts at this label.
        from_label: Option<String>,
        /// Optional label for `to()` — path checking ends at this label.
        to_label: Option<String>,
    },
    /// Gremlin index() — pair each element with its positional index.
    /// When `with_map` is true, produces a Map {index: element}; otherwise
    /// produces a List [element, index].
    GremlinIndex {
        child: Box<Operator>,
        alias: Identifier,
        out_alias: Identifier,
        with_map: bool,
    },
    /// Gremlin project() with by() modulation — evaluates sub-traversals per key.
    GremlinProject {
        /// (key_name, sub_traversal_operator, output_alias_of_traversal) per entry.
        entries: Vec<(String, Operator, Identifier)>,
        child: Box<Operator>,
        /// Alias of the traverser feeding into project.
        current_alias: Identifier,
        /// When true (ProductiveByStrategy), include Null values in the map
        /// instead of omitting keys with no value.
        productive: bool,
    },
}

impl Operator {
    /// Return references to all direct child operators.
    pub fn children(&self) -> Vec<&Operator> {
        match self {
            Operator::SingleRow
            | Operator::NodeScan { .. }
            | Operator::IndexScan { .. }
            | Operator::NodeByIdSeek { .. }
            | Operator::ShortestPath { .. } => vec![],

            Operator::Filter { child, .. }
            | Operator::Project { child, .. }
            | Operator::Aggregate { child, .. }
            | Operator::Distinct { child, .. }
            | Operator::Sort { child, .. }
            | Operator::Limit { child, .. }
            | Operator::Skip { child, .. }
            | Operator::Unwind { child, .. }
            | Operator::Set { child, .. }
            | Operator::Remove { child, .. }
            | Operator::Delete { child, .. }
            | Operator::ProduceResults { child, .. }
            | Operator::Expand { child, .. }
            | Operator::VarLenExpand { child, .. }
            | Operator::AssemblePath { child, .. }
            | Operator::MergeRelationship { child, .. }
            | Operator::GremlinSideEffect { child, .. }
            | Operator::Cap { child, .. }
            | Operator::GremlinAggregate { child, .. }
            | Operator::GremlinPath { child, .. }
            | Operator::GremlinSelect { child, .. }
            | Operator::GremlinMapSlice { child, .. }
            | Operator::GremlinSimplePath { child, .. }
            | Operator::GremlinCyclicPath { child, .. }
            | Operator::GremlinIndex { child, .. } => vec![child],

            Operator::GremlinGroup {
                child,
                key_traversal,
                value_traversal,
                ..
            } => {
                let mut v = vec![child.as_ref()];
                if let Some((op, _)) = key_traversal {
                    v.push(op.as_ref());
                }
                if let Some((op, _)) = value_traversal {
                    v.push(op.as_ref());
                }
                v
            }

            Operator::GremlinProject { entries, child, .. } => {
                let mut v = vec![child.as_ref()];
                for (_, op, _) in entries {
                    v.push(op);
                }
                v
            }

            Operator::ForEach { body, child, .. } | Operator::Repeat { body, child, .. } => {
                vec![child, body]
            }

            Operator::Apply { outer, inner }
            | Operator::LeftOuterApply {
                preserved: outer,
                optional_pattern: inner,
            } => vec![outer, inner],

            Operator::Union { left, right, .. } => vec![left, right],

            Operator::Create { child, .. }
            | Operator::Merge { child, .. }
            | Operator::Call { child, .. } => match child {
                Some(c) => vec![c],
                None => vec![],
            },

            Operator::Choose {
                predicate,
                if_true,
                if_false,
                child,
                ..
            } => {
                let mut v = vec![child.as_ref(), predicate.as_ref(), if_true.as_ref()];
                if let Some(f) = if_false {
                    v.push(f.as_ref());
                }
                v
            }

            Operator::ChooseMap {
                selector,
                options,
                child,
                ..
            } => {
                let mut v = vec![child.as_ref(), selector.as_ref()];
                for (_, branch, _) in options {
                    v.push(branch.as_ref());
                }
                v
            }

            Operator::Coalesce {
                alternatives,
                child,
            } => {
                let mut v = vec![child.as_ref()];
                v.extend(alternatives.iter());
                v
            }

            Operator::GremlinMatch {
                patterns, child, ..
            } => {
                let mut v = vec![child.as_ref()];
                v.extend(patterns.iter());
                v
            }
        }
    }
}

// ── Supporting types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggFunction {
    Count,
    Sum,
    Avg,
    Min,
    Max,
    Collect,
    /// Like Collect but includes null values (used by Gremlin fold()).
    CollectAll,
    StDev,
    StDevP,
    PercentileCont,
    PercentileDisc,
}

/// Type alias for a node specification in a CREATE pattern:
/// `(alias, labels, property_pairs)`
pub type CreateNodeSpec = (Option<Identifier>, Vec<Label>, Vec<(String, Expr)>);

/// Type alias for an edge specification in a CREATE pattern:
/// `(alias, src_alias, dst_alias, rel_type, direction, property_pairs)`
pub type CreateEdgeSpec = (
    Option<Identifier>,
    Identifier,
    Identifier,
    RelType,
    Direction,
    Vec<(String, Expr)>,
);

#[derive(Debug, Clone)]
pub struct CreatePattern {
    pub nodes: Vec<CreateNodeSpec>,
    pub edges: Vec<CreateEdgeSpec>,
}

#[derive(Debug, Clone)]
pub enum SetItem {
    Property {
        alias: Identifier,
        key: String,
        value: Expr,
    },
    Labels {
        alias: Identifier,
        labels: Vec<Label>,
    },
    Map {
        alias: Identifier,
        map: Expr,
        merge: bool,
    },
}

#[derive(Debug, Clone)]
pub enum RemoveItem {
    Property {
        alias: Identifier,
        key: String,
    },
    Labels {
        alias: Identifier,
        labels: Vec<Label>,
    },
}

// ── Cardinality Estimation ────────────────────────────────────────────────────

use std::collections::HashMap;

/// Graph-level statistics used by the cardinality estimator.
#[derive(Debug, Clone)]
pub struct GraphStatistics {
    /// Total node count across all labels.
    pub total_nodes: u64,
    /// Total edge count across all relationship types.
    pub total_edges: u64,
    /// Node count per label (label name -> count).
    pub nodes_per_label: HashMap<String, u64>,
    /// Edge count per relationship type (type name -> count).
    pub edges_per_type: HashMap<String, u64>,
}

impl GraphStatistics {
    /// Average out-degree: total_edges / total_nodes (or 1.0 if no nodes).
    fn avg_degree(&self) -> f64 {
        if self.total_nodes == 0 {
            1.0
        } else {
            self.total_edges as f64 / self.total_nodes as f64
        }
    }

    /// Average out-degree for a specific relationship type.
    fn avg_degree_for_type(&self, rel_type: &str) -> f64 {
        if self.total_nodes == 0 {
            return 1.0;
        }
        let edge_count = self
            .edges_per_type
            .get(rel_type)
            .copied()
            .unwrap_or(self.total_edges);
        edge_count as f64 / self.total_nodes as f64
    }
}

/// Default selectivity for unknown filter predicates.
const DEFAULT_FILTER_SELECTIVITY: f64 = 0.1;

/// Estimate cardinality (row count) for each operator in a plan tree.
///
/// Returns a `HashMap<*const Operator, f64>` mapping each operator pointer to its
/// estimated row count. Using raw pointers as keys avoids cloning operators;
/// the map is only valid while the operator tree is alive.
pub fn estimate_cardinality(
    op: &Operator,
    stats: &GraphStatistics,
) -> HashMap<*const Operator, f64> {
    let mut estimates: HashMap<*const Operator, f64> = HashMap::new();
    estimate_recursive(op, stats, &mut estimates);
    estimates
}

fn estimate_recursive(
    op: &Operator,
    stats: &GraphStatistics,
    estimates: &mut HashMap<*const Operator, f64>,
) -> f64 {
    let est = match op {
        Operator::SingleRow => 1.0,

        Operator::NodeScan { label, .. } => match label {
            Some(Label(l)) => stats.nodes_per_label.get(l.as_str()).copied().unwrap_or(0) as f64,
            None => stats.total_nodes as f64,
        },

        Operator::IndexScan { label, .. } => {
            let label_count = stats
                .nodes_per_label
                .get(label.0.as_str())
                .copied()
                .unwrap_or(0) as f64;
            // Index scan with equality: estimate ~10% of label count, at least 1
            (label_count * DEFAULT_FILTER_SELECTIVITY).max(1.0)
        }

        Operator::NodeByIdSeek { .. } => 1.0,

        Operator::Filter { child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            (child_est * DEFAULT_FILTER_SELECTIVITY).max(1.0)
        }

        Operator::Expand {
            rel_types, child, ..
        } => {
            let child_est = estimate_recursive(child, stats, estimates);
            let degree = if rel_types.len() == 1 {
                stats.avg_degree_for_type(&rel_types[0].0)
            } else {
                stats.avg_degree()
            };
            child_est * degree.max(1.0)
        }

        Operator::VarLenExpand {
            min, max, child, ..
        } => {
            let child_est = estimate_recursive(child, stats, estimates);
            let degree = stats.avg_degree().max(1.0);
            // Estimate: sum of degree^depth for depth in min..=max, simplified
            let depth_range = (*max as f64 - *min as f64 + 1.0).max(1.0);
            child_est * degree * depth_range
        }

        Operator::Limit { count, child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            match count {
                Expr::Integer(n) => child_est.min(*n as f64),
                _ => child_est.min(100.0), // default limit guess
            }
        }

        Operator::Skip { count, child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            match count {
                Expr::Integer(n) => (child_est - *n as f64).max(0.0),
                _ => (child_est - 10.0).max(0.0),
            }
        }

        Operator::Project { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. }
        | Operator::ProduceResults { child, .. }
        | Operator::AssemblePath { child, .. }
        | Operator::MergeRelationship { child, .. } => estimate_recursive(child, stats, estimates),

        Operator::ForEach { child, body, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            // Body is executed for side effects; estimate it but don't use it for row count.
            let _ = estimate_recursive(body, stats, estimates);
            child_est
        }

        Operator::Distinct { child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            // Assume distinct removes ~20% of rows
            (child_est * 0.8).max(1.0)
        }

        Operator::Aggregate { child, .. } => {
            let _child_est = estimate_recursive(child, stats, estimates);
            // Aggregation typically reduces to a small number of groups
            10.0_f64.min(stats.total_nodes as f64).max(1.0)
        }

        Operator::Unwind { child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            child_est * 5.0 // assume average list length of 5
        }

        Operator::Union { left, right, .. } => {
            let l = estimate_recursive(left, stats, estimates);
            let r = estimate_recursive(right, stats, estimates);
            l + r
        }

        Operator::Apply { outer, inner } => {
            let o = estimate_recursive(outer, stats, estimates);
            let i = estimate_recursive(inner, stats, estimates);
            o * i
        }

        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => {
            let p = estimate_recursive(preserved, stats, estimates);
            let o = estimate_recursive(optional_pattern, stats, estimates);
            p * o.max(1.0)
        }

        Operator::Create { child, .. }
        | Operator::Merge { child, .. }
        | Operator::Call { child, .. } => match child {
            Some(c) => estimate_recursive(c, stats, estimates),
            None => 1.0,
        },

        Operator::ShortestPath { .. } => 1.0,

        // Gremlin operators: conservative estimates.
        Operator::Repeat { child, .. } => {
            let child_est = estimate_recursive(child, stats, estimates);
            child_est * 3.0 // assume ~3 iterations on average
        }
        Operator::Choose { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::ChooseMap { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::Coalesce { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinSideEffect { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinAggregate { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinPath { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinMatch { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinSelect { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::GremlinGroup {
            side_effect_key: Some(_),
            child,
            ..
        } => estimate_recursive(child, stats, estimates),
        Operator::GremlinGroup { .. } => 1.0, // produces a single Map row
        Operator::GremlinMapSlice { child, .. }
        | Operator::GremlinSimplePath { child, .. }
        | Operator::GremlinCyclicPath { child, .. }
        | Operator::GremlinIndex { child, .. }
        | Operator::GremlinProject { child, .. } => estimate_recursive(child, stats, estimates),
        Operator::Cap { .. } => 1.0,
    };

    estimates.insert(op as *const Operator, est);
    est
}

/// Build `GraphStatistics` from a `GraphStore` for a given graph.
pub fn collect_graph_statistics(
    store: &dyn crate::storage::GraphStore,
    graph: &str,
) -> Result<GraphStatistics, crate::error::GqlError> {
    let gs = store
        .graph_stats(graph)
        .map_err(crate::error::GqlError::from)?;
    let label_names = store
        .all_label_names(graph)
        .map_err(crate::error::GqlError::from)?;
    let mut nodes_per_label = HashMap::new();
    for label in &label_names {
        let ids = store
            .find_nodes_by_label(graph, label)
            .map_err(crate::error::GqlError::from)?;
        nodes_per_label.insert(label.clone(), ids.len() as u64);
    }
    // Edge counts per type: use find_edges_by_type for each known type.
    // For now, we don't have an `all_rel_type_names` method, so we skip per-type
    // counts and rely on the total edge count + avg degree.
    Ok(GraphStatistics {
        total_nodes: gs.node_count,
        total_edges: gs.edge_count,
        nodes_per_label,
        edges_per_type: HashMap::new(),
    })
}

// ── Planner module ────────────────────────────────────────────────────────────

#[allow(clippy::module_inception)]
pub mod planner;
pub mod validator;

pub use planner::QueryPlanner;
pub use validator::validate_query;

/// Returns `true` if `name` is a Cypher aggregation function.
/// Single authoritative implementation used by both planner and validator.
pub(crate) fn is_aggregate_function(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "count"
            | "sum"
            | "avg"
            | "min"
            | "max"
            | "collect"
            | "stdev"
            | "stdevp"
            | "percentilecont"
            | "percentiledisc"
    )
}

/// Validate and normalize variable-length expand bounds.
/// Returns `(effective_min, effective_max)`, capping unbounded `max` at the
/// configured `varlen_max_depth` (env: `GQL_VARLEN_MAX_DEPTH`, default 30).
///
/// When `min > effective_max` (e.g. `*2..1` or `*1..0`), openCypher specifies that the
/// pattern matches zero paths.  We return the bounds as-is; the VarLenExpand executor
/// naturally produces no rows because no depth `d` can satisfy `d >= min && d <= max`
/// when `min > max`.
pub fn validate_varlen_bounds(min: u32, max: Option<u32>) -> Result<(u32, u32), GqlError> {
    let effective_max = max.unwrap_or(config::global().varlen_max_depth);
    Ok((min, effective_max))
}

/// Collect user-visible variable aliases from an operator tree.
///
/// Returns aliases that would be available in the row at the output of this operator.
/// Filters out internal aliases (those starting with `__`).
pub fn collect_visible_aliases(op: &Operator) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for alias in collect_all_aliases(op) {
        if !alias.starts_with("__") && seen.insert(alias.clone()) {
            result.push(alias);
        }
    }
    result
}

fn collect_all_aliases(op: &Operator) -> Vec<String> {
    let mut aliases = Vec::new();
    match op {
        Operator::NodeScan { alias, .. }
        | Operator::IndexScan { alias, .. }
        | Operator::NodeByIdSeek { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::Expand {
            rel_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(collect_all_aliases(child));
            if let Some(ra) = rel_alias {
                aliases.push(ra.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::VarLenExpand {
            path_alias,
            dst_alias,
            child,
            ..
        } => {
            aliases.extend(collect_all_aliases(child));
            if let Some(pa) = path_alias {
                aliases.push(pa.0.clone());
            }
            aliases.push(dst_alias.0.clone());
        }
        Operator::AssemblePath {
            path_alias, child, ..
        } => {
            aliases.extend(collect_all_aliases(child));
            aliases.push(path_alias.0.clone());
        }
        Operator::Filter { child, .. }
        | Operator::Sort { child, .. }
        | Operator::Limit { child, .. }
        | Operator::Skip { child, .. }
        | Operator::Distinct { child, .. }
        | Operator::Set { child, .. }
        | Operator::Remove { child, .. }
        | Operator::Delete { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::Unwind { alias, child, .. } => {
            aliases.extend(collect_all_aliases(child));
            aliases.push(alias.0.clone());
        }
        Operator::Apply { outer, inner } => {
            aliases.extend(collect_all_aliases(outer));
            aliases.extend(collect_all_aliases(inner));
        }
        Operator::LeftOuterApply {
            preserved,
            optional_pattern,
        } => {
            aliases.extend(collect_all_aliases(preserved));
            aliases.extend(collect_all_aliases(optional_pattern));
        }
        Operator::Create { patterns, child } => {
            if let Some(ch) = child {
                aliases.extend(collect_all_aliases(ch));
            }
            // Extract aliases from CREATE patterns (newly created nodes/edges).
            for pattern in patterns {
                for (alias, _, _) in &pattern.nodes {
                    if let Some(a) = alias {
                        aliases.push(a.0.clone());
                    }
                }
                for (alias, _, _, _, _, _) in &pattern.edges {
                    if let Some(a) = alias {
                        aliases.push(a.0.clone());
                    }
                }
            }
        }
        Operator::Merge {
            child, node_alias, ..
        } => {
            if let Some(ch) = child {
                aliases.extend(collect_all_aliases(ch));
            }
            if let Some(a) = node_alias {
                aliases.push(a.0.clone());
            }
        }
        Operator::MergeRelationship {
            child, rel_alias, ..
        } => {
            aliases.extend(collect_all_aliases(child));
            if let Some(a) = rel_alias {
                aliases.push(a.0.clone());
            }
        }
        Operator::Project { items, .. } => {
            // After a Project (WITH), only the projected items are visible.
            // Child aliases are hidden (WITH introduces a new scope).
            for (expr, alias) in items {
                if let Some(a) = alias {
                    aliases.push(a.0.clone());
                } else {
                    // Derive column name from expression.
                    match expr {
                        Expr::Variable(id) => aliases.push(id.0.clone()),
                        Expr::Property(obj, key) => {
                            if let Expr::Variable(id) = obj.as_ref() {
                                aliases.push(format!("{}.{}", id.0, key));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        Operator::Aggregate {
            keys, aggregations, ..
        } => {
            // After Aggregate, only the key and aggregation aliases are visible.
            for key in keys {
                if let Expr::Variable(id) = key {
                    aliases.push(id.0.clone());
                }
            }
            for (_, _, _, _, alias) in aggregations {
                aliases.push(alias.0.clone());
            }
        }
        Operator::Call {
            yield_items, child, ..
        } => {
            if let Some(ch) = child {
                aliases.extend(collect_all_aliases(ch));
            }
            for (name, alias) in yield_items {
                let col_name = alias.as_ref().unwrap_or(name);
                aliases.push(col_name.0.clone());
            }
        }
        Operator::Union { left, right, .. } => {
            aliases.extend(collect_all_aliases(left));
            aliases.extend(collect_all_aliases(right));
        }
        Operator::ShortestPath {
            path_alias,
            src_alias,
            dst_alias,
            ..
        } => {
            aliases.push(path_alias.0.clone());
            aliases.push(src_alias.0.clone());
            aliases.push(dst_alias.0.clone());
        }
        Operator::ForEach { child, .. } => {
            // FOREACH variable is scoped — not visible after the block.
            // Only propagate aliases from the child pipeline.
            aliases.extend(collect_all_aliases(child));
        }
        Operator::Repeat { child, .. }
        | Operator::Choose { child, .. }
        | Operator::ChooseMap { child, .. }
        | Operator::GremlinSideEffect { child, .. }
        | Operator::GremlinAggregate { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::Coalesce { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinPath { .. } => {
            aliases.push("path".to_string());
        }
        Operator::GremlinMatch { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinSelect { keys, .. } => {
            for (_, output_key) in keys {
                aliases.push(output_key.clone());
            }
        }
        Operator::GremlinGroup { alias, .. } => {
            aliases.push(alias.0.clone());
        }
        Operator::GremlinMapSlice { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinSimplePath { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinCyclicPath { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinIndex { child, .. } => {
            aliases.extend(collect_all_aliases(child));
        }
        Operator::GremlinProject { entries, .. } => {
            for (key, _, _) in entries {
                aliases.push(key.clone());
            }
        }
        Operator::Cap { keys, .. } => {
            for key in keys {
                aliases.push(key.clone());
            }
        }
        // SingleRow and ProduceResults have no aliases of their own
        // (ProduceResults aliases come from its columns, already surfaced via child).
        Operator::SingleRow | Operator::ProduceResults { .. } => {}
    }
    aliases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varlen_bounds_valid() {
        assert_eq!(validate_varlen_bounds(1, Some(5)).unwrap(), (1, 5));
        assert_eq!(
            validate_varlen_bounds(0, None).unwrap(),
            (0, config::global().varlen_max_depth)
        );
    }

    #[test]
    fn varlen_bounds_min_gt_max_returns_bounds() {
        // openCypher: min > max is not an error — the pattern simply matches no paths.
        // The VarLenExpand executor naturally returns 0 rows when no depth d satisfies d >= min && d <= max.
        assert_eq!(validate_varlen_bounds(5, Some(3)).unwrap(), (5, 3));
    }

    // ── Cardinality estimation tests ─────────────────────────────────────────

    fn test_stats() -> GraphStatistics {
        let mut nodes_per_label = HashMap::new();
        nodes_per_label.insert("Person".to_string(), 100);
        nodes_per_label.insert("City".to_string(), 10);
        GraphStatistics {
            total_nodes: 110,
            total_edges: 200,
            nodes_per_label,
            edges_per_type: HashMap::new(),
        }
    }

    #[test]
    fn estimate_node_scan_with_label() {
        let stats = test_stats();
        let op = Operator::NodeScan {
            label: Some(Label::new("Person")),
            alias: Identifier::new("n"),
        };
        let estimates = estimate_cardinality(&op, &stats);
        let est = estimates[&(&op as *const Operator)];
        assert!((est - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_node_scan_no_label() {
        let stats = test_stats();
        let op = Operator::NodeScan {
            label: None,
            alias: Identifier::new("n"),
        };
        let estimates = estimate_cardinality(&op, &stats);
        let est = estimates[&(&op as *const Operator)];
        assert!((est - 110.0).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_filter_applies_selectivity() {
        let stats = test_stats();
        let op = Operator::Filter {
            predicate: Expr::Bool(true),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let estimates = estimate_cardinality(&op, &stats);
        let est = estimates[&(&op as *const Operator)];
        // 100 * 0.1 = 10
        assert!((est - 10.0).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_limit_caps_rows() {
        let stats = test_stats();
        let op = Operator::Limit {
            count: Expr::Integer(5),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("Person")),
                alias: Identifier::new("n"),
            }),
        };
        let estimates = estimate_cardinality(&op, &stats);
        let est = estimates[&(&op as *const Operator)];
        assert!((est - 5.0).abs() < f64::EPSILON);
    }

    #[test]
    fn estimate_expand_multiplies_by_degree() {
        let stats = test_stats();
        let op = Operator::Expand {
            direction: Direction::Outgoing,
            rel_types: vec![],
            src_alias: Identifier::new("n"),
            rel_alias: None,
            dst_alias: Identifier::new("m"),
            child: Box::new(Operator::NodeScan {
                label: Some(Label::new("City")),
                alias: Identifier::new("n"),
            }),
        };
        let estimates = estimate_cardinality(&op, &stats);
        let est = estimates[&(&op as *const Operator)];
        // 10 nodes * (200/110 avg degree) ≈ 18.18
        let expected = 10.0 * (200.0 / 110.0);
        assert!((est - expected).abs() < 0.01);
    }

    #[test]
    fn estimate_single_row() {
        let stats = test_stats();
        let op = Operator::SingleRow;
        let estimates = estimate_cardinality(&op, &stats);
        let est = estimates[&(&op as *const Operator)];
        assert!((est - 1.0).abs() < f64::EPSILON);
    }
}
