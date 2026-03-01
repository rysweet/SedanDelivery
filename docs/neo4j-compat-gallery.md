---
layout: default
title: Neo4j Compatibility Gallery
---

# Neo4j Compatibility Gallery

Validating SedanDelivery's Bolt v4.4 protocol with real-world open-source security applications.
All 4 apps connect via **standard Neo4j drivers without any code modifications**.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                    Neo4j Compatibility Gallery                   │
├─────────────────────────────────────────────────────────────────┤
│                                                                 │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────┐  ┌────────┐ │
│  │   Neodash     │  │  neovis.js   │  │ GraphKer │  │NetSec  │ │
│  │  Dashboard    │  │  Threat Viz  │  │ CVE Graph│  │Analyzer│ │
│  │  :5005        │  │  :8080       │  │  :5001   │  │  :5002 │ │
│  └──────┬───────┘  └──────┬───────┘  └────┬─────┘  └───┬────┘ │
│         │     neo4j-driver (Bolt v4.4)     │            │      │
│         └──────────┬──────┴────────────────┴────────────┘      │
│                    │                                            │
│         ┌──────────▼──────────┐                                │
│         │  sedandelivery-bolt  │                                │
│         │   Bolt Server :7687  │                                │
│         └──────────┬──────────┘                                │
│                    │                                            │
│         ┌──────────▼──────────┐                                │
│         │     gql-core         │                                │
│         │  Rust Graph Engine   │                                │
│         └─────────────────────┘                                │
└─────────────────────────────────────────────────────────────────┘
```

---

## App 1: Neodash — Security Dashboard Builder

**Port 5005** · [Neo4j Labs](https://github.com/neo4j-labs/neodash) · Docker

A Neo4j Labs dashboard builder pre-configured with a 3-page MITRE ATT&CK security
dashboard. Tables, bar charts, and interactive graph visualizations — all powered by
Cypher queries against SedanDelivery.

**Dashboard Pages:**
- Threat Landscape Overview — group inventory, techniques-per-tactic chart
- Attack Paths & Kill Chains — full actor-technique-software graph
- Mitigations & Coverage — defense coverage analysis

![Neodash Dashboard](../neo4j-compat-gallery/screenshots/01-neodash-dashboard.png)

---

## App 2: neovis.js — Threat Intelligence Visualization

**Port 8080** · [neo4j-contrib/neovis.js](https://github.com/neo4j-contrib/neovis.js) · Browser JS

A standalone HTML page using the neovis.js library that connects directly to
SedanDelivery's Bolt endpoint from the browser. Four visualization tabs with
color-coded nodes, custom Cypher query editor, and pre-built examples.

**Tabs:** Threat Actor Network · Kill Chain View · CVE Impact Map · Full ATT&CK Graph

![neovis.js Threat Intelligence](../neo4j-compat-gallery/screenshots/02-neovis-threat-graph.png)

---

## App 3: GraphKer — CVE Knowledge Graph Explorer

**Port 5001** · Python Flask + neo4j driver v6.1.0

Inspired by [GraphKer](https://github.com/amberzovitis/GraphKer), this app loads
CVE/CWE/CPE vulnerability data into SedanDelivery and provides a web UI for
exploring relationships. Supports live NVD API fetching or offline sample data
(15 critical CVEs including Log4Shell, ProxyShell, Zerologon, Spring4Shell).

**Routes:** Dashboard · CVE Detail · Custom Cypher Query · Graph API

![GraphKer CVE Explorer](../neo4j-compat-gallery/screenshots/03-graphker-dashboard.png)

---

## App 4: Network Security Graph Analyzer

**Port 5002** · Python Flask + neo4j driver v6.1.0

Enterprise network security analysis powered by graph queries. Models 20 hosts
across 5 subnets (DMZ, Corporate, Server, Management, OT/SCADA) with firewalls,
services, and real CVEs. Computes risk scores, traces attack paths, and visualizes
vulnerability distribution.

### Dashboard
![Network Security Dashboard](../neo4j-compat-gallery/screenshots/04-network-dashboard.png)

### Network Topology
Interactive graph with color-coded subnets and firewall rules.

![Network Topology](../neo4j-compat-gallery/screenshots/04b-network-topology.png)

### Attack Path Analysis
Traces routes from Internet to critical assets with hop counts.

![Attack Paths](../neo4j-compat-gallery/screenshots/04c-attack-paths.png)

### Vulnerability Heatmap
CVE distribution across subnets with severity ratings.

![Vulnerability Heatmap](../neo4j-compat-gallery/screenshots/04d-vulnerabilities.png)

---

## Compatibility Matrix

| Feature | Status |
|---------|--------|
| Bolt v4.4 handshake | PASS |
| PackStream encoding | PASS |
| auth=None connections | PASS |
| CREATE / MATCH / MERGE | PASS |
| Variable-length paths | PASS |
| Aggregation (count, avg) | PASS |
| Parameterized queries | PASS |
| Python neo4j driver v6.1.0 | PASS |
| JS neo4j-driver (neovis.js) | PASS |
| Docker cross-container Bolt | PASS |

## Quick Start

```bash
# 1. Build the Bolt server
cargo build --release -p sedandelivery-bolt

# 2. Start it
./target/release/sedandelivery-bolt &

# 3. Load data + start all apps
./neo4j-compat-gallery/run-all.sh
```

See [`neo4j-compat-gallery/README.md`](https://github.com/cloud-ecosystem-security/SedanDelivery/tree/main/neo4j-compat-gallery) for full instructions, Docker Compose setup, and per-app documentation.
