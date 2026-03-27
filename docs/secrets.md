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
| `GH_AW_GITHUB_TOKEN` | Fallback GitHub token for gh-aw MCP server and tools, plus GHCR package-visibility updates before ACR mirroring | Lock.yml workflows (fallback chain: `GH_AW_GITHUB_MCP_SERVER_TOKEN \|\| GH_AW_GITHUB_TOKEN \|\| GITHUB_TOKEN`), `ghcr-publish.yml` | Same as `COPILOT_GITHUB_TOKEN` |
| `AZURE_CLIENT_ID` | Azure Service Principal app ID | `deploy.yml`, `secret-validation.lock.yml`, `ghcr-publish.yml` ACR mirror, Fabric provisioning | When SP is recreated |
| `AZURE_CLIENT_SECRET` | Azure Service Principal password | `deploy.yml`, `secret-validation.lock.yml`, `ghcr-publish.yml` ACR mirror, Fabric sync sidecar | **Every 90 days** (Azure default expiry) |
| `AZURE_TENANT_ID` | Azure AD tenant ID | `deploy.yml`, `secret-validation.lock.yml`, Fabric provisioning | Never (stable identifier) |
| `AZURE_SUBSCRIPTION_ID` | Azure subscription for resource deployment | `deploy.yml` | Never (stable identifier) |
| `DEPLOY_BOLT_TLS_CERT_PEM` | PEM certificate chain for the public Bolt endpoint | `deploy.yml` → Container App secret `bolt-tls-cert` | On certificate renewal or chain change |
| `DEPLOY_BOLT_TLS_KEY_PEM` | PEM private key for the public Bolt endpoint | `deploy.yml` → Container App secret `bolt-tls-key` | On private key rotation or certificate renewal |

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

**What**: GitHub token used for gh-aw internal operations (MCP server,
safe-outputs, issue creation) and by `ghcr-publish.yml` to PATCH GHCR package
visibility before the ACR mirror job runs. The lock.yml workflows use a fallback
chain: `GH_AW_GITHUB_MCP_SERVER_TOKEN || GH_AW_GITHUB_TOKEN || GITHUB_TOKEN`.

**Create**: Set this to a GitHub token whose identity can administer the
SedanDelivery container packages in `cloud-ecosystem-security`. A token that is
only suitable for Copilot API requests is not enough for the GHCR visibility
PATCH. If your current `gh` login already has the necessary package-management
access, you can set the secret directly:
```bash
gh auth token | gh secret set GH_AW_GITHUB_TOKEN \
  --repo cloud-ecosystem-security/SedanDelivery
```

You can smoke-check that access with:

```bash
gh api /orgs/cloud-ecosystem-security/packages/container/sedandelivery >/dev/null
```

If you use a PAT or automation token instead of your interactive `gh` session,
grant whatever org/repo package-management permission your GitHub setup requires
to change container package visibility for the SedanDelivery images.

**Rotation**: Same 7-day cadence as `COPILOT_GITHUB_TOKEN`. Refresh both at
the same time.

---

### Azure Service Principal (4 secrets)

**What**: An Azure AD service principal with `Contributor` role on the
subscription. Used by `deploy.yml` to provision infrastructure and deploy
container apps, and by `ghcr-publish.yml` to authenticate to
`sd2mainend655uvglnfeacr.azurecr.io` for manifest mirroring.

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

# Grant ACR data-plane push rights required by ghcr-publish mirroring
ACR_ID=$(az acr show -n sd2mainend655uvglnfeacr --query id -o tsv)
az role assignment create \
  --assignee "<appId from output>" \
  --role AcrPush \
  --scope "$ACR_ID"
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

`ghcr-publish.yml` needs both successful Azure login and `AcrPush` (or
equivalent data-plane push rights) on `sd2mainend655uvglnfeacr`. Without that
role, the login step can succeed while the later `docker buildx imagetools
create` mirror step fails to publish manifests.

---

### Bolt TLS certificate secrets (2 secrets)

**What**: The PEM certificate chain and private key used by the public Bolt
Container App. `deploy.yml` copies these GitHub environment secrets into Container App
secrets (`bolt-tls-cert` / `bolt-tls-key`), mounts them at `/mnt/secrets`, and
sets `BOLT_REQUIRE_TLS=true` so public TCP Bolt never runs plaintext.

