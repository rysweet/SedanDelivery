# SedanDelivery Graph Visualizer — Interactive Tutorial

This tutorial walks you through every feature of the Graph Visualizer using a pre-built supply chain demo graph with 23 nodes and 28 edges.


## Quick Start

```bash
# Run locally (three commands):
cargo run --release -p sd2-api &
./scripts/seed-demo-graph.sh
cd sedan-graph-ui/src/sedan-web && VITE_PROXY_TARGET=http://localhost:3001 VITE_PROXY_REWRITE=true npm run dev

# Open http://localhost:5173
```

## Step 1: Open the App

When you first open the app, you'll see the dark-mode interface with the toolbar across the top, the legend panel on the left, the graph canvas in the center, and the inspector panel on the right.

![App loaded — dark mode UI with toolbar, legend, canvas, and inspector](/graph-visualizer/images/01-app-loaded.png)

The status bar at the bottom shows **0 nodes · 0 edges** because no graph is selected yet.

## Step 2: Select the Demo Graph

Click the **graph selector** in the top-right corner of the toolbar. A dropdown appears showing all available graphs.

![Graph selector dropdown showing available graphs](/graph-visualizer/images/02-graph-selector.png)

Select **supply-chain** from the list. The graph data loads and the canvas populates with nodes and edges.

## Step 3: Explore the Graph Visually

The canvas now shows the supply chain network rendered with WebGL (Sigma.js). Nodes are color-coded by label:

- **Warehouses** (teal) — Seattle-HQ, Portland-DC, Denver-DC
- **Stores** (grey) — Downtown Seattle, Bellevue Mall, Portland Central, Denver Union
- **Drivers** (yellow) — Alex Thompson, Jordan Lee
- **Vehicles** (yellow) — WA-5501, OR-3302

Edges connect warehouses to stores (DELIVERS_TO), drivers to vehicles (DRIVES), and vehicles to warehouses (ASSIGNED_TO). The **minimap** in the bottom-right corner shows an overview of the entire graph.

![Supply chain graph visualization with nodes, edges, legend, and minimap](/graph-visualizer/images/03-supply-chain-loaded.png)

**Try these interactions:**
- **Click a node** to select it — the Inspector Panel on the right shows its labels, properties, and connection count
- **Double-click a node** to expand its neighborhood — connected nodes and edges are fetched and added to the canvas
- **Scroll** to zoom in and out
- **Drag** to pan the canvas

## Step 4: Use the Legend Panel

The **legend panel** on the left lists all node labels with colored dots and counts.

![Legend panel showing Driver, Store, Vehicle, Warehouse labels with colors](/graph-visualizer/images/04-legend-labels.png)

- Click the **eye icon** next to a label to hide those nodes
- Click it again to show them
- Use **Hide All** / **Show All** to toggle everything at once

This is useful for focusing on specific parts of the supply chain — for example, hide Drivers and Vehicles to see only the Warehouse → Store delivery network.

## Step 5: Run Cypher Queries

Click the **query editor** button in the toolbar (the `>_` terminal icon) to open the Monaco Cypher editor at the bottom of the screen.

![Cypher editor opened with Monaco syntax highlighting](/graph-visualizer/images/05-editor-opened.png)

Type a Cypher query. The editor provides syntax highlighting for keywords like `MATCH`, `RETURN`, `WHERE`, and `ORDER BY`.

![Cypher query typed with syntax highlighting](/graph-visualizer/images/05-query-typed.png)

Click **Execute** (or press **Ctrl+Enter**) to run the query. Results appear below the editor.

![Query results displayed in a table](/graph-visualizer/images/05-query-results.png)

### Try these queries:

**Delivery routes:**
```cypher
MATCH (w:Warehouse)-[d:DELIVERS_TO]->(s:Store)
RETURN w.name, s.name, s.city, d.frequency
ORDER BY w.name
```

**Supply chain path (2 hops):**
```cypher
MATCH (w:Warehouse)-[:DELIVERS_TO]->(s:Store)-[:STOCKS]->(p:Product)
RETURN w.name, s.name, p.name, p.price
ORDER BY p.price
```

