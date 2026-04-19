//! Operator-precedence parsing: boolean (OR/XOR/AND/NOT), comparison and string
//! predicates (`=`, `<>`, chained comparisons, `IS [NOT] NULL`, `STARTS WITH`,
//! `ENDS WITH`, `CONTAINS`), `IN`, and arithmetic (`+`, `-`, `*`, `/`, `%`, `^`,
//! unary `+`/`-`).

use crate::ast::{BinaryOp, Expr, Label, UnaryOp};
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;
use crate::parser::parser::helpers::is_keyword_usable_as_ident;
use crate::parser::parser::Parser;

impl<'src> Parser<'src> {
    // ── Expression parsing (Pratt / recursive descent) ────────────────────────

    pub(crate) fn parse_expr(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(ParseError::RecursionLimitExceeded {
                limit: self.max_depth,
            });
        }
        let result = self.parse_or();
        self.depth -= 1;
        result
    }

    fn parse_or(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_xor()?;
        while self.at(&TokenKind::Or) {
            self.advance();
            let right = self.parse_xor()?;
            left = Expr::BinaryOp {
                op: BinaryOp::Or,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_xor(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_and()?;
        while self.at(&TokenKind::Xor) {
            self.advance();
            let right = self.parse_and()?;
            left = Expr::BinaryOp {
                op: BinaryOp::Xor,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_not()?;
        while self.at(&TokenKind::And) {
            self.advance();
            let right = self.parse_not()?;
            left = Expr::BinaryOp {
                op: BinaryOp::And,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(ParseError::RecursionLimitExceeded {
                limit: self.max_depth,
            });
        }
        let result = if self.eat(&TokenKind::Not) {
            let expr = self.parse_not()?;
            Ok(Expr::UnaryOp {
                op: UnaryOp::Not,
                expr: Box::new(expr),
            })
        } else {
            self.parse_comparison()
        };
        self.depth -= 1;
        result
    }

    fn parse_comparison(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_in_pred()?;

        // Check for chained IS NULL / IS NOT NULL (post-fix predicates).
        left = self.parse_null_check(left)?;

        // Check for label check: `a:Label` or `a:Label1:Label2`
        // `a:A:B` is sugar for `a:A AND a:B` — rewrite to conjunctive AND.
        if self.at(&TokenKind::Colon)
            && self.pos + 1 < self.tokens.len()
            && (matches!(self.tokens[self.pos + 1].kind, TokenKind::Identifier)
                || is_keyword_usable_as_ident(&self.tokens[self.pos + 1].kind))
        {
            let label_inner = left.clone(); // the expression being label-checked
            let mut label_result: Option<Expr> = None;
            while self.at(&TokenKind::Colon) {
                if self.pos + 1 < self.tokens.len()
                    && (matches!(self.tokens[self.pos + 1].kind, TokenKind::Identifier)
                        || is_keyword_usable_as_ident(&self.tokens[self.pos + 1].kind))
                {
                    self.advance(); // consume ':'
                    let label_name = self.tokens[self.pos].text.to_string();
                    self.advance(); // consume label name
                    let check = Expr::HasLabel(
                        Box::new(label_inner.clone()),
                        Label::new(smol_str::SmolStr::new(&label_name)),
                    );
                    label_result = Some(match label_result {
                        None => check,
                        Some(prev) => Expr::BinaryOp {
                            op: BinaryOp::And,
                            left: Box::new(prev),
                            right: Box::new(check),
                        },
                    });
                } else {
                    break;
                }
            }
            if let Some(r) = label_result {
                left = r;
            }
        }

        // Track the previous right-hand side and operator for chained comparisons.
        // `a < b < c` → `(a < b) AND (b < c)` per openCypher semantics.
        let mut prev_right: Option<Expr> = None;
        loop {
            let op = match self.peek().clone() {
                TokenKind::Eq => BinaryOp::Eq,
                TokenKind::Neq => BinaryOp::Neq,
                TokenKind::Lt => BinaryOp::Lt,
                TokenKind::Lte => BinaryOp::Lte,
                TokenKind::Gt => BinaryOp::Gt,
                TokenKind::Gte => BinaryOp::Gte,
                TokenKind::RegexEq => BinaryOp::RegexMatch,
                _ => break,
            };
            self.advance();
            let right = self.parse_in_pred()?;
            let right = self.parse_null_check(right)?;

            if let Some(prev) = prev_right.take() {
                // Chained comparison: left is already `(a < b)`, now we have `< c`.
                // Build `(a < b) AND (b < c)` where `b` is the previous right operand.
                let chain_cmp = Expr::BinaryOp {
                    op,
                    left: Box::new(prev.clone()),
                    right: Box::new(right.clone()),
                };
                left = Expr::BinaryOp {
                    op: BinaryOp::And,
                    left: Box::new(left),
                    right: Box::new(chain_cmp),
                };
            } else {
                left = Expr::BinaryOp {
                    op,
                    left: Box::new(left),
                    right: Box::new(right.clone()),
                };
            }
            prev_right = Some(right);
        }
        Ok(left)
    }

    /// Parse IN predicate — higher precedence than comparison operators.
    /// `expr IN list` binds tighter than `=`, `<>`, `<`, `>`, `<=`, `>=`.
    fn parse_in_pred(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_string_pred()?;
        while self.at(&TokenKind::In) {
            self.advance();
            let right = self.parse_string_pred()?;
            left = Expr::BinaryOp {
                op: BinaryOp::In,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// Handles postfix `IS NULL` / `IS NOT NULL`.
    fn parse_null_check(&mut self, expr: Expr) -> Result<Expr, ParseError> {
        if self.eat(&TokenKind::Is) {
            if self.eat(&TokenKind::Not) {
                self.expect(&TokenKind::Null)?;
                Ok(Expr::IsNotNull(Box::new(expr)))
            } else {
                self.expect(&TokenKind::Null)?;
                Ok(Expr::IsNull(Box::new(expr)))
            }
        } else {
            Ok(expr)
        }
    }

    /// Handles `STARTS WITH`, `ENDS WITH`, `CONTAINS` string predicates.
    fn parse_string_pred(&mut self) -> Result<Expr, ParseError> {
        let left = self.parse_add()?;
        match self.peek().clone() {
            TokenKind::Starts => {
                self.advance();
                // Expect "WITH" as an identifier (the lexer doesn't have a With keyword for this).
                let text = self.tokens[self.pos].text.to_ascii_lowercase();
                if text == "with" {
                    self.advance();
                } else {
                    return Err(self.syntax_err("WITH after STARTS"));
                }
                let right = self.parse_add()?;
                Ok(Expr::BinaryOp {
                    op: BinaryOp::StartsWith,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            TokenKind::Ends => {
                self.advance();
                let text = self.tokens[self.pos].text.to_ascii_lowercase();
                if text == "with" {
                    self.advance();
                } else {
                    return Err(self.syntax_err("WITH after ENDS"));
                }
                let right = self.parse_add()?;
                Ok(Expr::BinaryOp {
                    op: BinaryOp::EndsWith,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            TokenKind::Contains => {
                self.advance();
                let right = self.parse_add()?;
                Ok(Expr::BinaryOp {
                    op: BinaryOp::Contains,
                    left: Box::new(left),
                    right: Box::new(right),
                })
            }
            _ => Ok(left),
        }
    }

    fn parse_add(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_mul()?;
        loop {
            let op = match self.peek().clone() {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Dash => BinaryOp::Sub,
                _ => break,
            };
            self.advance();
            let right = self.parse_mul()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    fn parse_mul(&mut self) -> Result<Expr, ParseError> {
        let mut left = self.parse_pow()?;
        loop {
            let op = match self.peek().clone() {
                TokenKind::Star => BinaryOp::Mul,
                TokenKind::Slash => BinaryOp::Div,
                TokenKind::Percent => BinaryOp::Mod,
                _ => break,
            };
            self.advance();
            let right = self.parse_pow()?;
            left = Expr::BinaryOp {
                op,
                left: Box::new(left),
                right: Box::new(right),
            };
        }
        Ok(left)
    }

    /// Parse exponentiation — higher precedence than `*`, `/`, `%`.
    /// Left-associative per Cypher spec: `a ^ b ^ c` = `(a ^ b) ^ c`.
    fn parse_pow(&mut self) -> Result<Expr, ParseError> {
        let mut base = self.parse_unary()?;
        while self.eat(&TokenKind::Caret) {
            let exp = self.parse_unary()?; // left-associative: rhs is just unary
            base = Expr::BinaryOp {
                op: BinaryOp::Pow,
                left: Box::new(base),
                right: Box::new(exp),
            };
        }
        Ok(base)
    }

    pub(super) fn parse_unary(&mut self) -> Result<Expr, ParseError> {
        self.depth += 1;
        if self.depth > self.max_depth {
            self.depth -= 1;
            return Err(ParseError::RecursionLimitExceeded {
                limit: self.max_depth,
            });
        }
        let result = if self.eat(&TokenKind::Dash) {
            // Special case: -9223372036854775808 = i64::MIN (the only valid use of
            // the 2^63 magnitude literal is immediately after unary minus).
            if self.at(&TokenKind::IntMinMagnitude) {
                self.advance();
                Ok(Expr::Integer(i64::MIN))
            } else {
                let expr = self.parse_unary()?;
                Ok(Expr::UnaryOp {
                    op: UnaryOp::Neg,
                    expr: Box::new(expr),
                })
            }
        } else if self.eat(&TokenKind::Plus) {
            // Unary plus is a no-op; just return the inner expression.
            self.parse_unary()
        } else {
            self.parse_postfix()
        };
        self.depth -= 1;
        result
    }
}
