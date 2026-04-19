//! Gremlin step model: the [`GremlinStep`] enum and the [`Pop`] modifier.

use super::Predicate;
use crate::types::GremlinValue;
use serde::{Deserialize, Serialize};

/// Gremlin Pop modifier for select() — controls which labeled value to return
/// when a label has been assigned multiple times in the traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pop {
    /// Return the first value assigned to the label.
    First,
    /// Return the last (most recent) value assigned to the label.
    Last,
    /// Return all values as a list.
    All,
    /// Return a single value unwrapped, or a list if multiple values exist.
    Mixed,
}

/// A single Gremlin traversal step.
#[derive(Debug, Clone, PartialEq)]
pub enum GremlinStep {
    // ── Source steps ──────────────────────────────────────────────────────────
    V(Option<Vec<GremlinValue>>),
    E(Option<Vec<GremlinValue>>),

    // ── Filter steps ─────────────────────────────────────────────────────────
    HasLabel(Vec<String>),
    Has(String, Option<GremlinValue>),
    /// `has("key", P.gt(30))` — property predicate filter
    HasPredicate(String, Predicate),
    HasId(Vec<GremlinValue>),
    /// `hasId(P.neq(xx1))` — ID predicate filter
    HasIdPredicate(Predicate),

    // ── Traversal steps ──────────────────────────────────────────────────────
    Out(Vec<String>),
    In(Vec<String>),
    Both(Vec<String>),
    OutE(Vec<String>),
    InE(Vec<String>),
    BothE(Vec<String>),
    InV,
    OutV,
    BothV,
    OtherV,

    // ── Element steps ────────────────────────────────────────────────────────
    Values(Vec<String>),
    /// `valueMap()` — keys to project; u8 token mode: 0=none, 1=all, 2=labels, 3=ids.
    ValueMap(Vec<String>, u8),
    Properties(Vec<String>),
    Id,
    Label,

    // ── Map/select steps ─────────────────────────────────────────────────────
    Select(Option<Pop>, Vec<String>),
    As(String),

    // ── Aggregation steps ────────────────────────────────────────────────────
    Count,
    Sum,
    Min,
    Max,
    Mean,
    SumLocal,
    MinLocal,
    MaxLocal,
    MeanLocal,
    Group(Option<String>),
    GroupCount(Option<String>),
    Fold,
    /// `fold(seed, Operator)` — reduce with a seed value and binary operator.
    FoldSeed(GremlinValue, String),
    Unfold,

    // ── Ordering & paging ────────────────────────────────────────────────────
    Order,
    /// `order(Scope.local)` — sort elements within a list
    SortLocal,
    By(String, bool), // (key, descending)
    /// `by(traversal)` — by modulation with a sub-traversal
    ByTraversal(Vec<GremlinStep>, bool),
    Limit(i64),
    Range(i64, i64),
    /// `limit(Scope.local, n)` — slice Map/List values
    LimitLocal(i64),
    /// `range(Scope.local, lo, hi)` — slice Map/List values
    RangeLocal(i64, i64),
    /// `skip(Scope.local, n)` — skip first n elements from list
    SkipLocal(i64),
    Dedup(Vec<String>),

    // ── Boolean steps ────────────────────────────────────────────────────────
    Where(Vec<GremlinStep>),
    Not(Vec<GremlinStep>),
    And(Vec<Vec<GremlinStep>>),
    Or(Vec<Vec<GremlinStep>>),
    Is(Predicate),

    // ── Mutation steps ───────────────────────────────────────────────────────
    AddV(String),
    AddVTraversal(Vec<GremlinStep>),
    AddE(String),
    Property(String, GremlinValue),
    From(Vec<GremlinStep>),
    To(Vec<GremlinStep>),
    Drop,
    /// `mergeV(searchMap)` — upsert vertex: find by criteria or create
    MergeV(GremlinValue),
    /// `mergeE(searchMap)` — upsert edge: find by criteria or create
    MergeE(GremlinValue),

    // ── Path steps ───────────────────────────────────────────────────────────
    Path,

    // ── Repeat/branch steps ──────────────────────────────────────────────────
    /// `repeat(body)` or `repeat("label", body)` — the optional label names the
    /// loop scope so that `loops("label")` can read the correct counter in
    /// nested repeat contexts.
    Repeat(Option<String>, Vec<GremlinStep>),
    Times(i64),
    Until(Vec<GremlinStep>),
    Emit,
    EmitPred(Vec<GremlinStep>),
    Choose(Vec<GremlinStep>, Vec<GremlinStep>, Option<Vec<GremlinStep>>),
    Coalesce(Vec<Vec<GremlinStep>>),
    Union(Vec<Vec<GremlinStep>>),

