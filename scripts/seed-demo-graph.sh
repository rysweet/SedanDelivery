#!/usr/bin/env bash
# seed-demo-graph.sh — Seed a rich supply-chain demo graph via sd2-api REST endpoints.
#
# Usage:
#   ./scripts/seed-demo-graph.sh                    # defaults to http://localhost:3001
#   ./scripts/seed-demo-graph.sh http://my-api:3001 # custom base URL
#
# Creates the "supply-chain" graph with:
#   26 nodes (Warehouse, Store, Driver, Vehicle, Product)
#   30 edges (DELIVERS_TO, DRIVES, ASSIGNED_TO, STOCKS, SUPPLIES)
#
# Idempotent: deletes any existing "supply-chain" graph before seeding.

set -euo pipefail

BASE="${1:-http://localhost:3001}"
GRAPH="supply-chain"

echo "=== SedanDelivery Demo Graph Seeder ==="
echo "API: $BASE"
echo "Graph: $GRAPH"
echo ""

# Helper: run a Cypher mutation
mutate() {
  local cypher="$1"
  curl -sf -X POST "$BASE/graphs/$GRAPH/mutate" \
    -H 'Content-Type: application/json' \
    -d "{\"cypher\": \"$cypher\"}" > /dev/null
}

# Clean slate
echo "Cleaning up any existing '$GRAPH' graph..."
curl -sf -X DELETE "$BASE/graphs/$GRAPH" > /dev/null 2>&1 || true

echo "Creating graph '$GRAPH'..."
curl -sf -X POST "$BASE/graphs" \
  -H 'Content-Type: application/json' \
  -d "{\"name\": \"$GRAPH\"}" > /dev/null

echo "Seeding nodes..."

# ── Warehouses ────────────────────────────────────────────────────────────────
mutate "CREATE (:Warehouse {name: 'Seattle-HQ', city: 'Seattle', state: 'WA', capacity: 50000, manager: 'Sarah Chen'})"
mutate "CREATE (:Warehouse {name: 'Portland-DC', city: 'Portland', state: 'OR', capacity: 35000, manager: 'Mike Rodriguez'})"
mutate "CREATE (:Warehouse {name: 'Denver-DC', city: 'Denver', state: 'CO', capacity: 28000, manager: 'Lisa Park'})"

# ── Stores ────────────────────────────────────────────────────────────────────
mutate "CREATE (:Store {name: 'Downtown Seattle', city: 'Seattle', revenue: 2400000, employees: 45})"
mutate "CREATE (:Store {name: 'Bellevue Mall', city: 'Bellevue', revenue: 1800000, employees: 32})"
mutate "CREATE (:Store {name: 'Portland Central', city: 'Portland', revenue: 1500000, employees: 28})"
mutate "CREATE (:Store {name: 'Eugene Outlet', city: 'Eugene', revenue: 900000, employees: 18})"
mutate "CREATE (:Store {name: 'Denver Union', city: 'Denver', revenue: 2100000, employees: 40})"
mutate "CREATE (:Store {name: 'Boulder Plaza', city: 'Boulder', revenue: 750000, employees: 15})"

# ── Drivers ───────────────────────────────────────────────────────────────────
mutate "CREATE (:Driver {name: 'Alex Thompson', license: 'CDL-A', experience: 8, rating: 4.9})"
mutate "CREATE (:Driver {name: 'Jordan Lee', license: 'CDL-A', experience: 5, rating: 4.7})"
mutate "CREATE (:Driver {name: 'Casey Kim', license: 'CDL-B', experience: 3, rating: 4.5})"
mutate "CREATE (:Driver {name: 'Pat Morgan', license: 'CDL-A', experience: 12, rating: 4.8})"
mutate "CREATE (:Driver {name: 'Sam Rivera', license: 'CDL-A', experience: 6, rating: 4.6})"

# ── Vehicles ──────────────────────────────────────────────────────────────────
mutate "CREATE (:Vehicle {plate: 'WA-5501', type: 'Semi-Truck', mileage: 120000, year: 2022})"
mutate "CREATE (:Vehicle {plate: 'OR-3302', type: 'Box-Truck', mileage: 85000, year: 2023})"
mutate "CREATE (:Vehicle {plate: 'CO-7703', type: 'Semi-Truck', mileage: 95000, year: 2021})"
mutate "CREATE (:Vehicle {plate: 'WA-5504', type: 'Cargo-Van', mileage: 45000, year: 2024})"

