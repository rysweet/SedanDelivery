# Fuzzing SedanDelivery Parsers

SedanDelivery ships a [`cargo-fuzz`](https://rust-fuzz.github.io/book/cargo-fuzz.html)
harness covering each of the three query language parsers:

| Target                | Crate          | Entry point                                 |
| --------------------- | -------------- | ------------------------------------------- |
| `cypher_parser`       | `cypher-core`  | `cypher_core::parser::parse_query`          |
| `gql_parser`          | `gql-engine`   | `gql_engine::parser::parse_gql`             |
| `gremlin_text_parser` | `gremlin-core` | `gremlin_core::text_parser::parse_traversal` |

The harness lives under `fuzz/` and is **deliberately excluded from the
Cargo workspace**. Pulling `libfuzzer-sys` into every workspace build would
force a nightly toolchain on contributors who never run fuzz campaigns.

## Contract under test

Every parser must satisfy:

> For any UTF-8 input, `parse(...)` returns `Result<_, ParseError>` — never
> panics, aborts, or runs indefinitely on bounded input.

A panic discovered by libFuzzer is a real bug. File an issue, attach the
input artifact, and treat the parser fix as blocking.

## Prerequisites

```bash
# Nightly Rust (required by cargo-fuzz / libFuzzer instrumentation)
rustup toolchain install nightly

# cargo-fuzz driver
cargo install cargo-fuzz
```

> **Stable Rust is not supported.** `libfuzzer-sys` relies on
> `-Zsanitizer=address` and the libFuzzer driver, both of which require the
> nightly compiler. The fuzz crate is isolated from the workspace specifically
> to keep stable builds working without nightly.

## Running a smoke campaign

The smoke script runs each target for 60 seconds and fails the script if any
target reports a crash:

```bash
bash scripts/fuzz_smoke.sh
```

Override duration or target list via environment variables:

```bash
FUZZ_TIME=300 bash scripts/fuzz_smoke.sh
FUZZ_TARGETS="cypher_parser" bash scripts/fuzz_smoke.sh
FUZZ_TOOLCHAIN=nightly-2026-04-01 bash scripts/fuzz_smoke.sh
```

## Running a targeted campaign

For deeper fuzzing of a single parser:

```bash
cd fuzz
cargo +nightly fuzz run cypher_parser -- \
    -max_total_time=3600 \
    -jobs=8 \
    -workers=8
```

Useful libFuzzer flags:

| Flag                       | Effect                                              |
| -------------------------- | --------------------------------------------------- |
| `-max_total_time=<sec>`    | Stop after N seconds                                |
| `-runs=<N>`                | Stop after N executions                             |
| `-jobs=<N> -workers=<N>`   | Run N parallel fuzzing processes                    |
| `-max_len=<bytes>`         | Cap individual input size (default 4096)            |
| `-dict=path/to/dict`       | Provide a token dictionary to bias mutations        |
| `-only_ascii=1`            | Restrict mutation alphabet to printable ASCII       |

## Corpus seeds

`fuzz/corpus/<target>/` contains hand-picked valid queries that bootstrap
libFuzzer's coverage. Add new seeds whenever you find an interesting input
class — reductions of crashes, regression cases, or queries from the TCK.

```bash
echo 'MATCH (n) RETURN count(*)' > fuzz/corpus/cypher_parser/new_seed
```

libFuzzer will incorporate the new seeds automatically on the next run and
will minimize / mutate them as it explores.

## Triaging a crash

When a campaign reports `ERROR: ...`, libFuzzer writes the offending input
to `fuzz/artifacts/<target>/crash-<sha>` and prints the path. To reproduce:

```bash
cd fuzz
cargo +nightly fuzz run cypher_parser artifacts/cypher_parser/crash-<sha>
```

To shrink the input to its minimal failing case:

```bash
cargo +nightly fuzz tmin cypher_parser artifacts/cypher_parser/crash-<sha>
```

Open a bug with the minimized input, the panic message, and the parser
location (use `RUST_BACKTRACE=1`).

## Known crash fixes

| Parser | Location | Input class | Fix |
| ------ | -------- | ----------- | --- |
| `gremlin_text_parser` | `collection.rs` numeric suffix | Multi-byte UTF-8 last char (e.g. `42é`) | `s.get(..s.len()-1)` replaces panicking `&s[..s.len()-1]` |

All crashes discovered by fuzzing are treated as blocking bugs. See
[Maintain CI Workflows](howto/maintain-ci-workflows.md#gremlin-text-parser-fuzz-safe-numeric-suffix-parsing)
for the full write-up of the `collection.rs` fix.

## CI integration

`scripts/fuzz_smoke.sh` is intended for local pre-merge use and for an
opt-in CI workflow. It is **not** wired into the default PR pipeline because
nightly Rust is not part of the standard CI image — see
`.github/workflows/fuzz-smoke.yml` for the opt-in workflow definition.
