# Auto PR Labeling Workflow

## Overview

The `auto-pr-labeling` workflow is an automated PR labeling agent built with GitHub Agentic Workflows (gh-aw). It automatically applies appropriate labels to pull requests based on file changes and content analysis.

## Purpose

Reduce manual toil in PR management by automatically:
- Applying labels based on file patterns (e.g., tests/ → testing, docs/ → documentation)
- Analyzing PR content using AI to suggest additional contextual labels
- Respecting manually-added labels (never removing user labels)
- Enforcing safety constraints (e.g., not auto-applying deploy labels)

## Trigger Conditions

The workflow triggers on:
- **pull_request opened**: When a new PR is created
- **pull_request synchronize**: When new commits are pushed to an existing PR

## Labeling Logic

### Pattern-Based Labels

The workflow analyzes changed files and applies labels based on patterns:

| File Pattern | Label Applied | Notes |
|--------------|---------------|-------|
| `tests/`, `*Tests/`, `*.test.*`, `*.spec.*` | `testing` | Test files and directories |
| `docs/`, `*.md` in root/`.github/` | `documentation` | Documentation files |
| `.github/workflows/*.md` | `agentic-workflows` | Workflow definitions |
| `*.AppHost/*` | `architecture` | AppHost projects (deploy labels skipped for safety) |
| `*.tf`, `*.bicep`, `*.arm.json` | `architecture` | Infrastructure as Code |
| `.pre-commit-config.yaml`, linter configs | `code-quality` | Code quality tooling |
| Paths with `auth`, `cred`, `secret` | `security` | Security-sensitive code (with judgment) |

### AI Content Analysis

After pattern-based labeling, the workflow uses AI to analyze PR title, description, and file contents to determine:

**PR Type** (one of):
- `bug` - Fixes a defect
- `enhancement` - Improves existing functionality
- `feature` - Adds new capability
- `refactor` - Restructures without behavior change

**Severity** (for bugs only):
- `critical` - System down, data loss, security breach
- `high` - Major functionality broken
- `medium` - Feature degraded with workarounds

**Domain** (when applicable):
- `performance` - Optimization work
- `audit` - Addresses audit findings

## Safety Features

### Label Preservation
- **Never removes** manually-added labels
- **Never replaces** existing labels
- Only **adds** labels that don't already exist

### Deploy Label Safety
- **DOES NOT** auto-apply `deploy` or `deploy-integration` labels
- These labels trigger deployments and require manual review

### Label Limits
- Maximum **5 labels** per PR (enforced by safe-outputs)
- Prioritizes: Pattern labels > PR type > Domain > Severity

## Permissions

The workflow uses principle of least privilege:
- `contents: read` - Read repository files
- `pull-requests: read` - Read PR details and changed files
- Label modifications use `safe-outputs` instead of direct write permissions

## Files

- **auto-pr-labeling.md** - Workflow source (markdown with YAML frontmatter)
- **auto-pr-labeling.lock.yml** - Compiled workflow (generated, do not edit)
- **TEST_PLAN_auto-pr-labeling.md** - Test scenarios and validation
- **README_auto-pr-labeling.md** - This documentation

## Maintenance

### Updating the Workflow

1. Edit `auto-pr-labeling.md` (the source file)
2. Compile: `gh aw compile .github/workflows/auto-pr-labeling.md`
3. Commit both `.md` and `.lock.yml` files
4. Test on a PR to verify behavior

### Adding New Label Patterns

To add new file patterns:
1. Edit the "Pattern-Based Labels" section in `auto-pr-labeling.md`
2. Add the pattern and corresponding label
3. Recompile the workflow
4. Update this README with the new pattern

### Debugging

If the workflow doesn't label as expected:
1. Check workflow run logs: `gh run view <run_id>`
2. Verify the PR matches trigger conditions (opened/synchronize)
3. Check if labels already exist (workflow won't duplicate)
4. Verify file patterns match expected paths
5. Review safe-outputs configuration in lock file

## Example Usage

**Scenario**: PR modifying test files and documentation

1. PR opened with changes to:
   - `tests/unit/test_auth.py`
   - `docs/authentication.md`

2. Workflow triggers on `pull_request.opened`

3. Pattern analysis:
   - `tests/` → `testing` label candidate
   - `docs/` → `documentation` label candidate

4. Content analysis:
   - PR title: "Fix authentication test flakiness"
   - AI determines: `bug` + `medium` severity

5. Final labels applied:
   - `testing`
   - `documentation`
   - `bug`
   - `medium`

6. Workflow posts comment: "🏷️ Auto-labeled based on file changes and content analysis"

## Known Limitations

1. **No weekly reporting** - Schedule triggers not implemented in current version
2. **No cross-repo labeling** - Only labels PRs in the repository where workflow runs
3. **No label removal** - Cannot remove incorrectly applied labels (intentional for safety)
4. **Pattern matching is basic** - Complex path matching may require custom logic
5. **AI analysis varies** - Content analysis depends on PR description quality

## Future Enhancements

Potential improvements for future iterations:
- Weekly label usage reports (when gh-aw supports schedule triggers)
- Label effectiveness metrics (track manual label changes after auto-labeling)
- Custom label patterns via configuration file
- Integration with project boards (auto-add to triaged column)
- Support for label removal (with strict safety checks)

## Related Documentation

- [GitHub Agentic Workflows](https://github.com/github/gh-aw)
- [gh-aw CLI Reference](https://github.github.com/gh-aw/introduction/overview/)
- [Safe Outputs Documentation](https://github.github.com/gh-aw/reference/safe-outputs/)
- [Repo Guardian Workflow](./repo-guardian.md) - Similar pattern-based validation workflow
