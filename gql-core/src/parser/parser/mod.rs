//! Recursive descent parser for openCypher / GQL.
//!
//! Entry point: [`parse_query`].
//!
//! Design notes:
//! - The `|` character is handled contextually:
//!   - Inside a label or rel-type list it is label/type disjunction: `(n:A|B)`.
//!   - Inside a list comprehension `[x IN list | expr]` it separates the
//!     filter variable from the projection expression.
//! - All error paths carry a byte offset from the original source.

mod clause;
mod expression;
mod helpers;
mod pattern;

use crate::ast::{Clause, Query};
use crate::error::ParseError;
use crate::parser::lexer::{Token, TokenKind};

// ── Parser struct ─────────────────────────────────────────────────────────────

/// Cursor-based recursive descent parser.
///
/// Tokens are pre-collected from the lexer before parsing begins, so every
/// lookahead is an O(1) slice index and there is no buffering complexity.
pub struct Parser<'src> {
    tokens: Vec<Token<'src>>,
    pos: usize,
    /// Current expression nesting depth; guards against deeply nested DoS inputs.
    depth: u32,
    /// Maximum allowed expression nesting depth (injected from deployment config).
    max_depth: u32,
}

impl<'src> Parser<'src> {
    /// Create a parser from a pre-lexed token stream.
    ///
    /// The token stream must end with a [`TokenKind::Eof`] sentinel.
    pub fn new(tokens: Vec<Token<'src>>) -> Self {
        let max_depth = crate::config::global().parser_max_depth;
        Self {
            tokens,
            pos: 0,
            depth: 0,
            max_depth,
        }
    }
}

// ── Top-level entry point ─────────────────────────────────────────────────────

/// Parse `src` into a [`Query`] AST.
///
/// Returns [`ParseError::EmptyQuery`] for blank/empty input.
/// Returns [`ParseError::Syntax`] for any structural grammar error.
pub fn parse_query(src: &str) -> Result<Query, ParseError> {
    parse_query_with_max_size(src, crate::config::global().max_query_size)
}

/// Like [`parse_query`] but with an explicit byte-length limit.
///
/// Useful for testing the size guard without relying on the global config.
pub fn parse_query_with_max_size(src: &str, max_query_size: usize) -> Result<Query, ParseError> {
    if src.len() > max_query_size {
        return Err(ParseError::QueryTooLarge {
            size: src.len(),
            limit: max_query_size,
        });
    }
    if src.trim().is_empty() {
        return Err(ParseError::EmptyQuery);
    }
    let tokens = crate::parser::lexer::lex_query(src)?;
    let mut p = Parser::new(tokens);
    p.parse_query_inner()
}

impl<'src> Parser<'src> {
    fn parse_query_inner(&mut self) -> Result<Query, ParseError> {
        let clauses = self.parse_clause_sequence()?;

        if clauses.is_empty() {
            return Err(ParseError::EmptyQuery);
        }

        // Parse optional UNION / UNION ALL parts.
        let mut unions = Vec::new();
        while self.at(&TokenKind::Union) {
            self.advance(); // consume UNION
            let all = self.eat(&TokenKind::All);
            let part = self.parse_clause_sequence()?;
            if part.is_empty() {
                return Err(self.syntax_err("clause after UNION"));
            }
            unions.push((all, part));
        }

        Ok(Query { clauses, unions })
    }

    /// Parse a sequence of clauses (separated by optional semicolons),
    /// stopping at UNION, EOF, or when no more clauses can be parsed.
    fn parse_clause_sequence(&mut self) -> Result<Vec<Clause>, ParseError> {
        let mut clauses = Vec::new();
        // Skip leading semicolons.
        while self.eat(&TokenKind::Semicolon) {}

        loop {
            if self.at(&TokenKind::Eof) || self.at(&TokenKind::Union) {
                break;
            }
            match self.parse_clause()? {
                Some(clause) => clauses.push(clause),
                None => break,
            }
            // Optional clause separator.
            while self.eat(&TokenKind::Semicolon) {}
        }

        Ok(clauses)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::*;

    fn parse(src: &str) -> Query {
        parse_query(src).unwrap_or_else(|e| panic!("parse error for {:?}: {}", src, e))
    }

    fn parse_err(src: &str) -> ParseError {
        parse_query(src).expect_err("expected a parse error")
    }

    // ── Basic MATCH/RETURN ────────────────────────────────────────────────────

    #[test]
    fn simple_match_return() {
        let q = parse("MATCH (n) RETURN n");
        assert_eq!(q.clauses.len(), 2);
        assert!(matches!(&q.clauses[0], Clause::Match(_)));
        assert!(matches!(&q.clauses[1], Clause::Return(_)));
    }

    #[test]
    fn match_with_label() {
        let q = parse("MATCH (n:Person) RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let node = &mc.patterns[0].start;
        assert_eq!(node.labels, vec![Label::new("Person")]);
    }

    #[test]
    fn match_return_property() {
        let q = parse("MATCH (n:Person) RETURN n.name");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::Property(_, key) if key == "name"));
    }

