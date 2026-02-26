# GitHub Actions Workflows

This directory contains production-ready CI/CD workflows for the CyberGym service.

## Workflows Overview

### 1. E2E Tests (`e2e-tests.yml`)

Runs end-to-end tests with mocked Azure services using TestContainers.

**Triggers**:
- Pull requests to `main`
- Push to `main`
- Manual workflow dispatch

**What it does**:
- Sets up PostgreSQL 15 test database
- Starts Azurite storage emulator
- Mocks Azure Service Bus and Key Vault
- Runs P0 priority E2E tests
- Generates coverage reports
- Posts test results to PR comments

**Duration**: ~30-45 minutes

**Requirements**:
- Tests must be marked with `@pytest.mark.p0` for P0 priority
- Tests directory: `tests/e2e/`

### 2. Deploy CyberGym (`deploy.yml`)

Unified deployment workflow with automatic branch-based isolation. Each PR can get its own complete Azure environment.

**Quick Start**:

```bash
# Get your own isolated environment (recommended)
gh pr edit <PR_NUM> --add-label "deploy"

# Or test in shared integration environment
gh pr edit <PR_NUM> --add-label "deploy-integration"
```

**How It Works**:

| Trigger | Environment | Isolation |
|---------|------------|-----------|
| PR + `deploy` label | Branch-specific | Fully isolated Azure resources |
| PR + `deploy-integration` label | Shared integration | Shared across integration PRs |
| Push to `main` | Production | Dedicated production |
| Manual dispatch | User-selected | As configured |

**Environment Types**:

1. **Branch Environments** (developer/PR testing)
   - Resource Group: `cybergym-{sanitized-branch}-rg`
   - Environment Name: `{sanitized-branch}`
   - Health Check: 5 retries × 10s (warning only on failure)
   - Isolation: **Fully isolated** - each branch gets separate Azure resources
   - Auto-cleanup: Delete resource group when done
   - Example: Branch `feature/add-auth` → `cybergym-feature-add-auth-rg`

2. **Integration Environment** (shared testing)
   - Resource Group: `cybergym-integration-rg`
   - Environment Name: `integration`
   - Health Check: 5 retries × 10s (fails workflow on timeout)
   - Isolation: **Shared** - reused across integration PRs
   - Persistent: Shared across PRs

3. **Production Environment** (live)
   - Resource Group: `cybergym-production-rg`
   - Environment Name: `production`
   - Health Check: 10 retries × 15s (fails workflow on timeout)
   - Isolation: **Dedicated production resources**
   - Persistent: Main branch only

**Resource Isolation**:

Each environment gets its own isolated Azure resources:
- ✅ **PostgreSQL Flexible Server** - Unique per environment (uses env name + unique suffix)
- ✅ **Key Vault** - Unique per environment (uses env name + unique suffix)
- ✅ **Service Bus Namespace** - Unique per environment (uses env name + unique suffix)
- ✅ **Container Apps** - Unique per environment within their Container Apps Environment
- ✅ **Container Apps Environment** - Scoped to resource group, isolated per deployment
- ✅ **Storage Accounts** - Unique per environment (if used)
- ✅ **Log Analytics** - Unique per environment

Aspire automatically generates globally unique names by combining:
1. The logical resource name (e.g., "postgres", "keyvault")
2. The environment name (e.g., "feature-add-auth", "integration", "production")
3. A unique hash/suffix to ensure global uniqueness

This ensures true isolation - deployments never interfere with each other.

**What It Does**:
1. Determines target environment (branch/integration/production)
2. Sanitizes branch names for Azure compliance
3. Provisions complete Azure infrastructure via Aspire + azd
4. Deploys GameboardTenant and Catalog services
5. Runs health checks with environment-specific strictness
6. Comments on PR with deployment URLs

**Duration**: ~15-30 minutes

**Branch Name Sanitization**:

Branch names are automatically cleaned for Azure:
- Only alphanumeric and hyphens (`a-z`, `0-9`, `-`)
- Converted to lowercase
- Max 20 characters
- Example: `feature/My_Cool-Feature` → `feature-my-cool-featu`

**Cleanup**:

