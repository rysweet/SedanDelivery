# Issue #133

You are working on SedanDelivery, a Rust GQL/Cypher query engine. Current TCK: 3719/3806 (97.7%). Fix all remaining temporal failures — 56 'Expected row not found' (mostly Temporal5/6/7/8/10) + 9 datetime parse errors.

Branch from main: feat/tck-r9-temporal-deep. Create PR when done.

DIAGNOSTICS - run these first:
```bash
cargo test --test tck 2>&1 | grep -B10 'Expected row #0 not found' | grep -B1 'Expected:\|Got:' | grep -v 'Step\|Defined\|Matched' | head -60
cargo test --test tck 2>&1 | grep 'cannot parse datetime\|cannot parse date\|cannot parse civil' | head -10
```

ROMAINING TEMPORAL FAILURES BY FEATURE:
- Temporal5 (~6): Duration accessors (.years, .months, .days, .hours, .minutes, .seconds, .monthsOfYear, .daysOfMonth)
- Temporal6 (~8): Duration between two temporal values (duration.between)
- Temporal7 (~1): Temporal truncation (date.truncate('week'), datetime.truncate('hour'))
- Temporal8 (~14): Temporal component access (.year, .month, .day, .hour, .epochMillis, .epochSeconds)
- Temporal10 (~10): Arithmetic with temporals and durations
- Temporal2 (~1): Other

COMPACT DATETIME FORMATS TO FIX (~9 parse errors):
1. '2015T214032-0100' — year + T + compact time (HHMMSS) + compact offset (-HHMM)
2. '2015202T21+18:00' — year + ordinal day + T + time + offset
3. '2015-W30T2140-02' — week date + compact time + compact offset (-HH)
4. '2015-W30T2140-00:00' — week date + compact time + offset
5. '2015-W30-2T214032.142Z' — week-day + compact time with fractional seconds
6. '2015-202T21:40:32+01:00' — ordinal date + time
7. '-999999999-01-01' — extreme negative year (return error, not panic)
8. 'duration from date string' — return error for duration('2012-02-02')
9. Zulu offset in civil time — strip Z for localdatetime parsing

IMPLEMENTATION APPROACH:
For compact datetime formats, add a preprocessing step in filter.rs before calling jiff::parse:
```rust
fn normalize_datetime_input(s: &str) -> String {
    // 1. Expand compact time HHMMSS to HH:MM:SS
    // 2. Expand compact offset -HHMM to -HH:MM and -HH to -HH:00
    // 3. Expand ordinal date YYYYDDD to proper date
    // 4. Handle Zulu in localdatetime
}
```

For wrong temporal values, the likely issues are:
- Duration accessor (.years etc) not extracting correct component
- Duration.between not using correct algorithm
- Temporal truncation rounding incorrectly
- epochMillis/epochSeconds computation wrong
- Temporal arithmetic with duration applying months incorrectly

KEY: Read the failing test cases in the .feature files to understand exact expected behavior.

AFTER FIXING:
1. cargo test -p gql-core --lib (308 pass)
2. cargo clippy -p gql-core -- -D warnings (clean)
3. cargo test --test tck 2>&1 | tail -5 (improvement)
4. Create PR

Key files: gql-core/src/executor/filter.rs, gql-core/src/types/value.rs

Follow DEFAULT_WORKFLOW.md autonomously. NO QUESTIONS. Work through Steps 0-22. Create PR when complete.