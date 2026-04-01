# Data Flow

Data transformation chains from API entry to storage and back for SedanDelivery.

## Query Pipeline Flow

All Cypher queries follow the same pipeline regardless of entry point (REST, Bolt, Redis module, CLI).

![Query Pipeline](dataflow.svg)

<details>
<summary>Mermaid source</summary>

```mermaid
flowchart LR
    subgraph entry["Entry Points"]
        rest["REST API\nPOST /query"]
        bolt["Bolt RUN\nmessage 0x10"]
        redis["GQL.QUERY\nRedis command"]
        cli["gql-cli\nREPL input"]
    end

    subgraph pipeline["Query Pipeline — cypher-core"]
        lexer["Lexer\nparser::lexer\nCypher text → tokens"]
        parser["Parser\nparser::parser\nTokens → AST"]
        validator["Validator\nplanner::validator\nSemantic checks"]
        planner["Planner\nplanner::QueryPlanner\nAST → operator tree\n28 physical operators"]
        executor["Executor\nexecutor::QueryExecutor\nOperator tree → results"]
        cache["Plan Cache\nplan_cache.rs\nHash-keyed, clear-on-capacity"]
    end

    subgraph storage["Storage — GraphStore trait"]
        mem["InMemoryGraphStore\nArc-based snapshots"]
        persistent["PersistentGraphStore\nWAL-based disk persistence"]
        cached["CachedGraphStore\nWrite-through cache"]
        redis_store["RedisGraphStore\nr2d2 pool + pipeline batch"]
        module_store["ModuleRedisStore\nIn-process ctx.call()"]
        fabric_store["FabricGraphStore\nREST + openCypher"]
        fanout["FanoutStore\nWrite to all backends"]
    end

    subgraph output["Output"]
        result["QueryResult\ncolumns + rows\nValue enum"]
    end

    rest --> lexer
    bolt --> lexer
    redis --> lexer
    cli --> lexer

    lexer --> parser
    parser --> cache
    cache -->|"miss"| validator
    cache -->|"hit"| executor
    validator --> planner
    planner --> executor
    executor --> mem
    executor --> persistent
    executor --> cached
    executor --> redis_store
    executor --> module_store
    executor --> fabric_store
    executor --> fanout
    mem --> result
    persistent --> result
    cached --> result
    redis_store --> result
    module_store --> result
    fabric_store --> result
    fanout --> result
```

</details>

See [dataflow.mmd](dataflow.mmd) and [dataflow.dot](dataflow.dot) for diagram sources.

## Mutation Data Flow

```mermaid
flowchart LR
    req["Cypher mutation\nCREATE / MERGE / SET\nDELETE / REMOVE"] --> parse["Parse + Plan\nMutationOp operators"]
    parse --> exec["Execute mutation\nagainst GraphStore"]
    exec --> write["GraphStore write\ncreate_node / create_edge\nset_node_property\ndelete_node / delete_edge"]
    write --> changelog["Changelog emission\n__sd2:changelog:graph\nRedis Stream XADD"]
    changelog --> sync["Fabric Sync sidecar\nXREADGROUP batch"]
    sync --> fabric["Fabric Graph DB\nHTTPS openCypher"]
    write --> response["QueryResult\nstats: nodes_created,\nedges_created, etc."]
```

## REST CRUD Data Flow

```mermaid
flowchart LR
    http_req["HTTP Request\nJSON body"] --> middleware["Middleware\nCORS → Rate Limit\n→ Entra Auth → Metrics"]
    middleware --> handler["Route Handler\naxum extractor"]
    handler --> deserialize["Deserialize\nJSON → Rust structs"]
    deserialize --> store_op["GraphStore method\ncreate_node / get_node\nupdate / delete"]
    store_op --> serialize["Serialize\nValue → JSON\nserialize.rs"]
    serialize --> http_resp["HTTP Response\nJSON body"]
```

## Redis Key Schema (Data at Rest)

| Key Pattern | Type | Contents |
|-------------|------|----------|
| `G:{graph}:node:{id}` | HASH | labels (JSON), props (JSON) |
| `G:{graph}:edge:{id}` | HASH | src, dst, rel_type, props (JSON) |
| `G:{graph}:label:{label}` | SET | Node IDs with that label |
| `G:{graph}:out:{node_id}` | SET | `edge_id:neighbor_id:rel_type` adjacency |
| `G:{graph}:in:{node_id}` | SET | Reverse adjacency |
| `G:{graph}:_nextid` | STRING | Auto-increment counter |
| `G:{graph}:_labels` | SET | All label names in graph |
| `G:{graph}:_reltypes` | SET | All relationship type names |
| `G:{graph}:_propidx:{label}:{prop}` | SET | Property index entries |
| `G:{graph}:write_lock` | STRING | Distributed write lock |
| `__sd2_registry:graphs` | SET | All graph names |
| `__sd2:changelog:{graph}` | STREAM | Mutation changelog for Fabric sync |

## Value Type System

The `Value` enum in `cypher-core::types::Value` is the unified value type:

| Variant | Rust Type | Use |
|---------|-----------|-----|
| Null | — | Missing values |
| Bool | bool | Boolean properties |
| Integer | i64 | Integer properties |
| Float | f64 | Floating point |
| String | Box\<str\> | String properties |
| List | Vec\<Value\> | List values |
| Map | IndexMap | Map/dictionary values |
| Node | NodeData | Full node with labels + props |
| Relationship | EdgeData | Full edge with type + props |
| Path | PathData | Graph path result |
| Date/Time/DateTime | jiff types | Temporal values |
| Duration | jiff Duration | Duration values |
| Point2D/Point3D | f64 coords | Spatial values |
