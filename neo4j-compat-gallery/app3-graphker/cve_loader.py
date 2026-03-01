#!/usr/bin/env python3
"""
cve_loader.py - Fetch CVE data from NVD and load into SedanDelivery.

Fetches recent critical CVEs from the NIST NVD API v2, creates a knowledge
graph of CVEs, CWEs, and affected Products, then loads everything through
the neo4j Python driver (Bolt protocol) into SedanDelivery.

Usage:
    python cve_loader.py             # Fetch from NVD API
    python cve_loader.py --sample    # Use hardcoded sample data (offline)
"""

import argparse
import sys
import time

import requests
from neo4j import GraphDatabase

from sample_cves import get_sample_cypher_statements

# ── Configuration ────────────────────────────────────────────────────────────

BOLT_URI = "bolt://localhost:7687"
NVD_API_URL = "https://services.nvd.nist.gov/rest/json/cves/2.0"
NVD_PARAMS = {
    "resultsPerPage": 20,
    "cvssV3Severity": "CRITICAL",
}
REQUEST_TIMEOUT = 30  # seconds


# ── NVD API helpers ──────────────────────────────────────────────────────────

def fetch_cves_from_nvd():
    """Fetch recent critical CVEs from the NVD API v2.

    Returns a list of CVE items (dicts) or raises on failure.
    """
    print(f"[*] Fetching CVEs from NVD API: {NVD_API_URL}")
    print(f"    Parameters: {NVD_PARAMS}")

    resp = requests.get(NVD_API_URL, params=NVD_PARAMS, timeout=REQUEST_TIMEOUT)
    resp.raise_for_status()

    data = resp.json()
    vulnerabilities = data.get("vulnerabilities", [])
    print(f"[+] Received {len(vulnerabilities)} CVEs from NVD")
    return vulnerabilities


def extract_description(cve_data):
    """Pull English description from CVE data."""
    for desc in cve_data.get("descriptions", []):
        if desc.get("lang") == "en":
            return desc["value"].replace("'", "\\'").replace('"', '\\"')
    return "No description available"


def extract_cvss(cve_data):
    """Extract CVSS v3.1 base score and severity."""
    metrics = cve_data.get("metrics", {})

    # Try cvssMetricV31, then cvssMetricV30
    for key in ("cvssMetricV31", "cvssMetricV30"):
        metric_list = metrics.get(key, [])
        if metric_list:
            cvss_data = metric_list[0].get("cvssData", {})
            return (
                cvss_data.get("baseScore", 0.0),
                cvss_data.get("baseSeverity", "UNKNOWN"),
            )
    return (0.0, "UNKNOWN")


def extract_cwes(cve_data):
    """Extract CWE IDs from weakness enumeration."""
    cwes = []
    for weakness in cve_data.get("weaknesses", []):
        for desc in weakness.get("description", []):
            val = desc.get("value", "")
            if val.startswith("CWE-"):
                cwes.append(val)
    return cwes


def extract_products(cve_data):
    """Extract affected products from CPE configurations."""
    products = []
    seen = set()
    configurations = cve_data.get("configurations", [])

    for config in configurations:
        for node in config.get("nodes", []):
            for cpe_match in node.get("cpeMatch", []):
                criteria = cpe_match.get("criteria", "")
                # CPE 2.3 format: cpe:2.3:a:vendor:product:version:...
                parts = criteria.split(":")
                if len(parts) >= 6:
                    vendor = parts[3]
                    product = parts[4]
                    version = parts[5] if parts[5] != "*" else "all"
                    key = (vendor, product, version)
                    if key not in seen:
                        seen.add(key)
                        products.append(key)

    return products


def build_cypher_from_nvd(vulnerabilities):
    """Convert NVD API response into Cypher CREATE statements."""
    statements = []
    all_cwes = {}  # id -> name
    all_products = set()

    cve_records = []

    for vuln in vulnerabilities:
        cve_data = vuln.get("cve", {})
        cve_id = cve_data.get("id", "UNKNOWN")
        description = extract_description(cve_data)
        cvss, severity = extract_cvss(cve_data)
        published = cve_data.get("published", "")[:10]  # date portion
        cwes = extract_cwes(cve_data)
        products = extract_products(cve_data)

        cve_records.append({
            "id": cve_id,
            "description": description,
            "cvss": cvss,
            "severity": severity,
            "published": published,
            "cwes": cwes,
            "products": products,
        })

        for cwe_id in cwes:
            if cwe_id not in all_cwes:
                all_cwes[cwe_id] = cwe_id  # Name will just be the ID for NVD data

        for p in products:
            all_products.add(p)

    # 1. Create CWE nodes
    for cwe_id, cwe_name in all_cwes.items():
        statements.append(
            f"CREATE (:CWE {{id: '{cwe_id}', name: '{cwe_name}'}})"
        )

    # 2. Create Product nodes
    for vendor, product, version in all_products:
        safe_vendor = vendor.replace("'", "\\'")
        safe_product = product.replace("'", "\\'")
        statements.append(
            f"MERGE (:Product {{vendor: '{safe_vendor}', product: '{safe_product}', version: '{version}'}})"
        )

    # 3. Create CVE nodes and relationships
    for rec in cve_records:
        cve_id = rec["id"]
        statements.append(
            f"CREATE (:CVE {{id: '{cve_id}', description: '{rec['description']}', "
            f"cvss: {rec['cvss']}, severity: '{rec['severity']}', published: '{rec['published']}'}})"
        )

        for cwe_id in rec["cwes"]:
            statements.append(
                f"MATCH (c:CVE {{id: '{cve_id}'}}), (w:CWE {{id: '{cwe_id}'}}) "
                f"CREATE (c)-[:HAS_WEAKNESS]->(w)"
            )

        for vendor, product, version in rec["products"]:
            safe_vendor = vendor.replace("'", "\\'")
            safe_product = product.replace("'", "\\'")
            statements.append(
                f"MATCH (c:CVE {{id: '{cve_id}'}}), "
                f"(p:Product {{vendor: '{safe_vendor}', product: '{safe_product}', version: '{version}'}}) "
                f"CREATE (c)-[:AFFECTS]->(p)"
            )

    return statements


