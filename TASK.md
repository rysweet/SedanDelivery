# Issue #101

You are working on SedanDelivery, a Rust GQL/Cypher query engine. Current TCK: 3526/3806 (92.7%). Fix temporal timezone and display bugs — the single largest remaining category (~136 failures in Temporal1-10 features).

Parent issue: #35. Branch from main: feat/tck-r7-temporal. Implement fixes, verify, create PR.

FIRST, understand the failures:
```bash
cargo test --test tck 2>&1 | grep -B5 'Expected row.*not found' | grep 'Temporal' | head -5
cargo test --test tck 2>&1 | grep -A5 'Temporal3' | grep 'Expected:\|Got:' | head -20
```

KEY FAILURE PATTERNS:

1. **Timezone offset not preserved in datetime construction** (~large, maybe 60+ failures)
   Expected: `1984-10-11T12:31:14.645876+01:00`
   Got: `1984-10-11T12:31:14.645876+00:00[UTC]`
   
   Also:
   Expected: `1984-10-11T16:31:14.645876+05:00`
   Got: `1984-10-11T12:31:14.645876+05:00[+05:00]`
   
   Issues:
   a. When constructing datetime with a timezone, the time component is being converted to UTC instead of keeping the original time in the given timezone
   b. The timezone name like `[UTC]` or `[+05:00]` should not appear in the output — just the offset `+05:00`
   c. The `[Pacific/Honolulu]` named timezone SHOULD appear: `2015-07-21T21:40:32.142+02:00[Europe/Berlin]`
   
   In gql-core/src/executor/filter.rs, find where datetime is constructed with timezone:
   - When `datetime({time: ..., timezone: '+01:00'})` is called, keep the time as-is, just apply the timezone offset to the OFFSET field, don't convert the time
   - The display format should be: `YYYY-MM-DDTHH:MM:SS.nnnnnnnnn+HH:MM` (no brackets unless named timezone)

2. **Duration with T designator but no time units** (~5 failures)
   Error: 'found a time designator T but did not find any time units'
   Example: `duration('PT')` or similar. The T should be optional.
   Fix in filter.rs: If T is found but no time units follow, that's valid (0 time units)

3. **localdatetime month out of range** (~2 failures)
   Error: 'month is not in the required range of 1..=12'
   This comes from ISO week dates being passed to localdatetime.
   Example: `localdatetime('2015-W30-2T19:32:24')`
   Fix: Detect week date format (contains W after year) and convert week+day to month+day.

4. **Temporal component selection** - date/time/datetime truncation functions
   Check: `date.truncate()`, `time.truncate()`, `localdatetime.truncate()`
   Check: `datetime.epochMillis`, `datetime.epochSeconds`
   Check: Temporal arithmetic: `date + duration`

5. **Temporal display format cleanup**
   - `datetime` should display as: `2015-07-21T21:40:32.142000000+00:00` (NOT `+00:00[UTC]`)
   - Named timezone: `2015-07-21T21:40:32.142+02:00[Europe/Berlin]` (with brackets)
   - `date` should display as: `2015-07-21` (ISO 8601)
   - `time` should display as: `21:40:32.142Z` or `21:40:32.142+02:00`
   - `localdatetime` should display as: `2015-07-21T21:40:32.142`
   - `duration` should display as: `P14DT16H12M` (ISO 8601)

6. **Temporal arithmetic**
   Check: `date('2015-01-01') + duration('P1Y1M') = date('2016-02-01')`
   Check: `datetime - datetime = duration`
   Check: `duration * 2`
   Check: `duration / 2`

Look at the Temporal feature files:
```bash
ls tests/tck/features/expressions/temporal/
cat tests/tck/features/expressions/temporal/Temporal3.feature | head -100
```

To run just temporal tests:
```bash
cargo test --test tck -- --scenario-name 'temporal' 2>&1 | tail -5
# or run individual features by looking at test names
cargo test --test tck 'Temporal' 2>&1 | tail -5
```

AFTER FIXING:
1. cargo test -p gql-core --lib
2. cargo test -p gql-core --test end_to_end
3. cargo test --test tck 2>&1 | tail -5
4. cargo clippy -p gql-core -- -D warnings
5. Create PR with count of scenarios improved

Key files: gql-core/src/executor/filter.rs (~2600 lines), gql-core/src/types/value.rs

Follow DEFAULT_WORKFLOW.md autonomously. NO QUESTIONS. Work through Steps 0-22. Create PR when complete.