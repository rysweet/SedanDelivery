//! Postfix operators: property access (including dotted/namespaced function
//! calls), subscript `[i]`, and slice `[start..end]`.

use smol_str::SmolStr;

use crate::ast::Expr;
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;
use crate::parser::parser::Parser;

impl<'src> Parser<'src> {
    pub(super) fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek().clone() {
                // Property access: expr.key — but peek ahead for namespaced function call
                TokenKind::Dot => {
                    self.advance();
                    let key = SmolStr::new(self.expect_map_key()?);
                    // Check if this is a namespaced function call: `ns.func(args)`
                    if self.at(&TokenKind::LParen) {
                        // Build the dotted function name from the expression chain.
                        let func_name = match &expr {
                            Expr::Variable(id) => format!("{}.{}", id.0, key),
                            Expr::Property(inner, prev_key) => {
                                // Handle deeper nesting like a.b.c()
                                fn build_name(e: &Expr) -> Option<String> {
                                    match e {
                                        Expr::Variable(id) => Some(id.0.clone()),
                                        Expr::Property(inner, k) => {
                                            build_name(inner).map(|n| format!("{}.{}", n, k))
                                        }
                                        _ => None,
                                    }
                                }
                                match build_name(inner) {
                                    Some(base) => format!("{}.{}.{}", base, prev_key, key),
                                    None => format!("{}", key), // fallback
                                }
                            }
                            _ => key.to_string(),
                        };
                        self.advance(); // consume (
                        let distinct = self.eat(&TokenKind::Distinct);
                        let mut args = Vec::new();
                        if !self.at(&TokenKind::RParen) {
                            let first = self.parse_expr()?;
                            let first = self.maybe_rewrite_quantifier_arg(first)?;
                            args.push(first);
                            while self.eat(&TokenKind::Comma) {
                                args.push(self.parse_expr()?);
                            }
                        }
                        self.expect(&TokenKind::RParen)?;
                        expr = Expr::FunctionCall {
                            name: SmolStr::new(func_name),
                            args,
                            distinct,
                        };
                    } else {
                        expr = Expr::Property(Box::new(expr), key);
                    }
                }
                // Subscript or slice: expr[index] or expr[start..end]
                TokenKind::LBracket => {
                    self.advance();
                    if self.at(&TokenKind::DoubleDot) {
                        // [..end]
                        self.advance();
                        let end = if self.at(&TokenKind::RBracket) {
                            None
                        } else {
                            Some(Box::new(self.parse_expr()?))
                        };
                        self.expect(&TokenKind::RBracket)?;
                        expr = Expr::Slice(Box::new(expr), None, end);
                    } else {
                        let idx = self.parse_expr()?;
                        if self.eat(&TokenKind::DoubleDot) {
                            // [start..end]
                            let end = if self.at(&TokenKind::RBracket) {
                                None
                            } else {
                                Some(Box::new(self.parse_expr()?))
                            };
                            self.expect(&TokenKind::RBracket)?;
                            expr = Expr::Slice(Box::new(expr), Some(Box::new(idx)), end);
                        } else {
                            self.expect(&TokenKind::RBracket)?;
                            expr = Expr::Subscript(Box::new(expr), Box::new(idx));
                        }
                    }
                }
                _ => break,
            }
        }
        Ok(expr)
    }
}
