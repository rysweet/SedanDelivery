//! Function-call parsing: generic `name(args)` plus the special quantifier
//! pattern `func(x IN list WHERE pred)` and the `reduce`/`extract`/`filter`
//! desugaring forms.

use smol_str::SmolStr;

use crate::ast::{BinaryOp, Expr, Identifier};
use crate::error::ParseError;
use crate::parser::lexer::TokenKind;
use crate::parser::parser::Parser;

impl<'src> Parser<'src> {
    pub(super) fn parse_func_call(&mut self, name_text: String) -> Result<Expr, ParseError> {
        // Handle reduce(accumulator = initial, x IN list | expression)
        if name_text.eq_ignore_ascii_case("reduce") {
            return self.parse_reduce_call();
        }
        // Handle extract(x IN list | expression) — desugar to ListComp
        if name_text.eq_ignore_ascii_case("extract") {
            return self.parse_extract_call();
        }
        // Handle filter(x IN list WHERE predicate) — desugar to ListComp
        if name_text.eq_ignore_ascii_case("filter") {
            return self.parse_filter_call();
        }

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

    /// Parse `reduce(accumulator = initial, variable IN list | expression)`.
    /// Current token is `(`.
    fn parse_reduce_call(&mut self) -> Result<Expr, ParseError> {
        self.advance(); // consume (
        let acc_name = self.expect_identifier()?;
        self.expect(&TokenKind::Eq)?;
        let initial = self.parse_expr()?;
        self.expect(&TokenKind::Comma)?;
        let var_name = self.expect_identifier()?;
        self.expect(&TokenKind::In)?;
        let list = self.parse_expr()?;
        self.expect(&TokenKind::Pipe)?;
        let expression = self.parse_expr()?;
        self.expect(&TokenKind::RParen)?;
        Ok(Expr::Reduce {
            accumulator: Identifier::new(acc_name),
            initial: Box::new(initial),
            variable: Identifier::new(var_name),
            list: Box::new(list),
            expression: Box::new(expression),
        })
    }

    /// Parse `extract(x IN list | expression)` — desugar to ListComp.
    /// Current token is `(`.
    fn parse_extract_call(&mut self) -> Result<Expr, ParseError> {
        self.advance(); // consume (
        let var_name = self.expect_identifier()?;
        self.expect(&TokenKind::In)?;
        let list = self.parse_expr()?;
        self.expect(&TokenKind::Pipe)?;
        let map_expr = self.parse_expr()?;
        self.expect(&TokenKind::RParen)?;
        Ok(Expr::ListComp {
            variable: Identifier::new(var_name),
            list: Box::new(list),
            filter: None,
            map: Box::new(map_expr),
        })
    }

    /// Parse `filter(x IN list WHERE predicate)` — desugar to ListComp.
    /// Current token is `(`.
    fn parse_filter_call(&mut self) -> Result<Expr, ParseError> {
        self.advance(); // consume (
        let var_name = self.expect_identifier()?;
        self.expect(&TokenKind::In)?;
        let list = self.parse_expr()?;
        self.expect(&TokenKind::Where)?;
        let predicate = self.parse_expr()?;
        self.expect(&TokenKind::RParen)?;
        let variable = Identifier::new(var_name);
        Ok(Expr::ListComp {
            variable: variable.clone(),
            list: Box::new(list),
            filter: Some(Box::new(predicate)),
            map: Box::new(Expr::Variable(variable)),
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
}
