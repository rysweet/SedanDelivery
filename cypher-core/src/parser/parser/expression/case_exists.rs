//! `CASE ... WHEN ... THEN ... [ELSE ...] END` and `EXISTS { ... }`
//! existential subqueries.

use crate::ast::Expr;
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;
use crate::parser::parser::Parser;

impl<'src> Parser<'src> {
    /// Parse CASE after the `CASE` keyword has been consumed.
    pub(super) fn parse_case(&mut self) -> Result<Expr, ParseError> {
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

    /// Parse `exists { pattern [WHERE predicate] }` or
    /// `exists { MATCH pattern ... RETURN ... }` after `exists` has been consumed.
    pub(super) fn parse_exists_subquery(&mut self) -> Result<Expr, ParseError> {
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
            let mut additional_clauses: Vec<crate::ast::Clause> = Vec::new();
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
                let mc = crate::ast::MatchClause {
                    patterns: patterns.clone(),
                    where_clause: where_clause.as_deref().cloned(),
                };
                let mut inner_clauses = vec![crate::ast::Clause::Match(mc)];
                inner_clauses.extend(additional_clauses);
                Some(Box::new(crate::ast::Query {
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
}
