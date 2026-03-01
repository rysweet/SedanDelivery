---
layout: default
title: Neo4j Compatibility Gallery
---

# Neo4j Compatibility Gallery

Four security-focused open-source applications running against SedanDelivery's Bolt v4.4
protocol — proving drop-in Neo4j compatibility with **zero code modifications** to any app.

## Quick Start

```bash
# Prerequisites: Rust toolchain, Python 3.11+, Node.js 18+
git clone https://github.com/cloud-ecosystem-security/SedanDelivery.git
cd SedanDelivery

# 1. Build the Bolt server (~75 seconds first time)
cargo build --release -p sedandelivery-bolt

# 2. Start the Bolt server (Neo4j-compatible on port 7687)
./target/release/sedandelivery-bolt &

# 3. Install Python dependencies
pip install neo4j flask requests

# 4. Load MITRE ATT&CK threat intelligence data (86 nodes, 142 relationships)
python3 neo4j-compat-gallery/data/load_data.py --clear

# 5. Load CVE vulnerability data (15 critical CVEs, 28 products)
python3 neo4j-compat-gallery/app3-graphker/cve_loader.py --sample --no-clear

# 6. Start all 4 apps
./neo4j-compat-gallery/run-all.sh
```

**After startup, open these URLs:**

| App | URL | What You'll See |
|-----|-----|-----------------|
| Neodash | [localhost:5005](http://localhost:5005) | Dashboard builder — connect with bolt://localhost:7687 |
| neovis.js | [localhost:8080](http://localhost:8080) | Threat intelligence graph visualization |
| GraphKer | [localhost:5001](http://localhost:5001) | CVE knowledge graph explorer |
| Network Security | [localhost:5002](http://localhost:5002) | Enterprise network attack surface analyzer |

**Stop everything:** `./neo4j-compat-gallery/run-all.sh --stop`

---

## How This Was Tested

Each app was validated end-to-end from a **fresh `git clone`** of this repository:

1. Built `sedandelivery-bolt` from source (Rust)
2. Started the Bolt server on `localhost:7687` with no authentication
3. Loaded security datasets via the standard **Python neo4j driver v6.1.0** (`auth=None`)
4. Started each app — they connect to SedanDelivery using the same driver configuration they would use for a real Neo4j instance
5. Verified all HTTP endpoints return 200, all graph queries return data, and all visualizations render
6. Captured screenshots with **Playwright** headless Chromium at 1600x1000 viewport
7. Verified write protection on the query endpoint (DETACH DELETE correctly blocked)

**Total graph after all data loaded:** 204 nodes, 413 relationships — all apps coexist in a single shared graph.

---

## App 1: Neodash — Security Dashboard Builder

**Port 5005** · [Neo4j Labs](https://github.com/neo4j-labs/neodash) · Docker container

Neodash is an open-source dashboard builder maintained by Neo4j Labs. It runs as a Docker
container and connects to any Bolt-compatible server. Here it connects to SedanDelivery
with the same `bolt://localhost:7687` connection string you would use for Neo4j.

The gallery includes a pre-built `dashboard.json` with 3 pages of MITRE ATT&CK analysis:
threat group inventory, techniques-per-tactic breakdown, and mitigation coverage graphs.

```bash
# Run standalone (Docker required)
docker run -d --network=host -p 5005:5005 \
  -e standalone=true -e standaloneProtocol=bolt \
  -e standaloneHost=localhost -e standalonePort=7687 \
  neo4jlabs/neodash:latest
```

![Neodash Dashboard](assets/images/gallery/01-neodash-dashboard.png)

---

## App 2: neovis.js — Threat Intelligence Visualization

**Port 8080** · [neo4j-contrib/neovis.js](https://github.com/neo4j-contrib/neovis.js) · Static HTML

A single HTML file that loads `neovis.js` from CDN. The JavaScript neo4j-driver connects
directly from the browser to SedanDelivery's Bolt endpoint — no backend server needed.

Four visualization tabs render the MITRE ATT&CK dataset with color-coded nodes
(red=ThreatGroup, blue=Technique, green=Mitigation, orange=Software, purple=CVE),
plus a custom Cypher query editor.

```bash
# Run with any static file server
cd neo4j-compat-gallery/app2-neovis
npx serve -l 8080
```

![neovis.js Threat Intelligence](assets/images/gallery/02-neovis-threat-graph.png)

---

## App 3: GraphKer — CVE Knowledge Graph Explorer

**Port 5001** · Python Flask + neo4j driver v6.1.0

Inspired by [GraphKer](https://github.com/amberzovitis/GraphKer), this app loads
real CVE/CWE/CPE data from the NVD API (or offline sample data with 15 critical CVEs
including Log4Shell, ProxyShell, Zerologon, and Spring4Shell) and provides a web UI
for exploring vulnerability relationships.

The `/query` endpoint includes **write protection** — destructive Cypher keywords
(CREATE, DELETE, SET, MERGE, DROP) are blocked.

```bash
cd neo4j-compat-gallery/app3-graphker
pip install -r requirements.txt
python3 cve_loader.py --sample    # Load 15 CVEs offline
python3 app.py                    # Start on port 5001
```

![GraphKer CVE Explorer](assets/images/gallery/03-graphker-dashboard.png)

![GraphKer CVE Detail — Log4Shell](assets/images/gallery/03b-graphker-cve-detail.png)

---

## App 4: Network Security Graph Analyzer

**Port 5002** · Python Flask + neo4j driver v6.1.0

The most complex app in the gallery. Models an enterprise network as a graph:
20 hosts across 5 subnets (DMZ, Corporate, Server, Management, OT/SCADA),
15 services, 10 firewall rules, and 12 real CVEs. Then runs graph-based security
analysis: attack path tracing, lateral movement detection, vulnerability heatmapping,
and composite risk scoring.

```bash
cd neo4j-compat-gallery/app4-network-security
pip install -r requirements.txt
python3 app.py    # Loads data on startup, starts on port 5002
```

### Dashboard — Risk Score and Subnet Breakdown
![Network Security Dashboard](assets/images/gallery/04-network-dashboard.png)

### Network Topology — Interactive Graph
20 hosts, 5 subnets, 10 firewalls. Click any node to inspect. Color-coded by zone.
![Network Topology](assets/images/gallery/04b-network-topology.png)

### Attack Path Analysis — Internet to SCADA
Traces attack vectors from the Internet to critical OT assets. The longest chain:
Internet -> web-server-01 (DMZ) -> app-server-01 (Server) -> jump-box-01 (Management) -> scada-hmi-01 (OT) — 4 hops.
![Attack Paths](assets/images/gallery/04c-attack-paths.png)

### Vulnerability Heatmap — Risk by Subnet
OT subnet shows the highest average CVSS (9.9). Corporate and DMZ are also critical.
Management subnet is clean (0 vulnerabilities).
![Vulnerability Heatmap](assets/images/gallery/04d-vulnerabilities.png)

---

## Compatibility Matrix

Every feature below was exercised by at least one gallery app during testing.

| Feature | Status | Tested By |
|---------|--------|-----------|
| Bolt v4.4 handshake | PASS | All 4 apps |
| PackStream encoding | PASS | All 4 apps |
| `auth=None` connections | PASS | All 4 apps |
| CREATE nodes & edges | PASS | Data loaders |
| MATCH with WHERE filters | PASS | All query endpoints |
| MERGE (upsert) | PASS | CVE loader |
| DETACH DELETE | PASS | Data cleanup |
| Variable-length paths (`*1..4`) | PASS | Attack path analyzer |
| Aggregation (count, avg, max) | PASS | Risk score, dashboards |
| Parameterized queries (`$param`) | PASS | CVE detail, analyzers |
| ORDER BY / LIMIT / SKIP | PASS | CVE listing, top vulns |
| String functions (CONTAINS, etc.) | PASS | Search features |
| Python neo4j driver v6.1.0 | PASS | Apps 3 & 4 |
| JavaScript neo4j-driver | PASS | App 2 (neovis.js) |
| Docker cross-container Bolt | PASS | App 1 (Neodash) |

## Architecture

![Gallery Architecture](assets/images/gallery/architecture.svg)

## Source Code

Full source code, Docker Compose configuration, data loaders, and per-app READMEs:
[`neo4j-compat-gallery/`](https://github.com/cloud-ecosystem-security/SedanDelivery/tree/main/neo4j-compat-gallery)