```bash
# Branch deployments - delete when done testing
# Use the sanitized branch name from the workflow output or PR comment
az group delete --name cybergym-{sanitized-branch}-rg --yes

# Integration - shared, keep unless resetting
az group delete --name cybergym-integration-rg --yes

# Production - never delete via command line
```

### 3. Validate Infrastructure (`validate-infrastructure.yml`)

Validates Bicep templates and previews infrastructure changes.

**Triggers**:
- Pull requests with `infrastructure/` changes
- Push to `main` with `infrastructure/` changes
- Manual workflow dispatch

**What it does**:
1. Validates all Bicep templates (`az bicep build`)
2. Runs Bicep linter on all templates
3. Validates parameter file JSON syntax
4. Generates what-if deployment preview for integration and production
5. Posts comprehensive validation report to PR

**Duration**: ~5-10 minutes

**Validation Steps**:
- **Bicep Build**: Ensures templates compile without errors
- **Linting**: Checks for best practices and warnings
- **Parameter Validation**: Verifies JSON syntax and structure
- **What-If**: Previews changes without deploying

## Azure Configuration

### Required GitHub Secrets

```bash
# Azure OIDC Authentication
AZURE_CLIENT_ID           # Federated credential client ID
AZURE_TENANT_ID           # Azure AD tenant ID
AZURE_SUBSCRIPTION_ID     # Azure subscription ID

# Production URL
PRODUCTION_URL            # Production application URL
```

### Setting up OIDC Authentication

1. Create an Azure AD App Registration
2. Create federated credentials for GitHub Actions:
   ```bash
   az ad app federated-credential create \
     --id <APP_ID> \
     --parameters @- <<EOF
   {
     "name": "github-actions-main",
     "issuer": "https://token.actions.githubusercontent.com",
     "subject": "repo:<ORG>/<REPO>:ref:refs/heads/main",
     "audiences": ["api://AzureADTokenExchange"]
   }
   EOF
   ```
3. Add secrets to GitHub repository settings

### Required Azure Permissions

The service principal needs:
- **Contributor** on subscription or resource groups
- **Key Vault Administrator** on Key Vaults (for secrets)
- **User Access Administrator** (for managed identity role assignments)

## GitHub Environment Configuration

The unified workflow uses three GitHub environment types for protection and approval:

### Branch Environment

Create in GitHub settings: Settings → Environments → New environment

Name: `branch`

**Protection Rules**: 
- None (allows any PR to deploy to isolated environments)
- These are temporary, developer-specific deployments

### Integration Environment

Name: `integration`

**Protection Rules**: 
- Optional: Require reviewers for shared environment changes
- Recommended: None (allow auto-deploy with label)

### Production Environment

Name: `production`

**Protection Rules**:
- **Required reviewers**: 2+ (configure in GitHub settings)
- **Wait timer**: Optional (e.g., 5 minutes for safety)
- **Deployment branches**: `main` only
- **Required status checks**: CI, tests must pass

## Workflow Best Practices

### For Developers

**PR Testing - Two Options**:

```bash
# Option 1: Isolated environment (recommended)
gh pr edit <PR_NUM> --add-label "deploy"
# ✅ Your own Azure resources, no conflicts with others
# ✅ Complete isolation - PostgreSQL, Key Vault, Service Bus, etc.
# ✅ Delete resource group when done

# Option 2: Shared integration (for compatibility testing)
gh pr edit <PR_NUM> --add-label "deploy-integration"
# ✅ Test against shared integration data
# ⚠️  Shared with other PRs - use carefully
```

**Choosing Between Options**:
- **Use `deploy`** (isolated): Feature development, bug fixes, most PR testing
- **Use `deploy-integration`**: Cross-PR integration tests, compatibility validation

**Local Development**:

For iterative development without workflow runs:

```bash
# Quick setup
az login && azd auth login
azd env new my-feature --location eastus2
azd env set AZURE_RESOURCE_GROUP cybergym-my-feature-rg
azd up

# Cleanup
az group delete --name cybergym-my-feature-rg --yes
```
# Review what-if results in PR comment
# Merge when validation passes
**Production Deployments**:
```bash
# Automatically triggered on merge to main
# If the `production` environment has required reviewers, an approval in GitHub is still required
# Deployment and any pending approvals appear in the Actions tab and Environments page
```

