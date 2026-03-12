#!/usr/bin/env python3
"""Multi-instance correctness test for Bolt scaling.

Validates that 2+ sedandelivery-bolt instances sharing a single Redis/Valkey
backend can handle concurrent writes without data corruption.

Tests:
  1. Concurrent node creation from both instances
  2. Concurrent edge creation from both instances
  3. Cross-instance read consistency
  4. No orphaned edges or corrupted adjacency

Usage:
    # With both Bolt instances already running (via docker-compose.scale-test.yml):
    python3 tests/test_multi_instance.py

    # Custom URIs:
    python3 tests/test_multi_instance.py \
        --bolt1 bolt://localhost:7687 \
        --bolt2 bolt://localhost:7689

    # Custom node count per instance:
    python3 tests/test_multi_instance.py --nodes-per-instance 200

Exit code 0 = all tests pass, 1 = failures.
"""

import argparse
import os
import sys
import time
from concurrent.futures import ThreadPoolExecutor, as_completed

try:
    from neo4j import GraphDatabase
except ImportError:
    print("ERROR: neo4j driver is required. Install with: pip install neo4j", file=sys.stderr)
    sys.exit(2)


# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------

DEFAULT_BOLT1_URI = "bolt://localhost:7687"
DEFAULT_BOLT2_URI = "bolt://localhost:7689"
DEFAULT_NODES_PER_INSTANCE = 100
GRAPH_NAME = "__multi_instance_test"


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

class BoltClient:
    """Thin wrapper around the Neo4j driver for test operations."""

    def __init__(self, uri: str, password: str, name: str):
        self.uri = uri
        self.name = name
        self.driver = GraphDatabase.driver(uri, auth=("neo4j", password))

    def close(self):
        self.driver.close()

    def run(self, query: str):
        """Run a single Cypher query and return list of record dicts."""
        with self.driver.session() as session:
            result = session.run(query)
            return [dict(record) for record in result]

    def run_void(self, query: str):
        """Run a query that returns no results."""
        with self.driver.session() as session:
            session.run(query).consume()


def wait_for_bolt(uri: str, password: str, timeout: int = 60) -> bool:
    """Wait for a Bolt endpoint to accept connections."""
    deadline = time.time() + timeout
    while time.time() < deadline:
        try:
            driver = GraphDatabase.driver(uri, auth=("neo4j", password))
            with driver.session() as session:
                session.run("RETURN 1").consume()
            driver.close()
            return True
        except Exception:
            time.sleep(1)
    return False


# ---------------------------------------------------------------------------
# Test functions
# ---------------------------------------------------------------------------

