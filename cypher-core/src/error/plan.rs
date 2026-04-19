use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum PlanError {
    #[error("undefined variable '{name}' at position {offset}")]
    UndefinedVariable { name: String, offset: usize },

    #[error("ambiguous variable '{name}': defined in multiple MATCH patterns")]
    AmbiguousVariable { name: String },

    #[error("cannot use aggregating function '{name}' outside of WITH or RETURN")]
    AggregateOutsideReturn { name: String },

    #[error("RETURN * requires at least one variable in scope")]
    ReturnStarNoVariables,

    #[error("ORDER BY references '{name}' which is not in RETURN projection")]
    OrderByNotInReturn { name: String },

    #[error("write clause '{clause}' cannot follow read-only clause in the same query")]
    WriteAfterRead { clause: String },

    /// General semantic error: a query violates a semantic rule (type conflict, invalid
    /// aggregation placement, constraint violation, etc.).  Replaces the previous abuse
    /// of `UndefinedVariable` as a catch-all for non-undefined-variable errors.
    #[error("{message}")]
    SemanticError { message: String },
}
