# API Contracts

Exhaustive inventory of all API entry points across SedanDelivery's three network interfaces:
REST API (gql-api), Bolt Protocol (sedandelivery-bolt), and Redis Module commands (gql-module).

## API Surface Diagram

### Mermaid

```mermaid
flowchart TD
    subgraph rest_api["gql-api — REST API :3001"]
        subgraph health_routes["Health"]
            GET_health["GET /health"]
            GET_metrics["GET /metrics"]
        end

        subgraph graph_mgmt["Graph Management"]
            GET_graphs["GET /graphs"]
            POST_graphs["POST /graphs"]
            DEL_graph["DELETE /graphs/:name"]
            GET_stats["GET /graphs/:name/stats"]
        end

        subgraph node_ops["Node Operations"]
            GET_nodes["GET /graphs/:name/nodes"]
            POST_node["POST /graphs/:name/nodes"]
            GET_node["GET /graphs/:name/nodes/:id"]
            PUT_node["PUT /graphs/:name/nodes/:id"]
            DEL_node["DELETE /graphs/:name/nodes/:id"]
        end

        subgraph edge_ops["Edge Operations"]
            GET_edges["GET /graphs/:name/edges"]
            POST_edge["POST /graphs/:name/edges"]
            GET_edge["GET /graphs/:name/edges/:id"]
            PUT_edge["PUT /graphs/:name/edges/:id"]
            DEL_edge["DELETE /graphs/:name/edges/:id"]
        end

        subgraph cypher_ops["Cypher Query"]
            POST_query["POST /graphs/:name/query"]
            POST_mutate["POST /graphs/:name/mutate"]
            POST_explain["POST /graphs/:name/explain"]
            POST_profile["POST /graphs/:name/profile"]
        end

        subgraph schema_ops["Schema & Traversal"]
            GET_schema["GET /graphs/:name/schema"]
            GET_neighbors["GET /graphs/:name/neighbors/:nid"]
        end

        subgraph import_export["Import/Export"]
            GET_export["GET /graphs/:name/export"]
            GET_data["GET /graphs/:name/data"]
            POST_import["POST /graphs/:name/import"]
        end

        subgraph admin_ops["Admin"]
            POST_audit["POST /admin/graphs/:name/audit"]
        end
    end

    subgraph middleware["REST Middleware Stack"]
        mw1["1. ConcurrencyLimiter (100)"]
        mw2["2. TraceLayer (structured logging)"]
        mw3["3. CorsLayer (origin allowlist)"]
        mw4["4. HTTP Metrics (Prometheus)"]
        mw5["5. Rate Limiting (100 rps/IP)"]
        mw6["6. Entra ID Auth (JWT)"]
    end

    subgraph bolt_proto["sedandelivery-bolt — Bolt v4.4 :7687"]
        HELLO["HELLO (0x01) — auth + version"]
        LOGON["LOGON (0x6A) — re-auth"]
        RUN["RUN (0x10) — execute Cypher"]
        PULL["PULL (0x3F) — fetch rows"]
        DISCARD["DISCARD (0x2F) — discard results"]
        BEGIN_TX["BEGIN (0x11) — start tx"]
        COMMIT_TX["COMMIT (0x12) — commit tx"]
        ROLLBACK_TX["ROLLBACK (0x13) — rollback tx"]
        RESET["RESET (0x0F) — clear session"]
        GOODBYE["GOODBYE (0x02) — close"]
    end

    subgraph redis_cmds["gql-module — Redis Commands"]
        GQL_QUERY["GQL.QUERY graph cypher"]
        GQL_MUTATE["GQL.MUTATE graph cypher"]
        GQL_CREATE["GQL.CREATE-GRAPH graph"]
        GQL_DELETE["GQL.DELETE-GRAPH graph"]
        GQL_LIST["GQL.LIST-GRAPHS"]
        GQL_EXPLAIN["GQL.EXPLAIN graph cypher"]
    end
```

See [routing.mmd](routing.mmd) and [routing.dot](routing.dot) for diagram sources.

## REST API Route Inventory

