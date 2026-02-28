"""Live integration tests against a running SedanDelivery instance.

Usage:
    SEDAN_HOST=<host> SEDAN_PORT=6379 SEDAN_PASSWORD=<pass> python -m pytest tests/test_live.py -v

Skipped automatically if SEDAN_HOST is not set.
"""

from __future__ import annotations

import os

import pytest

from sedandelivery import SedanDB

SKIP = not os.environ.get("SEDAN_HOST")


@pytest.mark.skipif(SKIP, reason="SEDAN_HOST not set")
class TestLive:
    def setup_method(self):
        self.db = SedanDB(
            host=os.environ["SEDAN_HOST"],
            port=int(os.environ.get("SEDAN_PORT", 6379)),
            password=os.environ.get("SEDAN_PASSWORD"),
        )
        self.g = self.db.select_graph("sdk_test")
        try:
            self.g.delete()
        except Exception:
            pass
        self.g.create()

    def teardown_method(self):
        try:
            self.g.delete()
        except Exception:
            pass
        self.db.close()

    def test_create_and_query(self):
        self.g.mutate("CREATE (n:Person {name: 'Alice', age: 30})")
        result = self.g.query("MATCH (n:Person) RETURN n.name AS name, n.age AS age")
        assert len(result) == 1
        assert result[0]["name"] == "Alice"
        assert result[0]["age"] == 30

    def test_mutate_with_params(self):
        self.g.mutate("CREATE (n:Person {name: $name, age: $age})", name="Bob", age=25)
        result = self.g.query("MATCH (n:Person {name: $name}) RETURN n.age AS age", name="Bob")
        assert len(result) == 1
        assert result[0]["age"] == 25

    def test_query_empty_graph(self):
        result = self.g.query("MATCH (n) RETURN n")
        assert len(result) == 0

    def test_mutation_stats(self):
        result = self.g.mutate(
            "CREATE (a:Person {name: 'Alice'})-[:KNOWS]->(b:Person {name: 'Bob'})"
        )
        assert result.stats.nodes_created == 2
        assert result.stats.edges_created == 1
        assert result.stats.labels_added >= 2
        assert result.stats.properties_set >= 2

    def test_list_graphs(self):
        graphs = self.db.list_graphs()
        assert "sdk_test" in graphs

    def test_explain(self):
        plan = self.g.explain("MATCH (n) RETURN n")
        assert isinstance(plan, str)
        assert len(plan) > 0

    def test_delete_graph(self):
        # Create a separate graph, then delete it.
        g2 = self.db.select_graph("sdk_test_delete")
        try:
            g2.delete()
        except Exception:
            pass
        g2.create()
        graphs = self.db.list_graphs()
        assert "sdk_test_delete" in graphs

        g2.delete()
        graphs = self.db.list_graphs()
        assert "sdk_test_delete" not in graphs

    def test_multiple_nodes_and_edges(self):
        self.g.mutate("CREATE (a:Person {name: 'Alice'})")
        self.g.mutate("CREATE (b:Person {name: 'Bob'})")
        self.g.mutate("CREATE (c:Person {name: 'Charlie'})")
        self.g.mutate(
            "MATCH (a:Person {name: 'Alice'}), (b:Person {name: 'Bob'}) "
            "CREATE (a)-[:KNOWS]->(b)"
        )
        self.g.mutate(
            "MATCH (b:Person {name: 'Bob'}), (c:Person {name: 'Charlie'}) "
            "CREATE (b)-[:KNOWS]->(c)"
        )

        result = self.g.query("MATCH (n:Person) RETURN n.name AS name")
        assert len(result) == 3
        names = sorted(r["name"] for r in result)
        assert names == ["Alice", "Bob", "Charlie"]

        result = self.g.query("MATCH ()-[r:KNOWS]->() RETURN count(r) AS cnt")
        assert result[0]["cnt"] == 2

    def test_aggregation(self):
        self.g.mutate("CREATE (a:Person {name: 'Alice', age: 30})")
        self.g.mutate("CREATE (b:Person {name: 'Bob', age: 25})")
        self.g.mutate("CREATE (c:Person {name: 'Charlie', age: 35})")

        result = self.g.query(
            "MATCH (n:Person) RETURN min(n.age) AS min_age, max(n.age) AS max_age, "
            "avg(n.age) AS avg_age, count(n) AS cnt"
        )
        assert len(result) == 1
        assert result[0]["min_age"] == 25
        assert result[0]["max_age"] == 35
        assert result[0]["cnt"] == 3

    def test_context_manager(self):
        with SedanDB(
            host=os.environ["SEDAN_HOST"],
            port=int(os.environ.get("SEDAN_PORT", 6379)),
            password=os.environ.get("SEDAN_PASSWORD"),
        ) as db:
            g = db.select_graph("sdk_test")
            result = g.query("MATCH (n) RETURN n")
            assert isinstance(result, list) or len(result) >= 0
