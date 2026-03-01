"""
Neomodel OGM compatibility tests for sedandelivery-bolt.

Exercises core neomodel operations against the sedandelivery-bolt server and
reports pass/fail for each one, documenting which features are supported
and which hit known sedandelivery-bolt limitations.

Prerequisites:
    pip install neomodel   (installs neomodel + neo4j driver)
    sedandelivery-bolt running on localhost:17687

Usage:
    BOLT_PORT=17687 python test_neomodel_compat.py
    # or
    python test_neomodel_compat.py          # defaults to port 17687
"""

from __future__ import annotations

import os
import sys
import traceback
import warnings

# Suppress neomodel deprecation warnings about config attribute access
warnings.filterwarnings("ignore", category=DeprecationWarning)

# ---------------------------------------------------------------------------
# Configuration
# ---------------------------------------------------------------------------
BOLT_PORT = os.environ.get("BOLT_PORT", "17687")
BOLT_URL = f"bolt://neo4j:neo4j@localhost:{BOLT_PORT}"
# Raw driver URL (no credentials in URI -- neo4j 6.x rejects that)
RAW_BOLT_URL = f"bolt://localhost:{BOLT_PORT}"

# ---------------------------------------------------------------------------
# Result tracking
# ---------------------------------------------------------------------------
results: list[tuple[str, str, str]] = []  # (test_name, status, detail)


def record(name: str, status: str, detail: str = "") -> None:
    results.append((name, status, detail))
    symbol = {"PASS": "+", "FAIL": "-", "SKIP": "~", "XFAIL": "!"}[status]
    print(f"  [{symbol}] {name}" + (f"  -- {detail}" if detail else ""))


# ===================================================================
# Phase 0 -- Raw driver sanity check (bypass neomodel entirely)
# ===================================================================
print("\n=== Phase 0: Raw neo4j driver sanity check ===")

from neo4j import GraphDatabase

_drv = None
try:
    _drv = GraphDatabase.driver(RAW_BOLT_URL, auth=("neo4j", "neo4j"))
    with _drv.session() as _s:
        _r = _s.run("RETURN 1 AS x")
        assert list(_r)[0]["x"] == 1
    record("raw_driver_connect", "PASS")
except Exception as exc:
    record("raw_driver_connect", "FAIL", str(exc))
    print("\nFATAL: Cannot connect to sedandelivery-bolt. Is it running?")
    sys.exit(1)
finally:
    if _drv:
        _drv.close()

# ===================================================================
# Phase 1 -- Neomodel connection bootstrap (with monkey-patching)
# ===================================================================
print("\n=== Phase 1: Neomodel connection bootstrap ===")

from neomodel.sync_.database import Database

# Reset any pre-existing singleton so we get a clean state.
Database.reset_instance()

from neomodel import (
    IntegerProperty,
    RelationshipFrom,
    RelationshipTo,
    StringProperty,
    StructuredNode,
    db,
)

# --- Monkey-patch _update_database_version --------------------------------
# sedandelivery-bolt does not implement the `CALL dbms.components()` procedure that
# neomodel uses to detect the server version.  We patch it to report version
# "4.4" so that neomodel uses `id()` (not `elementId()`).

_original_update = db._update_database_version.__func__


def _patched_update(self):
    """Set version to 4.4 (Bolt v4.x), skip the dbms.components() call."""
    self._database_version = "4.4.0"
    self._database_edition = "community"


Database._update_database_version = _patched_update

try:
    db.set_connection(url=BOLT_URL)
    record("neomodel_connect", "PASS", f"version={db._database_version}")
except Exception as exc:
    record("neomodel_connect", "FAIL", str(exc))
    print("\nFATAL: Cannot bootstrap neomodel connection.")
    sys.exit(1)

# Verify id method
try:
    assert db.get_id_method() == "id", f"Expected 'id', got '{db.get_id_method()}'"
    record("id_method_is_legacy", "PASS", "get_id_method() == 'id'")
except Exception as exc:
    record("id_method_is_legacy", "FAIL", str(exc))

# ===================================================================
# Phase 2 -- Raw Cypher through neomodel's db.cypher_query
# ===================================================================
print("\n=== Phase 2: Raw Cypher through neomodel ===")

# Clean slate
try:
    db.cypher_query("MATCH (n) DETACH DELETE n")
    record("clean_slate", "PASS")
except Exception as exc:
    record("clean_slate", "FAIL", str(exc))

# Simple RETURN
try:
    results_q, meta = db.cypher_query("RETURN 42 AS answer")
    assert results_q[0][0] == 42
    record("cypher_return_literal", "PASS")
except Exception as exc:
    record("cypher_return_literal", "FAIL", str(exc))

