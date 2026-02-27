# GitHub Actions Workflows

CI/CD workflows for the SedanDelivery Rust workspace (GQL query engine over Redis/Valkey).

## Workflows

| Workflow | File | Trigger | Purpose |
|----------|------|---------|---------|
| **CI** | `ci.yml` | Push/PR to `main` | Rust fmt, clippy, unit tests, integration tests, workspace check, MSRV |
| **CodeQL** | `codeql.yml` | Push/PR to `main` (`.rs`, `Cargo.toml`); weekly schedule | Static analysis security scanning for Rust code |
| **Dependency Review** | `dependency-review.yml` | PR to `main` when `Cargo.toml`/`Cargo.lock` change | Audit new/changed dependencies for known vulnerabilities |
| **Deploy** | `deploy.yml` | PR labeled `deploy`, push to `main`, manual dispatch | Build Docker image and deploy Valkey + GQL module to Azure Container Apps |
| **Undeploy** | `undeploy.yml` | Manual dispatch | Clean up Azure deployments (delete environment, list/cleanup orphans) |
| **Docs** | `docs.yml` | Push to `main` (`docs/**`), manual dispatch | Build and deploy documentation to GitHub Pages via Jekyll |
| **Auto Release** | `auto-release.yml` | Push to `main` | Generate version tag and create GitHub release |
| **Repo Guardian Gate** | `repo-guardian-gate.yml` | PR to `main`, issue comments | Enforce Repo Guardian findings as a blocking PR check |
| **Copilot Setup** | `copilot-setup-steps.yml` | Manual dispatch, self-path push | Configure environment for GitHub Copilot Agent |
| **Update PR Branches** | `update-pr-branches.yml` | Daily schedule (2 AM UTC), manual dispatch | Merge base branch into open PR branches to keep them current |

## Required Secrets

| Secret | Used By | Purpose |
|--------|---------|---------|
| `GITHUB_TOKEN` | All workflows | Repository access (automatic) |
| `AZURE_CLIENT_ID` | deploy, undeploy | Azure OIDC authentication |
| `AZURE_TENANT_ID` | deploy, undeploy | Azure AD tenant |
| `AZURE_SUBSCRIPTION_ID` | deploy, undeploy | Azure subscription |

## Notes

- **Cargo.lock is gitignored** -- CI cache keys use `Cargo.toml` hashes instead.
- **libclang** is installed in CI for `redis-module` bindgen builds.
- The `gql-module` crate produces a `cdylib` (`.so`) loaded by Valkey/Redis at runtime.
