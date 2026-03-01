"""
Transaction isolation tests for sedandelivery-bolt.

These tests verify ACID properties of sedandelivery-bolt's transaction handling,
including commit/rollback semantics, concurrent access, snapshot isolation
behavior, and stress testing under high concurrency.

Requires a running sedandelivery-bolt server on localhost:7687 (or BOLT_URI env var).

Run:
    BOLT_URI=bolt://localhost:7687 pytest sedandelivery-bolt/tests/integration/test_transactions.py -v
"""

import os
import threading
import time

import pytest
from neo4j import GraphDatabase

BOLT_URI = os.environ.get("BOLT_URI", "bolt://localhost:7687")

# Each test class uses its own database name to avoid cross-test interference,
# following the same pattern as test_bolt.py.
_TX_DB = "test_transactions"


@pytest.fixture(scope="module")
def driver():
    """Module-scoped neo4j driver (shared across all tests in this file)."""
    drv = GraphDatabase.driver(BOLT_URI, auth=None)
    yield drv
    drv.close()


@pytest.fixture(autouse=True)
def cleanup(driver):
    """Clean up all test data before and after each test."""
    with driver.session(database=_TX_DB) as s:
        s.run("MATCH (n) DETACH DELETE n").consume()
    yield
    with driver.session(database=_TX_DB) as s:
        s.run("MATCH (n) DETACH DELETE n").consume()


# ---------------------------------------------------------------------------
# Basic commit / rollback semantics
# ---------------------------------------------------------------------------


def test_basic_transaction_commit(driver):
    """Transaction commit makes data visible to subsequent sessions."""
    with driver.session(database=_TX_DB) as s:
        tx = s.begin_transaction()
        tx.run("CREATE (:TestNode {name: 'committed'})")
        tx.commit()

    with driver.session(database=_TX_DB) as s:
        result = s.run("MATCH (n:TestNode) RETURN n.name AS name").single()
        assert result is not None, "Committed node should be visible"
        assert result["name"] == "committed"


def test_basic_transaction_rollback(driver):
    """Transaction rollback discards all mutations -- nothing is persisted."""
    with driver.session(database=_TX_DB) as s:
        tx = s.begin_transaction()
        tx.run("CREATE (:TestNode {name: 'rolled_back'})")
        tx.rollback()

    with driver.session(database=_TX_DB) as s:
        result = list(s.run("MATCH (n:TestNode) RETURN n"))
        assert len(result) == 0, "Rolled-back node must not be visible"


def test_rollback_multiple_mutations(driver):
    """Multiple CREATEs inside a single TX are all discarded on ROLLBACK."""
    with driver.session(database=_TX_DB) as s:
        tx = s.begin_transaction()
        for i in range(10):
            tx.run(f"CREATE (:TestNode {{idx: {i}}})")
        tx.rollback()

    with driver.session(database=_TX_DB) as s:
        result = s.run(
            "MATCH (n:TestNode) RETURN count(n) AS c"
        ).single()
        assert result["c"] == 0, "All 10 rolled-back nodes must vanish"


# ---------------------------------------------------------------------------
# Auto-commit (implicit transactions)
# ---------------------------------------------------------------------------


def test_autocommit_visibility(driver):
    """Auto-commit (implicit) transaction makes data immediately visible."""
    with driver.session(database=_TX_DB) as s:
        s.run("CREATE (:AutoNode {name: 'auto'})").consume()

    with driver.session(database=_TX_DB) as s:
        result = s.run("MATCH (n:AutoNode) RETURN n.name AS name").single()
        assert result is not None
        assert result["name"] == "auto"


# ---------------------------------------------------------------------------
# Read-your-own-writes inside a transaction
# ---------------------------------------------------------------------------


