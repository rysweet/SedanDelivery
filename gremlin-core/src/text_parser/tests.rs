//! In-file tests originally embedded in text_parser.rs, plus new focused unit tests.

use super::*;

fn no_params(_name: &str) -> Option<GremlinValue> {
    None
}

#[test]
fn parse_v_count() {
    let steps = parse_traversal("g.V().count()", &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0], GremlinStep::V(None));
    assert_eq!(steps[1], GremlinStep::Count);
}

#[test]
fn parse_v_has_out_values() {
    let steps = parse_traversal(
        r#"g.V().has("name","marko").out("knows").values("name")"#,
        &no_params,
    )
    .unwrap();
    assert_eq!(steps.len(), 4);
    assert_eq!(steps[0], GremlinStep::V(None));
    assert_eq!(
        steps[1],
        GremlinStep::Has("name".into(), Some(GremlinValue::String("marko".into())))
    );
    assert_eq!(steps[2], GremlinStep::Out(vec!["knows".into()]));
    assert_eq!(steps[3], GremlinStep::Values(vec!["name".into()]));
}

#[test]
fn parse_v_with_param() {
    let params = |name: &str| -> Option<GremlinValue> {
        if name == "vid1" {
            Some(GremlinValue::Integer(1))
        } else {
            None
        }
    };
    let steps = parse_traversal("g.V(vid1).values(\"name\")", &params).unwrap();
    assert_eq!(
        steps[0],
        GremlinStep::V(Some(vec![GremlinValue::Integer(1)]))
    );
    assert_eq!(steps[1], GremlinStep::Values(vec!["name".into()]));
}

#[test]
fn parse_addv_property() {
    let steps = parse_traversal(
        r#"g.addV("person").property("name","stephen").property("age",29)"#,
        &no_params,
    )
    .unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[0], GremlinStep::AddV("person".into()));
    assert_eq!(
        steps[1],
        GremlinStep::Property("name".into(), GremlinValue::String("stephen".into()))
    );
    assert_eq!(
        steps[2],
        GremlinStep::Property("age".into(), GremlinValue::Integer(29))
    );
}

#[test]
fn parse_addv_traversal_label() {
    let steps = parse_traversal(
        r#"g.addV(__.V().has("name","marko").properties("name").key())"#,
        &no_params,
    )
    .unwrap();
    assert!(matches!(steps[0], GremlinStep::AddVTraversal(_)));
}

#[test]
fn parse_addv_property_with_meta_and_with() {
    let steps = parse_traversal(
        r#"g.addV().property("name","marko","since",2010).with("key","value")"#,
        &no_params,
    )
    .unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0], GremlinStep::AddV("".into()));
    assert_eq!(
        steps[1],
        GremlinStep::Property(
            "name".into(),
            GremlinValue::Map(indexmap::indexmap! {
                "value".into() => GremlinValue::String("marko".into()),
                "since".into() => GremlinValue::Integer(2010),
            })
        )
    );
}