    #[test]
    fn return_with_alias() {
        let q = parse("MATCH (n) RETURN n.name AS personName");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert_eq!(rc.items[0].alias, Some(Identifier::new("personName")));
    }

    #[test]
    fn return_distinct() {
        let q = parse("MATCH (n:Person) RETURN DISTINCT n.name");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(rc.distinct);
    }

    // ── WITH chaining ─────────────────────────────────────────────────────────

    #[test]
    fn with_chain() {
        let q = parse("MATCH (n) WITH n MATCH (n)-[r]->(m) RETURN m");
        assert_eq!(q.clauses.len(), 4);
        assert!(matches!(&q.clauses[1], Clause::With(_)));
    }

    #[test]
    fn with_where() {
        let q = parse("MATCH (n) WITH n WHERE n.age > 18 RETURN n");
        let Clause::With(wc) = &q.clauses[1] else {
            panic!()
        };
        assert!(wc.where_clause.is_some());
    }

    // ── Label disjunction ─────────────────────────────────────────────────────

    #[test]
    fn label_disjunction_pipe() {
        let q = parse("MATCH (n:Person|Animal) RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let node = &mc.patterns[0].start;
        assert_eq!(node.labels.len(), 2);
        assert_eq!(node.labels[0], Label::new("Person"));
        assert_eq!(node.labels[1], Label::new("Animal"));
    }

    #[test]
    fn label_colon_list() {
        let q = parse("MATCH (n:Person:Employee) RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.patterns[0].start.labels.len(), 2);
    }

    // ── Variable-length paths ─────────────────────────────────────────────────

    #[test]
    fn variable_length_range() {
        let q = parse("MATCH (a)-[r:KNOWS*1..5]->(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let step = &mc.patterns[0].steps[0];
        let range = step.rel.range.as_ref().unwrap();
        assert_eq!(range.min, Some(1));
        assert_eq!(range.max, Some(5));
    }

    #[test]
    fn variable_length_star_only() {
        let q = parse("MATCH (a)-[*]->(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let step = &mc.patterns[0].steps[0];
        let range = step.rel.range.as_ref().unwrap();
        assert_eq!(range.min, None);
        assert_eq!(range.max, None);
    }

    #[test]
    fn variable_length_exact() {
        let q = parse("MATCH (a)-[*3]->(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let range = mc.patterns[0].steps[0].rel.range.as_ref().unwrap();
        // *3 with no DoubleDot means exactly 3.
        assert_eq!(range.min, Some(3));
    }

    // ── Property maps ─────────────────────────────────────────────────────────

    #[test]
    fn node_with_property_map() {
        let q = parse("MATCH (n:Person {name: 'Alice', age: 30}) RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let Properties::Map(props) = mc.patterns[0].start.properties.as_ref().unwrap() else {
            panic!("expected Properties::Map");
        };
        assert_eq!(props.len(), 2);
        assert_eq!(props[0].0, "name");
        assert!(matches!(&props[0].1, Expr::Str(s) if s.as_ref() == "Alice"));
        assert_eq!(props[1].0, "age");
        assert!(matches!(&props[1].1, Expr::Integer(30)));
    }

    // ── WHERE with complex expressions ────────────────────────────────────────

    #[test]
    fn where_and_expression() {
        let q = parse("MATCH (n) WHERE n.age >= 18 AND n.age < 65 RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(mc.where_clause.is_some());
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::And,
                ..
            }
        ));
    }

    #[test]
    fn where_or_expression() {
        let q = parse("MATCH (n) WHERE n.age < 18 OR n.age > 65 RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::Or,
                ..
            }
        ));
    }

    #[test]
    fn where_not_expression() {
        let q = parse("MATCH (n) WHERE NOT n.active RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::UnaryOp {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    // ── CREATE ────────────────────────────────────────────────────────────────

    #[test]
    fn create_single_node() {
        let q = parse("CREATE (n:Person {name: 'Bob'})");
        assert_eq!(q.clauses.len(), 1);
        let Clause::Create(cc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(cc.patterns.len(), 1);
    }

    #[test]
    fn create_relationship() {
        let q = parse("CREATE (a:Person)-[:KNOWS]->(b:Person)");
        let Clause::Create(cc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(cc.patterns[0].steps.len(), 1);
        assert_eq!(cc.patterns[0].steps[0].rel.direction, Direction::Outgoing);
        assert_eq!(
            cc.patterns[0].steps[0].rel.rel_types,
            vec![RelType::new("KNOWS")]
        );
    }

    #[test]
    fn create_multiple_patterns() {
        let q = parse("CREATE (a:A), (b:B), (a)-[:R]->(b)");
        let Clause::Create(cc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(cc.patterns.len(), 3);
    }

    // ── MERGE ON CREATE SET / ON MATCH SET ────────────────────────────────────

    #[test]
    fn merge_on_create_set() {
        let q = parse("MERGE (n:Person {name: 'Alice'}) ON CREATE SET n.created = true");
        let Clause::Merge(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.on_create.len(), 1);
        assert!(mc.on_match.is_empty());
    }

    #[test]
    fn merge_on_match_set() {
        let q = parse("MERGE (n:Person {name: 'Bob'}) ON MATCH SET n.updated = true");
        let Clause::Merge(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(mc.on_create.is_empty());
        assert_eq!(mc.on_match.len(), 1);
    }

    #[test]
    fn merge_both_actions() {
        let q = parse("MERGE (n:Person {name: 'Carol'}) ON CREATE SET n.created = true ON MATCH SET n.updated = true");
        let Clause::Merge(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.on_create.len(), 1);
        assert_eq!(mc.on_match.len(), 1);
    }

    // ── ORDER BY + LIMIT + SKIP ───────────────────────────────────────────────

    #[test]
    fn return_order_by_limit_skip() {
        let q = parse("MATCH (n) RETURN n ORDER BY n.name SKIP 10 LIMIT 5");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert_eq!(rc.order_by.len(), 1);
        assert_eq!(rc.skip, Some(Expr::Integer(10)));
        assert_eq!(rc.limit, Some(Expr::Integer(5)));
    }

    #[test]
    fn order_by_desc() {
        let q = parse("MATCH (n) RETURN n ORDER BY n.age DESC");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(!rc.order_by[0].ascending);
    }

    #[test]
    fn order_by_multiple() {
        let q = parse("MATCH (n) RETURN n ORDER BY n.age ASC, n.name DESC");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert_eq!(rc.order_by.len(), 2);
        assert!(rc.order_by[0].ascending);
        assert!(!rc.order_by[1].ascending);
    }

    // ── All comparison operators ──────────────────────────────────────────────

    #[test]
    fn comparison_eq() {
        let q = parse("MATCH (n) WHERE n.x = 1 RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::Eq,
                ..
            }
        ));
    }

    #[test]
    fn comparison_neq() {
        let q = parse("MATCH (n) WHERE n.x <> 1 RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::Neq,
                ..
            }
        ));
    }

    #[test]
    fn comparison_lt_lte_gt_gte() {
        for (op_str, expected_op) in [
            ("< 5", BinaryOp::Lt),
            ("<= 5", BinaryOp::Lte),
            ("> 5", BinaryOp::Gt),
            (">= 5", BinaryOp::Gte),
        ] {
            let q = parse(&format!("MATCH (n) WHERE n.x {} RETURN n", op_str));
            let Clause::Match(mc) = &q.clauses[0] else {
                panic!()
            };
            match mc.where_clause.as_ref().unwrap() {
                Expr::BinaryOp { op, .. } => assert_eq!(*op, expected_op),
                _ => panic!("expected BinaryOp"),
            }
        }
    }

    #[test]
    fn comparison_in() {
        let q = parse("MATCH (n) WHERE n.x IN [1, 2, 3] RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::In,
                ..
            }
        ));
    }

    // ── Error cases ───────────────────────────────────────────────────────────

    #[test]
    fn error_unterminated_node_pattern() {
        let err = parse_err("MATCH (n");
        assert!(matches!(err, ParseError::Syntax { .. }));
    }

    #[test]
    fn error_empty_query() {
        let err = parse_err("");
        assert!(matches!(err, ParseError::EmptyQuery));
    }

    #[test]
    fn error_whitespace_only() {
        let err = parse_err("   ");
        assert!(matches!(err, ParseError::EmptyQuery));
    }

    #[test]
    fn error_no_clause() {
        let err = parse_err("42");
        assert!(matches!(err, ParseError::Syntax { .. }));
    }

    // ── Parameters ────────────────────────────────────────────────────────────

    #[test]
    fn parameter_in_where() {
        let q = parse("MATCH (n) WHERE n.name = $name RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        match mc.where_clause.as_ref().unwrap() {
            Expr::BinaryOp { right, .. } => {
                assert!(matches!(right.as_ref(), Expr::Parameter(p) if p == "name"));
            }
            _ => panic!("expected BinaryOp"),
        }
    }

    #[test]
    fn parameter_in_create() {
        let q = parse("CREATE (n:Person {name: $name})");
        let Clause::Create(cc) = &q.clauses[0] else {
            panic!()
        };
        let Properties::Map(props) = cc.patterns[0].start.properties.as_ref().unwrap() else {
            panic!("expected Properties::Map");
        };
        assert!(matches!(&props[0].1, Expr::Parameter(p) if p == "name"));
    }

    #[test]
    fn parameter_map_in_create() {
        let q = parse("CREATE (n:Person $create_params)");
        let Clause::Create(cc) = &q.clauses[0] else {
            panic!()
        };
        let props = cc.patterns[0].start.properties.as_ref().unwrap();
        assert!(
            matches!(props, Properties::Parameter(name) if name == "create_params"),
            "expected Properties::Parameter('create_params'), got {:?}",
            props
        );
    }

    #[test]
    fn parameter_map_in_match() {
        let q = parse("MATCH (n:Person $match_params) RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let props = mc.patterns[0].start.properties.as_ref().unwrap();
        assert!(
            matches!(props, Properties::Parameter(name) if name == "match_params"),
            "expected Properties::Parameter('match_params'), got {:?}",
            props
        );
    }

    #[test]
    fn parameter_map_in_merge() {
        let q = parse("MERGE (n:Person $merge_params) RETURN n");
        let Clause::Merge(mc) = &q.clauses[0] else {
            panic!()
        };
        let props = mc.pattern.start.properties.as_ref().unwrap();
        assert!(
            matches!(props, Properties::Parameter(name) if name == "merge_params"),
            "expected Properties::Parameter('merge_params'), got {:?}",
            props
        );
    }

    // ── CASE expression ───────────────────────────────────────────────────────

    #[test]
    fn case_simple() {
        let q = parse("RETURN CASE 1 WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(
            matches!(&rc.items[0].expr, Expr::Case { subject: Some(_), alternatives, default: Some(_) } if alternatives.len() == 2)
        );
    }

    #[test]
    fn case_generic() {
        let q = parse("RETURN CASE WHEN 1 = 1 THEN true ELSE false END");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(
            matches!(&rc.items[0].expr, Expr::Case { subject: None, alternatives, .. } if alternatives.len() == 1)
        );
    }

    // ── Function calls ────────────────────────────────────────────────────────

    #[test]
    fn count_star() {
        let q = parse("MATCH (n) RETURN count(*)");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::FunctionCall { name, .. } if name == "count"));
    }