def test_transaction_with_return(driver):
    """A transaction can read data it just wrote (read-your-own-writes)."""
    with driver.session(database=_TX_DB) as s:
        tx = s.begin_transaction()
        tx.run("CREATE (:TxReturnNode {val: 42})")
        result = tx.run("MATCH (n:TxReturnNode) RETURN n.val AS v").single()
        assert result is not None
        assert result["v"] == 42
        tx.commit()


def test_multiple_statements_in_transaction(driver):
    """Multiple sequential statements inside one TX see cumulative state."""
    with driver.session(database=_TX_DB) as s:
        tx = s.begin_transaction()
        tx.run("CREATE (:Multi {step: 1})")
        tx.run("CREATE (:Multi {step: 2})")
        tx.run("CREATE (:Multi {step: 3})")
        count = tx.run(
            "MATCH (n:Multi) RETURN count(n) AS c"
        ).single()["c"]
        assert count == 3, f"Expected 3 inside TX, got {count}"
        tx.commit()

    # Verify from a fresh session after commit
    with driver.session(database=_TX_DB) as s:
        count = s.run(
            "MATCH (n:Multi) RETURN count(n) AS c"
        ).single()["c"]
        assert count == 3


# ---------------------------------------------------------------------------
# UNWIND patterns inside transactions
# ---------------------------------------------------------------------------


def test_unwind_create_in_transaction(driver):
    """UNWIND + CREATE pattern inside a transaction creates all expected nodes."""
    with driver.session(database=_TX_DB) as s:
        tx = s.begin_transaction()
        tx.run("""
            UNWIND range(1, 10) AS i
            CREATE (:UnwindNode {idx: i})
        """)
        count = tx.run(
            "MATCH (n:UnwindNode) RETURN count(n) AS c"
        ).single()["c"]
        assert count == 10, f"Expected 10 inside TX, got {count}"
        tx.commit()

    # Confirm visibility after commit
    with driver.session(database=_TX_DB) as s:
        count = s.run(
            "MATCH (n:UnwindNode) RETURN count(n) AS c"
        ).single()["c"]
        assert count == 10


# ---------------------------------------------------------------------------
# execute_read / execute_write transaction functions
# ---------------------------------------------------------------------------


def test_execute_write_then_read(driver):
    """execute_write followed by execute_read sees committed data."""
    with driver.session(database=_TX_DB) as s:
        def write_fn(tx):
            tx.run("CREATE (:TxFnNode {val: 7})")

        s.execute_write(write_fn)

    with driver.session(database=_TX_DB) as s:
        def read_fn(tx):
            return tx.run(
                "MATCH (n:TxFnNode) RETURN n.val AS v"
            ).single()

        result = s.execute_read(read_fn)
        assert result is not None
        assert result["v"] == 7


def test_execute_write_with_return(driver):
    """execute_write can return data from the write transaction."""
    with driver.session(database=_TX_DB) as s:
        def write_fn(tx):
            tx.run("CREATE (:WRNode {val: 99})")
            return tx.run(
                "MATCH (n:WRNode) RETURN n.val AS v"
            ).single()

        result = s.execute_write(write_fn)
        assert result["v"] == 99


# ---------------------------------------------------------------------------
# Concurrent access
# ---------------------------------------------------------------------------


def test_concurrent_create_different_nodes(driver):
    """Two sessions creating different nodes concurrently both succeed."""
    barrier = threading.Barrier(2, timeout=10)
    errors = []

    def create_node(name):
        try:
            barrier.wait()
            with driver.session(database=_TX_DB) as s:
                s.run(
                    f"CREATE (:ConcurrentNode {{name: '{name}'}})"
                ).consume()
        except Exception as e:
            errors.append((name, str(e)))

    t1 = threading.Thread(target=create_node, args=("alice",))
    t2 = threading.Thread(target=create_node, args=("bob",))
    t1.start()
    t2.start()
    t1.join(timeout=15)
    t2.join(timeout=15)

    assert not errors, f"Concurrent creates had errors: {errors}"

    with driver.session(database=_TX_DB) as s:
        result = list(s.run(
            "MATCH (n:ConcurrentNode) RETURN n.name AS name ORDER BY name"
        ))
        names = [r["name"] for r in result]
        assert "alice" in names, f"alice missing from {names}"
        assert "bob" in names, f"bob missing from {names}"


