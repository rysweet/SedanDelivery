# Graph Services CI/CD Pipeline Guide

## Overview

This document describes the comprehensive CI/CD pipeline for CyberGym Graph Services, implemented in `graph-services-ci.yml`.

## Pipeline Architecture

### Trigger Conditions

**Push Events:**
- Branches: `main`, `feat/issue-208-*`
- Monitored paths:
  - `src/CyberGym.Graph*/**` - Core graph services
  - `src/CyberGym.Aad*/**` - Azure AD graph components
  - `src/CyberGym.Resource*/**` - Resource processing
  - `src/CyberGym.Relationship*/**` - Relationship engine
  - `src/CyberGym.AppHost/**` - Aspire orchestration
  - `tests/**` - All test projects
  - `*.sln` - Solution files

**Pull Request Events:**
- Target branch: `main`
- Same path filters as push events

## Pipeline Stages

### 1. Build and Unit Tests

**Purpose:** Validate code compilation and run fast unit tests.

**Steps:**
1. **Checkout:** Full git history for better analysis
2. **Setup .NET 10:** Install .NET 10.0.x SDK
3. **Cache NuGet:** Speed up builds by caching packages
4. **Restore:** Download all NuGet dependencies
5. **Build:** Compile entire solution in Release mode
6. **Unit Tests:** Run all tests excluding integration tests
   - Filter: `Category!=Integration`
   - Code coverage collection enabled (OpenCover format)
   - TRX format for test results

**Outputs:**
- Test results published to GitHub Actions UI
- Code coverage reports (HTML, Cobertura, Markdown)
- Coverage summary in PR comments
- Coverage data uploaded to Codecov

**Artifacts:**
- `coverage-report` (30-day retention)

### 2. Integration Tests

**Purpose:** Validate service interactions using Testcontainers.

**Dependencies:** Requires `build-and-test` to pass.

**Steps:**
1. **Checkout & Setup:** Standard .NET 10 environment
2. **Build:** Compile solution
3. **Integration Tests:** Run tests in `CyberGym.Integration.Tests`
   - Uses Testcontainers for PostgreSQL, Redis, Azurite
   - Realistic service dependencies
   - Code coverage enabled

**Environment Variables:**
- `TESTCONTAINERS_RYUK_DISABLED: false` - Container cleanup enabled
- `TESTCONTAINERS_HUB_IMAGE_NAME_PREFIX: ""` - Default Docker Hub

**Outputs:**
- Integration test results in GitHub Actions UI
- Coverage uploaded to Codecov with `integrationtests` flag

### 3. Security Scan

**Purpose:** Detect vulnerable NuGet packages.

**Dependencies:** Requires `build-and-test` to pass.

**Steps:**
1. **Restore:** Download all dependencies
2. **Security Audit:** Check for vulnerable packages (including transitive)
3. **Report:** Upload vulnerability findings

**Outputs:**
- Warnings for detected vulnerabilities
- `security-report` artifact (30-day retention)

### 4. Deploy to Development

**Purpose:** Deploy to Azure development environment using Aspire.

**Conditions:**
- Only on `main` branch
- Only on push events (not PRs)
- All previous jobs must pass

**Environment:**
- Name: `development`
- Protected by GitHub environment rules

**Steps:**
1. **Azure Login:** Authenticate using service principal
2. **Install azd:** Azure Developer CLI for Aspire
3. **Deploy:** Use Aspire orchestration
   - Environment: `dev`
   - Location: `eastus2`
   - Project: `src/CyberGym.AppHost`

**Required Secrets:**
- `AZURE_CREDENTIALS` - Service principal credentials (JSON)
- `AZURE_SUBSCRIPTION_ID` - Target Azure subscription

**Outputs:**
- Deployment URL in environment
- Deployment logs as artifacts (14-day retention)

### 5. Notification

**Purpose:** Summary status of all pipeline stages.

**Dependencies:** Always runs after all test stages (even on failure).

**Behavior:**
- Logs status of all jobs
- Fails if build or integration tests failed
- Provides clear error messages

## Configuration

### Environment Variables

```yaml
DOTNET_VERSION: '10.0.x'          # .NET SDK version
SOLUTION_FILE: 'CyberGym.Graph.sln'  # Solution to build
BUILD_CONFIGURATION: 'Release'     # Build configuration
```

### Required GitHub Secrets

| Secret | Purpose | Format |
|--------|---------|--------|
| `AZURE_CREDENTIALS` | Azure service principal | JSON with clientId, clientSecret, subscriptionId, tenantId |
| `AZURE_SUBSCRIPTION_ID` | Target Azure subscription | GUID |
| `CODECOV_TOKEN` | Code coverage reporting | Codecov API token (optional) |

