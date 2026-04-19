//! Read clauses: MATCH and OPTIONAL MATCH.

use crate::ast::MatchClause;
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;

use super::super::Parser;

impl<'src> Parser<'src> {
    pub(super) fn parse_match(&mut self, _optional: bool) -> Result<MatchClause, ParseError> {
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
}
