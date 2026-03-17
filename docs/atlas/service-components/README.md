# Service Component Architecture

Per-crate internal module structure for the 5 major SedanDelivery crates.
Each crate with 3+ internal modules gets its own diagram.

## gql-core — Query Engine Core

```mermaid
graph TD
    subgraph gql_core["gql-core"]
        subgraph parser_mod["parser/"]
            lexer["lexer/\ntokens, scan"]
            parser_inner["parser/\nclauses, expressions,\npatterns, subqueries"]
        end

        subgraph planner_mod["planner/"]
            validator["validator/\nexpression, scope,\nsemantic checks"]
            planner_inner["planner/\nmatch, mutation,\nresult planners"]
            operators["mod.rs\n28+ physical operators:\nNodeScan, IndexScan,\nExpand, Filter,\nAggregate, Create,\nMerge, ShortestPath..."]
        end

        subgraph executor_mod["executor/"]
            engine["engine/\nscan, mutations,\nhelpers, eval,\nproduce, shaping,\njoins, aggregation"]
            filter["filter/\nbuiltins (60+ functions),\noperators, temporal,\ncollections"]
            traversal["traversal.rs\nBFS, shortest path"]
            budget["budget.rs\ntimeout, row-limit"]
        end

        subgraph storage_mod["storage/"]
            graphstore["mod.rs\nGraphStore trait"]
            mem_store["mem/\nInMemoryGraphStore\nnodes, edges,\nindexing, query"]
            wal["wal.rs\nwrite-ahead log"]
        end

        subgraph types_mod["types/"]
            value["value/\nValue enum\ndisplay, convert,\ncompare"]
        end

        ast["ast/\nCypher AST types"]
        config["config.rs\nenv var config"]
        plan_cache["plan_cache.rs\nhash-keyed cache"]
    end

    lexer -->|"tokens"| parser_inner
    parser_inner -->|"AST"| validator
    validator -->|"valid AST"| planner_inner
    planner_inner -->|"operators"| operators
    operators -->|"op tree"| engine
    engine -->|"calls"| graphstore
    engine -->|"calls"| filter
    engine -->|"calls"| traversal
    engine -->|"checks"| budget
    graphstore -.->|"impl"| mem_store
    parser_inner -->|"uses"| ast
    engine -->|"produces"| value
```

See [gql-core.mmd](gql-core.mmd) for the Mermaid source.

## gql-redis-store — Redis Storage Backend

```mermaid
graph TD
    subgraph gql_redis["gql-redis-store"]
        lib["lib.rs\nRedisGraphStore\nGraphStore impl"]
        create_store_mod["create_store.rs\nBackend selection\nfrom env vars"]
        connection["connection.rs\nr2d2 pool, replica\nrouting, stats"]
        reads["reads.rs\nRedis read commands"]
        writes_mod["writes/\nnodes.rs, edges.rs,\ngraphs.rs, helpers.rs"]
        indexing["indexing.rs\nProperty index mgmt"]
        range_mod["range.rs\nRange query support"]
        fanout["fanout.rs\nFanoutStore\nmulti-backend writes"]
        keys["keys.rs\nRedis key schema\nG:graph:... patterns"]
    end

    create_store_mod -->|"creates"| lib
    create_store_mod -->|"creates"| fanout
    lib -->|"uses"| connection
    lib -->|"uses"| reads
    lib -->|"uses"| writes_mod
    lib -->|"uses"| indexing
    lib -->|"uses"| range_mod
    lib -->|"uses"| keys
    reads -->|"uses"| keys
    writes_mod -->|"uses"| keys
    fanout -->|"wraps"| lib
```

See [gql-redis-store.mmd](gql-redis-store.mmd) for the Mermaid source.

## sedandelivery-bolt — Bolt Protocol Server

