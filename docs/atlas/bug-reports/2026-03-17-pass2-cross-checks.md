## Pass 2 Cross-Check: BOLT_WRITE_CONCURRENCY default mismatch

**Pass 1 verdict:** Medium — Default value documented inconsistently across docs
**Pass 2 verdict:** CONFIRMED

**Rationale:** Independently verified all four locations. Code at
`sedandelivery-bolt/src/main.rs:121` sets default `128`. `docs/architecture.md:121`
correctly says `128`. But `docs/index.md:101` and `docs/auto-scaling.md:180,523`
both say `64`. Three of four documentation locations are wrong. Operators relying
on the primary env-var reference table will underestimate actual write concurrency
by 50%.

---

## Pass 2 Cross-Check: ADR-004 stale pool size default

**Pass 1 verdict:** Low — ADR-004 has stale default
**Pass 2 verdict:** CONFIRMED

**Rationale:** `gql-redis-store/src/connection.rs:33` defines `DEFAULT_POOL_SIZE: u32 = 64`.
`docs/ADR-004-distributed-redis.md:36` says `16`. Live reference (`docs/index.md:111`)
correctly says `64`. ADR was never updated when the default was bumped. Low severity
since live docs are correct, but ADR is misleading for design rationale readers.

---

## Pass 2 Cross-Check: BOLT_ENABLED phantom env var

**Pass 1 verdict:** Medium — Documented but never read
**Pass 2 verdict:** CONFIRMED

**Rationale:** Exhaustive search: zero matches for `BOLT_ENABLED` across all .rs files,
Dockerfiles, shell scripts, and docker-compose files. Only appears in docs/index.md
(twice) and specs/sedandelivery-bolt-spec.md as a hypothetical shell pattern. Ghost
feature — setting it has zero effect.

---

## Pass 2 Cross-Check: 29 undocumented environment variables

**Pass 1 verdict:** Medium — Large documentation gap
**Pass 2 verdict:** CONFIRMED (refined to 29+ vars)

**Rationale:** All five sample-checked variables confirmed: `BOLT_DRAIN_TIMEOUT_SECS`,
`BOLT_JSON_LOGS`, `GQL_API_TLS_CERT`, `GQL_API_TLS_KEY`, `GQL_CSV_IMPORT_DIR` — all
present in code, all absent from docs/index.md. Pass 2 also found 12 additional
`parse_env_range`/`parse_env_min` variables not in docs, bringing total gap to 40+
undocumented config knobs.
