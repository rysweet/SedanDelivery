//! Shared AST primitives: source spans, wrappers, identifier newtypes, and
//! relationship direction.

use std::ops::Deref;

use smol_str::SmolStr;

// ── Source spans ──────────────────────────────────────────────────────────────

/// A half-open byte range `[start, end)` into the original query string.
///
/// Used for error reporting: when the query planner or executor detects an
/// error, it attaches the `Span` of the offending AST node so the error
/// message can point at the right part of the source text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// A span covering zero bytes, used as a sentinel when no source position is available.
    pub fn zero() -> Self {
        Self { start: 0, end: 0 }
    }

    pub fn len(&self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(&self) -> bool {
        self.end <= self.start
    }
}

// ── Spanned<T> ────────────────────────────────────────────────────────────────

/// Wraps an AST node `T` with its source [`Span`].
///
/// Implements [`Deref<Target = T>`] so code that doesn't care about spans can
/// reach through transparently.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Spanned<T> {
    pub node: T,
    pub span: Span,
}

impl<T> Spanned<T> {
    pub fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }

    pub fn without_span(node: T) -> Self {
        Self {
            node,
            span: Span::zero(),
        }
    }
}

impl<T> Deref for Spanned<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.node
    }
}

// ── Identifier / Label / RelType newtypes ────────────────────────────────────

/// A Cypher variable name or property key, such as `n`, `r`, `person`.
///
/// Stored as a `String` because variable names can be backtick-quoted and
/// arbitrarily long.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Identifier(pub String);

impl Identifier {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Identifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for Identifier {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for Identifier {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// A node label, such as `Person` or `Movie`.
///
/// Uses [`SmolStr`] for inline storage: labels are almost always short ASCII
/// strings that fit in 22 bytes, avoiding a heap allocation per label.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Label(pub SmolStr);

impl Label {
    pub fn new(s: impl Into<SmolStr>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Label {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for Label {
    fn from(s: &str) -> Self {
        Self(SmolStr::new(s))
    }
}

impl From<String> for Label {
    fn from(s: String) -> Self {
        Self(SmolStr::new(s))
    }
}

/// A relationship type, such as `KNOWS` or `ACTED_IN`.
///
/// Uses [`SmolStr`] for the same reason as [`Label`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RelType(pub SmolStr);

impl RelType {
    pub fn new(s: impl Into<SmolStr>) -> Self {
        Self(s.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for RelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for RelType {
    fn from(s: &str) -> Self {
        Self(SmolStr::new(s))
    }
}

impl From<String> for RelType {
    fn from(s: String) -> Self {
        Self(SmolStr::new(s))
    }
}

// ── Direction ─────────────────────────────────────────────────────────────────

/// Direction of a relationship pattern.
///
/// ```text
/// (a)-[r]->(b)   → Outgoing  (from a's perspective)
/// (a)<-[r]-(b)   → Incoming  (from a's perspective)
/// (a)-[r]-(b)    → Undirected
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// `(a)-[r]->(b)` — follows edges from src to dst.
    Outgoing,
    /// `(a)<-[r]-(b)` — follows edges from dst to src.
    Incoming,
    /// `(a)-[r]-(b)` — follows edges in either direction.
    Undirected,
}
