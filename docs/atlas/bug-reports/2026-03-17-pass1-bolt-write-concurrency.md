## Bug: BOLT_WRITE_CONCURRENCY default mismatch between docs and code

**Layer**: api-contracts × runtime-topology
**Severity**: Medium
**Pass**: 1
**Evidence**:

- Code default is `128`: `sedandelivery-bolt/src/main.rs:121`
- code_quote: `gql_core::config::parse_env_range("BOLT_WRITE_CONCURRENCY", 128, 1, 10000);`
- `docs/index.md:101` documents default as `64`:
- code_quote: `| \`BOLT_WRITE_CONCURRENCY\` | \`64\` | Max concurrent write transactions |`
- `docs/auto-scaling.md:180` also says `64`:
- code_quote: `Write concurrency within a single replica is further bounded by \`BOLT_WRITE_CONCURRENCY\` (default 64)`
- `docs/auto-scaling.md:523` also says `64`
- `docs/architecture.md:121` correctly says `128`:
- code_quote: `Write concurrency semaphore (\`BOLT_WRITE_CONCURRENCY\`, default 128)`

**Impact**: Operators relying on docs/index.md or docs/auto-scaling.md will underestimate the
actual write concurrency, potentially misconfiguring their deployments. Capacity planning
based on documented defaults would be wrong.

**Fix**: Update docs/index.md and docs/auto-scaling.md to document the actual default of `128`,
or decide on the intended value and update code or docs consistently.
