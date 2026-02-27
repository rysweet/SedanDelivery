---
on:
  pull_request:
    types: [opened, synchronize, reopened]

permissions:
  contents: read
  issues: read
  pull-requests: read

safe-outputs:
  jobs:
    submit-review:
      description: "Submit a PR review with APPROVE or REQUEST_CHANGES action"
      runs-on: ubuntu-latest
      permissions:
        pull-requests: write
      inputs:
        action:
          description: "Review action: 'approve' for passing PRs, 'request_changes' for PRs with violations"
          required: true
          type: choice
          options: [approve, request_changes]
        body:
          description: "The review body text in markdown format"
          required: true
          type: string
      steps:
        - name: Submit PR review
          env:
            GH_TOKEN: ${{ github.token }}
            PR_NUMBER: ${{ github.event.pull_request.number }}
            REPO: ${{ github.repository }}
          run: |
            if [ ! -f "$GH_AW_AGENT_OUTPUT" ]; then
              echo "No agent output found, skipping review submission"
              exit 0
            fi

            # Parse all submit_review items from agent output
            ITEMS=$(jq -c '.items[] | select(.type == "submit_review")' "$GH_AW_AGENT_OUTPUT" 2>/dev/null || echo "")
            
            if [ -z "$ITEMS" ]; then
              echo "No submit_review items found in agent output"
              exit 0
            fi

            # Ensure there is exactly one submit_review item
            ITEM_COUNT=$(printf "%s\n" "$ITEMS" | wc -l | tr -d '[:space:]')
            if [ "$ITEM_COUNT" -ne 1 ]; then
              echo "Error: expected exactly 1 submit_review item, found $ITEM_COUNT"
              exit 1
            fi

            # Process the single submit_review item
            ACTION=$(printf "%s\n" "$ITEMS" | jq -r '.action')
            BODY=$(printf "%s\n" "$ITEMS" | jq -r '.body')
            if [ -z "$ACTION" ] || [ "$ACTION" = "null" ]; then
              echo "Error: action field is required"
              exit 1
            fi

            if [ -z "$BODY" ] || [ "$BODY" = "null" ]; then
              echo "Error: body field is required"
              exit 1
            fi

            # Write body to temp file for gh pr review
            printf '%s\n' "$BODY" > /tmp/review-body.md

            echo "Submitting review with action: $ACTION"
            
            # Use --repo flag since this job runs without git checkout
            # Fallback to comment if PR review fails (e.g., when "Allow GitHub Actions 
            # to create and approve pull requests" is disabled in repo settings)
            USED_FALLBACK="false"
            
            if [ "$ACTION" = "approve" ]; then
              if ! gh pr review "$PR_NUMBER" --repo "$REPO" --approve --body-file /tmp/review-body.md 2>/tmp/review-error.txt; then
                echo "::warning::PR approval failed (likely 'Allow GitHub Actions to create and approve pull requests' is disabled). Falling back to comment."
                cat /tmp/review-error.txt
                USED_FALLBACK="true"
                gh pr comment "$PR_NUMBER" --repo "$REPO" --body-file /tmp/review-body.md
              fi
            elif [ "$ACTION" = "request_changes" ]; then
              if ! gh pr review "$PR_NUMBER" --repo "$REPO" --request-changes --body-file /tmp/review-body.md 2>/tmp/review-error.txt; then
                echo "::warning::PR review failed. Falling back to comment."
                cat /tmp/review-error.txt
                USED_FALLBACK="true"
                # Prepend warning to body for comment fallback (using printf for POSIX portability)
                printf "⚠️ **Note**: This was intended as a REQUEST_CHANGES review but fell back to a comment.\n\n%s\n" "$(cat /tmp/review-body.md)" > /tmp/review-body.md
                gh pr comment "$PR_NUMBER" --repo "$REPO" --body-file /tmp/review-body.md
              fi
            else
              echo "Error: Unknown action '$ACTION'. Expected 'approve' or 'request_changes'"
              exit 1
            fi

            if [ "$USED_FALLBACK" = "true" ]; then
              echo "::notice::Review submission used fallback to comment. To enable PR reviews, go to Settings > Actions > General and enable 'Allow GitHub Actions to create and approve pull requests'."
            fi
            echo "Review/comment submitted successfully"

            # Fail the job if violations were found (request_changes)
            # This ensures the CI status reflects that the PR has issues
            if [ "$ACTION" = "request_changes" ]; then
              echo "::error::Repo Guardian found violations in this PR. See the REQUEST_CHANGES review for details."
              exit 1
            fi