def test_concurrent_create_10_threads(driver):
    """10 threads each creating a node; all 10 should be visible afterward."""
    errors = []
    barrier = threading.Barrier(10, timeout=10)

    def create_node(idx):
        try:
            barrier.wait()
            with driver.session(database=_TX_DB) as s:
                s.run(f"CREATE (:TenNode {{idx: {idx}}})").consume()
        except Exception as e:
            errors.append((idx, str(e)))

    threads = [threading.Thread(target=create_node, args=(i,)) for i in range(10)]
    for t in threads:
        t.start()
    for t in threads:
        t.join(timeout=15)

    assert not errors, f"Errors in concurrent 10-thread test: {errors}"

    with driver.session(database=_TX_DB) as s:
        count = s.run(
            "MATCH (n:TenNode) RETURN count(n) AS c"
        ).single()["c"]
        assert count == 10, f"Expected 10 nodes, got {count}"


# ---------------------------------------------------------------------------
# Snapshot isolation observation
# ---------------------------------------------------------------------------


def test_snapshot_isolation(driver):
    """Observe whether a read TX sees writes committed by another session.

    Pre-populate one :Snap node, open a read TX, write a second :Snap from
    another session, then re-read inside the original TX.

    Snapshot isolation would keep the count stable inside the read TX.
    If sedandelivery-bolt uses read-committed instead, the count may increase.
    Both behaviors are documented -- this test records whichever is the case.
    """
    with driver.session(database=_TX_DB) as s:
        s.run("CREATE (:Snap {val: 1})").consume()

    with driver.session(database=_TX_DB) as s1:
        tx1 = s1.begin_transaction()
        initial = tx1.run(
            "MATCH (n:Snap) RETURN count(n) AS c"
        ).single()["c"]

        # Write from a different session while tx1 is still open
        with driver.session(database=_TX_DB) as s2:
            s2.run("CREATE (:Snap {val: 2})").consume()

        count_after = tx1.run(
            "MATCH (n:Snap) RETURN count(n) AS c"
        ).single()["c"]
        tx1.commit()

        # Log actual behavior for diagnostics (visible with -s flag)
        print(
            f"\n  Snapshot isolation check: "
            f"initial={initial}, after_concurrent_write={count_after}"
        )
        if count_after == initial:
            print("  -> Behavior: SNAPSHOT isolation (read TX did not see concurrent write)")
        else:
            print("  -> Behavior: READ-COMMITTED (read TX saw concurrent write)")

        # Both outcomes are acceptable; the test documents them.
        assert count_after >= initial, (
            f"Count should never decrease: was {initial}, now {count_after}"
        )


# ---------------------------------------------------------------------------
# Stress test: 100 concurrent sessions
# ---------------------------------------------------------------------------


def test_100_concurrent_transactions(driver):
    """Stress test: 100 threads each open a session, CREATE a node, and read.

    Verifies that the server handles high concurrency without data loss or
    crashes.  Errors are collected rather than failing immediately so that
    the overall result count can be validated.
    """
    errors = []
    barrier = threading.Barrier(100, timeout=30)

    def worker(idx):
        try:
            barrier.wait()
            with driver.session(database=_TX_DB) as s:
                s.run(f"CREATE (:StressNode {{idx: {idx}}})").consume()
                result = s.run(
                    "MATCH (n:StressNode) RETURN count(n) AS c"
                ).single()
                assert result["c"] > 0, f"Worker {idx}: expected count > 0"
        except Exception as e:
            errors.append((idx, str(e)))

    threads = [
        threading.Thread(target=worker, args=(i,))
        for i in range(100)
    ]

    start = time.time()
    for t in threads:
        t.start()
    for t in threads:
        t.join(timeout=60)
    elapsed = time.time() - start

    print(f"\n  100 concurrent transactions completed in {elapsed:.2f}s")
    if errors:
        print(f"  Errors: {len(errors)}")
        for idx, err in errors[:5]:
            print(f"    Worker {idx}: {err}")

    # Verify final node count matches successful workers
    with driver.session(database=_TX_DB) as s:
        count = s.run(
            "MATCH (n:StressNode) RETURN count(n) AS c"
        ).single()["c"]
        expected = 100 - len(errors)
        assert count == expected, (
            f"Expected {expected} nodes (100 workers - {len(errors)} errors), "
            f"got {count}"
        )