```mermaid
graph TD
    subgraph bolt_crate["sedandelivery-bolt"]
        main_rs["main.rs\nTCP accept loop\nTLS + WebSocket"]
        
        subgraph connection_mod["connection/"]
            conn_mgr["Connection manager\nper-connection tasks\nsemaphore gating"]
        end

        subgraph protocol_mod["protocol/"]
            handler_mod["handler/\nhello, run, pull,\ntransaction, explain"]
            state["state.rs\nSession state machine"]
        end

        subgraph bridge_mod["bridge/"]
            bridge_exec["executor.rs\nBolt-to-GQL translation"]
            bridge_types["types.rs, temporal.rs,\nspatial.rs\nPackStream ↔ Value"]
        end

        subgraph message_mod["message/"]
            codec["codec.rs\nBolt binary framing"]
        end

        subgraph packstream_mod["packstream/"]
            encoder["encoder.rs"]
            decoder["decoder.rs"]
            ps_value["value.rs\nPackStream values"]
        end

        health["health.rs\nHTTP health endpoint"]
        metrics["metrics.rs\nPrometheus metrics"]
        bookmark["bookmark.rs\nRead-after-write"]
    end

    main_rs -->|"spawns"| conn_mgr
    conn_mgr -->|"dispatches"| handler_mod
    handler_mod -->|"uses"| state
    handler_mod -->|"calls"| bridge_exec
    bridge_exec -->|"converts"| bridge_types
    handler_mod -->|"reads/writes"| codec
    codec -->|"uses"| encoder
    codec -->|"uses"| decoder
    encoder -->|"produces"| ps_value
    decoder -->|"produces"| ps_value
    main_rs -->|"serves"| health
```

See [sedandelivery-bolt.mmd](sedandelivery-bolt.mmd) for the Mermaid source.

## gql-api — REST API Server

```mermaid
graph TD
    subgraph api_crate["gql-api"]
        api_main["main.rs\naxum server setup"]
        
        subgraph routes_mod["routes/"]
            r_graphs["graphs.rs"]
            r_nodes["nodes.rs"]
            r_edges["edges.rs"]
            r_query["query.rs"]
            r_schema["schema.rs"]
            r_neighbors["neighbors.rs"]
            r_export["export_import.rs"]
            r_admin["admin_audit.rs"]
        end

        auth["auth.rs\nEntra ID JWT\nJWKS cache"]
        rate_limit["rate_limit.rs\nPer-IP token bucket"]
        metrics_api["metrics.rs\nPrometheus +\nslow query log"]
        tls_mod["tls.rs\nTLS config"]
        serialize["serialize.rs\nValue to JSON"]
    end

    api_main -->|"mounts"| routes_mod
    api_main -->|"applies"| auth
    api_main -->|"applies"| rate_limit
    api_main -->|"applies"| metrics_api
    api_main -->|"configures"| tls_mod
    r_query -->|"uses"| serialize
    r_nodes -->|"uses"| serialize
    r_edges -->|"uses"| serialize
```

See [gql-api.mmd](gql-api.mmd) for the Mermaid source.

## gql-fabric-store — Fabric Storage Backend

```mermaid
graph TD
    subgraph fabric_crate["gql-fabric-store"]
        fabric_lib["lib.rs\nFabricGraphStore\n+ connection pool"]
        
        subgraph store_mod["store/"]
            store_nodes["nodes.rs"]
            store_edges["edges.rs"]
            store_admin["admin.rs"]
        end

        fabric_auth["auth.rs\nAzure AD token\nsingle-flight refresh"]
        parsing["parsing.rs\nFabric JSON response"]
        sanitize["sanitize.rs\nCypher injection\nprevention"]
    end

    fabric_lib -->|"delegates"| store_mod
    fabric_lib -->|"uses"| fabric_auth
    store_mod -->|"uses"| parsing
    store_mod -->|"uses"| sanitize
```

See [gql-fabric-store.mmd](gql-fabric-store.mmd) for the Mermaid source.
