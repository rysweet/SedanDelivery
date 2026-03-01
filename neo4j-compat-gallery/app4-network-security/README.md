# App 4: Network Security Graph Analyzer

A Python Flask application that models an enterprise network as a graph and performs security analysis using SedanDelivery's Bolt protocol server.

## What It Does

- Models enterprise network topology (hosts, subnets, firewalls, services)
- Maps CVE vulnerabilities to hosts
- Finds attack paths from external networks to critical assets
- Identifies lateral movement opportunities between subnets
- Calculates composite risk scores by subnet and overall

## Network Model

- **5 Subnets**: DMZ, Corporate, Server, Management, OT
- **20 Hosts**: web servers, databases, workstations, SCADA/PLC, etc.
- **15 Services**: HTTP, HTTPS, SSH, RDP, SQL, Modbus, OPC-UA, etc.
- **10 Firewalls/ACLs**: allow and deny rules between zones
- **12 Vulnerabilities**: real CVEs mapped to hosts (2023-2024)

## Pages

| Route | Description |
|-------|-------------|
| `/` | Dashboard with stats, risk score, subnet breakdown, attack surface |
| `/topology` | Full interactive network graph (vis.js) with click-to-inspect |
| `/attack-paths` | Attack path visualization with target selector |
| `/vulns` | Vulnerability heatmap by subnet with CVE details |
| `/api/graph` | JSON API - full graph data |
| `/api/analysis` | JSON API - complete security analysis |

## Prerequisites

- Python 3.11+
- SedanDelivery Bolt server running on `localhost:7687`

## Running

```bash
chmod +x run.sh
./run.sh
```

Then open http://localhost:5002

## Architecture

```
app.py              - Flask web app (routes, templates)
network_data.py     - Enterprise network dataset (Cypher CREATE statements)
analyzer.py         - Security analysis queries (Cypher MATCH patterns)
templates/
  base.html         - Dark theme layout with vis.js CDN
  index.html        - Dashboard
  topology.html     - Full network topology graph
  attack_paths.html - Attack path visualization
  vulns.html        - Vulnerability heatmap
```

## Connection

Uses the neo4j Python driver with no authentication:

```python
driver = GraphDatabase.driver("bolt://localhost:7687", auth=None)
```
