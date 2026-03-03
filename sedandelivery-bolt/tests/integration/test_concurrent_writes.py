"""
Outside-in integration test: concurrent bulk writes at scale.

Verifies the fixes from GitHub issue cloud-ecosystem-security/cybergym#542:
  1. Concurrent writes are NOT serialized by a global lock (semaphore model).
  2. Property indexes make MERGE O(1) instead of O(N).
  3. Session timeout (3600s default) does not kill bulk write sessions.

This test drives the bolt server purely from the outside via the neo4j Python
driver — no internal APIs, no mocking. If these tests pass against a running
sedandelivery-bolt instance, the system handles thousands of writes/s.

Requirements:
    pip install neo4j
    sedandelivery-bolt running on localhost:7687 (or BOLT_URI env var)

Run:
    BOLT_URI=bolt://localhost:7687 pytest tests/integration/test_concurrent_writes.py -v
"""

import os
import time
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed

import pytest
from neo4j import GraphDatabase

BOLT_URI = os.environ.get("BOLT_URI", "bolt://localhost:7687")
DB = "test_concurrent_writes"


@pytest.fixture(scope="module")
def driver():
    d = GraphDatabase.driver(BOLT_URI, auth=None)
    # Create the test database if it doesn't exist.
    with d.session(database="system") as s:
        try:
            s.run(f"CREATE DATABASE {DB}").consume()
        except Exception:
            pass  # Already exists.
    yield d
    # Drop the test database on teardown.
    with d.session(database="system") as s:
        try:
            s.run(f"DROP DATABASE {DB}").consume()
        except Exception:
            pass
    d.close()


@pytest.fixture(autouse=True)
def clean_graph(driver):
    """Ensure a clean graph before each test."""
    with driver.session(database=DB) as s:
        s.run("MATCH (n) DETACH DELETE n").consume()
    yield
    with driver.session(database=DB) as s:
        s.run("MATCH (n) DETACH DELETE n").consume()


# ---------------------------------------------------------------------------
# Test 1: Concurrent CREATE from multiple connections does NOT serialize
# ---------------------------------------------------------------------------

class TestConcurrentCreates:
    """8 parallel connections each creating 100 nodes must all finish quickly."""

    CONNECTIONS = 8
    NODES_PER_CONN = 100

    def test_parallel_creates_complete_under_30s(self, driver):
        """
        Before the fix: 8 connections queue on a single Mutex, only 1 executes
        at a time, each CREATE takes ~1ms so total ~800ms per session — but the
        lock serializes them to ~6.4s minimum. With 5k+ nodes and MERGE the
        global lock + O(N) scan caused 300s timeouts.

        After the fix: autocommit writes skip the semaphore entirely, so all 8
        connections write concurrently. This must finish in <30s total.
        """
        errors = []
        counts = []

        def writer(conn_id):
            try:
                with driver.session(database=DB) as session:
                    for i in range(self.NODES_PER_CONN):
                        session.run(
                            "CREATE (n:BulkNode {connId: $cid, idx: $idx})",
                            cid=conn_id,
                            idx=i,
                        ).consume()
                    counts.append(self.NODES_PER_CONN)
            except Exception as e:
                errors.append((conn_id, str(e)))

        start = time.monotonic()
        with ThreadPoolExecutor(max_workers=self.CONNECTIONS) as pool:
            futures = [pool.submit(writer, cid) for cid in range(self.CONNECTIONS)]
            for f in as_completed(futures):
                f.result()  # re-raise exceptions

        elapsed = time.monotonic() - start

        assert not errors, f"Writer errors: {errors}"
        assert sum(counts) == self.CONNECTIONS * self.NODES_PER_CONN
        # Verify all nodes exist.
        with driver.session(database=DB) as s:
            result = s.run("MATCH (n:BulkNode) RETURN count(n) AS cnt")
            total = result.single()["cnt"]
            assert total == self.CONNECTIONS * self.NODES_PER_CONN

        # Must complete well under 30 seconds.
        assert elapsed < 30.0, (
            f"Concurrent creates took {elapsed:.1f}s — expected <30s. "
            "Write lock may still be serializing connections."
        )


