# User Journey Scenarios

5 key end-to-end paths traced through the SedanDelivery system, from client entry
to final outcome.

## Journey 1: Bolt Client Executes a Cypher Query

```mermaid
sequenceDiagram
    actor User as Neo4j Driver Client
    participant TCP as TCP Listener<br/>main.rs
    participant Conn as Connection Manager<br/>connection/
    participant Handler as Protocol Handler<br/>handler/
    participant Bridge as Bridge<br/>bridge/executor.rs
    participant QE as QueryExecutor<br/>gql-core
    participant Store as GraphStore<br/>(Redis or InMemory)

    User->>TCP: TCP connect :7687
    TCP->>Conn: spawn per-connection task
    User->>Conn: Bolt handshake (magic + version)
    Conn-->>User: version agreement (v4.4)

    User->>Handler: HELLO {user_agent, auth}
    Handler->>Handler: validate password (BOLT_PASSWORD)
    Handler-->>User: SUCCESS {server, connection_id}

    User->>Handler: RUN "MATCH (p:Person) RETURN p" {} {}
    Handler->>Bridge: translate Bolt → GQL
    Bridge->>QE: parse_query → plan → execute
    QE->>Store: find_nodes_by_label("Person")
    Store-->>QE: Vec<NodeId>
    QE->>Store: get_nodes_batch(ids)
    Store-->>QE: Vec<NodeData>
    QE-->>Bridge: QueryResult {columns, rows}
    Bridge-->>Handler: PackStream SUCCESS {fields}
    Handler-->>User: SUCCESS {fields: ["p"], qid: 0}

    User->>Handler: PULL {n: -1}
    Handler-->>User: RECORD [node_data] (repeated)
    Handler-->>User: SUCCESS {bookmark, type: "r"}

    User->>Handler: GOODBYE
    Conn-->>TCP: connection closed
```

See [journey-bolt-query.mmd](journey-bolt-query.mmd).

## Journey 2: REST Client Creates a Graph and Adds Data

```mermaid
sequenceDiagram
    actor Client as REST Client
    participant MW as Middleware Stack<br/>CORS, rate limit, auth
    participant Router as axum Router
    participant GH as graphs.rs handler
    participant NH as nodes.rs handler
    participant EH as edges.rs handler
    participant Store as GraphStore

    Client->>MW: POST /graphs {name: "social"}
    MW->>MW: CORS check, rate limit, JWT validate
    MW->>Router: route to handler
    Router->>GH: create_graph("social")
    GH->>Store: create_graph("social")
    Store-->>GH: Ok
    GH-->>Client: 201 Created

    Client->>MW: POST /graphs/social/nodes {labels: ["Person"], properties: {name: "Alice"}}
    MW->>Router: route to handler
    Router->>NH: create_node("social", labels, props)
    NH->>Store: create_node("social", ["Person"], {name: "Alice"})
    Store-->>NH: NodeId(1)
    NH-->>Client: 201 {id: 1, labels: ["Person"], properties: {name: "Alice"}}

    Client->>MW: POST /graphs/social/nodes {labels: ["Person"], properties: {name: "Bob"}}
    MW->>Router: route
    Router->>NH: create_node
    NH->>Store: create_node
    Store-->>NH: NodeId(2)
    NH-->>Client: 201 {id: 2, ...}

    Client->>MW: POST /graphs/social/edges {src: 1, dst: 2, type: "KNOWS", properties: {since: 2020}}
    MW->>Router: route
    Router->>EH: create_edge("social", 1, 2, "KNOWS", props)
    EH->>Store: create_edge
    Store-->>EH: EdgeId(1)
    EH-->>Client: 201 {id: 1, src: 1, dst: 2, type: "KNOWS", ...}
```

See [journey-rest-crud.mmd](journey-rest-crud.mmd).

## Journey 3: Redis Module Processes GQL.QUERY