**Driver → Vehicle → Warehouse chain:**
```cypher
MATCH (d:Driver)-[:DRIVES]->(v:Vehicle)-[:ASSIGNED_TO]->(w:Warehouse)
RETURN d.name, d.rating, v.plate, v.type, w.name
ORDER BY d.rating
```

**Top stores by revenue:**
```cypher
MATCH (s:Store)
RETURN s.name, s.city, s.revenue, s.employees
ORDER BY s.revenue
```

## Step 6: View the Schema

Click the **Schema** button in the toolbar (table icon) to see an overview of the graph structure.


The schema panel shows:
- **Labels**: Warehouse (3), Store (6), Driver (5), Vehicle (4), Product (5)
- **Relationship Types**: DELIVERS_TO (7), DRIVES (5), ASSIGNED_TO (4), STOCKS (7), SUPPLIES (5)

## Step 7: Search the Graph

Use the **search bar** at the top of the canvas area. The search detects whether you're searching by label or property value:

- Type `Warehouse` (capitalized) → label search: finds all Warehouse nodes
- Type `seattle` (lowercase) → property search: finds nodes with "seattle" in any property

![Search results for Warehouse label](/graph-visualizer/images/07-search-results.png)

## Step 8: Create a New Node

Click the **Add Node** button in the toolbar (+ icon) to open the create node dialog.

![Add Node dialog with label and property fields](/graph-visualizer/images/09-add-node-dialog.png)

Fill in:
- **Labels**: `Store`
- **Properties**: Click "Add Property" and enter `name` = `Tacoma Express`, `city` = `Tacoma`, `revenue` = `650000`

Click **Create**. The new node appears in the canvas.

## Step 9: Change the Layout

Click the **layout dropdown** in the toolbar to switch between layout algorithms.

![Layout algorithm dropdown — ForceAtlas2, Circular, Random](/graph-visualizer/images/10-layout-dropdown.png)

Available layouts:
- **ForceAtlas2** (default) — force-directed layout that clusters connected nodes together
- **Circular** — arranges nodes in a circle
- **Random** — random positions (useful as a reset before re-running ForceAtlas2)

## Step 10: Export the Graph

Click the **Export** button in the toolbar (download icon) to open the export dialog.

![Export dialog with JSON format option](/graph-visualizer/images/08-export-dialog.png)

Select **JSON** format and click **Export** to download the full graph as a JSON file containing all nodes and edges with their properties.

## Step 11: Run a Mutation

In the Cypher editor, create a delivery route to the new store:

```cypher
MATCH (w:Warehouse {name: "Seattle-HQ"}), (s:Store {name: "Tacoma Express"})
CREATE (w)-[:DELIVERS_TO {frequency: "weekly", avgItems: 100}]->(s)
```

Click **Execute**. The editor detects this as a mutation (starts with `MATCH ... CREATE`) and sends it to the mutate endpoint. The result shows `edges_created: 1`.

## Full UI Overview

Here's the complete interface with all panels visible:

![Full UI overview — toolbar, legend, canvas with graph, inspector, status bar](/graph-visualizer/images/10-full-ui-overview.png)

## Demo Data Model

The supply chain graph has this structure:

```
Warehouse ──DELIVERS_TO──→ Store ──STOCKS──→ Product
    ↑                                            ↑
    │                                            │
Vehicle ←──ASSIGNED_TO                  SUPPLIES──── Warehouse
    ↑
    │
Driver ──DRIVES──→ Vehicle
```

| Entity | Count | Properties |
|--------|-------|-----------|
| Warehouse | 3 | name, city, state, capacity, manager |
| Store | 6 | name, city, revenue, employees |
| Driver | 5 | name, license, experience, rating |
| Vehicle | 4 | plate, type, mileage, year |
| Product | 5 | name, category, price, stock |

## Next Steps

- Read the [API Reference](api-reference.md) for all available endpoints
- Read the [User Guide](user-guide.md) for keyboard shortcuts and advanced features
- Read the [Deployment Guide](deployment.md) for Azure Container Apps deployment
