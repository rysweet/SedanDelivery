#!/usr/bin/env python3
"""
app.py - Flask web UI for the CVE Knowledge Graph Explorer.

Connects to SedanDelivery via the neo4j Python driver (Bolt protocol)
and provides a dark-themed, security-focused interface for browsing
CVEs, CWEs, and affected Products in a graph visualization.

Routes:
    GET /           Dashboard with statistics and recent CVEs
    GET /graph      JSON graph data for vis.js visualization
    GET /cve/<id>   Detail page for a specific CVE
    GET /query      Custom Cypher query page
"""

import json
import os

from flask import Flask, jsonify, render_template, request
from neo4j import GraphDatabase

# ── Configuration ────────────────────────────────────────────────────────────

BOLT_URI = os.environ.get("BOLT_URI", "bolt://localhost:7687")
APP_DIR = os.path.dirname(os.path.abspath(__file__))


def resolve_bolt_auth(default_username: str = "neo4j") -> tuple[str, str] | None:
    password = os.environ.get("BOLT_PASSWORD", "")
    if password == "":
        return None

    username = os.environ.get("BOLT_USERNAME", default_username) or default_username
    return (username, password)


def describe_bolt_auth(auth: tuple[str, str] | None) -> str:
    if auth is None:
        return "disabled (BOLT_PASSWORD unset)"
    return f"enabled for user '{auth[0]}'"

app = Flask(__name__, static_folder=os.path.join(APP_DIR, "static"))

driver = GraphDatabase.driver(BOLT_URI, auth=resolve_bolt_auth())


# ── Helpers ──────────────────────────────────────────────────────────────────

CYPHER_WRITE_KEYWORDS = {"CREATE", "DELETE", "DETACH", "SET", "REMOVE", "MERGE", "DROP", "CALL"}


def run_query(cypher, params=None):
    """Execute a read-only query and return list of record dicts."""
    with driver.session() as session:
        result = session.run(cypher, params or {})
        return [record.data() for record in result]


def run_read_only_query(cypher):
    """Execute a user-submitted query with write-operation filtering."""
    tokens = cypher.upper().split()
    for token in tokens:
        if token in CYPHER_WRITE_KEYWORDS:
            raise ValueError(f"Write operations not allowed in query explorer. Blocked keyword: {token}")
    if len(cypher) > 2000:
        raise ValueError("Query too long (max 2000 characters)")
    return run_query(cypher)


def node_to_vis(node_id, label, props, group):
    """Convert a graph node into a vis.js node dict."""
    title_parts = [f"<b>{label}</b>"]
    for k, v in props.items():
        title_parts.append(f"{k}: {v}")
    return {
        "id": node_id,
        "label": props.get("id", props.get("product", str(node_id))),
        "group": group,
        "title": "<br>".join(title_parts),
    }


# ── Routes ───────────────────────────────────────────────────────────────────

@app.route("/")
def dashboard():
    """Dashboard with CVE statistics and recent entries."""
    stats = {}
    try:
        rows = run_query("MATCH (c:CVE) RETURN count(c) AS cnt")
        stats["cve_count"] = rows[0]["cnt"] if rows else 0

        rows = run_query("MATCH (w:CWE) RETURN count(w) AS cnt")
        stats["cwe_count"] = rows[0]["cnt"] if rows else 0

        rows = run_query("MATCH (p:Product) RETURN count(p) AS cnt")
        stats["product_count"] = rows[0]["cnt"] if rows else 0

        rows = run_query("MATCH ()-[r:HAS_WEAKNESS]->() RETURN count(r) AS cnt")
        stats["weakness_rels"] = rows[0]["cnt"] if rows else 0

        rows = run_query("MATCH ()-[r:AFFECTS]->() RETURN count(r) AS cnt")
        stats["affects_rels"] = rows[0]["cnt"] if rows else 0

        recent = run_query(
            "MATCH (c:CVE) RETURN c.id AS id, c.description AS description, "
            "c.cvss AS cvss, c.severity AS severity, c.published AS published "
            "ORDER BY c.published DESC LIMIT 10"
        )
    except Exception as e:
        recent = []
        stats = {
            "cve_count": 0, "cwe_count": 0, "product_count": 0,
            "weakness_rels": 0, "affects_rels": 0,
        }
        print(f"[!] Dashboard query error: {e}")

    return render_template("index.html", stats=stats, recent=recent)


