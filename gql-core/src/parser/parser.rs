//! Recursive descent parser for openCypher / GQL.
//!
//! Entry point: [`parse_query`].
//!
//! Design notes:
//! - The `|` character is handled contextually:
//!   - Inside a label or rel-type list it is label/type disjunction: `(n:A|B)`.
//!   - Inside a list comprehension `[x IN list | expr]` it separates the
//!     filter variable from the projection expression.
//! - All error paths carry a byte offset from the original source.

use smol_str::SmolStr;

use crate::ast::{
    BinaryOp, CallClause, Clause, CreateClause, DeleteClause, Direction, Expr, ForeachClause,
    Identifier, Label, MapLiteral, MatchClause, MergeClause, NodePattern, PathPattern, PathStep,
    Properties, Query, RangeQuantifier, RelPattern, RelType, RemoveClause, RemoveClauseItem,
    ReturnClause, ReturnItem, SetClause, SetClauseItem, SortItem, UnaryOp, UnwindClause,
    WithClause,
};
use crate::error::ParseError;
use crate::parser::lexer::{Token, TokenKind};

// ── Parser struct ─────────────────────────────────────────────────────────────

/// Cursor-based recursive descent parser.
///
/// Tokens are pre-collected from the lexer before parsing begins, so every
/// lookahead is an O(1) slice index and there is no buffering complexity.
pub struct Parser<'src> {
    tokens: Vec<Token<'src>>,
    pos: usize,
    /// Current expression nesting depth; guards against deeply nested DoS inputs.
    depth: u32,
    /// Maximum allowed expression nesting depth (injected from deployment config).
    max_depth: u32,
}

impl<'src> Parser<'src> {
    /// Create a parser from a pre-lexed token stream.
    ///
    /// The token stream must end with a [`TokenKind::Eof`] sentinel.
    pub fn new(tokens: Vec<Token<'src>>) -> Self {
        let max_depth = crate::config::global().parser_max_depth;
        Self {
            tokens,
            pos: 0,
            depth: 0,
            max_depth,
        }
    }

    // ── Cursor helpers ────────────────────────────────────────────────────────

    /// Return the kind of the current token without advancing.
    fn peek(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    /// Return the kind of the token one position ahead without advancing.
    fn peek2(&self) -> &TokenKind {
        let idx = (self.pos + 1).min(self.tokens.len() - 1);
        &self.tokens[idx].kind
    }

    /// Peek two positions ahead (pos + 2).
    fn peek3(&self) -> &TokenKind {
        let idx = (self.pos + 2).min(self.tokens.len() - 1);
        &self.tokens[idx].kind
    }

    /// Byte offset of the current token.
    fn offset(&self) -> usize {
        self.tokens[self.pos].offset
    }

    /// Consume and return the current token.
    fn advance(&mut self) -> &Token<'src> {
        let tok = &self.tokens[self.pos];
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    /// Return `true` if the current token matches `kind`.
    fn at(&self, kind: &TokenKind) -> bool {
        self.peek() == kind
    }

    /// Consume the current token and return it if it matches `kind`,
    /// otherwise return `false` without consuming.
    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    /// Consume the current token if it matches `kind`, or return a
    /// [`ParseError::Syntax`] error.
    fn expect(&mut self, kind: &TokenKind) -> Result<&Token<'src>, ParseError> {
        if self.peek() == kind {
            // SAFETY: advance() returns a reference into self.tokens which
            // outlives this parser, but the borrow checker can't see through
            // the mutable borrow. We use an index to work around this.
            let idx = self.pos;
            self.pos += 1;
            Ok(&self.tokens[idx])
        } else {
            Err(self.syntax_err(format!("{:?}", kind)))
        }
    }

    /// Consume an identifier-shaped token (bare Identifier or any keyword
    /// that may be used as an identifier in property-access position).
    /// Returns the text of the token as an owned `String`.
    fn expect_identifier(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Identifier => {
                let tok = self.advance();
                // Strip backtick quotes if present.
                let text = tok.text;
                if text.starts_with('`') && text.ends_with('`') && text.len() >= 2 {
                    Ok(text[1..text.len() - 1].to_string())
                } else {
                    Ok(text.to_string())
                }
            }
            // Many keywords are valid as variable/property names in Cypher.
            kind if is_keyword_usable_as_ident(&kind) => {
                let tok = self.advance();
                Ok(tok.text.to_string())
            }
            _ => Err(self.syntax_err("identifier")),
        }
    }

    fn syntax_err(&self, expected: impl Into<String>) -> ParseError {
        let found = format!("{:?}", self.peek());
        ParseError::Syntax {
            offset: self.offset(),
            expected: expected.into(),
            found,
        }
    }
}

/// Returns `true` for keyword tokens that are also valid Cypher identifiers
/// (property keys, variable names, function names, relationship type names, etc.).
fn is_keyword_usable_as_ident(kind: &TokenKind) -> bool {
    matches!(
        kind,
        TokenKind::Count
            | TokenKind::All
            | TokenKind::Starts
            | TokenKind::Ends
            | TokenKind::Contains
            | TokenKind::Skip
            | TokenKind::Limit
            | TokenKind::Order
            | TokenKind::By
            | TokenKind::Distinct
            | TokenKind::As
            | TokenKind::Call
            | TokenKind::Yield
            | TokenKind::Null
            | TokenKind::True
            | TokenKind::False
            | TokenKind::End
            | TokenKind::Foreach
            | TokenKind::In
    )
}

/// Returns `true` for keyword tokens that can be used as map keys or property names.
/// This is a superset of `is_keyword_usable_as_ident` that includes literal keywords
/// and other context-sensitive keywords that are valid as property names.
fn is_keyword_usable_as_property(kind: &TokenKind) -> bool {
    is_keyword_usable_as_ident(kind)
        || matches!(
            kind,
            TokenKind::Null
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Case
                | TokenKind::When
                | TokenKind::Then
                | TokenKind::Else
                | TokenKind::End
                | TokenKind::In
                | TokenKind::Is
                | TokenKind::Not
                | TokenKind::And
                | TokenKind::Or
                | TokenKind::Xor
                | TokenKind::Match
                | TokenKind::Where
                | TokenKind::Return
                | TokenKind::Create
                | TokenKind::Delete
                | TokenKind::Set
                | TokenKind::Remove
                | TokenKind::Merge
                | TokenKind::With
                | TokenKind::Unwind
                | TokenKind::Optional
                | TokenKind::Detach
                | TokenKind::Foreach
                | TokenKind::Union
                | TokenKind::Starts
                | TokenKind::Ends
                | TokenKind::Contains
        )
}

// ── Top-level entry point ─────────────────────────────────────────────────────

/// Parse `src` into a [`Query`] AST.
///
/// Returns [`ParseError::EmptyQuery`] for blank/empty input.
/// Returns [`ParseError::Syntax`] for any structural grammar error.
pub fn parse_query(src: &str) -> Result<Query, ParseError> {
    parse_query_with_max_size(src, crate::config::global().max_query_size)
}

/// Like [`parse_query`] but with an explicit byte-length limit.
///
/// Useful for testing the size guard without relying on the global config.
pub fn parse_query_with_max_size(src: &str, max_query_size: usize) -> Result<Query, ParseError> {
    if src.len() > max_query_size {
        return Err(ParseError::QueryTooLarge {
            size: src.len(),
            limit: max_query_size,
        });
    }
    if src.trim().is_empty() {
        return Err(ParseError::EmptyQuery);
    }
    let tokens = crate::parser::lexer::lex_query(src)?;
    let mut p = Parser::new(tokens);
    p.parse_query_inner()
}

impl<'src> Parser<'src> {
    fn parse_query_inner(&mut self) -> Result<Query, ParseError> {
        let clauses = self.parse_clause_sequence()?;

        if clauses.is_empty() {
            return Err(ParseError::EmptyQuery);
        }

        // Parse optional UNION / UNION ALL parts.
        let mut unions = Vec::new();
        while self.at(&TokenKind::Union) {
            self.advance(); // consume UNION
            let all = self.eat(&TokenKind::All);
            let part = self.parse_clause_sequence()?;
            if part.is_empty() {
                return Err(self.syntax_err("clause after UNION"));
            }
            unions.push((all, part));
        }

        Ok(Query { clauses, unions })
    }

    /// Parse a sequence of clauses (separated by optional semicolons),
    /// stopping at UNION, EOF, or when no more clauses can be parsed.
    fn parse_clause_sequence(&mut self) -> Result<Vec<Clause>, ParseError> {
        let mut clauses = Vec::new();
        // Skip leading semicolons.
        while self.eat(&TokenKind::Semicolon) {}

        loop {
            if self.at(&TokenKind::Eof) || self.at(&TokenKind::Union) {
                break;
            }
            match self.parse_clause()? {
                Some(clause) => clauses.push(clause),
                None => break,
            }
            // Optional clause separator.
            while self.eat(&TokenKind::Semicolon) {}
        }

        Ok(clauses)
    }

    // ── Clause dispatcher ─────────────────────────────────────────────────────

    fn parse_clause(&mut self) -> Result<Option<Clause>, ParseError> {
        match self.peek().clone() {
            TokenKind::Match => {
                self.advance();
                let mc = self.parse_match(false)?;
                Ok(Some(Clause::Match(mc)))
            }
            TokenKind::Optional => {
                self.advance();
                self.expect(&TokenKind::Match)?;
                let mc = self.parse_match(true)?;
                Ok(Some(Clause::OptionalMatch(mc)))
            }
            TokenKind::Where => {
                self.advance();
                let expr = self.parse_expr()?;
                Ok(Some(Clause::Where(expr)))
            }
            TokenKind::Return => {
                self.advance();
                let rc = self.parse_return()?;
                Ok(Some(Clause::Return(rc)))
            }
            TokenKind::Create => {
                self.advance();
                let cc = self.parse_create()?;
                Ok(Some(Clause::Create(cc)))
            }
            TokenKind::Detach => {
                self.advance();
                self.expect(&TokenKind::Delete)?;
                let dc = self.parse_delete(true)?;
                Ok(Some(Clause::Delete(dc)))
            }
            TokenKind::Delete => {
                self.advance();
                let dc = self.parse_delete(false)?;
                Ok(Some(Clause::Delete(dc)))
            }
            TokenKind::With => {
                self.advance();
                let wc = self.parse_with()?;
                Ok(Some(Clause::With(wc)))
            }
            TokenKind::Unwind => {
                self.advance();
                let uc = self.parse_unwind()?;
                Ok(Some(Clause::Unwind(uc)))
            }
            TokenKind::Set => {
                self.advance();
                let sc = self.parse_set()?;
                Ok(Some(Clause::Set(sc)))
            }
            TokenKind::Remove => {
                self.advance();
                let rc = self.parse_remove()?;
                Ok(Some(Clause::Remove(rc)))
            }
            TokenKind::Merge => {
                self.advance();
                let mc = self.parse_merge()?;
                Ok(Some(Clause::Merge(mc)))
            }
            TokenKind::Call => {
                self.advance();
                let cc = self.parse_call()?;
                Ok(Some(Clause::Call(cc)))
            }
            TokenKind::Foreach => {
                self.advance();
                let fc = self.parse_foreach()?;
                Ok(Some(Clause::Foreach(fc)))
            }
            TokenKind::Eof | TokenKind::Semicolon => Ok(None),
            _ => Err(self.syntax_err("clause keyword (MATCH, RETURN, CREATE, ...)")),
        }
    }

