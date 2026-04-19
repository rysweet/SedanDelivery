//! Mutation clauses: CREATE, DELETE, SET, REMOVE, MERGE.

use smol_str::SmolStr;

use crate::ast::{
    CreateClause, DeleteClause, Identifier, MergeClause, RemoveClause, RemoveClauseItem, SetClause,
    SetClauseItem,
};
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;

use super::super::Parser;

impl<'src> Parser<'src> {
    // ── CREATE ────────────────────────────────────────────────────────────────

    pub(super) fn parse_create(&mut self) -> Result<CreateClause, ParseError> {
        let mut patterns = Vec::new();
        patterns.push(self.parse_path_pattern()?);
        while self.eat(&TokenKind::Comma) {
            patterns.push(self.parse_path_pattern()?);
        }
        Ok(CreateClause { patterns })
    }

    // ── DELETE ────────────────────────────────────────────────────────────────

    pub(super) fn parse_delete(&mut self, detach: bool) -> Result<DeleteClause, ParseError> {
        let mut exprs = Vec::new();
        exprs.push(self.parse_expr()?);
        while self.eat(&TokenKind::Comma) {
            exprs.push(self.parse_expr()?);
        }
        Ok(DeleteClause { exprs, detach })
    }

    // ── SET ───────────────────────────────────────────────────────────────────

    pub(super) fn parse_set(&mut self) -> Result<SetClause, ParseError> {
        let mut items = Vec::new();
        items.push(self.parse_set_item()?);
        while self.eat(&TokenKind::Comma) {
            items.push(self.parse_set_item()?);
        }
        Ok(SetClause { items })
    }

    pub(in crate::parser::parser) fn parse_set_item(
        &mut self,
    ) -> Result<SetClauseItem, ParseError> {
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

    pub(super) fn parse_remove(&mut self) -> Result<RemoveClause, ParseError> {
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

    pub(super) fn parse_merge(&mut self) -> Result<MergeClause, ParseError> {
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
}
