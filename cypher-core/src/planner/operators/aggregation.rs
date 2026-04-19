//! Aggregation function kinds used by `Operator::Aggregate` and
//! `Operator::GremlinGroup`.

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
