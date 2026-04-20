# embedded-fuzz

Property-test / fuzz harness for the three embedded engines:

| Engine | Crate |
| --- | --- |
| openCypher | [`sedan-cypher-embedded`](../../sedan-cypher-embedded) |
| GQL | [`sedan-gql-embedded`](../../sedan-gql-embedded) |
| Gremlin | [`sedan-gremlin-embedded`](../../sedan-gremlin-embedded) |

## What it checks

The harness asserts **invariants**, not specific output values:

1. `execute`, `execute_with_parameters`, `execute_with_context`, and
   `execute_with_budget` never panic on bounded random input.
2. They never deadlock — every test relies on the engines' default
   execution budgets, which provide an upper bound on running time.
3. Every error returned is one of the public error variants
   (`GqlError::{Parse,Plan,Execution,Storage,Type}` or
   `GremlinError::{Parse,Compile,Execution}`). A non-matching variant
   would be a compile error; a panic instead of an error would be a
   test failure.
4. Parameter values that bind to a `RETURN $p AS p` round-trip
   without mutation.

Generators (in [`src/lib.rs`](src/lib.rs)) build syntactically-valid
queries from a small grammar — random `MATCH`/`RETURN`/`WHERE`/`LIMIT`
combinations for Cypher and GQL, random `g.V()`/`g.E()` chains with
`.has`/`.hasLabel`/`.values`/`.count`/`.limit` steps for Gremlin —
plus a "raw garbage" generator that exercises the parser with arbitrary
text.

All proptest runs use a pinned seed (`embedded_fuzz::FUZZ_SEED`) so
failures are reproducible across machines.

## Running

Fast (128 cases per property, runs in CI):

```bash
cargo test -p embedded-fuzz
```

Soak (10 000 cases per property, run on demand):

```bash
cargo test -p embedded-fuzz -- --ignored
```

## Reporting bugs

If a property fails:

1. Re-run with `RUST_BACKTRACE=1` to capture the panic / assertion site.
2. Capture the proptest minimisation output (printed automatically).
3. File an issue tagged `embedded-fuzz` with the failing input and
   reference it from the originating PR.
