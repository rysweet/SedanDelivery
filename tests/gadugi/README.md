# SedanDelivery Graph Visualizer - Gadugi Agentic Tests

These YAML scenarios describe end-to-end user flows for the Graph Visualizer SPA.
They are written for the [gadugi-agentic-test](https://github.com/gadugi/gadugi-agentic-test)
runner, which uses an AI agent to interact with the browser based on natural-language
step descriptions rather than fragile pixel-coordinate selectors.

---

## Prerequisites

Both the frontend and API must be running before executing any scenario.

### Start the application (Aspire orchestrator)

```bash
# From the repository root
dotnet run --project sedan-graph-api/SedanGraphApi.AppHost
```

This starts:
- React SPA at `http://localhost:5173`
- .NET API at `http://localhost:5000` (proxied via Vite as `/api`)

### Start services individually (alternative)

```bash
# Terminal 1 — API
dotnet run --project sedan-graph-api/SedanGraphApi.Api

# Terminal 2 — Frontend
cd sedan-graph-ui/src/sedan-web
npm install
npm run dev
```

---

## Running Tests

```bash
# Run a single scenario
gadugi-agentic-test run tests/gadugi/01-create-graph.yaml

# Run all scenarios in order
gadugi-agentic-test run tests/gadugi/

# Run with verbose output
gadugi-agentic-test run tests/gadugi/03-search.yaml --verbose

# Run with a specific browser
gadugi-agentic-test run tests/gadugi/ --browser chromium
```

Each scenario is self-contained: the `setup` block seeds required data via the API
before any browser steps, and the `teardown` block removes all created data after
the scenario finishes (pass or fail).

---

## Scenario Inventory

| File | Scenario | Key Feature Tested |
|------|----------|--------------------|
| `01-create-graph.yaml` | First-Time Graph Creation | GraphSelector dropdown, inline graph name input, empty-state messages |
| `02-create-nodes-edges.yaml` | Create Nodes and Edges | Create Node dialog, property key/value rows, Legend Panel counts, Cypher CREATE edge |
| `03-search.yaml` | Search Graph | Search Bar label vs property search, result count badge, Inspector on click |
| `04-inspect-edit-node.yaml` | Node Inspection and Editing | Inspector Panel property editor, Save Changes, two-step delete confirmation |
| `05-cypher-query.yaml` | Cypher Query Execution | Monaco editor, Execute button, results table, mutation badge, error display |
| `06-schema.yaml` | Schema View | Schema Panel labels/relationship columns, refresh button, mutual exclusivity with editor |
| `07-export.yaml` | Export Graph | Export dialog format selection (JSON/CSV), Cancel, download trigger |
| `08-legend.yaml` | Legend Panel Interaction | Label visibility toggle, toggle-all, opacity state, collapse panel |
| `09-context-menu.yaml` | Right-Click Context Menu | Node/edge/canvas context menus, menu items, Escape to dismiss |
| `10-delete-node.yaml` | Delete Node | Inspector two-step confirm, keyboard Delete shortcut, node count decrement |

---

## Test Data Setup

All scenarios that require pre-existing data use `api_call` actions in the `setup`
block. The API accepts JSON at `http://localhost:5000`. Key endpoints:

```
POST   /api/graphs                              Create a graph
DELETE /api/graphs/{name}                       Delete a graph and all its data
POST   /api/graphs/{name}/nodes                 Create a node
POST   /api/graphs/{name}/mutate                Execute a Cypher mutate statement
POST   /api/graphs/{name}/query                 Execute a Cypher read query
```

Example setup payload for a node:
```json
{
  "labels": ["Person"],
  "properties": { "name": "Alice", "age": 30 }
}
```

Example mutate payload to create an edge:
```json
{
  "statement": "MATCH (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}) CREATE (a)-[:KNOWS]->(b)"
}
```

---

## Important Notes for the Test Agent

### Canvas interaction
The graph canvas uses a Sigma.js WebGL renderer. Node positions are determined by
the layout algorithm (ForceAtlas2 by default) and are non-deterministic. The agent
should use generous `timeout_ms` values (5000-8000ms) for canvas-related verification
steps and should click on the `canvas` element at its center position when trying to
select nodes.

### Monaco editor
The Cypher editor (Monaco) renders in an iframe-like sandboxed context. To type into
it, target the `textarea` inside `.monaco-editor`. Use `Control+a` to select all
existing text before typing replacement content.

### Two-step delete confirmation
Both the Inspector Panel "Delete Node" / "Delete Edge" buttons and the GraphSelector
delete button require two clicks: the first click changes the button label to
"Confirm Delete?" (or turns the trash icon red), and the second click performs the
deletion. Do not rely on a dialog appearing — the confirmation is inline.

### Left/right panel collapse
The left (Legend) and right (Inspector) panels are always mounted in the DOM but have
CSS width transitions (0 or 240/300px). A panel at width 0 is technically present but
not interactable. The right panel opens automatically when a node or edge is selected.

### Toolbar tooltips
The toolbar buttons use `title` attributes that match their tooltip content
(e.g. `title="Add Node (N)"`, `title="Cypher Query Editor"`, `title="Schema View"`).
These are reliable selectors that do not depend on icon SVG structure.

---

## Cleanup After Failures

If a test run is interrupted before teardown runs, the test graph may persist.
To clean up manually:

```bash
curl -X DELETE http://localhost:5000/api/graphs/test-graph
```

---

## CI Integration

To run these tests in CI, ensure both services are healthy before invoking the runner:

```bash
# Wait for API health check
until curl -sf http://localhost:5000/health; do sleep 1; done

# Wait for SPA to be served
until curl -sf http://localhost:5173; do sleep 1; done

# Run all gadugi scenarios
gadugi-agentic-test run tests/gadugi/ --reporter junit --output test-results/gadugi.xml
```
