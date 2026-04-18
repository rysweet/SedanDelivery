//! Gremlin bytecode representation and deserialization from GraphSON.

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

/// A Gremlin predicate for comparison operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Predicate {
    Eq(GremlinValue),
    Neq(GremlinValue),
    Gt(GremlinValue),
    Gte(GremlinValue),
    Lt(GremlinValue),
    Lte(GremlinValue),
    Within(Vec<GremlinValue>),
    Without(Vec<GremlinValue>),
    Between(GremlinValue, GremlinValue),
    /// `P.inside(low, high)` — exclusive range (low < x < high)
    Inside(GremlinValue, GremlinValue),
    // Text predicates
    Containing(GremlinValue),
    NotContaining(GremlinValue),
    StartingWith(GremlinValue),
    NotStartingWith(GremlinValue),
    EndingWith(GremlinValue),
    NotEndingWith(GremlinValue),
    Regex(GremlinValue),
    NotRegex(GremlinValue),
    /// `P.typeOf(GType.STRING)` — check if value is of the specified Gremlin type
    IsType(GremlinValue),
    /// Compound predicate: all must match (P.gt(1).and(P.lt(5)))
    And(Vec<Predicate>),
    /// Compound predicate: any must match (P.gt(5).or(P.lt(1)))
    Or(Vec<Predicate>),
    /// Negation of a predicate: P.not(P.eq(1))
    Not(Box<Predicate>),
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

/// Raw Gremlin bytecode as sent over the wire (GraphSON format).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bytecode {
    #[serde(default)]
    pub source_instructions: Vec<(String, Vec<GremlinValue>)>,
    #[serde(default)]
    pub step_instructions: Vec<(String, Vec<GremlinValue>)>,
}