```mermaid
sequenceDiagram
    actor Client as Redis Client
    participant Redis as Valkey 8.1<br/>main thread
    participant Module as gql-module<br/>libgql_module.so
    participant QE as QueryExecutor<br/>gql-core
    participant Store as InMemoryGraphStore

    Client->>Redis: GQL.QUERY social "MATCH (n) RETURN n.name"
    Redis->>Module: cmd_gql_query(ctx, args)
    Module->>Module: catch_unwind (panic safety)
    Module->>Module: validate: no write clauses (CREATE/SET/DELETE/MERGE)
    Module->>QE: parse_query → plan → execute
    QE->>Store: find_all_nodes("social")
    Store-->>QE: node data
    QE-->>Module: QueryResult {columns: ["n.name"], rows}
    Module-->>Redis: RESP array of arrays
    Redis-->>Client: [[name1], [name2], ...]
```

See [journey-redis-query.mmd](journey-redis-query.mmd).

## Journey 4: Fabric Sync Replicates a Mutation

```mermaid
sequenceDiagram
    actor Client as Bolt Driver
    participant Bolt as sedandelivery-bolt
    participant QE as QueryExecutor
    participant Redis as RedisGraphStore
    participant Stream as Redis Stream<br/>__sd2:changelog:social
    participant Sync as sedandelivery-fabric-sync
    participant Fabric as Microsoft Fabric<br/>Graph Database

    Client->>Bolt: RUN "CREATE (:Person {name: 'Eve'})" {} {db: "social"}
    Bolt->>QE: parse → plan → execute
    QE->>Redis: create_node("social", ["Person"], {name: "Eve"})
    Redis-->>QE: NodeId(42)
    Redis->>Stream: XADD __sd2:changelog:social {op: CREATE_NODE, id: 42, ...}
    QE-->>Bolt: QueryResult {stats: nodes_created: 1}
    Bolt-->>Client: SUCCESS + PULL → SUCCESS {bookmark}

    Note over Sync: Polling loop (SD2_SYNC_BLOCK_MS=1000)
    Sync->>Stream: XREADGROUP GROUP fabric-sync consumer-1 BLOCK 1000
    Stream-->>Sync: [{op: CREATE_NODE, id: 42, labels: ["Person"], props: {name: "Eve"}}]
    Sync->>Fabric: POST /openCypher "CREATE (:Person {name: 'Eve'})"
    Fabric-->>Sync: 200 OK
    Sync->>Stream: XACK __sd2:changelog:social fabric-sync msg-id

    Note over Sync: On failure after SD2_SYNC_MAX_RETRIES
    Sync->>Sync: XADD __sd2:changelog:dlq (dead-letter queue)
```

See [journey-fabric-sync.mmd](journey-fabric-sync.mmd).

## Journey 5: Explicit Transaction with Snapshot Isolation

```mermaid
sequenceDiagram
    actor Client as Neo4j Driver
    participant Handler as Protocol Handler
    participant Bridge as Bridge Executor
    participant QE as QueryExecutor
    participant Store as GraphStore

    Client->>Handler: BEGIN {db: "social"}
    Handler->>Store: tx_snapshot(["social"])
    Store-->>Handler: snapshot (Arc clone / Redis snapshot)
    Handler-->>Client: SUCCESS {}

    Client->>Handler: RUN "CREATE (:Task {title: 'Build Atlas'})" {} {}
    Handler->>Bridge: execute on snapshot
    Bridge->>QE: parse → plan → execute (against snapshot)
    QE->>Store: create_node on snapshot
    Store-->>QE: NodeId
    Handler-->>Client: SUCCESS {fields: [], qid: 0}
    Client->>Handler: PULL {n: -1}
    Handler-->>Client: SUCCESS {stats: {nodes_created: 1}}

    Client->>Handler: RUN "MATCH (t:Task) RETURN t.title" {} {}
    Handler->>Bridge: execute on same snapshot
    Bridge->>QE: parse → plan → execute
    QE->>Store: read from snapshot (sees uncommitted node)
    Handler-->>Client: SUCCESS + RECORD ["Build Atlas"] + SUCCESS

    Client->>Handler: COMMIT
    Handler->>Store: tx_commit(snapshot)
    Store-->>Handler: Ok (writes applied atomically)
    Handler-->>Client: SUCCESS {bookmark: "FB:..."}

    Note over Store: Other connections now see the committed data
```

See [journey-explicit-tx.mmd](journey-explicit-tx.mmd).
