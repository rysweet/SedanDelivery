# Compile-time Dependencies

Cargo workspace dependency graph for SedanDelivery — 8 crates plus a loadtest binary.

## Workspace Dependency Graph

### Mermaid

```mermaid
graph TD
    subgraph workspace["Workspace Crates"]
        gql_core["gql-core<br/><i>lib</i>"]
        gql_redis["gql-redis-store<br/><i>lib</i>"]
        gql_fabric["gql-fabric-store<br/><i>lib</i>"]
        gql_module["gql-module<br/><i>cdylib</i>"]
        gql_api["gql-api<br/><i>bin</i>"]
        gql_cli["gql-cli<br/><i>bin</i>"]
        bolt["sedandelivery-bolt<br/><i>bin</i>"]
        sync["sedandelivery-fabric-sync<br/><i>bin</i>"]
        loadtest["tests/loadtest<br/><i>bin</i>"]
    end

    gql_redis -->|"depends on"| gql_core
    gql_fabric -->|"depends on"| gql_core
    gql_module -->|"depends on"| gql_core
    gql_cli -->|"depends on"| gql_core
    gql_api -->|"depends on"| gql_core
    gql_api -->|"depends on"| gql_redis
    bolt -->|"depends on"| gql_core
    bolt -->|"depends on"| gql_redis
    sync -->|"depends on"| gql_core
    sync -->|"depends on"| gql_redis
    sync -->|"depends on"| gql_fabric
    loadtest -->|"depends on"| gql_core
```

See [dependencies.mmd](dependencies.mmd) and [dependencies.dot](dependencies.dot) for diagram sources.

## External Dependencies

### Shared (workspace-level)

| Crate | Version | Purpose | License |
|-------|---------|---------|---------|
| thiserror | 2.0 | Error derive macros | MIT/Apache-2.0 |
| indexmap | 2.0 | Ordered hash maps | MIT/Apache-2.0 |
| rustc-hash | 2.0 | Fast hashing | MIT/Apache-2.0 |
| regex | 1.10 | Pattern matching | MIT/Apache-2.0 |
| typed-arena | 2.0 | Arena allocation | MIT |
| jiff | 0.2 | Temporal types | MIT |
| smol_str | 0.3 | Short inline strings | MIT/Apache-2.0 |
| petgraph | 0.8 | Graph algorithms | MIT/Apache-2.0 |
| serde | 1.0 | Serialization | MIT/Apache-2.0 |
| serde_json | 1.0 | JSON support | MIT/Apache-2.0 |
| tracing | 0.1 | Structured logging | MIT |
| tokio | 1 | Async runtime | MIT |

### Per-Crate Key Dependencies

| Crate | Key Dependencies |
|-------|-----------------|
| gql-core | regex, typed-arena, jiff, smol_str, petgraph, indexmap |
| gql-redis-store | redis, r2d2, gql-core |
| gql-fabric-store | reqwest, gql-core |
| gql-module | redis-module (bindgen, requires libclang), gql-core |
| gql-api | axum, tower, tower-http, hyper, gql-core, gql-redis-store |
| sedandelivery-bolt | tokio, gql-core, gql-redis-store |
| sedandelivery-fabric-sync | tokio, gql-core, gql-redis-store, gql-fabric-store |

### Dev/Test Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| proptest | 1.6 | Property-based testing |
| criterion | 0.8 | Benchmarking |
| insta | 1.40 | Snapshot testing |
| cucumber | 0.22 | BDD/TCK tests |

## Dependency Policy

- All dependencies must be MIT or Apache-2.0 licensed
- No GPL, LGPL, or SSPL dependencies permitted
- Shared versions pinned in workspace `[workspace.dependencies]`
- Members reference shared deps with `{ workspace = true }`

## Circular Dependencies

**None detected.** The dependency graph is a clean DAG with `gql-core` as the leaf.
