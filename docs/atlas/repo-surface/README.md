# Repository Surface

Overview of the SedanDelivery repository structure — a multi-language Rust graph database
engine supporting openCypher, Gremlin, and GQL with Bolt, REST, Gremlin WebSocket, and Redis module interfaces.

## Repository Structure Diagram

![Repository Structure](repo-surface.svg)

<details>
<summary>Mermaid source</summary>

```mermaid
graph TD
    ROOT["SedanDelivery/"]
    
    subgraph core_crates["Core Engine Crates"]
        CYPHER_CORE["cypher-core/<br/><i>openCypher parser, planner,<br/>executor, GraphStore trait</i>"]
        GREMLIN_CORE["gremlin-core/<br/><i>Gremlin bytecode compiler<br/>+ text parser</i>"]
        GQL_ENGINE["gql-engine/<br/><i>GQL (ISO/IEC 39075)<br/>parser + planner</i>"]
        SD2_REDIS_STORE["sd2-redis-store/<br/><i>Redis/Valkey GraphStore impl<br/>+ FanoutStore</i>"]
        GQL_FABRIC["gql-fabric-store/<br/><i>Microsoft Fabric<br/>GraphStore impl</i>"]
    end

    subgraph entry_points["Entry Point Crates"]
        SD2_REDIS_MODULE["sd2-redis-module/<br/><i>Valkey/Redis loadable<br/>module (.so cdylib)</i>"]
        SD2_API["sd2-api/<br/><i>HTTP REST API<br/>(axum, port 3001)</i>"]
        BOLT["sedandelivery-bolt/<br/><i>Neo4j Bolt v4.4<br/>TCP server (port 7687)</i>"]
        GREMLIN_WS["sedandelivery-gremlin/<br/><i>Gremlin WebSocket<br/>server (port 8182)</i>"]
        GREMLIN_TEXT["sedandelivery-gremlin-server/<br/><i>Gremlin text eval<br/>server (port 8182)</i>"]
        SYNC["sedandelivery-fabric-sync/<br/><i>Changelog replication<br/>to Fabric</i>"]
        CLI["gql-cli/<br/><i>Interactive REPL<br/>for development</i>"]
    end

    subgraph embedded_crates["Embeddable Libraries"]
        CYPHER_EMB["sedan-cypher-embedded/<br/><i>In-process openCypher</i>"]
        GQL_EMB["sedan-gql-embedded/<br/><i>In-process GQL/Cypher</i>"]
        GREMLIN_EMB["sedan-gremlin-embedded/<br/><i>In-process Gremlin</i>"]
    end

    subgraph sdks["Client SDKs"]
        SDK_PY["sdk/python/<br/><i>Python client</i>"]
        SDK_NET["sdk/dotnet/<br/><i>.NET client</i>"]
        SDK_RS["sdk/rust/<br/><i>Rust client</i>"]
    end

    subgraph infra_deploy["Infrastructure & Deploy"]
        INFRA["infra/<br/><i>main.bicep, KEDA,<br/>Prometheus alerts</i>"]
        DOCKER["Dockerfile.*<br/><i>4 container images</i>"]
        COMPOSE["docker-compose*.yml<br/><i>5 compose profiles</i>"]
        SCRIPTS["scripts/<br/><i>Test, provision,<br/>validate scripts</i>"]
        GH_ACTIONS[".github/workflows/<br/><i>30+ CI/CD workflows</i>"]
    end

    subgraph testing["Testing"]
        TESTS["tests/<br/><i>E2E, load tests,<br/>QA scenarios</i>"]
        NEO4J_COMPAT["neo4j-compat-gallery/<br/><i>4 Neo4j app<br/>compatibility tests</i>"]
    end

    subgraph docs_group["Documentation"]
        DOCS["docs/<br/><i>Architecture, ADRs,<br/>tutorials, guides</i>"]
        SEDAN_UI["sedan-graph-ui/<br/><i>.NET BFF + React<br/>graph visualizer</i>"]
    end

    ROOT --> core_crates
    ROOT --> entry_points
    ROOT --> embedded_crates
    ROOT --> sdks
    ROOT --> infra_deploy
    ROOT --> testing
    ROOT --> docs_group
```

</details>

See [repo-surface.mmd](repo-surface.mmd) and [repo-surface.dot](repo-surface.dot) for diagram sources.

## Build System

- **Language**: Rust (edition 2021)
- **Build**: Cargo workspace with 17 members
- **License**: MIT OR Apache-2.0
- **Release profile**: LTO fat, opt-level 3, codegen-units 1, symbols stripped
- **Dev profile**: opt-level 1 (arena-heavy code)

## Key Configuration Files

| File | Purpose |
|------|---------|
| `Cargo.toml` | Workspace root with shared dependency versions |
| `Cargo.lock` | Lockfile for reproducible builds |
| `rustfmt.toml` | Formatting configuration |
| `clippy.toml` | Clippy lint configuration |
| `.pre-commit-config.yaml` | Pre-commit hooks |
| `docs/_config.yml` | Jekyll GitHub Pages configuration |