# ── Products ──────────────────────────────────────────────────────────────────
mutate "CREATE (:Product {name: 'Laptop Pro X', category: 'Electronics', price: 1299, stock: 500})"
mutate "CREATE (:Product {name: 'Wireless Mouse', category: 'Electronics', price: 29, stock: 5000})"
mutate "CREATE (:Product {name: 'Standing Desk', category: 'Furniture', price: 599, stock: 200})"
mutate "CREATE (:Product {name: 'Ergonomic Chair', category: 'Furniture', price: 449, stock: 300})"
mutate "CREATE (:Product {name: 'USB-C Hub', category: 'Electronics', price: 49, stock: 3000})"

echo "Seeding relationships..."

# ── DELIVERS_TO (Warehouse → Store) ──────────────────────────────────────────
mutate "MATCH (w:Warehouse {name: 'Seattle-HQ'}), (s:Store {name: 'Downtown Seattle'}) CREATE (w)-[:DELIVERS_TO {frequency: 'daily', avgItems: 500}]->(s)"
mutate "MATCH (w:Warehouse {name: 'Seattle-HQ'}), (s:Store {name: 'Bellevue Mall'}) CREATE (w)-[:DELIVERS_TO {frequency: 'daily', avgItems: 350}]->(s)"
mutate "MATCH (w:Warehouse {name: 'Portland-DC'}), (s:Store {name: 'Portland Central'}) CREATE (w)-[:DELIVERS_TO {frequency: 'daily', avgItems: 400}]->(s)"
mutate "MATCH (w:Warehouse {name: 'Portland-DC'}), (s:Store {name: 'Eugene Outlet'}) CREATE (w)-[:DELIVERS_TO {frequency: 'weekly', avgItems: 150}]->(s)"
mutate "MATCH (w:Warehouse {name: 'Denver-DC'}), (s:Store {name: 'Denver Union'}) CREATE (w)-[:DELIVERS_TO {frequency: 'daily', avgItems: 450}]->(s)"
mutate "MATCH (w:Warehouse {name: 'Denver-DC'}), (s:Store {name: 'Boulder Plaza'}) CREATE (w)-[:DELIVERS_TO {frequency: 'biweekly', avgItems: 100}]->(s)"
mutate "MATCH (w:Warehouse {name: 'Seattle-HQ'}), (s:Store {name: 'Portland Central'}) CREATE (w)-[:DELIVERS_TO {frequency: 'weekly', avgItems: 200}]->(s)"

# ── DRIVES (Driver → Vehicle) ────────────────────────────────────────────────
mutate "MATCH (d:Driver {name: 'Alex Thompson'}), (v:Vehicle {plate: 'WA-5501'}) CREATE (d)-[:DRIVES {since: 2022}]->(v)"
mutate "MATCH (d:Driver {name: 'Jordan Lee'}), (v:Vehicle {plate: 'OR-3302'}) CREATE (d)-[:DRIVES {since: 2023}]->(v)"
mutate "MATCH (d:Driver {name: 'Casey Kim'}), (v:Vehicle {plate: 'WA-5504'}) CREATE (d)-[:DRIVES {since: 2024}]->(v)"
mutate "MATCH (d:Driver {name: 'Pat Morgan'}), (v:Vehicle {plate: 'CO-7703'}) CREATE (d)-[:DRIVES {since: 2021}]->(v)"
mutate "MATCH (d:Driver {name: 'Sam Rivera'}), (v:Vehicle {plate: 'WA-5501'}) CREATE (d)-[:DRIVES {since: 2024}]->(v)"

# ── ASSIGNED_TO (Vehicle → Warehouse) ────────────────────────────────────────
mutate "MATCH (v:Vehicle {plate: 'WA-5501'}), (w:Warehouse {name: 'Seattle-HQ'}) CREATE (v)-[:ASSIGNED_TO]->(w)"
mutate "MATCH (v:Vehicle {plate: 'OR-3302'}), (w:Warehouse {name: 'Portland-DC'}) CREATE (v)-[:ASSIGNED_TO]->(w)"
mutate "MATCH (v:Vehicle {plate: 'CO-7703'}), (w:Warehouse {name: 'Denver-DC'}) CREATE (v)-[:ASSIGNED_TO]->(w)"
mutate "MATCH (v:Vehicle {plate: 'WA-5504'}), (w:Warehouse {name: 'Seattle-HQ'}) CREATE (v)-[:ASSIGNED_TO]->(w)"