@app.route("/graph")
def graph_data():
    """Return JSON graph data for vis.js visualization."""
    nodes = []
    edges = []
    node_ids = set()

    try:
        # CVE nodes
        cves = run_query(
            "MATCH (c:CVE) RETURN c.id AS id, c.description AS description, "
            "c.cvss AS cvss, c.severity AS severity, c.published AS published"
        )
        for i, row in enumerate(cves):
            nid = f"cve_{i}"
            node_ids.add(row["id"])
            nodes.append({
                "id": nid,
                "label": row["id"],
                "group": "CVE",
                "title": f"<b>{row['id']}</b><br>CVSS: {row.get('cvss', 'N/A')}<br>"
                         f"Severity: {row.get('severity', 'N/A')}<br>"
                         f"{(row.get('description') or '')[:100]}...",
                "value": float(row.get("cvss", 5)) * 3,
                "_key": row["id"],
            })

        # CWE nodes
        cwes = run_query("MATCH (w:CWE) RETURN w.id AS id, w.name AS name")
        for i, row in enumerate(cwes):
            nid = f"cwe_{i}"
            nodes.append({
                "id": nid,
                "label": row["id"],
                "group": "CWE",
                "title": f"<b>{row['id']}</b><br>{row.get('name', '')}",
                "value": 15,
                "_key": row["id"],
            })

        # Product nodes
        products = run_query(
            "MATCH (p:Product) RETURN p.vendor AS vendor, p.product AS product, "
            "p.version AS version"
        )
        for i, row in enumerate(products):
            nid = f"prod_{i}"
            label = f"{row['vendor']}:{row['product']}"
            nodes.append({
                "id": nid,
                "label": label,
                "group": "Product",
                "title": f"<b>{row['vendor']}</b><br>Product: {row['product']}<br>"
                         f"Version: {row.get('version', 'N/A')}",
                "value": 10,
                "_key": f"{row['vendor']}_{row['product']}_{row.get('version', '')}",
            })

        # Build lookup: key -> vis node id
        key_to_nid = {}
        for n in nodes:
            key_to_nid[n.get("_key", "")] = n["id"]

        # HAS_WEAKNESS edges
        rels = run_query(
            "MATCH (c:CVE)-[:HAS_WEAKNESS]->(w:CWE) "
            "RETURN c.id AS cve_id, w.id AS cwe_id"
        )
        for row in rels:
            from_id = key_to_nid.get(row["cve_id"])
            to_id = key_to_nid.get(row["cwe_id"])
            if from_id and to_id:
                edges.append({
                    "from": from_id,
                    "to": to_id,
                    "label": "HAS_WEAKNESS",
                    "color": {"color": "#ff6b6b"},
                    "arrows": "to",
                })

        # AFFECTS edges
        rels = run_query(
            "MATCH (c:CVE)-[:AFFECTS]->(p:Product) "
            "RETURN c.id AS cve_id, p.vendor AS vendor, p.product AS product, "
            "p.version AS version"
        )
        for row in rels:
            from_id = key_to_nid.get(row["cve_id"])
            pkey = f"{row['vendor']}_{row['product']}_{row.get('version', '')}"
            to_id = key_to_nid.get(pkey)
            if from_id and to_id:
                edges.append({
                    "from": from_id,
                    "to": to_id,
                    "label": "AFFECTS",
                    "color": {"color": "#ffa726"},
                    "arrows": "to",
                })

        # Strip internal keys
        for n in nodes:
            n.pop("_key", None)

    except Exception as e:
        print(f"[!] Graph query error: {e}")
        return jsonify({"nodes": [], "edges": [], "error": "Graph query failed"})

    return jsonify({"nodes": nodes, "edges": edges})


@app.route("/cve/<cve_id>")
def cve_detail(cve_id):
    """Detail page for a specific CVE."""
    cve = None
    weaknesses = []
    products = []

    try:
        rows = run_query(
            "MATCH (c:CVE {id: $id}) RETURN c.id AS id, c.description AS description, "
            "c.cvss AS cvss, c.severity AS severity, c.published AS published",
            {"id": cve_id},
        )
        if rows:
            cve = rows[0]

        weaknesses = run_query(
            "MATCH (c:CVE {id: $id})-[:HAS_WEAKNESS]->(w) "
            "RETURN w.id AS id, w.name AS name",
            {"id": cve_id},
        )

        products = run_query(
            "MATCH (c:CVE {id: $id})-[:AFFECTS]->(p:Product) "
            "RETURN p.vendor AS vendor, p.product AS product, p.version AS version",
            {"id": cve_id},
        )
    except Exception as e:
        print(f"[!] CVE detail query error: {e}")

    return render_template(
        "cve_detail.html", cve=cve, weaknesses=weaknesses, products=products
    )


@app.route("/query", methods=["GET", "POST"])
def query_page():
    """Custom Cypher query page."""
    results = None
    error = None
    cypher = ""
    columns = []

    if request.method == "POST":
        cypher = request.form.get("cypher", "").strip()
        if cypher:
            try:
                results = run_read_only_query(cypher)
                if results:
                    columns = list(results[0].keys())
            except ValueError as e:
                error = str(e)
            except Exception:
                error = "Query execution failed. Check syntax and try again."

    return render_template(
        "query.html", cypher=cypher, results=results, error=error, columns=columns
    )


# ── Startup ──────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    port = int(os.environ.get("PORT", 5001))
    auth = resolve_bolt_auth()
    print(f"[*] CVE Knowledge Graph Explorer starting on port {port}")
    print(f"[*] Bolt connection: {BOLT_URI}")
    print(f"[*] Bolt auth: {describe_bolt_auth(auth)}")
    app.run(host="0.0.0.0", port=port, debug=False)