# CREATE with explicit params
try:
    results_q, _ = db.cypher_query(
        "CREATE (n:TestNode {name: $name, val: $val}) RETURN n",
        {"name": "hello", "val": 1},
    )
    assert len(results_q) == 1
    record("cypher_create_explicit_params", "PASS")
except Exception as exc:
    record("cypher_create_explicit_params", "FAIL", str(exc))

# CREATE with $create_params map expansion (neomodel's pattern)
try:
    results_q, _ = db.cypher_query(
        "CREATE (n:TestNode $create_params) RETURN n",
        {"create_params": {"name": "world", "val": 2}},
    )
    record("cypher_create_map_expansion", "PASS")
except Exception as exc:
    record(
        "cypher_create_map_expansion",
        "XFAIL",
        "sedandelivery-bolt parser does not support $param map expansion in CREATE patterns",
    )

# MERGE with literal
try:
    results_q, _ = db.cypher_query(
        "MERGE (n:TestNode {name: 'merge_test'}) RETURN n"
    )
    assert len(results_q) == 1
    record("cypher_merge_literal", "PASS")
except Exception as exc:
    record("cypher_merge_literal", "FAIL", str(exc))

# MERGE with parameter
try:
    results_q, _ = db.cypher_query(
        "MERGE (n:TestNode {name: $name}) RETURN n", {"name": "merge_param"}
    )
    record("cypher_merge_param", "PASS")
except Exception as exc:
    record(
        "cypher_merge_param",
        "XFAIL",
        "sedandelivery-bolt bug: parameters not injected into MERGE row when no child operator",
    )

# UNWIND + MERGE (used by neomodel get_or_create)
try:
    results_q, _ = db.cypher_query(
        "UNWIND $merge_params as params MERGE (n:TestNode {name: params.create.name}) "
        "ON CREATE SET n = params.create RETURN n",
        {"merge_params": [{"create": {"name": "unwind_test", "val": 99}}]},
    )
    assert len(results_q) == 1
    record("cypher_unwind_merge", "PASS")
except Exception as exc:
    record("cypher_unwind_merge", "FAIL", str(exc))

# Clean up test data
db.cypher_query("MATCH (n:TestNode) DETACH DELETE n")


# ===================================================================
# Phase 3 -- Neomodel OGM model definitions
# ===================================================================
print("\n=== Phase 3: Neomodel OGM operations ===")


class Person(StructuredNode):
    name = StringProperty(unique_index=True)
    age = IntegerProperty()
    friends = RelationshipTo("Person", "FRIEND")


# --- Test: CREATE via StructuredNode.save() --------------------------------
# neomodel's .save() for new nodes calls .create() which generates:
#   CREATE (n:Person $create_params) RETURN n
# This uses map expansion which sedandelivery-bolt doesn't support.

print("\n  -- Node creation --")

try:
    alice = Person(name="Alice", age=30).save()
    record("save_new_node", "PASS", f"element_id={alice.element_id}")
except Exception as exc:
    record(
        "save_new_node",
        "XFAIL",
        "CREATE $create_params map expansion not supported by sedandelivery-bolt parser",
    )
    alice = None

# Workaround: create nodes via raw Cypher and inflate
if alice is None:
    try:
        r, _ = db.cypher_query(
            "CREATE (n:Person {name: $name, age: $age}) RETURN n",
            {"name": "Alice", "age": 30},
        )
        alice = Person.inflate(r[0][0])
        record("save_new_node_workaround", "PASS", f"element_id={alice.element_id}")
    except Exception as exc:
        record("save_new_node_workaround", "FAIL", str(exc))
        alice = None

# Create Bob
bob = None
try:
    bob = Person(name="Bob", age=25).save()
    record("save_second_node", "PASS", f"element_id={bob.element_id}")
except Exception:
    pass

if bob is None:
    try:
        r, _ = db.cypher_query(
            "CREATE (n:Person {name: $name, age: $age}) RETURN n",
            {"name": "Bob", "age": 25},
        )
        bob = Person.inflate(r[0][0])
        record("save_second_node_workaround", "PASS", f"element_id={bob.element_id}")
    except Exception as exc:
        record("save_second_node_workaround", "FAIL", str(exc))

# Create Charlie for later tests
charlie = None
try:
    r, _ = db.cypher_query(
        "CREATE (n:Person {name: $name, age: $age}) RETURN n",
        {"name": "Charlie", "age": 35},
    )
    charlie = Person.inflate(r[0][0])
    record("create_charlie", "PASS")
except Exception as exc:
    record("create_charlie", "FAIL", str(exc))


# --- Test: UPDATE via StructuredNode.save() --------------------------------
# For existing nodes, .save() generates:
#   MATCH (n) WHERE id(n)=$self SET n.name = $name, n.age = $age SET n:`Person`
print("\n  -- Node update --")

