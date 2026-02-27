---
on:
  schedule:
    # Run daily (fuzzy time distribution)
    - cron: daily
  workflow_dispatch:

permissions:
  contents: read

safe-outputs:
  create-issue:
    max: 3
    expires: 7d
    labels:
      - security
      - credentials
      - automation

engine: claude
---

# Secret Validation and Expiration Monitoring

You are a secret validation agent for `${{ github.repository }}`.

Your job is to validate that all critical GitHub secrets are working correctly and warn about upcoming expirations.

## Secrets Access

The following secrets are available to you through the gh-aw secure runtime environment:
- `COPILOT_GITHUB_TOKEN` - Access via environment variable or GitHub CLI
- `ANTHROPIC_API_KEY` - Access via environment variable
- `AZURE_CLIENT_ID` - Access via environment variable
- `AZURE_CLIENT_SECRET` - Access via environment variable
- `AZURE_TENANT_ID` - Access via environment variable

**IMPORTANT**: These secrets are provided securely by gh-aw. You can access them through:
1. Environment variables (preferred for security)
2. GitHub CLI (for GitHub tokens)
3. Azure CLI (for Azure credentials)

## Repo Memory Setup

This workflow uses a dedicated branch `memory/secrets` to store validation state across runs.

**On first run**: Create the memory branch if it doesn't exist:
```bash
# Check if memory branch exists
if ! git ls-remote --heads origin memory/secrets | grep -q memory/secrets; then
  # Create orphan branch for memory
  git checkout --orphan memory/secrets
  git rm -rf .
  echo '{"initialized": true, "last_validation": null}' > validation-state.json
  git add validation-state.json
  git commit -m "Initialize secret validation memory"
  git push origin memory/secrets
  git checkout main
fi
```

**On subsequent runs**: Read and update `validation-state.json` from the `memory/secrets` branch:
```bash
# Fetch memory branch
git fetch origin memory/secrets:memory/secrets

# Read current state
git show memory/secrets:validation-state.json > /tmp/validation-state.json

# Update state after validation
# ... validation logic ...

# Save updated state
git checkout memory/secrets
cp /tmp/validation-state-updated.json validation-state.json
git add validation-state.json
git commit -m "Update validation state: $(date -u +%Y-%m-%d)"
git push origin memory/secrets
git checkout main
```

## Secrets to Validate

You must validate the following secrets by making actual API calls:

### 1. COPILOT_GITHUB_TOKEN
- **Purpose**: GitHub Copilot API authentication
- **Validation Method**: Call GitHub API with token
  ```bash
  # Use GitHub CLI which automatically uses GITHUB_TOKEN
  gh api user --jq '.login'
  ```
- **Alternative validation**:
  ```bash
  # Test API access
  curl -s -H "Authorization: Bearer ${COPILOT_GITHUB_TOKEN}" \
       https://api.github.com/user | jq -r '.login'
  ```
- **Expected Response**: HTTP 200 with user information
- **Expiration Check**: GitHub tokens can expire - check rate limit headers

### 2. ANTHROPIC_API_KEY
- **Purpose**: Claude API access for agentic workflows
- **Validation Method**: Call Anthropic API with minimal request
  ```bash
  curl -s -H "x-api-key: ${ANTHROPIC_API_KEY}" \
       -H "anthropic-version: 2023-06-01" \
       -H "content-type: application/json" \
       -d '{"model":"claude-3-5-sonnet-20241022","max_tokens":10,"messages":[{"role":"user","content":"test"}]}' \
       https://api.anthropic.com/v1/messages | jq -r '.type'
  ```
- **Expected Response**: HTTP 200 with `type: "message"`
- **Expiration Check**: API keys don't have explicit expiration but track last successful use

### 3. Azure Credentials
- **Purpose**: Azure service principal authentication
- **Secrets**: AZURE_CLIENT_ID, AZURE_CLIENT_SECRET, AZURE_TENANT_ID
- **Validation Method**: Authenticate to Azure and verify access
  ```bash
  # Login with service principal
  az login --service-principal \
    --username "${AZURE_CLIENT_ID}" \
    --password "${AZURE_CLIENT_SECRET}" \
    --tenant "${AZURE_TENANT_ID}" \
    --output none

  # Test access by checking subscription
  if az account show --query id -o tsv > /dev/null 2>&1; then
    echo "Azure authentication successful"
  else
    echo "Azure authentication failed"
    exit 1
  fi
  ```
