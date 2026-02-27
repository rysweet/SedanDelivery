# Mutation Testing Workflow

## Overview

This workflow implements automated mutation testing for the CyberGym project using Stryker.NET. Mutation testing validates the **quality and effectiveness** of the test suite by introducing controlled code changes (mutations) and verifying that tests detect them.

## What is Mutation Testing?

Mutation testing answers the question: **"How good are my tests at catching bugs?"**

### The Process

1. **Mutate**: Stryker.NET introduces small code changes (mutations)
   - Example: Change `>` to `>=` in a conditional
   - Example: Remove a null check
   - Example: Change `+` to `-` in arithmetic

2. **Test**: Run the test suite against the mutated code

3. **Evaluate**:
   - **Killed mutation** ✅: Tests failed (detected the bug) - GOOD
   - **Survived mutation** ❌: Tests passed (bug went undetected) - BAD

4. **Score**: Calculate mutation score = (Killed / Total) × 100%

### Why It Matters

**Code coverage** tells you **what code** was executed during tests.
**Mutation testing** tells you **how well** your tests validate that code.

Example:
```csharp
// Code
public bool IsPositive(int value) => value > 0;

// Weak test (100% coverage, but...)
[Test]
public void IsPositive_Returns_True() {
    Assert.True(IsPositive(5)); // Only tests one case
}
```

Mutations that would survive:
- `value > 0` → `value >= 0` (boundary condition untested)
- `value > 0` → `value != 0` (negative values untested)

**Strong test suite** would kill these mutations:
```csharp
[Test] public void IsPositive_ZeroReturnsFalse() => Assert.False(IsPositive(0));
[Test] public void IsPositive_PositiveReturnsTrue() => Assert.True(IsPositive(1));
[Test] public void IsPositive_NegativeReturnsFalse() => Assert.False(IsPositive(-1));
```

## Workflow Configuration

### Schedule

- **Frequency**: Weekly (Sunday at 4 AM UTC)
- **Trigger**: Manual dispatch also available
- **Rationale**: Compute-intensive (10-100x longer than normal tests), run during off-peak hours

### Target Mutation Score

**Minimum acceptable**: **80%**

This is an industry-standard threshold for high-quality test suites:
- **<60%**: Critical - Test suite quality is poor
- **60-70%**: Needs improvement - Significant gaps in test validation
- **70-80%**: Acceptable - Good baseline, room for improvement
- **80-90%**: Good - High-quality test suite
- **>90%**: Excellent - Exceptional test suite quality

### Prerequisites

**CRITICAL**: This workflow requires **≥80% code coverage** as a foundation.

Mutation testing validates test quality, but requires sufficient test presence first. If code coverage <80%, the workflow defers mutation testing and recommends focusing on coverage improvement.

## Configuration Files

### Stryker Config: `tests/CyberGym.Catalog.Tests/stryker-config.json`

```json
{
  "stryker-config": {
    "mutate": ["**/*.cs", "!**/*Tests.cs"],
    "concurrency": 4,
    "timeout": 20000,
    "thresholds": {
      "high": 80,
      "low": 60,
      "break": 50
    }
  }
}
```

**Key Settings**:
- **concurrency**: 4 parallel mutants (balance speed vs. resource usage)
- **timeout**: 20 seconds (10x typical test execution time)
- **thresholds**: 80% high / 60% low / 50% break

### Exclusions

Code excluded from mutation testing:
- Test files (`*Tests.cs`)
- Generated code (`*.g.cs`, `*.designer.cs`)
- Migrations (`**/Migrations/**`)
- Startup/Program.cs (infrastructure code)
- Boilerplate methods (Equals, GetHashCode, ToString)

### Mutation Types

Stryker.NET supports various mutation types:
- **Arithmetic**: `+` ↔ `-`, `*` ↔ `/`
- **Logical**: `&&` ↔ `||`, `!` removed
- **Comparison**: `>` ↔ `>=`, `==` ↔ `!=`
- **Conditional**: `if (x)` ↔ `if (!x)`
- **Statement**: Remove statements, change return values

## Outputs

### Weekly Report Issue

Created with label `[mutation]`, auto-closes previous week's report.

**Contents**:
- Current mutation score and status
- Trend analysis (last 8 weeks)
- Mutation score by project area
- Critical survived mutations with recommendations
- Compute cost tracking
- AI-powered test improvement suggestions

### Regression Alert Issue

Created when mutation score drops >5% week-over-week.

