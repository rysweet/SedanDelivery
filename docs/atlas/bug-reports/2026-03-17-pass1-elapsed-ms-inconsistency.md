## Bug: QueryResponse vs ProfileResponse inconsistent elapsed_ms serialization

**Layer**: api-contracts × data-flow
**Severity**: Medium
**Pass**: 1
**Evidence**:

- `gql-api/src/models.rs` `QueryResponse` has NO `rename_all = "camelCase"`:
- code_quote: `pub struct QueryResponse { pub columns: Vec<String>, pub rows: Vec<Json>, pub elapsed_ms: u64 }`
- Serializes as `elapsed_ms`
- `gql-api/src/models.rs` `ProfileResponse` HAS `#[serde(rename_all = "camelCase")]`:
- code_quote: `#[serde(rename_all = "camelCase")] pub struct ProfileResponse { ... pub elapsed_ms: u64 }`
- Serializes as `elapsedMs`
- Tests confirm: `assert_eq!(json["elapsed_ms"], 42);` for QueryResponse

**Impact**: Clients consuming both `/query` and `/profile` must use different key names
(`elapsed_ms` vs `elapsedMs`) for the same logical field. Breaks uniform response parsing.

**Fix**: Add `#[serde(rename_all = "camelCase")]` to `QueryResponse`, or remove it from
`ProfileResponse`, to make serialization consistent.