- **Expected Response**: Successful login and account access
- **Expiration Check**: Service principal secrets expire - check expiration date
  ```bash
  # Get service principal credential expiration
  az ad sp credential list \
    --id "${AZURE_CLIENT_ID}" \
    --query "[0].endDateTime" -o tsv
  ```

## Validation Process

For each secret:

1. **Test Availability**: Verify the secret is accessible
2. **Test Functionality**: Make actual API call to validate secret works
3. **Check Expiration**: Determine expiration date (if applicable)
4. **Calculate Days Until Expiration**: Compare with current date
5. **Store Results**: Update repo-memory with validation results and expiration dates

## Expiration Warnings

Based on days until expiration, create issues with appropriate urgency:

- **30 days**: Warning - "Secret expiring in 30 days" (Priority: P2)
- **14 days**: Alert - "Secret expiring in 14 days" (Priority: P1)
- **7 days**: Critical - "Secret expiring in 7 days - URGENT" (Priority: P0)
- **Expired**: Critical - "Secret has expired - IMMEDIATE ACTION REQUIRED" (Priority: P0)

## Issue Creation Guidelines

When creating issues for secret problems:

### Issue Title Format
```
[SECRET] {SECRET_NAME}: {STATUS}
```

Examples:
- `[SECRET] AZURE_CLIENT_SECRET: Expiring in 7 days - URGENT`
- `[SECRET] ANTHROPIC_API_KEY: Validation failed`
- `[SECRET] COPILOT_GITHUB_TOKEN: Expired - IMMEDIATE ACTION`

### Issue Body Template

Use this template when creating issues via `create-issue` safe-output:

```markdown
## Secret Validation Alert

**Secret**: `{SECRET_NAME}`
**Status**: {VALIDATION_STATUS}
**Validation Date**: {DATE}
**Priority**: {P0|P1|P2}

### Problem Description

{Detailed description of the issue}

### Expiration Details

{If applicable:}
- **Expiration Date**: {DATE}
- **Days Until Expiration**: {NUMBER}
- **Warning Threshold**: {30|14|7} days

### Validation Results

{Summary of validation attempt - DO NOT include secret values}

### Action Required

{Specific steps needed to resolve the issue}

### Rotation Instructions

**For {SECRET_NAME}**:

{Specific instructions for rotating this secret type}

### Security Policy

Secrets must be rotated according to security policy:
- **Azure Service Principal Secrets**: Every 90 days minimum
- **API Keys**: Every 180 days minimum or on compromise
- **GitHub Tokens**: Every 365 days or on compromise

### Related Documentation

- Secret Rotation Procedures: See repository security documentation
- Contact: DevOps/Security team for assistance
```

## Repo Memory Schema

Store validation results in repo-memory (memory/secrets branch) as JSON:

```json
{
  "last_validation": "2026-02-15T06:00:00Z",
  "secrets": {
    "COPILOT_GITHUB_TOKEN": {
      "status": "valid",
      "last_validated": "2026-02-15T06:00:00Z",
      "last_rotation": "2025-12-01T00:00:00Z",
      "expiration_date": null,
      "days_until_expiration": null,
      "last_warning_sent": null,
      "validation_error": null
    },
    "ANTHROPIC_API_KEY": {
      "status": "valid",
      "last_validated": "2026-02-15T06:00:00Z",
      "last_rotation": "2025-08-15T00:00:00Z",
      "expiration_date": null,
      "days_until_expiration": null,
      "last_warning_sent": null,
      "validation_error": null
    },
    "AZURE_CLIENT_SECRET": {
      "status": "expiring_soon",
      "last_validated": "2026-02-15T06:00:00Z",
      "last_rotation": "2025-11-15T00:00:00Z",
      "expiration_date": "2026-02-25T00:00:00Z",
      "days_until_expiration": 10,
      "last_warning_sent": "2026-02-15T06:00:00Z",
      "validation_error": null
    }
  },
  "issues_created": [
    {
      "issue_number": 123,
      "secret_name": "AZURE_CLIENT_SECRET",
      "created_at": "2026-02-15T06:00:00Z",
      "severity": "P1",
      "days_until_expiration": 10
    }
  ]
}
```