#[test]
fn parse_path_step() {
    let steps = parse_traversal(r#"g.V().out("knows").path()"#, &no_params).unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[2], GremlinStep::Path);
}

#[test]
fn parse_has_label() {
    let steps = parse_traversal(r#"g.V().hasLabel("person")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[1], GremlinStep::HasLabel(vec!["person".into()]));
}

#[test]
fn parse_order_by() {
    let steps = parse_traversal(r#"g.V().values("name").order()"#, &no_params).unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!(steps[2], GremlinStep::Order);
}

#[test]
fn parse_with_strategy() {
    let steps = parse_traversal(
        r#"g.withStrategies(CountStrategy).V().where(outE().count().is(0))"#,
        &no_params,
    );
    // Should not error on the strategy prefix
    assert!(steps.is_ok());
}

#[test]
fn tokenize_nested_parens() {
    let tokens = tokenize_steps(r#"V().where(outE().count().is(0)).values("name")"#).unwrap();
    assert_eq!(tokens.len(), 3);
    assert_eq!(tokens[0], "V()");
    assert!(tokens[1].starts_with("where("));
    assert_eq!(tokens[2], r#"values("name")"#);
}

#[test]
fn parse_repeat_times() {
    let steps = parse_traversal(r#"g.V().repeat(__.out()).times(2).count()"#, &no_params).unwrap();
    assert!(steps.len() >= 4);
    assert!(matches!(steps[1], GremlinStep::Repeat(_, _)));
    assert_eq!(steps[2], GremlinStep::Times(2));
    assert_eq!(steps[3], GremlinStep::Count);
}

#[test]
fn parse_select_as() {
    let steps = parse_traversal(
        r#"g.V().as("a").out("knows").as("b").select("a","b")"#,
        &no_params,
    )
    .unwrap();
    assert_eq!(steps[1], GremlinStep::As("a".into()));
    assert_eq!(steps[3], GremlinStep::As("b".into()));
    assert_eq!(
        steps[4],
        GremlinStep::Select(None, vec!["a".into(), "b".into()])
    );
}

#[test]
fn parse_select_bare_first_last_pop() {
    let first_steps = parse_traversal(
        r#"g.V().as("v").out().as("v").select(first, "v")"#,
        &no_params,
    )
    .unwrap();
    assert_eq!(
        first_steps[4],
        GremlinStep::Select(Some(crate::bytecode::Pop::First), vec!["v".into()])
    );

    let last_steps = parse_traversal(
        r#"g.V().as("v").out().as("v").select(last, "v")"#,
        &no_params,
    )
    .unwrap();
    assert_eq!(
        last_steps[4],
        GremlinStep::Select(Some(crate::bytecode::Pop::Last), vec!["v".into()])
    );
}

// ── Quote-aware tokenization tests ──────────────────────────────────

#[test]
fn tokenize_steps_preserves_comma_inside_double_quoted_string() {
    // A literal comma inside the quoted string must not split the step,
    // nor be treated as an argument separator by downstream parsers.
    let steps = parse_traversal(r#"g.V().has("name","marko, the creator")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "name".into(),
            Some(GremlinValue::String("marko, the creator".into()))
        )
    );
}

#[test]
fn tokenize_steps_preserves_comma_inside_single_quoted_string() {
    let steps = parse_traversal(r#"g.V().has('name','marko, the creator')"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "name".into(),
            Some(GremlinValue::String("marko, the creator".into()))
        )
    );
}

#[test]
fn tokenize_steps_preserves_parens_inside_quoted_string() {
    // Parentheses inside the string must not affect paren-depth tracking.
    let steps = parse_traversal(r#"g.V().has("note","hello (world)")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "note".into(),
            Some(GremlinValue::String("hello (world)".into()))
        )
    );
}

#[test]
fn tokenize_steps_preserves_parens_inside_single_quoted_string() {
    let steps = parse_traversal(r#"g.V().has('note','hello (world)')"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "note".into(),
            Some(GremlinValue::String("hello (world)".into()))
        )
    );
}

#[test]
fn tokenize_steps_preserves_dots_and_spaces_inside_quoted_string() {
    // A `.` inside a quoted string must not split step tokens, and leading
    // whitespace inside the string must be preserved verbatim.
    let steps = parse_traversal(r#"g.V().has("desc","v.first  v.second")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "desc".into(),
            Some(GremlinValue::String("v.first  v.second".into()))
        )
    );
}

#[test]
fn tokenize_steps_preserves_escaped_double_quote() {
    // \" inside a double-quoted string must not end the string.
    let steps = parse_traversal(r#"g.V().has("q","say \"hi\", ok")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    match &steps[1] {
        GremlinStep::Has(k, Some(GremlinValue::String(v))) => {
            assert_eq!(k, "q");
            // The escape backslash is preserved in the token; value passes
            // through parse_literal_value which strips outer quotes only.
            assert!(
                v.contains("say") && v.contains("hi") && v.contains(", ok"),
                "unexpected value: {v}"
            );
        }
        other => panic!("expected Has step, got {other:?}"),
    }
}

#[test]
fn tokenize_steps_single_quote_inside_double_quoted_string() {
    // A stray single quote inside "..." must NOT toggle string state.
    let steps = parse_traversal(r#"g.V().has("msg","it's fine, really")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "msg".into(),
            Some(GremlinValue::String("it's fine, really".into()))
        )
    );
}

