//! Individual clause parsers (MATCH, WHERE, CREATE, DELETE, SET, RETURN,
//! WITH, UNWIND, CALL, MERGE, FOREACH, REMOVE, etc.)

use smol_str::SmolStr;

use crate::ast::{
    CallClause, Clause, CreateClause, DeleteClause, Expr, ForeachClause, Identifier, LoadCsvClause,
    MatchClause, MergeClause, RemoveClause, RemoveClauseItem, ReturnClause, ReturnItem, SetClause,
    SetClauseItem, SortItem, UnwindClause, WithClause,
};
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

    // ── LOAD CSV ──────────────────────────────────────────────────────────────

    fn parse_load_csv(&mut self) -> Result<LoadCsvClause, ParseError> {
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

    pub(super) fn parse_set_item(&mut self) -> Result<SetClauseItem, ParseError> {
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

    // ── CALL { subquery } ──────────────────────────────────────────────────────

    fn parse_call_subquery(&mut self) -> Result<Vec<Clause>, ParseError> {
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
}
