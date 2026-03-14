//! Path pattern parsing (MATCH patterns), node/edge patterns, properties, labels.

use smol_str::SmolStr;

use crate::ast::{
    Direction, Identifier, Label, MapLiteral, NodePattern, PathPattern, PathStep, Properties,
    RangeQuantifier, RelPattern, RelType, ShortestPathMode,
};
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;

use super::helpers::is_keyword_usable_as_ident;
use super::Parser;

impl<'src> Parser<'src> {
    // ── Path patterns ─────────────────────────────────────────────────────────

    /// Detect whether the current position starts a `shortestPath(` or `allShortestPaths(` call.
    /// Returns the mode and does NOT advance — the caller is responsible for consuming tokens.
    fn peek_shortest_path_function(&self) -> Option<ShortestPathMode> {
        if !matches!(self.peek(), TokenKind::Identifier) {
            return None;
        }
        let text = self.tokens[self.pos].text;
        if text.eq_ignore_ascii_case("shortestPath") {
            // Next token must be LParen
            if self.peek2() == &TokenKind::LParen {
                return Some(ShortestPathMode::Single);
            }
        } else if text.eq_ignore_ascii_case("allShortestPaths")
            && self.peek2() == &TokenKind::LParen
        {
            return Some(ShortestPathMode::All);
        }
        None
    }

    pub(super) fn parse_path_pattern(&mut self) -> Result<PathPattern, ParseError> {
        // Optional path variable assignment: `p = (a)...`
        // Look for `Identifier =` where the next-next token starts a path or shortest path call.
        let variable = if (matches!(self.peek(), TokenKind::Identifier)
            || is_keyword_usable_as_ident(self.peek()))
            && self.peek2() == &TokenKind::Eq
        {
            // After `=`, we accept either `(` (direct path) or an identifier
            // that is shortestPath/allShortestPaths followed by `(`.
            let peek3 = self.peek3();
            let is_path_start = matches!(peek3, TokenKind::LParen);
            let is_shortest_path_start = if matches!(peek3, TokenKind::Identifier) {
                // peek3 is the identifier, peek at pos+3 for LParen
                let idx4 = (self.pos + 3).min(self.tokens.len() - 1);
                matches!(self.tokens[idx4].kind, TokenKind::LParen) && {
                    let t = self.tokens[self.pos + 2].text;
                    t.eq_ignore_ascii_case("shortestPath")
                        || t.eq_ignore_ascii_case("allShortestPaths")
                }
            } else {
                false
            };
            if is_path_start || is_shortest_path_start {
                let name = self.expect_identifier()?;
                self.advance(); // consume `=`
                Some(Identifier::new(name))
            } else {
                None
            }
        } else {
            None
        };

        // Check for shortestPath(...) or allShortestPaths(...) wrapper.
        let shortest_path_mode = self.peek_shortest_path_function();
        if let Some(mode) = shortest_path_mode {
            self.advance(); // consume `shortestPath` or `allShortestPaths`
            self.expect(&TokenKind::LParen)?; // consume `(`

            // Parse the inner path pattern (which starts with a node pattern `(...)`)
            let start = self.parse_node_pattern()?;
            let mut steps = Vec::new();
            while matches!(self.peek(), TokenKind::Dash | TokenKind::LeftArrow) {
                let (rel, node) = self.parse_rel_pattern()?;
                steps.push(PathStep { rel, node });
            }

            self.expect(&TokenKind::RParen)?; // consume closing `)` of shortestPath(...)

            return Ok(PathPattern {
                variable,
                start,
                steps,
                shortest_path: Some(mode),
            });
        }

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
            shortest_path: None,
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
    pub(super) fn parse_label_list(&mut self) -> Result<Vec<Label>, ParseError> {
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

    pub(super) fn parse_map_literal(&mut self) -> Result<MapLiteral, ParseError> {
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
}
