# ADR-003: Parser Strategy — Hand-Written Recursive Descent Parser

**Status**: Superseded
**Date**: 2026-02-26

> **Note**: The original decision selected a lalrpop LALR(1) grammar combined with
> a hand-written lexer. During implementation, a hand-written recursive descent
> parser proved simpler to maintain, produced better error messages, and handled
> Cypher's syntax (including quantified path patterns) without requiring a build-step
> code generator. The lalrpop approach was never shipped.

## Context

Options evaluated:
1. `nom` — parser combinator
2. `pest` — PEG grammar
3. `lalrpop` — LALR(1) grammar generator
4. Hand-written recursive descent
5. `tree-sitter` — runtime parsing

## Decision

Hand-written lexer (zero-copy, `&'src str` token slices) combined with a hand-written recursive descent parser.

## Rationale

**Why recursive descent**: Cypher has operator precedence, left-recursive expressions (`n.a.b.c`), and complex pattern syntax. A hand-written Pratt parser handles precedence climbing naturally, produces precise error messages with span information, and requires no build-step code generation. The parser is straightforward to extend for GQL ISO 20030 features such as quantified path patterns.

**Why hand-written lexer**: A hand-written lexer returns `&'src str` slices from the original query string, enabling zero-copy token representation during parsing. This matters for query latency in a Redis module hot path.

**Why not nom**: Parser becomes unreadable at Cypher's scale; no native left-recursion support; error messages require significant extra work.

**Why not pest**: Allocates heavily via `Pairs` iterator; no typed AST — must manually traverse parse tree.

**Why not tree-sitter**: Runtime parsing library designed for editor integration, not embedded library use. Too large a dependency for a Redis module `.so`.

**Why not lalrpop**: Originally planned, but the build-step code generation added complexity, error messages were less precise than hand-written alternatives, and extending the grammar for GQL ISO 20030 quantified path patterns would have required GLR support that lalrpop does not provide.

## Consequences

No build-step code generation is required. The parser lives in `gql-core/src/parser/` as plain Rust. Lexer lifetime `'src` must be threaded carefully through parser functions.
