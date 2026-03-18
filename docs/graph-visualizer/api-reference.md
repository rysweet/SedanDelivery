---
title: Graph Visualizer API Reference
description: Complete reference for the gql-api Rust HTTP service (port 3001)
type: reference
updated: 2026-02-27
---

# API Reference

This document covers the gql-api Rust service that runs on port 3001. The
`.NET BFF` (`SedanDelivery.Api`, port 5000) proxies these same endpoints at
`/api/*` with added CORS and rate limiting. Unless you are building a custom
integration, prefer the BFF at port 5000.

## Contents

- [Graph Name Constraints](#graph-name-constraints)
- [Error Format](#error-format)
- [Value Types](#value-types)
- [Health](#health)
- [Graphs](#graphs)
- [Nodes](#nodes)
- [Edges](#edges)
- [Query and Mutate](#query-and-mutate)
- [Schema](#schema)
- [Neighbors](#neighbors)
- [Export and Import](#export-and-import)
- [Rate Limits](#rate-limits)

## Graph Name Constraints

All graph names must match the pattern:

```
^[a-zA-Z0-9_.\-]{1,64}$
```

Requests with names that do not match return `400 Bad Request` with:

```json
{ "error": "invalid graph name: ..." }
```

## Error Format

All errors use a consistent JSON envelope:

```json
{ "error": "human-readable description" }
```

| HTTP Status | Meaning |
|---|---|
| 400 | Invalid input (bad graph name, empty Cypher, bad property value) |
| 404 | Graph, node, or edge not found |
| 409 | Graph already exists |
| 422 | Unprocessable (plan or execution error not caused by parse failure) |
| 429 | Rate limit exceeded (BFF only, on `/query` and `/mutate`) |
| 500 | Internal server error |

## Value Types

Node and edge properties accept JSON values. The gql-core engine maps them to
its internal `Value` type as follows:

| JSON type | gql-core Value |
|---|---|
| `null` | `Value::Null` |
| `true` / `false` | `Value::Bool` |
| integer (no decimal) | `Value::Integer` |
| float | `Value::Float` |
| string | `Value::String` |
| array | `Value::List` |
| object | `Value::Map` |

All properties are round-tripped through JSON; no type information is lost.

## Health

### GET /health

Returns the service status.

**Response 200**

```json
{
  "status": "ok",
  "version": "0.1.0",
  "demo_graph_exists": false
}
```

## Graphs

### GET /graphs

Lists all graph names, sorted alphabetically.

**Response 200**

```json
{
  "graphs": ["delivery-network", "test-graph"]
}
```

### POST /graphs

Creates a new named graph.

**Request body**

```json
{ "name": "delivery-network" }
```

**Response 201**

```json
{ "name": "delivery-network" }
```

**Response 409** — graph already exists

```json
{ "error": "graph 'delivery-network' already exists" }
```

### DELETE /graphs/{name}

Deletes a graph and all its nodes and edges. This operation is irreversible.

**Response 204** — no body

**Response 404**

```json
{ "error": "graph 'delivery-network' not found" }
```

### GET /graphs/{name}/stats

Returns aggregate counts for the graph.

**Response 200**

```json
{
  "nodeCount": 142,
  "edgeCount": 318,
  "labelCount": 4,
  "relTypeCount": 6
}
```

## Nodes

All node endpoints are scoped under `/graphs/{name}/nodes`.

### GET /graphs/{name}/nodes

Lists nodes with pagination.

**Query parameters**

| Parameter | Type | Default | Description |
|---|---|---|---|
| `limit` | integer | 100 | Maximum nodes to return |
| `offset` | integer | 0 | Number of nodes to skip |

**Response 200**

```json
{
  "nodes": [
    {
      "id": "1",
      "labels": ["Warehouse"],
      "properties": {
        "name": "Seattle-HQ",
        "capacity": 5000
      }
    }
  ],
  "total": 142
}
```

### POST /graphs/{name}/nodes

Creates a new node. Returns the created node with its assigned ID.

**Request body**

```json
{
  "labels": ["Warehouse"],
  "properties": {
    "name": "Seattle-HQ",
    "capacity": 5000
  }
}
```

Both `labels` and `properties` are optional and default to empty.

**Response 201**

```json
{
  "id": "1",
  "labels": ["Warehouse"],
  "properties": {
    "name": "Seattle-HQ",
    "capacity": 5000
  }
}
```

### GET /graphs/{name}/nodes/{id}

Returns a single node by its integer ID.

**Response 200**

```json
{
  "id": "1",
  "labels": ["Warehouse"],
  "properties": {
    "name": "Seattle-HQ",
    "capacity": 5000
  }
}
```

**Response 404**

```json
{ "error": "node '999' not found in graph 'delivery-network'" }
```

### PUT /graphs/{name}/nodes/{id}

Replaces a node's labels and properties entirely. To update individual
properties, fetch the node first and merge before sending.

**Request body** — same structure as `POST /nodes`

**Response 200** — the updated node

### DELETE /graphs/{name}/nodes/{id}

Deletes the node and all edges incident to it.

**Response 204** — no body

## Edges

All edge endpoints are scoped under `/graphs/{name}/edges`.

### GET /graphs/{name}/edges

Lists edges with pagination.

**Query parameters**

| Parameter | Type | Default |
|---|---|---|
| `limit` | integer | 100 |
| `offset` | integer | 0 |

**Response 200**

```json
{
  "edges": [
    {
      "id": "1",
      "type": "SUPPLIES",
      "source": "1",
      "target": "2",
      "properties": {
        "frequency": "daily"
      }
    }
  ],
  "total": 318
}
```

### POST /graphs/{name}/edges

Creates a directed edge. Both `source` and `target` must be existing node IDs
in the same graph.

**Request body**

```json
{
  "source": "1",
  "target": "2",
  "type": "SUPPLIES",
  "properties": {
    "frequency": "daily"
  }
}
```

`type` is required and must be non-empty. `properties` is optional.

**Response 201**

```json
{
  "id": "1",
  "type": "SUPPLIES",
  "source": "1",
  "target": "2",
  "properties": {
    "frequency": "daily"
  }
}
```

**Response 400** — source or target node does not exist

### GET /graphs/{name}/edges/{id}

Returns a single edge by ID.

### PUT /graphs/{name}/edges/{id}

Replaces an edge's properties. `source`, `target`, and `type` are also
accepted and will replace the existing values.

**Response 200** — the updated edge

### DELETE /graphs/{name}/edges/{id}

Deletes an edge.

**Response 204** — no body

## Query and Mutate

Both endpoints accept Cypher statements and are rate-limited to 100 requests
per 60-second window per IP when accessed through the BFF.

### POST /graphs/{name}/query

Executes a read-only Cypher statement (MATCH, RETURN, etc.). Write clauses
submitted here will be rejected at the planning stage.

**Request body**

```json
{
  "cypher": "MATCH (w:Warehouse) RETURN w.name, w.capacity ORDER BY w.capacity DESC"
}
```

**Response 200**

```json
{
  "columns": ["w.name", "w.capacity"],
  "rows": [
    ["Seattle-HQ", 5000],
    ["Portland-DC", 3200]
  ],
  "elapsedMs": 3
}
```

Each element of `rows` is a JSON array whose values correspond positionally to
`columns`. Scalar values are JSON primitives; node/edge values are serialized
as objects.

**Response 400** — parse or planning error

```json
{ "error": "unexpected token at line 1, column 7: 'CREAT'" }
```

**Response 404** — graph not found

### POST /graphs/{name}/mutate

Executes a write Cypher statement (CREATE, MERGE, SET, DELETE, etc.).

**Request body**

```json
{
  "cypher": "CREATE (w:Warehouse {name: 'Tacoma-Cross', capacity: 2800})"
}
```

**Response 200**

```json
{
  "nodesCreated": 1,
  "nodesDeleted": 0,
  "edgesCreated": 0,
  "edgesDeleted": 0,
  "propertiesSet": 2
}
```

## Schema

### GET /graphs/{name}/schema

Returns label counts and relationship-type counts derived from a full scan of
all nodes and edges. Results are sorted alphabetically.

**Response 200**

```json
{
  "labels": [
    { "name": "Driver",     "count": 48 },
    { "name": "Route",      "count": 37 },
    { "name": "Warehouse",  "count": 12 }
  ],
  "relationshipTypes": [
    { "name": "ASSIGNED_TO", "count": 48 },
    { "name": "CONNECTS",    "count": 156 },
    { "name": "SUPPLIES",    "count": 114 }
  ]
}
```

## Neighbors

### GET /graphs/{name}/neighbors/{nodeId}

Returns the subgraph consisting of the specified node plus all directly
connected nodes and the edges between them.

**Response 200**

```json
{
  "nodes": [
    { "id": "1", "labels": ["Warehouse"], "properties": { "name": "Seattle-HQ" } },
    { "id": "2", "labels": ["Route"],     "properties": { "name": "SEA-PDX" } }
  ],
  "edges": [
    { "id": "1", "type": "CONNECTS", "source": "1", "target": "2", "properties": {} }
  ]
}
```

**Response 404** — node or graph not found

## Export and Import

### GET /graphs/{name}/export

Exports all nodes, edges, and a schema summary as a single JSON document.

**Response 200**

```json
{
  "nodes": [
    { "id": "1", "labels": ["Warehouse"], "properties": { "name": "Seattle-HQ" } }
  ],
  "edges": [
    { "id": "1", "type": "SUPPLIES", "source": "1", "target": "2", "properties": {} }
  ],
  "schema": {
    "labels": [{ "name": "Warehouse", "count": 1 }],
    "relationshipTypes": [{ "name": "SUPPLIES", "count": 1 }]
  }
}
```

### POST /graphs/{name}/import

Bulk-inserts nodes and edges. The target graph must already exist. Node IDs in
the import payload are stable identifiers used only to wire up edges; the
engine assigns new internal IDs.

**Request body**

```json
{
  "nodes": [
    {
      "id": "n1",
      "labels": ["Warehouse"],
      "properties": { "name": "Seattle-HQ", "capacity": 5000 }
    },
    {
      "id": "n2",
      "labels": ["Warehouse"],
      "properties": { "name": "Portland-DC", "capacity": 3200 }
    }
  ],
  "edges": [
    {
      "source": "n1",
      "target": "n2",
      "type": "SUPPLIES",
      "properties": { "frequency": "daily" }
    }
  ]
}
```

`id` on nodes is optional. When omitted, edges must reference nodes by the
engine-assigned numeric ID returned from a previous `POST /nodes` call.

**Response 200**

```json
{
  "nodesCreated": 2,
  "edgesCreated": 1
}
```

**Response 400** — an edge references a node ID that was not in the import
payload and does not exist in the graph

## Rate Limits

Rate limits are enforced by the .NET BFF only. Direct calls to gql-api on port
3001 are not rate-limited.

| Endpoint pattern | Policy | Limit |
|---|---|---|
| `POST /api/graphs/{name}/query` | sliding window | 100 req / 60 s per IP |
| `POST /api/graphs/{name}/mutate` | sliding window | 100 req / 60 s per IP |

When the limit is exceeded the BFF returns `429 Too Many Requests`. Up to 10
requests are queued; requests beyond the queue limit are rejected immediately.

The `X-Request-Id` header is present on every BFF response and can be used
to correlate requests in logs.
