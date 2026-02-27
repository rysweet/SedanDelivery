# Issue #105

You are working on SedanDelivery, a Rust GQL/Cypher query engine. Current TCK: 3526/3806 (92.7%). Fix miscellaneous remaining failures across TypeConversion, Precedence, Return, and OrderBy features.

Parent issue: #35. Branch from main: feat/tck-r7-misc. Implement fixes, verify, create PR.

FIRST, enumerate remaining non-temporal failures:
```bash
cargo test --test tck 2>&1 | grep -B2 'Expected row.*not found\|Row.*does not match' | grep 'Defined:' | grep -v 'Temporal\|temporal' | awk '{print $2}' | cut -d: -f1 | sort | uniq -c | sort -rn | head -20
```

FAILURE 1: TypeConversion4 (~3 failures), TypeConversion2 (~2 failures)
Run: cargo test --test tck 2>&1 | grep -A30 'TypeConversion' | head -80
Look at tests/tck/features/expressions/typeConversion/
Common issues:
- toString(date) should produce ISO 8601 string
- toFloat(null) should return null (not error)
- toInteger(3.0) = 3
- toBooleanOrNull, toIntegerOrNull, toFloatOrNull functions
Fix in filter.rs: Add missing type conversion functions or fix edge cases.

FAILURE 2: Precedence2 (~2 failures)
Run: cargo test --test tck 2>&1 | grep -A30 'Precedence2' | head -60
Look at tests/tck/features/expressions/precedence/Precedence2.feature
Issue likely: Operator precedence wrong in parser (AND vs OR vs NOT vs comparison)
Fix in parser.rs: Verify precedence: NOT > AND > OR, comparison > logical

FAILURE 3: Return4 (~3 failures)
Run: cargo test --test tck 2>&1 | grep -A30 'Return4' | head -80
Look at tests/tck/features/clauses/return/Return4.feature
Common return clause issues:
- RETURN * with ORDER BY
- RETURN with SKIP and LIMIT combined
- RETURN DISTINCT edge cases
Fix in engine.rs/planner.rs: Check RETURN clause handling.

FAILURE 4: WithOrderBy1/2 (~2+2 failures)
Run: cargo test --test tck 2>&1 | grep -A30 'WithOrderBy' | head -80
Look at tests/tck/features/clauses/with-orderBy/
Issues:
- ORDER BY after WITH not matching
- NULL ordering (nulls first or last?)
- ORDER BY expression evaluation
Fix in engine.rs: Check sort/order implementation.

FAILURE 5: Match8 (~2 failures)
Run: cargo test --test tck 2>&1 | grep -A30 'Match8' | head -60
Look at tests/tck/features/clauses/match/Match8.feature

FAILURE 6: Graph6 (~2 failures)
Run: cargo test --test tck 2>&1 | grep -A30 'Graph6' | head -60
Check graph functions: labels(), keys(), properties(), nodes(), relationships(), id()

FAILURE 7: Aggregation2/8 (~2+2 failures not already handled)
Check count(DISTINCT expr), avg(expr), sum(expr) edge cases.

FAILURE 8: count() function name issue
Expected column name: `6` (just the value)
Got column name: `count(*): 2` (function expression as name)
This suggests the result comparison in runner.rs needs to handle anonymous columns differently.
Check runner.rs: When expected value is just a number without column name, compare values only.

For each failure:
1. Get the specific failing query from the feature file
2. Run it manually (using the test infrastructure)
3. Trace through the code to find the bug
4. Fix it

AFTER FIXING:
1. cargo test -p gql-core --lib
2. cargo test -p gql-core --test end_to_end
3. cargo test --test tck 2>&1 | tail -5
4. cargo clippy -p gql-core -- -D warnings
5. Create PR with count of scenarios improved

Key files: gql-core/src/executor/engine.rs, gql-core/src/executor/filter.rs, gql-core/tests/tck/runner.rs, gql-core/src/parser/parser.rs, gql-core/src/planner/planner.rs

Follow DEFAULT_WORKFLOW.md autonomously. NO QUESTIONS. Work through Steps 0-22. Create PR when complete.