    // ── MATCH ─────────────────────────────────────────────────────────────────

    fn parse_match(&mut self, _optional: bool) -> Result<MatchClause, ParseError> {
        let mut patterns = Vec::new();
        patterns.push(self.parse_path_pattern()?);
        while self.eat(&TokenKind::Comma) {
            patterns.push(self.parse_path_pattern()?);
        }
        let where_clause = if self.eat(&TokenKind::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(MatchClause {
            patterns,
            where_clause,
        })
    }

    // ── RETURN ────────────────────────────────────────────────────────────────

    fn parse_return(&mut self) -> Result<ReturnClause, ParseError> {
        let distinct = self.eat(&TokenKind::Distinct);
        let items = self.parse_return_items()?;
        let (order_by, skip, limit) = self.parse_return_modifiers()?;
        Ok(ReturnClause {
            distinct,
            items,
            order_by,
            skip,
            limit,
        })
    }

    /// Parse a comma-separated list of return/with projection items.
    fn parse_return_items(&mut self) -> Result<Vec<ReturnItem>, ParseError> {
        let mut items = Vec::new();
        // Handle RETURN *
        if self.at(&TokenKind::Star) {
            self.advance();
            items.push(ReturnItem {
                expr: Expr::Variable(Identifier::new("*")),
                alias: None,
            });
            return Ok(items);
        }
        items.push(self.parse_return_item()?);
        while self.eat(&TokenKind::Comma) {
            items.push(self.parse_return_item()?);
        }
        Ok(items)
    }

    fn parse_return_item(&mut self) -> Result<ReturnItem, ParseError> {
        let expr = self.parse_expr()?;
        let alias = if self.eat(&TokenKind::As) {
            Some(Identifier::new(self.expect_identifier()?))
        } else {
            None
        };
        Ok(ReturnItem { expr, alias })
    }

    /// Parse ORDER BY / SKIP / LIMIT modifiers shared by RETURN and WITH.
    #[allow(clippy::type_complexity)]
    fn parse_return_modifiers(
        &mut self,
    ) -> Result<(Vec<SortItem>, Option<Expr>, Option<Expr>), ParseError> {
        let mut order_by = Vec::new();
        let mut skip = None;
        let mut limit = None;

        if self.at(&TokenKind::Order) && matches!(self.peek2(), TokenKind::By) {
            self.advance(); // ORDER
            self.advance(); // BY
            order_by.push(self.parse_sort_item()?);
            while self.eat(&TokenKind::Comma) {
                order_by.push(self.parse_sort_item()?);
            }
        }

        if self.eat(&TokenKind::Skip) {
            skip = Some(self.parse_expr()?);
        }
        if self.eat(&TokenKind::Limit) {
            limit = Some(self.parse_expr()?);
        }

        Ok((order_by, skip, limit))
    }

    fn parse_sort_item(&mut self) -> Result<SortItem, ParseError> {
        let expr = self.parse_expr()?;
        let ascending = if self.at(&TokenKind::Identifier) {
            let text = self.tokens[self.pos].text.to_ascii_lowercase();
            match text.as_str() {
                "asc" | "ascending" => {
                    self.advance();
                    true
                }
                "desc" | "descending" => {
                    self.advance();
                    false
                }
                _ => true, // Default ascending
            }
        } else {
            // Default ascending
            true
        };
        Ok(SortItem { expr, ascending })
    }

    // ── CREATE ────────────────────────────────────────────────────────────────

    fn parse_create(&mut self) -> Result<CreateClause, ParseError> {
        let mut patterns = Vec::new();
        patterns.push(self.parse_path_pattern()?);
        while self.eat(&TokenKind::Comma) {
            patterns.push(self.parse_path_pattern()?);
        }
        Ok(CreateClause { patterns })
    }

    // ── DELETE ────────────────────────────────────────────────────────────────

    fn parse_delete(&mut self, detach: bool) -> Result<DeleteClause, ParseError> {
        let mut exprs = Vec::new();
        exprs.push(self.parse_expr()?);
        while self.eat(&TokenKind::Comma) {
            exprs.push(self.parse_expr()?);
        }
        Ok(DeleteClause { exprs, detach })
    }

    // ── WITH ──────────────────────────────────────────────────────────────────

    fn parse_with(&mut self) -> Result<WithClause, ParseError> {
        let distinct = self.eat(&TokenKind::Distinct);
        let items = self.parse_return_items()?;
        let (order_by, skip, limit) = self.parse_return_modifiers()?;
        let where_clause = if self.eat(&TokenKind::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };
        Ok(WithClause {
            distinct,
            items,
            where_clause,
            order_by,
            skip,
            limit,
        })
    }

    // ── UNWIND ────────────────────────────────────────────────────────────────

    fn parse_unwind(&mut self) -> Result<UnwindClause, ParseError> {
        let expr = self.parse_expr()?;
        self.expect(&TokenKind::As)?;
        let alias = Identifier::new(self.expect_identifier()?);
        Ok(UnwindClause { expr, alias })
    }

    // ── SET ───────────────────────────────────────────────────────────────────

    fn parse_set(&mut self) -> Result<SetClause, ParseError> {
        let mut items = Vec::new();
        items.push(self.parse_set_item()?);
        while self.eat(&TokenKind::Comma) {
            items.push(self.parse_set_item()?);
        }
        Ok(SetClause { items })
    }

    fn parse_set_item(&mut self) -> Result<SetClauseItem, ParseError> {
        // Handle `SET (n).prop = value` — parenthesized node/rel reference
        if self.at(&TokenKind::LParen) {
            self.advance(); // consume '('
            let alias = Identifier::new(self.expect_identifier()?);
            self.expect(&TokenKind::RParen)?;
            self.expect(&TokenKind::Dot)?;
            let prop = SmolStr::new(self.expect_identifier()?);
            self.expect(&TokenKind::Eq)?;
            let value = self.parse_expr()?;
            return Ok(SetClauseItem::SetProperty { alias, prop, value });
        }
        let alias = Identifier::new(self.expect_identifier()?);

        match self.peek().clone() {
            // n.prop = expr
            TokenKind::Dot => {
                self.advance();
                let prop = SmolStr::new(self.expect_identifier()?);
                self.expect(&TokenKind::Eq)?;
                let value = self.parse_expr()?;
                Ok(SetClauseItem::SetProperty { alias, prop, value })
            }
            // n = expr  (replace)
            TokenKind::Eq => {
                self.advance();
                let map = self.parse_expr()?;
                Ok(SetClauseItem::SetMap {
                    alias,
                    map,
                    merge: false,
                })
            }
            // n += expr  (merge) — emitted as Plus then Eq
            TokenKind::Plus => {
                self.advance();
                self.expect(&TokenKind::Eq)?;
                let map = self.parse_expr()?;
                Ok(SetClauseItem::SetMap {
                    alias,
                    map,
                    merge: true,
                })
            }
            // n:Label
            TokenKind::Colon => {
                let labels = self.parse_label_list()?;
                Ok(SetClauseItem::SetLabels { alias, labels })
            }
            _ => Err(self.syntax_err("'.', '=', '+=', or ':' after SET target")),
        }
    }

    // ── REMOVE ────────────────────────────────────────────────────────────────

    fn parse_remove(&mut self) -> Result<RemoveClause, ParseError> {
        let mut items = Vec::new();
        items.push(self.parse_remove_item()?);
        while self.eat(&TokenKind::Comma) {
            items.push(self.parse_remove_item()?);
        }
        Ok(RemoveClause { items })
    }

    fn parse_remove_item(&mut self) -> Result<RemoveClauseItem, ParseError> {
        let alias = Identifier::new(self.expect_identifier()?);
        match self.peek().clone() {
            TokenKind::Dot => {
                self.advance();
                let prop = SmolStr::new(self.expect_identifier()?);
                Ok(RemoveClauseItem::RemoveProperty { alias, prop })
            }
            TokenKind::Colon => {
                let labels = self.parse_label_list()?;
                Ok(RemoveClauseItem::RemoveLabels { alias, labels })
            }
            _ => Err(self.syntax_err("'.' or ':' after REMOVE target")),
        }
    }

    // ── MERGE ─────────────────────────────────────────────────────────────────

    fn parse_merge(&mut self) -> Result<MergeClause, ParseError> {
        let pattern = self.parse_path_pattern()?;
        let mut on_create = Vec::new();
        let mut on_match = Vec::new();

        loop {
            // Check for ON keyword (identifier "ON" in Cypher)
            if self.at(&TokenKind::Identifier)
                && self.tokens[self.pos].text.eq_ignore_ascii_case("ON")
            {
                self.advance(); // consume ON
                match self.peek().clone() {
                    TokenKind::Create => {
                        self.advance();
                        self.expect(&TokenKind::Set)?;
                        on_create.push(self.parse_set_item()?);
                        while self.eat(&TokenKind::Comma) {
                            on_create.push(self.parse_set_item()?);
                        }
                    }
                    TokenKind::Match => {
                        self.advance();
                        self.expect(&TokenKind::Set)?;
                        on_match.push(self.parse_set_item()?);
                        while self.eat(&TokenKind::Comma) {
                            on_match.push(self.parse_set_item()?);
                        }
                    }
                    _ => return Err(self.syntax_err("CREATE or MATCH after ON")),
                }
            } else {
                break;
            }
        }

        Ok(MergeClause {
            pattern,
            on_create,
            on_match,
        })
    }

    // ── CALL ──────────────────────────────────────────────────────────────────

    fn parse_call(&mut self) -> Result<CallClause, ParseError> {
        // Parse procedure name: `proc.name` or just `name`
        let mut procedure = self.expect_identifier()?;
        while self.eat(&TokenKind::Dot) {
            let part = self.expect_identifier()?;
            procedure = format!("{}.{}", procedure, part);
        }

        // Parse arguments: `(arg1, arg2, ...)`
        let explicit_args = self.at(&TokenKind::LParen);
        let args = if self.eat(&TokenKind::LParen) {
            let mut a = Vec::new();
            if !self.at(&TokenKind::RParen) {
                a.push(self.parse_expr()?);
                while self.eat(&TokenKind::Comma) {
                    a.push(self.parse_expr()?);
                }
            }
            self.expect(&TokenKind::RParen)?;
            a
        } else {
            Vec::new()
        };

        // Optional YIELD clause: `YIELD var1 AS alias1, var2` or `YIELD *`
        let mut yield_star = false;
        let yield_items = if self.eat(&TokenKind::Yield) {
            // Check for YIELD *
            if self.eat(&TokenKind::Star) {
                yield_star = true;
                Vec::new()
            } else {
                let mut items = Vec::new();
                let name = Identifier::new(self.expect_identifier()?);
                let alias = if self.eat(&TokenKind::As) {
                    Some(Identifier::new(self.expect_identifier()?))
                } else {
                    None
                };
                items.push((name, alias));
                while self.eat(&TokenKind::Comma) {
                    let name = Identifier::new(self.expect_identifier()?);
                    let alias = if self.eat(&TokenKind::As) {
                        Some(Identifier::new(self.expect_identifier()?))
                    } else {
                        None
                    };
                    items.push((name, alias));
                }
                items
            }
        } else {
            Vec::new()
        };

        Ok(CallClause {
            procedure,
            args,
            yield_items,
            explicit_args,
            yield_star,
        })
    }

    // ── FOREACH ────────────────────────────────────────────────────────────────

    fn parse_foreach(&mut self) -> Result<ForeachClause, ParseError> {
        self.expect(&TokenKind::LParen)?;
        let variable = Identifier::new(self.expect_identifier()?);
        self.expect(&TokenKind::In)?;
        let list = self.parse_expr()?;
        self.expect(&TokenKind::Pipe)?;
        // Parse inner clauses (mutating operations: SET, CREATE, DELETE, etc.)
        let mut clauses = Vec::new();
        loop {
            if self.at(&TokenKind::RParen) {
                break;
            }
            match self.parse_clause()? {
                Some(clause) => clauses.push(clause),
                None => break,
            }
        }
        self.expect(&TokenKind::RParen)?;
        Ok(ForeachClause {
            variable,
            list,
            clauses,
        })
    }

    // ── Path patterns ─────────────────────────────────────────────────────────

    fn parse_path_pattern(&mut self) -> Result<PathPattern, ParseError> {
        // Optional path variable assignment: `p = (a)...`
        // Look for `Identifier =` where the next-next token starts a path.
        let variable = if (matches!(self.peek(), TokenKind::Identifier)
            || is_keyword_usable_as_ident(self.peek()))
            && self.peek2() == &TokenKind::Eq
            && matches!(self.peek3(), TokenKind::LParen)
        {
            let name = self.expect_identifier()?;
            self.advance(); // consume `=`
            Some(Identifier::new(name))
        } else {
            None
        };

        let start = self.parse_node_pattern()?;
        let mut steps = Vec::new();

        // Continue parsing relationship + node steps.
        while matches!(self.peek(), TokenKind::Dash | TokenKind::LeftArrow) {
            let (rel, node) = self.parse_rel_pattern()?;
            steps.push(PathStep { rel, node });
        }

        Ok(PathPattern {
            variable,
            start,
            steps,
        })
    }

    fn parse_node_pattern(&mut self) -> Result<NodePattern, ParseError> {
        self.expect(&TokenKind::LParen)?;

        // Optional variable name.
        let variable = match self.peek().clone() {
            TokenKind::Identifier => {
                let name = self.expect_identifier()?;
                Some(Identifier::new(name))
            }
            k if is_keyword_usable_as_ident(&k) => {
                let name = self.expect_identifier()?;
                Some(Identifier::new(name))
            }
            TokenKind::Colon
            | TokenKind::RParen
            | TokenKind::LBrace
            | TokenKind::DollarSign
            | TokenKind::Where => None,
            _ => None,
        };

        let labels = if self.at(&TokenKind::Colon) {
            self.parse_label_list()?
        } else {
            Vec::new()
        };

        let properties = if self.at(&TokenKind::LBrace) {
            Some(Properties::Map(self.parse_map_literal()?))
        } else if self.at(&TokenKind::DollarSign) {
            self.advance();
            let name = self.expect_identifier()?;
            Some(Properties::Parameter(name.into()))
        } else {
            None
        };

        // Optional inline WHERE clause: `(n:Person WHERE n.age > 25)`
        let where_clause = if self.eat(&TokenKind::Where) {
            Some(self.parse_expr()?)
        } else {
            None
        };

        self.expect(&TokenKind::RParen)?;

        Ok(NodePattern {
            variable,
            labels,
            properties,
            where_clause,
        })
    }

    /// Parse `-[rel_pattern]->node` or `<-[rel_pattern]-node` or `-[rel_pattern]-node`.
    ///
    /// On entry the current token is Dash or LeftArrow.
    fn parse_rel_pattern(&mut self) -> Result<(RelPattern, NodePattern), ParseError> {
        let incoming_left = self.eat(&TokenKind::LeftArrow);

        // If not LeftArrow we must have a Dash.
        if !incoming_left {
            self.expect(&TokenKind::Dash)?;
        }

        // Parse optional `[...]` bracket block.
        let (variable, rel_types, range, properties, where_clause) = if self
            .at(&TokenKind::LBracket)
        {
            self.advance(); // consume [

            // Optional variable.
            let var = match self.peek().clone() {
                TokenKind::Identifier => {
                    let name = self.expect_identifier()?;
                    Some(Identifier::new(name))
                }
                k if is_keyword_usable_as_ident(&k) => {
                    let name = self.expect_identifier()?;
                    Some(Identifier::new(name))
                }
                TokenKind::Colon | TokenKind::RBracket | TokenKind::Star | TokenKind::Where => None,
                _ => None,
            };

            // Optional rel types `:A|B|C`.
            let types = if self.at(&TokenKind::Colon) {
                self.parse_rel_type_list()?
            } else {
                Vec::new()
            };

            // Optional range `*min..max`.
            let range = if self.eat(&TokenKind::Star) {
                Some(self.parse_range_quantifier()?)
            } else {
                None
            };

            // Optional properties.
            let props = if self.at(&TokenKind::LBrace) {
                Some(Properties::Map(self.parse_map_literal()?))
            } else if self.at(&TokenKind::DollarSign) {
                self.advance();
                let name = self.expect_identifier()?;
                Some(Properties::Parameter(name.into()))
            } else {
                None
            };

            // Optional inline WHERE clause: `-[r:KNOWS WHERE r.since > 2020]->`
            let wc = if self.eat(&TokenKind::Where) {
                Some(self.parse_expr()?)
            } else {
                None
            };

            self.expect(&TokenKind::RBracket)?;
            (var, types, range, props, wc)
        } else {
            // No bracket block — anonymous untyped relationship.
            (None, Vec::new(), None, None, None)
        };

        // Determine direction by what follows.
        let direction = if incoming_left {
            // `<-[...]-`  or  `<-[...]->`  — decide by consuming the trailing `-` or `->`.
            if self.eat(&TokenKind::Arrow) {
                // `<-[...]->` — treat as undirected (both ends marked is a no-op in practice).
                Direction::Undirected
            } else {
                self.expect(&TokenKind::Dash)?;
                Direction::Incoming
            }
        } else {
            // Started with `-`, ends with `->` or `-`.
            if self.eat(&TokenKind::Arrow) {
                Direction::Outgoing
            } else {
                self.expect(&TokenKind::Dash)?;
                Direction::Undirected
            }
        };

        let rel = RelPattern {
            variable,
            rel_types,
            direction,
            range,
            properties,
            where_clause,
        };

        let node = self.parse_node_pattern()?;
        Ok((rel, node))
    }

    // ── Label / rel-type lists ────────────────────────────────────────────────

    /// Parse `:A:B:C` or `:A|B|C` — used for node labels.
    fn parse_label_list(&mut self) -> Result<Vec<Label>, ParseError> {
        let mut labels = Vec::new();
        self.expect(&TokenKind::Colon)?;
        labels.push(Label::new(SmolStr::new(self.expect_identifier()?)));
        while matches!(self.peek(), TokenKind::Colon | TokenKind::Pipe) {
            self.advance();
            labels.push(Label::new(SmolStr::new(self.expect_identifier()?)));
        }
        Ok(labels)
    }

    /// Parse `:A|B|C` or `:A|:B|:C` — used for relationship types inside `[...]`.
    fn parse_rel_type_list(&mut self) -> Result<Vec<RelType>, ParseError> {
        let mut types = Vec::new();
        self.expect(&TokenKind::Colon)?;
        types.push(RelType::new(SmolStr::new(self.expect_identifier()?)));
        while self.eat(&TokenKind::Pipe) {
            // Accept optional colon before each subsequent type: `:A|:B` or `:A|B`
            self.eat(&TokenKind::Colon);
            types.push(RelType::new(SmolStr::new(self.expect_identifier()?)));
        }
        Ok(types)
    }

    /// Parse the optional range quantifier after `*` in a rel pattern: `[min][..max]`.
    ///
    /// Possible forms:
    /// - `*`             → both None
    /// - `*3`            → min=3, max=3
    /// - `*3..`          → min=3, max=None
    /// - `*..5`          → min=None, max=5
    /// - `*1..5`         → min=1, max=5
    /// - `*1..5` (with DoubleDot token already consumed by lexer)
    fn parse_range_quantifier(&mut self) -> Result<RangeQuantifier, ParseError> {
        // After `*` is consumed the next token could be:
        // - `]`          → `*`  (any length)
        // - IntLiteral   → `*N` possibly followed by `..` or `..M`
        // - `..`         → `*..M`

        let min = if let TokenKind::IntLiteral(n) = self.peek().clone() {
            self.advance();
            Some(
                u32::try_from(n)
                    .map_err(|_| self.syntax_err("non-negative integer for range quantifier"))?,
            )
        } else {
            None
        };

        let max =
            if self.eat(&TokenKind::DoubleDot) {
                if let TokenKind::IntLiteral(n) = self.peek().clone() {
                    self.advance();
                    Some(u32::try_from(n).map_err(|_| {
                        self.syntax_err("non-negative integer for range quantifier")
                    })?)
                } else {
                    None
                }
            } else if min.is_some() {
                // `*3` without `..` means exactly 3 hops.
                min
            } else {
                None
            };

        // If we had `*3` (no DoubleDot), min==max==Some(3).
        // If we had `*3..` with DoubleDot consumed, min=Some(3), max=None already.
        // Re-compute: if DoubleDot was NOT present and min is set, max = min.
        // We handle this by checking whether we consumed DoubleDot (via the logic above).
        Ok(RangeQuantifier { min, max })
    }

    // ── Map literal ───────────────────────────────────────────────────────────

    fn parse_map_literal(&mut self) -> Result<MapLiteral, ParseError> {
        self.expect(&TokenKind::LBrace)?;
        let mut pairs = Vec::new();
        if !self.at(&TokenKind::RBrace) {
            let key = SmolStr::new(self.expect_map_key()?);
            self.expect(&TokenKind::Colon)?;
            let val = self.parse_expr()?;
            pairs.push((key, val));
            while self.eat(&TokenKind::Comma) {
                if self.at(&TokenKind::RBrace) {
                    break; // trailing comma
                }
                let key = SmolStr::new(self.expect_map_key()?);
                self.expect(&TokenKind::Colon)?;
                let val = self.parse_expr()?;
                pairs.push((key, val));
            }
        }
        self.expect(&TokenKind::RBrace)?;
        Ok(pairs)
    }

    /// Accept an identifier or a keyword that can be used as a map key or property name.
    fn expect_map_key(&mut self) -> Result<String, ParseError> {
        match self.peek().clone() {
            TokenKind::Identifier => {
                let tok = self.advance();
                let text = tok.text;
                if text.starts_with('`') && text.ends_with('`') && text.len() >= 2 {
                    Ok(text[1..text.len() - 1].to_string())
                } else {
                    Ok(text.to_string())
                }
            }
            kind if is_keyword_usable_as_property(&kind) => {
                let tok = self.advance();
                Ok(tok.text.to_string())
            }
            _ => Err(self.syntax_err("identifier or property name")),
        }
    }

    // ── Expression parsing (Pratt / recursive descent) ────────────────────────

    pub(crate) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(ParseError::RecursionLimitExceeded {
                limit: self.max_depth,
            });
        }
        let result = self.parse_or();
        self.depth -= 1;
        result
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_xor()?;
        while self.at(&TokenKind::Or) {
            self.advance();
            let right = self.parse_xor()?;
            left = Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_xor(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.at(&TokenKind::Xor) {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::BinaryOp {
                op: BinaryOp::Xor,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_not()?;
        while self.at(&TokenKind::And) {
            self.advance();
            let right = self.parse_not()?;
            left = Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(ParseError::RecursionLimitExceeded {
                limit: self.max_depth,
            });
        }
        let result = if self.eat(&TokenKind::Not) {
            let expr = self.parse_not()?;
            Ok(Expr::UnaryOp {
                op: UnaryOp::Not,
                expr: Box::new(expr),
            })
        } else {
            self.parse_comparison()
        };
        self.depth -= 1;
        result
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_in_pred()?;

        // Check for chained IS NULL / IS NOT NULL (post-fix predicates).
        left = self.parse_null_check(left)?;

        // Check for label check: `a:Label` or `a:Label1:Label2`
        // `a:A:B` is sugar for `a:A AND a:B` — rewrite to conjunctive AND.
        if self.at(&TokenKind::Colon)
            && self.pos + 1 < self.tokens.len()
            && (matches!(self.tokens[self.pos + 1].kind, TokenKind::Identifier)
                || is_keyword_usable_as_ident(&self.tokens[self.pos + 1].kind))
        {
            let label_inner = left.clone(); // the expression being label-checked
            let mut label_result: Option<Expr> = None;
            while self.at(&TokenKind::Colon) {
                if self.pos + 1 < self.tokens.len()
                    && (matches!(self.tokens[self.pos + 1].kind, TokenKind::Identifier)
                        || is_keyword_usable_as_ident(&self.tokens[self.pos + 1].kind))
                {
                    self.advance(); // consume ':'
                    let label_name = self.tokens[self.pos].text.to_string();
                    self.advance(); // consume label name
                    let check = Expr::HasLabel(
                        Box::new(label_inner.clone()),
                        Label::new(smol_str::SmolStr::new(&label_name)),
                    );
                    label_result = Some(match label_result {
                        None => check,
                        Some(prev) => Expr::BinaryOp {
                            op: BinaryOp::And,
                            left: Box::new(prev),
                            right: Box::new(check),
                        },
                    });
                } else {
                    break;
                }
            }
            if let Some(r) = label_result {
                left = r;
            }
        }

        // Track the previous right-hand side and operator for chained comparisons.
        // `a < b < c` → `(a < b) AND (b < c)` per openCypher semantics.
        let mut prev_right: Option<Expr> = None;
        loop {
            let op = match self.peek().clone() {
                TokenKind::Eq => BinaryOp::Eq,
                TokenKind::Neq => BinaryOp::Neq,
                TokenKind::Lt => BinaryOp::Lt,
                TokenKind::Lte => BinaryOp::Lte,
                TokenKind::Gt => BinaryOp::Gt,
                TokenKind::Gte => BinaryOp::Gte,
                TokenKind::RegexEq => BinaryOp::RegexMatch,
                _ => break,
            };
            self.advance();
            let right = self.parse_in_pred()?;
            let right = self.parse_null_check(right)?;

            if let Some(prev) = prev_right.take() {
                // Chained comparison: left is already `(a < b)`, now we have `< c`.
                // Build `(a < b) AND (b < c)` where `b` is the previous right operand.
                let chain_cmp = Expr::BinaryOp {
                    op,
                    left: Box::new(prev.clone()),
                    right: Box::new(right.clone()),
                };
                left = Expr::BinaryOp {
                    op: BinaryOp::And,
                    left: Box::new(left),
                    right: Box::new(chain_cmp),
                };
            } else {
                left = Expr::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right.clone()),
                };
            }
            prev_right = Some(right);
        }
        Ok(left)
    }

    /// Parse IN predicate — higher precedence than comparison operators.
    /// `expr IN list` binds tighter than `=`, `<>`, `<`, `>`, `<=`, `>=`.
    fn parse_in_pred(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_string_pred()?;
        while self.at(&TokenKind::In) {
            self.advance();
            let right = self.parse_string_pred()?;
            left = Expr::BinaryOp {
                op: BinaryOp::In,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// Handles postfix `IS NULL` / `IS NOT NULL`.
    fn parse_null_check(&mut self, expr: Expr) -> Result<Expr, ParseError> {
        if self.eat(&TokenKind::Is) {
            if self.eat(&TokenKind::Not) {
                self.expect(&TokenKind::Null)?;
                Ok(Expr::IsNotNull(Box::new(expr)))
            } else {
                self.expect(&TokenKind::Null)?;
                Ok(Expr::IsNull(Box::new(expr)))
            }
        } else {
            Ok(expr)
        }
    }

    /// Handles `STARTS WITH`, `ENDS WITH`, `CONTAINS` string predicates.
    fn parse_string_pred(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_add()?;
        match self.peek().clone() {
            TokenKind::Starts => {
                self.advance();
                // Expect "WITH" as an identifier (the lexer doesn't have a With keyword for this).
                let text = self.tokens[self.pos].text.to_ascii_lowercase();
                if text == "with" {
                    self.advance();
                } else {
                    return Err(self.syntax_err("WITH after STARTS"));
                }
                let right = self.parse_add()?;
                Ok(Expr::BinaryOp {
                    op: BinaryOp::StartsWith,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            TokenKind::Ends => {
                self.advance();
                let text = self.tokens[self.pos].text.to_ascii_lowercase();
                if text == "with" {
                    self.advance();
                } else {
                    return Err(self.syntax_err("WITH after ENDS"));
                }
                let right = self.parse_add()?;
                Ok(Expr::BinaryOp {
                    op: BinaryOp::EndsWith,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            TokenKind::Contains => {
                self.advance();
                let right = self.parse_add()?;
                Ok(Expr::BinaryOp {
                    op: BinaryOp::Contains,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            _ => Ok(left),
        }
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek().clone() {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Dash => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_pow()?;
        loop {
            let op = match self.peek().clone() {
                TokenKind::Star => BinaryOp::Mul,
                TokenKind::Slash => BinaryOp::Div,
                TokenKind::Percent => BinaryOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_pow()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// Parse exponentiation — higher precedence than `*`, `/`, `%`.
    /// Left-associative per Cypher spec: `a ^ b ^ c` = `(a ^ b) ^ c`.
    fn parse_pow(&mut self) -> Result<Expr, ParseError> {
        let mut base = self.parse_unary()?;
        while self.eat(&TokenKind::Caret) {
            let exp = self.parse_unary()?; // left-associative: rhs is just unary
            base = Expr::BinaryOp {
                op: BinaryOp::Pow,
                left: Box::new(base),
                right: Box::new(exp),
            };
        }
        Ok(base)
    }

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(ParseError::RecursionLimitExceeded {
                limit: self.max_depth,
            });
        }
        let result = if self.eat(&TokenKind::Dash) {
            // Special case: -9223372036854775808 = i64::MIN (the only valid use of
            // the 2^63 magnitude literal is immediately after unary minus).
            if self.at(&TokenKind::IntMinMagnitude) {
                self.advance();
                Ok(Expr::Integer(i64::MIN))
            } else {
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                })
            }
        } else if self.eat(&TokenKind::Plus) {
            // Unary plus is a no-op; just return the inner expression.
            self.parse_unary()
        } else {
            self.parse_postfix()
        };
        self.depth -= 1;
        result
    }

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek().clone() {
                // Property access: expr.key — but peek ahead for namespaced function call
                TokenKind::Dot => {
                    self.advance();
                    let key = SmolStr::new(self.expect_map_key()?);
                    // Check if this is a namespaced function call: `ns.func(args)`
                    if self.at(&TokenKind::LParen) {
                        // Build the dotted function name from the expression chain.
                        let func_name = match &expr {
                            Expr::Variable(id) => format!("{}.{}", id.0, key),
                            Expr::Property(inner, prev_key) => {
                                // Handle deeper nesting like a.b.c()
                                fn build_name(e: &Expr) -> Option<String> {
                                    match e {
                                        Expr::Variable(id) => Some(id.0.clone()),
                                        Expr::Property(inner, k) => {
                                            build_name(inner).map(|n| format!("{}.{}", n, k))
                                        }
                                        _ => None,
                                    }
                                }
                                match build_name(inner) {
                                    Some(base) => format!("{}.{}.{}", base, prev_key, key),
                                    None => format!("{}", key), // fallback
                                }
                            }
                            _ => key.to_string(),
                        };
                        self.advance(); // consume (
                        let distinct = self.eat(&TokenKind::Distinct);
                        let mut args = Vec::new();
                        if !self.at(&TokenKind::RParen) {
                            let first = self.parse_expr()?;
                            let first = self.maybe_rewrite_quantifier_arg(first)?;
                            args.push(first);
                            while self.eat(&TokenKind::Comma) {
                                args.push(self.parse_expr()?);
                            }
                        }
                        self.expect(&TokenKind::RParen)?;
                        expr = Expr::FunctionCall {
                            name: SmolStr::new(func_name),
                            args,
                            distinct,
                        };
                    } else {
                        expr = Expr::Property(Box::new(expr), key);
                    }
                }
                // Subscript or slice: expr[index] or expr[start..end]
                TokenKind::LBracket => {
                    self.advance();
                    if self.at(&TokenKind::DoubleDot) {
                        // [..end]
                        self.advance();
                        let end = if self.at(&TokenKind::RBracket) {
                            None
                        } else {
                            Some(Box::new(self.parse_expr()?))
                        };
                        self.expect(&TokenKind::RBracket)?;
                        expr = Expr::Slice(Box::new(expr), None, end);
                    } else {
                        let idx = self.parse_expr()?;
                        if self.eat(&TokenKind::DoubleDot) {
                            // [start..end]
                            let end = if self.at(&TokenKind::RBracket) {
                                None
                            } else {
                                Some(Box::new(self.parse_expr()?))
                            };
                            self.expect(&TokenKind::RBracket)?;
                            expr = Expr::Slice(Box::new(expr), Some(Box::new(idx)), end);
                        } else {
                            self.expect(&TokenKind::RBracket)?;
                            expr = Expr::Subscript(Box::new(expr), Box::new(idx));
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
        match self.peek().clone() {
            // Integer literal
            TokenKind::IntLiteral(n) => {
                self.advance();
                Ok(Expr::Integer(n))
            }
            // 9223372036854775808 (2^63) alone is too large — only valid after unary minus.
            TokenKind::IntMinMagnitude => {
                let tok = self.advance();
                Err(ParseError::IntegerOverflow {
                    literal: tok.text.to_string(),
                    offset: tok.offset,
                })
            }
            // Float literal
            TokenKind::FloatLiteral(f) => {
                self.advance();
                Ok(Expr::Float(f))
            }
            // String literal
            TokenKind::StringLiteral(s) => {
                self.advance();
                Ok(Expr::Str(s.into_boxed_str()))
            }
            // Boolean true
            TokenKind::True => {
                self.advance();
                Ok(Expr::Bool(true))
            }
            // Boolean false
            TokenKind::False => {
                self.advance();
                Ok(Expr::Bool(false))
            }
            // NULL
            TokenKind::Null => {
                self.advance();
                Ok(Expr::Null)
            }
            // Parameter: $name or $1 (numeric parameter names are valid in Cypher)
            TokenKind::DollarSign => {
                self.advance();
                let name = match self.peek().clone() {
                    TokenKind::IntLiteral(n) => {
                        let text = self.tokens[self.pos].text.to_string();
                        self.advance();
                        let _ = n;
                        text
                    }
                    _ => self.expect_identifier()?,
                };
                Ok(Expr::Parameter(name))
            }
            // CASE expression
            TokenKind::Case => {
                self.advance();
                self.parse_case()
            }
            // List literal or list comprehension: [...]
            TokenKind::LBracket => {
                self.advance();
                self.parse_list_or_comprehension()
            }
            // Map literal: {...}
            TokenKind::LBrace => {
                let map = self.parse_map_literal()?;
                Ok(Expr::MapLiteral(map))
            }
            // Parenthesised expression or pattern expression
            TokenKind::LParen => {
                // Try to detect pattern expression: `(n)-[...]->()` etc.
                // A pattern expression starts with `(` followed by identifier/label
                // and continues with a relationship indicator `-` or has no `)` before `-`.
                if self.looks_like_pattern_expr() {
                    return self.parse_pattern_expr();
                }
                self.advance();
                let expr = self.parse_expr()?;
                self.expect(&TokenKind::RParen)?;
                Ok(expr)
            }
            // count(*) — special case; also allow `count` as a plain identifier
            TokenKind::Count => {
                self.advance();
                if !self.at(&TokenKind::LParen) {
                    // Used as a plain variable reference (e.g. `RETURN count` where count is an alias)
                    return Ok(Expr::Variable(Identifier::new("count")));
                }
                self.advance(); // consume (
                let distinct = self.eat(&TokenKind::Distinct);
                let args = if self.at(&TokenKind::Star) {
                    self.advance();
                    vec![Expr::Variable(Identifier::new("*"))]
                } else if self.at(&TokenKind::RParen) {
                    vec![]
                } else {
                    let mut a = vec![self.parse_expr()?];
                    while self.eat(&TokenKind::Comma) {
                        a.push(self.parse_expr()?);
                    }
                    a
                };
                self.expect(&TokenKind::RParen)?;
                Ok(Expr::FunctionCall {
                    name: SmolStr::new("count"),
                    args,
                    distinct,
                })
            }
            // Identifier or function call
            TokenKind::Identifier => {
                let name_text = self.tokens[self.pos].text.to_string();

                // Handle `exists { pattern }` — existential subquery
                if name_text.eq_ignore_ascii_case("exists")
                    && matches!(self.peek2(), TokenKind::LBrace)
                {
                    self.advance(); // consume `exists`
                    return self.parse_exists_subquery();
                }

                self.advance();

                if self.at(&TokenKind::LParen) {
                    self.parse_func_call(name_text)
                } else {
                    Ok(Expr::Variable(Identifier::new(name_text)))
                }
            }
            // Keywords usable as identifiers / function names
            k if is_keyword_usable_as_ident(&k) => {
                let name_text = self.tokens[self.pos].text.to_string();
                self.advance();
                if self.at(&TokenKind::LParen) {
                    self.parse_func_call(name_text)
                } else {
                    Ok(Expr::Variable(Identifier::new(name_text)))
                }
            }
            _ => Err(self.syntax_err("expression")),
        }
    }

    // ── Function call ─────────────────────────────────────────────────────────

    /// Parse a function call: `name(args)`. The name has already been consumed;
    /// the current token is `(`.
    ///
    /// Also handles the quantifier pattern: `all(x IN list WHERE pred)`.
    fn parse_func_call(&mut self, name_text: String) -> Result<Expr, ParseError> {
        self.advance(); // consume (
        let distinct = self.eat(&TokenKind::Distinct);
        let mut args = Vec::new();
        if !self.at(&TokenKind::RParen) {
            if self.at(&TokenKind::Star) {
                self.advance();
                args.push(Expr::Variable(Identifier::new("*")));
            } else {
                let first = self.parse_expr()?;
                // Rewrite quantifier pattern: func(x IN list WHERE pred)
                let first = self.maybe_rewrite_quantifier_arg(first)?;
                args.push(first);
                while self.eat(&TokenKind::Comma) {
                    args.push(self.parse_expr()?);
                }
            }
        }
        self.expect(&TokenKind::RParen)?;
        Ok(Expr::FunctionCall {
            name: SmolStr::new(name_text),
            args,
            distinct,
        })
    }

    /// If the parsed argument is `x IN list` and the current token is WHERE,
    /// rewrite it as a ListComp quantifier argument:
    /// `all(x IN list WHERE pred)` → `FunctionCall("all", [ListComp(...)])`
    fn maybe_rewrite_quantifier_arg(&mut self, arg: Expr) -> Result<Expr, ParseError> {
        if !self.at(&TokenKind::Where) {
            return Ok(arg);
        }
        match arg {
            Expr::BinaryOp {
                op: BinaryOp::In,
                left,
                right,
            } => {
                if let Expr::Variable(ref var) = *left {
                    self.advance(); // consume WHERE
                    let pred = self.parse_expr()?;
                    Ok(Expr::ListComp {
                        variable: var.clone(),
                        list: right,
                        filter: Some(Box::new(pred)),
                        map: Box::new(Expr::Variable(var.clone())),
                    })
                } else {
                    // Not a simple variable on the left of IN — don't rewrite.
                    Ok(Expr::BinaryOp {
                        op: BinaryOp::In,
                        left,
                        right,
                    })
                }
            }
            other => Ok(other),
        }
    }

    // ── CASE expression ───────────────────────────────────────────────────────

    /// Parse CASE after the `CASE` keyword has been consumed.
    fn parse_case(&mut self) -> Result<Expr, ParseError> {
        // Check whether there is a subject expression before WHEN.
        let subject = if !self.at(&TokenKind::When) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };

        let mut alternatives = Vec::new();
        while self.eat(&TokenKind::When) {
            let cond = self.parse_expr()?;
            self.expect(&TokenKind::Then)?;
            let result = self.parse_expr()?;
            alternatives.push((cond, result));
        }

        let default = if self.eat(&TokenKind::Else) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };

        self.expect(&TokenKind::End)?;

        Ok(Expr::Case {
            subject,
            alternatives,
            default,
        })
    }

    // ── Existential subquery ───────────────────────────────────────────────────

    /// Parse `exists { pattern [WHERE predicate] }` or `exists { MATCH pattern ... RETURN ... }`
    /// after `exists` has been consumed.
    fn parse_exists_subquery(&mut self) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::LBrace)?;
        let mut patterns = Vec::new();
        // Parse MATCH patterns inside the subquery (without the MATCH keyword).
        // The first pattern starts with `(`, or there's an explicit MATCH keyword.
        if self.at(&TokenKind::Match) {
            // Full subquery with MATCH ... [WITH ... WHERE ... RETURN ...]
            self.advance();
            patterns.push(self.parse_path_pattern()?);
            while self.eat(&TokenKind::Comma) {
                patterns.push(self.parse_path_pattern()?);
            }
            let where_clause = if self.eat(&TokenKind::Where) {
                Some(Box::new(self.parse_expr()?))
            } else {
                None
            };
            // Check for write clauses; parse additional clauses into a full inner Query.
            let mut additional_clauses: Vec<Clause> = Vec::new();
            while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
                if matches!(
                    self.tokens[self.pos].kind,
                    TokenKind::Set
                        | TokenKind::Create
                        | TokenKind::Delete
                        | TokenKind::Merge
                        | TokenKind::Remove
                ) {
                    return Err(ParseError::Syntax {
                        offset: self.tokens[self.pos].offset,
                        expected: "write clause not allowed inside existential subquery"
                            .to_string(),
                        found: String::new(),
                    });
                }
                if let Some(clause) = self.parse_clause()? {
                    additional_clauses.push(clause);
                }
            }
            self.expect(&TokenKind::RBrace)?;
            let full_query = if additional_clauses.is_empty() {
                None
            } else {
                // Build a full inner Query: MATCH(patterns+where) + additional clauses
                let mc = MatchClause {
                    patterns: patterns.clone(),
                    where_clause: where_clause.as_deref().cloned(),
                };
                let mut inner_clauses = vec![Clause::Match(mc)];
                inner_clauses.extend(additional_clauses);
                Some(Box::new(Query {
                    clauses: inner_clauses,
                    unions: Vec::new(),
                }))
            };
            return Ok(Expr::ExistsSubquery {
                patterns,
                where_clause,
                full_query,
            });
        }
        // Simple pattern form: `exists { (n)-->() }`
        patterns.push(self.parse_path_pattern()?);
        while self.eat(&TokenKind::Comma) {
            patterns.push(self.parse_path_pattern()?);
        }
        let where_clause = if self.eat(&TokenKind::Where) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect(&TokenKind::RBrace)?;
        Ok(Expr::ExistsSubquery {
            patterns,
            where_clause,
            full_query: None,
        })
    }

    // ── Pattern expression ───────────────────────────────────────────────────

    /// Check if the current position looks like a pattern expression.
    /// Scans ahead from `(` to find the matching `)` and checks if it's
    /// followed by `-` or `<-` (relationship indicator).
    ///
    /// Also checks that the token immediately after `(` looks like a node pattern
    /// start (identifier, label colon, property map, or empty `()`) — not an
    /// arithmetic expression like `(4 ^ 3) - ...` or `(-3) - ...`.
    fn looks_like_pattern_expr(&self) -> bool {
        // The first token inside `(` must look like a node pattern start:
        // an identifier, `:Label`, `{` property map, or `)` (empty node).
        // If it's a literal (integer, float, string) or an operator, it's a
        // parenthesised arithmetic expression, not a pattern.
        let first_inside = self.pos + 1;
        if first_inside >= self.tokens.len() {
            return false;
        }
        match &self.tokens[first_inside].kind {
            // Literals and pure operators are never pattern starters.
            TokenKind::IntLiteral(_)
            | TokenKind::FloatLiteral(_)
            | TokenKind::StringLiteral(_)
            | TokenKind::True
            | TokenKind::False
            | TokenKind::Null
            | TokenKind::Plus
            | TokenKind::Dash
            | TokenKind::Star
            | TokenKind::Slash
            | TokenKind::Percent
            | TokenKind::Caret
            | TokenKind::LBracket => return false,
            // `{` can start a node-pattern property map: ({ name: 'x' })
            // but only when followed by a relationship indicator after closing `)`.
            // Fall through to the depth-scan check below.
            _ => {}
        }
        // Also reject if the token is not a recognized pattern start.
        {
            let first_inner = &self.tokens[first_inside].kind;
            let is_pattern_start = matches!(
                first_inner,
                TokenKind::Identifier | TokenKind::Colon | TokenKind::LBrace | TokenKind::RParen
            ) || is_keyword_usable_as_ident(first_inner);
            if !is_pattern_start {
                return false;
            }
        }

        // We need to find the matching `)` for the current `(` and check
        // if it's followed by a relationship indicator.
        let mut depth = 0;
        let mut i = self.pos;
        while i < self.tokens.len() {
            match &self.tokens[i].kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        // Check what follows the closing paren.
                        let next_idx = (i + 1).min(self.tokens.len() - 1);
                        return matches!(
                            self.tokens[next_idx].kind,
                            TokenKind::Dash | TokenKind::LeftArrow
                        );
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// Parse a pattern expression: `(n)-[:REL]->(m)` used as a boolean predicate.
    fn parse_pattern_expr(&mut self) -> Result<Expr, ParseError> {
        let mut patterns = Vec::new();
        patterns.push(self.parse_path_pattern()?);
        while self.eat(&TokenKind::Comma) {
            patterns.push(self.parse_path_pattern()?);
        }
        Ok(Expr::PatternExpr(patterns))
    }

    /// Check if we're at the start of a pattern comprehension inside `[...]`.
    /// Scans ahead to find `|` at the same bracket depth (inside `[`).
    fn looks_like_pattern_comprehension(&self) -> bool {
        let mut depth_paren = 0;
        let mut depth_bracket = 0; // we're already inside one `[`
        let mut i = self.pos;
        while i < self.tokens.len() {
            match &self.tokens[i].kind {
                TokenKind::LParen => depth_paren += 1,
                TokenKind::RParen => depth_paren -= 1,
                TokenKind::LBracket => depth_bracket += 1,
                TokenKind::RBracket => {
                    if depth_bracket == 0 {
                        return false; // hit the outer `]` without finding `|`
                    }
                    depth_bracket -= 1;
                }
                TokenKind::Pipe => {
                    if depth_paren == 0 && depth_bracket == 0 {
                        return true; // found `|` at the right level
                    }
                }
                TokenKind::In => {
                    // If we find `IN` at the top level, it's a regular list comprehension
                    if depth_paren == 0 && depth_bracket == 0 {
                        return false;
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    // ── List literal / list comprehension ─────────────────────────────────────

    /// Parse after `[` has been consumed.
    ///
    /// Forms:
    /// - `[expr, expr, ...]`          — list literal
    /// - `[x IN expr | mapExpr]`      — list comprehension
    /// - `[x IN expr WHERE pred | mapExpr]` — filtered list comprehension
    fn parse_list_or_comprehension(&mut self) -> Result<Expr, ParseError> {
        // Empty list.
        if self.at(&TokenKind::RBracket) {
            self.advance();
            return Ok(Expr::List(Vec::new()));
        }

        // Check for pattern comprehension: `[(pattern) | mapExpr]`
        // A pattern starts with `(` and the pattern is followed by `|` before `]`.
        if self.at(&TokenKind::LParen) && self.looks_like_pattern_comprehension() {
            let pattern = self.parse_path_pattern()?;
            let where_clause = if self.eat(&TokenKind::Where) {
                Some(Box::new(self.parse_expr()?))
            } else {
                None
            };
            self.expect(&TokenKind::Pipe)?;
            let map_expr = self.parse_expr()?;
            self.expect(&TokenKind::RBracket)?;
            // Model as a PatternExpr wrapped in a list context
            // We reuse ExistsSubquery temporarily but should use a proper PatternComp
            // For now, return as a FunctionCall to __patternComp which the executor can handle
            return Ok(Expr::FunctionCall {
                name: SmolStr::new("__patternComp"),
                args: vec![
                    Expr::PatternExpr(vec![pattern]),
                    map_expr,
                    match where_clause {
                        Some(w) => *w,
                        None => Expr::Bool(true),
                    },
                ],
                distinct: false,
            });
        }

        // Check for named-path pattern comprehension: `[p = (pattern) | mapExpr]`
        // Detect: identifier followed by `=` followed by `(`
        let is_named_pattern_comp = matches!(self.peek(), TokenKind::Identifier)
            && self.pos + 1 < self.tokens.len()
            && matches!(self.tokens[self.pos + 1].kind, TokenKind::Eq)
            && self.pos + 2 < self.tokens.len()
            && matches!(self.tokens[self.pos + 2].kind, TokenKind::LParen);
        if is_named_pattern_comp {
            let path_var = self.expect_identifier()?; // consume path variable name
            self.expect(&TokenKind::Eq)?; // consume `=`
            let mut pattern = self.parse_path_pattern()?;
            // Bind the path variable in the PathPattern so the executor can
            // store the assembled path under that name (e.g. `p`).
            if pattern.variable.is_none() {
                pattern.variable = Some(crate::ast::Identifier::new(path_var));
            }
            let where_clause = if self.eat(&TokenKind::Where) {
                Some(Box::new(self.parse_expr()?))
            } else {
                None
            };
            self.expect(&TokenKind::Pipe)?;
            let map_expr = self.parse_expr()?;
            self.expect(&TokenKind::RBracket)?;
            return Ok(Expr::FunctionCall {
                name: SmolStr::new("__patternComp"),
                args: vec![
                    Expr::PatternExpr(vec![pattern]),
                    map_expr,
                    match where_clause {
                        Some(w) => *w,
                        None => Expr::Bool(true),
                    },
                ],
                distinct: false,
            });
        }

        // Peek for list comprehension pattern: `ident IN ...`
        // We check: current token is Identifier AND peek2 is `IN`.
        let is_comprehension =
            matches!(self.peek(), TokenKind::Identifier) && matches!(self.peek2(), TokenKind::In);

        if is_comprehension {
            let var_name = self.expect_identifier()?;
            self.expect(&TokenKind::In)?;
            let list_expr = self.parse_expr()?;

            // Optional WHERE clause inside comprehension.
            let filter = if self.eat(&TokenKind::Where) {
                Some(self.parse_expr()?)
            } else {
                None
            };

            // Optional `| mapExpr` separator.
            let map_expr = if self.eat(&TokenKind::Pipe) {
                self.parse_expr()?
            } else {
                Expr::Variable(Identifier::new(var_name.clone()))
            };

            self.expect(&TokenKind::RBracket)?;

            Ok(Expr::ListComp {
                variable: Identifier::new(var_name),
                list: Box::new(list_expr),
                filter: filter.map(Box::new),
                map: Box::new(map_expr),
            })
        } else {
            // Ordinary list literal.
            let mut items = Vec::new();
            items.push(self.parse_expr()?);
            while self.eat(&TokenKind::Comma) {
                if self.at(&TokenKind::RBracket) {
                    break; // trailing comma
                }
                items.push(self.parse_expr()?);
            }
            self.expect(&TokenKind::RBracket)?;
            Ok(Expr::List(items))
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    fn parse(src: &str) -> Query {
        parse_query(src).unwrap_or_else(|e| panic!("parse error for {:?}: {}", src, e))
    }

    fn parse_err(src: &str) -> ParseError {
        parse_query(src).expect_err("expected a parse error")
    }

    // ── Basic MATCH/RETURN ────────────────────────────────────────────────────

    #[test]
    fn simple_match_return() {
        let q = parse("MATCH (n) RETURN n");
        assert_eq!(q.clauses.len(), 2);
        assert!(matches!(&q.clauses[0], Clause::Match(_)));
        assert!(matches!(&q.clauses[1], Clause::Return(_)));
    }

    #[test]
    fn match_with_label() {
        let q = parse("MATCH (n:Person) RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let node = &mc.patterns[0].start;
        assert_eq!(node.labels, vec![Label::new("Person")]);
    }

    #[test]
    fn match_return_property() {
        let q = parse("MATCH (n:Person) RETURN n.name");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::Property(_, key) if key == "name"));
    }

    #[test]
    fn return_with_alias() {
        let q = parse("MATCH (n) RETURN n.name AS personName");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert_eq!(rc.items[0].alias, Some(Identifier::new("personName")));
    }

    #[test]
    fn return_distinct() {
        let q = parse("MATCH (n:Person) RETURN DISTINCT n.name");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(rc.distinct);
    }

    // ── WITH chaining ─────────────────────────────────────────────────────────

    #[test]
    fn with_chain() {
        let q = parse("MATCH (n) WITH n MATCH (n)-[r]->(m) RETURN m");
        assert_eq!(q.clauses.len(), 4);
        assert!(matches!(&q.clauses[1], Clause::With(_)));
    }

    #[test]
    fn with_where() {
        let q = parse("MATCH (n) WITH n WHERE n.age > 18 RETURN n");
        let Clause::With(wc) = &q.clauses[1] else {
            panic!()
        };
        assert!(wc.where_clause.is_some());
    }

    // ── Label disjunction ─────────────────────────────────────────────────────

    #[test]
    fn label_disjunction_pipe() {
        let q = parse("MATCH (n:Person|Animal) RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let node = &mc.patterns[0].start;
        assert_eq!(node.labels.len(), 2);
        assert_eq!(node.labels[0], Label::new("Person"));
        assert_eq!(node.labels[1], Label::new("Animal"));
    }

    #[test]
    fn label_colon_list() {
        let q = parse("MATCH (n:Person:Employee) RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.patterns[0].start.labels.len(), 2);
    }

    // ── Variable-length paths ─────────────────────────────────────────────────

    #[test]
    fn variable_length_range() {
        let q = parse("MATCH (a)-[r:KNOWS*1..5]->(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let step = &mc.patterns[0].steps[0];
        let range = step.rel.range.as_ref().unwrap();
        assert_eq!(range.min, Some(1));
        assert_eq!(range.max, Some(5));
    }

    #[test]
    fn variable_length_star_only() {
        let q = parse("MATCH (a)-[*]->(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let step = &mc.patterns[0].steps[0];
        let range = step.rel.range.as_ref().unwrap();
        assert_eq!(range.min, None);
        assert_eq!(range.max, None);
    }

    #[test]
    fn variable_length_exact() {
        let q = parse("MATCH (a)-[*3]->(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let range = mc.patterns[0].steps[0].rel.range.as_ref().unwrap();
        // *3 with no DoubleDot means exactly 3.
        assert_eq!(range.min, Some(3));
    }

    // ── Property maps ─────────────────────────────────────────────────────────

    #[test]
    fn node_with_property_map() {
        let q = parse("MATCH (n:Person {name: 'Alice', age: 30}) RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let Properties::Map(props) = mc.patterns[0].start.properties.as_ref().unwrap() else {
            panic!("expected Properties::Map");
        };
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].0, "name");
        assert!(matches!(&props[0].1, Expr::Str(s) if s.as_ref() == "Alice"));
        assert_eq!(props[1].0, "age");
        assert!(matches!(&props[1].1, Expr::Integer(30)));
    }

    // ── WHERE with complex expressions ────────────────────────────────────────

    #[test]
    fn where_and_expression() {
        let q = parse("MATCH (n) WHERE n.age >= 18 AND n.age < 65 RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(mc.where_clause.is_some());
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::And,
                ..
            }
        ));
    }

    #[test]
    fn where_or_expression() {
        let q = parse("MATCH (n) WHERE n.age < 18 OR n.age > 65 RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::Or,
                ..
            }
        ));
    }

    #[test]
    fn where_not_expression() {
        let q = parse("MATCH (n) WHERE NOT n.active RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    // ── CREATE ────────────────────────────────────────────────────────────────

    #[test]
    fn create_single_node() {
        let q = parse("CREATE (n:Person {name: 'Bob'})");
        assert_eq!(q.clauses.len(), 1);
        let Clause::Create(cc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(cc.patterns.len(), 1);
    }

    #[test]
    fn create_relationship() {
        let q = parse("CREATE (a:Person)-[:KNOWS]->(b:Person)");
        let Clause::Create(cc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(cc.patterns[0].steps.len(), 1);
        assert_eq!(cc.patterns[0].steps[0].rel.direction, Direction::Outgoing);
        assert_eq!(
            cc.patterns[0].steps[0].rel.rel_types,
            vec![RelType::new("KNOWS")]
        );
    }

    #[test]
    fn create_multiple_patterns() {
        let q = parse("CREATE (a:A), (b:B), (a)-[:R]->(b)");
        let Clause::Create(cc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(cc.patterns.len(), 3);
    }

    // ── MERGE ON CREATE SET / ON MATCH SET ────────────────────────────────────

    #[test]
    fn merge_on_create_set() {
        let q = parse("MERGE (n:Person {name: 'Alice'}) ON CREATE SET n.created = true");
        let Clause::Merge(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.on_create.len(), 1);
        assert!(mc.on_match.is_empty());
    }

    #[test]
    fn merge_on_match_set() {
        let q = parse("MERGE (n:Person {name: 'Bob'}) ON MATCH SET n.updated = true");
        let Clause::Merge(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(mc.on_create.is_empty());
        assert_eq!(mc.on_match.len(), 1);
    }

    #[test]
    fn merge_both_actions() {
        let q = parse("MERGE (n:Person {name: 'Carol'}) ON CREATE SET n.created = true ON MATCH SET n.updated = true");
        let Clause::Merge(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.on_create.len(), 1);
        assert_eq!(mc.on_match.len(), 1);
    }

    // ── ORDER BY + LIMIT + SKIP ───────────────────────────────────────────────

    #[test]
    fn return_order_by_limit_skip() {
        let q = parse("MATCH (n) RETURN n ORDER BY n.name SKIP 10 LIMIT 5");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert_eq!(rc.order_by.len(), 1);
        assert_eq!(rc.skip, Some(Expr::Integer(10)));
        assert_eq!(rc.limit, Some(Expr::Integer(5)));
    }

    #[test]
    fn order_by_desc() {
        let q = parse("MATCH (n) RETURN n ORDER BY n.age DESC");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(!rc.order_by[0].ascending);
    }

    #[test]
    fn order_by_multiple() {
        let q = parse("MATCH (n) RETURN n ORDER BY n.age ASC, n.name DESC");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert_eq!(rc.order_by.len(), 2);
        assert!(rc.order_by[0].ascending);
        assert!(!rc.order_by[1].ascending);
    }

    // ── All comparison operators ──────────────────────────────────────────────

    #[test]
    fn comparison_eq() {
        let q = parse("MATCH (n) WHERE n.x = 1 RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                ..
            }
        ));
    }

    #[test]
    fn comparison_neq() {
        let q = parse("MATCH (n) WHERE n.x <> 1 RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::Neq,
                ..
            }
        ));
    }

    #[test]
    fn comparison_lt_lte_gt_gte() {
        for (op_str, expected_op) in [
            ("< 5", BinaryOp::Lt),
            ("<= 5", BinaryOp::Lte),
            ("> 5", BinaryOp::Gt),
            (">= 5", BinaryOp::Gte),
        ] {
            let q = parse(&format!("MATCH (n) WHERE n.x {} RETURN n", op_str));
            let Clause::Match(mc) = &q.clauses[0] else {
                panic!()
            };
            match mc.where_clause.as_ref().unwrap() {
                Expr::BinaryOp { op, .. } => assert_eq!(*op, expected_op),
                _ => panic!("expected BinaryOp"),
            }
        }
    }

    #[test]
    fn comparison_in() {
        let q = parse("MATCH (n) WHERE n.x IN [1, 2, 3] RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::In,
                ..
            }
        ));
    }

    // ── Error cases ───────────────────────────────────────────────────────────

    #[test]
    fn error_unterminated_node_pattern() {
        let err = parse_err("MATCH (n");
        assert!(matches!(err, ParseError::Syntax { .. }));
    }

    #[test]
    fn error_empty_query() {
        let err = parse_err("");
        assert!(matches!(err, ParseError::EmptyQuery));
    }

    #[test]
    fn error_whitespace_only() {
        let err = parse_err("   ");
        assert!(matches!(err, ParseError::EmptyQuery));
    }

    #[test]
    fn error_no_clause() {
        let err = parse_err("42");
        assert!(matches!(err, ParseError::Syntax { .. }));
    }

    // ── Parameters ────────────────────────────────────────────────────────────

    #[test]
    fn parameter_in_where() {
        let q = parse("MATCH (n) WHERE n.name = $name RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        match mc.where_clause.as_ref().unwrap() {
            Expr::BinaryOp { right, .. } => {
                assert!(matches!(right.as_ref(), Expr::Parameter(p) if p == "name"));
            }
            _ => panic!("expected BinaryOp"),
        }
    }

    #[test]
    fn parameter_in_create() {
        let q = parse("CREATE (n:Person {name: $name})");
        let Clause::Create(cc) = &q.clauses[0] else {
            panic!()
        };
        let Properties::Map(props) = cc.patterns[0].start.properties.as_ref().unwrap() else {
            panic!("expected Properties::Map");
        };
        assert!(matches!(&props[0].1, Expr::Parameter(p) if p == "name"));
    }

    #[test]
    fn parameter_map_in_create() {
        let q = parse("CREATE (n:Person $create_params)");
        let Clause::Create(cc) = &q.clauses[0] else {
            panic!()
        };
        let props = cc.patterns[0].start.properties.as_ref().unwrap();
        assert!(
            matches!(props, Properties::Parameter(name) if name == "create_params"),
            "expected Properties::Parameter('create_params'), got {:?}",
            props
        );
    }

    #[test]
    fn parameter_map_in_match() {
        let q = parse("MATCH (n:Person $match_params) RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let props = mc.patterns[0].start.properties.as_ref().unwrap();
        assert!(
            matches!(props, Properties::Parameter(name) if name == "match_params"),
            "expected Properties::Parameter('match_params'), got {:?}",
            props
        );
    }

    #[test]
    fn parameter_map_in_merge() {
        let q = parse("MERGE (n:Person $merge_params) RETURN n");
        let Clause::Merge(mc) = &q.clauses[0] else {
            panic!()
        };
        let props = mc.pattern.start.properties.as_ref().unwrap();
        assert!(
            matches!(props, Properties::Parameter(name) if name == "merge_params"),
            "expected Properties::Parameter('merge_params'), got {:?}",
            props
        );
    }

    // ── CASE expression ───────────────────────────────────────────────────────

    #[test]
    fn case_simple() {
        let q = parse("RETURN CASE 1 WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(
            matches!(&rc.items[0].expr, Expr::Case { subject: Some(_), alternatives, default: Some(_) } if alternatives.len() == 2)
        );
    }

    #[test]
    fn case_generic() {
        let q = parse("RETURN CASE WHEN 1 = 1 THEN true ELSE false END");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(
            matches!(&rc.items[0].expr, Expr::Case { subject: None, alternatives, .. } if alternatives.len() == 1)
        );
    }

    // ── Function calls ────────────────────────────────────────────────────────

    #[test]
    fn count_star() {
        let q = parse("MATCH (n) RETURN count(*)");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::FunctionCall { name, .. } if name == "count"));
    }

    #[test]
    fn count_distinct() {
        let q = parse("MATCH (n) RETURN count(DISTINCT n)");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        match &rc.items[0].expr {
            Expr::FunctionCall { name, distinct, .. } => {
                assert_eq!(name, "count");
                assert!(distinct);
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn function_call_size() {
        let q = parse("MATCH (n) RETURN size(n.name)");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::FunctionCall { name, .. } if name == "size"));
    }

    #[test]
    fn function_call_no_args() {
        let q = parse("RETURN timestamp()");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(
            matches!(&rc.items[0].expr, Expr::FunctionCall { name, args, .. } if name == "timestamp" && args.is_empty())
        );
    }

    // ── IS NULL / IS NOT NULL ─────────────────────────────────────────────────

    #[test]
    fn is_null() {
        let q = parse("MATCH (n) WHERE n.name IS NULL RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(mc.where_clause.as_ref().unwrap(), Expr::IsNull(_)));
    }

    #[test]
    fn is_not_null() {
        let q = parse("MATCH (n) WHERE n.name IS NOT NULL RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::IsNotNull(_)
        ));
    }

    // ── SET and REMOVE ────────────────────────────────────────────────────────

    #[test]
    fn set_property() {
        let q = parse("MATCH (n) SET n.name = 'Alice'");
        let Clause::Set(sc) = &q.clauses[1] else {
            panic!()
        };
        assert!(matches!(&sc.items[0], SetClauseItem::SetProperty { prop, .. } if prop == "name"));
    }

    #[test]
    fn set_label() {
        let q = parse("MATCH (n) SET n:Person");
        let Clause::Set(sc) = &q.clauses[1] else {
            panic!()
        };
        assert!(
            matches!(&sc.items[0], SetClauseItem::SetLabels { labels, .. } if labels[0] == Label::new("Person"))
        );
    }

    #[test]
    fn remove_property() {
        let q = parse("MATCH (n) REMOVE n.name");
        let Clause::Remove(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(
            matches!(&rc.items[0], RemoveClauseItem::RemoveProperty { prop, .. } if prop == "name")
        );
    }

    #[test]
    fn remove_label() {
        let q = parse("MATCH (n:Person) REMOVE n:Person");
        let Clause::Remove(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(
            matches!(&rc.items[0], RemoveClauseItem::RemoveLabels { labels, .. } if !labels.is_empty())
        );
    }

    // ── UNWIND ────────────────────────────────────────────────────────────────

    #[test]
    fn unwind_clause() {
        let q = parse("UNWIND [1, 2, 3] AS x RETURN x");
        assert!(matches!(&q.clauses[0], Clause::Unwind(_)));
        let Clause::Unwind(uc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(uc.alias, Identifier::new("x"));
    }

    // ── DELETE ────────────────────────────────────────────────────────────────

    #[test]
    fn delete_node() {
        let q = parse("MATCH (n) DELETE n");
        let Clause::Delete(dc) = &q.clauses[1] else {
            panic!()
        };
        assert!(!dc.detach);
        assert_eq!(dc.exprs.len(), 1);
    }

    #[test]
    fn detach_delete() {
        let q = parse("MATCH (n) DETACH DELETE n");
        let Clause::Delete(dc) = &q.clauses[1] else {
            panic!()
        };
        assert!(dc.detach);
    }

    // ── Relationship direction ────────────────────────────────────────────────

    #[test]
    fn incoming_relationship() {
        let q = parse("MATCH (a)<-[r:KNOWS]-(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.patterns[0].steps[0].rel.direction, Direction::Incoming);
    }

    #[test]
    fn undirected_relationship() {
        let q = parse("MATCH (a)-[r]-(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.patterns[0].steps[0].rel.direction, Direction::Undirected);
    }

    // ── Path variable binding ─────────────────────────────────────────────────

    #[test]
    fn path_variable() {
        let q = parse("MATCH p = (a)-[:R]->(b) RETURN p");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.patterns[0].variable, Some(Identifier::new("p")));
    }

    // ── Arithmetic expressions ────────────────────────────────────────────────

    #[test]
    fn arithmetic_precedence() {
        // 2 + 3 * 4 should parse as 2 + (3 * 4)
        let q = parse("RETURN 2 + 3 * 4");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        match &rc.items[0].expr {
            Expr::BinaryOp {
                op: BinaryOp::Add,
                right,
                ..
            } => {
                assert!(matches!(
                    right.as_ref(),
                    Expr::BinaryOp {
                        op: BinaryOp::Mul,
                        ..
                    }
                ));
            }
            _ => panic!("expected Add at top level"),
        }
    }

    #[test]
    fn unary_negation() {
        let q = parse("RETURN -42");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            &rc.items[0].expr,
            Expr::UnaryOp {
                op: UnaryOp::Neg,
                ..
            }
        ));
    }

    // ── List literals ─────────────────────────────────────────────────────────

    #[test]
    fn list_literal() {
        let q = parse("RETURN [1, 2, 3]");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::List(items) if items.len() == 3));
    }

    #[test]
    fn empty_list() {
        let q = parse("RETURN []");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::List(items) if items.is_empty()));
    }

    // ── Map literal ───────────────────────────────────────────────────────────

    #[test]
    fn map_literal_expr() {
        let q = parse("RETURN {name: 'Alice', age: 30}");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::MapLiteral(pairs) if pairs.len() == 2));
    }

    // ── Regex match ───────────────────────────────────────────────────────────

    #[test]
    fn regex_match_operator() {
        let q = parse("MATCH (n) WHERE n.name =~ 'A.*' RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::RegexMatch,
                ..
            }
        ));
    }

    // ── Rel type disjunction ──────────────────────────────────────────────────

    #[test]
    fn rel_type_disjunction() {
        let q = parse("MATCH (a)-[r:KNOWS|LIKES]->(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let rel = &mc.patterns[0].steps[0].rel;
        assert_eq!(rel.rel_types.len(), 2);
        assert_eq!(rel.rel_types[0], RelType::new("KNOWS"));
        assert_eq!(rel.rel_types[1], RelType::new("LIKES"));
    }

    // ── Subscript access ──────────────────────────────────────────────────────

    #[test]
    fn subscript_access() {
        let q = parse("RETURN list[0]");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::Subscript(_, _)));
    }

    // ── STARTS WITH / ENDS WITH / CONTAINS ───────────────────────────────────

    #[test]
    fn starts_with() {
        let q = parse("MATCH (n) WHERE n.name STARTS WITH 'A' RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::StartsWith,
                ..
            }
        ));
    }

    #[test]
    fn ends_with() {
        let q = parse("MATCH (n) WHERE n.name ENDS WITH 'son' RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::EndsWith,
                ..
            }
        ));
    }

    #[test]
    fn contains_predicate() {
        let q = parse("MATCH (n) WHERE n.name CONTAINS 'ali' RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::Contains,
                ..
            }
        ));
    }

    // ── OPTIONAL MATCH ────────────────────────────────────────────────────────

    #[test]
    fn optional_match() {
        let q = parse("MATCH (n) OPTIONAL MATCH (n)-[r]->(m) RETURN m");
        assert!(matches!(&q.clauses[1], Clause::OptionalMatch(_)));
    }

    // ── Multiple path steps ───────────────────────────────────────────────────

    #[test]
    fn multi_step_path() {
        let q = parse("MATCH (a)-[:R1]->(b)-[:R2]->(c) RETURN c");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.patterns[0].steps.len(), 2);
    }

    // ── List comprehension parsing ─────────────────────────────────────────────

    #[test]
    fn list_comprehension_identity() {
        // [x IN [1,2,3]] — no filter, no explicit map (variable itself)
        let q = parse("RETURN [x IN [1, 2, 3]]");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        match &rc.items[0].expr {
            Expr::ListComp {
                variable, filter, ..
            } => {
                assert_eq!(variable.0, "x");
                assert!(filter.is_none());
            }
            other => panic!("expected ListComp, got {other:?}"),
        }
    }

    #[test]
    fn list_comprehension_with_filter_and_map() {
        // [x IN range(1,10) WHERE x > 5 | x * 2]
        let q = parse("RETURN [x IN range(1, 10) WHERE x > 5 | x * 2]");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        match &rc.items[0].expr {
            Expr::ListComp {
                variable,
                filter,
                map,
                ..
            } => {
                assert_eq!(variable.0, "x");
                assert!(filter.is_some(), "expected a filter");
                assert!(
                    matches!(**map, Expr::BinaryOp { .. }),
                    "expected BinaryOp map expr"
                );
            }
            other => panic!("expected ListComp, got {other:?}"),
        }
    }
}
