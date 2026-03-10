"""
Network Security Graph Analyzer
Flask web application for enterprise network security analysis.
Connects to SedanDelivery Bolt server on localhost:7687.
"""

import json
import os

from flask import Flask, render_template, jsonify, request
from neo4j import GraphDatabase

from network_data import load_network_data
from analyzer import (
    find_attack_paths,
    find_exposed_services,
    find_critical_vulns,
    find_lateral_paths,
    network_risk_score,
    get_full_graph,
)

app = Flask(__name__)

BOLT_URI = os.environ.get("BOLT_URI", "bolt://localhost:7687")


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

driver = GraphDatabase.driver(BOLT_URI, auth=resolve_bolt_auth())


def get_session():
    return driver.session()


@app.route('/')
def index():
    """Dashboard: network overview with stats, risk score, and top vulns."""
    with get_session() as session:
        risk = network_risk_score(session)
        exposed = find_exposed_services(session)
        vulns = find_critical_vulns(session)

        # Deduplicate vulns by CVE for the top-vulns table
        seen = set()
        unique_vulns = []
        for v in vulns:
            if v['cve'] not in seen:
                seen.add(v['cve'])
                unique_vulns.append(v)

        graph = get_full_graph(session)

    return render_template('index.html',
                           risk=risk,
                           exposed=exposed,
                           vulns=unique_vulns[:10],
                           graph_json=json.dumps(graph))


@app.route('/topology')
def topology():
    """Full network topology graph visualization."""
    with get_session() as session:
        graph = get_full_graph(session)
    return render_template('topology.html', graph_json=json.dumps(graph))


@app.route('/attack-paths')
def attack_paths():
    """Attack path visualization from Internet to critical assets."""
    with get_session() as session:
        paths = find_attack_paths(session, source='Internet')
        lateral = find_lateral_paths(session)
        graph = get_full_graph(session)

        # Get list of critical hosts for the target selector
        result = session.run("MATCH (h:Host {critical: true}) RETURN h.name AS name ORDER BY h.name")
        critical_hosts = [r['name'] for r in result]

    return render_template('attack_paths.html',
                           paths=paths,
                           lateral=lateral,
                           critical_hosts=critical_hosts,
                           graph_json=json.dumps(graph))


@app.route('/vulns')
def vulns():
    """Vulnerability heatmap showing risk by subnet."""
    with get_session() as session:
        vulns = find_critical_vulns(session)
        risk = network_risk_score(session)
        graph = get_full_graph(session)
    return render_template('vulns.html',
                           vulns=vulns,
                           risk=risk,
                           graph_json=json.dumps(graph))


# ── JSON APIs ────────────────────────────────────────────────────────

@app.route('/api/graph')
def api_graph():
    """JSON API: full graph data for external consumers."""
    with get_session() as session:
        graph = get_full_graph(session)
    return jsonify(graph)


@app.route('/api/analysis')
def api_analysis():
    """JSON API: full security analysis results."""
    with get_session() as session:
        risk = network_risk_score(session)
        exposed = find_exposed_services(session)
        vulns = find_critical_vulns(session)
        paths = find_attack_paths(session, source='Internet')
        lateral = find_lateral_paths(session)

    return jsonify({
        'risk': risk,
        'exposed_services': exposed,
        'vulnerabilities': vulns,
        'attack_paths': paths,
        'lateral_movement': lateral,
    })


@app.route('/api/attack-paths')
def api_attack_paths():
    """JSON API: attack paths to a specific target."""
    target = request.args.get('target')
    with get_session() as session:
        paths = find_attack_paths(session, source='Internet', target=target)
    return jsonify({'target': target, 'paths': paths})


if __name__ == '__main__':
    auth = resolve_bolt_auth()
    port = int(os.environ.get("PORT", "5002"))
    print("=" * 60)
    print("  Network Security Graph Analyzer")
    print("  Connecting to SedanDelivery Bolt: " + BOLT_URI)
    print("  Bolt auth: " + describe_bolt_auth(auth))
    print("=" * 60)

    # Load data on startup
    print("\n[*] Loading enterprise network data...")
    with get_session() as session:
        summary = load_network_data(session)
    print(f"[+] Loaded: {summary}")

    print(f"\n[*] Starting Flask on http://localhost:{port}")
    print("=" * 60)
    app.run(host='0.0.0.0', port=port, debug=False)