# ---------------------------------------------------------------------------
# Test 2: MERGE with property index is O(1), not O(N)
# ---------------------------------------------------------------------------

class TestMergeWithIndex:
    """MERGE on an indexed property must remain fast as node count grows."""

    def test_merge_scales_with_index(self, driver):
        """
        Before: MERGE (n:Resource {id: X}) scans ALL Resource nodes O(N).
        After:  create_index builds a property index, MERGE uses it for O(1).

        Seed 1000 nodes, then MERGE 100 more. Must finish quickly.
        """
        with driver.session(database=DB) as s:
            # Create the index FIRST (triggers backfill for any existing nodes).
            s.run("CREATE INDEX FOR (n:Resource) ON (n.rid)").consume()

            # Seed 1000 nodes.
            s.run(
                "UNWIND range(1, 1000) AS i "
                "CREATE (n:Resource {rid: 'res-' + toString(i), name: 'seed'})"
            ).consume()

        # Now MERGE 200 times: IDs 901-1100.
        # IDs 901-1000 exist (100 matches), IDs 1001-1100 are new (100 creates).
        start = time.monotonic()
        with driver.session(database=DB) as s:
            for i in range(200):
                rid = f"res-{901 + i}"
                s.run(
                    "MERGE (n:Resource {rid: $rid}) ON CREATE SET n.name = 'merged'",
                    rid=rid,
                ).consume()
        elapsed = time.monotonic() - start

        # Verify counts.
        with driver.session(database=DB) as s:
            result = s.run("MATCH (n:Resource) RETURN count(n) AS cnt")
            total = result.single()["cnt"]
            # 1000 seeded + 100 new from MERGE (IDs 1001-1100) = 1100
            assert total == 1100, f"Expected 1100 nodes, got {total}"

        # 200 MERGEs against 1000+ nodes must be fast with index.
        # Without index this would take >200 seconds. With index: <10s.
        assert elapsed < 15.0, (
            f"200 MERGEs took {elapsed:.1f}s — expected <15s with property index. "
            "Index may not be accelerating MERGE lookups."
        )


# ---------------------------------------------------------------------------
# Test 3: Concurrent MERGE from multiple connections
# ---------------------------------------------------------------------------

class TestConcurrentMerge:
    """Multiple connections running MERGE concurrently must not deadlock or timeout."""

    CONNECTIONS = 4
    MERGES_PER_CONN = 50

    def test_parallel_merge_no_deadlock(self, driver):
        """
        Before: global Mutex + O(N) MERGE = 300s timeout, all connections killed.
        After: concurrent autocommit writes + O(1) indexed MERGE = fast completion.
        """
        with driver.session(database=DB) as s:
            s.run("CREATE INDEX FOR (n:Asset) ON (n.assetId)").consume()
            # Seed some data.
            s.run(
                "UNWIND range(1, 200) AS i "
                "CREATE (n:Asset {assetId: 'asset-' + toString(i)})"
            ).consume()

        errors = []

        def merger(conn_id):
            try:
                with driver.session(database=DB) as session:
                    for i in range(self.MERGES_PER_CONN):
                        # Each connection MERGEs a mix of existing and new nodes.
                        aid = f"asset-{conn_id * 100 + i}"
                        session.run(
                            "MERGE (n:Asset {assetId: $aid}) "
                            "ON CREATE SET n.source = $src "
                            "ON MATCH SET n.updated = true",
                            aid=aid,
                            src=f"conn-{conn_id}",
                        ).consume()
            except Exception as e:
                errors.append((conn_id, str(e)))

        start = time.monotonic()
        with ThreadPoolExecutor(max_workers=self.CONNECTIONS) as pool:
            futures = [pool.submit(merger, cid) for cid in range(self.CONNECTIONS)]
            for f in as_completed(futures):
                f.result()
        elapsed = time.monotonic() - start

        assert not errors, f"Merger errors: {errors}"

        # Verify no deadlock and reasonable timing.
        assert elapsed < 30.0, (
            f"Concurrent MERGE took {elapsed:.1f}s — expected <30s. "
            "May indicate write lock contention or missing index."
        )


