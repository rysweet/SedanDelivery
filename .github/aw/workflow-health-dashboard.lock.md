---
on:
  schedule: weekly on monday
  workflow_dispatch:

permissions:
  contents: read
  actions: read
  discussions: read
  issues: read

engine: claude

tools:
  github:
    toolsets: [repos, issues, actions]
    mode: remote
    read-only: true
  repo-memory:
    branch-name: memory/workflow-health

network:
  firewall: true
  allowed:
    - defaults
    - github

safe-outputs:
  create-discussion:
    title-prefix: '[Workflow Health]'
    max: 1
  create-issue:
    title-prefix: '[Workflow Health Critical]'
    labels: [automation, workflow-health]
    max: 2
---

# Workflow Health Dashboard

You are a workflow health monitoring agent for `${{ github.repository }}`.

Your mission is to generate comprehensive weekly health reports for all 12 agentic workflows, tracking their performance, reliability, and effectiveness.

## Monitored Workflows

Track the following 12 agentic workflows:

1. **test-coverage-enforcement** - Weekly test coverage analysis
2. **container-security-scanning** - Container vulnerability scanning
3. **auto-pr-labeling** - Automatic PR categorization
4. **docs-validation** - Documentation quality checks
5. **sbom-generation** - Software Bill of Materials creation
6. **stale-pr-management** - Inactive PR cleanup
7. **changelog-generation** - Automated changelog updates
8. **infrastructure-drift-detection** - Infrastructure drift monitoring
9. **license-compliance-scanning** - License compliance verification
10. **performance-testing** - Performance regression detection
11. **canary-deployment** - Canary deployment analysis
12. **mutation-testing** - Mutation test coverage

## Execution Strategy

### Step 1: Collect Workflow Run Data

For each of the 12 workflows, query GitHub Actions API to retrieve:

```bash
gh api \
  repos/${{ github.repository }}/actions/workflows \
  --jq '.workflows[] | select(.name | contains("WORKFLOW_NAME")) | .id'
```

Then for each workflow ID, get the last 7 days of runs:

```bash
gh api \
  "repos/${{ github.repository }}/actions/workflows/{workflow_id}/runs?per_page=100&created=>=$(date -d '7 days ago' --iso-8601)" \
  --jq '.workflow_runs[] | {id, status, conclusion, created_at, updated_at}'
```

### Step 2: Calculate Metrics

For each workflow, compute:

#### **Execution Success Rate**
- Total runs in past 7 days
- Successful runs (conclusion = "success")
- Failed runs (conclusion = "failure", "timed_out", "cancelled")
- Success rate percentage: `(successful / total) * 100`

#### **Average Runtime**
- Calculate duration for each run: `updated_at - created_at`
- Compute average duration across all runs
- Identify anomalies (runs > 2x average)

#### **Issue Creation Trends**
- Query repo-memory for each workflow's branch (e.g., `memory/coverage`, `memory/default`)
- Count issues created by workflow in past 7 days
- Categorize by severity (if workflow includes labels)

#### **SafeOutputs Usage**
- Analyze workflow configurations for safe-output limits
- Track how close workflows are to hitting limits
- Flag workflows consistently hitting >80% of limits

#### **Repo-Memory Growth**
- For each workflow's repo-memory branch:
  ```bash
  gh api repos/${{ github.repository }}/branches/memory/{branch-name} \
    --jq '.commit.commit.tree.sha'
  ```
- Calculate branch size and growth rate week-over-week
- Flag branches approaching storage limits

#### **Error Rates**
- Parse workflow run logs for common failure patterns
- Categorize errors:
  - API failures (GitHub Copilot, GitHub API)
  - Timeout errors
  - Configuration errors
  - Tool failures
- Track frequency of each error type

### Step 3: Trend Analysis

Compare current week metrics with historical data from repo-memory:

1. **Load Historical Data**: Read from `/tmp/gh-aw/repo-memory/workflow-health/history.json`
2. **Calculate Deltas**: Week-over-week changes for each metric
3. **Identify Trends**:
   - ↗ Improving: Success rate increasing, runtime decreasing
   - ↘ Declining: Success rate decreasing, errors increasing
   - → Stable: Metrics within ±5% of previous week