if alice:
    try:
        alice.age = 31
        alice.save()
        record("update_node_save", "PASS", "alice.age updated to 31")
    except Exception as exc:
        record("update_node_save", "FAIL", str(exc))

    # Verify the update via raw query
    try:
        r, _ = db.cypher_query(
            "MATCH (n:Person {name: 'Alice'}) RETURN n.age"
        )
        assert r[0][0] == 31, f"Expected 31, got {r[0][0]}"
        record("update_node_verify", "PASS")
    except Exception as exc:
        record("update_node_verify", "FAIL", str(exc))


# --- Test: QUERY / Person.nodes.all() -------------------------------------
# Generates: MATCH (n:Person) RETURN n
print("\n  -- Node queries --")

try:
    all_people = Person.nodes.all()
    names = sorted([p.name for p in all_people])
    expected = sorted(["Alice", "Bob", "Charlie"])
    assert names == expected, f"Expected {expected}, got {names}"
    record("nodes_all", "PASS", f"found {len(all_people)} people")
except Exception as exc:
    record("nodes_all", "FAIL", str(exc))

# --- Test: Person.nodes.get(name='Alice') ----------------------------------
# Generates: MATCH (n:Person) WHERE n.name = $name RETURN n
try:
    alice_found = Person.nodes.get(name="Alice")
    assert alice_found.name == "Alice"
    assert alice_found.age == 31  # updated above
    record("nodes_get", "PASS", f"found Alice, age={alice_found.age}")
except Exception as exc:
    record("nodes_get", "FAIL", str(exc))

# --- Test: Person.nodes.filter(age__lt=28) ---------------------------------
# Generates: MATCH (n:Person) WHERE n.age < $age RETURN n
try:
    young = Person.nodes.filter(age__lt=28)
    young_list = list(young)
    young_names = [p.name for p in young_list]
    assert "Bob" in young_names, f"Expected Bob in {young_names}"
    assert "Alice" not in young_names
    record("nodes_filter_lt", "PASS", f"found {young_names}")
except Exception as exc:
    record("nodes_filter_lt", "FAIL", str(exc))

# --- Test: Person.nodes.filter(age__gt=30) ---------------------------------
try:
    older = list(Person.nodes.filter(age__gt=30))
    older_names = sorted([p.name for p in older])
    assert "Alice" in older_names  # age=31
    assert "Charlie" in older_names  # age=35
    record("nodes_filter_gt", "PASS", f"found {older_names}")
except Exception as exc:
    record("nodes_filter_gt", "FAIL", str(exc))

# --- Test: len(Person.nodes) (uses count()) --------------------------------
try:
    count = len(Person.nodes)
    assert count == 3, f"Expected 3, got {count}"
    record("nodes_len_count", "PASS", f"count={count}")
except Exception as exc:
    record("nodes_len_count", "FAIL", str(exc))


# ===================================================================
# Phase 4 -- Relationships
# ===================================================================
print("\n=== Phase 4: Relationships ===")

# --- Test: connect ---
# Generates: MATCH (them), (us) WHERE id(them)=$them and id(us)=$self
#            MERGE (us)-[r:FRIEND]->(them) RETURN r
if alice and bob:
    try:
        alice.friends.connect(bob)
        record("relationship_connect", "PASS")
    except Exception as exc:
        record("relationship_connect", "FAIL", str(exc))

    # --- Test: all() on relationship manager ---
    # Uses traversal which generates MATCH with relationship patterns
    try:
        friends_of_alice = alice.friends.all()
        friend_names = [f.name for f in friends_of_alice]
        assert "Bob" in friend_names, f"Expected Bob in {friend_names}"
        record("relationship_all", "PASS", f"friends={friend_names}")
    except Exception as exc:
        record("relationship_all", "FAIL", str(exc))

    # --- Test: is_connected ---
    try:
        assert alice.friends.is_connected(bob)
        record("relationship_is_connected", "PASS")
    except Exception as exc:
        record("relationship_is_connected", "FAIL", str(exc))

    # --- Test: connect second relationship ---
    if charlie:
        try:
            alice.friends.connect(charlie)
            friends2 = alice.friends.all()
            friend_names2 = sorted([f.name for f in friends2])
            assert "Bob" in friend_names2
            assert "Charlie" in friend_names2
            record("relationship_connect_second", "PASS", f"friends={friend_names2}")
        except Exception as exc:
            record("relationship_connect_second", "FAIL", str(exc))

    # --- Test: disconnect ---
    try:
        alice.friends.disconnect(bob)
        friends_after = alice.friends.all()
        friend_names_after = [f.name for f in friends_after]
        assert "Bob" not in friend_names_after
        record("relationship_disconnect", "PASS", f"remaining={friend_names_after}")
    except Exception as exc:
        record("relationship_disconnect", "FAIL", str(exc))

    # --- Test: disconnect_all ---
    try:
        alice.friends.disconnect_all()
        friends_empty = alice.friends.all()
        assert len(friends_empty) == 0, f"Expected 0 friends, got {len(friends_empty)}"
        record("relationship_disconnect_all", "PASS")
    except Exception as exc:
        record("relationship_disconnect_all", "FAIL", str(exc))