    // ── Side-effect steps ────────────────────────────────────────────────────
    SideEffect(Vec<GremlinStep>),
    Aggregate(String),
    Cap(Vec<String>),

    // ── Misc steps ───────────────────────────────────────────────────────────
    Constant(GremlinValue),
    Inject(Vec<GremlinValue>),
    Match(Vec<Vec<GremlinStep>>),
    Project(Vec<String>),
    Identity,
    /// `element()` — return the owning element from a property object
    Element,
    /// `tail()` or `tail(n)` — keep only the last n elements (default 1)
    Tail(i64),
    /// `tail(Scope.local)` or `tail(Scope.local, n)` — last n elements of a list
    TailLocal(i64),
    /// `skip(n)` — drop the first n elements
    Skip(i64),
    /// `elementMap(keys...)` — like valueMap but includes id and label
    ElementMap(Vec<String>),
    /// `hasKey(keys...)` — filter elements that have all listed property keys
    HasKey(Vec<String>),
    /// `hasValue(values...)` — filter elements that have any of the listed values
    HasValue(Vec<GremlinValue>),
    /// `hasValue(P.gt(30))` — filter properties by predicate on value
    HasValuePredicate(Predicate),
    /// `optional(traversal)` — execute traversal, fall back to identity if empty
    Optional(Vec<GremlinStep>),
    /// `simplePath()` — filter to paths with no repeated elements
    SimplePath,
    /// `cyclicPath()` — filter to paths with repeated elements
    CyclicPath,
    /// `coin(probability)` — random filter (0.0 to 1.0)
    Coin(f64),
    /// `map(traversal)` — apply sub-traversal to each traverser
    Map(Vec<GremlinStep>),
    /// `flatMap(traversal)` — apply sub-traversal, flatten results
    FlatMap(Vec<GremlinStep>),
    /// `filter(traversal)` — keep traversers that produce results for the sub-traversal
    FilterTraversal(Vec<GremlinStep>),
    /// `none` predicate / `none()` step — matches nothing
    None,
    /// `barrier()` — bulk synchronization step
    Barrier,
    /// barrier(Barrier.normSack)
    BarrierNormSack,
    /// `sample(n)` — random sample of n traversers
    Sample(i64),
    /// `sample(Scope.local, n)` — sample elements within a list
    SampleLocal(i64),
    /// `discard()` — consume traversal without returning results
    Discard,
    /// `loops()` — returns the number of loops in a repeat context
    /// `loops()` or `loops("label")` — returns the loop iteration counter.
    /// When a label is provided, reads the counter for that specific repeat scope.
    Loops(Option<String>),
    /// `index()` — pair each element with its positional index
    Index,
    /// `index().with(WithOptions.indexer, WithOptions.map)` — index as a Map
    IndexWithMap,

    /// `sack()` — retrieve the current sack value
    Sack,
    /// `sack(operator)` — apply binary op: sack = sack OP current_value
    /// Operator string: "sum", "minus", "mult", "div", "assign"
    SackOp(String),
    /// `withSack(initValue)` — initialize the sack with a value
    WithSack(GremlinValue, Option<String>),
    /// `withSideEffect(key, value)` — register a named side-effect value
    /// The optional third field is a reduction operator (e.g., "sum", "minus")
    /// from `withSideEffect(key, init, Operator.op)`.
    WithSideEffect(String, GremlinValue, Option<String>),
    /// `option(token, traversal)` — option step for branch/choose
    Option(GremlinValue, Vec<GremlinStep>),
    /// withBulk(false)
    WithBulkFalse,
    /// Flag indicating ProductiveByStrategy is active
    ProductiveByStrategy,
    /// Flag indicating ReadOnlyStrategy is active — mutating steps must error
    ReadOnlyStrategy,
    /// SeedStrategy with seed value for deterministic random operations
    SeedStrategy(i64),
    /// Flag indicating ReservedKeysVerificationStrategy is active
    /// Contains the set of extra reserved keys (besides "id" and "label")
    ReservedKeysVerificationStrategy(Vec<String>),
    /// Flag indicating EdgeLabelVerificationStrategy(throwException:true) is active
    EdgeLabelVerificationStrategy,
    /// Flag indicating ConnectiveStrategy is disabled via withoutStrategies
    WithoutConnectiveStrategy,
    /// `dedup(Scope.local)` — deduplicate elements within a list
    DedupLocal(Vec<String>),

