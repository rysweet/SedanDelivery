# GitHub Token Permissions for Copilot Agent

## Overview

This document explains how GitHub token permissions work with the Copilot coding agent and how to enable specific capabilities like posting PR reviews.

## Current Configuration

The repository is configured with the following permissions in `.github/workflows/copilot-setup-steps.yml`:

```yaml
permissions:
  contents: read           # Read repository contents
  pull-requests: write     # Post PR comments and reviews
```

## What Each Permission Enables

### `contents: read`
- Clone and read repository files
- View commit history
- Access repository metadata
- **Required for**: Basic Copilot operations

### `pull-requests: write`
- Post comments on pull requests
- Create and submit code reviews
- Add review comments to PR files
- Update PR descriptions
- **Required for**: Posting automated PR reviews

## How Copilot Uses These Permissions

When the Copilot agent runs:

1. **Setup Phase**: The `copilot-setup-steps` job runs with the permissions defined in the workflow
2. **Agent Phase**: Copilot receives a token based on these permissions
3. **Operations**: Tools like `gh` CLI use the `GITHUB_TOKEN` environment variable with these permissions

## Testing Permissions

To verify the token has the correct permissions:

```bash
# Check authentication status
gh auth status

# Try posting a test comment (requires pull-requests: write)
gh pr comment <PR_NUMBER> --body "Test comment"

# Check what permissions the token has
curl -H "Authorization: token $GITHUB_TOKEN" \
  https://api.github.com/repos/OWNER/REPO | jq .permissions
```

## Common Issues and Solutions

### Issue: "Resource not accessible by integration"

**Symptom**: Cannot post PR comments or reviews
```
GraphQL: Resource not accessible by integration (addComment)
```

**Solution**: Add `pull-requests: write` permission to the workflow (already configured in this repo)

### Issue: "Permission denied"

**Symptom**: Cannot push changes or create branches

**Solution**: Add `contents: write` permission if needed (use with caution)

### Issue: Token doesn't work in subshells

**Symptom**: Commands work directly but not in scripts

**Solution**: Ensure `GH_TOKEN` environment variable is exported:
```yaml
env:
  GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

## Security Best Practices

### Principle of Least Privilege

Only grant permissions actually needed:
- ✅ Start with minimal permissions
- ✅ Add permissions as needed for specific features
- ❌ Don't grant `write` permissions unnecessarily
- ❌ Avoid using `permissions: write-all`

### Safe Permissions for Copilot

**Low Risk** (generally safe to enable):
- `contents: read` - Read repository
- `pull-requests: write` - Comment on PRs
- `issues: write` - Comment on issues
- `checks: write` - Update status checks

**Medium Risk** (use with care):
- `contents: write` - Modify repository
- `deployments: write` - Trigger deployments

**High Risk** (use only if absolutely necessary):
- `actions: write` - Modify workflows
- `administration: write` - Change repo settings

## Available Permissions

Full list of permissions you can configure:

| Permission | Read | Write | Description |
|------------|------|-------|-------------|
| `actions` | ✓ | ✓ | GitHub Actions workflows |
| `checks` | ✓ | ✓ | Status checks |
| `contents` | ✓ | ✓ | Repository contents |
| `deployments` | ✓ | ✓ | Deployments |
| `discussions` | ✓ | ✓ | Discussions |
| `issues` | ✓ | ✓ | Issues |
| `packages` | ✓ | ✓ | GitHub Packages |
| `pull-requests` | ✓ | ✓ | Pull requests |
| `repository-projects` | ✓ | ✓ | Projects |
| `security-events` | ✓ | ✓ | Security alerts |
| `statuses` | ✓ | ✓ | Commit statuses |

## Examples

### Basic Read-Only Configuration
```yaml
permissions:
  contents: read
```

### Code Review Bot Configuration (Current)
```yaml
permissions:
  contents: read
  pull-requests: write
```

### Full CI/CD Bot Configuration
```yaml
permissions:
  contents: write
  pull-requests: write
  issues: write
  checks: write
  statuses: write
```

### Deployment Bot Configuration
```yaml
permissions:
  contents: read
  deployments: write
  statuses: write
```

## Modifying Permissions

To change permissions:

1. **Edit the workflow file**:
   ```bash
   nano .github/workflows/copilot-setup-steps.yml
   ```

2. **Update the permissions block**:
   ```yaml
   permissions:
     contents: read
     pull-requests: write
     issues: write  # Add new permission
   ```

3. **Commit and push**:
   ```bash
   git add .github/workflows/copilot-setup-steps.yml
   git commit -m "Add issues write permission"
   git push
   ```

4. **Test in a new Copilot session**:
   - The permissions take effect in the next workflow run
   - Create a new PR or comment to trigger Copilot

## Using Alternative Tokens

If you need permissions beyond what GITHUB_TOKEN provides:

### Option 1: Personal Access Token (PAT)

1. Create PAT at https://github.com/settings/tokens
2. Select required scopes
3. Add to repository secrets as `COPILOT_PAT`
4. Update workflow:
   ```yaml
   env:
     GH_TOKEN: ${{ secrets.COPILOT_PAT }}
   ```

### Option 2: GitHub App Token

1. Create GitHub App with required permissions
2. Install app on repository
3. Use `actions/create-github-app-token` action
4. Reference in workflow

## Troubleshooting

### Check Current Permissions

```bash
# Method 1: Using gh CLI
gh auth status

# Method 2: Using API
curl -H "Authorization: token $GITHUB_TOKEN" \
  https://api.github.com/repos/cloud-ecosystem-security/SedanDelivery \
  | jq .permissions

# Method 3: Test specific operation
gh pr comment 1 --body "test" --dry-run
```

### Debug Token Issues

```bash
# Print token info (be careful not to expose actual token)
echo "Token starts with: ${GITHUB_TOKEN:0:10}..."

# Check if GH_TOKEN is set
env | grep GH_TOKEN

# Verify gh CLI is using correct token
gh api user
```

## Additional Resources

- [GitHub Actions Permissions](https://docs.github.com/en/actions/security-guides/automatic-token-authentication#permissions-for-the-github_token)
- [Workflow Syntax - Permissions](https://docs.github.com/en/actions/using-workflows/workflow-syntax-for-github-actions#permissions)
- [GitHub Copilot Documentation](https://docs.github.com/en/copilot)
- [Security Best Practices for GitHub Actions](https://docs.github.com/en/actions/security-guides/security-hardening-for-github-actions)

## Summary

- ✅ This repository is configured with `pull-requests: write` permission
- ✅ Copilot can now post PR comments and reviews
- ✅ Permissions follow security best practices
- ✅ Token is automatically available via `GITHUB_TOKEN` environment variable

For questions or issues, refer to the troubleshooting section or GitHub documentation.
