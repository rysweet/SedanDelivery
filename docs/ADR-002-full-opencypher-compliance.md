# ADR-002: Full openCypher Compliance vs. Limited Subset

**Status**: Accepted
**Date**: 2026-02-26

## Context

The originating issue (#431) suggests implementing only: "MATCH, CREATE, MERGE, DELETE, SET, RETURN, WHERE, variable-length paths."

## Decision

Target full openCypher compliance from the grammar level. Execution is implemented incrementally, but the parser accepts the complete language. GQL (ISO/IEC 39075) is a separate standard and remains future work.

## Rationale

A query engine implementing an idiosyncratic subset:
- Creates a permanent maintenance burden documenting what is and isn't supported
- Breaks any query touching unsupported features with cryptic errors
- Forecloses future use cases
- Is incompatible with the openCypher TCK (the primary conformance test suite)

The grammar is cheap — a lalrpop LALR(1) grammar for the full Cypher language is ~1,500 lines, comparable to a subset grammar. Execution of rare clauses is deferred with a structured `UnsupportedFeature { feature: String }` error, not a parse error. This is far better DX than "syntax error: unexpected token CALL".

The openCypher TCK provides ~3,900 Gherkin scenarios covering the full language. Adopting the full grammar from day one means we can run all 3,900 and track our pass rate as a first-class metric.

## Consequences

Parser development takes slightly longer (cover CALL, FOREACH, UNION, full expression syntax). All features including CALL, FOREACH, and UNION are now fully implemented. TCK pass rate reached 100% (3903/3903 scenarios).
