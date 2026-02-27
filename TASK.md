# Issue #134

You are working on SedanDelivery, a Rust GQL/Cypher query engine. Current TCK: 3719/3806 (97.7%). Fix non-temporal remaining failures.

Branch from main: feat/tck-r9-engine-fixes. Create PR when done.

REMAINING FAILURES:

1. Integer multiply overflow (~4 failures)
'attempt to multiply with overflow' — Rust panic in arithmetic ops.
Fix in filter.rs/engine.rs: Replace `*` with `i64::checked_mul()` or `i64::saturating_mul()` for all integer arithmetic. Also handle `+`, `-`, `/` with checked variants.
```bash
cargo test --test tck 2>&1 | grep -B20 'multiply with overflow' | grep 'query\|Defined:\|Scenario' | head -10
```

2. Row count 15→16 (~4 failures, same scenario running 4 times)
```bash
cargo test --test tck 2>&1 | grep -B20 'expected 15 rows, got 16' | grep 'Defined:\|Scenario' | head -5
```
Probably a Match6 (named paths) issue. Look at the failing query and fix.

3. Row count 1→0 (~2 failures)
Query returns 0 rows when it should return 1.
```bash
cargo test --test tck 2>&1 | grep -B20 'expected 1 rows, got 0' | grep 'Defined:\|query' | head -10
```

4. Row count 1→2 (~1 failure)
```bash
cargo test --test tck 2>&1 | grep -B20 'expected 1 rows, got 2' | grep 'Defined:\|query' | head -10
```

5. +nodes expected 10 got 4 (~1 failure)
CREATE not creating all expected nodes.

6. Row #0/#1 does not match (~4 failures)
Values returned but in wrong order or with wrong content.
```bash
cargo test --test tck 2>&1 | grep -B20 'Row #0 does not match\|Row #1 does not match' | grep 'Defined:' | head -10
```

7. Expected row #2 not found (~2 failures)
Specific rows missing from result set.

8. TypeError expected but succeeded (~1 failure)
```bash
cargo test --test tck 2>&1 | grep -B20 'TypeError' | head -30
```

For each failure:
1. Find the exact failing scenario in the .feature file
2. Read the query and expected results
3. Trace through engine.rs to find why the result is wrong
4. Fix the root cause
5. Verify with cargo test -p gql-core --lib

AFTER FIXING:
1. cargo test -p gql-core --lib (308 pass)
2. cargo clippy -p gql-core -- -D warnings (clean)
3. cargo test --test tck 2>&1 | tail -5 (improvement)
4. Create PR

Key files: gql-core/src/executor/engine.rs, gql-core/src/executor/filter.rs, gql-core/src/parser/parser.rs

Follow DEFAULT_WORKFLOW.md autonomously. NO QUESTIONS. Work through Steps 0-22. Create PR when complete.