## Bug: ADR-004 documents stale GQL_REDIS_POOL_SIZE default of 16

**Layer**: compile-deps × data-flow
**Severity**: Low
**Pass**: 1
**Evidence**:

- ADR-004 documents pool size default as `16`: `docs/ADR-004-distributed-redis.md:36`
- code_quote: `- \`GQL_REDIS_POOL_SIZE\` (default 16)`
- Actual code default is `64`: `gql-redis-store/src/connection.rs:33`
- code_quote: `pub(crate) const DEFAULT_POOL_SIZE: u32 = 64;`
- Other docs correctly say `64`: `docs/index.md:111`, `docs/DEPLOY.md:256`, `docs/cluster-mode.md:123`

**Impact**: Engineers reading ADR-004 for historical design context may be confused
by the stale default value. Low severity since the ADR is a historical record, but
a note clarifying the current value would prevent confusion.

**Fix**: Add a note to ADR-004 that the pool size default was later increased from 16 to 64,
or update the value inline with a "(updated)" marker.
