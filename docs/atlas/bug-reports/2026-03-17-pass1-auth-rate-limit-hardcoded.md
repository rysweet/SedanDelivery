## Bug: BOLT_AUTH_MAX_ATTEMPTS / BOLT_AUTH_WINDOW_SECS documented as configurable but hardcoded

**Layer**: api-contracts × runtime-topology
**Severity**: High
**Pass**: 1
**Evidence**:

- `docs/index.md:102-103` documents these as configurable env vars:
- code_quote: `| \`BOLT_AUTH_MAX_ATTEMPTS\` | \`10\` | Max failed auth attempts per IP per window |`
- code_quote: `| \`BOLT_AUTH_WINDOW_SECS\` | \`60\` | Auth rate limit sliding window duration |`
- Actual code uses hardcoded constants with NO env var reads:
- `sedandelivery-bolt/src/protocol/handler/hello.rs:37`:
- code_quote: `const MAX_FAILURES_PER_IP: u32 = 10;`
- `sedandelivery-bolt/src/protocol/handler/hello.rs:40`:
- code_quote: `const STALE_AFTER_SECS: u64 = 3600;`
- The stale timeout is **3600s (1 hour)** in code, but docs say **60s** window
- An IP blocked after 10 failed attempts waits 1 hour, not 60 seconds

**Impact**: Two issues: (1) operators cannot configure auth rate limiting despite docs claiming
they can; (2) the 3600s stale timeout vs 60s documented window creates confusion about actual
security posture. Legitimate users accidentally locked out face a 1-hour ban.

**Fix**: Read `BOLT_AUTH_MAX_ATTEMPTS` and `BOLT_AUTH_WINDOW_SECS` from env vars in hello.rs,
or update docs to state these are not configurable and document the true values.
