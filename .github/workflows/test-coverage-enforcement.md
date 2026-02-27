---
on:
  schedule: weekly
  workflow_dispatch:

permissions:
  contents: read
  issues: write
  pull-requests: read
  actions: read

engine: copilot

tools:
  github:
    toolsets: [repos, issues, pull_requests, actions]
    mode: remote
    read-only: false
  repo-memory:
    branch-name: memory/coverage

network:
  firewall: true
  allowed:
    - defaults
    - github

safe-outputs:
  create-issue:
    title-prefix: "[coverage] "
    labels: [quality, automation]
    max: 2
    expires: 7
    close-older-issues: true

threat-detection:
  prompt: |
    Check for:
    - Secret leaks in coverage reports
    - Exposure of internal file paths or infrastructure details
---

# Test Coverage Enforcement and Trend Analysis

You are a test coverage analysis agent for `${{ github.repository }}`.

## Your Mission

Monitor code coverage trends, enforce quality thresholds, and create actionable issues when coverage degrades or falls below acceptable levels.

## Current Coverage Threshold

**Minimum acceptable coverage: 70%**

## Tasks

### 1. Analyze Current Coverage

Run the .NET test suite with coverage collection:

```bash
dotnet test CyberGym.sln --configuration Release \
  --collect:"XPlat Code Coverage" \
  --results-directory ./coverage-results \
  --verbosity normal
```

Parse the coverage results from the generated Cobertura XML files. Extract:
- Overall line coverage percentage
- Coverage by project/assembly
- Uncovered critical files (if any)

### 2. Check Coverage Against Threshold

Compare current coverage to the 70% threshold:

- If **below 70%**: This is a CRITICAL issue requiring immediate attention
- If **70-80%**: This is acceptable but should be improved
- If **above 80%**: This is good, continue monitoring

### 3. Retrieve Historical Coverage Data

Use the `repo-memory` tool to retrieve historical coverage data stored in the `memory/coverage` branch. This data tracks coverage trends over time.

Expected data structure (JSON):
```json
{
  "coverage_history": [
    {
      "date": "2026-02-07",
      "percentage": 72.5,
      "commit": "abc123..."
    }
  ]
}
```

If no history exists, initialize it with current coverage.

### 4. Calculate Trends

Compare current coverage with the last 4 weeks of data:
- Calculate trend direction (improving/declining/stable)
- Identify significant drops (>5% decrease week-over-week)
- Calculate average coverage over the period

### 5. Update Memory with New Data

Store the current coverage data in repo-memory:
- Append current date, coverage percentage, and commit SHA
- Keep the last 12 weeks of data (remove older entries)
- Commit changes to memory/coverage branch

### 6. Generate Weekly Report Issue

Create a comprehensive weekly coverage report as a GitHub issue with:

**Title**: Weekly Code Coverage Report - [DATE]

**Body Structure**:

```markdown
## Coverage Summary

**Current Coverage**: [X.X]%
**Status**: [CRITICAL / NEEDS IMPROVEMENT / GOOD / EXCELLENT]
**Threshold**: 70% minimum

### Trend Analysis

[Chart or text representation of last 4-8 weeks]
- Week 1: X.X%
- Week 2: X.X%
- Week 3: X.X%
- Week 4: X.X%

**Trend**: [Improving ↗ / Declining ↘ / Stable →]

### Coverage by Project

| Project | Coverage | Status |
|---------|----------|--------|
| Project.A | XX% | ✅/⚠️/❌ |
| Project.B | XX% | ✅/⚠️/❌ |

### Action Items

[If below threshold]
- [ ] Immediate action required: Coverage is below 70% threshold
- [ ] Review untested code in critical paths
- [ ] Add unit tests for core business logic

[If declining trend]
- [ ] Investigate recent changes causing coverage drop
- [ ] Review PRs merged in the last week
- [ ] Ensure new features include tests

[If stable/improving]
- [ ] Continue current testing practices
- [ ] Consider increasing threshold to 75%

### Uncovered Critical Areas

[List any critical files or modules with low/no coverage]

### Recommendations

[AI-generated recommendations based on coverage patterns and trends]
```

### 7. Create Regression Issue (if needed)

If coverage dropped by more than 5% from last week, create a SEPARATE issue:

**Title**: URGENT: Code Coverage Regression Detected

**Body**:
```markdown
## Alert: Significant Coverage Drop

Coverage has dropped from **[X]%** to **[Y]%** (-[Z]% change).

### Details
- Previous week: [X]%
- Current week: [Y]%
- Change: [Z]%
- Commits since last check: [link to compare view]

### Potential Causes
[Analyze recent commits and PRs for clues]

### Action Required
1. Review recent changes
2. Identify untested code paths
3. Add tests to restore coverage
4. Do not merge new PRs until coverage is restored above 70%

### Affected Projects
[List projects with biggest coverage drops]
```

## Important Notes

- Be concise but thorough in your analysis
- Include links to relevant commits, PRs, or code files
- Use emoji indicators for quick visual status (✅⚠️❌)
- Focus on actionable recommendations
- If coverage is consistently excellent (>85% for 4+ weeks), note this as a win and suggest maintaining standards

## Error Handling

If coverage collection fails:
1. Try to determine why (build failure, missing tests, configuration issue)
2. Create an issue documenting the failure
3. Include error logs and suggested fixes
4. Tag with `[coverage]` and `bug` labels

## Remember

The goal is continuous improvement. Small incremental gains are valuable. Celebrate improvements and provide constructive guidance for areas needing attention.
