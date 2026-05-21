# Gremlin Executor Module Map

Internal reference for the `cypher-core::executor::engine::gremlin` module tree.

## Module Layout

```
gremlin/
├── mod.rs              # Top-level module declarations
├── helpers/            # Shared helpers, all re-exported as pub(super)
│   ├── mod.rs          # Re-exports from submodules
│   ├── operator.rs     # Operator tree inspection (barrier detection, expand)
│   ├── predicate.rs    # P.* predicate parsing/evaluation, option arg parsing
│   ├── random.rs       # JavaCompatibleRandom RNG (sample/coin steps)
│   └── value_format.rs # Value→string conversion, map entry merging
├── branch/             # Branch steps (choose, coalesce)
│   ├── mod.rs
│   ├── choose.rs       # exec_choose, exec_choose_map
│   └── coalesce.rs     # exec_coalesce
├── group.rs            # group() step
├── path/               # Path tracking steps
├── projection/         # Projection steps (select values, properties)
├── repeat/             # repeat() loop step
├── select/             # select() step
├── select_helpers.rs   # Shared select utilities
├── side_effect/        # Side-effect steps
│   ├── mod.rs
│   ├── aggregate.rs    # aggregate() / store()
│   ├── cap.rs          # cap() — emit accumulated side-effects
│   └── passthrough.rs  # sideEffect() — run traversal, pass rows through
└── tests.rs            # Gremlin executor unit tests
```

## `branch/` Submodule

Decomposed from the former monolithic `branch.rs` (issue #1796).
Follows the same `mod.rs` + per-step file pattern used by `side_effect/`.

### `choose.rs`

Two `pub(crate)` entry points and one private helper:

| Method | Visibility | Operator | Purpose |
|--------|-----------|----------|---------|
| `exec_choose` | `pub(crate)` | `Choose` | Predicate-based if/then/else dispatch |
| `exec_choose_map` | `pub(crate)` | `ChooseMap` | Value-based multi-branch dispatch with `Pick.any`/`Pick.unproductive` |
| `eval_choose_map_traversal_option` | private | — | Evaluate `__.identity()`, `__.hasLabel()`, `__.has()` tokens against a value |

Both entry points use **batch execution**: all input rows are classified first (Phase 1), then each branch is executed once with its collected rows (Phase 2+). This allows aggregating steps (`fold`, `count`, `limit`) to operate on the full branch stream.

Imports `super::super::helpers::*` — the glob brings in all `pub(super)` helpers; `#[allow(unused_imports)]` suppresses warnings for the unused subset. Functions actually called: `eval_gremlin_predicate`, `parse_gremlin_option_args`, `trim_gremlin_option_string`, `parse_gremlin_option_value`.

### `coalesce.rs`

| Method | Visibility | Operator | Purpose |
|--------|-----------|----------|---------|
| `exec_coalesce` | `pub(crate)` | `Coalesce` | Try alternatives in order; return first non-empty result |

Uses **per-row evaluation**: each input row is tested against alternatives individually, returning results from the first non-empty alternative. Rows with no matching alternative are dropped (standard Gremlin semantics).

Does **not** import the helpers glob — no helper functions are needed.

## Import Conventions

All helpers are re-exported as `pub(super)` from `helpers/mod.rs`, making them visible within the `gremlin/` tree but not outside it. Submodules two levels deep (e.g., `branch/choose.rs`) reach them via `super::super::helpers::*`; one-level files (e.g., `group.rs`) use `super::helpers::*`.

## Adding a New Branch Step

1. Create `branch/<step_name>.rs` with an `impl<'a> QueryExecutor<'a>` block
2. Add `mod <step_name>;` to `branch/mod.rs` with a doc comment entry
3. Wire the new `Operator` variant dispatch in `exec_operator()` (in the main executor)
4. Run `cargo clippy -p cypher-core -- -D warnings` and `cargo test -p cypher-core --lib`
