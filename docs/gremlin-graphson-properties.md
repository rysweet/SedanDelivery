# Gremlin GraphSON Property Serialization

The Gremlin text-eval server (`sedandelivery-gremlin-server`) resolves vertex
and edge properties from the graph store when converting query results to
GraphSON v3 output. This means clients always receive the full property map
for every vertex and edge, matching the behavior expected by TinkerPop
drivers and tooling.

## How It Works

When `execute_gremlin_text` returns results, each `Value::Node`, `Value::Edge`,
and `Value::Path` is converted to a `GremlinValue` via the internal
`value_to_gremlin` function. This function looks up the entity in the
`GraphStore` to populate labels, relationship metadata, and properties:

```
Value::Node(nid) ──► store.get_node(graph, nid) ──► GremlinValue::Vertex {
                                                        id, label, properties
                                                    }

Value::Edge(eid) ──► store.get_edge(graph, eid) ──► GremlinValue::Edge {
                                                        id, label, in_v, out_v,
                                                        properties
                                                    }

Value::Path { nodes, edges } ──► interleaved lookups ──► GremlinValue::Path {
                                                             labels, objects
                                                         }
```

### Vertex Resolution

For `Value::Node(nid)`, the server calls `store.get_node(graph, nid)`:

| Store result       | Label                          | Properties                    |
|--------------------|--------------------------------|-------------------------------|
| `Ok(Some(data))`   | `data.labels.join(":")`        | Converted from `data.properties` |
| `Ok(Some(data))` with empty labels | `"vertex"` (fallback) | Converted from `data.properties` |
| `Ok(None)`         | `"vertex"`                     | Empty                         |
| `Err(_)`           | `"vertex"`                     | Empty                         |

### Edge Resolution

For `Value::Edge(eid)`, the server calls `store.get_edge(graph, eid)`:

| Store result       | Label          | in_v         | out_v        | Properties                    |
|--------------------|----------------|--------------|--------------|-------------------------------|
| `Ok(Some(data))`   | `data.rel_type`| `data.dst.0` | `data.src.0` | Converted from `data.properties` |
| `Ok(None)`         | `"edge"`       | `0`          | `0`          | Empty                         |
| `Err(_)`           | `"edge"`       | `0`          | `0`          | Empty                         |

### Path Resolution