/// Errors that can occur during bytecode deserialization.
#[derive(Debug, thiserror::Error)]
pub enum BytecodeError {
    #[error("unknown step name: {0}")]
    UnknownStep(String),
    #[error("invalid argument for step '{step}': {reason}")]
    InvalidArgument { step: String, reason: String },
    #[error("missing required argument for step '{0}'")]
    MissingArgument(String),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Deserialize GraphSON bytecode JSON into a sequence of typed `GremlinStep`s.
///
/// Expected JSON structure:
/// ```json
/// {
///   "step": [
///     ["V"],
///     ["has", "name", "marko"],
///     ["out", "knows"],
///     ["values", "name"]
///   ]
/// }
/// ```
pub fn deserialize_bytecode(json: &serde_json::Value) -> Result<Vec<GremlinStep>, BytecodeError> {
    let steps_array = json
        .get("step")
        .or_else(|| json.get("step_instructions"))
        .and_then(|v| v.as_array())
        .ok_or_else(|| BytecodeError::InvalidArgument {
            step: "root".into(),
            reason: "expected 'step' array".into(),
        })?;

    let mut result = Vec::with_capacity(steps_array.len());
    for step_entry in steps_array {
        let arr = step_entry
            .as_array()
            .ok_or_else(|| BytecodeError::InvalidArgument {
                step: "entry".into(),
                reason: "expected array".into(),
            })?;
        if arr.is_empty() {
            continue;
        }
        let name = arr[0]
            .as_str()
            .ok_or_else(|| BytecodeError::InvalidArgument {
                step: "entry".into(),
                reason: "step name must be a string".into(),
            })?;
        let args = &arr[1..];
        let step = parse_step(name, args)?;
        result.push(step);
    }
    Ok(result)
}

fn json_to_gremlin_value(v: &serde_json::Value) -> GremlinValue {
    match v {
        serde_json::Value::Null => GremlinValue::Null,
        serde_json::Value::Bool(b) => GremlinValue::Bool(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                GremlinValue::Integer(i)
            } else {
                GremlinValue::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        serde_json::Value::String(s) => GremlinValue::String(s.clone()),
        serde_json::Value::Array(arr) => {
            GremlinValue::List(arr.iter().map(json_to_gremlin_value).collect())
        }
        serde_json::Value::Object(obj) => {
            let map = obj
                .iter()
                .map(|(k, v)| (k.clone(), json_to_gremlin_value(v)))
                .collect();
            GremlinValue::Map(map)
        }
    }
}

fn collect_string_args(args: &[serde_json::Value]) -> Vec<String> {
    args.iter()
        .filter_map(|a| a.as_str().map(String::from))
        .collect()
}

fn collect_value_args(args: &[serde_json::Value]) -> Vec<GremlinValue> {
    args.iter().map(json_to_gremlin_value).collect()
}

/// Parse a single nested-traversal argument (either bare `{"step":[...]}` or
/// GraphSON-wrapped `{"@type":"g:Bytecode","@value":{"step":[...]}}`).
fn parse_inner_traversal(
    step_name: &str,
    arg: &serde_json::Value,
) -> Result<Vec<GremlinStep>, BytecodeError> {
    let inner = arg.get("@value").unwrap_or(arg);
    deserialize_bytecode(inner).map_err(|e| BytecodeError::InvalidArgument {
        step: step_name.into(),
        reason: format!("nested traversal: {e}"),
    })
}

/// Parse all args as nested traversals. Used by steps like `union`, `and`,
/// `or`, `coalesce`, `match` that accept a variadic list of sub-traversals.
fn parse_inner_traversals(
    step_name: &str,
    args: &[serde_json::Value],
) -> Result<Vec<Vec<GremlinStep>>, BytecodeError> {
    args.iter()
        .map(|a| parse_inner_traversal(step_name, a))
        .collect()
}

/// Parse the single-traversal argument, returning MissingArgument if absent.
fn parse_single_inner_traversal(
    step_name: &str,
    args: &[serde_json::Value],
) -> Result<Vec<GremlinStep>, BytecodeError> {
    let arg = args
        .first()
        .ok_or_else(|| BytecodeError::MissingArgument(step_name.into()))?;
    parse_inner_traversal(step_name, arg)
}

fn parse_step(name: &str, args: &[serde_json::Value]) -> Result<GremlinStep, BytecodeError> {
    match name {
        "V" => {
            if args.is_empty() {
                Ok(GremlinStep::V(None))
            } else {
                Ok(GremlinStep::V(Some(collect_value_args(args))))
            }
        }
        "E" => {
            if args.is_empty() {
                Ok(GremlinStep::E(None))
            } else {
                Ok(GremlinStep::E(Some(collect_value_args(args))))
            }
        }
        "hasLabel" => Ok(GremlinStep::HasLabel(collect_string_args(args))),
        "has" => {
            let key = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("has".into()))?
                .to_string();
            let value = args.get(1).map(json_to_gremlin_value);
            Ok(GremlinStep::Has(key, value))
        }
        "hasId" => Ok(GremlinStep::HasId(collect_value_args(args))),
        "out" => Ok(GremlinStep::Out(collect_string_args(args))),
        "in" => Ok(GremlinStep::In(collect_string_args(args))),
        "both" => Ok(GremlinStep::Both(collect_string_args(args))),
        "outE" => Ok(GremlinStep::OutE(collect_string_args(args))),
        "inE" => Ok(GremlinStep::InE(collect_string_args(args))),
        "bothE" => Ok(GremlinStep::BothE(collect_string_args(args))),
        "inV" => Ok(GremlinStep::InV),
        "outV" => Ok(GremlinStep::OutV),
        "bothV" => Ok(GremlinStep::BothV),
        "otherV" => Ok(GremlinStep::OtherV),
        "values" => Ok(GremlinStep::Values(collect_string_args(args))),
        "valueMap" => Ok(GremlinStep::ValueMap(collect_string_args(args), 0)),
        "properties" => Ok(GremlinStep::Properties(collect_string_args(args))),
        "id" => Ok(GremlinStep::Id),
        "label" => Ok(GremlinStep::Label),
        "select" => Ok(GremlinStep::Select(None, collect_string_args(args))),
        "as" => {
            let alias = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("as".into()))?
                .to_string();
            Ok(GremlinStep::As(alias))
        }
        "count" => Ok(GremlinStep::Count),
        "sum" => Ok(GremlinStep::Sum),
        "min" => Ok(GremlinStep::Min),
        "max" => Ok(GremlinStep::Max),
        "mean" => Ok(GremlinStep::Mean),
        "group" => Ok(GremlinStep::Group(None)),
        "groupCount" => Ok(GremlinStep::GroupCount(None)),
        "fold" => Ok(GremlinStep::Fold),
        "unfold" => Ok(GremlinStep::Unfold),
        "order" => Ok(GremlinStep::Order),
        "limit" => {
            let n = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("limit".into()))?;
            Ok(GremlinStep::Limit(n))
        }
        "range" => {
            let low = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("range".into()))?;
            let high = args
                .get(1)
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("range".into()))?;
            Ok(GremlinStep::Range(low, high))
        }
        "dedup" => Ok(GremlinStep::Dedup(collect_string_args(args))),
        "is" => {
            let pred = parse_predicate_from_args(args)?;
            Ok(GremlinStep::Is(pred))
        }
        "addV" => {
            let label = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("addV".into()))?
                .to_string();
            Ok(GremlinStep::AddV(label))
        }
        "addE" => {
            let label = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("addE".into()))?
                .to_string();
            Ok(GremlinStep::AddE(label))
        }
        "property" => {
            let key = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("property".into()))?
                .to_string();
            let value = args
                .get(1)
                .map(json_to_gremlin_value)
                .unwrap_or(GremlinValue::Null);
            Ok(GremlinStep::Property(key, value))
        }
        "drop" => Ok(GremlinStep::Drop),
        "path" => Ok(GremlinStep::Path),
        "times" => {
            let n = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("times".into()))?;
            Ok(GremlinStep::Times(n))
        }
        "emit" => Ok(GremlinStep::Emit),
        "aggregate" => {
            let key = args
                .first()
                .and_then(|a| a.as_str())
                .ok_or_else(|| BytecodeError::MissingArgument("aggregate".into()))?
                .to_string();
            Ok(GremlinStep::Aggregate(key))
        }
        "cap" => Ok(GremlinStep::Cap(collect_string_args(args))),
        "constant" => {
            let value = args
                .first()
                .map(json_to_gremlin_value)
                .unwrap_or(GremlinValue::Null);
            Ok(GremlinStep::Constant(value))
        }
        "inject" => Ok(GremlinStep::Inject(collect_value_args(args))),
        "project" => Ok(GremlinStep::Project(collect_string_args(args))),
        "identity" => Ok(GremlinStep::Identity),
        "tail" => {
            let n = args.first().and_then(|a| a.as_i64()).unwrap_or(1);
            Ok(GremlinStep::Tail(n))
        }
        "skip" => {
            let n = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("skip".into()))?;
            Ok(GremlinStep::Skip(n))
        }
        "elementMap" => Ok(GremlinStep::ElementMap(collect_string_args(args))),
        "hasKey" => Ok(GremlinStep::HasKey(collect_string_args(args))),
        "hasValue" => Ok(GremlinStep::HasValue(collect_value_args(args))),
        "simplePath" => Ok(GremlinStep::SimplePath),
        "cyclicPath" => Ok(GremlinStep::CyclicPath),
        "coin" => {
            let p = args
                .first()
                .and_then(|a| a.as_f64())
                .ok_or_else(|| BytecodeError::MissingArgument("coin".into()))?;
            Ok(GremlinStep::Coin(p))
        }
        "none" => Ok(GremlinStep::None),
        "barrier" => {
            let is_norm_sack = args.iter().any(|a| {
                a.as_str()
                    .is_some_and(|s| s.contains("normSack") || s.contains("Barrier.normSack"))
            });
            if is_norm_sack {
                Ok(GremlinStep::BarrierNormSack)
            } else {
                Ok(GremlinStep::Barrier)
            }
        }
        "sample" => {
            let n = args
                .first()
                .and_then(|a| a.as_i64())
                .ok_or_else(|| BytecodeError::MissingArgument("sample".into()))?;
            Ok(GremlinStep::Sample(n))
        }
        "discard" => Ok(GremlinStep::Discard),
        "loops" => {
            // Optional label argument names a specific repeat scope.
            let label = args.first().and_then(|a| a.as_str()).map(String::from);
            Ok(GremlinStep::Loops(label))
        }
        "sack" => Ok(GremlinStep::Sack),
        "repeat" => {
            // Bytecode form: ["repeat", <inner-bytecode>]
            // or:            ["repeat", "<label>", <inner-bytecode>]
            // Inner is either a bare {"step":[...]} object or a wrapped
            // GraphSON value {"@type":"g:Bytecode","@value":{"step":[...]}}.
            let (label, body_json) = match args {
                [body] => (None, body),
                [maybe_label, body] => {
                    if let Some(lbl) = maybe_label.as_str() {
                        (Some(lbl.to_string()), body)
                    } else {
                        (None, maybe_label)
                    }
                }
                _ => {
                    return Err(BytecodeError::MissingArgument("repeat".into()));
                }
            };
            let inner_bytecode = body_json.get("@value").unwrap_or(body_json);
            let body = deserialize_bytecode(inner_bytecode).map_err(|e| {
                BytecodeError::InvalidArgument {
                    step: "repeat".into(),
                    reason: format!("nested traversal: {e}"),
                }
            })?;
            Ok(GremlinStep::Repeat(label, body))
        }

        // ── Nested-traversal steps that take a single sub-traversal ──────────
        "until" => Ok(GremlinStep::Until(parse_single_inner_traversal(
            "until", args,
        )?)),
        "where" => Ok(GremlinStep::Where(parse_single_inner_traversal(
            "where", args,
        )?)),
        "not" => Ok(GremlinStep::Not(parse_single_inner_traversal("not", args)?)),
        "from" => Ok(GremlinStep::From(parse_single_inner_traversal(
            "from", args,
        )?)),
        "to" => Ok(GremlinStep::To(parse_single_inner_traversal("to", args)?)),
        "optional" => Ok(GremlinStep::Optional(parse_single_inner_traversal(
            "optional", args,
        )?)),
        "map" => Ok(GremlinStep::Map(parse_single_inner_traversal("map", args)?)),
        "flatMap" => Ok(GremlinStep::FlatMap(parse_single_inner_traversal(
            "flatMap", args,
        )?)),
        "filter" => Ok(GremlinStep::FilterTraversal(parse_single_inner_traversal(
            "filter", args,
        )?)),
        "sideEffect" => Ok(GremlinStep::SideEffect(parse_single_inner_traversal(
            "sideEffect",
            args,
        )?)),

        // ── Nested-traversal steps that take a variadic list of sub-traversals ──
        "and" => Ok(GremlinStep::And(parse_inner_traversals("and", args)?)),
        "or" => Ok(GremlinStep::Or(parse_inner_traversals("or", args)?)),
        "union" => Ok(GremlinStep::Union(parse_inner_traversals("union", args)?)),
        "coalesce" => Ok(GremlinStep::Coalesce(parse_inner_traversals(
            "coalesce", args,
        )?)),
        "match" => Ok(GremlinStep::Match(parse_inner_traversals("match", args)?)),

        // ── Choose: (predicate, true_branch, optional_false_branch) ──────────
        "choose" => match args {
            [pred, t] => Ok(GremlinStep::Choose(
                parse_inner_traversal("choose", pred)?,
                parse_inner_traversal("choose", t)?,
                None,
            )),
            [pred, t, f] => Ok(GremlinStep::Choose(
                parse_inner_traversal("choose", pred)?,
                parse_inner_traversal("choose", t)?,
                Some(parse_inner_traversal("choose", f)?),
            )),
            _ => Err(BytecodeError::MissingArgument("choose".into())),
        },

        // ── Option: (token_value, sub_traversal) ─────────────────────────────
        "option" => {
            if args.len() < 2 {
                return Err(BytecodeError::MissingArgument("option".into()));
            }
            let token = json_to_gremlin_value(&args[0]);
            let body = parse_inner_traversal("option", &args[1])?;
            Ok(GremlinStep::Option(token, body))
        }

        // ── By: scalar key form, traversal form, or (key, asc/desc) form ─────
        "by" => match args {
            [] => Ok(GremlinStep::By(String::new(), false)),
            [a] => {
                if let Some(s) = a.as_str() {
                    Ok(GremlinStep::By(s.to_string(), false))
                } else {
                    Ok(GremlinStep::ByTraversal(
                        parse_inner_traversal("by", a)?,
                        false,
                    ))
                }
            }
            [a, b] => {
                let descending = b
                    .as_str()
                    .is_some_and(|s| s.eq_ignore_ascii_case("desc") || s.contains("decr"));
                if let Some(s) = a.as_str() {
                    Ok(GremlinStep::By(s.to_string(), descending))
                } else {
                    Ok(GremlinStep::ByTraversal(
                        parse_inner_traversal("by", a)?,
                        descending,
                    ))
                }
            }
            _ => Err(BytecodeError::InvalidArgument {
                step: "by".into(),
                reason: format!("unexpected by() arity {}", args.len()),
            }),
        },

        _ => Err(BytecodeError::UnknownStep(name.into())),
    }
}

