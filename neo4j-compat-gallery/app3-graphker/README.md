# App 3: GraphKer-style CVE Knowledge Graph Explorer

A Python application inspired by [GraphKer](https://github.com/geofragkos/GraphKer) that builds a vulnerability knowledge graph from real CVE data and provides an interactive web UI for exploration.

## What It Does

1. **Fetches CVE data** from the NVD (National Vulnerability Database) API v2, or uses hardcoded sample data for offline demos
2. **Creates a knowledge graph** in SedanDelivery via the neo4j Python driver (Bolt v4.4 protocol)
3. **Provides a Flask web UI** with graph visualization, CVE details, and custom Cypher queries

### Graph Schema

```
(:CVE {id, description, cvss, severity, published})
  -[:HAS_WEAKNESS]-> (:CWE {id, name})
  -[:AFFECTS]->      (:Product {vendor, product, version})
```

### Sample Data (15 CVEs)

The offline sample includes well-known critical vulnerabilities:

| CVE | Name | CVSS |
|-----|------|------|
| CVE-2021-44228 | Log4Shell | 10.0 |
| CVE-2021-34473 | ProxyShell | 9.8 |
| CVE-2020-1472 | Zerologon | 10.0 |
| CVE-2022-22965 | Spring4Shell | 9.8 |
| CVE-2023-34362 | MOVEit Transfer SQLi | 9.8 |
| CVE-2024-3094 | XZ Utils Backdoor | 10.0 |
| CVE-2021-26855 | ProxyLogon | 9.8 |
| CVE-2023-23397 | Outlook EoP | 9.8 |
| CVE-2024-21887 | Ivanti Connect Secure | 9.1 |
| CVE-2023-27997 | FortiOS Heap Overflow | 9.8 |
| ... and 5 more | | |

## Prerequisites

- Python 3.11+
- SedanDelivery Bolt server running on `localhost:7687`
- pip (Python package manager)

## Quick Start

```bash
# Make run script executable
chmod +x run.sh

# Load sample data and start the web UI
./run.sh

# Or fetch live data from NVD API
./run.sh --api

# Or just load data without starting the server
./run.sh --load-only
```

The web UI will be available at **http://localhost:5001**.

## Manual Steps

```bash
# Install dependencies
pip install -r requirements.txt

# Load sample CVE data
python3 cve_loader.py --sample

# Or fetch from NVD API (requires internet)
python3 cve_loader.py

# Start the Flask web server
python3 app.py
```

## Web UI Routes

| Route | Description |
|-------|-------------|
| `GET /` | Dashboard with statistics, graph visualization, and recent CVEs |
| `GET /graph` | JSON endpoint returning vis.js graph data |
| `GET /cve/<id>` | Detail page for a specific CVE |
| `GET /query` | Custom Cypher query page with example queries |

## Configuration

| Environment Variable | Default | Description |
|---------------------|---------|-------------|
| `BOLT_URI` | `bolt://localhost:7687` | SedanDelivery Bolt endpoint |
| `PORT` | `5001` | Flask web server port |

## File Structure

```
app3-graphker/
  app.py             Flask web application
  cve_loader.py      NVD API fetcher and graph loader
  sample_cves.py     Hardcoded sample CVE data (15 CVEs)
  requirements.txt   Python dependencies
  run.sh             Setup and launch script
  README.md          This file
  templates/
    index.html       Dashboard with vis.js graph
    cve_detail.html  CVE detail page
    query.html       Custom Cypher query page
```

## Example Cypher Queries

```cypher
-- Top CVEs by CVSS score
MATCH (c:CVE) RETURN c.id, c.cvss, c.severity ORDER BY c.cvss DESC LIMIT 10

-- CWEs ranked by number of associated CVEs
MATCH (c:CVE)-[:HAS_WEAKNESS]->(w:CWE)
RETURN w.id, w.name, count(c) AS cve_count
ORDER BY cve_count DESC

-- Most affected products
MATCH (c:CVE)-[:AFFECTS]->(p:Product)
RETURN p.vendor, p.product, count(c) AS vuln_count
ORDER BY vuln_count DESC LIMIT 10

-- Full attack paths
MATCH (c:CVE)-[:HAS_WEAKNESS]->(w:CWE), (c)-[:AFFECTS]->(p:Product)
RETURN c.id, w.id, p.vendor, p.product
LIMIT 20
```

## Neo4j Driver Compatibility

This application demonstrates SedanDelivery's compatibility with the official neo4j Python driver. The connection uses `auth=None` since SedanDelivery's Bolt server does not require authentication:

```python
from neo4j import GraphDatabase
driver = GraphDatabase.driver("bolt://localhost:7687", auth=None)
```

All standard Cypher operations (CREATE, MATCH, MERGE, DETACH DELETE, ORDER BY, LIMIT, aggregation functions) work through the Bolt v4.4 protocol.
