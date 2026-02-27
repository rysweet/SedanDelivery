# SedanDelivery Graph Visualizer — Interactive Tutorial

This tutorial walks you through the Graph Visualizer using a pre-built supply chain demo graph with 23 nodes and 28 edges.

## Quick Start (One Command)

```bash
# Deploy and seed the demo:
gh workflow run "Deploy SedanDelivery"

# Or run locally:
cargo run -p gql-api &
./scripts/seed-demo-graph.sh
cd sedan-graph-ui/src/sedan-web && npm run dev
# Open http://localhost:5173
```

## Step 1: Select the Demo Graph

Open the app and click the **graph selector** dropdown in the top-right corner of the toolbar. Select **supply-chain** from the list.

The canvas will populate with 23 nodes across 5 labels (Warehouse, Store, Driver, Vehicle, Product) connected by 28 edges.

The **legend panel** on the left shows all 5 node labels with their colors:
- Warehouse (blue)
- Store (green)
- Driver (amber)
- Vehicle (coral)
- Product (lavender)

## Step 2: Explore Visually

**Click a node** to select it. The **Inspector Panel** on the right shows its labels, properties, and connection count.

**Double-click a node** to expand its neighborhood — connected nodes and edges are fetched and added to the canvas.

Try double-clicking a Warehouse node — you should see Store nodes appear as delivery destinations, and Vehicle nodes assigned to the warehouse.

## Step 3: Run Cypher Queries

Click the **Query Editor** button in the toolbar (terminal icon) to open the Monaco Cypher editor.

### Query 1: Delivery Routes

```cypher
MATCH (w:Warehouse)-[d:DELIVERS_TO]->(s:Store)
RETURN w.name, s.name, s.city, d.frequency
ORDER BY w.name
```

This shows all warehouse-to-store delivery routes with their frequency (daily, weekly, biweekly).

### Query 2: Supply Chain Path (Warehouse → Store → Product)

```cypher
MATCH (w:Warehouse)-[:DELIVERS_TO]->(s:Store)-[:STOCKS]->(p:Product)
RETURN w.name, s.name, p.name, p.price
ORDER BY p.price
```

This traces the two-hop path from warehouses through stores to products they stock.

### Query 3: Driver → Vehicle → Warehouse Chain

```cypher
MATCH (d:Driver)-[:DRIVES]->(v:Vehicle)-[:ASSIGNED_TO]->(w:Warehouse)
RETURN d.name, d.rating, v.plate, v.type, w.name
ORDER BY d.rating
```

Shows which drivers operate which vehicles at which warehouses.

### Query 4: Top Stores by Revenue

```cypher
MATCH (s:Store)
RETURN s.name, s.city, s.revenue, s.employees
ORDER BY s.revenue
```

### Query 5: Full 3-Hop Supply Chain

```cypher
MATCH (w:Warehouse)-[:SUPPLIES]->(p:Product)<-[:STOCKS]-(s:Store)
RETURN w.name AS supplier, p.name AS product, p.price, s.name AS store
ORDER BY p.name
```

This traces the full supply chain: which warehouse supplies which product, and which stores stock it.

## Step 4: Search the Graph

Use the **search bar** at the top of the canvas area.

- Type `Warehouse` and press Enter — all Warehouse nodes will be highlighted (label search).
- Type `seattle` and press Enter — nodes with "seattle" in any property will appear (property search).
- Type `Product` and press Enter — all Product nodes appear.

## Step 5: Use the Legend Panel

The **legend panel** on the left lists all node labels with colored dots.

- Click the **eye icon** next to "Driver" to hide all driver nodes.
- Click it again to show them.
- This is useful for focusing on specific parts of the supply chain (e.g., hide Products to see only the logistics network).

## Step 6: View the Schema

Click the **Schema** button in the toolbar (table icon).

The schema panel shows:
- **Labels**: Warehouse (3), Store (6), Driver (5), Vehicle (4), Product (5)
- **Relationship Types**: DELIVERS_TO (7), DRIVES (5), ASSIGNED_TO (4), STOCKS (7), SUPPLIES (5)

## Step 7: Edit Node Properties

1. Click a Store node (e.g., "Downtown Seattle")
2. In the Inspector Panel, find the `revenue` property
3. Change it to `2500000`
4. Click **Save Changes**
5. A success toast should appear confirming the update

## Step 8: Create a New Node

1. Click the **Add Node** button in the toolbar (+ icon)
2. In the dialog, enter:
   - Labels: `Store`
   - Properties: `name` = `Tacoma Express`, `city` = `Tacoma`, `revenue` = `650000`
3. Click **Create**
4. The new node appears in the canvas

## Step 9: Run a Mutation

In the Cypher editor, create a delivery route to the new store:

```cypher
MATCH (w:Warehouse {name: "Seattle-HQ"}), (s:Store {name: "Tacoma Express"})
CREATE (w)-[:DELIVERS_TO {frequency: "weekly", avgItems: 100}]->(s)
```

Click **Execute**. The mutation stats will show `edges_created: 1`.

## Step 10: Export the Graph

1. Click the **Export** button in the toolbar (download icon)
2. Select **JSON** format
3. Click **Export** — a JSON file downloads containing all 24 nodes and 29 edges

## Sample Data Details

The demo graph models a regional supply chain:

```
Warehouse ──DELIVERS_TO──→ Store ──STOCKS──→ Product
    ↑                                            ↑
    │                                            │
Vehicle ←──ASSIGNED_TO                  SUPPLIES──── Warehouse
    ↑
    │
Driver ──DRIVES──→ Vehicle
```

| Entity | Count | Example Properties |
|--------|-------|--------------------|
| Warehouse | 3 | name, city, state, capacity, manager |
| Store | 6 | name, city, revenue, employees |
| Driver | 5 | name, license, experience, rating |
| Vehicle | 4 | plate, type, mileage, year |
| Product | 5 | name, category, price, stock |
