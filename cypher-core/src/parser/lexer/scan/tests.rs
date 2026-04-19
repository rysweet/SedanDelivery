//! Tests for the `scan` lexer dispatch and helpers.

use super::super::*;

fn kinds(src: &str) -> Vec<TokenKind> {
    lex_query(src)
        .unwrap()
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

// ── Simple query tokenisation ─────────────────────────────────────────────

#[test]
fn simple_match_return() {
    let toks = kinds("MATCH (n) RETURN n");
    assert_eq!(
        toks,
        vec![
            TokenKind::Match,
            TokenKind::LParen,
            TokenKind::Identifier,
            TokenKind::RParen,
            TokenKind::Return,
            TokenKind::Identifier,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn create_with_label_and_properties() {
    let toks = kinds("CREATE (p:Person {name: 'Alice', age: 30})");
    assert!(toks.contains(&TokenKind::Create));
    assert!(toks.contains(&TokenKind::Colon));
    assert!(toks.contains(&TokenKind::LBrace));
    assert!(toks.contains(&TokenKind::StringLiteral("Alice".to_string())));
    assert!(toks.contains(&TokenKind::IntLiteral(30)));
}

#[test]
fn where_clause_operators() {
    let toks = kinds("WHERE n.age >= 18 AND n.age < 65");
    assert!(toks.contains(&TokenKind::Where));
    assert!(toks.contains(&TokenKind::Gte));
    assert!(toks.contains(&TokenKind::And));
    assert!(toks.contains(&TokenKind::Lt));
}

// ── All operators ─────────────────────────────────────────────────────────

#[test]
fn all_comparison_operators() {
    let toks = kinds("= <> < <= > >=");
    assert_eq!(
        toks,
        vec![
            TokenKind::Eq,
            TokenKind::Neq,
            TokenKind::Lt,
            TokenKind::Lte,
            TokenKind::Gt,
            TokenKind::Gte,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn arrow_operators() {
    let toks = kinds("-> <- -");
    assert_eq!(
        toks,
        vec![
            TokenKind::Arrow,
            TokenKind::LeftArrow,
            TokenKind::Dash,
            TokenKind::Eof
        ]
    );
}

#[test]
fn double_dot_operator() {
    let toks = kinds("[*1..3]");
    assert!(toks.contains(&TokenKind::DoubleDot));
    assert!(!toks.contains(&TokenKind::Dot)); // `..` is not two dots
}

#[test]
fn regex_eq_operator() {
    let toks = kinds("n.name =~ 'A.*'");
    assert!(toks.contains(&TokenKind::RegexEq));
}

#[test]
fn pipe_operator() {
    let toks = kinds("MATCH (n:A|B)");
    assert!(toks.contains(&TokenKind::Pipe));
}

#[test]
fn arithmetic_operators() {
    let toks = kinds("1 + 2 - 3 * 4 / 5 % 6 ^ 7");
    assert!(toks.contains(&TokenKind::Plus));
    assert!(toks.contains(&TokenKind::Dash));
    assert!(toks.contains(&TokenKind::Star));
    assert!(toks.contains(&TokenKind::Slash));
    assert!(toks.contains(&TokenKind::Percent));
    assert!(toks.contains(&TokenKind::Caret));
}

// ── String literal escapes ────────────────────────────────────────────────

#[test]
fn string_single_quoted() {
    let toks = kinds("'hello world'");
    assert_eq!(toks[0], TokenKind::StringLiteral("hello world".to_string()));
}

#[test]
fn string_double_quoted() {
    let toks = kinds("\"hello world\"");
    assert_eq!(toks[0], TokenKind::StringLiteral("hello world".to_string()));
}

#[test]
fn string_escape_newline() {
    let toks = kinds(r"'line1\nline2'");
    assert_eq!(
        toks[0],
        TokenKind::StringLiteral("line1\nline2".to_string())
    );
}

#[test]
fn string_escape_tab() {
    let toks = kinds(r"'col1\tcol2'");
    assert_eq!(toks[0], TokenKind::StringLiteral("col1\tcol2".to_string()));
}

#[test]
fn string_escape_backslash() {
    let toks = kinds(r"'a\\b'");
    assert_eq!(toks[0], TokenKind::StringLiteral("a\\b".to_string()));
}

#[test]
fn string_escape_quote_in_single() {
    let toks = kinds(r"'it\'s'");
    assert_eq!(toks[0], TokenKind::StringLiteral("it's".to_string()));
}

#[test]
fn string_unicode_escape() {
    let toks = kinds(r"'\u0041'"); // U+0041 = 'A'
    assert_eq!(toks[0], TokenKind::StringLiteral("A".to_string()));
}

// ── Unterminated string ───────────────────────────────────────────────────

#[test]
fn unterminated_string_errors() {
    let result = lex_query("'unterminated");
    assert!(matches!(
        result,
        Err(ParseError::UnterminatedString { offset: 0 })
    ));
}

#[test]
fn unterminated_double_quoted_string_errors() {
    let result = lex_query("\"oops");
    assert!(matches!(
        result,
        Err(ParseError::UnterminatedString { offset: 0 })
    ));
}

// ── Integer / float literals ──────────────────────────────────────────────

#[test]
fn integer_literal() {
    let toks = kinds("42");
    assert_eq!(toks[0], TokenKind::IntLiteral(42));
}

#[test]
fn negative_integer_is_dash_then_literal() {
    // Cypher parses `-1` as unary minus applied to `1`.
    let toks = kinds("-1");
    assert_eq!(toks[0], TokenKind::Dash);
    assert_eq!(toks[1], TokenKind::IntLiteral(1));
}

#[test]
fn float_with_decimal() {
    let toks = kinds("1.23456");
    assert_eq!(toks[0], TokenKind::FloatLiteral(1.23456));
}

#[test]
fn float_with_exponent() {
    let toks = kinds("1.5e3");
    assert_eq!(toks[0], TokenKind::FloatLiteral(1500.0));
}

#[test]
fn float_exponent_negative() {
    let toks = kinds("2e-2");
    assert!(matches!(toks[0], TokenKind::FloatLiteral(v) if (v - 0.02).abs() < 1e-10));
}

#[test]
fn integer_overflow_error() {
    // 2^63 + 1 overflows i64.
    let result = lex_query("9999999999999999999");
    assert!(matches!(result, Err(ParseError::IntegerOverflow { .. })));
}

// ── Identifiers ───────────────────────────────────────────────────────────

#[test]
fn plain_identifier() {
    let tokens = lex_query("myVar").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Identifier);
    assert_eq!(tokens[0].text, "myVar");
}

#[test]
fn backtick_identifier() {
    let tokens = lex_query("`has spaces`").unwrap();
    assert_eq!(tokens[0].kind, TokenKind::Identifier);
    assert_eq!(tokens[0].text, "`has spaces`");
}

// ── Keywords ─────────────────────────────────────────────────────────────

#[test]
fn keywords_case_insensitive() {
    let toks = kinds("match MATCH Match");
    assert_eq!(toks[0], TokenKind::Match);
    assert_eq!(toks[1], TokenKind::Match);
    assert_eq!(toks[2], TokenKind::Match);
}

#[test]
fn all_keywords_recognised() {
    let query = "MATCH OPTIONAL WHERE RETURN CREATE MERGE SET REMOVE DELETE DETACH \
                 WITH UNWIND ORDER BY LIMIT SKIP DISTINCT AS IN NOT AND OR XOR \
                 IS NULL TRUE FALSE CALL FOREACH UNION ALL CASE WHEN THEN ELSE END \
                 STARTS ENDS CONTAINS COUNT";
    let toks = lex_query(query).unwrap();
    let non_eof_non_space: Vec<_> = toks.iter().filter(|t| !t.kind.is_eof()).collect();
    // Should have exactly as many tokens as keywords listed.
    assert_eq!(non_eof_non_space.len(), 40);
}

// ── Comments ─────────────────────────────────────────────────────────────

#[test]
fn line_comment_skipped() {
    let toks = kinds("MATCH // this is a comment\nRETURN");
    assert_eq!(
        toks,
        vec![TokenKind::Match, TokenKind::Return, TokenKind::Eof]
    );
}

#[test]
fn block_comment_skipped() {
    let toks = kinds("MATCH /* ignore this */ RETURN");
    assert_eq!(
        toks,
        vec![TokenKind::Match, TokenKind::Return, TokenKind::Eof]
    );
}

#[test]
fn nested_block_comments_are_not_nested() {
    // Standard Cypher: `/* /* */` ends at the first `*/`
    let toks = kinds("MATCH /* outer /* inner */ RETURN");
    // "RETURN" is consumed inside the comment, so only Match + Eof remain.
    // (or just Eof if there's nothing after; depends on what follows `*/`)
    // The point is we don't panic and Eof is present.
    assert!(toks.last().unwrap().is_eof());
}

// ── Token offsets ─────────────────────────────────────────────────────────

#[test]
fn token_offsets_correct() {
    let src = "MATCH (n)";
    let tokens = lex_query(src).unwrap();
    assert_eq!(tokens[0].offset, 0); // MATCH
    assert_eq!(tokens[1].offset, 6); // (
    assert_eq!(tokens[2].offset, 7); // n
    assert_eq!(tokens[3].offset, 8); // )
}

// ── Parameter sigil ───────────────────────────────────────────────────────

#[test]
fn dollar_sign_emitted() {
    let toks = kinds("$name");
    assert_eq!(toks[0], TokenKind::DollarSign);
    assert_eq!(toks[1], TokenKind::Identifier);
}

// ── Semicolon and other punctuation ──────────────────────────────────────

#[test]
fn semicolon_and_braces() {
    let toks = kinds(";{}[]()");
    assert_eq!(
        toks,
        vec![
            TokenKind::Semicolon,
            TokenKind::LBrace,
            TokenKind::RBrace,
            TokenKind::LBracket,
            TokenKind::RBracket,
            TokenKind::LParen,
            TokenKind::RParen,
            TokenKind::Eof,
        ]
    );
}

// ── Empty input ───────────────────────────────────────────────────────────

#[test]
fn empty_input_gives_eof() {
    let toks = kinds("");
    assert_eq!(toks, vec![TokenKind::Eof]);
}

#[test]
fn whitespace_only_gives_eof() {
    let toks = kinds("   \n\t  ");
    assert_eq!(toks, vec![TokenKind::Eof]);
}

// ── Invalid escape ────────────────────────────────────────────────────────

#[test]
fn invalid_escape_sequence_errors() {
    let result = lex_query(r"'\q'");
    assert!(matches!(
        result,
        Err(ParseError::InvalidEscape { char: 'q', .. })
    ));
}
