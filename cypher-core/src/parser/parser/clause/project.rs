//! Projection clauses: RETURN and WITH, plus the shared
//! ORDER BY / SKIP / LIMIT modifiers and projection-item helpers.

use crate::ast::{Expr, Identifier, ReturnClause, ReturnItem, SortItem, WithClause};
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;

use super::super::Parser;

impl<'src> Parser<'src> {
    // ── RETURN ────────────────────────────────────────────────────────────────

    pub(super) fn parse_return(&mut self) -> Result<ReturnClause, ParseError> {
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

    // ── WITH ──────────────────────────────────────────────────────────────────

    pub(super) fn parse_with(&mut self) -> Result<WithClause, ParseError> {
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
}