fn parse_predicate_from_args(args: &[serde_json::Value]) -> Result<Predicate, BytecodeError> {
    if args.is_empty() {
        return Err(BytecodeError::MissingArgument("is".into()));
    }
    // Simple scalar: is(42) → Eq
    if !args[0].is_object() {
        return Ok(Predicate::Eq(json_to_gremlin_value(&args[0])));
    }
    // Predicate object: {"@type": "g:P", "@value": {"predicate": "gt", "value": 10}}
    let obj = args[0]
        .as_object()
        .ok_or_else(|| BytecodeError::InvalidArgument {
            step: "is".into(),
            reason: "predicate must be an object".into(),
        })?;
    if let Some(pval) = obj.get("@value") {
        let pred_name = pval
            .get("predicate")
            .and_then(|p| p.as_str())
            .unwrap_or("eq");
        let value = pval
            .get("value")
            .map(json_to_gremlin_value)
            .unwrap_or(GremlinValue::Null);
        match pred_name {
            "eq" => Ok(Predicate::Eq(value)),
            "neq" => Ok(Predicate::Neq(value)),
            "gt" => Ok(Predicate::Gt(value)),
            "gte" => Ok(Predicate::Gte(value)),
            "lt" => Ok(Predicate::Lt(value)),
            "lte" => Ok(Predicate::Lte(value)),
            "within" => {
                if let GremlinValue::List(items) = value {
                    Ok(Predicate::Within(items))
                } else {
                    Ok(Predicate::Within(vec![value]))
                }
            }
            "without" => {
                if let GremlinValue::List(items) = value {
                    Ok(Predicate::Without(items))
                } else {
                    Ok(Predicate::Without(vec![value]))
                }
            }
            "between" => {
                let low = value;
                let high = pval
                    .get("other")
                    .map(json_to_gremlin_value)
                    .unwrap_or(GremlinValue::Null);
                Ok(Predicate::Between(low, high))
            }
            _ => Err(BytecodeError::UnknownStep(format!("predicate:{pred_name}"))),
        }
    } else {
        // Plain value comparison
        Ok(Predicate::Eq(json_to_gremlin_value(&args[0])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_simple_traversal() {
        let json: serde_json::Value = serde_json::json!({
            "step": [
                ["V"],
                ["has", "name", "marko"],
                ["out", "knows"],
                ["values", "name"]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps.len(), 4);
        assert_eq!(steps[0], GremlinStep::V(None));
        assert_eq!(
            steps[1],
            GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into())))
        );
        assert_eq!(steps[2], GremlinStep::Out(vec!["knows".into()]));
        assert_eq!(steps[3], GremlinStep::Values(vec!["name".into()]));
    }

    #[test]
    fn deserialize_v_with_ids() {
        let json: serde_json::Value = serde_json::json!({
            "step": [["V", 1, 2, 3]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::V(Some(vec![
                GremlinValue::Integer(1),
                GremlinValue::Integer(2),
                GremlinValue::Integer(3),
            ]))
        );
    }

    #[test]
    fn deserialize_mutation_steps() {
        let json: serde_json::Value = serde_json::json!({
            "step": [
                ["addV", "person"],
                ["property", "name", "alice"],
                ["property", "age", 30]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0], GremlinStep::AddV("person".into()));
        assert_eq!(
            steps[1],
            GremlinStep::Property("name".into(), GremlinValue::String("alice".into()))
        );
        assert_eq!(
            steps[2],
            GremlinStep::Property("age".into(), GremlinValue::Integer(30))
        );
    }

    #[test]
    fn unknown_step_returns_error() {
        let json: serde_json::Value = serde_json::json!({
            "step": [["nonexistent_step"]]
        });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::UnknownStep(_)));
    }

    #[test]
    fn deserialize_limit_and_dedup() {
        let json: serde_json::Value = serde_json::json!({
            "step": [
                ["V"],
                ["dedup"],
                ["limit", 10]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[1], GremlinStep::Dedup(vec![]));
        assert_eq!(steps[2], GremlinStep::Limit(10));
    }

    // ── Source step coverage ────────────────────────────────────────────────

    #[test]
    fn deserialize_e_no_args() {
        let json = serde_json::json!({ "step": [["E"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::E(None));
    }

    #[test]
    fn deserialize_e_with_ids() {
        let json = serde_json::json!({ "step": [["E", 10, 20]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::E(Some(vec![
                GremlinValue::Integer(10),
                GremlinValue::Integer(20),
            ]))
        );
    }

    // ── Filter step coverage ────────────────────────────────────────────────

    #[test]
    fn deserialize_has_label() {
        let json = serde_json::json!({ "step": [["hasLabel", "Person", "Animal"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::HasLabel(vec!["Person".into(), "Animal".into()])
        );
    }

    #[test]
    fn deserialize_has_key_only() {
        let json = serde_json::json!({ "step": [["has", "age"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Has("age".into(), None));
    }

    #[test]
    fn deserialize_has_id() {
        let json = serde_json::json!({ "step": [["hasId", 1, 2]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::HasId(vec![GremlinValue::Integer(1), GremlinValue::Integer(2),])
        );
    }

    #[test]
    fn deserialize_has_missing_key_returns_error() {
        let json = serde_json::json!({ "step": [["has"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    // ── Traversal step coverage ─────────────────────────────────────────────

    #[test]
    fn deserialize_in_both_edge_steps() {
        let json = serde_json::json!({
            "step": [
                ["in", "knows"],
                ["both", "likes", "follows"],
                ["outE", "created"],
                ["inE"],
                ["bothE"],
                ["inV"],
                ["outV"],
                ["bothV"],
                ["otherV"]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::In(vec!["knows".into()]));
        assert_eq!(
            steps[1],
            GremlinStep::Both(vec!["likes".into(), "follows".into()])
        );
        assert_eq!(steps[2], GremlinStep::OutE(vec!["created".into()]));
        assert_eq!(steps[3], GremlinStep::InE(vec![]));
        assert_eq!(steps[4], GremlinStep::BothE(vec![]));
        assert_eq!(steps[5], GremlinStep::InV);
        assert_eq!(steps[6], GremlinStep::OutV);
        assert_eq!(steps[7], GremlinStep::BothV);
        assert_eq!(steps[8], GremlinStep::OtherV);
    }

    // ── Element step coverage ───────────────────────────────────────────────

    #[test]
    fn deserialize_element_steps() {
        let json = serde_json::json!({
            "step": [
                ["values", "name", "age"],
                ["valueMap"],
                ["properties", "name"],
                ["id"],
                ["label"]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Values(vec!["name".into(), "age".into()])
        );
        assert_eq!(steps[1], GremlinStep::ValueMap(vec![], 0));
        assert_eq!(steps[2], GremlinStep::Properties(vec!["name".into()]));
        assert_eq!(steps[3], GremlinStep::Id);
        assert_eq!(steps[4], GremlinStep::Label);
    }

    // ── Map/select step coverage ────────────────────────────────────────────

    #[test]
    fn deserialize_select_and_as() {
        let json = serde_json::json!({
            "step": [
                ["as", "a"],
                ["select", "a", "b"]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::As("a".into()));
        assert_eq!(
            steps[1],
            GremlinStep::Select(None, vec!["a".into(), "b".into()])
        );
    }

    #[test]
    fn deserialize_as_missing_arg_returns_error() {
        let json = serde_json::json!({ "step": [["as"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    // ── Aggregation step coverage ───────────────────────────────────────────

    #[test]
    fn deserialize_aggregation_steps() {
        let json = serde_json::json!({
            "step": [
                ["count"],
                ["sum"],
                ["min"],
                ["max"],
                ["mean"],
                ["group"],
                ["groupCount"],
                ["fold"],
                ["unfold"]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Count);
        assert_eq!(steps[1], GremlinStep::Sum);
        assert_eq!(steps[2], GremlinStep::Min);
        assert_eq!(steps[3], GremlinStep::Max);
        assert_eq!(steps[4], GremlinStep::Mean);
        assert_eq!(steps[5], GremlinStep::Group(None));
        assert_eq!(steps[6], GremlinStep::GroupCount(None));
        assert_eq!(steps[7], GremlinStep::Fold);
        assert_eq!(steps[8], GremlinStep::Unfold);
    }

    // ── Ordering & paging step coverage ─────────────────────────────────────

    #[test]
    fn deserialize_order() {
        let json = serde_json::json!({ "step": [["order"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Order);
    }

    #[test]
    fn deserialize_range() {
        let json = serde_json::json!({ "step": [["range", 2, 5]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Range(2, 5));
    }

    #[test]
    fn deserialize_range_missing_arg_returns_error() {
        let json = serde_json::json!({ "step": [["range", 2]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    #[test]
    fn deserialize_limit_missing_arg_returns_error() {
        let json = serde_json::json!({ "step": [["limit"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    // ── Mutation step coverage ──────────────────────────────────────────────

    #[test]
    fn deserialize_add_e() {
        let json = serde_json::json!({ "step": [["addE", "knows"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::AddE("knows".into()));
    }

    #[test]
    fn deserialize_add_e_missing_label_returns_error() {
        let json = serde_json::json!({ "step": [["addE"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    #[test]
    fn deserialize_add_v_missing_label_returns_error() {
        let json = serde_json::json!({ "step": [["addV"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    #[test]
    fn deserialize_drop() {
        let json = serde_json::json!({ "step": [["drop"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Drop);
    }

    #[test]
    fn deserialize_property_no_value() {
        let json = serde_json::json!({ "step": [["property", "key"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Property("key".into(), GremlinValue::Null)
        );
    }

    #[test]
    fn deserialize_property_missing_key_returns_error() {
        let json = serde_json::json!({ "step": [["property"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    // ── Misc step coverage ──────────────────────────────────────────────────

    #[test]
    fn deserialize_path_and_identity() {
        let json = serde_json::json!({ "step": [["path"], ["identity"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Path);
        assert_eq!(steps[1], GremlinStep::Identity);
    }

    #[test]
    fn deserialize_times() {
        let json = serde_json::json!({ "step": [["times", 3]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Times(3));
    }

    #[test]
    fn deserialize_times_missing_arg_returns_error() {
        let json = serde_json::json!({ "step": [["times"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    #[test]
    fn deserialize_emit() {
        let json = serde_json::json!({ "step": [["emit"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Emit);
    }

    #[test]
    fn deserialize_aggregate() {
        let json = serde_json::json!({ "step": [["aggregate", "x"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Aggregate("x".into()));
    }

    #[test]
    fn deserialize_aggregate_missing_key_returns_error() {
        let json = serde_json::json!({ "step": [["aggregate"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    #[test]
    fn deserialize_cap() {
        let json = serde_json::json!({ "step": [["cap", "x", "y"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Cap(vec!["x".into(), "y".into()]));
    }

    #[test]
    fn deserialize_constant_and_inject() {
        let json = serde_json::json!({
            "step": [
                ["constant", "hello"],
                ["inject", 1, 2, 3]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Constant(GremlinValue::String("hello".into()))
        );
        assert_eq!(
            steps[1],
            GremlinStep::Inject(vec![
                GremlinValue::Integer(1),
                GremlinValue::Integer(2),
                GremlinValue::Integer(3),
            ])
        );
    }

    #[test]
    fn deserialize_project() {
        let json = serde_json::json!({ "step": [["project", "a", "b"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Project(vec!["a".into(), "b".into()]));
    }

    #[test]
    fn deserialize_tail_default_and_explicit() {
        let json = serde_json::json!({ "step": [["tail"], ["tail", 5]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Tail(1));
        assert_eq!(steps[1], GremlinStep::Tail(5));
    }

    #[test]
    fn deserialize_skip() {
        let json = serde_json::json!({ "step": [["skip", 3]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Skip(3));
    }

    #[test]
    fn deserialize_skip_missing_arg_returns_error() {
        let json = serde_json::json!({ "step": [["skip"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    #[test]
    fn deserialize_element_map() {
        let json = serde_json::json!({ "step": [["elementMap", "name"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::ElementMap(vec!["name".into()]));
    }

    #[test]
    fn deserialize_has_key_and_has_value() {
        let json = serde_json::json!({
            "step": [
                ["hasKey", "name", "age"],
                ["hasValue", 42, "hello"]
            ]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::HasKey(vec!["name".into(), "age".into()])
        );
        assert_eq!(
            steps[1],
            GremlinStep::HasValue(vec![
                GremlinValue::Integer(42),
                GremlinValue::String("hello".into()),
            ])
        );
    }

    #[test]
    fn deserialize_simple_and_cyclic_path() {
        let json = serde_json::json!({ "step": [["simplePath"], ["cyclicPath"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::SimplePath);
        assert_eq!(steps[1], GremlinStep::CyclicPath);
    }

    #[test]
    fn deserialize_coin() {
        let json = serde_json::json!({ "step": [["coin", 0.5]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Coin(0.5));
    }

    #[test]
    fn deserialize_coin_missing_arg_returns_error() {
        let json = serde_json::json!({ "step": [["coin"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    #[test]
    fn deserialize_none_and_barrier() {
        let json = serde_json::json!({ "step": [["none"], ["barrier"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::None);
        assert_eq!(steps[1], GremlinStep::Barrier);
    }

    #[test]
    fn deserialize_barrier_norm_sack() {
        let json = serde_json::json!({ "step": [["barrier", "Barrier.normSack"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::BarrierNormSack);
    }

    #[test]
    fn deserialize_sample() {
        let json = serde_json::json!({ "step": [["sample", 5]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Sample(5));
    }

    #[test]
    fn deserialize_sample_missing_arg_returns_error() {
        let json = serde_json::json!({ "step": [["sample"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    #[test]
    fn deserialize_discard_loops_sack() {
        let json = serde_json::json!({ "step": [["discard"], ["loops"], ["sack"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Discard);
        assert_eq!(steps[1], GremlinStep::Loops(None));
        assert_eq!(steps[2], GremlinStep::Sack);
    }

    #[test]
    fn deserialize_loops_with_label() {
        let json = serde_json::json!({ "step": [["loops", "outer"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::Loops(Some("outer".into())));
    }

    #[test]
    fn deserialize_repeat_with_inner_bytecode() {
        let json = serde_json::json!({
            "step": [["repeat", { "step": [["out"]] }], ["times", 3]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Repeat(None, vec![GremlinStep::Out(vec![])])
        );
        assert_eq!(steps[1], GremlinStep::Times(3));
    }

    #[test]
    fn deserialize_repeat_with_graphson_wrapped_inner() {
        // Standard TinkerPop GraphSON wraps nested traversals in @type/@value.
        let json = serde_json::json!({
            "step": [[
                "repeat",
                { "@type": "g:Bytecode", "@value": { "step": [["out"]] } }
            ]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Repeat(None, vec![GremlinStep::Out(vec![])])
        );
    }

    #[test]
    fn deserialize_repeat_with_label() {
        let json = serde_json::json!({
            "step": [["repeat", "outer", { "step": [["out"]] }]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Repeat(Some("outer".into()), vec![GremlinStep::Out(vec![])])
        );
    }

    // ── Predicate parsing coverage ──────────────────────────────────────────

    #[test]
    fn deserialize_is_with_scalar() {
        let json = serde_json::json!({ "step": [["is", 42]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Is(Predicate::Eq(GremlinValue::Integer(42)))
        );
    }

    #[test]
    fn deserialize_is_with_gt_predicate() {
        let json = serde_json::json!({
            "step": [["is", { "@type": "g:P", "@value": { "predicate": "gt", "value": 10 } }]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Is(Predicate::Gt(GremlinValue::Integer(10)))
        );
    }

    #[test]
    fn deserialize_is_with_all_comparison_predicates() {
        for (pred_name, expected_ctor) in [
            ("eq", Predicate::Eq(GremlinValue::Integer(5)) as Predicate),
            ("neq", Predicate::Neq(GremlinValue::Integer(5))),
            ("gte", Predicate::Gte(GremlinValue::Integer(5))),
            ("lt", Predicate::Lt(GremlinValue::Integer(5))),
            ("lte", Predicate::Lte(GremlinValue::Integer(5))),
        ] {
            let json = serde_json::json!({
                "step": [["is", { "@type": "g:P", "@value": { "predicate": pred_name, "value": 5 } }]]
            });
            let steps = deserialize_bytecode(&json).unwrap();
            assert_eq!(
                steps[0],
                GremlinStep::Is(expected_ctor),
                "failed for predicate: {pred_name}"
            );
        }
    }

    #[test]
    fn deserialize_is_with_within_list() {
        let json = serde_json::json!({
            "step": [["is", { "@type": "g:P", "@value": { "predicate": "within", "value": [1, 2, 3] } }]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Is(Predicate::Within(vec![
                GremlinValue::Integer(1),
                GremlinValue::Integer(2),
                GremlinValue::Integer(3),
            ]))
        );
    }

    #[test]
    fn deserialize_is_with_without_scalar() {
        let json = serde_json::json!({
            "step": [["is", { "@type": "g:P", "@value": { "predicate": "without", "value": "x" } }]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Is(Predicate::Without(vec![GremlinValue::String("x".into())]))
        );
    }

    #[test]
    fn deserialize_is_with_between() {
        let json = serde_json::json!({
            "step": [["is", { "@type": "g:P", "@value": { "predicate": "between", "value": 1, "other": 10 } }]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Is(Predicate::Between(
                GremlinValue::Integer(1),
                GremlinValue::Integer(10),
            ))
        );
    }

    #[test]
    fn deserialize_is_missing_arg_returns_error() {
        let json = serde_json::json!({ "step": [["is"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::MissingArgument(_)));
    }

    #[test]
    fn deserialize_is_unknown_predicate_returns_error() {
        let json = serde_json::json!({
            "step": [["is", { "@type": "g:P", "@value": { "predicate": "bogus", "value": 1 } }]]
        });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::UnknownStep(_)));
    }

    // ── Structural / edge-case coverage ─────────────────────────────────────

    #[test]
    fn deserialize_empty_step_array_skipped() {
        let json = serde_json::json!({ "step": [[], ["V"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0], GremlinStep::V(None));
    }

    #[test]
    fn deserialize_missing_step_key_returns_error() {
        let json = serde_json::json!({ "data": [["V"]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::InvalidArgument { .. }));
    }

    #[test]
    fn deserialize_non_array_entry_returns_error() {
        let json = serde_json::json!({ "step": ["not_an_array"] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::InvalidArgument { .. }));
    }

    #[test]
    fn deserialize_non_string_step_name_returns_error() {
        let json = serde_json::json!({ "step": [[42]] });
        let err = deserialize_bytecode(&json).unwrap_err();
        assert!(matches!(err, BytecodeError::InvalidArgument { .. }));
    }

    #[test]
    fn deserialize_step_instructions_alias() {
        // The deserializer also accepts "step_instructions" as key
        let json = serde_json::json!({ "step_instructions": [["V"], ["count"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0], GremlinStep::V(None));
        assert_eq!(steps[1], GremlinStep::Count);
    }

    // ── json_to_gremlin_value coverage ──────────────────────────────────────

    #[test]
    fn json_to_gremlin_value_all_types() {
        assert_eq!(
            json_to_gremlin_value(&serde_json::json!(null)),
            GremlinValue::Null
        );
        assert_eq!(
            json_to_gremlin_value(&serde_json::json!(true)),
            GremlinValue::Bool(true)
        );
        assert_eq!(
            json_to_gremlin_value(&serde_json::json!(42)),
            GremlinValue::Integer(42)
        );
        assert_eq!(
            json_to_gremlin_value(&serde_json::json!("hi")),
            GremlinValue::String("hi".into())
        );
        assert_eq!(
            json_to_gremlin_value(&serde_json::json!([1, "a"])),
            GremlinValue::List(vec![
                GremlinValue::Integer(1),
                GremlinValue::String("a".into())
            ])
        );
        let map = json_to_gremlin_value(&serde_json::json!({"k": 99}));
        if let GremlinValue::Map(m) = &map {
            assert_eq!(m.get("k"), Some(&GremlinValue::Integer(99)));
        } else {
            panic!("expected Map, got {map:?}");
        }
    }

    #[test]
    fn json_to_gremlin_value_float() {
        // f64 that isn't representable as i64
        let val = json_to_gremlin_value(&serde_json::json!(1.5));
        if let GremlinValue::Float(f) = val {
            assert!((f - 1.5).abs() < f64::EPSILON);
        } else {
            panic!("expected Float, got {val:?}");
        }
    }

    // ── Bytecode struct serde coverage ──────────────────────────────────────

    #[test]
    fn bytecode_struct_round_trip() {
        let bc = Bytecode {
            source_instructions: vec![("withSack".into(), vec![GremlinValue::Integer(0)])],
            step_instructions: vec![
                ("V".into(), vec![]),
                (
                    "has".into(),
                    vec![
                        GremlinValue::String("name".into()),
                        GremlinValue::String("marko".into()),
                    ],
                ),
            ],
        };
        let json = serde_json::to_string(&bc).unwrap();
        let decoded: Bytecode = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, bc);
    }

    #[test]
    fn bytecode_struct_defaults_empty() {
        let bc: Bytecode = serde_json::from_str("{}").unwrap();
        assert!(bc.source_instructions.is_empty());
        assert!(bc.step_instructions.is_empty());
    }

    // ── Error Display coverage ──────────────────────────────────────────────

    #[test]
    fn bytecode_error_display() {
        let e1 = BytecodeError::UnknownStep("foo".into());
        assert!(e1.to_string().contains("foo"));

        let e2 = BytecodeError::InvalidArgument {
            step: "has".into(),
            reason: "bad".into(),
        };
        assert!(e2.to_string().contains("has"));
        assert!(e2.to_string().contains("bad"));

        let e3 = BytecodeError::MissingArgument("limit".into());
        assert!(e3.to_string().contains("limit"));

        let e4 = BytecodeError::Json(serde_json::from_str::<i32>("bad").unwrap_err());
        assert!(!e4.to_string().is_empty());
    }
}

#[cfg(test)]
mod nested_traversal_tests {
    use super::*;

    type SingleCtor = fn(Vec<GremlinStep>) -> GremlinStep;
    type VariadicCtor = fn(Vec<Vec<GremlinStep>>) -> GremlinStep;

    fn out_only() -> serde_json::Value {
        serde_json::json!({ "step": [["out"]] })
    }

    #[test]
    fn deserialize_single_inner_steps() {
        // Each step that takes one nested traversal.
        let cases: &[(&str, SingleCtor)] = &[
            ("until", GremlinStep::Until),
            ("where", GremlinStep::Where),
            ("not", GremlinStep::Not),
            ("from", GremlinStep::From),
            ("to", GremlinStep::To),
            ("optional", GremlinStep::Optional),
            ("map", GremlinStep::Map),
            ("flatMap", GremlinStep::FlatMap),
            ("filter", GremlinStep::FilterTraversal),
            ("sideEffect", GremlinStep::SideEffect),
        ];
        for (name, ctor) in cases {
            let json = serde_json::json!({ "step": [[*name, out_only()]] });
            let steps =
                deserialize_bytecode(&json).unwrap_or_else(|e| panic!("{name} failed: {e}"));
            assert_eq!(steps[0], ctor(vec![GremlinStep::Out(vec![])]), "{name}");
        }
    }

    #[test]
    fn deserialize_variadic_inner_steps() {
        let cases: &[(&str, VariadicCtor)] = &[
            ("and", GremlinStep::And),
            ("or", GremlinStep::Or),
            ("union", GremlinStep::Union),
            ("coalesce", GremlinStep::Coalesce),
            ("match", GremlinStep::Match),
        ];
        for (name, ctor) in cases {
            let json = serde_json::json!({
                "step": [[*name, out_only(), out_only()]]
            });
            let steps =
                deserialize_bytecode(&json).unwrap_or_else(|e| panic!("{name} failed: {e}"));
            assert_eq!(
                steps[0],
                ctor(vec![
                    vec![GremlinStep::Out(vec![])],
                    vec![GremlinStep::Out(vec![])]
                ]),
                "{name}"
            );
        }
    }

    #[test]
    fn deserialize_choose_two_and_three_arg() {
        // Two-arg choose: (predicate, true_branch).
        let json = serde_json::json!({
            "step": [["choose", out_only(), out_only()]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Choose(
                vec![GremlinStep::Out(vec![])],
                vec![GremlinStep::Out(vec![])],
                None
            )
        );

        // Three-arg choose: (predicate, true_branch, false_branch).
        let json = serde_json::json!({
            "step": [["choose", out_only(), out_only(), out_only()]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert!(matches!(steps[0], GremlinStep::Choose(_, _, Some(_))));
    }

    #[test]
    fn deserialize_option_with_token_and_body() {
        let json = serde_json::json!({
            "step": [["option", "tokenA", out_only()]]
        });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::Option(
                GremlinValue::String("tokenA".into()),
                vec![GremlinStep::Out(vec![])]
            )
        );
    }

    #[test]
    fn deserialize_by_string_and_traversal_forms() {
        let json = serde_json::json!({ "step": [["by", "name"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::By("name".into(), false));

        let json = serde_json::json!({ "step": [["by", "name", "desc"]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(steps[0], GremlinStep::By("name".into(), true));

        let json = serde_json::json!({ "step": [["by", out_only()]] });
        let steps = deserialize_bytecode(&json).unwrap();
        assert_eq!(
            steps[0],
            GremlinStep::ByTraversal(vec![GremlinStep::Out(vec![])], false)
        );
    }

    #[test]
    fn deserialize_nested_inner_traversal_propagates_errors() {
        // Inner step is unknown — error should bubble with context.
        let json = serde_json::json!({
            "step": [["where", { "step": [["definitely_not_a_step"]] }]]
        });
        let err = deserialize_bytecode(&json).unwrap_err();
        match err {
            BytecodeError::InvalidArgument { step, reason } => {
                assert_eq!(step, "where");
                assert!(reason.contains("nested traversal"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }
}
