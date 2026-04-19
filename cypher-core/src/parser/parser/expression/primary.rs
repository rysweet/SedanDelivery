//! Primary expressions: literals (integer, float, string, bool, null),
//! parameters, parenthesised expressions, identifiers/function-call dispatch,
//! `count(*)`, and entry points to CASE / list / map / pattern parsers.

use smol_str::SmolStr;

use crate::ast::{Expr, Identifier};
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;
use crate::parser::parser::helpers::is_keyword_usable_as_ident;
use crate::parser::parser::Parser;

impl<'src> Parser<'src> {
    pub(super) fn parse_primary(&mut self) -> Result<Expr, ParseError> {
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
}
