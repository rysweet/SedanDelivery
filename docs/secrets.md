# Repository Secrets Configuration

This document lists every GitHub Actions secret required by SedanDelivery,
what it's used for, how to create it, and when it needs rotation.

---

## Quick status check

```bash
# Verify all gh-aw secrets are configured
gh aw secrets bootstrap

# List all secrets (names only, no values)
gh api repos/cloud-ecosystem-security/SedanDelivery/actions/secrets \
  --jq '.secrets[] | "\(.name)  (updated: \(.updated_at))"'
```

---

## Secret inventory

| Secret | Purpose | Used by | Rotation |
|--------|---------|---------|----------|
| `ANTHROPIC_API_KEY` | Claude API access for gh-aw agentic workflows | `secret-validation.lock.yml` (engine: claude) | On compromise or key rotation |
| `COPILOT_GITHUB_TOKEN` | GitHub Copilot engine for gh-aw agentic workflows | `auto-pr-labeling.lock.yml`, `docs-validation.lock.yml`, `license-compliance-scan.lock.yml`, `mutation-testing.lock.yml`, `stale-pr-management.lock.yml`, `test-coverage-enforcement.lock.yml` | **Every 7 days** (EMU PAT limit) |
| `GH_AW_GITHUB_TOKEN` | Fallback GitHub token for gh-aw MCP server and tools | Lock.yml workflows (fallback chain: `GH_AW_GITHUB_MCP_SERVER_TOKEN \|\| GH_AW_GITHUB_TOKEN \|\| GITHUB_TOKEN`) | Same as `COPILOT_GITHUB_TOKEN` |
| `AZURE_CLIENT_ID` | Azure Service Principal app ID | `deploy.yml`, `secret-validation.lock.yml`, Fabric provisioning | When SP is recreated |
| `AZURE_CLIENT_SECRET` | Azure Service Principal password | `deploy.yml`, `secret-validation.lock.yml`, Fabric sync sidecar | **Every 90 days** (Azure default expiry) |
| `AZURE_TENANT_ID` | Azure AD tenant ID | `deploy.yml`, `secret-validation.lock.yml`, Fabric provisioning | Never (stable identifier) |
| `AZURE_SUBSCRIPTION_ID` | Azure subscription for resource deployment | `deploy.yml` | Never (stable identifier) |

---

## How to create or refresh each secret

### ANTHROPIC_API_KEY

**What**: An Anthropic API key for Claude model access.