# ── STOCKS (Store → Product) ─────────────────────────────────────────────────
mutate "MATCH (s:Store {name: 'Downtown Seattle'}), (p:Product {name: 'Laptop Pro X'}) CREATE (s)-[:STOCKS {quantity: 50}]->(p)"
mutate "MATCH (s:Store {name: 'Downtown Seattle'}), (p:Product {name: 'Wireless Mouse'}) CREATE (s)-[:STOCKS {quantity: 500}]->(p)"
mutate "MATCH (s:Store {name: 'Bellevue Mall'}), (p:Product {name: 'Laptop Pro X'}) CREATE (s)-[:STOCKS {quantity: 30}]->(p)"
mutate "MATCH (s:Store {name: 'Bellevue Mall'}), (p:Product {name: 'Standing Desk'}) CREATE (s)-[:STOCKS {quantity: 20}]->(p)"
mutate "MATCH (s:Store {name: 'Portland Central'}), (p:Product {name: 'Ergonomic Chair'}) CREATE (s)-[:STOCKS {quantity: 40}]->(p)"
mutate "MATCH (s:Store {name: 'Denver Union'}), (p:Product {name: 'USB-C Hub'}) CREATE (s)-[:STOCKS {quantity: 300}]->(p)"
mutate "MATCH (s:Store {name: 'Denver Union'}), (p:Product {name: 'Standing Desk'}) CREATE (s)-[:STOCKS {quantity: 15}]->(p)"

# ── SUPPLIES (Warehouse → Product) ───────────────────────────────────────────
mutate "MATCH (w:Warehouse {name: 'Seattle-HQ'}), (p:Product {name: 'Laptop Pro X'}) CREATE (w)-[:SUPPLIES {leadTimeDays: 2}]->(p)"
mutate "MATCH (w:Warehouse {name: 'Seattle-HQ'}), (p:Product {name: 'Wireless Mouse'}) CREATE (w)-[:SUPPLIES {leadTimeDays: 1}]->(p)"
mutate "MATCH (w:Warehouse {name: 'Seattle-HQ'}), (p:Product {name: 'USB-C Hub'}) CREATE (w)-[:SUPPLIES {leadTimeDays: 1}]->(p)"
mutate "MATCH (w:Warehouse {name: 'Portland-DC'}), (p:Product {name: 'Ergonomic Chair'}) CREATE (w)-[:SUPPLIES {leadTimeDays: 3}]->(p)"
mutate "MATCH (w:Warehouse {name: 'Denver-DC'}), (p:Product {name: 'Standing Desk'}) CREATE (w)-[:SUPPLIES {leadTimeDays: 4}]->(p)"

echo ""
echo "Verifying..."
STATS=$(curl -sf "$BASE/graphs/$GRAPH/stats")
echo "Stats: $STATS"

SCHEMA=$(curl -sf "$BASE/graphs/$GRAPH/schema")
echo "Schema: $SCHEMA"

echo ""
echo "=== Demo graph '$GRAPH' seeded successfully! ==="
echo ""
echo "Try these queries in the Cypher editor:"
echo ""
echo "  1. See all warehouses and their stores:"
echo "     MATCH (w:Warehouse)-[:DELIVERS_TO]->(s:Store) RETURN w.name, s.name, s.city"
echo ""
echo "  2. Find the delivery chain from warehouse to product:"
echo "     MATCH (w:Warehouse)-[:DELIVERS_TO]->(s:Store)-[:STOCKS]->(p:Product)"
echo "     RETURN w.name, s.name, p.name, p.price"
echo ""
echo "  3. Which drivers serve which warehouses?"
echo "     MATCH (d:Driver)-[:DRIVES]->(v:Vehicle)-[:ASSIGNED_TO]->(w:Warehouse)"
echo "     RETURN d.name, v.plate, w.name"
echo ""
echo "  4. Top stores by revenue:"
echo "     MATCH (s:Store) RETURN s.name, s.revenue ORDER BY s.revenue"
echo ""
echo "  5. Full supply chain path (3 hops):"
echo "     MATCH (w:Warehouse)-[:SUPPLIES]->(p:Product)<-[:STOCKS]-(s:Store)<-[:DELIVERS_TO]-(w2:Warehouse)"
echo "     RETURN w.name, p.name, s.name, w2.name"