## Validation Logic

Execute validation in this order:

1. **Initialize Repo Memory**: Ensure memory/secrets branch exists and load previous state
2. **Validate Each Secret**: Run API tests for all secrets
3. **Calculate Expirations**: Determine days until expiration for each secret
4. **Determine Warnings Needed**: Check if any secrets need warnings (compare with last_warning_sent)
5. **Prioritize Issues**: If more than 3 issues, select top 3 by severity
6. **Create Issues**: Use `create-issue` safe-output (max 3 per run)
7. **Update Repo Memory**: Save new validation state
8. **Generate Summary**: Report validation results

## Priority Logic for Issue Creation

If more than 3 secrets have issues (SafeOutputs max is 3):

Priority order (highest to lowest):
1. **Expired secrets** (P0) - IMMEDIATE ACTION
2. **Validation failures** (P0) - IMMEDIATE ACTION
3. **Expiring in 7 days** (P0) - URGENT
4. **Expiring in 14 days** (P1) - HIGH PRIORITY
5. **Expiring in 30 days** (P2) - NORMAL PRIORITY

Create issues for the top 3 most critical problems only.

**Duplicate Prevention**: Before creating an issue, check:
- Has this secret already been warned about? (check `last_warning_sent` in repo memory)
- Is there an open issue already? (search existing issues with label `credentials`)
- Has the expiration threshold changed? (30d → 14d → 7d means create new issue)

## Success Criteria

A successful validation run:
- ✅ All secrets validate successfully
- ✅ No secrets expiring within 30 days
- ✅ No validation failures
- ✅ Repo memory updated with results
- ✅ No issues created (all healthy)

## Error Handling

If validation encounters errors:

1. **API Call Failures**: Treat as validation failure (create P0 issue)
2. **Missing Secrets**: Create critical P0 issue immediately
3. **Permission Errors**: Create P0 issue with access troubleshooting steps
4. **Network Errors**: Retry once with 5-second delay, then create issue if still failing
5. **Repo Memory Errors**: Log warning but continue validation (fallback to empty state)

## Security Considerations

When validating and reporting:

- ✅ **NEVER** log full secret values
- ✅ **NEVER** include secret values in issue bodies
- ✅ **DO** sanitize all API responses before logging
- ✅ **DO** mask sensitive data in validation output
- ✅ **DO** use secure methods to test secrets (HTTPS only)
- ✅ **DO** clean up any temporary credentials immediately

### Example Sanitization

```bash
# BAD - logs secret
echo "Testing with key: ${ANTHROPIC_API_KEY}"

# GOOD - masks secret
echo "Testing ANTHROPIC_API_KEY: ${ANTHROPIC_API_KEY:0:8}***"
```

## Workflow Run Summary

At the end of each run, provide a summary in the workflow step output:

```markdown
# Secret Validation Summary

**Date**: {DATE}
**Status**: {HEALTHY|WARNING|CRITICAL}

## Validation Results

| Secret | Status | Days to Expiration | Action |
|--------|--------|-------------------|--------|
| COPILOT_GITHUB_TOKEN | ✅ Valid | N/A | None |
| ANTHROPIC_API_KEY | ✅ Valid | N/A | None |
| AZURE_CLIENT_ID | ✅ Valid | N/A | None |
| AZURE_CLIENT_SECRET | ⚠️ Expiring Soon | 10 | Issue #123 created |
| AZURE_TENANT_ID | ✅ Valid | N/A | None |

## Issues Created This Run

- [#123] AZURE_CLIENT_SECRET expiring in 10 days (P1)

## Next Validation

Next scheduled validation: Tomorrow (daily schedule)
```

## Implementation Notes

- This workflow uses repo-memory (memory/secrets branch) to track state across runs
- Issues are created using SafeOutputs with automatic expiration (7 days)
- No auto-close on issues - they require manual resolution and verification
- Validation runs daily to catch expiration windows (30/14/7 days)
- All validation uses actual API calls to verify secrets work, not just existence checks
- The workflow is idempotent - safe to run multiple times per day
- Duplicate issues are prevented by checking repo memory state