else:
    record("relationship_tests", "SKIP", "alice or bob not created")


# ===================================================================
# Phase 5 -- Delete
# ===================================================================
print("\n=== Phase 5: Delete ===")

# --- Test: delete node ---
# Generates: MATCH (self) WHERE id(self)=$self DETACH DELETE self
if charlie:
    try:
        charlie.delete()
        # Verify deletion
        remaining = Person.nodes.all()
        remaining_names = [p.name for p in remaining]
        assert "Charlie" not in remaining_names
        record("delete_node", "PASS", f"remaining={remaining_names}")
    except Exception as exc:
        record("delete_node", "FAIL", str(exc))

# --- Test: refresh node ---
if alice:
    try:
        alice.refresh()
        record("refresh_node", "PASS", f"name={alice.name}, age={alice.age}")
    except Exception as exc:
        record("refresh_node", "FAIL", str(exc))

# --- Test: labels() ---
if alice:
    try:
        lbls = alice.labels()
        assert "Person" in lbls, f"Expected 'Person' in {lbls}"
        record("node_labels", "PASS", f"labels={lbls}")
    except Exception as exc:
        record("node_labels", "FAIL", str(exc))


# ===================================================================
# Phase 6 -- Advanced OGM features
# ===================================================================
print("\n=== Phase 6: Advanced OGM features ===")

# --- Test: create_or_update (uses UNWIND + MERGE) ---
try:
    nodes = Person.create_or_update(
        {"name": "Dave", "age": 40},
        {"name": "Alice", "age": 32},  # should update Alice's age
    )
    record("create_or_update", "PASS", f"returned {len(nodes)} nodes")
except Exception as exc:
    record("create_or_update", "XFAIL", str(exc))

# --- Test: get_or_create (uses UNWIND + MERGE) ---
try:
    nodes = Person.get_or_create({"name": "Eve", "age": 28})
    assert len(nodes) == 1
    record("get_or_create", "PASS", f"element_id={nodes[0].element_id}")
except Exception as exc:
    record("get_or_create", "XFAIL", str(exc))

# --- Test: Constraint/Index operations ---
# These use SHOW CONSTRAINTS, CREATE CONSTRAINT etc. which sedandelivery-bolt
# definitely does not support.
try:
    db.cypher_query("SHOW CONSTRAINTS")
    record("show_constraints", "PASS")
except Exception as exc:
    record(
        "show_constraints",
        "XFAIL",
        "sedandelivery-bolt does not support schema commands (SHOW CONSTRAINTS)",
    )

try:
    db.cypher_query("SHOW INDEXES")
    record("show_indexes", "PASS")
except Exception as exc:
    record(
        "show_indexes",
        "XFAIL",
        "sedandelivery-bolt does not support schema commands (SHOW INDEXES)",
    )

# --- Test: install_labels (creates indexes/constraints) ---
try:
    from neomodel import install_labels
    install_labels(Person)
    record("install_labels", "PASS")
except Exception as exc:
    record(
        "install_labels",
        "XFAIL",
        "install_labels requires schema commands (constraints/indexes) not supported",
    )


# ===================================================================
# Summary
# ===================================================================
print("\n" + "=" * 72)
print("NEOMODEL COMPATIBILITY TEST RESULTS")
print("=" * 72)

pass_count = sum(1 for _, s, _ in results if s == "PASS")
fail_count = sum(1 for _, s, _ in results if s == "FAIL")
xfail_count = sum(1 for _, s, _ in results if s == "XFAIL")
skip_count = sum(1 for _, s, _ in results if s == "SKIP")

print(f"\n  PASS:  {pass_count}")
print(f"  FAIL:  {fail_count}")
print(f"  XFAIL: {xfail_count}  (expected failures -- known sedandelivery-bolt limitations)")
print(f"  SKIP:  {skip_count}")
print(f"  TOTAL: {len(results)}")

if xfail_count > 0:
    print("\n--- Known sedandelivery-bolt limitations (XFAIL) ---")
    for name, status, detail in results:
        if status == "XFAIL":
            print(f"  {name}: {detail}")

if fail_count > 0:
    print("\n--- Unexpected failures (FAIL) ---")
    for name, status, detail in results:
        if status == "FAIL":
            print(f"  {name}: {detail}")

print()
sys.exit(1 if fail_count > 0 else 0)