**Contents**:
- Regression details (previous vs. current)
- New survived mutations
- Analysis of recent code changes
- High-priority action items
- Merge blocker recommendations

### Historical Data

Stored in `repo-memory` (branch: `memory/mutation`):
- Mutation score trends
- Detailed metrics (killed, survived, timeout, no-coverage)
- Runtime metrics (compute cost)
- 12 weeks of history retained

## Compute Cost

### Runtime Expectations

Mutation testing is **extremely compute-intensive**:
- **Small project** (500 mutations): 15-30 minutes
- **Medium project** (1000 mutations): 30-60 minutes
- **Large project** (2000+ mutations): 1-2 hours

**Factors affecting runtime**:
- Number of mutations (proportional to code size)
- Test suite execution time (multiplied per mutation)
- Concurrency level (higher = faster, more resources)
- Timeout configuration (higher = longer for slow tests)

### Cost Optimization

1. **Start small**: Test critical modules first, expand gradually
2. **Increase concurrency**: If CI runners support it (trade-off: resource usage)
3. **Optimize tests**: Faster tests = faster mutation testing
4. **Exclude low-value code**: Skip generated code, simple DTOs
5. **Run selectively**: Weekly schedule, not on every commit

### CI/CD Integration

**Current approach**: Weekly batch run (off-peak hours)

**Future consideration**: Incremental mutation testing in CI
- Only mutate changed files in PRs
- Faster feedback loop
- Gate merges on mutation score

## Interpreting Results

### Survived Mutations - What to Do?

When a mutation survives, it indicates a **gap in test validation**:

**Example Survived Mutation**:
```
File: CatalogService.cs:42
Original: if (items.Count > 0)
Mutated: if (items.Count >= 0)
Status: Survived (tests passed)
```

**Interpretation**: Tests don't validate the boundary condition (empty vs. non-empty list)

**Action**: Add test case:
```csharp
[Test]
public void GetItems_EmptyList_ReturnsEmptyResult() {
    var result = service.GetItems(new List<Item>());
    Assert.Empty(result);
}
```

### Prioritizing Survived Mutations

Focus on:
1. **Critical business logic**: Controllers, domain services, business rules
2. **Security-sensitive code**: Authentication, authorization, validation
3. **Data integrity**: Database operations, state management
4. **Error handling**: Exception paths, null checks

Lower priority:
- DTOs and simple models (low mutation value)
- Logging statements (unless business-critical)
- UI formatting code (unless affects data display)

### No Coverage vs. Survived

- **No Coverage**: Code not executed by tests (code coverage issue)
  - **Fix**: Add tests to execute the code
- **Survived Mutation**: Code executed, but test assertions don't validate correctness
  - **Fix**: Improve test assertions and edge case coverage

## Maintenance

### Adjusting the Threshold

Start conservatively, increase over time:
1. **Initial**: Set threshold to current baseline (e.g., 70%)
2. **Ratchet**: Gradually increase as test quality improves
3. **Target**: Achieve 80%+ over 6-12 months
4. **Maintain**: Keep threshold at 80% once achieved

### Expanding Coverage

Incremental rollout strategy:
1. **Phase 1**: Catalog project (current)
2. **Phase 2**: GameboardTenant project
3. **Phase 3**: TenantScanning project
4. **Phase 4**: CLI tools (lower priority)

### Configuration Tuning

Adjust based on results:
- **Too slow**: Increase concurrency, reduce timeout, exclude more files
- **Too many mutations**: Increase ignore patterns, focus on critical paths
- **Too many survived**: Lower threshold temporarily, improve tests gradually

## References

- **Stryker.NET Documentation**: https://stryker-mutator.io/docs/stryker-net/
- **Mutation Testing Best Practices**: https://stryker-mutator.io/docs/mutation-testing-elements/introduction/
- **GitHub Copilot Workflows**: https://github.github.com/gh-aw/

## Related Workflows

- **test-coverage-enforcement**: Prerequisite - ensures ≥80% code coverage
- **ci.yml**: Normal test execution (fast feedback)
- **repo-guardian**: Code quality gate

## Deferred Status

**NOTE**: This workflow is currently **deferred** until the code coverage prerequisite (≥80%) is met.

Current code coverage: ~70% (see test-coverage-enforcement workflow)

**Activation plan**:
1. Increase code coverage to 80%+ (in progress)
2. Merge this PR to integration branch
3. Enable workflow (currently disabled by default)
4. Run initial baseline
5. Monitor weekly reports

Until then, the workflow specification exists but is not actively executed.
