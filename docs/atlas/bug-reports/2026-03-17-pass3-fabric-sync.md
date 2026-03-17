## Journey: fabric-sync

### Verdict: NEEDS_ATTENTION

| Criterion | Status | Evidence |
|-----------|--------|----------|
| api-contracts routes match | pass | Changelog emission via XADD in gql-module, consumed via XREADGROUP in sync_loop.rs |
| data-flow complete | warn | Changelog format depends on ChangeLogStore wrapper; Fabric store receives openCypher mutations |
| runtime-topology matches | pass | Sync container reads from Redis Streams, posts to Fabric endpoint |
| service-components reachable | pass | main.rs → sync_loop.rs → FabricGraphStore chain verified |
| No dead code on critical path | pass | All sync symbols are used when GQL_FABRIC_ENDPOINT is set |

**Verdict Rationale:** The Fabric sync journey traces through all layers but has one attention item.
The changelog stream names evolved from `__sd2:changelog` (legacy) to `__sd2:changelog:{graph}`
(per-graph). The `discover_changelog_streams` function at `sedandelivery-fabric-sync/src/sync_loop.rs:137`
handles both formats. However, if a graph is created after the sync sidecar starts, the sidecar
must re-discover streams. The XREADGROUP polling loop blocks for `SD2_SYNC_BLOCK_MS` (default 1000ms),
so there's a brief window where new graph changelogs may be missed until the next discovery cycle.
This is a known design trade-off, not a bug, but warrants documentation.