**Set**:
```bash
# Production deploys read GitHub environment `main`.
gh secret set DEPLOY_BOLT_TLS_CERT_PEM --env main < bolt-cert.pem
gh secret set DEPLOY_BOLT_TLS_KEY_PEM  --env main < bolt-key.pem

# Branch / PR deploys read the same sanitized GitHub environment name that
# deploy.yml derives from the branch:
BRANCH_NAME="$(git rev-parse --abbrev-ref HEAD)"
BRANCH_ENV="$(printf '%s' "$BRANCH_NAME" | sed 's/[^a-zA-Z0-9-]/-/g' | tr '[:upper:]' '[:lower:]' | cut -c1-20 | sed 's/^-*//;s/-*$//')"
gh secret set DEPLOY_BOLT_TLS_CERT_PEM --env "${BRANCH_ENV:-manual-deploy}" < bolt-cert.pem
gh secret set DEPLOY_BOLT_TLS_KEY_PEM  --env "${BRANCH_ENV:-manual-deploy}" < bolt-key.pem
```

Production always reads GitHub environment `main`. Manual branch deploys fall
back to `manual-deploy` when the sanitized branch name is empty, and PR deploys
fall back to `pr-<number>`.

**Rotation**:
- `DEPLOY_BOLT_TLS_CERT_PEM`: Update whenever the certificate renews or the
  intermediate chain changes.
- `DEPLOY_BOLT_TLS_KEY_PEM`: Update whenever the private key rotates. Rotate
  the certificate secret at the same time if the key pair changes.

For the default ACA-generated hostname, use `bolt+ssc://` (or the equivalent
trust override) only for temporary bootstrap or trusted-network validation. To
offer production-authenticated TLS to untrusted clients, separately bind a
custom domain with a matching CA-issued certificate and switch clients to
`bolt+s://`.

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
| `BOLT_PASSWORD` | Generated on first deploy or reused from existing `bolt-password` Container App secret | `sd2-{env}-bolt` |
| `BOLT_TLS_CERT` | Mounted from Container App secret `bolt-tls-cert`, populated from GitHub secret `DEPLOY_BOLT_TLS_CERT_PEM` | `sd2-{env}-bolt` |
| `BOLT_TLS_KEY` | Mounted from Container App secret `bolt-tls-key`, populated from GitHub secret `DEPLOY_BOLT_TLS_KEY_PEM` | `sd2-{env}-bolt` |
| `GQL_FABRIC_CHANGELOG` | Set to `1` when Fabric is enabled | `sd2-{env}-redis` (main) |
| `GQL_FABRIC_ENDPOINT` | Output of `provision-fabric.sh` | `sd2-{env}-fabric-sync` (sidecar) |
| `GQL_FABRIC_TENANT_ID` | `AZURE_TENANT_ID` secret | `sd2-{env}-fabric-sync` (sidecar) |
| `GQL_FABRIC_CLIENT_ID` | `AZURE_CLIENT_ID` secret | `sd2-{env}-fabric-sync` (sidecar) |
| `GQL_FABRIC_CLIENT_SECRET` | `secretref:fabric-client-secret` | `sd2-{env}-fabric-sync` (sidecar) |

The deploy workflow reuses `bolt-password` on updates so routine image rollouts
do not silently rotate client credentials. Delete the Container App secret only
when you intentionally want to force a new Bolt password on the next deploy.

For environments that predate `bolt-password`, the first post-#736 deploy
generates a one-time migration password and stores it as the new secret. After
that bootstrap deploy, subsequent rollouts reuse the stored value unless you
delete the secret intentionally.

`DEPLOY_BOLT_TLS_CERT_PEM` and `DEPLOY_BOLT_TLS_KEY_PEM` are required for
public Bolt deploys. The workflow fails closed if either secret is missing
instead of exposing a public plaintext Bolt endpoint. Retrieve the generated or
reused Bolt password after deploy with:

```bash
az containerapp secret show \
  --name sd2-<env>-bolt \
  --resource-group rg-sd2-<env> \
  --secret-name bolt-password \
  --query value -o tsv
```

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
- Deploy/runtime ACR pulls use managed identity (`--registry-identity system`),
  not admin credentials, while `ghcr-publish.yml` mirror pushes use the
  `AZURE_CLIENT_ID` / `AZURE_CLIENT_SECRET` service principal secrets — see
  PR #226 and the ACR mirror workflow reference
- The `secret-validation` workflow runs daily to detect expiring or broken
  secrets before they cause deployment failures

---

## See also

- [Validation guide](validation.md) — full test + CI documentation
- [Benchmarks](benchmarks.md) — performance baseline tracking
- [GitHub Agentic Workflows docs](https://github.github.com/gh-aw/reference/auth/) — token setup reference
