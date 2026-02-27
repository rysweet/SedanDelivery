# E2E Agentic Test Scenarios

End-to-end test scenarios for the SedanDelivery Graph Visualizer SPA using the
[gadugi-agentic-test](https://github.com/rysweet/gadugi-agentic-test) framework.

## Scenarios

| File | Flow | Tags |
|------|------|------|
| `01-app-smoke-test.yaml` | SPA loads, toolbar renders, canvas present | smoke, critical |
| `02-graph-lifecycle.yaml` | Create, select, delete a graph | critical, graph-management |
| `03-node-crud.yaml` | Create nodes via dialog, inspect, verify | critical, node-crud |
| `04-cypher-query-execution.yaml` | Monaco editor, MATCH queries, CREATE mutations | critical, cypher |
| `05-search-and-filter.yaml` | Label search, property search, legend toggles | critical, search |
| `06-schema-view.yaml` | Labels, relationship types, counts | schema |
| `07-export-import.yaml` | Export dialog, JSON format, import dialog | export, import |
| `08-toolbar-interactions.yaml` | Mode switch, layout, fit view, disabled undo | toolbar |
| `09-error-handling.yaml` | Invalid names, bad Cypher, toast notifications | errors |
| `10-api-contract-validation.yaml` | Direct gql-api HTTP contract validation | api, contract |

## Prerequisites

Start the full stack:

```bash
# Terminal 1: Rust API
cd /path/to/repo && cargo run -p gql-api

# Terminal 2: .NET BFF
cd sedan-graph-ui && GQL_API_URL=http://localhost:3001 dotnet run --project src/SedanDelivery.Api

# Terminal 3: React SPA
cd sedan-graph-ui/src/sedan-web && npm run dev
```

Or via Aspire:

```bash
cd sedan-graph-ui && dotnet run --project src/SedanDelivery.AppHost
```

## Running

```bash
# All E2E scenarios
gadugi-test run tests/gadugi/e2e/*.yaml

# Smoke tests only
gadugi-test run tests/gadugi/e2e/*.yaml --tags smoke

# Critical flows
gadugi-test run tests/gadugi/e2e/*.yaml --tags critical
```