    #[test]
    fn count_distinct() {
        let q = parse("MATCH (n) RETURN count(DISTINCT n)");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        match &rc.items[0].expr {
            Expr::FunctionCall { name, distinct, .. } => {
                assert_eq!(name, "count");
                assert!(distinct);
            }
            _ => panic!("expected FunctionCall"),
        }
    }

    #[test]
    fn function_call_size() {
        let q = parse("MATCH (n) RETURN size(n.name)");
        let Clause::Return(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::FunctionCall { name, .. } if name == "size"));
    }

    #[test]
    fn function_call_no_args() {
        let q = parse("RETURN timestamp()");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(
            matches!(&rc.items[0].expr, Expr::FunctionCall { name, args, .. } if name == "timestamp" && args.is_empty())
        );
    }

    // ── IS NULL / IS NOT NULL ─────────────────────────────────────────────────

    #[test]
    fn is_null() {
        let q = parse("MATCH (n) WHERE n.name IS NULL RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(mc.where_clause.as_ref().unwrap(), Expr::IsNull(_)));
    }

    #[test]
    fn is_not_null() {
        let q = parse("MATCH (n) WHERE n.name IS NOT NULL RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::IsNotNull(_)
        ));
    }

    // ── SET and REMOVE ────────────────────────────────────────────────────────

    #[test]
    fn set_property() {
        let q = parse("MATCH (n) SET n.name = 'Alice'");
        let Clause::Set(sc) = &q.clauses[1] else {
            panic!()
        };
        assert!(matches!(&sc.items[0], SetClauseItem::SetProperty { prop, .. } if prop == "name"));
    }

    #[test]
    fn set_label() {
        let q = parse("MATCH (n) SET n:Person");
        let Clause::Set(sc) = &q.clauses[1] else {
            panic!()
        };
        assert!(
            matches!(&sc.items[0], SetClauseItem::SetLabels { labels, .. } if labels[0] == Label::new("Person"))
        );
    }

    #[test]
    fn remove_property() {
        let q = parse("MATCH (n) REMOVE n.name");
        let Clause::Remove(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(
            matches!(&rc.items[0], RemoveClauseItem::RemoveProperty { prop, .. } if prop == "name")
        );
    }

    #[test]
    fn remove_label() {
        let q = parse("MATCH (n:Person) REMOVE n:Person");
        let Clause::Remove(rc) = &q.clauses[1] else {
            panic!()
        };
        assert!(
            matches!(&rc.items[0], RemoveClauseItem::RemoveLabels { labels, .. } if !labels.is_empty())
        );
    }

    // ── UNWIND ────────────────────────────────────────────────────────────────

    #[test]
    fn unwind_clause() {
        let q = parse("UNWIND [1, 2, 3] AS x RETURN x");
        assert!(matches!(&q.clauses[0], Clause::Unwind(_)));
        let Clause::Unwind(uc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(uc.alias, Identifier::new("x"));
    }

    // ── DELETE ────────────────────────────────────────────────────────────────

    #[test]
    fn delete_node() {
        let q = parse("MATCH (n) DELETE n");
        let Clause::Delete(dc) = &q.clauses[1] else {
            panic!()
        };
        assert!(!dc.detach);
        assert_eq!(dc.exprs.len(), 1);
    }

    #[test]
    fn detach_delete() {
        let q = parse("MATCH (n) DETACH DELETE n");
        let Clause::Delete(dc) = &q.clauses[1] else {
            panic!()
        };
        assert!(dc.detach);
    }

    // ── Relationship direction ────────────────────────────────────────────────

    #[test]
    fn incoming_relationship() {
        let q = parse("MATCH (a)<-[r:KNOWS]-(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.patterns[0].steps[0].rel.direction, Direction::Incoming);
    }

    #[test]
    fn undirected_relationship() {
        let q = parse("MATCH (a)-[r]-(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.patterns[0].steps[0].rel.direction, Direction::Undirected);
    }

    // ── Path variable binding ─────────────────────────────────────────────────

    #[test]
    fn path_variable() {
        let q = parse("MATCH p = (a)-[:R]->(b) RETURN p");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.patterns[0].variable, Some(Identifier::new("p")));
    }

    // ── Arithmetic expressions ────────────────────────────────────────────────

    #[test]
    fn arithmetic_precedence() {
        // 2 + 3 * 4 should parse as 2 + (3 * 4)
        let q = parse("RETURN 2 + 3 * 4");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        match &rc.items[0].expr {
            Expr::BinaryOp {
                op: BinaryOp::Add,
                right,
                ..
            } => {
                assert!(matches!(
                    right.as_ref(),
                    Expr::BinaryOp {
                        op: BinaryOp::Mul,
                        ..
                    }
                ));
            }
            _ => panic!("expected Add at top level"),
        }
    }

    #[test]
    fn unary_negation() {
        let q = parse("RETURN -42");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            &rc.items[0].expr,
            Expr::UnaryOp {
                op: UnaryOp::Neg,
                ..
            }
        ));
    }

    // ── List literals ─────────────────────────────────────────────────────────

    #[test]
    fn list_literal() {
        let q = parse("RETURN [1, 2, 3]");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::List(items) if items.len() == 3));
    }

    #[test]
    fn empty_list() {
        let q = parse("RETURN []");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::List(items) if items.is_empty()));
    }

    // ── Map literal ───────────────────────────────────────────────────────────

    #[test]
    fn map_literal_expr() {
        let q = parse("RETURN {name: 'Alice', age: 30}");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::MapLiteral(pairs) if pairs.len() == 2));
    }

    // ── Regex match ───────────────────────────────────────────────────────────

    #[test]
    fn regex_match_operator() {
        let q = parse("MATCH (n) WHERE n.name =~ 'A.*' RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::RegexMatch,
                ..
            }
        ));
    }

    // ── Rel type disjunction ──────────────────────────────────────────────────

    #[test]
    fn rel_type_disjunction() {
        let q = parse("MATCH (a)-[r:KNOWS|LIKES]->(b) RETURN a");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        let rel = &mc.patterns[0].steps[0].rel;
        assert_eq!(rel.rel_types.len(), 2);
        assert_eq!(rel.rel_types[0], RelType::new("KNOWS"));
        assert_eq!(rel.rel_types[1], RelType::new("LIKES"));
    }

    // ── Subscript access ──────────────────────────────────────────────────────

    #[test]
    fn subscript_access() {
        let q = parse("RETURN list[0]");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(&rc.items[0].expr, Expr::Subscript(_, _)));
    }

    // ── STARTS WITH / ENDS WITH / CONTAINS ───────────────────────────────────

    #[test]
    fn starts_with() {
        let q = parse("MATCH (n) WHERE n.name STARTS WITH 'A' RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::StartsWith,
                ..
            }
        ));
    }

    #[test]
    fn ends_with() {
        let q = parse("MATCH (n) WHERE n.name ENDS WITH 'son' RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::EndsWith,
                ..
            }
        ));
    }

    #[test]
    fn contains_predicate() {
        let q = parse("MATCH (n) WHERE n.name CONTAINS 'ali' RETURN n");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert!(matches!(
            mc.where_clause.as_ref().unwrap(),
            Expr::BinaryOp {
                op: BinaryOp::Contains,
                ..
            }
        ));
    }

    // ── OPTIONAL MATCH ────────────────────────────────────────────────────────

    #[test]
    fn optional_match() {
        let q = parse("MATCH (n) OPTIONAL MATCH (n)-[r]->(m) RETURN m");
        assert!(matches!(&q.clauses[1], Clause::OptionalMatch(_)));
    }

    // ── Multiple path steps ───────────────────────────────────────────────────

    #[test]
    fn multi_step_path() {
        let q = parse("MATCH (a)-[:R1]->(b)-[:R2]->(c) RETURN c");
        let Clause::Match(mc) = &q.clauses[0] else {
            panic!()
        };
        assert_eq!(mc.patterns[0].steps.len(), 2);
    }

    // ── List comprehension parsing ─────────────────────────────────────────────

    #[test]
    fn list_comprehension_identity() {
        // [x IN [1,2,3]] — no filter, no explicit map (variable itself)
        let q = parse("RETURN [x IN [1, 2, 3]]");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        match &rc.items[0].expr {
            Expr::ListComp {
                variable, filter, ..
            } => {
                assert_eq!(variable.0, "x");
                assert!(filter.is_none());
            }
            other => panic!("expected ListComp, got {other:?}"),
        }
    }

    #[test]
    fn list_comprehension_with_filter_and_map() {
        // [x IN range(1,10) WHERE x > 5 | x * 2]
        let q = parse("RETURN [x IN range(1, 10) WHERE x > 5 | x * 2]");
        let Clause::Return(rc) = &q.clauses[0] else {
            panic!()
        };
        match &rc.items[0].expr {
            Expr::ListComp {
                variable,
                filter,
                map,
                ..
            } => {
                assert_eq!(variable.0, "x");
                assert!(filter.is_some(), "expected a filter");
                assert!(
                    matches!(**map, Expr::BinaryOp { .. }),
                    "expected BinaryOp map expr"
                );
            }
            other => panic!("expected ListComp, got {other:?}"),
        }
    }
}