    // ── String steps ────────────────────────────────────────────────────────
    /// `toLower()` — lowercase string
    ToLower,
    /// `toUpper()` — uppercase string
    ToUpper,
    /// `trim()` — trim whitespace
    Trim,
    /// `lTrim()` — trim leading whitespace
    LTrim,
    /// `rTrim()` — trim trailing whitespace
    RTrim,
    /// `length()` — string length or list size
    Length,
    /// `reverse()` — reverse string or list
    Reverse,
    /// `replace(old, new)` — string replacement
    Replace(String, String),
    /// `split(separator)` — split string into list
    Split(String),
    /// `substring(start)` or `substring(start, length)` — extract substring
    Substring(i64, Option<i64>),
    /// `substring(Scope.local, start, end)` — substring each element in a list
    SubstringLocal(i64, Option<i64>),
    /// `concat(traversals...)` — concatenate string results
    Concat(Vec<Vec<GremlinStep>>),
    /// `asString()` — convert to string
    AsString,
    /// `asString(Scope.local)` — convert elements of a list/map to strings
    AsStringLocal,
    /// `asNumber()` — convert to number (integer or double)
    /// Optional target type: "int", "long", "float", "double"
    AsNumber,
    AsNumberTyped(String),
    /// `asBool()` — convert to boolean
    AsBool,
    /// `conjoin(separator)` — join list elements with separator
    Conjoin(String),
    /// `format(pattern)` — format string
    Format(String),
    /// String operation with `Scope.local` — apply element-wise to list.
    /// The String is the Gremlin function name (e.g. "gremlin_tolower_local").
    StringOpLocal(String),
    /// `typeOf()` — return type name as string
    TypeOf,
    /// `math(expression)` — evaluate mathematical expression
    Math(String),

    // ── Collection steps ────────────────────────────────────────────────────
    /// `combine(traversal)` — append two lists
    Combine(Vec<GremlinStep>),
    /// `intersect(traversal)` — set intersection
    Intersect(Vec<GremlinStep>),
    /// `difference(traversal)` — set difference
    Difference(Vec<GremlinStep>),
    /// `disjunct(traversal)` — symmetric difference
    Disjunct(Vec<GremlinStep>),
    /// `merge(traversal)` — list merge (synonym of combine for lists)
    MergeList(Vec<GremlinStep>),
    /// `product(traversal)` — cartesian product of two lists
    Product(Vec<GremlinStep>),
    /// `all(predicate)` — true if all elements satisfy predicate
    All(Predicate),
    /// `any(predicate)` — true if any element satisfies predicate
    Any(Predicate),
    /// `none(predicate)` — true if no elements satisfy predicate
    NonePred(Predicate),

    // ── Unsupported steps (parsed but not compilable) ──────────────────────────
    /// `tree()` or `tree(sideEffectKey)` — build a tree structure from traversal paths
    Tree(Option<String>),
    /// `subgraph(sideEffectKey)` — capture a subgraph of traversed elements
    Subgraph(String),
    /// `call(service, args...)` — external service call
    Call(Vec<GremlinValue>),
    /// `shortestPath()` — shortest path algorithm
    ShortestPath,
    /// `pageRank()` — page rank algorithm
    PageRank,
    /// `connectedComponent()` — connected component algorithm
    ConnectedComponent,
    /// `peerPressure()` — peer pressure algorithm
    PeerPressure,
    /// `io(file)` — file I/O operations
    Io(String),
    /// `asDate()` — convert to date
    AsDate,
    /// `dateAdd(unit, amount)` — date arithmetic
    DateAdd(String, i64),
    /// `dateDiff(traversal)` — date difference
    DateDiff(Vec<GremlinStep>),
    /// `toE(direction, labels...)` — edge-to-vertex step
    ToE(Vec<GremlinValue>),

    // ── Predicate wrappers (used inside Is/Where) ────────────────────────────
    Eq(GremlinValue),
    Neq(GremlinValue),
    Gt(GremlinValue),
    Gte(GremlinValue),
    Lt(GremlinValue),
    Lte(GremlinValue),
    Within(Vec<GremlinValue>),
    Without(Vec<GremlinValue>),
}