For `Value::Path { nodes, edges }`, the server interleaves vertex and edge
lookups to produce the full path. The `objects` array alternates between
vertices and edges: `[v0, e0, v1, e1, ..., vN]`. Each element is resolved
from the store using the same vertex/edge rules above. The `labels` field
is a `Vec<Vec<String>>` with one empty inner vec per object (matching
TinkerPop's per-step label convention).

If a path contains `N` nodes, it contains `N-1` edges. When the edge list
is shorter than expected, remaining positions are filled with vertex-only
entries.

### Property Conversion

Entity properties are stored as `IndexMap<String, Value>` on `NodeData` and
`EdgeData`. Each entry is recursively converted to `GremlinValue` via
`value_to_gremlin`, which handles scalar types directly and resolves any
nested entity references (Node, Edge, Path) from the store. In practice,
property values are scalars, lists, or maps — but the recursive design
ensures correctness for all `Value` variants.

The helper function `convert_props` handles this conversion:

```rust
// Pseudocode — actual implementation is internal
fn convert_props(
    props: &IndexMap<String, Value>,
    store: &dyn GraphStore,
    graph: &str,
) -> IndexMap<String, GremlinValue>
```

## GraphSON v3 Output Format

The resolved `GremlinValue` is serialized to GraphSON v3 JSON by
`gremlin_core::graphson::to_graphson`. The output for a vertex with
properties looks like:

```json
{
  "@type": "g:Vertex",
  "@value": {
    "id": { "@type": "g:Int64", "@value": 42 },
    "label": "person",
    "properties": {
      "name": "Alice",
      "age": { "@type": "g:Int64", "@value": 30 }
    }
  }
}
```

> **Note:** Property values are recursively encoded via `to_graphson`:
> strings become plain JSON strings (not `g:String`), integers use `g:Int64`,
> floats use `g:Double`. Our encoding places each property value directly in
> the `properties` map — this is a simplification compared to the full
> TinkerPop GraphSON v3 spec, which wraps vertex properties in
> `g:VertexProperty` objects. Standard TinkerPop clients handle both forms.

An edge with properties:

```json
{
  "@type": "g:Edge",
  "@value": {
    "id": { "@type": "g:Int64", "@value": 7 },
    "label": "KNOWS",
    "inV": { "@type": "g:Int64", "@value": 2 },
    "outV": { "@type": "g:Int64", "@value": 1 },
    "properties": {
      "since": { "@type": "g:Int64", "@value": 2020 }
    }
  }
}
```

## Graceful Fallback

If a vertex or edge ID no longer exists in the store (e.g., it was deleted
between query execution and result serialization), the server returns a
minimal stub with the correct ID but default label and empty properties.
No error is raised — this is a best-effort serialization path, not a
correctness-critical query path. Store errors are silently absorbed and
logged at `debug` level.

This design ensures that:

1. Clients never receive a serialization error for a successfully executed query
2. Race conditions between concurrent mutations and reads do not crash the
   response pipeline
3. The output always conforms to GraphSON v3 structure

## API Reference

### `execute_gremlin_text`

```rust
pub fn execute_gremlin_text(
    store: &dyn GraphStore,
    graph: &str,
    gremlin: &str,
    bindings: &HashMap<String, serde_json::Value>,
) -> Result<Vec<serde_json::Value>, GqlError>
```

The public API is unchanged. The `store` and `graph` parameters that were
already passed to this function are now threaded through to
`value_to_gremlin` for entity resolution. No new parameters, configuration,
or feature flags are required.

### `value_to_gremlin` (internal)

```rust
fn value_to_gremlin(
    val: &Value,
    store: &dyn GraphStore,
    graph: &str,
) -> GremlinValue
```

Converts a cypher-core `Value` to a `GremlinValue`. Scalar types (null,
bool, int, float, string) are converted directly. Container types (list,
map) recurse. Entity types (node, edge, path) resolve data from the store.

## Configuration

No configuration is required. Property serialization is always enabled.
There is no feature flag to disable it — the prior behavior of returning
empty properties was a bug, not a feature.

## Examples

### Gremlin Console

```groovy
// Add a vertex with properties
gremlin> g.addV('person').property('name', 'Alice').property('age', 30)
==>v[1]

// Query it back — properties are included
gremlin> g.V(1).valueMap(true)
==> {id: 1, label: person, name: [Alice], age: [30]}

// Traverse and see edge properties
gremlin> g.V(1).addE('knows').to(g.addV('person').property('name', 'Bob')).property('since', 2020)
==>e[3][1-knows->2]

gremlin> g.E(3).valueMap(true)
==> {id: 3, label: knows, since: [2020]}

// Path traversal includes properties on all elements
gremlin> g.V(1).outE('knows').inV().path()
==> path[v[1], e[3][1-knows->2], v[2]]
```

### Python (gremlinpython)

```python
from gremlin_python.driver.client import Client

client = Client('ws://localhost:8182/gremlin', 'g')

# Vertex with properties
result = client.submit("g.V().has('name', 'Alice').elementMap()")
for r in result:
    print(r)
    # {'id': 1, 'label': 'person', 'name': 'Alice', 'age': 30}
```

### Java (TinkerPop Driver)

```java
Cluster cluster = Cluster.build("localhost").port(8182).create();
Client client = cluster.connect();

ResultSet rs = client.submit("g.V().has('name','Alice').elementMap()");
for (Result r : rs) {
    Map<Object, Object> map = r.get(LinkedHashMap.class);
    System.out.println(map);
    // {id=1, label=person, name=Alice, age=30}
}
```

## Testing

The property serialization is covered by unit tests in
`sedandelivery-gremlin-server/src/executor.rs`:

| Test | What it verifies |
|------|-----------------|
| `value_to_gremlin_node` | Vertex with real label and properties from store |
| `value_to_gremlin_edge` | Edge with rel_type, src/dst, and properties from store |
| `value_to_gremlin_path` | Path interleaving nodes and edges, each with properties |
| `value_to_gremlin_node_not_found` | Graceful fallback when node ID doesn't exist |
| `value_to_gremlin_edge_not_found` | Graceful fallback when edge ID doesn't exist |
| `value_to_gremlin_multi_props` | Vertex with string, int, and float properties |

Run the tests:

```bash
cargo test -p sedandelivery-gremlin-server --lib
```

## Related Documentation

- [Gremlin Implementation Plan](gremlin-implementation-plan) — Architecture and execution pipeline
- [Gremlin Error Handling](gremlin-error-handling) — Error guarantees across the pipeline
- [Architecture Overview](architecture) — Crate map and request flows
