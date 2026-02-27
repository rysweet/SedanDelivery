# Test Coverage Enforcement Workflow

## Overview

This GitHub Agentic Workflow monitors code coverage trends, enforces quality thresholds, and creates actionable issues when coverage degrades or falls below the 70% minimum threshold.

## Architecture

### Workflow Components

1. **Source File**: `test-coverage-enforcement.md` - Natural language workflow definition
2. **Compiled File**: `test-coverage-enforcement.lock.yml` - Generated GitHub Actions workflow
3. **CI Integration**: Modified `ci.yml` to enforce coverage threshold on every PR

### How It Works

```
┌─────────────────────────────────────────────────────────────┐
│  Weekly Schedule / Manual Trigger                           │
└────────────────────┬────────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────────┐
│  Run .NET Tests with Coverage Collection                    │
│  - dotnet test --collect:"XPlat Code Coverage"             │
│  - Parse Cobertura XML results                              │
└────────────────────┬────────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────────┐
│  Retrieve Historical Data (repo-memory)                     │
│  - Branch: memory/coverage                                  │
│  - Last 12 weeks of coverage data                          │
└────────────────────┬────────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────────┐
│  Analyze Trends & Calculate Metrics                        │
│  - Week-over-week comparison                               │
│  - 4-8 week trend analysis                                 │
│  - Project-level breakdown                                 │
└────────────────────┬────────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────────┐
│  Update Memory with Current Data                           │
│  - Append current coverage + date + commit                 │
│  - Keep last 12 weeks only                                 │
└────────────────────┬────────────────────────────────────────┘
                     │
                     ▼
┌─────────────────────────────────────────────────────────────┐
│  Generate GitHub Issues (SafeOutputs)                       │
│  - Weekly report issue                                      │
│  - Regression alert (if >5% drop)                          │
└─────────────────────────────────────────────────────────────┘
```

## Coverage Threshold

**Minimum Required: 70%**

### Enforcement Points

1. **CI Pipeline** (`ci.yml`): Every PR must maintain 70%+ coverage to merge
2. **Weekly Workflow**: Monitors trends and creates issues for violations
3. **Regression Detection**: Alerts on significant drops (>5%)

### Status Levels

| Coverage Range | Status | Action |
|---------------|--------|--------|
| < 70% | 🔴 CRITICAL | Block PR, immediate action required |
| 70-80% | 🟡 ACCEPTABLE | Monitor, should improve |
| 80-90% | 🟢 GOOD | Continue current practices |
| 90%+ | ✅ EXCELLENT | Celebrate, maintain standards |

## Memory Storage (repo-memory)

### Branch: `memory/coverage`

Stores historical coverage data in JSON format:

```json
{
  "coverage_history": [
    {
      "date": "2026-02-14",
      "percentage": 75.3,
      "commit": "abc123def456",
      "projects": {
        "CyberGym.Core": 82.5,
        "CyberGym.Api": 68.0
      }
    }
  ]
}
```

### Data Retention

- **Stored**: Last 12 weeks
- **Updated**: Weekly (or on-demand via workflow_dispatch)
- **Purpose**: Trend analysis, regression detection

## Generated Issues

### 1. Weekly Coverage Report

**Title**: `[coverage] Weekly Code Coverage Report - [DATE]`

**Labels**: `quality`, `automation`

**Expires**: 7 days (auto-closed to prevent clutter)

**Content**:
- Current coverage percentage and status
- 4-8 week trend visualization
- Per-project breakdown
- Actionable recommendations
- Links to relevant commits/PRs

### 2. Regression Alert (if needed)

**Title**: `[coverage] URGENT: Code Coverage Regression Detected`

**Labels**: `quality`, `automation`

**Triggered When**: Coverage drops >5% from previous week

**Content**:
- Before/after coverage comparison
- Commits responsible for drop
- Affected projects
- Action items to restore coverage

## Security Features

### SafeOutputs Configuration

```yaml
safe-outputs:
  create-issue:
    title-prefix: "[coverage] "
    labels: [quality, automation]
    max: 2                      # Limit to 2 issues per run
    expires: 7                  # Auto-close after 7 days
    close-older-issues: true    # Close previous week's report
```

### Threat Detection

Checks for:
- Secret leaks in coverage reports
- Exposure of internal file paths
- Infrastructure details in output

### Network Firewall

```yaml
network:
  firewall: true
  allowed:
    - defaults    # Certificates, schemas
    - github      # GitHub APIs only
```

## Compilation

This workflow must be compiled before use:

```bash
# Install gh-aw CLI extension (if not already installed)
gh extension install github/gh-aw

# Compile the workflow
cd .github/workflows
gh aw compile test-coverage-enforcement.md

# This generates test-coverage-enforcement.lock.yml
```

### What Compilation Does

1. **Schema Validation**: Validates frontmatter YAML
2. **Action Pinning**: Pins all actions to specific SHA hashes
3. **Security Scanning**: Runs actionlint, zizmor, poutine
4. **SafeOutput Generation**: Creates permission-isolated jobs
5. **Lock File Creation**: Generates production-ready `.lock.yml`

## CI Integration

The `ci.yml` workflow has been updated to enforce coverage on every PR:

### New Steps

1. **Run tests with coverage**: Collects XPlat Code Coverage
2. **Install coverage tool**: Installs reportgenerator-globaltool
3. **Generate coverage report**: Creates HTML + text summary
4. **Check coverage threshold**: Enforces 70% minimum (fails PR if below)

### Example Output

```
Current coverage: 75.3%
✓ Coverage 75.3% meets the threshold of 70%
```

or

```
Current coverage: 68.5%
✗ Coverage 68.5% is below the required threshold of 70%
```

## Usage

### Automatic (Weekly)

Runs every week automatically to:
- Generate coverage report issue
- Update historical trends
- Detect regressions

### Manual Trigger

```bash
# Run coverage analysis now
gh workflow run test-coverage-enforcement.md
```

### View Historical Data

```bash
# Clone the memory branch
git fetch origin memory/coverage:memory/coverage
git checkout memory/coverage

# View coverage history
cat coverage-data.json | jq '.coverage_history'
```

## Monitoring

### Check Workflow Status

```bash
gh run list --workflow=test-coverage-enforcement.lock.yml
```

### View Latest Run

```bash
gh run view --workflow=test-coverage-enforcement.lock.yml
```

### Download Artifacts

```bash
gh run download <run-id>
```

## Troubleshooting

### Workflow Doesn't Compile

**Problem**: `gh aw compile` fails

**Solution**:
1. Check frontmatter YAML syntax
2. Ensure all required fields are present
3. Validate against gh-aw schema
4. Check actions-lock.json for version conflicts

### Coverage Collection Fails

**Problem**: Tests run but no coverage generated

**Solution**:
1. Verify .NET 10 is installed
2. Check test project has coverage tools
3. Ensure Cobertura format is supported
4. Verify file paths in reportgenerator command

### Memory Branch Not Found

**Problem**: `memory/coverage` branch doesn't exist

**Solution**:
1. The workflow will create it on first run
2. Or manually create: `git checkout --orphan memory/coverage`
3. Initialize with empty JSON: `echo '{"coverage_history":[]}' > coverage-data.json`
4. Commit and push

### Issues Not Created

**Problem**: Workflow runs but no issues appear

**Solution**:
1. Check SafeOutputs logs in workflow run
2. Verify threat detection passed
3. Check issue creation permissions (issues: write)
4. Ensure labels exist in repository

## Future Enhancements

### Planned Features

- [ ] Per-file coverage heatmap
- [ ] Integration with PR comments (show coverage diff)
- [ ] Slack/Teams notifications for regressions
- [ ] Configurable threshold per project
- [ ] Coverage badges for README
- [ ] Quarterly trend reports

### Configuration Options

Consider making these configurable:
- Coverage threshold (currently hardcoded 70%)
- Regression sensitivity (currently 5%)
- Memory retention period (currently 12 weeks)
- Report frequency (currently weekly)

## References

### Documentation

- [GitHub Agentic Workflows](https://github.com/github/gh-aw)
- [gh-aw Foundation Document](./.claude/gh-aw-foundation.md)
- [.NET Code Coverage](https://learn.microsoft.com/en-us/dotnet/core/testing/unit-testing-code-coverage)
- [ReportGenerator](https://github.com/danielpalme/ReportGenerator)

### Related Workflows

- `ci.yml` - Enforces coverage threshold on every PR
- `repo-guardian.md` - Example gh-aw workflow for reference

## Maintenance

### Update Workflow

1. Edit `test-coverage-enforcement.md`
2. Run `gh aw compile`
3. Commit both `.md` and `.lock.yml` files
4. Push changes

### Review Coverage Data

Monthly review recommended:
- Check trend direction
- Identify problematic areas
- Adjust thresholds if needed
- Celebrate improvements

### Archive Old Issues

The workflow auto-closes issues after 7 days, but consider:
- Manually closing resolved regression alerts
- Archiving/pinning significant reports
- Extracting insights for quarterly reviews
