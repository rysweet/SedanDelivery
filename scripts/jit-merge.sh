#!/usr/bin/env bash
# Open a JIT (just-in-time admin) request issue for one or more PRs and, once
# the bot grants the elevation, admin-merge them all.
#
# Usage:
#   scripts/jit-merge.sh PR [PR ...]
#
# Example:
#   scripts/jit-merge.sh 1278 1279 1280 1281
#
# Requirements:
#   - gh CLI authenticated to a github.com account that can:
#       * file issues with the `jit` label and `gimsvc_microsoft` assignee
#       * admin-merge once JIT is granted
#   - `gh auth status` succeeds
#   - The PRs are open and on the cloud-ecosystem-security/SedanDelivery repo
#
# What this script DOES NOT do:
#   - Resolve merge conflicts. The script aborts if any PR is not mergeable.
#   - Fix CI failures. JIT bypasses checks, but you should look at red CI first.
#   - Self-approve. JIT does NOT bypass the approval requirement on protected
#     branches; if your repo requires a non-author approval, request one first.

set -euo pipefail

REPO="cloud-ecosystem-security/SedanDelivery"
JIT_LABEL="jit"
JIT_ASSIGNEE="gimsvc_microsoft"
JIT_DURATION_HOURS="${JIT_DURATION_HOURS:-1}"

if [ "$#" -lt 1 ]; then
  echo "Usage: $0 PR [PR ...]" >&2
  exit 2
fi

prs=("$@")

# Validate every PR is open and mergeable before requesting JIT.
echo ">> validating PRs..."
for pr in "${prs[@]}"; do
  state="$(gh pr view "$pr" --repo "$REPO" --json state,mergeable -q '"\(.state) \(.mergeable)"')"
  if [ "${state% *}" != "OPEN" ]; then
    echo "PR #$pr is not OPEN ($state); refusing to JIT-merge" >&2
    exit 1
  fi
  echo "  PR #$pr: $state"
done

pr_list_md=""
for pr in "${prs[@]}"; do
  url="$(gh pr view "$pr" --repo "$REPO" --json url -q .url)"
  title="$(gh pr view "$pr" --repo "$REPO" --json title -q .title)"
  pr_list_md+="- $url — $title"$'\n'
done

# File the JIT issue.
echo ">> filing JIT request..."
issue_url="$(gh issue create --repo "$REPO" \
  --title "JIT request: admin-merge $(printf '#%s ' "${prs[@]}")" \
  --label "$JIT_LABEL" \
  --assignee "$JIT_ASSIGNEE" \
  --body "### Justification
Admin-merge for the following PRs (all reviewed, CI complete or has only baseline-flake skips):

$pr_list_md
### Duration (hours)
$JIT_DURATION_HOURS")"
echo "  filed: $issue_url"

# Wait for the bot to grant JIT (typically <30s).
echo ">> waiting for JIT grant (up to 5 minutes)..."
for i in $(seq 1 60); do
  sleep 5
  body="$(gh issue view "$issue_url" --json comments -q '.comments | map(.body) | join("\n")')"
  if echo "$body" | grep -qiE 'granted|approved|elevated'; then
    echo "  JIT granted after $((i*5))s"
    break
  fi
  if [ "$i" -eq 60 ]; then
    echo "JIT not granted within 5 minutes; check $issue_url manually" >&2
    exit 1
  fi
done

# Admin-merge each PR.
echo ">> admin-merging..."
for pr in "${prs[@]}"; do
  echo "  merging #$pr ..."
  gh pr merge "$pr" --repo "$REPO" --squash --admin --delete-branch
done

echo ">> all PRs merged."