class MultiInstanceTest:
    """Test suite for multi-instance Bolt correctness."""

    def __init__(self, bolt1: BoltClient, bolt2: BoltClient, nodes_per_instance: int):
        self.bolt1 = bolt1
        self.bolt2 = bolt2
        self.nodes_per_instance = nodes_per_instance
        self.passed = 0
        self.failed = 0
        self.errors: list[str] = []

    def _report(self, name: str, ok: bool, detail: str = ""):
        if ok:
            self.passed += 1
            print(f"  [PASS] {name}")
        else:
            self.failed += 1
            msg = f"{name}: {detail}" if detail else name
            self.errors.append(msg)
            print(f"  [FAIL] {name}" + (f" — {detail}" if detail else ""))

    def setup(self):
        """Create the test graph from instance 1."""
        print("\n--- Setup ---")
        try:
            self.bolt1.run_void(f"DROP GRAPH {GRAPH_NAME}")
        except Exception:
            pass  # Graph may not exist yet
        self.bolt1.run_void(f"CREATE GRAPH {GRAPH_NAME}")
        print(f"  Created graph: {GRAPH_NAME}")

    def teardown(self):
        """Drop the test graph."""
        print("\n--- Teardown ---")
        try:
            self.bolt1.run_void(f"DROP GRAPH {GRAPH_NAME}")
            print(f"  Dropped graph: {GRAPH_NAME}")
        except Exception as e:
            print(f"  Warning: could not drop graph: {e}")

    def test_concurrent_node_creation(self):
        """Create nodes concurrently from both instances, verify total count."""
        print("\n--- Test: Concurrent Node Creation ---")
        n = self.nodes_per_instance

        def create_batch(client: BoltClient, prefix: str, count: int):
            """Create `count` nodes with labels and properties identifying the source."""
            for i in range(count):
                client.run_void(
                    f"USE {GRAPH_NAME} CREATE (:TestNode:{prefix} "
                    f"{{source: '{prefix}', seq: {i}}})"
                )

        with ThreadPoolExecutor(max_workers=2) as pool:
            f1 = pool.submit(create_batch, self.bolt1, "Bolt1", n)
            f2 = pool.submit(create_batch, self.bolt2, "Bolt2", n)
            for f in as_completed([f1, f2]):
                f.result()  # Raise any exception from the workers

        # Verify total count from both instances
        expected = n * 2
        for client in [self.bolt1, self.bolt2]:
            result = client.run(
                f"USE {GRAPH_NAME} MATCH (n:TestNode) RETURN count(n) AS cnt"
            )
            actual = result[0]["cnt"]
            self._report(
                f"Node count from {client.name} = {expected}",
                actual == expected,
                f"expected {expected}, got {actual}",
            )

        # Verify per-source counts
        for client in [self.bolt1, self.bolt2]:
            for source in ["Bolt1", "Bolt2"]:
                result = client.run(
                    f"USE {GRAPH_NAME} MATCH (n:{source}) RETURN count(n) AS cnt"
                )
                actual = result[0]["cnt"]
                self._report(
                    f"{source} nodes from {client.name} = {n}",
                    actual == n,
                    f"expected {n}, got {actual}",
                )

    def test_concurrent_edge_creation(self):
        """Create edges concurrently from both instances, verify correctness."""
        print("\n--- Test: Concurrent Edge Creation ---")
        n = self.nodes_per_instance

        def create_edges(client: BoltClient, source_label: str, target_label: str, rel_type: str):
            """Create edges between nodes created by one instance pointing to the other."""
            for i in range(n):
                client.run_void(
                    f"USE {GRAPH_NAME} "
                    f"MATCH (a:{source_label} {{seq: {i}}}), (b:{target_label} {{seq: {i}}}) "
                    f"CREATE (a)-[:{rel_type} {{seq: {i}}}]->(b)"
                )

        with ThreadPoolExecutor(max_workers=2) as pool:
            f1 = pool.submit(create_edges, self.bolt1, "Bolt1", "Bolt2", "CONNECTS_12")
            f2 = pool.submit(create_edges, self.bolt2, "Bolt2", "Bolt1", "CONNECTS_21")
            for f in as_completed([f1, f2]):
                f.result()

        # Verify edge counts
        for rel_type in ["CONNECTS_12", "CONNECTS_21"]:
            for client in [self.bolt1, self.bolt2]:
                result = client.run(
                    f"USE {GRAPH_NAME} MATCH ()-[r:{rel_type}]->() RETURN count(r) AS cnt"
                )
                actual = result[0]["cnt"]
                self._report(
                    f"{rel_type} edges from {client.name} = {n}",
                    actual == n,
                    f"expected {n}, got {actual}",
                )

    def test_no_orphaned_edges(self):
        """Verify every edge connects two existing nodes (no dangling references)."""
        print("\n--- Test: No Orphaned Edges ---")

        # Check that every relationship has valid start and end nodes
        for client in [self.bolt1, self.bolt2]:
            # Total relationships
            result = client.run(
                f"USE {GRAPH_NAME} MATCH (a)-[r]->(b) RETURN count(r) AS cnt"
            )
            total_edges = result[0]["cnt"]

            # All edges should connect TestNode to TestNode
            result = client.run(
                f"USE {GRAPH_NAME} MATCH (a:TestNode)-[r]->(b:TestNode) RETURN count(r) AS cnt"
            )
            valid_edges = result[0]["cnt"]

            self._report(
                f"No orphaned edges from {client.name}",
                total_edges == valid_edges,
                f"total={total_edges}, valid={valid_edges}",
            )

    def test_cross_instance_read_consistency(self):
        """Write from one instance, read from the other."""
        print("\n--- Test: Cross-Instance Read Consistency ---")

        # Create a unique node from bolt1
        marker = f"cross_check_{int(time.time())}"
        self.bolt1.run_void(
            f"USE {GRAPH_NAME} CREATE (:Marker {{id: '{marker}'}})"
        )

        # Read it from bolt2
        result = self.bolt2.run(
            f"USE {GRAPH_NAME} MATCH (m:Marker {{id: '{marker}'}}) RETURN m.id AS id"
        )
        found = len(result) == 1 and result[0]["id"] == marker
        self._report(
            "Cross-instance read (bolt1 -> bolt2)",
            found,
            f"expected marker '{marker}', got {result}",
        )

        # Create from bolt2, read from bolt1
        marker2 = f"cross_check_reverse_{int(time.time())}"
        self.bolt2.run_void(
            f"USE {GRAPH_NAME} CREATE (:Marker {{id: '{marker2}'}})"
        )
        result = self.bolt1.run(
            f"USE {GRAPH_NAME} MATCH (m:Marker {{id: '{marker2}'}}) RETURN m.id AS id"
        )
        found = len(result) == 1 and result[0]["id"] == marker2
        self._report(
            "Cross-instance read (bolt2 -> bolt1)",
            found,
            f"expected marker '{marker2}', got {result}",
        )

    def test_adjacency_integrity(self):
        """Verify adjacency lists are not corrupted by concurrent writes."""
        print("\n--- Test: Adjacency Integrity ---")

        for client in [self.bolt1, self.bolt2]:
            # Every Bolt1 node should have exactly 1 outgoing CONNECTS_12 edge
            result = client.run(
                f"USE {GRAPH_NAME} "
                f"MATCH (a:Bolt1) "
                f"OPTIONAL MATCH (a)-[r:CONNECTS_12]->() "
                f"WITH a, count(r) AS deg "
                f"WHERE deg <> 1 "
                f"RETURN count(a) AS bad"
            )
            bad = result[0]["bad"]
            self._report(
                f"Bolt1 CONNECTS_12 out-degree=1 from {client.name}",
                bad == 0,
                f"{bad} nodes with wrong out-degree",
            )

            # Every Bolt2 node should have exactly 1 outgoing CONNECTS_21 edge
            result = client.run(
                f"USE {GRAPH_NAME} "
                f"MATCH (a:Bolt2) "
                f"OPTIONAL MATCH (a)-[r:CONNECTS_21]->() "
                f"WITH a, count(r) AS deg "
                f"WHERE deg <> 1 "
                f"RETURN count(a) AS bad"
            )
            bad = result[0]["bad"]
            self._report(
                f"Bolt2 CONNECTS_21 out-degree=1 from {client.name}",
                bad == 0,
                f"{bad} nodes with wrong out-degree",
            )

    def run_all(self) -> bool:
        """Run all tests and return True if all passed."""
        self.setup()
        try:
            self.test_concurrent_node_creation()
            self.test_concurrent_edge_creation()
            self.test_no_orphaned_edges()
            self.test_cross_instance_read_consistency()
            self.test_adjacency_integrity()
        finally:
            self.teardown()

        print("\n" + "=" * 60)
        print(f"  Passed: {self.passed}")
        print(f"  Failed: {self.failed}")
        if self.errors:
            print("  Errors:")
            for e in self.errors:
                print(f"    - {e}")
        print("=" * 60)
        return self.failed == 0


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="Multi-instance Bolt correctness test")
    parser.add_argument("--bolt1", default=DEFAULT_BOLT1_URI, help="URI for Bolt instance 1")
    parser.add_argument("--bolt2", default=DEFAULT_BOLT2_URI, help="URI for Bolt instance 2")
    parser.add_argument(
        "--nodes-per-instance",
        type=int,
        default=DEFAULT_NODES_PER_INSTANCE,
        help="Number of nodes each instance creates (default: 100)",
    )
    parser.add_argument(
        "--password",
        default=os.environ.get("BOLT_PASSWORD", os.environ.get("VALKEY_PASSWORD", "")),
        help="Bolt auth password (defaults to BOLT_PASSWORD or VALKEY_PASSWORD env var)",
    )
    parser.add_argument(
        "--wait-timeout",
        type=int,
        default=60,
        help="Seconds to wait for each Bolt endpoint to be ready (default: 60)",
    )
    args = parser.parse_args()

    if not args.password:
        print("ERROR: No password provided. Set BOLT_PASSWORD or VALKEY_PASSWORD, or use --password.", file=sys.stderr)
        sys.exit(2)

    print("=" * 60)
    print("  Multi-Instance Bolt Correctness Test")
    print("=" * 60)
    print(f"  Bolt 1: {args.bolt1}")
    print(f"  Bolt 2: {args.bolt2}")
    print(f"  Nodes per instance: {args.nodes_per_instance}")
    print()

    # Wait for both instances to be ready
    print("Waiting for Bolt instances...")
    for name, uri in [("bolt-1", args.bolt1), ("bolt-2", args.bolt2)]:
        if not wait_for_bolt(uri, args.password, timeout=args.wait_timeout):
            print(f"ERROR: {name} ({uri}) did not become ready within {args.wait_timeout}s", file=sys.stderr)
            sys.exit(2)
        print(f"  {name} is ready")

    bolt1 = BoltClient(args.bolt1, args.password, "bolt-1")
    bolt2 = BoltClient(args.bolt2, args.password, "bolt-2")

    try:
        suite = MultiInstanceTest(bolt1, bolt2, args.nodes_per_instance)
        success = suite.run_all()
    finally:
        bolt1.close()
        bolt2.close()

    sys.exit(0 if success else 1)


if __name__ == "__main__":
    main()
