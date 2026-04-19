//! Control-flow and side-effecting clauses: UNWIND, CALL,
//! CALL { subquery }, FOREACH, and LOAD CSV.

use crate::ast::{CallClause, Clause, ForeachClause, Identifier, LoadCsvClause, UnwindClause};
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;

use super::super::Parser;

impl<'src> Parser<'src> {
    // ── LOAD CSV ──────────────────────────────────────────────────────────────

    pub(super) fn parse_load_csv(&mut self) -> Result<LoadCsvClause, ParseError> {
        self.expect(&TokenKind::Csv)?;
        let with_headers = if self.eat(&TokenKind::With) {
            self.expect(&TokenKind::Headers)?;
            true
        } else {
            false
        };
        // FROM is a contextual keyword (not reserved) to avoid breaking $from params.
        if *self.peek() == TokenKind::Identifier
            && self.tokens[self.pos].text.eq_ignore_ascii_case("from")
        {
            self.advance();
        } else {
            return Err(self.syntax_err("FROM"));
        }
        let source = self.parse_expr()?;
        self.expect(&TokenKind::As)?;
        let alias = Identifier::new(self.expect_identifier()?);
        Ok(LoadCsvClause {
            with_headers,
            source,
            alias,
        })
    }

    // ── UNWIND ────────────────────────────────────────────────────────────────

    pub(super) fn parse_unwind(&mut self) -> Result<UnwindClause, ParseError> {
        let expr = self.parse_expr()?;
        self.expect(&TokenKind::As)?;
        let alias = Identifier::new(self.expect_identifier()?);
        Ok(UnwindClause { expr, alias })
    }

    // ── CALL ──────────────────────────────────────────────────────────────────

    pub(super) fn parse_call(&mut self) -> Result<CallClause, ParseError> {
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

    // ── CALL { subquery } ─────────────────────────────────────────────────────

    pub(super) fn parse_call_subquery(&mut self) -> Result<Vec<Clause>, ParseError> {
        self.expect(&TokenKind::LBrace)?;
        let mut clauses = Vec::new();
        loop {
            if self.at(&TokenKind::RBrace) {
                break;
            }
            match self.parse_clause()? {
                Some(clause) => clauses.push(clause),
                None => break,
            }
        }
        self.expect(&TokenKind::RBrace)?;
        if clauses.is_empty() {
            return Err(self.syntax_err("at least one clause inside CALL { }"));
        }
        Ok(clauses)
    }

    // ── FOREACH ───────────────────────────────────────────────────────────────

    pub(super) fn parse_foreach(&mut self) -> Result<ForeachClause, ParseError> {
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
}