---

# Repo Guardian

You are a repository guardian agent for `${{ github.repository }}`.
Your job is to examine every file changed in PR #`${{ github.event.pull_request.number }}`
and determine whether any file is **ephemeral content** that does not belong in the repository.

## Rules

### What to reject: Point-in-Time Documents

Files whose content describes something that happened during development and
will become stale as development continues. These belong in **issues, PR comments,
commit messages, or external logs** — not in the repo.

Examples:
- Meeting notes, meeting minutes
- Sprint retrospectives, sprint reviews, sprint planning notes
- Status updates, weekly reports, daily standups
- Development diaries or journals
- Investigation notes (unless they are formal Architecture Decision Records)
- Postmortems (unless they follow a durable incident-response template)
- Files with date prefixes that suggest a snapshot in time (e.g. `2024-01-15-deployment-notes.md`)
- Content using language like "As of today..." or "Currently we are..." that will become stale

### What to reject: Temporary Scripts

Scripts that are specific to a moment in time and are not durable, reusable,
or part of the project's permanent tooling.

Examples:
- One-off fix scripts (`fix-permissions.sh`, `one-off-migration.py`)
- Debug scripts (`debug-auth.sh`, `temp-test.py`)
- Quick-fix / hack / workaround scripts
- Scripts with hardcoded environment-specific values (specific IPs, dates, paths)
- Scripts that say "run this once" or "delete after use"
- Scratch files or throwaway utilities

### What is NOT a violation

Do NOT flag these:
- `CHANGELOG.md`, `HISTORY.md` — durable by design
- Architecture Decision Records (ADRs) — even with dates, these are durable reference docs
- Configuration files (`.yml`, `.json`, `.toml`) for the project
- GitHub Actions workflows (`.github/workflows/`)
- Reusable scripts that are part of the project's toolchain (parameterized, documented)
- Test fixtures and test data
- The `repo-guardian.config.json` configuration file

## Override mechanism

Before reporting violations, check all PR comments. If any comment from a
non-bot user with OWNER, MEMBER, or COLLABORATOR association contains
`repo-guardian:override` **followed by a non-empty reason**, do NOT block the PR.
The reason must be present for auditability purposes. Instead of submitting a
REQUEST_CHANGES review, submit an APPROVE review acknowledging the override,
who authorized it, and the reason provided.

## How to analyze

For each changed file in the PR:
1. Check the **filename** for temporal indicators (dates, "temp", "hack", "one-off", etc.)
2. Read the **file content** and assess whether it is durable reference material or ephemeral
3. Use your judgment — a file named `2024-01-15-architecture-decision.md` containing a proper ADR is fine; a file named `notes-from-tuesday.md` is not
4. Consider the file's location — scripts in `scripts/` with proper docs and parameterization are durable; a script in the repo root called `fix-thing.sh` is likely temporary

## How to report

You MUST use the `submit_review` tool to report your findings. Do NOT use any other method.

**If violations are found:**
Call `submit_review` with:
- `action`: `"request_changes"`
- `body`: A markdown message with:
  - Header: `## Repo Guardian - Action Required`
  - List each violating file with the filename, why it was flagged (quote the problematic content or pattern), and where the content should go instead
  - Override instructions: "To override, add a PR comment containing `repo-guardian:override <reason>` where `<reason>` is a required non-empty justification for allowing the file(s)"

**If no violations are found:**
Call `submit_review` with:
- `action`: `"approve"`
- `body`: A markdown message with header `## Repo Guardian - Passed` and a brief summary

**If an override was found:**
Call `submit_review` with:
- `action`: `"approve"`
- `body`: A markdown message acknowledging the override, who authorized it, and the reason provided

Be thorough but avoid false positives. When in doubt, flag it with a note that it may be intentional.