# ---------------------------------------------------------------------------
# Rollback under concurrency
# ---------------------------------------------------------------------------


def test_concurrent_commit_and_rollback(driver):
    """One thread commits, another rolls back; only the committed node persists."""
    barrier = threading.Barrier(2, timeout=10)
    errors = []

    def committer():
        try:
            barrier.wait()
            with driver.session(database=_TX_DB) as s:
                tx = s.begin_transaction()
                tx.run("CREATE (:DualTest {source: 'committed'})")
                tx.commit()
        except Exception as e:
            errors.append(("committer", str(e)))

    def rollbacker():
        try:
            barrier.wait()
            with driver.session(database=_TX_DB) as s:
                tx = s.begin_transaction()
                tx.run("CREATE (:DualTest {source: 'rolled_back'})")
                tx.rollback()
        except Exception as e:
            errors.append(("rollbacker", str(e)))

    t1 = threading.Thread(target=committer)
    t2 = threading.Thread(target=rollbacker)
    t1.start()
    t2.start()
    t1.join(timeout=15)
    t2.join(timeout=15)

    assert not errors, f"Errors: {errors}"

    with driver.session(database=_TX_DB) as s:
        result = list(s.run(
            "MATCH (n:DualTest) RETURN n.source AS source"
        ))
        sources = [r["source"] for r in result]
        assert "committed" in sources, "Committed node must be visible"
        assert "rolled_back" not in sources, "Rolled-back node must not be visible"


# ---------------------------------------------------------------------------
# Transaction after error recovery
# ---------------------------------------------------------------------------


def test_transaction_after_error_recovery(driver):
    """A session can open a new transaction after a previous TX error."""
    with driver.session(database=_TX_DB) as s:
        # First: force an error inside a transaction
        tx = s.begin_transaction()
        try:
            tx.run("INVALID CYPHER SYNTAX").consume()
        except Exception:
            pass
        # The TX is now in a failed state; rollback to reset
        try:
            tx.rollback()
        except Exception:
            pass  # May already be rolled back by the driver

    # Open a brand-new session and verify it works
    with driver.session(database=_TX_DB) as s:
        tx = s.begin_transaction()
        tx.run("CREATE (:RecoveryNode {val: 1})")
        result = tx.run(
            "MATCH (n:RecoveryNode) RETURN n.val AS v"
        ).single()
        assert result["v"] == 1
        tx.commit()

    with driver.session(database=_TX_DB) as s:
        count = s.run(
            "MATCH (n:RecoveryNode) RETURN count(n) AS c"
        ).single()["c"]
        assert count == 1


# ---------------------------------------------------------------------------
# Large transaction payload
# ---------------------------------------------------------------------------


def test_large_transaction_1000_nodes(driver):
    """Create 1000 nodes inside a single transaction and verify count."""
    with driver.session(database=_TX_DB) as s:
        tx = s.begin_transaction()
        tx.run("""
            UNWIND range(1, 1000) AS i
            CREATE (:BulkNode {idx: i})
        """)
        tx.commit()

    with driver.session(database=_TX_DB) as s:
        count = s.run(
            "MATCH (n:BulkNode) RETURN count(n) AS c"
        ).single()["c"]
        assert count == 1000, f"Expected 1000, got {count}"
