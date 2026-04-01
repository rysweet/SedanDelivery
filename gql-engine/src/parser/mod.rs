//! GQL (ISO/IEC 39075) parser.
//!
//! Reuses the cypher-core lexer and parser as a foundation, then applies
//! GQL-specific transformations where the two standards diverge.
//!
//! Key GQL differences handled here:
//! - `FILTER` clause (GQL uses `FILTER` instead of standalone `WHERE` after `MATCH`)
//! - `INSERT` as alias for `CREATE`
//! - `FOR` as alias for `FOREACH`
//! - `MANDATORY MATCH` (error if no match found)
//! - `LET` bindings (planned — currently lowered to `WITH`)
//! - Path quantifiers: `ANY SHORTEST`, `ALL SHORTEST`, `ANY` path modes

pub mod rewrite;

use cypher_core::ast::Query;
use cypher_core::error::ParseError;

/// Parse a GQL query string into a [`Query`] AST.
///
/// The query is first normalised from GQL syntax to the cypher-core AST
/// representation. GQL-only constructs are lowered to their openCypher
/// equivalents where possible; constructs with no equivalent produce
/// a [`GqlParseError`].
pub fn parse_gql(src: &str) -> Result<GqlQuery, GqlParseError> {
    let normalised = rewrite::normalise_gql_source(src);
    let query = cypher_core::parser::parse_query(&normalised).map_err(GqlParseError::Parse)?;
    let metadata = rewrite::extract_gql_metadata(src);
    Ok(GqlQuery { query, metadata })
}

/// A parsed GQL query: the cypher-core AST plus GQL-specific metadata.
#[derive(Debug, Clone)]
pub struct GqlQuery {
    /// The core AST (shared with openCypher).
    pub query: Query,
    /// GQL-specific metadata extracted during parsing.
    pub metadata: GqlMetadata,
}

/// GQL-specific metadata that cannot be represented in the cypher-core AST.
#[derive(Debug, Clone, Default)]
pub struct GqlMetadata {
    /// Whether `MANDATORY MATCH` was used (any match clause).
    pub mandatory_match: bool,
    /// Path mode if specified: `WALK`, `TRAIL`, `SIMPLE`, `ACYCLIC`.
    pub path_mode: Option<PathMode>,
    /// Path search modifier if specified.
    pub path_search: Option<PathSearch>,
}

/// GQL path modes control which kinds of paths are returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathMode {
    /// Default: any path including repeated nodes and edges.
    Walk,
    /// No repeated edges.
    Trail,
    /// No repeated nodes (except start = end for cycles).
    Simple,
    /// No repeated nodes at all.
    Acyclic,
}

/// GQL path search modifiers control how many matching paths are returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSearch {
    /// Return any single shortest path.
    AnyShortest,
    /// Return all shortest paths.
    AllShortest,
    /// Return any single path.
    Any,
    /// Return all paths.
    All,
}

/// Errors specific to GQL parsing.
#[derive(Debug, thiserror::Error)]
pub enum GqlParseError {
    /// Underlying cypher-core parse error.
    #[error("{0}")]
    Parse(#[from] ParseError),
    /// GQL construct not yet supported.
    #[error("unsupported GQL construct: {0}")]
    Unsupported(String),
    /// GQL semantic constraint violated.
    #[error("GQL semantic error: {0}")]
    Semantic(String),
}
