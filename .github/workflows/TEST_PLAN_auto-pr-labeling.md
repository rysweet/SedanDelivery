# Test Plan: Auto PR Labeling Workflow

## Overview

This document defines test scenarios for the auto-pr-labeling workflow to validate correct behavior before deployment.

## Test Scenarios

### Scenario 1: Pattern-Based Labeling - Testing Changes

**Setup**:
- Create test PR with changes to files matching `tests/` pattern
- PR should have no existing labels

**Expected Behavior**:
1. Workflow triggers on PR opened
2. Detects files in `tests/` directory
3. Applies `testing` label via safe-outputs
4. Posts comment explaining labeling decision
5. Does not exceed 5 label limit

**Validation**:
```bash
# After workflow runs
gh pr view <pr_number> --json labels | jq '.labels[].name' | grep testing
gh pr view <pr_number> --json comments | jq '.comments[-1].body' | grep "Auto-labeled"
```

### Scenario 2: Pattern-Based Labeling - Documentation Changes

**Setup**:
- Create test PR with changes to `docs/*.md` files
- PR should have no existing labels

**Expected Behavior**:
1. Workflow triggers on PR opened
2. Detects files in `docs/` directory
3. Applies `documentation` label
4. Posts explanatory comment

**Validation**:
```bash
gh pr view <pr_number> --json labels | jq '.labels[].name' | grep documentation
```

### Scenario 3: Pattern-Based Labeling - Workflow Changes

**Setup**:
- Create test PR with changes to `.github/workflows/*.md`
- PR should have no existing labels

**Expected Behavior**:
1. Workflow triggers on PR opened
2. Detects workflow files
3. Applies `agentic-workflows` label
4. Posts explanatory comment

**Validation**:
```bash
gh pr view <pr_number> --json labels | jq '.labels[].name' | grep agentic-workflows
```

### Scenario 4: Content Analysis - Bug Fix

**Setup**:
- Create test PR with:
  - Title: "Fix broken authentication redirect"
  - Description: "Fixes bug where users were redirected to wrong page after login"
  - Changes to `src/auth/` files

**Expected Behavior**:
1. Workflow analyzes PR content
2. Identifies as bug fix → applies `bug` label
3. Identifies security domain → applies `security` label
4. Total labels ≤ 5

**Validation**:
```bash
gh pr view <pr_number> --json labels | jq '.labels[].name' | grep -E '(bug|security)'
```

### Scenario 5: Label Preservation - Manual Labels Respected

**Setup**:
- Create test PR with changes to `tests/` files
- Manually add `critical` label before workflow runs
- Re-trigger workflow with new commit (synchronize event)

**Expected Behavior**:
1. Workflow detects existing `critical` label
2. Adds `testing` label based on file patterns
3. **DOES NOT remove** the `critical` label
4. Final labels: `critical`, `testing`

**Validation**:
```bash
# Before workflow
gh pr view <pr_number> --json labels

# After workflow
gh pr view <pr_number> --json labels | jq '.labels[].name' | grep critical
gh pr view <pr_number> --json labels | jq '.labels[].name' | grep testing
```

### Scenario 6: Safety - Deploy Labels Not Auto-Applied

**Setup**:
- Create test PR with changes to `*.AppHost/*.csproj` files
- PR should have no existing labels

**Expected Behavior**:
1. Workflow detects AppHost files
2. Applies `architecture` label (safe)
3. **DOES NOT apply** `deploy` or `deploy-integration` labels
4. Posts comment explaining conservative labeling

**Validation**:
```bash
gh pr view <pr_number> --json labels | jq '.labels[].name' | grep architecture
gh pr view <pr_number> --json labels | jq '.labels[].name' | grep -E '(deploy|deploy-integration)' && echo "FAIL: Deploy label was applied" || echo "PASS: No deploy label"
```

### Scenario 7: Label Limit - Maximum 5 Labels