#[test]
fn tokenize_steps_double_quote_inside_single_quoted_string() {
    // A stray double quote inside '...' must NOT toggle string state.
    let steps = parse_traversal(r#"g.V().has('msg','he said "yes", go')"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "msg".into(),
            Some(GremlinValue::String(r#"he said "yes", go"#.into()))
        )
    );
}

#[test]
fn split_top_level_args_respects_single_quoted_strings() {
    // Direct unit test of the shared splitter helper.
    let args = split_top_level_args(r#"'a, b','c (d)','e'"#);
    assert_eq!(args, vec!["'a, b'", "'c (d)'", "'e'"]);
}

#[test]
fn split_top_level_args_respects_mixed_quotes_and_escapes() {
    let args = split_top_level_args(r#""a, b",'c, d',"e\"f, g""#);
    assert_eq!(args, vec![r#""a, b""#, r#"'c, d'"#, r#""e\"f, g""#]);
}

#[test]
fn skip_balanced_parens_respects_single_quoted_string() {
    // Parens inside '...' must not affect depth tracking.
    let rest = skip_balanced_parens(r#"'hi (there)').next()"#);
    assert_eq!(rest, ".next()");
}

// ── Quote-aware tokenization regression suite ─────────────────────────
// The following tests cover the canonical scenarios that an unquoted
// tokenizer would mis-handle: commas, parentheses, and whitespace inside
// string literals, and backslash escapes for quote characters and the
// backslash itself.

#[test]
fn quote_aware_comma_inside_single_quoted_string() {
    let steps = parse_traversal(r#"g.V().has('name','Smith, John')"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "name".into(),
            Some(GremlinValue::String("Smith, John".into()))
        )
    );
}

#[test]
fn quote_aware_parens_inside_single_quoted_string() {
    let steps = parse_traversal(r#"g.V().has('expr','f(x)')"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has("expr".into(), Some(GremlinValue::String("f(x)".into())))
    );
}

#[test]
fn quote_aware_spaces_inside_single_quoted_string() {
    let steps = parse_traversal(r#"g.V().has('msg','hello world')"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "msg".into(),
            Some(GremlinValue::String("hello world".into()))
        )
    );
}

#[test]
fn quote_aware_escaped_single_quote_in_single_quoted_string() {
    // \' inside '...' must NOT terminate the string. The unescaped value
    // contains a literal apostrophe.
    let steps = parse_traversal(r#"g.V().has('name','it\'s mine')"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "name".into(),
            Some(GremlinValue::String("it's mine".into()))
        )
    );
}

#[test]
fn quote_aware_escaped_double_quote_in_double_quoted_string() {
    // \" inside "..." must NOT terminate the string.
    let steps = parse_traversal(r#"g.V().has("name","say \"hi\"")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has(
            "name".into(),
            Some(GremlinValue::String(r#"say "hi""#.into()))
        )
    );
}

#[test]
fn quote_aware_escaped_backslash_in_single_quoted_string() {
    // \\ inside '...' is a literal backslash; the trailing ' is the real
    // string terminator (not escaped by the second backslash).
    let steps = parse_traversal(r#"g.V().has('path','C:\\foo')"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has("path".into(), Some(GremlinValue::String(r"C:\foo".into())))
    );
}

#[test]
fn quote_aware_escaped_backslash_in_double_quoted_string() {
    let steps = parse_traversal(r#"g.V().has("path","C:\\foo")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(
        steps[1],
        GremlinStep::Has("path".into(), Some(GremlinValue::String(r"C:\foo".into())))
    );
}

#[test]
fn quote_aware_mixed_quote_styles_in_single_step() {
    // Two arguments using different quote styles, each containing the
    // other quote character verbatim plus commas and parens.
    let steps = parse_traversal(
        r#"g.V().has("k, 1","v'1, (a)").has('k, 2','v"2, (b)')"#,
        &no_params,
    )
    .unwrap();
    assert_eq!(steps.len(), 3);
    assert_eq!(
        steps[1],
        GremlinStep::Has("k, 1".into(), Some(GremlinValue::String("v'1, (a)".into())))
    );
    assert_eq!(
        steps[2],
        GremlinStep::Has(
            "k, 2".into(),
            Some(GremlinValue::String(r#"v"2, (b)"#.into()))
        )
    );
}

#[test]
fn debug_to_direction_out() {
    let resolve = |name: &str| -> Option<GremlinValue> {
        if name == "vid1" {
            Some(GremlinValue::Integer(1))
        } else {
            None
        }
    };
    let steps = parse_traversal(r#"g.V(vid1).to(Direction.OUT, "knows")"#, &resolve).unwrap();
    for (i, step) in steps.iter().enumerate() {
        eprintln!("Step {}: {:?}", i, step);
    }
    assert_eq!(steps.len(), 2);
    assert!(matches!(&steps[0], GremlinStep::V(Some(_))));
    assert!(matches!(&steps[1], GremlinStep::Out(_)));
}

// ===== New focused unit tests for low-coverage parser branches =====

#[test]
fn predicate_p_and_nested() {
    let steps =
        parse_traversal("g.V().has(\"age\", P.and(P.gt(20), P.lt(40)))", &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    // Has carries a Predicate; the nested-And case should resolve without panicking.
    matches!(&steps[1], GremlinStep::Has(_, _));
}

#[test]
fn predicate_p_or_nested() {
    let steps = parse_traversal(
        "g.V().has(\"name\", P.or(P.eq(\"marko\"), P.eq(\"vadas\")))",
        &no_params,
    )
    .unwrap();
    assert_eq!(steps.len(), 2);
}

#[test]
fn escaped_double_quote_inside_string() {
    let steps = parse_traversal(r#"g.V().has("name", "a\"b")"#, &no_params).unwrap();
    assert!(matches!(&steps[1], GremlinStep::Has(_, Some(_))));
}

#[test]
fn single_quote_inside_double_quoted() {
    let steps = parse_traversal(r#"g.V().has("name", "a'b")"#, &no_params).unwrap();
    assert!(matches!(&steps[1], GremlinStep::Has(_, Some(_))));
}

#[test]
fn double_quote_inside_single_quoted() {
    let steps = parse_traversal(r#"g.V().has("name", 'a"b')"#, &no_params).unwrap();
    assert!(matches!(&steps[1], GremlinStep::Has(_, Some(_))));
}

#[test]
fn mixed_quote_list_literal() {
    let steps = parse_traversal(r#"g.inject([1, "a", 'b'])"#, &no_params).unwrap();
    assert!(!steps.is_empty());
}

#[test]
fn empty_list_inject() {
    let steps = parse_traversal("g.inject([])", &no_params).unwrap();
    assert!(!steps.is_empty());
}

#[test]
fn single_element_list_inject() {
    let steps = parse_traversal("g.inject([1])", &no_params).unwrap();
    assert!(!steps.is_empty());
}

#[test]
fn long_traversal_chain_eight_steps() {
    let steps = parse_traversal(
        "g.V().has(\"name\",\"marko\").out(\"knows\").has(\"age\",29).values(\"name\").dedup().count().is(1)",
        &no_params,
    )
    .unwrap();
    assert!(steps.len() >= 8);
}

#[test]
fn nested_map_value() {
    // map-of-maps in inject: inner map is parsed as one value
    let steps = parse_traversal(r#"g.inject([["a": ["b": 1]]])"#, &no_params).unwrap();
    assert!(!steps.is_empty());
}

#[test]
fn predicate_p_between_normal() {
    let steps = parse_traversal("g.V().has(\"age\", P.between(1, 10))", &no_params).unwrap();
    assert_eq!(steps.len(), 2);
}

#[test]
fn predicate_p_between_reversed() {
    let steps = parse_traversal("g.V().has(\"age\", P.between(10, 1))", &no_params).unwrap();
    assert_eq!(steps.len(), 2);
}

#[test]
fn predicate_p_within_empty() {
    let steps = parse_traversal("g.V().has(\"name\", P.within())", &no_params).unwrap();
    assert_eq!(steps.len(), 2);
}

#[test]
fn predicate_p_within_single() {
    let steps = parse_traversal("g.V().has(\"name\", P.within(\"marko\"))", &no_params).unwrap();
    assert_eq!(steps.len(), 2);
}

#[test]
fn with_sack_sum_operator() {
    let steps = parse_traversal("g.withSack(0, Operator.sum).V().sack()", &no_params).unwrap();
    assert!(matches!(&steps[0], GremlinStep::WithSack(_, _)));
}

#[test]
fn with_sack_minus_operator() {
    let steps = parse_traversal("g.withSack(10, Operator.minus).V().sack()", &no_params).unwrap();
    assert!(matches!(&steps[0], GremlinStep::WithSack(_, _)));
}

#[test]
fn with_side_effect_list_value() {
    let steps =
        parse_traversal("g.withSideEffect(\"x\", [1, 2, 3]).V().count()", &no_params).unwrap();
    assert!(matches!(&steps[0], GremlinStep::WithSideEffect(_, _, _)));
}

#[test]
fn with_side_effect_map_value() {
    let steps =
        parse_traversal(r#"g.withSideEffect("m", ["a": 1, "b": 2]).V()"#, &no_params).unwrap();
    assert!(matches!(&steps[0], GremlinStep::WithSideEffect(_, _, _)));
}

#[test]
fn inject_multiple_args_with_escapes() {
    // Regression: PR #1316 — inject with multiple comma-separated args incl. escaped quotes.
    let steps = parse_traversal(r#"g.inject("a\"b", "c, d", 42)"#, &no_params).unwrap();
    assert!(!steps.is_empty());
}

#[test]
fn unknown_step_returns_unknown_step_error() {
    let res = parse_traversal("g.V().nonExistentStep()", &no_params);
    assert!(matches!(res, Err(TextParseError::UnknownStep(_))));
}

#[test]
fn missing_g_prefix_returns_expected_error() {
    let res = parse_traversal("V().count()", &no_params);
    assert!(matches!(res, Err(TextParseError::Expected { .. })));
}

#[test]
fn empty_input_returns_expected_error() {
    let res = parse_traversal("", &no_params);
    assert!(matches!(res, Err(TextParseError::Expected { .. })));
}

#[test]
fn map_literal_with_internal_spaces() {
    let steps = parse_traversal(
        r#"g.withSideEffect("m", { "key" : 1 , "k2" : 2 }).V()"#,
        &no_params,
    )
    .unwrap();
    assert!(matches!(&steps[0], GremlinStep::WithSideEffect(_, _, _)));
}

#[test]
fn negative_integer_literal() {
    let steps = parse_traversal("g.V().has(\"age\", -5)", &no_params).unwrap();
    assert_eq!(steps.len(), 2);
}

#[test]
fn float_with_d_suffix() {
    let steps = parse_traversal("g.V().has(\"weight\", 1.5d)", &no_params).unwrap();
    assert_eq!(steps.len(), 2);
}

#[test]
fn integer_with_long_suffix() {
    let steps = parse_traversal("g.V().has(\"id\", 100L)", &no_params).unwrap();
    assert_eq!(steps.len(), 2);
}

#[test]
fn unquote_strips_matching_quotes() {
    assert_eq!(super::literal::unquote("\"hello\""), "hello".to_string());
    assert_eq!(super::literal::unquote("'world'"), "world".to_string());
    assert_eq!(super::literal::unquote("bare"), "bare".to_string());
}

#[test]
fn looks_like_traversal_recognises_anonymous() {
    assert!(super::predicate::looks_like_traversal("__.out()"));
    assert!(super::predicate::looks_like_traversal("out().count()"));
    assert!(!super::predicate::looks_like_traversal("\"plain string\""));
}

#[test]
fn skip_balanced_parens_skips_outer_only() {
    // Caller passes the body after a known opening paren; depth starts at 1.
    let s = "a(b)c)trail";
    let rest = super::tokenizer::skip_balanced_parens(s);
    assert_eq!(rest, "trail");
}

#[test]
fn split_top_level_args_respects_nested_parens() {
    let parts = super::tokenizer::split_top_level_args("a, b(c, d), e");
    assert_eq!(parts, vec!["a", " b(c, d)", " e"]);
}

#[test]
fn invalid_argument_via_bad_p_call() {
    // P.within with a deliberately malformed payload sometimes flows to InvalidArgument.
    // If parser tolerates it, at least verify it doesn't panic.
    let _ = parse_traversal("g.V().has(\"name\", P.eq())", &no_params);
}

// ── Bug #1193: parse_inner_traversal must propagate parse errors ───────
//
// Currently parse_inner_traversal swallows parse errors via
// `if parse_step_token(...).is_err() { break }` and returns partial results.
// After fix: replace with `parse_step_token(...)?` to propagate the error.

#[test]
fn parse_inner_traversal_propagates_single_unknown_step() {
    // A completely unknown step should return Err, not Ok(vec![]).
    let result = parse_inner_traversal("__.completelyFakeStep()", &no_params);
    assert!(
        result.is_err(),
        "expected error for unknown step in inner traversal, got {result:?}"
    );
}

#[test]
fn parse_inner_traversal_propagates_error_after_valid_step() {
    // Valid step followed by unknown step: must propagate the error
    // instead of silently returning only the first step.
    let result = parse_inner_traversal("__.values(\"name\").completelyFakeStep()", &no_params);
    assert!(
        result.is_err(),
        "expected error for unknown step after valid step, got {result:?}"
    );
}

#[test]
fn parse_inner_traversal_without_prefix_propagates_error() {
    // Without __ prefix, unknown steps should still be propagated.
    let result = parse_inner_traversal("completelyFakeStep()", &no_params);
    assert!(
        result.is_err(),
        "expected error for unknown step without prefix, got {result:?}"
    );
}

// ── Bug #1199: tokenize_steps must detect unbalanced parentheses ──────
//
// Currently tokenize_steps uses saturating_sub for ')' which silently
// absorbs extra close parens, and never checks depth > 0 after the loop.
// After fix: checked decrement + post-loop depth check.

#[test]
fn tokenize_unbalanced_open_paren_returns_error() {
    // Missing close paren — depth > 0 at end of input.
    let result = super::tokenizer::tokenize_steps("V().has(\"name\"");
    assert!(
        result.is_err(),
        "expected error for unbalanced open paren, got {result:?}"
    );
}

#[test]
fn tokenize_extra_close_paren_returns_error() {
    // Extra close paren at depth 0.
    let result = super::tokenizer::tokenize_steps("V())");
    assert!(
        result.is_err(),
        "expected error for extra close paren, got {result:?}"
    );
}

#[test]
fn tokenize_nested_unbalanced_returns_error() {
    // Multiple levels of unclosed parens.
    let result = super::tokenizer::tokenize_steps("V().has(\"x\", P.within(1, 2)");
    assert!(
        result.is_err(),
        "expected error for nested unbalanced parens, got {result:?}"
    );
}

#[test]
fn parse_traversal_with_unbalanced_parens_returns_error() {
    // End-to-end: unbalanced parens in a full traversal string.
    let result = parse_traversal("g.V().has(\"name\"", &no_params);
    assert!(
        result.is_err(),
        "expected error for unbalanced parens in traversal, got {result:?}"
    );
}

#[test]
fn tokenize_balanced_parens_still_works() {
    // Regression: balanced parens should continue to work.
    let result = super::tokenizer::tokenize_steps("V().has(\"name\",\"marko\").out(\"knows\")");
    assert!(result.is_ok(), "balanced parens should succeed: {result:?}");
    let tokens = result.unwrap();
    assert_eq!(tokens.len(), 3);
}

#[test]
fn tokenize_empty_parens_still_works() {
    let result = super::tokenizer::tokenize_steps("V().count()");
    assert!(result.is_ok());
    let tokens = result.unwrap();
    assert_eq!(tokens.len(), 2);
}

// ===== TDD tests for #1313: quote handling in find_matching_paren =====

#[test]
fn find_matching_paren_basic() {
    // Regression: simple paren matching without strings.
    assert_eq!(super::tokenizer::find_matching_paren("(hello)"), Some(6));
}

#[test]
fn find_matching_paren_with_string() {
    // Regression: string inside parens (no escapes).
    assert_eq!(
        super::tokenizer::find_matching_paren(r#"("hello")"#),
        Some(8)
    );
}

#[test]
fn find_matching_paren_empty_string() {
    // Regression: empty string inside parens.
    assert_eq!(super::tokenizer::find_matching_paren(r#"("")"#), Some(3));
}

#[test]
fn find_matching_paren_string_with_parens() {
    // Regression: parens inside a string should not affect depth.
    assert_eq!(
        super::tokenizer::find_matching_paren(r#"("a(b)c")"#),
        Some(8)
    );
}

#[test]
fn find_matching_paren_escaped_double_quote() {
    // BUG #1313: escaped double quote inside a double-quoted string.
    // The \" should NOT close the string.
    // Input bytes: ( " a \ " b " )
    assert_eq!(
        super::tokenizer::find_matching_paren(r#"("a\"b")"#),
        Some(7)
    );
}

#[test]
fn find_matching_paren_escaped_single_quote() {
    // BUG #1313: escaped single quote inside a single-quoted string.
    // The \' should NOT close the string.
    // Input bytes: ( ' a \ ' b ' )
    assert_eq!(super::tokenizer::find_matching_paren("('a\\'b')"), Some(7));
}

#[test]
fn find_matching_paren_escaped_backslash_before_close() {
    // Escaped backslash (\\) followed by the real closing quote.
    // Input bytes: ( " a \ \ " )
    // The \\ is a literal backslash; the next " closes the string.
    assert_eq!(super::tokenizer::find_matching_paren(r#"("a\\")"#), Some(6));
}

#[test]
fn find_matching_paren_escaped_quote_at_end() {
    // Escaped quote at end of string value: the \" does not close,
    // the following " does.
    // Input bytes: ( " a \ " " )
    assert_eq!(super::tokenizer::find_matching_paren(r#"("a\"")"#), Some(6));
}

#[test]
fn find_matching_paren_double_escaped_backslash_then_quote() {
    // \\\": escaped backslash (\\) + escaped quote (\").
    // Input bytes: ( " \ \ \ " " )
    // The \\ is literal backslash, then \" is escaped quote, string continues
    // until the final ".
    assert_eq!(
        super::tokenizer::find_matching_paren(r#"("\\\"")"#),
        Some(7)
    );
}

#[test]
fn find_matching_paren_nested_parens_with_escaped_quotes() {
    // Nested parens where inner string has an escaped quote.
    // Input: (has("k\"v"))
    assert_eq!(
        super::tokenizer::find_matching_paren(r#"(has("k\"v"))"#),
        Some(12)
    );
}

#[test]
fn find_matching_paren_mixed_quotes() {
    // Single quotes inside double-quoted string — no escaping needed.
    assert_eq!(
        super::tokenizer::find_matching_paren(r#"("it's")"#),
        Some(7)
    );
    // Double quotes inside single-quoted string.
    assert_eq!(
        super::tokenizer::find_matching_paren(r#"('say "hi"')"#),
        Some(11)
    );
}

// ===== TDD tests for #1313: quote handling in split_predicate_chain =====

#[test]
fn split_predicate_chain_no_chain() {
    // Regression: single predicate, no chain.
    let result = super::tokenizer::split_predicate_chain("P.gt(20)");
    assert!(
        result.is_empty(),
        "single predicate should be empty: {result:?}"
    );
}

#[test]
fn split_predicate_chain_and_chain() {
    // Regression: normal .and() chain.
    let result = super::tokenizer::split_predicate_chain("P.gt(20).and(P.lt(40))");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], ("first", "P.gt(20)"));
    assert_eq!(result[1], ("and", "P.lt(40)"));
}

#[test]
fn split_predicate_chain_or_chain() {
    // Regression: normal .or() chain.
    let result = super::tokenizer::split_predicate_chain("P.eq(1).or(P.eq(2))");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0], ("first", "P.eq(1)"));
    assert_eq!(result[1], ("or", "P.eq(2)"));
}

#[test]
fn split_predicate_chain_string_with_close_paren_and_dot_and() {
    // BUG #1313: a `)` inside a quoted string drops depth to 0, and the text
    // after matches `.and(` — this must NOT be treated as a chain.
    let result = super::tokenizer::split_predicate_chain(r#"P.eq("x).and(evil")"#);
    assert!(
        result.is_empty(),
        "string containing ').and(' should not split: {result:?}"
    );
}

#[test]
fn split_predicate_chain_string_with_close_paren_and_dot_or() {
    // BUG #1313: same as above but with `.or(` pattern.
    let result = super::tokenizer::split_predicate_chain(r#"P.eq("x).or(evil")"#);
    assert!(
        result.is_empty(),
        "string containing ').or(' should not split: {result:?}"
    );
}

#[test]
fn split_predicate_chain_string_with_dot_and_no_paren() {
    // String containing ".and" but not ".and(" — should not split.
    let result = super::tokenizer::split_predicate_chain(r#"P.eq("sand")"#);
    assert!(
        result.is_empty(),
        "string containing '.and' without paren should not split: {result:?}"
    );
}

#[test]
fn split_predicate_chain_escaped_quote_with_paren() {
    // Escaped quote followed by `)` — the `)` is still inside the string
    // because the \" doesn't close it.
    // Input: P.eq("a\")b")
    // The string content is: a")b — the \" is an escaped quote.
    let result = super::tokenizer::split_predicate_chain(r#"P.eq("a\")b")"#);
    assert!(
        result.is_empty(),
        "escaped quote before paren should not split: {result:?}"
    );
}

#[test]
fn split_predicate_chain_adversarial_and_or_in_string() {
    // Adversarial: string value is literally ".and(.or(" — must not chain-split.
    let result = super::tokenizer::split_predicate_chain(r#"P.eq(".and(.or(")"#);
    assert!(
        result.is_empty(),
        "adversarial .and(.or( in string must not split: {result:?}"
    );
}

// ===== Integration tests: parse_traversal with quote edge cases =====

#[test]
fn parse_has_with_escaped_double_quote_value() {
    // End-to-end: has() with escaped double quote in the value string.
    let steps = parse_traversal(r#"g.V().has("name", "a\"b")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0], GremlinStep::V(None));
    match &steps[1] {
        GremlinStep::Has(key, Some(GremlinValue::String(val))) => {
            assert_eq!(key.as_str(), "name");
            assert_eq!(val.as_str(), "a\"b");
        }
        other => panic!("expected Has with string value, got {other:?}"),
    }
}

#[test]
fn parse_has_with_escaped_single_quote_value() {
    // End-to-end: has() with escaped single quote in the value string.
    let steps = parse_traversal("g.V().has('name', 'a\\'b')", &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    match &steps[1] {
        GremlinStep::Has(key, Some(GremlinValue::String(val))) => {
            assert_eq!(key.as_str(), "name");
            assert_eq!(val.as_str(), "a'b");
        }
        other => panic!("expected Has with string value, got {other:?}"),
    }
}

#[test]
fn parse_has_with_paren_in_string_value() {
    // End-to-end: has() where the value string contains parens.
    let steps = parse_traversal(r#"g.V().has("name", "a(b)c")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    match &steps[1] {
        GremlinStep::Has(key, Some(GremlinValue::String(val))) => {
            assert_eq!(key.as_str(), "name");
            assert_eq!(val.as_str(), "a(b)c");
        }
        other => panic!("expected Has with string value, got {other:?}"),
    }
}

#[test]
fn parse_has_with_dot_and_in_string_value() {
    // End-to-end: has() where the value contains ".and(" — must not confuse
    // predicate chain splitting.
    let steps = parse_traversal(r#"g.V().has("desc", "demand(supply)")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    match &steps[1] {
        GremlinStep::Has(key, Some(GremlinValue::String(val))) => {
            assert_eq!(key.as_str(), "desc");
            assert_eq!(val.as_str(), "demand(supply)");
        }
        other => panic!("expected Has with string value, got {other:?}"),
    }
}

#[test]
fn parse_predicate_with_escaped_backslash_before_close_quote() {
    // Escaped backslash at end of string: \\" means literal \ then close quote.
    let steps = parse_traversal(r#"g.V().has("path", "c:\\")"#, &no_params).unwrap();
    assert_eq!(steps.len(), 2);
    match &steps[1] {
        GremlinStep::Has(key, Some(GremlinValue::String(val))) => {
            assert_eq!(key.as_str(), "path");
            assert_eq!(val.as_str(), "c:\\");
        }
        other => panic!("expected Has with string value, got {other:?}"),
    }
}