### Service Principal Setup

```bash
# Create service principal
az ad sp create-for-rbac \
  --name "SedanDelivery-github-actions" \
  --role contributor \
  --scopes /subscriptions/{subscription-id} \
  --sdk-auth

# Store output in AZURE_CREDENTIALS secret
```

## Test Categories

The pipeline distinguishes between test types using categories:

**Unit Tests:**
- Fast, isolated tests
- No external dependencies
- Run in `build-and-test` job
- Filter: `Category!=Integration`

**Integration Tests:**
- Test service interactions
- Use Testcontainers for dependencies
- Run in `integration-tests` job
- Located in: `tests/CyberGym.Integration.Tests`

### Marking Tests

```csharp
[Fact]
public void UnitTest_FastIsolated()
{
    // Runs in build-and-test job
}

[Fact]
[Trait("Category", "Integration")]
public void IntegrationTest_UsesDependencies()
{
    // Runs in integration-tests job
}
```

## Code Coverage

### Collection
- Format: OpenCover XML
- Tool: XPlat Code Coverage (built into .NET)
- Collected for both unit and integration tests

### Reporting
- Generator: ReportGenerator
- Formats: HTML, Cobertura, MarkdownSummaryGithub
- Destination: Codecov (optional)

### PR Integration
- Coverage summary automatically added to PR
- Shows coverage changes
- Helps reviewers assess test quality

## Performance Optimizations

### Caching
- NuGet packages cached between runs
- Cache key based on `*.csproj` file hashes
- Significant speedup for restore operations

### Parallel Execution
- Unit and integration tests run sequentially (dependency)
- Security scan runs in parallel with integration tests
- Build artifacts cached between jobs

### Testcontainers
- Containers created/destroyed per test run
- Ryuk enabled for automatic cleanup
- Parallel test execution supported

## Troubleshooting

### Build Failures

**Symptom:** Build step fails
**Solutions:**
1. Check for .NET 10 compatibility issues
2. Verify all NuGet packages restore correctly
3. Review compiler warnings/errors in logs

### Integration Test Failures

**Symptom:** Tests pass locally but fail in CI
**Solutions:**
1. Check Docker availability on runner
2. Verify Testcontainers configuration
3. Ensure sufficient resources for containers
4. Review container logs in test output

### Deployment Failures

**Symptom:** Deploy step fails
**Solutions:**
1. Verify Azure credentials are valid
2. Check subscription permissions
3. Ensure Aspire AppHost is configured correctly
4. Review Azure resource quotas

### Coverage Report Issues

**Symptom:** Coverage not generated or uploaded
**Solutions:**
1. Verify coverlet.collector package is installed
2. Check TestResults directory exists
3. Ensure Codecov token is valid (if used)

## Best Practices

### Test Organization
- Keep unit tests fast (< 100ms each)
- Use meaningful test names
- Tag integration tests appropriately
- Maintain high test coverage (> 80%)

### Code Quality
- Address security vulnerabilities promptly
- Review coverage reports in PRs
- Keep dependencies up to date
- Use semantic versioning

### Deployment
- Test in dev environment first
- Monitor deployment logs
- Use environment protection rules
- Implement rollback procedures

## Monitoring and Metrics

### GitHub Actions Insights
- View run history: Actions > Graph Services CI/CD
- Analyze job durations for optimization
- Track failure patterns

### Coverage Trends
- Monitor coverage over time
- Identify under-tested areas
- Set coverage thresholds

### Security Scanning
- Review vulnerability reports regularly
- Update packages promptly
- Track security audit history

## Future Enhancements

### Planned Improvements
1. **Staging Environment:** Add staging deployment after dev
2. **Performance Tests:** Add performance benchmarking job
3. **Container Registry:** Publish Docker images
4. **Release Automation:** Automated release creation
5. **E2E Tests:** End-to-end testing with full stack

### Configuration Options
- Matrix builds for multiple .NET versions
- Multi-region deployment
- Canary deployments
- Feature flag integration

## Support

### Resources
- [GitHub Actions Documentation](https://docs.github.com/en/actions)
- [.NET Testing Documentation](https://docs.microsoft.com/en-us/dotnet/core/testing/)
- [Testcontainers Documentation](https://dotnet.testcontainers.org/)
- [Azure Developer CLI](https://learn.microsoft.com/en-us/azure/developer/azure-developer-cli/)

### Contact
- Issues: GitHub Issues
- Questions: Team Slack channel
- Security: security@SedanDelivery.example.com