Expected data structure in repo-memory:
```json
{
  "week": "2026-02-15",
  "workflows": {
    "test-coverage-enforcement": {
      "success_rate": 95.2,
      "avg_runtime_seconds": 420,
      "total_runs": 7,
      "issues_created": 1,
      "errors": 0
    }
  }
}
```

### Step 4: Generate Recommendations

Based on metrics, provide actionable recommendations:

**High Success Rate (>95%)**:
- ✅ Workflow is healthy
- Consider: Expanding coverage or increasing frequency

**Moderate Success Rate (80-95%)**:
- ⚠️ Needs attention
- Investigate: Common failure patterns
- Recommend: Error handling improvements

**Low Success Rate (<80%)**:
- ❌ Critical issue
- Action required: Review workflow configuration
- Consider: Temporarily disable or rewrite workflow

**Runtime Anomalies**:
- If average runtime >2x baseline: Investigate performance regression
- If high variance: Analyze workload patterns

**SafeOutputs Pressure**:
- If usage >80% of limit: Increase limits or optimize workflow
- If consistently hitting limit: Workflow may be too noisy

**Repo-Memory Growth**:
- If >10% growth per week: Review data retention strategy
- If approaching 100MB: Implement cleanup policy

### Step 5: Create Weekly Dashboard Discussion

Post comprehensive dashboard to GitHub Discussions:

**Title**: `[Workflow Health] Weekly Dashboard - 2026-02-15`

**Body Structure**:

```markdown
# Workflow Health Dashboard
**Week of February 15, 2026**

## Executive Summary

- **Total Workflows Monitored**: 12
- **Overall Health Score**: 89% (Good)
- **Critical Issues**: 1 workflow needs immediate attention
- **Trends**: 8 improving, 2 stable, 2 declining

---

## Workflow Health Matrix

| Workflow | Success Rate | Avg Runtime | Issues Created | Trend | Status |
|----------|-------------|-------------|----------------|-------|--------|
| Test Coverage | 95.2% | 7m 0s | 1 | ↗ +2.3% | ✅ Healthy |
| Container Scan | 87.5% | 12m 30s | 2 | → -0.5% | ⚠️ Monitor |
| Auto Labeling | 100% | 1m 15s | 0 | ↗ +5.0% | ✅ Excellent |
| Docs Validation | 78.3% | 4m 45s | 3 | ↘ -8.2% | ❌ Critical |
| SBOM Generation | 92.1% | 8m 20s | 1 | ↗ +1.2% | ✅ Healthy |
| Stale PR Mgmt | 100% | 2m 10s | 0 | → 0.0% | ✅ Excellent |
| Changelog | 85.7% | 5m 30s | 2 | ↘ -3.1% | ⚠️ Monitor |
| Drift Detection | 91.2% | 15m 0s | 1 | ↗ +4.5% | ✅ Healthy |
| License Scan | 88.9% | 10m 15s | 1 | → +0.8% | ✅ Healthy |
| Performance Test | 82.4% | 22m 0s | 2 | → -1.2% | ⚠️ Monitor |
| Canary Deploy | 100% | 3m 45s | 0 | ↗ +2.0% | ✅ Excellent |
| Mutation Test | 90.0% | 18m 30s | 1 | ↗ +3.5% | ✅ Healthy |

---

## Critical Attention Required

### 🚨 Docs Validation Workflow

**Current Status**: 78.3% success rate (BELOW 80% threshold)

**Key Issues**:
- 5 of 23 runs failed in past week
- Common error: "Broken link detection timeout"
- Runtime increased from 3m to 4m 45s (+58%)

**Recommended Actions**:
1. [ ] Review timeout configuration (increase from 5m to 10m)
2. [ ] Optimize link validation algorithm
3. [ ] Consider splitting large docs into batches
4. [ ] Investigate recent documentation changes causing failures

**Impact**: Documentation quality checks are unreliable, potentially allowing broken links to merge

---

## Metric Deep Dive

### Execution Success Rates

**Top Performers** (100% success):
- Auto PR Labeling (84/84 runs)
- Stale PR Management (84/84 runs)
- Canary Deployment (7/7 runs)

**Needs Improvement** (<85% success):
- Docs Validation: 78.3% (18/23 successful)
- Performance Testing: 82.4% (28/34 successful)

### Runtime Performance

**Fastest Workflows**:
- Auto PR Labeling: 1m 15s avg
- Stale PR Management: 2m 10s avg
- Canary Deployment: 3m 45s avg

**Slowest Workflows**:
- Performance Testing: 22m 0s avg
- Mutation Testing: 18m 30s avg
- Drift Detection: 15m 0s avg

**Runtime Anomalies**:
- Docs Validation: +58% increase (3m → 4m 45s)
- Container Scan: -12% improvement (14m 15s → 12m 30s) ✅

### Issue Creation Activity

**Most Active Workflows** (issues created in past week):
- Docs Validation: 3 issues
- Container Scan: 2 issues
- Performance Testing: 2 issues
- Changelog: 2 issues

**Total Issues Created**: 12 (avg 1.7 per day)

### SafeOutputs Utilization

| Workflow | Limit | Avg Usage | Peak Usage | Pressure |
|----------|-------|-----------|------------|----------|
| Test Coverage | 2 issues | 1.2 | 2 | 🟡 High |
| Stale PR Mgmt | 5 comments | 3.8 | 5 | 🟡 High |
| Auto Labeling | 10 labels | 6.2 | 8 | 🟢 OK |
| Docs Validation | 3 issues | 2.1 | 3 | 🟡 High |

**Recommendations**:
- Consider increasing issue limit for Docs Validation (frequently hits max)
- Stale PR Management efficiently uses comment budget

### Repo-Memory Health

| Branch | Size | Growth | Retention | Status |
|--------|------|--------|-----------|--------|
| memory/coverage | 2.3 MB | +8% | 12 weeks | 🟢 OK |
| memory/default | 15.7 MB | +12% | All | 🟡 Monitor |
| memory/workflow-health | 450 KB | +5% | 26 weeks | 🟢 OK |
| memory/drift | 8.1 MB | +22% | 8 weeks | 🔴 High |

**Action Required**:
- Drift detection repo-memory growing rapidly (+22% week-over-week)
- Review retention policy and implement cleanup for branches >10MB

---

## Error Analysis

### Common Error Patterns

1. **API Timeouts** (18 occurrences)
   - Affected: Container Scan, Performance Testing
   - Cause: Long-running queries to GitHub API
   - Fix: Implement pagination and caching

2. **Copilot API Errors** (7 occurrences)
   - Affected: Changelog, Docs Validation
   - Error: "missing finish_reason for choice 0"
   - Fix: Add retry logic with exponential backoff

3. **Network Errors** (4 occurrences)
   - Affected: SBOM Generation
   - Cause: Transient connection issues to package registries
   - Fix: Already has retry logic, consider increasing attempts

4. **Timeout Errors** (3 occurrences)
   - Affected: Docs Validation
   - Cause: Link validation exceeding 5-minute timeout
   - Fix: Increase timeout to 10 minutes

---

## Week-over-Week Comparison

### Improving Workflows ↗

- **Auto PR Labeling**: +5.0% success rate
- **Drift Detection**: +4.5% success rate
- **Mutation Testing**: +3.5% success rate
- **Test Coverage**: +2.3% success rate

### Declining Workflows ↘

- **Docs Validation**: -8.2% success rate (CRITICAL)
- **Changelog**: -3.1% success rate

### Stable Workflows →

- **Stale PR Management**: 0.0% change
- **Container Scan**: -0.5% change

---

## Recommendations & Action Items

### Immediate Actions (This Week)

- [ ] **CRITICAL**: Fix Docs Validation workflow timeout issues
- [ ] Implement cleanup policy for drift detection repo-memory
- [ ] Add retry logic to Changelog workflow for Copilot API errors

### Short-Term Improvements (Next 2 Weeks)

- [ ] Optimize Performance Testing runtime (target: <15 minutes)
- [ ] Increase issue limits for high-pressure workflows
- [ ] Implement caching for GitHub API queries in Container Scan

### Long-Term Optimization (Next Month)

- [ ] Review and standardize error handling across all workflows
- [ ] Implement centralized metrics collection for better observability
- [ ] Consider workflow dependency analysis to prevent cascading failures

---

## Success Stories 🎉

- **Auto PR Labeling** maintained perfect 100% success rate for 3rd consecutive week
- **Stale PR Management** processed 47 PRs with zero errors
- **Container Scan** improved runtime by 12% through optimization

---

**Next Dashboard**: Monday, February 22, 2026

*Generated by Workflow Health Dashboard Agent*
*Run ID: ${{ github.run_id }} | Workflow: workflow-health-dashboard*
```

