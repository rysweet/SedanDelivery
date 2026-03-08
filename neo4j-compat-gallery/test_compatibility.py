#!/usr/bin/env python3
"""Neo4j Compatibility Gallery — Automated Test Suite.

Starts the SedanDelivery Bolt server, loads all 3 datasets, and runs 31
protocol compatibility queries that mirror real-world usage from Neodash,
neovis.js, GraphKer, and the Network Security Analyzer.

Usage:
    # Against a running Bolt server:
    python3 test_compatibility.py

    # Start Bolt server automatically (requires built binary):
    python3 test_compatibility.py --start-server

    # Custom URI:
    python3 test_compatibility.py --uri bolt://host:7687

Exit code 0 = all tests pass, 1 = failures.
"""
import argparse
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent

# ---------------------------------------------------------------------------
# Bolt server management
# ---------------------------------------------------------------------------

_server_proc = None


def start_bolt_server(binary: str | None = None) -> int:
    """Start the Bolt server and return its PID."""
    global _server_proc
    if binary is None:
        binary = str(REPO_ROOT / "target" / "release" / "sedandelivery-bolt")
    if not os.path.isfile(binary):
        print(f"ERROR: Bolt binary not found at {binary}", file=sys.stderr)
        print("       Build it with: cargo build --release -p sedandelivery-bolt", file=sys.stderr)
        sys.exit(2)

    env = {**os.environ, "RUST_LOG": "warn"}
    _server_proc = subprocess.Popen(
        [binary], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    # Wait for it to be ready
    for _ in range(30):
        import socket
        try:
            with socket.create_connection(("localhost", 7687), timeout=1):
                return _server_proc.pid
        except (ConnectionRefusedError, OSError):
            time.sleep(0.5)
    print("ERROR: Bolt server did not start within 15 seconds", file=sys.stderr)
    _server_proc.kill()
    sys.exit(2)


def stop_bolt_server():
    global _server_proc
    if _server_proc is not None:
        _server_proc.send_signal(signal.SIGTERM)
        _server_proc.wait(timeout=10)
        _server_proc = None


# ---------------------------------------------------------------------------
# Data loaders
# ---------------------------------------------------------------------------

def load_mitre_attack(session):
    """Load MITRE ATT&CK dataset."""
    loader = SCRIPT_DIR / "data" / "load_data.py"
    if not loader.exists():
        print("  SKIP  MITRE loader not found")
        return
    # Use the Python module directly
    sys.path.insert(0, str(SCRIPT_DIR / "data"))
    from load_data import load_cypher_file, verify_data
    cypher_file = SCRIPT_DIR / "data" / "load_mitre_attack.cypher"
    session.run("MATCH (n) DETACH DELETE n")
    stats = load_cypher_file(session, cypher_file)
    print(f"  Loaded MITRE ATT&CK: {stats['nodes_created']} nodes, {stats['relationships_created']} rels")


def load_cve_data(session):
    """Load CVE knowledge graph (sample mode)."""
    sys.path.insert(0, str(SCRIPT_DIR / "app3-graphker"))
    from sample_cves import get_sample_cypher_statements
    stmts = get_sample_cypher_statements()
    for stmt in stmts:
        session.run(stmt)
    print(f"  Loaded CVE data: {len(stmts)} statements")


def load_network_data(session):
    """Load enterprise network dataset."""
    sys.path.insert(0, str(SCRIPT_DIR / "app4-network-security"))
    from network_data import load_network_data as _load
    stats = _load(session)
    if stats:
        print(f"  Loaded Network: {stats.get('hosts', '?')} hosts, {stats.get('subnets', '?')} subnets")
    else:
        print("  Loaded Network data")


# ---------------------------------------------------------------------------
# Test runner
# ---------------------------------------------------------------------------

class TestRunner:
    def __init__(self, session):
        self.session = session
        self.passed = 0
        self.failed = 0
        self.failures = []

    def test(self, name, query, check, params=None):
        try:
            r = self.session.run(query, **(params or {}))
            records = list(r)
            ok, detail = check(records)
            if ok:
                self.passed += 1
                print(f"  PASS  {name}: {detail}")
            else:
                self.failed += 1
                self.failures.append(f"{name}: {detail}")
                print(f"  FAIL  {name}: {detail}")
        except Exception as e:
            self.failed += 1
            self.failures.append(f"{name}: EXCEPTION {e}")
            print(f"  FAIL  {name}: {e}")

    def sv(self, records):
        """Extract single value."""
        return records[0]["v"] if records else None


def run_tests(uri: str) -> int:
    """Run all compatibility tests. Returns exit code."""
    from neo4j import GraphDatabase

    driver = GraphDatabase.driver(uri)

    # Load datasets
    print("\n--- Loading datasets ---")
    with driver.session() as s:
        load_mitre_attack(s)
        load_cve_data(s)
        load_network_data(s)

    # Run tests
    with driver.session() as s:
        t = TestRunner(s)
        sv = t.sv

        print("\n--- App1/App2: MITRE ATT&CK (Neodash + neovis.js) ---")
        t.test("ThreatGroup count", "MATCH (n:ThreatGroup) RETURN count(n) AS v",
               lambda r: (sv(r) == 8, sv(r)))
        t.test("Tactic count", "MATCH (n:Tactic) RETURN count(n) AS v",
               lambda r: (sv(r) == 14, sv(r)))
        t.test("Technique count", "MATCH (n:Technique) RETURN count(n) AS v",
               lambda r: (sv(r) == 29, sv(r)))
        t.test("Software count", "MATCH (n:Software) RETURN count(n) AS v",
               lambda r: (sv(r) == 11, sv(r)))
        t.test("Group->Software traversal",
               "MATCH (g:ThreatGroup)-[:USES_SOFTWARE]->(s:Software) RETURN g.name, s.name",
               lambda r: (len(r) > 0, f"{len(r)} pairs"))
        t.test("Kill chain multi-hop",
               "MATCH (g:ThreatGroup)-[:USES]->(t:Technique)-[:BELONGS_TO]->(tac:Tactic) "
               "RETURN g.name, t.name, tac.name LIMIT 10",
               lambda r: (len(r) > 0, f"{len(r)} paths"))
        t.test("Aggregation bar chart",
               "MATCH (t:Technique)-[:BELONGS_TO]->(tac:Tactic) "
               "RETURN tac.name, count(t) AS cnt ORDER BY cnt DESC LIMIT 3",
               lambda r: (len(r) == 3, [(x["tac.name"], x["cnt"]) for x in r]))
        t.test("WHERE IN list",
               "MATCH (t:Tactic) WHERE t.name IN ['Initial Access','Execution','Persistence'] "
               "RETURN count(t) AS v",
               lambda r: (sv(r) == 3, sv(r)))
        t.test("String CONTAINS",
               "MATCH (t:Technique) WHERE t.name CONTAINS 'Phishing' RETURN t.name AS v",
               lambda r: (len(r) > 0, [x["v"] for x in r]))
        t.test("Mimikatz users",
               "MATCH (g:ThreatGroup)-[:USES_SOFTWARE]->(s:Software {name:'Mimikatz'}) "
               "RETURN g.name AS v",
               lambda r: (len(r) == 2, sorted([x["v"] for x in r])))
        t.test("COLLECT + size",
               "MATCH (g:ThreatGroup)-[:USES]->(t:Technique) "
               "WITH g, collect(t.name) AS techs RETURN g.name, size(techs) AS n "
               "ORDER BY n DESC LIMIT 3",
               lambda r: (len(r) == 3, [(x["g.name"], x["n"]) for x in r]))
        t.test("OPTIONAL MATCH",
               "MATCH (g:ThreatGroup) OPTIONAL MATCH (g)-[:CONDUCTED]->(c:Campaign) "
               "RETURN g.name, c.name LIMIT 8",
               lambda r: (len(r) > 0, f"{len(r)} rows"))
        t.test("DISTINCT",
               "MATCH (g:ThreatGroup)-[:USES]->(t:Technique) "
               "RETURN DISTINCT t.name AS tech ORDER BY tech",
               lambda r: (len(r) > 0, f"{len(r)} distinct techniques"))
        t.test("UNWIND",
               "UNWIND ['APT28','Lazarus Group','Sandworm'] AS n "
               "MATCH (g:ThreatGroup {name: n}) RETURN g.name, g.country",
               lambda r: (len(r) == 3, [(x["g.name"], x["g.country"]) for x in r]))
        t.test("EXISTS subquery",
               'MATCH (g:ThreatGroup) WHERE EXISTS { '
               '(g)-[:USES_SOFTWARE]->(:Software {name: "Mimikatz"}) } RETURN g.name AS v',
               lambda r: (len(r) == 2, sorted([x["v"] for x in r])))
        t.test("Parameterized query",
               "MATCH (n) WHERE n.name = $val RETURN n.name AS v",
               lambda r: (len(r) > 0, [x["v"] for x in r]),
               params={"val": "Mimikatz"})
        t.test("CASE expression",
               "MATCH (c:CVE) RETURN c.id, "
               "CASE WHEN c.cvss >= 9.0 THEN 'CRITICAL' "
               "WHEN c.cvss >= 7.0 THEN 'HIGH' ELSE 'MEDIUM' END AS sev LIMIT 5",
               lambda r: (len(r) == 5, f"{len(r)} results"))
        t.test("ORDER BY + SKIP + LIMIT",
               "MATCH (c:CVE) RETURN c.id AS v ORDER BY c.cvss DESC SKIP 2 LIMIT 3",
               lambda r: (len(r) == 3, [x["v"] for x in r]))

        print("\n--- App3: GraphKer CVE Knowledge Graph ---")
        t.test("CVE count", "MATCH (c:CVE) RETURN count(c) AS v",
               lambda r: (sv(r) >= 15, sv(r)))
        t.test("Product count (MERGE)", "MATCH (p:Product) RETURN count(p) AS v",
               lambda r: (sv(r) >= 20, sv(r)))
        t.test("HAS_WEAKNESS rels", "MATCH ()-[r:HAS_WEAKNESS]->() RETURN count(r) AS v",
               lambda r: (sv(r) >= 10, sv(r)))
        t.test("AFFECTS rels", "MATCH ()-[r:AFFECTS]->() RETURN count(r) AS v",
               lambda r: (sv(r) >= 20, sv(r)))
        t.test("Log4Shell->Products",
               "MATCH (c:CVE {id:'CVE-2021-44228'})-[:AFFECTS]->(p:Product) "
               "RETURN p.vendor, p.product",
               lambda r: (len(r) >= 2, [(x["p.vendor"], x["p.product"]) for x in r]))
        t.test("Top CVEs by CVSS",
               "MATCH (c:CVE) RETURN c.id AS v, c.cvss ORDER BY c.cvss DESC LIMIT 3",
               lambda r: (len(r) == 3, [(x["v"], x["c.cvss"]) for x in r]))
        t.test("CVE aggregation",
               "MATCH (c:CVE) RETURN avg(c.cvss) AS a, max(c.cvss) AS mx, count(c) AS n",
               lambda r: (r[0]["n"] >= 15,
                          f"avg={round(r[0]['a'], 1)} max={r[0]['mx']} n={r[0]['n']}"))

        print("\n--- App4: Network Security Analyzer ---")
        t.test("Host count", "MATCH (h:Host) RETURN count(h) AS v",
               lambda r: (sv(r) == 20, sv(r)))
        t.test("Subnet count", "MATCH (s:Subnet) RETURN count(s) AS v",
               lambda r: (sv(r) == 5, sv(r)))
        t.test("Service count", "MATCH (s:Service) RETURN count(s) AS v",
               lambda r: (sv(r) >= 10, sv(r)))
        t.test("Vulnerability count", "MATCH (v:Vulnerability) RETURN count(v) AS v",
               lambda r: (sv(r) >= 10, sv(r)))
        t.test("Host-Subnet rels", "MATCH (:Host)-[:IN_SUBNET]->(:Subnet) RETURN count(*) AS v",
               lambda r: (sv(r) == 20, sv(r)))
        t.test("DMZ hosts",
               "MATCH (h:Host)-[:IN_SUBNET]->(s:Subnet {name:'DMZ'}) RETURN h.name AS v",
               lambda r: (len(r) >= 3, [x["v"] for x in r]))

    driver.close()

    # Summary
    total = t.passed + t.failed
    print(f"\n{'=' * 60}")
    if t.failed == 0:
        print(f"  ALL {t.passed} TESTS PASSED")
        print(f"{'=' * 60}")
        print("\n  App1 (Neodash):     Dashboard queries, aggregation, traversal")
        print("  App2 (neovis.js):   Kill chains, threat networks, custom Cypher")
        print("  App3 (GraphKer):    CVE/CWE/Product graph, MERGE, NVD data")
        print("  App4 (Network Sec): 20 hosts, 5 subnets, services, vulnerabilities")
        return 0
    else:
        print(f"  {t.passed} PASSED, {t.failed} FAILED out of {total}")
        print(f"{'=' * 60}")
        for f in t.failures:
            print(f"  FAIL: {f}")
        return 1


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="Neo4j Compatibility Gallery Test Suite")
    parser.add_argument("--uri", default="bolt://localhost:7687", help="Bolt URI")
    parser.add_argument("--start-server", action="store_true",
                        help="Start the Bolt server before testing (requires built binary)")
    parser.add_argument("--binary", default=None,
                        help="Path to sedandelivery-bolt binary (default: target/release/)")
    args = parser.parse_args()

    if args.start_server:
        pid = start_bolt_server(args.binary)
        print(f"Started Bolt server (PID {pid})")

    try:
        code = run_tests(args.uri)
    finally:
        stop_bolt_server()

    sys.exit(code)


if __name__ == "__main__":
    main()
