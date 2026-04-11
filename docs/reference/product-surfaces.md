---
title: Product Surface Reference
description: Current SedanDelivery runtime and distribution surfaces, with embedded engines audited against the live non-embedded baselines.
last_updated: 2026-04-11
review_schedule: as-needed
doc_type: reference
---

# Product Surface Reference

## Overview

SedanDelivery ships in multiple form factors. The embedded crates are one in-process delivery surface, not the whole product, so their quality bar is set against the live server, module, CLI, packaging, and documentation baselines below.

## Runtime and Distribution Inventory

| Form factor | Current surfaces | Primary contract | Notes |
| --- | --- | --- | --- |
| Embedded libraries | `sedan-cypher-embedded`, `sedan-gql-embedded`, `sedan-gremlin-embedded` | In-process query execution against a caller-supplied `GraphStore` | No listeners or runtime-only env vars; process-wide engine limits still apply |
| Network/server runtimes | `sd2-api`, `sedandelivery-bolt`, `sedandelivery-gremlin`, `sedandelivery-gremlin-server` | Long-running protocol servers | Primary baseline for user-visible runtime behavior |
| Module form factors | `sd2-redis-module`, `sd2-valkey-module` | In-process Redis/Valkey integration | `sd2-redis-module` is the primary graph-query module surface; `sd2-valkey-module` is a standalone Valkey workspace exposing `SD2.MERGE_EDGES` |
| CLI/dev tooling | `gql-cli` | In-process developer/demo surface | Useful low-friction baseline for Cypher wiring and examples |
| Sidecar/runtime support | `sedandelivery-fabric-sync` | Redis/Valkey-to-Fabric changelog replication | Inventory and packaging baseline unless query-contract evidence makes it directly relevant |
| Packaging and deploy surfaces | `Dockerfile`, `Dockerfile.api`, `Dockerfile.bolt`, `Dockerfile.gremlin`, `Dockerfile.sync`, `docker-compose*.yml`, `.github/workflows/*.yml`, `scripts/validate.sh`, top-level docs | Build, ship, validate, and describe runtime surfaces | Embedded crates are documented and tested here, but they are not standalone container entrypoints |

## Comparison Baselines by Concern

| Concern | Embedded audit target | Comparison baseline |
| --- | --- | --- |
| Cypher behavior and wiring | `sedan-cypher-embedded` | `gql-cli`, `sd2-api`, `sedandelivery-bolt`, `sd2-redis-module` |
| GQL behavior and wiring | `sedan-gql-embedded` | `gql-engine`, plus Cypher-facing baselines `gql-cli`, `sd2-api`, `sedandelivery-bolt`, and `sd2-redis-module` for shared-contract compatibility |
| Gremlin behavior and wiring | `sedan-gremlin-embedded` | `sedandelivery-gremlin`, `sedandelivery-gremlin-server` |
| Docs, packaging, runtime presence, and validation | Embedded crates as libraries | Root `Cargo.toml`, crate manifests, `README.md`, `docs/architecture.md`, `Dockerfile*`, `docker-compose*.yml`, `.github/workflows/*.yml`, and `scripts/validate.sh` |
| Inventory-only surfaces unless directly implicated | Embedded docs and packaging story | `sd2-valkey-module`, `sedandelivery-fabric-sync` |

## Embedded Contract Map

| Embedded crate | Current wiring | Shared substrate | Intended difference from non-embedded baselines |
| --- | --- | --- | --- |
| `sedan-cypher-embedded` | `cypher-core` parser -> planner -> executor | `GraphStore`, `QueryResult`, `Value`, physical operators | Cypher-only in-process API; it should reject GQL-only keywords such as `INSERT` |
| `sedan-gql-embedded` | `gql-engine` parser -> planner -> executor -> `cypher-core` | `QueryResult`, `Value`, storage and executor layer | GQL-first in-process API that still preserves shared Cypher compatibility |
| `sedan-gremlin-embedded` | `gremlin-core` parser/compiler -> `cypher-core` executor | `GraphStore`, `QueryResult`, `Value`, physical operators | Gremlin text traversal entry surface |

## Packaging and Runtime Presence

| Surface | Ships as | Embedded status | Notes |
| --- | --- | --- | --- |
| Cargo workspace | Libraries and binaries | Yes | The embedded crates are first-class workspace members |
| Docker images | Server, module, and sidecar entrypoints | No | `Dockerfile*` package long-running runtimes, not library-only form factors |
| Compose profiles | Local and test deployments | No | `docker-compose*.yml` exercise server/module/runtime surfaces |
| GitHub workflows | Build, test, deploy, and docs automation | Yes | CI and local validation now run dedicated embedded and native-GQL contract commands |
| Top-level docs | README, architecture, validation, reference, tutorials | Yes | The embedded surface is documented as a first-class form factor, not as the whole product |

## Validation

| Scope | Command | What it proves |
| --- | --- | --- |
| Workspace compile | `cargo check --workspace` | Embedded crates still compile as first-class workspace members |
| Shared Cypher baseline | `cargo test -p cypher-core --test end_to_end` | Embedded and non-embedded Cypher surfaces still share the same parse -> plan -> execute contract |
| Embedded wrapper contract | `cargo test -p sedan-cypher-embedded -p sedan-gql-embedded -p sedan-gremlin-embedded` | The three in-process libraries expose their intended public behavior |
| Native GQL contract baseline | `cargo test -p gql-engine --test outside_in` | GQL-only syntax such as `INSERT`, `FILTER`, `MANDATORY MATCH`, and `FOR` stays covered end to end |
| Module/server baseline | `cargo test -p sd2-redis-module --test integration` | Redis/Valkey module behavior still matches the shared contract |

## Intentional Differences

Not every cross-form-factor difference is a bug.

| Difference | Intentional | Reason |
| --- | --- | --- |
| Embedded crates do not bind ports or require server-specific deployment env vars | Yes | They are libraries, not runtimes; they can still inherit process-wide engine limit env vars such as `GQL_QUERY_TIMEOUT_MS` |
| `sd2-valkey-module` is inventory-only for the embedded audit unless module-specific evidence says otherwise | Yes | It is a standalone Valkey module focused on `SD2.MERGE_EDGES`, not the primary query comparator |
| `sedandelivery-fabric-sync` is a packaging/runtime baseline, not a primary query-semantics comparator | Yes | It is a replication sidecar, not a query entry surface |

## See Also

- [Embedded Engines Reference](./embedded-engines.md)
- [Tutorial: Embed SedanDelivery In-Process](../tutorials/embed-sedandelivery.md)
- [Architecture Overview](../architecture.md)
