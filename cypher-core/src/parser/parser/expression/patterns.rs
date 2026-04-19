//! Pattern expressions used as predicates (`(n)-[:R]->(m)`), pattern
//! comprehensions inside brackets, and the list-literal / list-comprehension
//! parser.

use smol_str::SmolStr;

use crate::ast::{Expr, Identifier};
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;
use crate::parser::parser::helpers::is_keyword_usable_as_ident;
use crate::parser::parser::Parser;

impl<'src> Parser<'src> {
    /// Check if the current position looks like a pattern expression.
    /// Scans ahead from `(` to find the matching `)` and checks if it's
    /// followed by `-` or `<-` (relationship indicator).
    ///
    /// Also checks that the token immediately after `(` looks like a node pattern
    /// start (identifier, label colon, property map, or empty `()`) — not an
    /// arithmetic expression like `(4 ^ 3) - ...` or `(-3) - ...`.
    pub(super) fn looks_like_pattern_expr(&self) -> bool {
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
    pub(super) fn parse_pattern_expr(&mut self) -> Result<Expr, ParseError> {
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
                TokenKind::Pipe
                    if depth_paren == 0 && depth_bracket == 0 => {
                        return true; // found `|` at the right level
                    }
                TokenKind::In
                    // If we find `IN` at the top level, it's a regular list comprehension
                    if depth_paren == 0 && depth_bracket == 0 => {
                        return false;
                    }
                TokenKind::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    /// Parse after `[` has been consumed.
    ///
    /// Forms:
    /// - `[expr, expr, ...]`          — list literal
    /// - `[x IN expr | mapExpr]`      — list comprehension
    /// - `[x IN expr WHERE pred | mapExpr]` — filtered list comprehension
    pub(super) fn parse_list_or_comprehension(&mut self) -> Result<Expr, ParseError> {
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
