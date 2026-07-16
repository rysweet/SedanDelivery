# GitHub Actions Workflows

CI/CD workflows for the SedanDelivery Rust workspace (openCypher query engine over Redis/Valkey).

## Workflows

| Workflow | File | Trigger | Purpose |
|----------|------|---------|---------|
| **CI** | `ci.yml` | Push/PR to `main` | Rust fmt, clippy, unit tests, integration tests, workspace check, MSRV |
| **CodeQL** | `codeql.yml` | Push/PR to `main` (`.rs`, `Cargo.toml`); weekly schedule | Static analysis security scanning for Rust code |
| **Dependency Review** | `dependency-review.yml` | PR to `main` when `Cargo.toml`/`Cargo.lock` change | Audit new/changed dependencies for known vulnerabilities |
| **QA Playwright Tests** | `qa-playwright.yml` | PR to `main`, push to `main`, manual dispatch | Runs `sedan-web` Playwright E2E against a local `sd2-api` test backend |
| **Repo Guardian Gate** | `repo-guardian-gate.yml` | PR to `main`, issue comments | Enforce Repo Guardian findings as a blocking PR check |
| **Copilot Setup** | `copilot-setup-steps.yml` | Manual dispatch, self-path push | Configure environment for GitHub Copilot Agent |
| **Update PR Branches** | `update-pr-branches.yml` | Daily schedule (2 AM UTC), manual dispatch | Merge base branch into open PR branches to keep them current |

## Maintenance Notes

- **Arithmetic in `run:` blocks**: Never use `((var++))` or `((var--))` under `set -e` — they return exit code 1 when the pre-increment value is 0, aborting the step. Use `var=$((var + 1))` instead. See [Maintain CI Workflows](../../docs/howto/maintain-ci-workflows.md#github-actions-arithmetic-varvar-vs-varvar--1).
- **Docker validation whitelist**: `scripts/validate-dockerfiles.sh` has a `DOCKER_WHITELIST` array for workspace members excluded from Docker images. Update it when adding new test/bench/example crates. See [Maintain CI Workflows](../../docs/howto/maintain-ci-workflows.md#docker-validation-whitelist).
- **Gremlin TCK concurrency**: The cucumber harness uses `--concurrency`, not `--test-threads`. See [Maintain CI Workflows](../../docs/howto/maintain-ci-workflows.md#gremlin-tck-ci-concurrency).

## Required Secrets

| Secret | Used By | Purpose |
|--------|---------|---------|
| `GITHUB_TOKEN` | All workflows | Repository access (automatic) |

## Notes

- **Cargo.lock is committed** -- CI cache keys still use `Cargo.toml` hashes, and
  Docker validation now checks that every workflow-built Rust Dockerfile copies
  `Cargo.lock` before any `cargo build` layer. PR CI also runs the builder
  stages to catch real Docker regressions before merge.
- **libclang** is installed in CI for `redis-module` bindgen builds.
- The `sd2-redis-module` crate produces a `cdylib` (`.so`) loaded by Valkey/Redis at runtime.
- The web UI Playwright project lives under `sedan-graph-ui/src/sedan-web`.
- `qa-playwright.yml` starts local `sd2-api` on port `3001` and then runs the Playwright suite on the Vite dev server at port `5175` in E2E test-auth mode.
