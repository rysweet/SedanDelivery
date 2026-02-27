# Graph Services CI/CD Quick Start

## Prerequisites

### GitHub Secrets Setup

Configure these secrets in your GitHub repository (Settings > Secrets and variables > Actions):

```bash
# 1. Create Azure Service Principal
az ad sp create-for-rbac \
  --name "SedanDelivery-github-actions" \
  --role contributor \
  --scopes /subscriptions/{YOUR_SUBSCRIPTION_ID} \
  --sdk-auth > azure-credentials.json

# 2. Add to GitHub Secrets:
AZURE_CREDENTIALS      # Paste entire azure-credentials.json content
AZURE_SUBSCRIPTION_ID  # Your Azure subscription ID
CODECOV_TOKEN         # Optional: From codecov.io
```

### Environment Protection (Optional but Recommended)

1. Go to Settings > Environments
2. Create "development" environment
3. Add protection rules:
   - Required reviewers (for production deploys)
   - Wait timer
   - Deployment branches (limit to main)

## Workflow Triggers

### Automatic Triggers

**Push to main or feature branches:**
```bash
git push origin main
git push origin feat/issue-208-my-feature
```

**Pull Request to main:**
```bash
gh pr create --base main
```

### Manual Trigger

```bash
# Via GitHub CLI
gh workflow run graph-services-ci.yml

# Via GitHub UI
Actions > Graph Services CI/CD > Run workflow
```

## Pipeline Overview

```
┌─────────────────────┐
│  Build & Unit Tests │ ─┐
└─────────────────────┘  │
                         ├──> ┌──────────────────┐
┌─────────────────────┐  │    │  Deploy to Dev   │
│ Integration Tests   │ ─┤    │  (main only)     │
└─────────────────────┘  │    └──────────────────┘
                         │
┌─────────────────────┐  │
│  Security Scan      │ ─┘
└─────────────────────┘
```

**Duration:** ~8-12 minutes (without deployment)

## What Gets Tested

### Unit Tests (Fast)
- Core graph operations
- Data models and validation
- Business logic
- Utilities and helpers

### Integration Tests (Slower)
- Database operations (PostgreSQL via Testcontainers)
- Cache operations (Redis via Testcontainers)
- Azure Storage (Azurite via Testcontainers)
- Service interactions

### Security Scan
- Vulnerable NuGet packages
- Transitive dependencies
- Security advisories

## Viewing Results

### Test Results
```bash
# Via GitHub CLI
gh run list --workflow=graph-services-ci.yml
gh run view {run-id}

# View latest run
gh run view --log
```

### Coverage Reports
1. Go to Actions > Latest Run
2. Download "coverage-report" artifact
3. Extract and open `index.html`

### Pull Request Checks
- All checks must pass before merge
- Coverage summary appears in PR comments
- Test results visible in Checks tab

## Local Testing

### Run Unit Tests
```bash
dotnet test CyberGym.Graph.sln \
  --configuration Release \
  --filter "Category!=Integration"
```

### Run Integration Tests
```bash
# Requires Docker running
dotnet test tests/CyberGym.Integration.Tests/CyberGym.Integration.Tests.csproj
```

### Check for Vulnerabilities
```bash
dotnet list package --vulnerable --include-transitive
```

### Generate Coverage Report
```bash
# Run tests with coverage
dotnet test --collect:"XPlat Code Coverage" --results-directory ./coverage

# Generate HTML report
dotnet tool install -g dotnet-reportgenerator-globaltool
reportgenerator \
  -reports:./coverage/**/coverage.cobertura.xml \
  -targetdir:./coverage/report \
  -reporttypes:Html

# Open report
open ./coverage/report/index.html  # macOS
xdg-open ./coverage/report/index.html  # Linux
```

## Common Workflows

### Feature Development
```bash
# 1. Create feature branch
git checkout -b feat/issue-208-my-feature

# 2. Make changes and commit
git add .
git commit -m "feat: implement new feature"

# 3. Push triggers CI
git push origin feat/issue-208-my-feature

# 4. Create PR (CI runs again)
gh pr create --base main --fill

# 5. Address any CI failures
# 6. Merge when all checks pass
```

### Hotfix to Production
```bash
# 1. Create hotfix branch from main
git checkout main
git pull
git checkout -b hotfix/critical-fix

# 2. Make fix and test locally
dotnet test

# 3. Create PR
git push origin hotfix/critical-fix
gh pr create --base main --fill

# 4. After PR approval and merge, deployment triggers automatically
```

### Checking Deployment Status
```bash
# View deployment
gh workflow view graph-services-ci.yml

# Check environment status
gh api repos/{owner}/{repo}/deployments

# View Azure resources
az resource list --tag environment=dev --query "[?contains(name, 'SedanDelivery')]"
```

## Troubleshooting

### "Build failed"
```bash
# Check for .NET 10 compatibility
dotnet build CyberGym.Graph.sln

# Update packages if needed
dotnet restore --force
```

### "Integration tests failed"
```bash
# Ensure Docker is running
docker ps

# Check Testcontainers can access Docker
docker run --rm hello-world

# Run tests with verbose logging
dotnet test tests/CyberGym.Integration.Tests/ --logger "console;verbosity=detailed"
```

### "Deployment failed"
```bash
# Verify Azure credentials
az login --service-principal \
  -u {clientId} \
  -p {clientSecret} \
  --tenant {tenantId}

# Check Aspire configuration
cd src/CyberGym.AppHost
azd init --from-code
```

### "Coverage not uploading"
```bash
# Verify Codecov token (optional)
# Check TestResults directory exists
ls -la TestResults/

# Manual upload
bash <(curl -s https://codecov.io/bash) -t {CODECOV_TOKEN}
```

## Performance Tips

### Faster Local Builds
```bash
# Use incremental builds
dotnet build --no-restore

# Skip tests during development
dotnet build --no-restore /p:RunAnalyzers=false
```

### Faster CI Runs
- Keep unit tests fast (< 100ms each)
- Use test categories appropriately
- Cache dependencies (already configured)
- Run tests in parallel (where safe)

## Monitoring

### Key Metrics
- **Build time:** Should be < 5 minutes
- **Test time:** Unit tests < 2 minutes, Integration < 5 minutes
- **Coverage:** Target > 80%
- **Security:** Zero high-severity vulnerabilities

### Weekly Checklist
- [ ] Review failed builds
- [ ] Update vulnerable packages
- [ ] Check coverage trends
- [ ] Verify deployment health
- [ ] Review artifact retention

## Getting Help

### Debug a Failed Run
```bash
# Download logs
gh run view {run-id} --log > build.log

# View specific job
gh run view {run-id} --job {job-id}

# Re-run failed jobs
gh run rerun {run-id} --failed
```

### Common Commands
```bash
# List recent runs
gh run list --limit 10

# Watch a running workflow
gh run watch

# Cancel a run
gh run cancel {run-id}

# Download artifacts
gh run download {run-id}
```

## Next Steps

1. **Set up branch protection:** Require CI to pass before merge
2. **Configure Codecov:** For better coverage insights
3. **Add deployment environments:** Staging, production
4. **Set up notifications:** Slack/Teams integration
5. **Create runbooks:** For common deployment scenarios

## Quick Links

- [Full CI/CD Guide](./GRAPH_SERVICES_CI_GUIDE.md)
- [GitHub Actions Docs](https://docs.github.com/en/actions)
- [Testcontainers .NET](https://dotnet.testcontainers.org/)
- [Azure Developer CLI](https://learn.microsoft.com/en-us/azure/developer/azure-developer-cli/)
