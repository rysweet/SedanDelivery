---
title: How-To — JIT Admin Merge
description: Use the jit-merge script to admin-merge one or more PRs after JIT elevation.
last_updated: 2026-04-18
review_schedule: as-needed
doc_type: how-to
---

# JIT admin-merge for SedanDelivery PRs

For protected branches, ordinary contributors can't merge directly even when
their PR is approved and green. Microsoft's JIT (just-in-time admin)
flow grants temporary elevation in exchange for a justification on a tracked
issue.

`scripts/jit-merge.sh` automates the whole loop: file the JIT issue, poll for
grant, then `gh pr merge --admin` each PR.

## Usage

```bash
# Single PR
scripts/jit-merge.sh 1278

# Multiple PRs (one JIT issue covers all)
scripts/jit-merge.sh 1278 1279 1280 1281
```

Override the requested elevation duration (default 1 hour):

```bash
JIT_DURATION_HOURS=2 scripts/jit-merge.sh 1278 1279
```

## What the script does

1. Validates each PR is `OPEN` (refuses to JIT-merge closed/draft PRs).
2. Files an issue on `cloud-ecosystem-security/SedanDelivery` with:
   - title `JIT request: admin-merge #1278 #1279 ...`
   - label `jit`
   - assignee `gimsvc_microsoft`
   - body containing `### Justification` and `### Duration (hours)` sections
3. Polls the issue every 5s for up to 5 minutes, looking for a comment that
   contains `granted`, `approved`, or `elevated`.
4. Runs `gh pr merge --squash --admin --delete-branch` for each PR.

## What the script does NOT do

- **Resolve merge conflicts**: aborts if any PR is not mergeable. Rebase first.
- **Fix CI failures**: JIT bypasses required checks but you should look at red
  CI before requesting elevation.
- **Self-approve**: if your repo requires a non-author approval, request one
  before running the script. JIT does not bypass approval requirements.

## Requirements

- `gh` CLI authenticated to a github.com account that has permission to:
  - file issues with the `jit` label and `gimsvc_microsoft` assignee
  - admin-merge once JIT is granted
- `gh auth status` succeeds for the account you want to use. Switch with
  `gh auth switch` if needed.

## Troubleshooting

- **"JIT not granted within 5 minutes"** — check the JIT issue manually; the
  bot may have requested clarification, or this account may not be eligible.
- **`gh pr merge` fails with "must be approved"** — JIT does not waive the
  approval requirement on protected branches. Get an approving review and
  re-run.
- **PR is `MERGEABLE` but the merge fails** — try running `gh pr merge`
  manually with `-v` to see the underlying GitHub API error.