# ── Database operations ──────────────────────────────────────────────────────

def clear_database(driver):
    """Remove all existing nodes and relationships."""
    print("[*] Clearing existing data...")
    with driver.session() as session:
        session.run("MATCH (n) DETACH DELETE n")
    print("[+] Database cleared")


def load_statements(driver, statements):
    """Execute a list of Cypher statements."""
    success = 0
    errors = 0

    with driver.session() as session:
        for i, stmt in enumerate(statements):
            try:
                session.run(stmt)
                success += 1
            except Exception as e:
                errors += 1
                print(f"[!] Statement {i + 1} failed: {e}")
                print(f"    Statement: {stmt[:120]}...")

    print(f"[+] Loaded {success}/{len(statements)} statements ({errors} errors)")
    return success, errors


def verify_load(driver):
    """Print summary counts after loading."""
    with driver.session() as session:
        cve_count = session.run("MATCH (c:CVE) RETURN count(c) AS cnt").single()["cnt"]
        cwe_count = session.run("MATCH (w:CWE) RETURN count(w) AS cnt").single()["cnt"]

        # Use a label-less match for Product to handle label variations
        prod_count = session.run("MATCH (p:Product) RETURN count(p) AS cnt").single()["cnt"]

        rel_weakness = session.run(
            "MATCH ()-[r:HAS_WEAKNESS]->() RETURN count(r) AS cnt"
        ).single()["cnt"]
        rel_affects = session.run(
            "MATCH ()-[r:AFFECTS]->() RETURN count(r) AS cnt"
        ).single()["cnt"]

    print()
    print("=" * 50)
    print("  CVE Knowledge Graph Summary")
    print("=" * 50)
    print(f"  CVE nodes:            {cve_count}")
    print(f"  CWE nodes:            {cwe_count}")
    print(f"  Product nodes:        {prod_count}")
    print(f"  HAS_WEAKNESS rels:    {rel_weakness}")
    print(f"  AFFECTS rels:         {rel_affects}")
    print("=" * 50)


# ── Main ─────────────────────────────────────────────────────────────────────

def main():
    parser = argparse.ArgumentParser(
        description="Load CVE knowledge graph into SedanDelivery"
    )
    parser.add_argument(
        "--sample",
        action="store_true",
        help="Use hardcoded sample data instead of fetching from NVD API",
    )
    parser.add_argument(
        "--bolt-uri",
        default=BOLT_URI,
        help=f"Bolt URI for SedanDelivery (default: {BOLT_URI})",
    )
    parser.add_argument(
        "--no-clear",
        action="store_true",
        help="Do not clear existing data before loading",
    )
    args = parser.parse_args()

    # Build the Cypher statements
    if args.sample:
        print("[*] Using sample CVE data (offline mode)")
        statements = get_sample_cypher_statements()
    else:
        try:
            vulnerabilities = fetch_cves_from_nvd()
            if not vulnerabilities:
                print("[!] No CVEs returned from NVD. Falling back to sample data.")
                statements = get_sample_cypher_statements()
            else:
                statements = build_cypher_from_nvd(vulnerabilities)
        except Exception as e:
            print(f"[!] NVD API request failed: {e}")
            print("[*] Falling back to sample data...")
            statements = get_sample_cypher_statements()

    print(f"[*] Generated {len(statements)} Cypher statements")

    # Connect and load
    print(f"[*] Connecting to SedanDelivery at {args.bolt_uri}")
    driver = GraphDatabase.driver(args.bolt_uri, auth=None)

    try:
        driver.verify_connectivity()
        print("[+] Connected successfully")

        if not args.no_clear:
            clear_database(driver)

        start = time.time()
        load_statements(driver, statements)
        elapsed = time.time() - start
        print(f"[*] Load completed in {elapsed:.2f}s")

        verify_load(driver)
    except Exception as e:
        print(f"[!] Fatal error: {e}")
        sys.exit(1)
    finally:
        driver.close()

    print("\n[+] CVE knowledge graph loaded. Ready for exploration.")


if __name__ == "__main__":
    main()