**Create**: Generate at [console.anthropic.com/settings/keys](https://console.anthropic.com/settings/keys).

**Set**:
```bash
gh secret set ANTHROPIC_API_KEY \
  --repo cloud-ecosystem-security/SedanDelivery \
  --body "sk-ant-api03-..."
```

**Rotation**: Only when the key is compromised or rotated in the Anthropic dashboard.

---

### COPILOT_GITHUB_TOKEN

**What**: A GitHub OAuth or PAT token with Copilot API access. Used by all
gh-aw workflows that use `engine: copilot` (6 of 7 workflows).

**Create** (recommended — uses your current `gh` CLI session):
```bash
gh auth token | gh secret set COPILOT_GITHUB_TOKEN \
  --repo cloud-ecosystem-security/SedanDelivery
```

**Alternative** (fine-grained PAT via GitHub UI):
1. Go to https://github.com/settings/personal-access-tokens/new
2. Name: `SedanDelivery Copilot`
3. Repository access: **Public repositories**
4. Permissions → Account permissions → **Copilot Requests: Read**
5. Generate, then: `gh secret set COPILOT_GITHUB_TOKEN --body "github_pat_..."`

**Rotation**: **Every 7 days** in EMU organizations (GitHub enforces max 7-day
PAT lifetime). Set a calendar reminder or use `gh aw secrets bootstrap` to
check status.

**Quick refresh**:
```bash
gh auth token | gh secret set COPILOT_GITHUB_TOKEN \
  --repo cloud-ecosystem-security/SedanDelivery
```

---

### GH_AW_GITHUB_TOKEN

**What**: Fallback GitHub token for gh-aw internal operations (MCP server,
safe-outputs, issue creation). The lock.yml workflows use a fallback chain:
`GH_AW_GITHUB_MCP_SERVER_TOKEN || GH_AW_GITHUB_TOKEN || GITHUB_TOKEN`.

**Create**: Same as `COPILOT_GITHUB_TOKEN` — set to your current auth token:
```bash
gh auth token | gh secret set GH_AW_GITHUB_TOKEN \
  --repo cloud-ecosystem-security/SedanDelivery
```

**Rotation**: Same 7-day cadence as `COPILOT_GITHUB_TOKEN`. Refresh both at
the same time.

---

### Azure Service Principal (4 secrets)

**What**: An Azure AD service principal with `Contributor` role on the
subscription. Used by `deploy.yml` to provision infrastructure and deploy
container apps.

**Create** (one-time):
```bash
# Login as an Azure admin
az login

# Create the service principal
SUBSCRIPTION_ID=$(az account show --query id -o tsv)
az ad sp create-for-rbac \
  --name "sedan-deploy" \
  --role Contributor \
  --scopes "/subscriptions/${SUBSCRIPTION_ID}" \
  --output json
# Output: { "appId": "...", "password": "...", "tenant": "..." }

# Set all 4 secrets
gh secret set AZURE_CLIENT_ID     --body "<appId from output>"
gh secret set AZURE_CLIENT_SECRET --body "<password from output>"
gh secret set AZURE_TENANT_ID     --body "<tenant from output>"
gh secret set AZURE_SUBSCRIPTION_ID --body "$SUBSCRIPTION_ID"
```

**Rotation**:
- `AZURE_CLIENT_ID`: Stable — only changes if SP is recreated
- `AZURE_CLIENT_SECRET`: **Every 90 days** (Azure default credential expiry).
  To rotate:
  ```bash
  az ad sp credential reset --id <appId> --output json
  # Set the new password:
  gh secret set AZURE_CLIENT_SECRET --body "<new password>"
  ```
- `AZURE_TENANT_ID`: Stable — never changes
- `AZURE_SUBSCRIPTION_ID`: Stable — never changes

---

### Fabric Graph Replication (uses existing Azure secrets)

Fabric Graph replication is **enabled by default** (`ENABLE_FABRIC: 'true'` in
`deploy.yml`). It does not require additional GitHub Actions secrets -- it reuses
the existing Azure service principal credentials:

| Secret | Fabric usage |
|--------|-------------|
| `AZURE_CLIENT_ID` | Passed to `scripts/provision-fabric.sh` and injected as `GQL_FABRIC_CLIENT_ID` on the sync sidecar |
| `AZURE_CLIENT_SECRET` | Passed to `scripts/provision-fabric.sh` and stored as Container App secret `fabric-client-secret` on the sync sidecar |
| `AZURE_TENANT_ID` | Passed to `scripts/provision-fabric.sh` and injected as `GQL_FABRIC_TENANT_ID` on the sync sidecar |

The service principal must have permission to create Fabric workspaces and
Graph Databases. If your SP was created with `Contributor` role on the subscription
(as described above), it already has sufficient access.

**Container App secrets (injected at deploy time, not stored in GitHub):**

| Container App env var | Source | Container |
|-----------------------|--------|-----------|
| `GQL_FABRIC_CHANGELOG` | Set to `1` when Fabric is enabled | `sd2-{env}-redis` (main) |
| `GQL_FABRIC_ENDPOINT` | Output of `provision-fabric.sh` | `sd2-{env}-fabric-sync` (sidecar) |
| `GQL_FABRIC_TENANT_ID` | `AZURE_TENANT_ID` secret | `sd2-{env}-fabric-sync` (sidecar) |
| `GQL_FABRIC_CLIENT_ID` | `AZURE_CLIENT_ID` secret | `sd2-{env}-fabric-sync` (sidecar) |
| `GQL_FABRIC_CLIENT_SECRET` | `secretref:fabric-client-secret` | `sd2-{env}-fabric-sync` (sidecar) |

When `AZURE_CLIENT_SECRET` is rotated (every 90 days), the Fabric sync sidecar
must be redeployed to pick up the new secret. Re-running the deploy workflow
handles this automatically.

See the [Fabric Graph Tutorial](FABRIC_TUTORIAL.md) for the full walkthrough.

---

## Bulk refresh (all expiring secrets at once)

For the weekly refresh of GitHub tokens and periodic Azure rotation:

```bash
# Refresh GitHub tokens (weekly)
gh auth token | gh secret set COPILOT_GITHUB_TOKEN \
  --repo cloud-ecosystem-security/SedanDelivery
gh auth token | gh secret set GH_AW_GITHUB_TOKEN \
  --repo cloud-ecosystem-security/SedanDelivery

# Verify
gh aw secrets bootstrap
```

```bash
# Rotate Azure SP secret (every 90 days)
az ad sp credential reset --id "$(gh secret list --json name | jq -r '.[] | select(.name=="AZURE_CLIENT_ID")')" --output json
# Then set the new AZURE_CLIENT_SECRET
```

---

## Automated secret monitoring

The `secret-validation.lock.yml` workflow runs daily and validates that all
secrets are functional by making actual API calls (GitHub API, Anthropic API,
Azure login). If a secret is missing, expired, or invalid, it creates an
issue tagged `[SECRET]` with rotation instructions.

To trigger a manual validation:
```bash
gh workflow run "secret-validation.lock.yml" \
  --repo cloud-ecosystem-security/SedanDelivery
```

---

## Security policy

- **Never** commit secret values to source control
- **Never** log secret values in CI output (use `echo "::add-mask::$SECRET"`)
- Store deployment secrets as Container App secrets via `secretref:` (not
  plain environment variables) — see PR #223
- ACR authentication uses managed identity (`--registry-identity system`),
  not admin credentials — see PR #226
- The `secret-validation` workflow runs daily to detect expiring or broken
  secrets before they cause deployment failures

---

## See also

- [Validation guide](validation.md) — full test + CI documentation
- [Benchmarks](benchmarks.md) — performance baseline tracking
- [GitHub Agentic Workflows docs](https://github.github.com/gh-aw/reference/auth/) — token setup reference