### Step 6: Create Critical Issues (If Needed)

If any workflows have <80% success rate, create a separate critical issue:

**Title**: `[Workflow Health Critical] {workflow-name} Requires Immediate Attention`

**Body**:
```markdown
## Critical Workflow Issue

**Workflow**: {workflow-name}
**Success Rate**: {X}% (BELOW 80% threshold)
**Monitoring Period**: Past 7 days

### Key Metrics

- Total Runs: {X}
- Successful: {X}
- Failed: {X}
- Average Runtime: {X}

### Failure Analysis

{Detailed breakdown of failure patterns}

### Recent Failures

| Run ID | Date | Error | Log Link |
|--------|------|-------|----------|
| ... | ... | ... | ... |

### Recommended Actions

1. [ ] {Action 1}
2. [ ] {Action 2}
3. [ ] {Action 3}

### Impact

{Description of what functionality is affected by this workflow's poor health}

### Next Steps

Please investigate and resolve within 48 hours. If issues persist, consider:
- Temporarily disabling workflow
- Reverting recent changes
- Consulting with team for major redesign

---

*Auto-generated by Workflow Health Dashboard*
*This issue will auto-close if workflow health improves above 85%*
```

## Step 7: Update Repo-Memory

Store current week's metrics in `/tmp/gh-aw/repo-memory/workflow-health/history.json`:

```json
{
  "last_updated": "2026-02-15T09:00:00Z",
  "weeks": [
    {
      "week_start": "2026-02-15",
      "workflows": {
        "test-coverage-enforcement": {
          "success_rate": 95.2,
          "avg_runtime_seconds": 420,
          "total_runs": 7,
          "successful_runs": 7,
          "failed_runs": 0,
          "issues_created": 1,
          "errors": []
        }
        // ... other workflows
      },
      "overall_health_score": 89.0
    }
    // Keep last 26 weeks of data
  ]
}
```

Maintain rolling 26-week history (6 months) for trend analysis.

## Important Notes

- **Fail Safe**: If any data collection fails, use partial data and note gaps in dashboard
- **Time Zones**: All timestamps in UTC
- **Pagination**: Use `per_page=100` for API queries to reduce request count
- **Rate Limiting**: Implement 1-second delays between API calls if approaching rate limits
- **Data Validation**: Verify all calculated percentages are between 0-100
- **Friendly Tone**: Dashboard should celebrate successes and gently highlight issues

## Error Handling

If issues occur:
1. Log detailed error to workflow summary
2. Generate partial dashboard with available data
3. Note missing data sections clearly
4. Do NOT create critical issues if data collection failed
5. Store partial results in repo-memory for next run

## Success Criteria

A successful dashboard run:
- Collects metrics for all 12 workflows
- Generates comprehensive discussion post
- Creates critical issues only for genuine problems (success rate <80%)
- Updates repo-memory with complete weekly snapshot
- Provides actionable recommendations
- Maintains positive, constructive tone

---

Begin by checking the current date and calculating the past 7 days' date range. Query GitHub Actions API for all workflow runs, analyze their health, and generate the comprehensive dashboard. Use repo-memory to track trends and maintain historical context.

Good luck keeping our workflows healthy!