| Method | Path | Handler | Auth | Request Body | Response |
|--------|------|---------|------|-------------|----------|
| GET | /health | health_handler | None | — | 200 OK |
| GET | /metrics | metrics | None | — | Prometheus text |
| GET | /graphs | list_graphs | Entra JWT | — | JSON array |
| POST | /graphs | create_graph | Entra JWT | `{ name }` | 201 Created |
| DELETE | /graphs/:name | delete_graph | Entra JWT | — | 200 OK |
| GET | /graphs/:name/stats | graph_stats | Entra JWT | — | JSON stats |
| GET | /graphs/:name/nodes | list_nodes | Entra JWT | — | JSON array |
| POST | /graphs/:name/nodes | create_node | Entra JWT | `{ labels, properties }` | JSON node |
| GET | /graphs/:name/nodes/:id | get_node | Entra JWT | — | JSON node |
| PUT | /graphs/:name/nodes/:id | update_node | Entra JWT | `{ properties }` | JSON node |
| DELETE | /graphs/:name/nodes/:id | delete_node | Entra JWT | — | 200 OK |
| GET | /graphs/:name/edges | list_edges | Entra JWT | — | JSON array |
| POST | /graphs/:name/edges | create_edge | Entra JWT | `{ src, dst, type, props }` | JSON edge |
| GET | /graphs/:name/edges/:id | get_edge | Entra JWT | — | JSON edge |
| PUT | /graphs/:name/edges/:id | update_edge | Entra JWT | `{ properties }` | JSON edge |
| DELETE | /graphs/:name/edges/:id | delete_edge | Entra JWT | — | 200 OK |
| POST | /graphs/:name/query | query_handler | Entra JWT | `{ query, params }` | QueryResult |
| POST | /graphs/:name/mutate | mutate_handler | Entra JWT | `{ query, params }` | QueryResult |
| POST | /graphs/:name/explain | explain_handler | Entra JWT | `{ query }` | Plan text |
| POST | /graphs/:name/profile | profile_handler | Entra JWT | `{ query }` | Profile data |
| GET | /graphs/:name/schema | schema_handler | Entra JWT | — | JSON schema |
| GET | /graphs/:name/neighbors/:nid | neighbors_handler | Entra JWT | — | JSON neighbors |
| GET | /graphs/:name/export | export_handler | Entra JWT | — | JSON full graph |
| GET | /graphs/:name/data | export_handler | Entra JWT | — | JSON data only |
| POST | /graphs/:name/import | import_handler | Entra JWT | JSON graph data | 200 OK |
| POST | /admin/graphs/:name/audit | audit_handler | Entra JWT | — | Audit report |

## Bolt Protocol Messages

| Message | Tag | Handler File | Purpose |
|---------|-----|-------------|---------|
| HELLO | 0x01 | handler/hello.rs:226 | Version negotiation + auth |
| LOGON | 0x6A | handler/hello.rs:264 | Re-authenticate session |
| RUN | 0x10 | handler/run.rs:535 | Execute Cypher query |
| PULL | 0x3F | handler/pull.rs:18 | Fetch result rows |
| DISCARD | 0x2F | handler/pull.rs:217 | Discard pending results |
| BEGIN | 0x11 | handler/transaction.rs:19 | Start explicit transaction |
| COMMIT | 0x12 | handler/transaction.rs:66 | Commit transaction |
| ROLLBACK | 0x13 | handler/transaction.rs:98 | Rollback transaction |
| RESET | 0x0F | handler/transaction.rs:121 | Clear session state |
| GOODBYE | 0x02 | — | Close connection |

## Redis Module Commands

| Command | Args | Handler | Read/Write |
|---------|------|---------|------------|
| GQL.QUERY | `<graph> <cypher>` | cmd_gql_query (lib.rs:193) | Read-only |
| GQL.MUTATE | `<graph> <cypher>` | cmd_gql_mutate (lib.rs:243) | Write |
| GQL.CREATE-GRAPH | `<graph>` | cmd_gql_create_graph (lib.rs:302) | Write |
| GQL.DELETE-GRAPH | `<graph>` | cmd_gql_delete_graph (lib.rs:334) | Write |
| GQL.LIST-GRAPHS | — | cmd_gql_list_graphs (lib.rs:365) | Read-only |
| GQL.EXPLAIN | `<graph> <cypher>` | cmd_gql_explain (lib.rs:385) | Read-only |

## Auth Configuration

| Interface | Auth Method | Config |
|-----------|------------|--------|
| REST API | Microsoft Entra ID JWT | `GQL_API_ENTRA_TENANT_ID`, `GQL_API_ENTRA_CLIENT_ID` |
| Bolt | Password-based | `BOLT_PASSWORD` |
| Redis Module | Redis ACL / `requirepass` | `VALKEY_PASSWORD` |
