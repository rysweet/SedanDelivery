# App 2: neovis.js — MITRE ATT&CK Threat Intelligence Dashboard

Interactive graph visualization of the MITRE ATT&CK dataset using
[neovis.js](https://github.com/neo4j-contrib/neovis.js) in the browser and a
small Python Bolt proxy (`server.py`) behind it.

## What is neovis.js?

neovis.js combines the [neo4j-javascript-driver](https://github.com/neo4j/neo4j-javascript-driver) with [vis.js](https://visjs.org/) to render interactive graph visualizations directly in the browser. It connects to any Bolt-compatible server (Neo4j or SedanDelivery) and turns Cypher query results into force-directed graph layouts.

## Prerequisites

- **SedanDelivery Bolt** running on `localhost:7687` (no auth)
- **MITRE ATT&CK dataset** loaded (86 nodes, 142 relationships)
- **Python** 3.11+ with pip

## Quick Start

```bash
# 1. Ensure SedanDelivery Bolt is running
cd /home/azureuser/src/SedanDelivery
export BOLT_USERNAME=neo4j
export BOLT_PASSWORD=change-me   # optional; omit for intentional no-auth mode
cargo run --release --bin sedandelivery-bolt

# 2. Load the MITRE ATT&CK dataset (if not already loaded)
python3 neo4j-compat-gallery/data/load_data.py

# 3. Start the dashboard
cd neo4j-compat-gallery/app2-neovis
./run.sh
```

Open <http://localhost:8080> in your browser.

## Features

### Four Visualization Views

| View | Description | Cypher Pattern |
|------|-------------|----------------|
| **Threat Actor Network** | ThreatGroups connected to Techniques and Software | `(ThreatGroup)-[USES]->(Technique)` |
| **Kill Chain View** | Emotet -> TrickBot -> Ryuk delivery chain with implementing techniques | `(Emotet)-[DELIVERS]->(TrickBot)-[DELIVERS]->(Ryuk)` |
| **CVE Impact Map** | CVEs linked to Infrastructure targets and enabling Techniques | `(CVE)-[ENABLES/TARGETS]->(...)` |
| **Full ATT&CK Graph** | Complete graph — all 86 nodes and 142 relationships | `(n)-[r]->(m)` |

### Interactive Features

- **Click nodes** to inspect all properties in the detail panel
- **Zoom/pan** with mouse wheel and drag
- **Custom Cypher queries** with Ctrl+Enter execution
- **Pre-built example queries** in the sidebar
- **Zoom controls** and fit-to-screen button
- **Physics re-stabilization** for layout adjustments

### Node Color Legend

| Label | Color |
|-------|-------|
| ThreatGroup | Red |
| Technique | Blue |
| Mitigation | Green |
| Software | Orange |
| CVE | Purple |
| Campaign | Yellow |
| Tactic | Cyan |
| Infrastructure | Pink |

## Architecture

```
index.html          Single-page application (no build step required)
  |
  +-- server.py     Tiny Python proxy using neo4j-driver
  |     |
  |     +-- bolt://localhost:7687   SedanDelivery Bolt v4.4 server
  |
  +-- vis-network / neovis.js loaded from CDN
```

All UI dependencies load from CDN. The only local runtime dependency is the
Python `neo4j` package used by the proxy server.

## Custom Queries

The dashboard includes a Cypher query editor. Example queries to try:

```cypher
-- Russian threat groups and their techniques
MATCH (tg:ThreatGroup)-[r:USES]->(t:Technique)
WHERE tg.country = 'Russia'
RETURN tg, r, t

-- Techniques with the most mitigations
MATCH (m:Mitigation)-[r:MITIGATES]->(t:Technique)
RETURN m, r, t

-- Full attack path from threat group to infrastructure
MATCH p=(tg:ThreatGroup)-[*1..4]->(i:Infrastructure)
RETURN p LIMIT 50

-- Software that implements credential access techniques
MATCH (s:Software)-[:IMPLEMENTS]->(t:Technique)-[:BELONGS_TO]->(tac:Tactic {name: 'Credential Access'})
RETURN s, t, tac
```

## Compatibility Notes

The gallery proxy uses the official Python `neo4j` driver and derives auth from
`BOLT_USERNAME` / `BOLT_PASSWORD`. If `BOLT_PASSWORD` is unset, it
intentionally falls back to `auth=None` for no-auth demos.

## Troubleshooting

| Issue | Solution |
|-------|----------|
| "Connecting..." never resolves | Verify SedanDelivery Bolt is running: `nc -z localhost 7687` |
| Empty graph on a view | Ensure the MITRE ATT&CK dataset is loaded |
| Port 8080 in use | Change `HTTP_PORT` in `run.sh` or stop the other process |
| Proxy cannot connect to Bolt | Export `BOLT_PASSWORD`/`BOLT_USERNAME` to match the Bolt server |