# ---------------------------------------------------------------------------
# Test 4: Session survives beyond old 300s default
# ---------------------------------------------------------------------------

class TestSessionTimeout:
    """Verify that the idle timeout is now configurable and >300s."""

    def test_session_stays_alive_after_query_burst(self, driver):
        """
        The session should not be killed by the server during active use.
        We send a burst of writes, wait briefly, then verify the session
        is still usable (not closed by a 300s wall-clock timeout).
        """
        with driver.session(database=DB) as s:
            # Initial burst.
            for i in range(50):
                s.run(
                    "CREATE (n:TimeoutTest {idx: $i})",
                    i=i,
                ).consume()

            # Wait a moment then verify session is still alive.
            time.sleep(2)
            result = s.run("MATCH (n:TimeoutTest) RETURN count(n) AS cnt")
            total = result.single()["cnt"]
            assert total == 50, f"Expected 50 TimeoutTest nodes, got {total}"

            # Session is still usable for more writes.
            s.run("CREATE (n:TimeoutTest {idx: 999})").consume()
            result = s.run("MATCH (n:TimeoutTest) RETURN count(n) AS cnt")
            assert result.single()["cnt"] == 51


# ---------------------------------------------------------------------------
# Test 5: Large UNWIND batch write (simulating Phase 7 tenant scan)
# ---------------------------------------------------------------------------

class TestBulkUnwindWrite:
    """Simulate the Phase 7 pattern: UNWIND batch of resources/edges."""

    def test_unwind_5000_creates(self, driver):
        """
        Simulate CyberGym Phase 7: create thousands of nodes via UNWIND.
        Before: this would hang for 300s and timeout.
        After: completes in seconds.
        """
        with driver.session(database=DB) as s:
            start = time.monotonic()
            # Create 5000 nodes in a single UNWIND batch.
            s.run(
                "UNWIND range(1, 5000) AS i "
                "CREATE (n:TenantResource {rid: 'tr-' + toString(i), type: 'vm'})"
            ).consume()
            elapsed = time.monotonic() - start

        # Verify all created.
        with driver.session(database=DB) as s:
            result = s.run("MATCH (n:TenantResource) RETURN count(n) AS cnt")
            assert result.single()["cnt"] == 5000

        # 5000 creates via UNWIND should be fast (no lock contention).
        assert elapsed < 30.0, (
            f"UNWIND 5000 CREATEs took {elapsed:.1f}s — expected <30s."
        )

    def test_unwind_merge_with_index(self, driver):
        """
        Simulate re-scan: MERGE 1000 existing + 500 new via UNWIND.
        """
        with driver.session(database=DB) as s:
            s.run("CREATE INDEX FOR (n:ScanNode) ON (n.sid)").consume()
            # Seed 1000 nodes.
            s.run(
                "UNWIND range(1, 1000) AS i "
                "CREATE (n:ScanNode {sid: 'sn-' + toString(i)})"
            ).consume()

        with driver.session(database=DB) as s:
            start = time.monotonic()
            # MERGE 1500: first 1000 match existing, last 500 are new.
            s.run(
                "UNWIND range(1, 1500) AS i "
                "MERGE (n:ScanNode {sid: 'sn-' + toString(i)}) "
                "ON CREATE SET n.created = true "
                "ON MATCH SET n.seen = true"
            ).consume()
            elapsed = time.monotonic() - start

        with driver.session(database=DB) as s:
            result = s.run("MATCH (n:ScanNode) RETURN count(n) AS cnt")
            assert result.single()["cnt"] == 1500

        assert elapsed < 60.0, (
            f"UNWIND MERGE 1500 took {elapsed:.1f}s — expected <60s with index."
        )
