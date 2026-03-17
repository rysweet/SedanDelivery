## Bug: API reference docs show snake_case but API returns camelCase

**Layer**: api-contracts × data-flow
**Severity**: High
**Pass**: 1
**Evidence**:

- `docs/graph-visualizer/api-reference.md` shows stats response as snake_case:
  `"node_count": 142, "edge_count": 318`
- Actual code at `gql-api/src/models.rs` uses `#[serde(rename_all = "camelCase")]`:
- code_quote: `#[serde(rename_all = "camelCase")] pub struct GraphStatsResponse { pub node_count: u64, ... }`
- Tests confirm: `assert_eq!(json["nodeCount"], 100);`
- Same issue for `MutateResponse` (docs show `"nodes_created"`, code sends `"nodesCreated"`)
- Same issue for `ImportResponse` and `SchemaResponse`

**Impact**: Any client built against the API reference will use wrong field names and fail
to parse responses. Frontend/SDK developers will waste time debugging JSON key mismatches.

**Fix**: Update all JSON examples in API reference docs to use camelCase field names matching
actual serialization.
