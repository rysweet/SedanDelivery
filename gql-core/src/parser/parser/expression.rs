//! Expression parsing with operator precedence, unary/binary ops, function calls,
//! subscripts, slicing, list comprehensions, CASE expressions.

use smol_str::SmolStr;

use crate::ast::{BinaryOp, Expr, Identifier, Label, UnaryOp};
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;

use super::helpers::is_keyword_usable_as_ident;
use super::Parser;

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

    fn parse_unary(&mut self) -> Result<Expr, ParseError> {
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

    fn parse_postfix(&mut self) -> Result<Expr, ParseError> {
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

    fn parse_primary(&mut self) -> Result<Expr, ParseError> {
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

    // ── Function call ─────────────────────────────────────────────────────────

    /// Parse a function call: `name(args)`. The name has already been consumed;
    /// the current token is `(`.
    ///
    /// Also handles the quantifier pattern: `all(x IN list WHERE pred)`.
    fn parse_func_call(&mut self, name_text: String) -> Result<Expr, ParseError> {
        self.advance(); // consume (
        let distinct = self.eat(&TokenKind::Distinct);
        let mut args = Vec::new();
        if !self.at(&TokenKind::RParen) {
            if self.at(&TokenKind::Star) {
                self.advance();
                args.push(Expr::Variable(Identifier::new("*")));
            } else {
                let first = self.parse_expr()?;
                // Rewrite quantifier pattern: func(x IN list WHERE pred)
                let first = self.maybe_rewrite_quantifier_arg(first)?;
                args.push(first);
                while self.eat(&TokenKind::Comma) {
                    args.push(self.parse_expr()?);
                }
            }
        }
        self.expect(&TokenKind::RParen)?;
        Ok(Expr::FunctionCall {
            name: SmolStr::new(name_text),
            args,
            distinct,
        })
    }

    /// If the parsed argument is `x IN list` and the current token is WHERE,
    /// rewrite it as a ListComp quantifier argument:
    /// `all(x IN list WHERE pred)` → `FunctionCall("all", [ListComp(...)])`
    pub(super) fn maybe_rewrite_quantifier_arg(&mut self, arg: Expr) -> Result<Expr, ParseError> {
        if !self.at(&TokenKind::Where) {
            return Ok(arg);
        }
        match arg {
            Expr::BinaryOp {
                op: BinaryOp::In,
                left,
                right,
            } => {
                if let Expr::Variable(ref var) = *left {
                    self.advance(); // consume WHERE
                    let pred = self.parse_expr()?;
                    Ok(Expr::ListComp {
                        variable: var.clone(),
                        list: right,
                        filter: Some(Box::new(pred)),
                        map: Box::new(Expr::Variable(var.clone())),
                    })
                } else {
                    // Not a simple variable on the left of IN — don't rewrite.
                    Ok(Expr::BinaryOp {
                        op: BinaryOp::In,
                        left,
                        right,
                    })
                }
            }
            other => Ok(other),
        }
    }

    // ── CASE expression ───────────────────────────────────────────────────────

    /// Parse CASE after the `CASE` keyword has been consumed.
    fn parse_case(&mut self) -> Result<Expr, ParseError> {
        // Check whether there is a subject expression before WHEN.
        let subject = if !self.at(&TokenKind::When) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };

        let mut alternatives = Vec::new();
        while self.eat(&TokenKind::When) {
            let cond = self.parse_expr()?;
            self.expect(&TokenKind::Then)?;
            let result = self.parse_expr()?;
            alternatives.push((cond, result));
        }

        let default = if self.eat(&TokenKind::Else) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };

        self.expect(&TokenKind::End)?;

        Ok(Expr::Case {
            subject,
            alternatives,
            default,
        })
    }

    // ── Existential subquery ───────────────────────────────────────────────────

    /// Parse `exists { pattern [WHERE predicate] }` or `exists { MATCH pattern ... RETURN ... }`
    /// after `exists` has been consumed.
    fn parse_exists_subquery(&mut self) -> Result<Expr, ParseError> {
        self.expect(&TokenKind::LBrace)?;
        let mut patterns = Vec::new();
        // Parse MATCH patterns inside the subquery (without the MATCH keyword).
        // The first pattern starts with `(`, or there's an explicit MATCH keyword.
        if self.at(&TokenKind::Match) {
            // Full subquery with MATCH ... [WITH ... WHERE ... RETURN ...]
            self.advance();
            patterns.push(self.parse_path_pattern()?);
            while self.eat(&TokenKind::Comma) {
                patterns.push(self.parse_path_pattern()?);
            }
            let where_clause = if self.eat(&TokenKind::Where) {
                Some(Box::new(self.parse_expr()?))
            } else {
                None
            };
            // Check for write clauses; parse additional clauses into a full inner Query.
            let mut additional_clauses: Vec<crate::ast::Clause> = Vec::new();
            while !self.at(&TokenKind::RBrace) && !self.at(&TokenKind::Eof) {
                if matches!(
                    self.tokens[self.pos].kind,
                    TokenKind::Set
                        | TokenKind::Create
                        | TokenKind::Delete
                        | TokenKind::Merge
                        | TokenKind::Remove
                ) {
                    return Err(ParseError::Syntax {
                        offset: self.tokens[self.pos].offset,
                        expected: "write clause not allowed inside existential subquery"
                            .to_string(),
                        found: String::new(),
                    });
                }
                if let Some(clause) = self.parse_clause()? {
                    additional_clauses.push(clause);
                }
            }
            self.expect(&TokenKind::RBrace)?;
            let full_query = if additional_clauses.is_empty() {
                None
            } else {
                // Build a full inner Query: MATCH(patterns+where) + additional clauses
                let mc = crate::ast::MatchClause {
                    patterns: patterns.clone(),
                    where_clause: where_clause.as_deref().cloned(),
                };
                let mut inner_clauses = vec![crate::ast::Clause::Match(mc)];
                inner_clauses.extend(additional_clauses);
                Some(Box::new(crate::ast::Query {
                    clauses: inner_clauses,
                    unions: Vec::new(),
                }))
            };
            return Ok(Expr::ExistsSubquery {
                patterns,
                where_clause,
                full_query,
            });
        }
        // Simple pattern form: `exists { (n)-->() }`
        patterns.push(self.parse_path_pattern()?);
        while self.eat(&TokenKind::Comma) {
            patterns.push(self.parse_path_pattern()?);
        }
        let where_clause = if self.eat(&TokenKind::Where) {
            Some(Box::new(self.parse_expr()?))
        } else {
            None
        };
        self.expect(&TokenKind::RBrace)?;
        Ok(Expr::ExistsSubquery {
            patterns,
            where_clause,
            full_query: None,
        })
    }

    // ── Pattern expression ───────────────────────────────────────────────────

    /// Check if the current position looks like a pattern expression.
    /// Scans ahead from `(` to find the matching `)` and checks if it's
    /// followed by `-` or `<-` (relationship indicator).
    ///
    /// Also checks that the token immediately after `(` looks like a node pattern
    /// start (identifier, label colon, property map, or empty `()`) — not an
    /// arithmetic expression like `(4 ^ 3) - ...` or `(-3) - ...`.
    fn looks_like_pattern_expr(&self) -> bool {
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
    fn parse_pattern_expr(&mut self) -> Result<Expr, ParseError> {
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
                TokenKind::Pipe => {
                    if depth_paren == 0 && depth_bracket == 0 {
                        return true; // found `|` at the right level
                    }
                }
                TokenKind::In => {
                    // If we find `IN` at the top level, it's a regular list comprehension
                    if depth_paren == 0 && depth_bracket == 0 {
                        return false;
                    }
                }
                TokenKind::Eof => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    // ── List literal / list comprehension ─────────────────────────────────────

    /// Parse after `[` has been consumed.
    ///
    /// Forms:
    /// - `[expr, expr, ...]`          — list literal
    /// - `[x IN expr | mapExpr]`      — list comprehension
    /// - `[x IN expr WHERE pred | mapExpr]` — filtered list comprehension
    fn parse_list_or_comprehension(&mut self) -> Result<Expr, ParseError> {
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
