//! Physical plan operator types and supporting AST-like structures.
//!
//! Defines the complete physical plan operator set (33 operators) plus
//! supporting types (CREATE patterns, SET/REMOVE items, aggregation functions).

use crate::ast::{Direction, Expr, Identifier, Label, RelType};

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
pub enum GremlinSelectTraversal {
    /// Evaluate a compiled by(traversal) sub-traversal for the selected key.
    Traversal {
        op: Box<Operator>,
        out_alias: Identifier,
    },
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
        /// Intermediate path aliases produced by the repeat body (excluding the
        /// seed alias and body-output alias).  Used by the executor to accumulate
        /// edge values in `__gremlin_path_acc` when the body is e.g. `outE().inV()`.
        body_path_aliases: Vec<String>,
        /// Shadow aliases for Pop.first support.  Each pair is
        /// `(shadow_alias, original_alias)`: the executor copies the original
        /// seed value into the shadow column before the first iteration so that
        /// `select(Pop.first, label)` can retrieve the initial binding even
        /// after remap overwrites the original alias.
        pop_origins: Vec<(String, String)>,
        /// When true, a downstream `path()` step requires per-traverser path
        /// accumulation (`__gremlin_path_acc`).  The vertex-counting fast path
        /// cannot provide this and must bail.
        needs_path: bool,
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
        /// When true, the side-effect was initialized as a Set (e.g. `withSideEffect("a", {})`).
        /// Aggregate deduplicates values instead of appending duplicates.
        set_semantics: bool,
        /// When true (Operator.assign), each aggregate call REPLACES the accumulated
        /// list instead of appending to it. Only the last call's values survive to cap().
        assign_semantics: bool,
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
    /// Gremlin tree() — build a tree structure from traversal paths.
    GremlinTree {
        child: Box<Operator>,
        /// Ordered list of aliases that form the path elements for each traverser.
        path_aliases: Vec<Identifier>,
        /// Optional side-effect key. When set, the tree is accumulated as a
        /// side-effect and traversers pass through. When None, tree() is a barrier
        /// that emits a single tree Map.
        side_effect_key: Option<String>,
        /// Output column name (used in barrier mode).
        alias: Identifier,
    },
    /// Gremlin match() — declarative pattern matching with named bindings.
    GremlinMatch {
        /// Each pattern is a compiled sub-traversal that binds variables.
        patterns: Vec<Operator>,
        /// Per-pattern produced labels: labels whose values should be
        /// extracted from the pattern output (excludes seed labels that
        /// the pattern consumed but may have overwritten internally).
        pattern_labels: Vec<Vec<(String, Identifier)>>,
        /// Maps step labels (e.g. "a", "b") to internal aliases (e.g. v_1).
        /// Used to inject seed values when executing each pattern.
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
        /// Optional by(traversal) modulators aligned to `keys`.
        by_traversals: Vec<Option<GremlinSelectTraversal>>,
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
        /// When true, the group key is the traverser itself (no by() key modulator was applied).
        /// Null traversers are valid group keys in this case and must NOT be skipped.
        key_is_identity: bool,
        /// When true, the Collect agg was set explicitly by fold() (not default).
        /// Forces List result even for single-member groups.
        explicit_fold: bool,
        child: Box<Operator>,
    },
    /// Gremlin `sample(n)` — barrier sampling of the traverser stream with an
    /// optional weight/probability expression from a single by() modulator.
    GremlinSample {
        count: usize,
        weight: Option<Expr>,
        seed: Option<i64>,
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
        /// By-modulator keys for path element comparison.
        by_keys: Vec<String>,
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
        /// Cycling by-modulator keys for path element comparison.
        by_keys: Vec<String>,
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
        /// (key_name, sub_traversal_operator, output_alias_of_traversal, by_is_identity) per entry.
        /// `by_is_identity` is true when the by() modulator is empty (identity) — in that
        /// case a Null result is the traverser itself being null and must be kept in the map.
        entries: Vec<(String, Operator, Identifier, bool)>,
        child: Box<Operator>,
        /// Alias of the traverser feeding into project.
        current_alias: Identifier,
        /// Alias holding the projected map after project() completes.
        out_alias: Identifier,
        /// When true (ProductiveByStrategy), include Null values in the map
        /// instead of omitting keys with no value.
        productive: bool,
    },
    /// Gremlin barrier() / barrier(normSack).
    GremlinBarrier {
        child: Box<Operator>,
        current_alias: Identifier,
        out_alias: Identifier,
        norm_sack: bool,
        sack_merge_op: Option<String>,
        with_bulk: bool,
    },
}

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
    DynamicLabels {
        alias: Identifier,
        value: Expr,
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