### For Operators

**Monitoring Deployments**:
- Check GitHub Actions tab for workflow status
- Review step summaries for detailed information
- Check PR comments for test results

**Handling Failures**:
- E2E test failures: Review test artifacts
- Deployment failures: Check Azure portal for resource status
- Smoke test failures: Automatic rollback, investigate logs

**Manual Interventions**:
```bash
# Check deployment status
az deployment group show \
  --resource-group <RG_NAME> \
  --name main

# View container app logs
az containerapp logs show \
  --name <APP_NAME> \
  --resource-group <RG_NAME> \
  --follow

# Manual rollback (if needed)
az tag update \
  --resource-id "<RESOURCE_GROUP_ID>" \
  --operation Merge \
  --tags activeSlot=<previous-slot>
```

## Artifacts and Reports

All workflows upload artifacts for debugging:

- **E2E Test Results**: JUnit XML, HTML reports, coverage
- **What-If Results**: Deployment previews for each environment
- **Test Artifacts**: Logs, screenshots, error traces

**Artifact Retention**: 30 days

**Accessing Artifacts**:
1. Go to workflow run
2. Scroll to "Artifacts" section
3. Download zip file

## Troubleshooting

### Common Issues

**1. OIDC Authentication Failures**
```
Error: Failed to login to Azure
```
Solution: Verify federated credentials and ensure subject matches repository.

**2. Resource Group Not Found**
```
Error: Resource group 'rg-cybergym-integration' not found
```
Solution: Workflows create resource groups automatically. Check Azure permissions.

**3. E2E Tests Timeout**
```
Error: Test execution exceeded 45 minutes
```
Solution: Check if tests are hanging on Azure service connections. Verify mocks are working.

**4. What-If Deployment Fails**
```
Error: Template validation failed
```
Solution: Fix Bicep template errors identified in validation step.

**5. Smoke Tests Fail After Deployment**
```
Error: Health check returned HTTP 500
```
Solution: Check container app logs, verify environment configuration.

### Debug Mode

Enable debug logging:
```bash
# Set repository variable
gh variable set ACTIONS_STEP_DEBUG --body "true"
gh variable set ACTIONS_RUNNER_DEBUG --body "true"
```

## Matrix Strategy

Workflows use matrix strategies for parallel execution:

- **E2E Tests**: Single job (no matrix)
- **Deploy Integration**: Single environment
- **Deploy Production**: Blue-green slots
- **Validate Infrastructure**: Integration + Production what-if

## Concurrency Control

All workflows use concurrency groups to prevent conflicts:

```yaml
concurrency:
  group: <workflow-name>-${{ github.ref }}
  cancel-in-progress: true  # For tests/validation
  cancel-in-progress: false # For deployments
```

## Security Considerations

- **OIDC Authentication**: No long-lived secrets
- **Masked Outputs**: Sensitive values are masked in logs
- **Environment Secrets**: Stored in GitHub environments
- **Least Privilege**: Service principal has minimal required permissions
- **Approval Gates**: Production deployments require manual approval

## Performance Optimization

- **Caching**: Poetry dependencies cached between runs
- **Parallel Execution**: What-if runs in parallel for multiple environments
- **Artifact Compression**: Test results compressed before upload
- **Timeout Guards**: All jobs have appropriate timeouts

## Maintenance

**Regular Tasks**:
- Review and update Bicep templates quarterly
- Update action versions monthly
- Review artifact retention policies
- Clean up old deployments (blue/green slots)

**Monitoring**:
- Track workflow success rates
- Monitor deployment durations
- Review test coverage trends

## Support

For issues with workflows:
1. Check workflow logs in GitHub Actions tab
2. Review this README for common issues
3. Contact DevOps team: [Team Contact]

For infrastructure issues:
1. Check Azure portal for resource status
2. Review container app logs
3. Contact Platform team: [Team Contact]

---

**Last Updated**: 2026-02-06
**Version**: 1.0.0
**Maintainer**: DevOps Team
