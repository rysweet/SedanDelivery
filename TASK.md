# Issue #39

You are working on SedanDelivery, a Rust GQL/Cypher query engine over Redis/Valkey. Current TCK compliance: 3149/3806 (82.7%). Your goal is to fix ~70 TCK row count failures.

Parent issue: #35 (cloud-ecosystem-security/SedanDelivery)

IMPORTANT: Branch from main. Create branch feat/tck-r5-rowcount. Make all fixes, verify improvement, create PR.

FIRST: Run this to understand the failures:
```bash
cargo test --test tck 2>&1 | grep 'Expected.*rows\|row count\|expected.*found' | head -20
cargo test --test tck 2>&1 | grep 'Step panicked' | sed 's/.*: //' | sort | uniq -c | sort -rn | head -20
```

FIXES TO IMPLEMENT:

1. **collect() as first-class aggregate (~20 failures)**
   In gql-core/src/executor/filter.rs and/or engine.rs, ensure `collect()` works correctly as an aggregate function.
   TCK examples:
   ```cypher
   MATCH (n) RETURN collect(n.name) AS names
   MATCH (n) WITH n ORDER BY n.name RETURN collect(n.name) AS names
   ```
   - collect() should gather values into a list
   - collect(NULL) items should be skipped (NULL is not added to the list)
   - collect() over empty set should return empty list []
   Check the aggregation code path in engine.rs.

2. **percentileDisc() and percentileCont() functions (~10 failures)**
   In gql-core/src/executor/filter.rs, implement:
   - `percentileDisc(value, percentile)` - discrete percentile (nearest rank method)
   - `percentileCont(value, percentile)` - continuous percentile (linear interpolation)
   TCK examples:
   ```cypher
   MATCH (n) RETURN percentileDisc(n.age, 0.5) AS median
   MATCH (n) RETURN percentileCont(n.age, 0.5) AS median
   ```
   These are aggregate functions like avg(), max(), min().

3. **OPTIONAL MATCH edge cases (~20 failures)**
   In gql-core/src/executor/engine.rs, fix OPTIONAL MATCH to correctly generate NULL rows.
   TCK examples:
   ```cypher
   OPTIONAL MATCH (n)-[:KNOWS]->(m) RETURN n, m
   -- If no match, should return row with n=null, m=null
   MATCH (n) OPTIONAL MATCH (n)-[:KNOWS]->(m) RETURN n, m
   -- If no match for some n, should return n with m=null
   ```
   Key: OPTIONAL MATCH with no results should produce one NULL row per input row.

4. **Multi-pattern MATCH with shared variables (~20 failures)**
   In gql-core/src/executor/engine.rs, fix MATCH with multiple patterns sharing variables.
   TCK examples:
   ```cypher
   MATCH (a)-[:KNOWS]->(b), (b)-[:KNOWS]->(c) RETURN a, b, c
   -- b must be the same node in both patterns
   MATCH (a)-[r1]->(b)<-[r2]-(c) RETURN a, b, c
   -- b must be in the middle of both relationships
   ```
   The cross-product of matches should be filtered to only rows where shared variables have consistent values.

5. **stdev() and stdevp() functions**
   Implement standard deviation aggregates if not already done:
   - `stdev(x)` - sample standard deviation
   - `stdevp(x)` - population standard deviation

DEBUGGING APPROACH:
- Run the TCK and capture failures that mention row count mismatches
- Look for scenarios with different numbers of result rows than expected
- Focus on aggregation and MATCH pattern execution

AFTER IMPLEMENTING:
1. Run: `cargo test -p gql-core --lib` (all unit tests must pass)
2. Run: `cargo test -p gql-core --test end_to_end` (all integration tests must pass)
3. Run: `cargo test --test tck 2>&1 | tail -5` (should show improvement)
4. Run: `cargo clippy -p gql-core -- -D warnings` (no warnings)
5. Create PR with: how many TCK scenarios improved, from X to Y

Key files:
- gql-core/src/executor/engine.rs (~3500 lines)
- gql-core/src/executor/filter.rs (~2600 lines)
- gql-core/src/planner/planner.rs (~3100 lines)

Follow DEFAULT_WORKFLOW.md autonomously. NO QUESTIONS. Work through Steps 0-22. Create PR when complete.