**Setup**:
- Create test PR with changes across multiple domains:
  - `tests/` (testing)
  - `docs/` (documentation)
  - `.github/workflows/` (agentic-workflows)
  - `src/auth/` (security)
  - Performance optimization (performance)
  - Bug fix in title (bug)

**Expected Behavior**:
1. Workflow identifies 6+ potential labels
2. Prioritizes most important labels
3. Applies exactly 5 labels (enforced by safe-outputs max: 5)
4. Does not fail or error

**Validation**:
```bash
label_count=$(gh pr view <pr_number> --json labels | jq '.labels | length')
if [ "$label_count" -le 5 ]; then
  echo "PASS: Label count within limit ($label_count)"
else
  echo "FAIL: Too many labels ($label_count)"
fi
```

### Scenario 8: Weekly Report Generation

**Setup**:
- Wait for Monday 9 AM UTC schedule trigger OR manually trigger workflow with schedule event

**Expected Behavior**:
1. Workflow fetches PRs from past 7 days
2. Aggregates label statistics
3. Creates GitHub issue with title: "Weekly PR Label Usage Report - [Date Range]"
4. Issue contains:
   - Total PRs labeled
   - Most common labels
   - Label distribution chart (if possible)
5. Issue labeled with `audit-report`

**Validation**:
```bash
# Check for recent report issue
gh issue list --label audit-report --limit 1 --json number,title,createdAt
gh issue view <issue_number> --json body | jq -r '.body' | grep "Total PRs labeled"
```

### Scenario 9: Synchronize Event - Label Updates on Push

**Setup**:
- Create PR with `tests/` changes → gets `testing` label
- Push additional commit with `docs/` changes
- Workflow should trigger on synchronize event

**Expected Behavior**:
1. Workflow re-analyzes PR with new files
2. Detects new `docs/` files
3. Adds `documentation` label
4. Preserves existing `testing` label
5. Posts comment about additional labeling

**Validation**:
```bash
gh pr view <pr_number> --json labels | jq '.labels[].name' | grep -E '(testing|documentation)' | wc -l
# Should be 2
```

### Scenario 10: Workflow Compilation

**Setup**:
- Run `gh aw compile` on the workflow markdown file

**Expected Behavior**:
1. Compilation succeeds without errors
2. Generates `.github/workflows/auto-pr-labeling.lock.yml`
3. Lock file contains correct triggers (pull_request, schedule)
4. Lock file has safe-outputs configuration
5. Lock file has permissions set correctly

**Validation**:
```bash
cd /home/azureuser/src/SedanDelivery
gh aw compile .github/workflows/auto-pr-labeling.md
test -f .github/workflows/auto-pr-labeling.lock.yml && echo "PASS: Lock file created" || echo "FAIL: Lock file missing"
grep "safe-outputs" .github/workflows/auto-pr-labeling.lock.yml && echo "PASS: SafeOutputs configured" || echo "FAIL: SafeOutputs missing"
```

## Test Execution Order

1. **Scenario 10** (Compilation) - MUST pass before any runtime tests
2. Scenarios 1-3 (Pattern-based) - Can run in parallel
3. Scenario 4 (Content analysis) - Independent
4. Scenario 5 (Label preservation) - Requires manual setup
5. Scenario 6 (Safety) - Critical safety test
6. Scenario 7 (Label limits) - Tests constraint enforcement
7. Scenario 9 (Synchronize) - Tests event handling
8. Scenario 8 (Weekly report) - Can be tested independently via manual trigger

## Success Criteria

- All scenarios pass validation checks
- Workflow compiles without errors
- No deploy/deploy-integration labels auto-applied
- Manual labels always preserved
- Maximum 5 labels enforced
- Weekly reports generate correctly

## Failure Recovery

If any scenario fails:
1. Review workflow logs: `gh run view <run_id>`
2. Check for missing permissions in frontmatter
3. Verify safe-outputs configuration
4. Validate toolsets are correctly configured
5. Re-compile and re-test
