//! Individual clause parsers (MATCH, WHERE, CREATE, DELETE, SET, RETURN,
//! WITH, UNWIND, CALL, MERGE, FOREACH, REMOVE, etc.)
//!
//! The clause parsers live in focused submodules:
//!
//! - [`read`] — MATCH / OPTIONAL MATCH (read-only graph patterns).
//! - [`project`] — RETURN / WITH and their shared ORDER BY / SKIP / LIMIT
//!   modifiers (projection clauses).
//! - [`write`] — CREATE / DELETE / SET / REMOVE / MERGE (mutation clauses).
//! - [`control`] — UNWIND / CALL / CALL { subquery } / FOREACH / LOAD CSV
//!   (control-flow and side-effecting clauses).
//!
//! This module file owns only the clause dispatcher (`parse_clause`); each
//! sibling file adds its own `impl<'src> Parser<'src>` block with the
//! relevant `parse_*` helpers.

mod control;
mod project;
mod read;
mod write;

use crate::ast::Clause;
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;

use super::Parser;

impl<'src> Parser<'src> {
    // ── Clause dispatcher ─────────────────────────────────────────────────────

    pub(super) fn parse_clause(&mut self) -> Result<Option<Clause>, ParseError> {
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
                // Distinguish CALL { subquery } from CALL procedure.name(...)
                if self.at(&TokenKind::LBrace) {
                    let inner = self.parse_call_subquery()?;
                    Ok(Some(Clause::CallSubquery { inner }))
                } else {
                    let cc = self.parse_call()?;
                    Ok(Some(Clause::Call(cc)))
                }
            }
            TokenKind::Foreach => {
                self.advance();
                let fc = self.parse_foreach()?;
                Ok(Some(Clause::Foreach(fc)))
            }
            TokenKind::Load => {
                self.advance();
                let lc = self.parse_load_csv()?;
                Ok(Some(Clause::LoadCsv(lc)))
            }
            TokenKind::Eof | TokenKind::Semicolon => Ok(None),
            _ => Err(self.syntax_err("clause keyword (MATCH, RETURN, CREATE, ...)")),
        }
    }
}
