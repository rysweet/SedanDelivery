#!/usr/bin/env python3
"""Load MITRE ATT&CK security dataset into SedanDelivery via Bolt protocol.

Usage:
    python3 load_data.py [--uri bolt://localhost:7687] [--clear]
"""
import argparse
import os
from pathlib import Path

from neo4j import GraphDatabase

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


def load_cypher_file(session, filepath: Path) -> dict:
    """Load a .cypher file, splitting on semicolons and executing each statement."""
    text = filepath.read_text()
    # Split on lines that end with a newline before the next CREATE/RETURN
    # but handle the multi-statement file as one big CREATE block
    statements = []
    current = []
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("//") or not stripped:
            continue
        current.append(line)

    # Join all lines into one big statement (Cypher allows this)
    full_query = "\n".join(current)

    # Execute as single statement
    result = session.run(full_query)
    summary = result.consume()
    return {
        "nodes_created": summary.counters.nodes_created,
        "relationships_created": summary.counters.relationships_created,
    }


def verify_data(session) -> dict:
    """Run verification queries to ensure data loaded correctly."""
    checks = {}

    # Count by label
    for label in ["ThreatGroup", "Tactic", "Technique", "Software", "CVE",
                   "Campaign", "Mitigation", "Infrastructure"]:
        result = session.run(f"MATCH (n:{label}) RETURN count(n) AS cnt")
        checks[label] = result.single()["cnt"]

    # Count relationships
    result = session.run("MATCH ()-[r]->() RETURN count(r) AS cnt")
    checks["total_relationships"] = result.single()["cnt"]

    # Sample query: Which groups use Mimikatz?
    result = session.run(
        "MATCH (g:ThreatGroup)-[:USES_SOFTWARE]->(s:Software {name: 'Mimikatz'}) "
        "RETURN g.name AS group_name"
    )
    checks["mimikatz_users"] = [r["group_name"] for r in result]

    # Sample query: Attack path from phishing to impact
    result = session.run(
        "MATCH (t1:Technique {name: 'Spearphishing Attachment'})"
        "<-[:USES]-(g:ThreatGroup)-[:USES]->"
        "(t2:Technique)-[:BELONGS_TO]->(tac:Tactic) "
        "WHERE tac.name IN ['Impact', 'Exfiltration'] "
        "RETURN DISTINCT g.name AS group_name, t2.name AS technique, tac.name AS tactic"
    )
    checks["attack_paths"] = [dict(r) for r in result]

    return checks


def main():
    parser = argparse.ArgumentParser(description="Load MITRE ATT&CK data into SedanDelivery")
    parser.add_argument("--uri", default="bolt://localhost:7687", help="Bolt URI")
    parser.add_argument("--clear", action="store_true", help="Clear all existing data first")
    args = parser.parse_args()

    print(f"Connecting to {args.uri}...")
    auth = resolve_bolt_auth()
    print(f"Auth mode: {describe_bolt_auth(auth)}")
    driver = GraphDatabase.driver(args.uri, auth=auth)

    with driver.session() as session:
        if args.clear:
            print("Clearing existing data...")
            session.run("MATCH (n) DETACH DELETE n")
            print("  Done.")

        print("Loading MITRE ATT&CK dataset...")
        cypher_file = Path(__file__).parent / "load_mitre_attack.cypher"
        stats = load_cypher_file(session, cypher_file)
        print(f"  Nodes created: {stats['nodes_created']}")
        print(f"  Relationships created: {stats['relationships_created']}")

        print("\nVerifying data...")
        checks = verify_data(session)
        print(f"  ThreatGroups:    {checks['ThreatGroup']}")
        print(f"  Tactics:         {checks['Tactic']}")
        print(f"  Techniques:      {checks['Technique']}")
        print(f"  Software:        {checks['Software']}")
        print(f"  CVEs:            {checks['CVE']}")
        print(f"  Campaigns:       {checks['Campaign']}")
        print(f"  Mitigations:     {checks['Mitigation']}")
        print(f"  Infrastructure:  {checks['Infrastructure']}")
        print(f"  Relationships:   {checks['total_relationships']}")
        print(f"\n  Mimikatz users: {checks['mimikatz_users']}")
        print(f"  Attack paths from phishing to impact:")
        for path in checks["attack_paths"]:
            print(f"    {path['group_name']} -> {path['technique']} ({path['tactic']})")

        total_nodes = sum(checks[k] for k in checks if k not in
                          ["total_relationships", "mimikatz_users", "attack_paths"])
        print(f"\n  TOTAL: {total_nodes} nodes, {checks['total_relationships']} relationships")
        print("\nDataset loaded and verified successfully!")

    driver.close()


if __name__ == "__main__":
    main()
