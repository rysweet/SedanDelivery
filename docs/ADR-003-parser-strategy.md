# ADR-003: Parser Strategy — Hand-Written Lexer + lalrpop LALR(1) Grammar

**Status**: Accepted
**Date**: 2026-02-26

## Context

Options evaluated:
1. `nom` — parser combinator
2. `pest` — PEG grammar
3. `lalrpop` — LALR(1) grammar generator
4. Hand-written recursive descent
5. `tree-sitter` — runtime parsing

## Decision

Hand-written lexer (zero-copy, `&'src str` token slices) combined with a `lalrpop` LALR(1) grammar.

## Rationale

**Why lalrpop**: Cypher has operator precedence (requiring precedence climbing or LALR), left-recursive expressions (`n.a.b.c`), and complex pattern syntax. LALR(1) handles all of this correctly by construction. The grammar file is formal documentation. lalrpop generates typed Rust code directly from grammar actions.

**Why hand-written lexer**: lalrpop's built-in regex lexer allocates; a hand-written lexer returns `&'src str` slices from the original query string, enabling zero-copy token representation during parsing. This matters for query latency in a Redis module hot path.

**Why not nom**: Parser becomes unreadable at Cypher's scale; no native left-recursion support; error messages require significant extra work.

**Why not pest**: Allocates heavily via `Pairs` iterator; no typed AST — must manually traverse parse tree.

**Why not tree-sitter**: Runtime parsing library designed for editor integration, not embedded library use. Too large a dependency for a Redis module `.so`.

**Reserve**: If lalrpop proves insufficient for GQL ISO 20030 extensions (particularly quantified path patterns which may require GLR), fall back to a hand-written Pratt parser for expressions + recursive descent for clauses.

## Consequences

Build step runs lalrpop code generation. Grammar file in `gql-core/src/parser/cypher.lalrpop` serves as the canonical language specification. Lexer lifetime `'src` must be threaded carefully through parser actions.
