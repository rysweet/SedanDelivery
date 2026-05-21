# Gremlin Compiler Helpers — Test Coverage Reference

Unit test suite for the `gremlin-core::compiler::helpers` submodules, covering
the free functions that lower Gremlin predicates, values, steps, and
`where/by` modulators into `cypher-core` AST expressions and `Operator` nodes.

**Test file:** `gremlin-core/src/compiler/helpers_unit_tests.rs`
**Registered in:** `gremlin-core/src/compiler/mod.rs` (`#[cfg(test)] mod helpers_unit_tests;`)
**Issue:** [#1337](https://github.com/cloud-ecosystem-security/SedanDelivery/issues/1337)

## Quick start

```bash
# Run only the helpers unit tests
cargo test -p gremlin-core --lib -- helpers_unit_tests

# Run all gremlin-core lib tests (includes helpers)
cargo test -p gremlin-core --lib

# Check coverage for the helpers module
cargo llvm-cov -p gremlin-core --lib --html \
  --output-dir target/llvm-cov/html/gremlin-core
# Open target/llvm-cov/html/gremlin-core/html/index.html
```

## Architecture

The `compiler::helpers` module was split from a monolithic 1812-line
`helpers.rs` into 8 focused submodules. Each submodule contains `pub(in
crate::compiler)` functions that the main compiler step-handlers call during
traversal compilation. The test file exercises these functions directly,
bypassing the full parse → compile pipeline to reach branch combinations
that end-to-end tests do not exercise.

```text
compiler/helpers/
├── conversion.rs        # GremlinValue ↔ Expr / Value conversions
├── extract.rs           # by-traversal key, select label, math variable extraction
├── operators.rs         # Operator tree rewrites (sack, scan injection)
├── predicate_expr.rs    # Predicate → Expr lowering (property + null-aware)
├── predicate_label.rs   # Predicate → label-aware expressions (has/hasLabel)
├── predicates.rs        # GremlinStep sequences → Predicate construction
├── steps.rs             # Step-sequence analysis (path tracking, prescan, splitting)
└── where_by.rs          # where().by() modulator target extraction and rewriting
```

## Per-submodule coverage

### conversion (30 tests)

Covers `gremlin_value_to_expr`, `gremlin_value_to_id_expr`,
`gremlin_value_to_value`, `is_gremlin_id_key`, `gremlin_has_key_expr`,
and `make_by_expr`.

| Function | What's tested |
|----------|---------------|
| `gremlin_value_to_expr` | All 10 `GremlinValue` variants: `Null`, `Bool`, `Integer`, `Float`, `String`, `List`, `Map`, `Vertex`, `Edge`, `Path` |
| `gremlin_value_to_id_expr` | Numeric string coercion (`"42"` → `Expr::Integer(42)`), non-numeric passthrough, non-string passthrough |
| `gremlin_value_to_value` | Round-trip value fidelity for all variants |
| `is_gremlin_id_key` | `"~id"`, `"T.id"`, `"t[id]"` → true; other keys → false |
| `gremlin_has_key_expr` | Property key access expression construction |
| `make_by_expr` | All 8 branches: identity, property key, label, id, traversal, string functions (toUpper/toLower), unfold-values-fold, in-values-agg |

### extract (18 tests)

Covers `extract_by_traversal_key`, `extract_select_traversal_label`,
and `extract_math_variables`.

| Function | What's tested |
|----------|---------------|
| `extract_by_traversal_key` | All 12 match arms: `Values(["k"])`, `Label`, `Id`, `Select(_, labels)`, `Values+ToUpper`, `Values+ToLower`, `Unfold+Values+Fold`, `In+Values+Min`, `In+Values+Max`, `In+Values+Sum`, `In+Values+Count`, unrecognized patterns |
| `extract_select_traversal_label` | Single-label extraction, multi-label rejection |
| `extract_math_variables` | Underscore default (`_`), named variables (`a + b`), duplicate deduplication, mixed expressions |

### predicate_expr (36 tests)

Covers `predicate_to_expr`, `predicate_to_prop_expr`, and
`predicate_to_args`.

| Function | What's tested |
|----------|---------------|
| `predicate_to_expr` | Null-aware branches for all 6 core ops (Eq, Neq, Gt, Gte, Lt, Lte): null RHS produces `IsNull`/`IsNotNull`, non-null RHS produces `BinaryOp` |
| `predicate_to_prop_expr` | All 15 Predicate variants: Eq/Neq/Gt/Gte/Lt/Lte/Within/Without/Between/Inside/Containing/StartingWith/EndingWith/Regex + compound And/Or/Not |
| `predicate_to_args` | Operator-name + value-expr pairs for all/any/none step integration |

### predicate_label (35 tests)

Covers `predicate_string_ref`, `predicate_to_binary_op`,
`predicate_has_label_refs`, `predicate_label_ref`, and
`predicate_to_label_aware_expr`.

| Function | What's tested |
|----------|---------------|
| `predicate_string_ref` | String value extraction from `Eq(String)` and non-string rejection |
| `predicate_to_binary_op` | All predicate variants → `BinaryOp` mapping (Eq→Eq, Neq→Neq, Gt→Gt, Gte→Gte, Lt→Lt, Lte→Lte) |
| `predicate_has_label_refs` | Multi-label extraction from `Within([String, String, ...])` |
| `predicate_label_ref` | Single label from `Eq`, list from `Within`, rejection of non-string predicates |
| `predicate_to_label_aware_expr` | Full label-aware expression construction: single label, multiple labels (OR chain), predicate with label context |

### predicates (25 tests)

Covers `step_to_predicate` and `steps_to_predicate`.

| Function | What's tested |
|----------|---------------|
| `step_to_predicate` | All step variants that produce predicates: `Is(pred)`, `Has(key, Some(pred))`, `Has(key, None)`, `HasLabel(labels)`, `HasKey(keys)`, `HasValue(pred)`, `HasId(ids)`, unrecognized step → `None` |
| `steps_to_predicate` | Single-step conversion, two-step `And` combination, multi-step chaining, label reference extraction from predicate chains |

### steps (30 tests)

Covers step-sequence analysis and preprocessing utilities.

| Function | What's tested |
|----------|---------------|
| `is_simple_predicate_steps` | Simple steps (Has, HasLabel, HasId, HasKey, HasValue, Is, Not, And, Or) → true; complex steps (Out, Values, Count) → false |
| `steps_need_path_tracking` | Path/SimplePath/CyclicPath trigger tracking; non-path steps do not |
| `steps_need_label_stacks` | `Select` with `Pop.first`/`Pop.all`/`Pop.mixed` triggers stacks; `Pop.last` and no-pop do not |
| `collect_aggregate_keys_prescan` | `Aggregate("x")` and `GroupCount("g")` keys are collected; `Cap("x")` consumes |
| `split_at_empty_connective` | Splits step sequences at bare `And`/`Or` connectives; preserves steps when no split point exists |
| `preprocess_infix_connectives` | Rewrites infix `And`/`Or` markers into compound predicate steps |
| `split_trailing_agg` | Splits trailing aggregation steps (`Count`, `Sum`, `Min`, `Max`, `Mean`, `Fold`) from the main step sequence |
| `split_trailing_group_value_agg` | Separates trailing group-value aggregation patterns (`by(count())`, `by(sum)`) |

### operators (14 tests)

Covers `Operator` tree rewrites for sack injection, scan insertion, alias
rewriting, and sack-by modulation.

| Function | What's tested |
|----------|---------------|
| `inject_sack_init` | Wraps an operator with sack initialization (constant value, `sum`/`assign` operators) |
| `inject_scan_into_child` | Injects `ScanNodes`/`ScanEdges` into the child position of filter/project operators |
| `rewrite_create_alias` | Replaces placeholder alias in `Create`/`MergeRelationship`/`Apply`/`Set` operators; recursive child traversal |
| `rewrite_sack_by` | Rewrites `sack(op).by(key)` patterns into the appropriate operator tree shape |

### where_by (22 tests)

Covers `where().by()` modulator target extraction and expression rewriting.

| Function | What's tested |
|----------|---------------|
| `WhereByTarget` | Value-key and label-key target construction; null-equality wrapping |
| `extract_where_by_targets` | Single-by and dual-by target extraction from step sequences following a `Where` step |
| `collect_predicate_labels_in_order` | Ordered label collection from predicate trees (respects traversal order) |
| `wrap_where_by_null_eq` | Null-safe equality wrapping: `prop(a) = prop(b)` → `CASE WHEN prop(a) IS NULL AND prop(b) IS NULL THEN TRUE WHEN prop(a) = prop(b) THEN TRUE ELSE FALSE END` |
| `finalize_where_by_modulation` | Full where-by pipeline: target extraction → expression rewriting → null wrapping → operator tree update |
| `rewrite_where_by_expr` | Expression tree rewriting with target substitution |
| `rewrite_where_by_target` | Identifier substitution within `WhereByTarget` nodes |

## Test design principles

1. **Direct function calls, not end-to-end.** Tests call helper functions
   directly using constructed `GremlinStep` slices, `Predicate` values, and
   `GremlinValue` instances. This makes tests fast, deterministic, and
   independent of the parser.

2. **All branches, not just happy paths.** Each `match` arm and `if/else`
   branch in the helpers is exercised by at least one test. Null-aware
   predicates, empty lists, unknown step types, and boundary values are
   all covered.

3. **Structural assertions.** Tests assert on the shape of the returned
   `Expr` or `Operator` tree (variant, key fields) rather than exact
   `Debug` string equality. This makes tests resilient to internal field
   additions in `cypher-core` AST types.

4. **No I/O.** All tests use in-memory data only. No file system, no
   network, no `GraphStore`. Each test runs in microseconds.

5. **One concern per test.** Each `#[test]` function exercises one function
   with one specific input pattern, making failures easy to diagnose.

## Relationship to other test modules

| Module | Approach | What it covers |
|--------|----------|----------------|
| `helpers_unit_tests.rs` | Direct function calls | All 8 helpers submodules at the function level |
| `helpers_tests.rs` | End-to-end (parse → compile) | Helpers exercised indirectly through representative traversals |
| `tests.rs` | End-to-end (parse → compile) | Core compiler step-handlers |
| `coverage_tests{1-6}.rs` | End-to-end (parse → compile) | Targeted traversal patterns for coverage gaps in other compiler modules |
| `by_tests.rs` / `filter_tests.rs` / `strings_tests.rs` | End-to-end | Step-specific compiler behavior |

The helpers unit tests complement (not replace) the end-to-end tests. The
end-to-end tests validate that the compiler produces correct operator trees
for real Gremlin traversals. The unit tests validate that each helper
function handles all its input variants correctly in isolation.

## Adding new tests

When adding a new helper function or extending an existing one:

1. Add the function to the appropriate submodule in `compiler/helpers/`.
2. Add tests to the corresponding section in `helpers_unit_tests.rs`.
3. Follow the naming convention: `test_<submodule>_<function>_<variant>`.
4. Run the full suite to verify no regressions:

```bash
cargo test -p gremlin-core --lib -- helpers_unit_tests
cargo clippy -p gremlin-core -- -D warnings
```

5. Check coverage improvement:

```bash
cargo llvm-cov -p gremlin-core --lib --summary-only
```

## Coverage impact

Before #1337, the `compiler::helpers` submodules averaged ~21% line coverage.
After adding ~210 direct unit tests, the helpers submodules individually
exceed 70% line coverage, and the overall `gremlin-core` crate coverage
rises from **79.20%** to approximately **82%**.

| Submodule | Pre-#1337 | Post-#1337 | Tests added |
|-----------|----------:|----------:|------------:|
| `conversion.rs` | ~21% | ~80% | 30 |
| `extract.rs` | ~25% | ~85% | 18 |
| `predicate_expr.rs` | ~25% | ~75% | 36 |
| `predicate_label.rs` | 19.58% | ~75% | 35 |
| `predicates.rs` | ~30% | ~80% | 25 |
| `steps.rs` | ~35% | ~85% | 30 |
| `operators.rs` | ~20% | ~70% | 14 |
| `where_by.rs` | ~20% | ~70% | 22 |
| **Total** | **~21%** | **~77%** | **210** |
