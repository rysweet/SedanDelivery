## Bug: gql-api REST service missing from all docker-compose files

**Layer**: runtime-topology × api-contracts
**Severity**: Medium
**Pass**: 1
**Evidence**:

- `Dockerfile.api` exists and builds a standalone REST API server
- code_quote: `EXPOSE 3001` (Dockerfile.api)
- `docs/index.md:123-133` documents the gql-api configuration section with env vars
- `grep -n 'gql-api\|3001' docker-compose*.yml` returns zero results across all 5 compose files
- All compose files only define `valkey` + `sedandelivery-bolt` services

**Impact**: Users cannot `docker compose up` to run the REST API. Only Bolt protocol is
accessible via compose; REST API users must build and run manually.

**Fix**: Add a `gql-api` service definition to docker-compose.yml (optionally behind a profile
like `--profile api`) with proper GQL_REDIS_URL pointing to the valkey service.
