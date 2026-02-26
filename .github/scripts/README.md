# PR Branch Updater

This directory contains tools to automatically update PR branches by merging their base branches into them.

## Overview

When working with many open PRs, it's important to keep their branches up to date with their base branches (usually `main` or `integration`). This helps:
- Prevent merge conflicts from accumulating
- Ensure CI/CD checks run against the latest code
- Make PRs easier to review and merge

## Available Tools

### 1. GitHub Actions Workflow

**File:** `.github/workflows/update-pr-branches.yml`

Automatically updates PR branches using GitHub Actions.

#### Features:
- ✅ Update all open PRs or specific PRs by number
- ✅ Runs on a schedule (daily at 2 AM UTC)
- ✅ Can be triggered manually via workflow_dispatch
- ✅ Skips PRs that are already up to date
- ✅ Handles merge conflicts gracefully (skips and reports)
- ✅ Provides detailed summary of results

#### Usage:

**Manual Trigger (All PRs):**
1. Go to Actions → Update PR Branches
2. Click "Run workflow"
3. Leave "pr_numbers" empty
4. Click "Run workflow"

**Manual Trigger (Specific PRs):**
1. Go to Actions → Update PR Branches
2. Click "Run workflow"
3. Enter PR numbers separated by commas (e.g., `123,456,789`)
4. Click "Run workflow"

**Automatic Schedule:**
- The workflow runs automatically every day at 2 AM UTC
- No action required

### 2. Bash Script

**File:** `.github/scripts/update-pr-branches.sh`

A standalone bash script for local or manual execution.

#### Requirements:
- Git
- GitHub CLI (`gh`)
- Authenticated GitHub CLI (`gh auth login`)

#### Usage:

**Update all open PRs:**
```bash
./.github/scripts/update-pr-branches.sh
```

**Update specific PRs:**
```bash
./.github/scripts/update-pr-branches.sh 123 456 789
```

#### Output:
The script provides colored output with:
- 🟢 Green: Successfully updated PRs
- 🟡 Yellow: Skipped PRs (already up to date or closed)
- 🔴 Red: Failed PRs (merge conflicts or other errors)

## How It Works

Both tools follow the same process:

1. **Fetch PR Information**
   - Get list of open PRs
   - Extract head branch (PR branch) and base branch

2. **Check if Update Needed**
   - Compare PR branch with base branch
   - Skip if PR branch is already up to date

3. **Merge Base Branch**
   - Checkout PR branch
   - Merge base branch into PR branch
   - Create merge commit

4. **Push Changes**
   - Push updated PR branch to remote
   - Update the PR automatically

5. **Handle Conflicts**
   - If merge conflict occurs, abort merge
   - Report conflict for manual resolution
   - Continue with next PR

## Handling Merge Conflicts

When a PR has merge conflicts that prevent automatic updates:

1. The tools will skip that PR and report it
2. You need to manually resolve conflicts:
   ```bash
   git checkout <pr-branch>
   git merge <base-branch>
   # Resolve conflicts in your editor
   git add .
   git commit
   git push origin <pr-branch>
   ```

## Best Practices

1. **Run regularly**: Update PR branches frequently to prevent conflicts from accumulating
2. **Before merging**: Always update PR branch before merging to ensure tests pass against latest code
3. **Monitor failures**: Check the workflow summary or script output for failed updates
4. **Resolve conflicts promptly**: Address merge conflicts as soon as they're reported

## Troubleshooting

### Workflow fails with "permission denied"
- Ensure the workflow has `contents: write` and `pull-requests: write` permissions
- Check repository settings → Actions → General → Workflow permissions

### Script fails with "gh: command not found"
- Install GitHub CLI: https://cli.github.com/
- Run `gh auth login` to authenticate

### Some PRs consistently fail to update
- Check if the PR has merge conflicts
- Verify the base branch exists and is accessible
- Ensure you have write access to the repository

## Security Considerations

- The workflow uses `GITHUB_TOKEN` which is automatically provided by GitHub Actions
- The token has limited permissions (scoped to the repository)
- The script uses your personal GitHub CLI authentication
- Both tools only update branches, they never auto-merge PRs

## Future Enhancements

Possible improvements:
- Add Slack/email notifications for failed updates
- Auto-comment on PRs with merge conflicts
- Support for different merge strategies (rebase, squash)
- Integration with branch protection rules
- Automatic retry mechanism for transient failures
