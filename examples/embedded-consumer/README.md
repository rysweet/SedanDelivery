# Embedded consumer example

A minimal end-to-end demo of using **all three** SedanDelivery embedded engines
from a single Rust process, sharing a single in-memory store:

- [`sedan-cypher-embedded`](../../sedan-cypher-embedded) — openCypher
- [`sedan-gql-embedded`](../../sedan-gql-embedded) — GQL (ISO/IEC 39075)
- [`sedan-gremlin-embedded`](../../sedan-gremlin-embedded) — Apache TinkerPop Gremlin

## Run

```bash
cd examples/embedded-consumer
cargo run
```

Expected output:

```
cypher: 1 row(s)
  Str("Alice") -> Str("Bob")
gql: 2 row(s)
  Str("Alice") (age Int(30))
  Str("Bob") (age Int(25))
gremlin: g.V().count() = Some(Int(2))
after cypher CREATE, gremlin sees Some(Int(3)) Person vertices
```

## What it shows

1. Create a single `InMemoryGraphStore` shared by all three engines.
2. Seed data through the storage trait (no parser involved).
3. Run a Cypher pattern query.
4. Run a GQL query.
5. Run a Gremlin traversal.
6. **Mutate via one engine, read via another** — the store is the single source of truth.

## Why a separate workspace?

The example uses `[workspace]` in its own `Cargo.toml` to demonstrate exactly
what a downstream consumer needs in *their* `Cargo.toml`: the three embedded
crates and `indexmap` (re-used for property maps). The main SedanDelivery
workspace `excludes` this directory so it doesn't pollute the main build, but
CI still builds & runs it from a separate job (see
`.github/workflows/embedded-consumer-example.yml`).

## Using in your own crate

In production you would depend on the published crates:

```toml
[dependencies]
sedan-cypher-embedded   = "0.1"
sedan-gql-embedded      = "0.1"
sedan-gremlin-embedded  = "0.1"
indexmap                = "2"
```

For in-repo development, this example uses `path = "..."` deps to always
build against the current workspace sources.
