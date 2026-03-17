# Code Atlas Bug Hunt — Summary

3-pass bug hunt conducted 2026-03-17. All findings have been resolved
in this branch.

## Pass 1 + Pass 2 Findings (all fixed)

| # | Bug | Severity | Resolution |
|---|-----|----------|-----------|
| 1 | BOLT_WRITE_CONCURRENCY docs said 64, code default is 128 | High | docs/index.md updated to 128 |
| 2 | BOLT_AUTH rate limit hardcoded, STALE_AFTER 3600 vs documented 60 | High | Made configurable via env vars, default 60s |
| 3 | API reference docs used snake_case, code serializes camelCase | High | Docs + QueryResponse normalized to camelCase |
| 4 | BOLT_ALLOW_SELF_SIGNED documented but not implemented | High | Already absent from main |
| 5 | QueryResponse vs ProfileResponse inconsistent elapsed_ms | Medium | QueryResponse now has camelCase |
| 6 | BOLT_ENABLED documented but never read in code | Medium | Removed from docs |
| 7 | 40+ env vars used in code but not documented | Medium | 43 vars added to docs/index.md |
| 8 | gql-api missing from docker-compose | Medium | Added behind --profile api |
| 9 | ADR-004 documented pool size default as 16 (now 64) | Low | Note added to ADR |
| 10 | REDIS_URL fallback undocumented | Low | Documented in docs/index.md |
| 11 | gql-cli had zero tests | Low | 2 smoke tests added |
| 12 | Dockerfile.api unnecessary lib.rs touch | Low | Removed |
| 13 | architecture.md directory tree incomplete | Low | 3 dirs added |

## Pass 3 Journey Verdicts

| Journey | Verdict |
|---------|---------|
| Bolt client query | PASS |
| REST CRUD | PASS |
| Redis module query | PASS |
| Fabric sync replication | PASS (stream discovery race is known, documented) |
| Explicit transaction | PASS |
