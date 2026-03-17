## Bug: BOLT_ENABLED env var documented but never read in code

**Layer**: runtime-topology × api-contracts
**Severity**: Low
**Pass**: 1
**Evidence**:

- `docs/index.md:95` and `docs/index.md:223` document `BOLT_ENABLED` with default `true`:
- code_quote: `| \`BOLT_ENABLED\` | \`true\` | Enable/disable Bolt server in container |`
- `grep -rn 'BOLT_ENABLED' --include="*.rs"` returns zero matches in Rust code
- The variable only appears in `docs/specs/sedandelivery-bolt-spec.md:2291` in a shell snippet:
- code_quote: `if [ "${BOLT_ENABLED:-true}" = "true" ]; then`
- Neither `entrypoint.sh`, `Dockerfile.bolt`, nor `sedandelivery-bolt/src/main.rs` reference it

**Impact**: Operators setting `BOLT_ENABLED=false` would expect the Bolt server to not start,
but the variable is silently ignored. Low severity since Bolt is a dedicated container and
disabling it would just mean not deploying it.

**Fix**: Either implement the env var check in `sedandelivery-bolt/src/main.rs` or remove it
from the documentation to avoid confusion.
