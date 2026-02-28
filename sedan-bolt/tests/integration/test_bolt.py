"""
Integration tests for sedan-bolt via the neo4j Python driver.

Requirements:
    pip install neo4j
    sedan-bolt running on localhost:7687 (or BOLT_URI env var)

Run:
    BOLT_URI=bolt://localhost:7687 pytest tests/integration/test_bolt.py -v
"""
import os
import pytest
from neo4j import GraphDatabase
from neo4j.exceptions import (
    CypherSyntaxError,
    DatabaseError,
)

BOLT_URI = os.environ.get("BOLT_URI", "bolt://localhost:7687")


@pytest.fixture(scope="module")
def driver():
    d = GraphDatabase.driver(BOLT_URI, auth=None)
    yield d
    d.close()


def _session(driver, db_name):
    """Helper to open a session with a given database name."""
    return driver.session(database=db_name)


# ---------------------------------------------------------------------------
# Phase 5: Basic query execution (expanded)
# ---------------------------------------------------------------------------

class TestBasicQueries:
    """Test basic RETURN expressions, types, and simple operations."""

    DB = "test_basic"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_return_literal_integer(self):
        records = list(self.s.run("RETURN 1 AS n"))
        assert len(records) == 1
        assert records[0]["n"] == 1

    def test_return_string(self):
        records = list(self.s.run("RETURN 'hello' AS greeting"))
        assert records[0]["greeting"] == "hello"

    def test_return_float(self):
        records = list(self.s.run("RETURN 3.14 AS pi"))
        assert abs(records[0]["pi"] - 3.14) < 0.001

    def test_return_null(self):
        records = list(self.s.run("RETURN null AS x"))
        assert records[0]["x"] is None

    def test_return_boolean_true_and_false(self):
        records = list(self.s.run("RETURN true AS t, false AS f"))
        assert records[0]["t"] is True
        assert records[0]["f"] is False

    def test_return_list(self):
        records = list(self.s.run("RETURN [1, 2, 3] AS nums"))
        assert records[0]["nums"] == [1, 2, 3]

    def test_return_map(self):
        records = list(self.s.run("RETURN {a: 1, b: 'two'} AS m"))
        m = records[0]["m"]
        assert m["a"] == 1
        assert m["b"] == "two"

    def test_return_arithmetic_addition(self):
        records = list(self.s.run("RETURN 1 + 2 AS r"))
        assert records[0]["r"] == 3

    def test_return_arithmetic_subtraction(self):
        records = list(self.s.run("RETURN 10 - 3 AS r"))
        assert records[0]["r"] == 7

    def test_return_arithmetic_multiplication(self):
        records = list(self.s.run("RETURN 10 * 5 AS r"))
        assert records[0]["r"] == 50

    def test_return_arithmetic_division(self):
        records = list(self.s.run("RETURN 10 / 3 AS r"))
        # Integer division in Cypher returns integer
        assert records[0]["r"] == 3

    def test_return_arithmetic_modulo(self):
        records = list(self.s.run("RETURN 10 % 3 AS r"))
        assert records[0]["r"] == 1

    def test_return_arithmetic_power(self):
        records = list(self.s.run("RETURN 2 ^ 3 AS r"))
        assert records[0]["r"] == 8.0

    def test_return_string_concatenation(self):
        records = list(self.s.run("RETURN 'hello' + ' ' + 'world' AS r"))
        assert records[0]["r"] == "hello world"

    def test_return_nested_list(self):
        records = list(self.s.run("RETURN [[1, 2], [3, 4]] AS r"))
        assert records[0]["r"] == [[1, 2], [3, 4]]

    def test_return_nested_map(self):
        records = list(self.s.run("RETURN {a: {b: 1, c: 'x'}} AS r"))
        r = records[0]["r"]
        assert r["a"]["b"] == 1
        assert r["a"]["c"] == "x"

    def test_return_map_in_list(self):
        records = list(self.s.run("RETURN [{a: 1}, {a: 2}] AS r"))
        assert records[0]["r"] == [{"a": 1}, {"a": 2}]

    def test_return_deep_nesting(self):
        records = list(self.s.run("RETURN {a: {b: {c: [1, {d: 2}]}}} AS r"))
        r = records[0]["r"]
        assert r["a"]["b"]["c"][0] == 1
        assert r["a"]["b"]["c"][1]["d"] == 2

    def test_return_multiple_columns_mixed_types(self):
        records = list(self.s.run(
            "RETURN 1 AS a, 'hello' AS b, true AS c, null AS d, [1,2] AS e, {k: 'v'} AS f"
        ))
        assert len(records) == 1
        r = records[0]
        assert r["a"] == 1
        assert r["b"] == "hello"
        assert r["c"] is True
        assert r["d"] is None
        assert r["e"] == [1, 2]
        assert r["f"] == {"k": "v"}

    def test_unwind_basic(self):
        records = list(self.s.run("UNWIND [1, 2, 3] AS x RETURN x"))
        values = [r["x"] for r in records]
        assert values == [1, 2, 3]

    def test_unwind_range(self):
        records = list(self.s.run("UNWIND range(1, 10) AS x RETURN x"))
        values = [r["x"] for r in records]
        assert values == list(range(1, 11))

    def test_unwind_different_types(self):
        records = list(self.s.run(
            "UNWIND [1, 'two', true, null, 3.14] AS x RETURN x"
        ))
        values = [r["x"] for r in records]
        assert values[0] == 1
        assert values[1] == "two"
        assert values[2] is True
        assert values[3] is None
        assert abs(values[4] - 3.14) < 0.001

    def test_large_result_set(self):
        records = list(self.s.run("UNWIND range(1, 100) AS x RETURN x"))
        assert len(records) == 100
        assert records[0]["x"] == 1
        assert records[99]["x"] == 100

    def test_empty_result_from_match(self):
        records = list(self.s.run("MATCH (n:NonExistentLabel) RETURN n"))
        assert len(records) == 0

    def test_parameters_addition(self):
        records = list(self.s.run("RETURN $x + $y AS sum", x=10, y=20))
        assert records[0]["sum"] == 30

    def test_parameters_string(self):
        records = list(self.s.run("RETURN $name AS n", name="Alice"))
        assert records[0]["n"] == "Alice"

    def test_parameters_list(self):
        records = list(self.s.run("RETURN $items AS r", items=[1, 2, 3]))
        assert records[0]["r"] == [1, 2, 3]

    def test_result_summary_timing(self):
        result = self.s.run("RETURN 1 AS n")
        summary = result.consume()
        assert summary.result_available_after is not None

    def test_result_summary_query_type_read(self):
        result = self.s.run("RETURN 1 AS n")
        summary = result.consume()
        assert summary.query_type == "r"

    def test_result_summary_server_agent(self):
        result = self.s.run("RETURN 1 AS n")
        summary = result.consume()
        assert "SedanDelivery" in summary.server.agent

    def test_negation(self):
        records = list(self.s.run("RETURN -42 AS r"))
        assert records[0]["r"] == -42

    def test_boolean_not(self):
        records = list(self.s.run("RETURN NOT true AS r"))
        assert records[0]["r"] is False

    def test_boolean_and(self):
        records = list(self.s.run("RETURN true AND false AS r"))
        assert records[0]["r"] is False

    def test_boolean_or(self):
        records = list(self.s.run("RETURN true OR false AS r"))
        assert records[0]["r"] is True

    def test_in_list(self):
        records = list(self.s.run("RETURN 2 IN [1, 2, 3] AS r"))
        assert records[0]["r"] is True

    def test_not_in_list(self):
        records = list(self.s.run("RETURN 5 IN [1, 2, 3] AS r"))
        assert records[0]["r"] is False

    def test_is_null(self):
        records = list(self.s.run("RETURN null IS NULL AS r"))
        assert records[0]["r"] is True

    def test_is_not_null(self):
        records = list(self.s.run("RETURN 1 IS NOT NULL AS r"))
        assert records[0]["r"] is True


# ---------------------------------------------------------------------------
# Phase 6: Full Cypher CRUD (expanded)
# ---------------------------------------------------------------------------

class TestCypherCreate:
    """Test CREATE operations and node/relationship structures."""

    DB = "test_create"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_create_and_match_node(self):
        self.s.run("CREATE (:Person {name: 'Alice', age: 30})").consume()
        records = list(self.s.run(
            "MATCH (p:Person {name: 'Alice'}) RETURN p.name AS name, p.age AS age"
        ))
        assert len(records) == 1
        assert records[0]["name"] == "Alice"
        assert records[0]["age"] == 30

    def test_create_multiple_nodes_one_query(self):
        self.s.run(
            "CREATE (a:Multi {name: 'A'}), (b:Multi {name: 'B'}), (c:Multi {name: 'C'})"
        ).consume()
        records = list(self.s.run(
            "MATCH (m:Multi) RETURN m.name AS n ORDER BY n"
        ))
        names = [r["n"] for r in records]
        assert "A" in names
        assert "B" in names
        assert "C" in names

    def test_create_with_multiple_labels(self):
        self.s.run("CREATE (n:Person:Employee {name: 'Bob'})").consume()
        records = list(self.s.run(
            "MATCH (n:Person:Employee {name: 'Bob'}) RETURN labels(n) AS l"
        ))
        assert len(records) == 1
        labels = records[0]["l"]
        assert "Person" in labels
        assert "Employee" in labels

    def test_create_relationship(self):
        self.s.run("""
            CREATE (a:Friend {name: 'Alice'})
            CREATE (b:Friend {name: 'Bob'})
            CREATE (a)-[:KNOWS {since: 2020}]->(b)
        """).consume()
        records = list(self.s.run("""
            MATCH (a:Friend)-[r:KNOWS]->(b:Friend)
            RETURN a.name AS from, b.name AS to, r.since AS since
        """))
        assert len(records) >= 1
        row = records[0]
        assert row["from"] == "Alice"
        assert row["to"] == "Bob"
        assert row["since"] == 2020

    def test_return_node_object(self):
        self.s.run("CREATE (:Animal {species: 'Cat', name: 'Whiskers'})").consume()
        records = list(self.s.run("MATCH (a:Animal) RETURN a LIMIT 1"))
        assert len(records) == 1
        node = records[0]["a"]
        assert "Animal" in node.labels
        assert node["species"] == "Cat"
        assert node["name"] == "Whiskers"

    def test_return_relationship_object(self):
        self.s.run("CREATE (:X1)-[:REL {w: 1.5}]->(:Y1)").consume()
        records = list(self.s.run("MATCH ()-[r:REL]->() RETURN r LIMIT 1"))
        assert len(records) == 1
        rel = records[0]["r"]
        assert rel.type == "REL"
        assert rel["w"] == 1.5

    def test_mutation_stats_nodes(self):
        result = self.s.run("CREATE (:StatsNode {x: 1})")
        summary = result.consume()
        counters = summary.counters
        assert counters.nodes_created == 1
        assert counters.properties_set >= 1

    def test_mutation_stats_relationships(self):
        result = self.s.run("CREATE (:SN1)-[:SR {w: 2}]->(:SN2)")
        summary = result.consume()
        counters = summary.counters
        assert counters.nodes_created == 2
        assert counters.relationships_created == 1

    def test_query_type_write(self):
        result = self.s.run("CREATE (:QTNode)")
        summary = result.consume()
        assert summary.query_type in ("w", "rw")


class TestCypherMatch:
    """Test MATCH with WHERE clause variations, ORDER BY, LIMIT, SKIP, DISTINCT."""

    DB = "test_match"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_where_greater_than(self):
        records = list(self.s.run(
            "UNWIND [1,2,3,4,5] AS x WITH x WHERE x > 3 RETURN x ORDER BY x"
        ))
        assert [r["x"] for r in records] == [4, 5]

    def test_where_less_than(self):
        records = list(self.s.run(
            "UNWIND [1,2,3,4,5] AS x WITH x WHERE x < 3 RETURN x ORDER BY x"
        ))
        assert [r["x"] for r in records] == [1, 2]

    def test_where_equals(self):
        records = list(self.s.run(
            "UNWIND [1,2,3] AS x WITH x WHERE x = 2 RETURN x"
        ))
        assert [r["x"] for r in records] == [2]

    def test_where_not_equals(self):
        records = list(self.s.run(
            "UNWIND [1,2,3] AS x WITH x WHERE x <> 2 RETURN x ORDER BY x"
        ))
        assert [r["x"] for r in records] == [1, 3]

    def test_where_greater_or_equal(self):
        records = list(self.s.run(
            "UNWIND [1,2,3,4,5] AS x WITH x WHERE x >= 4 RETURN x ORDER BY x"
        ))
        assert [r["x"] for r in records] == [4, 5]

    def test_where_less_or_equal(self):
        records = list(self.s.run(
            "UNWIND [1,2,3,4,5] AS x WITH x WHERE x <= 2 RETURN x ORDER BY x"
        ))
        assert [r["x"] for r in records] == [1, 2]

    def test_where_contains(self):
        records = list(self.s.run(
            "UNWIND ['apple', 'banana', 'grape'] AS x "
            "WITH x WHERE x CONTAINS 'an' RETURN x ORDER BY x"
        ))
        assert [r["x"] for r in records] == ["banana"]

    def test_where_starts_with(self):
        records = list(self.s.run(
            "UNWIND ['apple', 'banana', 'grape'] AS x "
            "WITH x WHERE x STARTS WITH 'ba' RETURN x"
        ))
        assert [r["x"] for r in records] == ["banana"]

    def test_where_ends_with(self):
        records = list(self.s.run(
            "UNWIND ['apple', 'banana', 'grape'] AS x "
            "WITH x WHERE x ENDS WITH 'pe' RETURN x ORDER BY x"
        ))
        assert [r["x"] for r in records] == ["grape"]

    def test_order_by_ascending(self):
        records = list(self.s.run(
            "UNWIND [3, 1, 4, 1, 5] AS x RETURN x ORDER BY x"
        ))
        assert [r["x"] for r in records] == [1, 1, 3, 4, 5]

    def test_order_by_descending(self):
        records = list(self.s.run(
            "UNWIND [3, 1, 4, 1, 5] AS x RETURN x ORDER BY x DESC"
        ))
        assert [r["x"] for r in records] == [5, 4, 3, 1, 1]

    def test_limit(self):
        records = list(self.s.run(
            "UNWIND range(1, 10) AS x RETURN x LIMIT 3"
        ))
        assert len(records) == 3
        assert [r["x"] for r in records] == [1, 2, 3]

    def test_skip(self):
        records = list(self.s.run(
            "UNWIND range(1, 10) AS x RETURN x SKIP 7"
        ))
        assert [r["x"] for r in records] == [8, 9, 10]

    def test_order_by_limit_skip_combined(self):
        records = list(self.s.run(
            "UNWIND range(1, 10) AS x RETURN x ORDER BY x DESC SKIP 2 LIMIT 3"
        ))
        assert [r["x"] for r in records] == [8, 7, 6]

    def test_distinct(self):
        records = list(self.s.run(
            "UNWIND [1, 1, 2, 2, 3] AS x RETURN DISTINCT x ORDER BY x"
        ))
        assert [r["x"] for r in records] == [1, 2, 3]


class TestCypherMutations:
    """Test SET, REMOVE, DELETE, DETACH DELETE, and MERGE."""

    DB = "test_mutations"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_set_property(self):
        self.s.run("CREATE (n:SetTest {val: 1})").consume()
        self.s.run("MATCH (n:SetTest) SET n.val = 99").consume()
        records = list(self.s.run("MATCH (n:SetTest) RETURN n.val AS v"))
        assert records[0]["v"] == 99

    def test_set_multiple_properties(self):
        self.s.run("CREATE (n:SetMulti {a: 1, b: 2})").consume()
        self.s.run("MATCH (n:SetMulti) SET n.a = 10, n.b = 20").consume()
        records = list(self.s.run("MATCH (n:SetMulti) RETURN n.a AS a, n.b AS b"))
        assert records[0]["a"] == 10
        assert records[0]["b"] == 20

    def test_remove_property(self):
        self.s.run("CREATE (n:RemTest {x: 1, y: 2})").consume()
        self.s.run("MATCH (n:RemTest) REMOVE n.y").consume()
        records = list(self.s.run("MATCH (n:RemTest) RETURN n.x AS x, n.y AS y"))
        assert records[0]["x"] == 1
        assert records[0]["y"] is None

    def test_delete_all_nodes(self):
        self.s.run("CREATE (:DelAll {v: 1})").consume()
        self.s.run("CREATE (:DelAll {v: 2})").consume()
        self.s.run("MATCH (n:DelAll) DELETE n").consume()
        records = list(self.s.run("MATCH (n:DelAll) RETURN count(n) AS c"))
        assert records[0]["c"] == 0

    def test_detach_delete_with_relationships(self):
        self.s.run("CREATE (:DD1)-[:R]->(:DD1)").consume()
        self.s.run("MATCH (n:DD1) DETACH DELETE n").consume()
        records = list(self.s.run("MATCH (n:DD1) RETURN count(n) AS c"))
        assert records[0]["c"] == 0

    def test_merge_creates_when_not_exists(self):
        self.s.run("MERGE (n:MergeNew {key: 'abc'})").consume()
        records = list(self.s.run(
            "MATCH (n:MergeNew {key: 'abc'}) RETURN n.key AS k"
        ))
        assert len(records) == 1
        assert records[0]["k"] == "abc"

    def test_merge_matches_when_exists(self):
        self.s.run("CREATE (:MergeExist {key: 'exists'})").consume()
        self.s.run("MERGE (n:MergeExist {key: 'exists'})").consume()
        records = list(self.s.run(
            "MATCH (n:MergeExist {key: 'exists'}) RETURN count(n) AS c"
        ))
        # Should still be 1 (not duplicated)
        assert records[0]["c"] == 1

    def test_merge_on_create_set(self):
        self.s.run(
            "MERGE (n:MOC {key: 'new'}) ON CREATE SET n.created = true"
        ).consume()
        records = list(self.s.run(
            "MATCH (n:MOC {key: 'new'}) RETURN n.created AS c"
        ))
        assert records[0]["c"] is True


class TestCypherClauses:
    """Test WITH, OPTIONAL MATCH, CASE, and query chaining."""

    DB = "test_clauses"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_with_clause_chaining(self):
        records = list(self.s.run("WITH 1 AS x RETURN x + 1 AS y"))
        assert records[0]["y"] == 2

    def test_with_multi_step_chaining(self):
        records = list(self.s.run(
            "WITH 10 AS x WITH x * 2 AS y RETURN y"
        ))
        assert records[0]["y"] == 20

    def test_with_where(self):
        records = list(self.s.run(
            "UNWIND range(1, 5) AS x WITH x WHERE x > 3 RETURN x ORDER BY x"
        ))
        assert [r["x"] for r in records] == [4, 5]

    def test_optional_match_returns_null(self):
        records = list(self.s.run("OPTIONAL MATCH (n:Missing) RETURN n"))
        assert len(records) == 1
        assert records[0]["n"] is None

    def test_optional_match_is_null_check(self):
        records = list(self.s.run(
            "OPTIONAL MATCH (n:NoSuchLabel) RETURN n IS NULL AS r"
        ))
        assert records[0]["r"] is True

    def test_case_generic(self):
        records = list(self.s.run(
            "RETURN CASE WHEN true THEN 1 ELSE 0 END AS r"
        ))
        assert records[0]["r"] == 1

    def test_case_generic_false(self):
        records = list(self.s.run(
            "RETURN CASE WHEN false THEN 1 ELSE 0 END AS r"
        ))
        assert records[0]["r"] == 0

    def test_case_value_comparison(self):
        records = list(self.s.run(
            "RETURN CASE 2 WHEN 1 THEN 'one' WHEN 2 THEN 'two' ELSE 'other' END AS r"
        ))
        assert records[0]["r"] == "two"

    def test_case_default(self):
        records = list(self.s.run(
            "RETURN CASE 99 WHEN 1 THEN 'one' ELSE 'other' END AS r"
        ))
        assert records[0]["r"] == "other"


class TestCypherAggregation:
    """Test aggregation functions: count, sum, avg, min, max, collect."""

    DB = "test_agg"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_count(self):
        records = list(self.s.run(
            "UNWIND [1, 2, 3] AS x RETURN count(x) AS c"
        ))
        assert records[0]["c"] == 3

    def test_count_star(self):
        records = list(self.s.run(
            "UNWIND [1, 2, 3] AS x RETURN count(*) AS c"
        ))
        assert records[0]["c"] == 3

    def test_sum(self):
        records = list(self.s.run(
            "UNWIND range(1, 5) AS x RETURN sum(x) AS s"
        ))
        assert records[0]["s"] == 15

    def test_avg(self):
        records = list(self.s.run(
            "UNWIND [1, 2, 3, 4, 5] AS x RETURN avg(x) AS a"
        ))
        assert records[0]["a"] == 3.0

    def test_min(self):
        records = list(self.s.run(
            "UNWIND [3, 1, 4, 1, 5] AS x RETURN min(x) AS m"
        ))
        assert records[0]["m"] == 1

    def test_max(self):
        records = list(self.s.run(
            "UNWIND [3, 1, 4, 1, 5] AS x RETURN max(x) AS m"
        ))
        assert records[0]["m"] == 5

    def test_collect(self):
        records = list(self.s.run(
            "UNWIND [1, 2, 3] AS x RETURN collect(x) AS c"
        ))
        assert records[0]["c"] == [1, 2, 3]

    def test_aggregation_on_created_nodes(self):
        self.s.run("UNWIND range(1, 5) AS i CREATE (:AggNode {val: i})").consume()
        records = list(self.s.run(
            "MATCH (n:AggNode) RETURN count(n) AS c, sum(n.val) AS s"
        ))
        assert records[0]["c"] == 5
        assert records[0]["s"] == 15


# ---------------------------------------------------------------------------
# Functions
# ---------------------------------------------------------------------------

class TestStringFunctions:
    """Test string functions: toUpper, toLower, trim, substring, size."""

    DB = "test_strfn"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_toUpper(self):
        records = list(self.s.run("RETURN toUpper('hello') AS r"))
        assert records[0]["r"] == "HELLO"

    def test_toLower(self):
        records = list(self.s.run("RETURN toLower('HELLO') AS r"))
        assert records[0]["r"] == "hello"

    def test_trim(self):
        records = list(self.s.run("RETURN trim('  hello  ') AS r"))
        assert records[0]["r"] == "hello"

    def test_substring_with_length(self):
        records = list(self.s.run("RETURN substring('hello', 1, 3) AS r"))
        assert records[0]["r"] == "ell"

    def test_substring_from_start(self):
        records = list(self.s.run("RETURN substring('hello', 0, 2) AS r"))
        assert records[0]["r"] == "he"

    def test_size_of_string(self):
        records = list(self.s.run("RETURN size('hello') AS r"))
        assert records[0]["r"] == 5

    def test_size_of_list(self):
        records = list(self.s.run("RETURN size([1, 2, 3]) AS r"))
        assert records[0]["r"] == 3

    def test_size_of_empty_list(self):
        records = list(self.s.run("RETURN size([]) AS r"))
        assert records[0]["r"] == 0

    def test_size_of_empty_string(self):
        records = list(self.s.run("RETURN size('') AS r"))
        assert records[0]["r"] == 0


class TestMathFunctions:
    """Test math functions: abs, ceil, floor, round."""

    DB = "test_mathfn"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_abs_negative(self):
        records = list(self.s.run("RETURN abs(-5) AS r"))
        assert records[0]["r"] == 5

    def test_abs_positive(self):
        records = list(self.s.run("RETURN abs(5) AS r"))
        assert records[0]["r"] == 5

    def test_abs_float(self):
        records = list(self.s.run("RETURN abs(-3.14) AS r"))
        assert abs(records[0]["r"] - 3.14) < 0.001

    def test_ceil(self):
        records = list(self.s.run("RETURN ceil(2.3) AS r"))
        assert records[0]["r"] == 3.0

    def test_ceil_negative(self):
        records = list(self.s.run("RETURN ceil(-2.3) AS r"))
        assert records[0]["r"] == -2.0

    def test_floor(self):
        records = list(self.s.run("RETURN floor(2.9) AS r"))
        assert records[0]["r"] == 2.0

    def test_floor_negative(self):
        records = list(self.s.run("RETURN floor(-2.1) AS r"))
        assert records[0]["r"] == -3.0

    def test_round(self):
        records = list(self.s.run("RETURN round(2.5) AS r"))
        assert records[0]["r"] == 3.0

    def test_round_down(self):
        records = list(self.s.run("RETURN round(2.3) AS r"))
        assert records[0]["r"] == 2.0


class TestTypeFunctions:
    """Test type conversion functions: toInteger, toFloat, toString."""

    DB = "test_typefn"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_toInteger_from_string(self):
        records = list(self.s.run("RETURN toInteger('42') AS r"))
        assert records[0]["r"] == 42

    def test_toInteger_from_float(self):
        records = list(self.s.run("RETURN toInteger(3.9) AS r"))
        assert records[0]["r"] == 3

    def test_toFloat_from_int(self):
        records = list(self.s.run("RETURN toFloat(5) AS r"))
        assert records[0]["r"] == 5.0

    def test_toFloat_from_string(self):
        records = list(self.s.run("RETURN toFloat('3.14') AS r"))
        assert abs(records[0]["r"] - 3.14) < 0.001

    def test_toString_from_int(self):
        records = list(self.s.run("RETURN toString(42) AS r"))
        assert records[0]["r"] == "42"

    def test_toString_from_bool(self):
        records = list(self.s.run("RETURN toString(true) AS r"))
        assert records[0]["r"] == "true"

    def test_toString_from_float(self):
        records = list(self.s.run("RETURN toString(3.14) AS r"))
        assert "3.14" in records[0]["r"]


class TestListComprehensions:
    """Test list comprehensions."""

    DB = "test_listcomp"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_list_comp_with_filter_and_map(self):
        records = list(self.s.run(
            "RETURN [x IN range(1, 5) WHERE x > 2 | x * 2] AS r"
        ))
        assert records[0]["r"] == [6, 8, 10]

    def test_list_comp_identity(self):
        records = list(self.s.run(
            "RETURN [x IN [1, 2, 3] | x] AS r"
        ))
        assert records[0]["r"] == [1, 2, 3]

    def test_list_comp_transform(self):
        records = list(self.s.run(
            "RETURN [x IN [1, 2, 3] | x * x] AS r"
        ))
        assert records[0]["r"] == [1, 4, 9]


# ---------------------------------------------------------------------------
# Phase 7: Error handling (expanded)
# ---------------------------------------------------------------------------

class TestErrorHandling:
    """Test various error conditions and recovery."""

    DB = "test_errors"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_syntax_error(self):
        with pytest.raises(CypherSyntaxError):
            result = self.s.run("THIS IS NOT CYPHER")
            result.consume()

    def test_syntax_error_bare_return(self):
        with pytest.raises(CypherSyntaxError):
            result = self.s.run("RETURN")
            result.consume()

    def test_syntax_error_unclosed_string(self):
        with pytest.raises(CypherSyntaxError):
            result = self.s.run("RETURN 'unclosed")
            result.consume()

    def test_division_by_zero(self):
        with pytest.raises(DatabaseError):
            result = self.s.run("RETURN 1/0 AS r")
            result.consume()

    def test_recovery_after_syntax_error(self):
        try:
            self.s.run("INVALID QUERY").consume()
        except Exception:
            pass
        records = list(self.s.run("RETURN 42 AS answer"))
        assert records[0]["answer"] == 42

    def test_recovery_after_division_by_zero(self):
        try:
            self.s.run("RETURN 1/0 AS r").consume()
        except Exception:
            pass
        records = list(self.s.run("RETURN 99 AS answer"))
        assert records[0]["answer"] == 99

    def test_recovery_after_multiple_errors(self):
        for _ in range(3):
            try:
                self.s.run("INVALID QUERY").consume()
            except Exception:
                pass
        records = list(self.s.run("RETURN 1 AS n"))
        assert records[0]["n"] == 1

    def test_very_long_query_string(self):
        long_str = "a" * 5000
        records = list(self.s.run(f"RETURN '{long_str}' AS r"))
        assert len(records[0]["r"]) == 5000


# ---------------------------------------------------------------------------
# Connection handling
# ---------------------------------------------------------------------------

class TestConnectionHandling:
    """Test multiple sessions, query patterns, and driver behavior."""

    DB = "test_conn"

    def test_multiple_sequential_sessions(self, driver):
        """Multiple sessions on the same driver work correctly."""
        with driver.session(database=self.DB) as s1:
            records = list(s1.run("RETURN 1 AS n"))
            assert records[0]["n"] == 1

        with driver.session(database=self.DB) as s2:
            records = list(s2.run("RETURN 2 AS n"))
            assert records[0]["n"] == 2

    def test_multiple_queries_same_session(self, driver):
        """Multiple queries on the same session maintain state."""
        with driver.session(database="test_conn_multi") as s:
            s.run("CREATE (:MQ {n: 1})").consume()
            s.run("CREATE (:MQ {n: 2})").consume()
            records = list(s.run("MATCH (q:MQ) RETURN q.n AS n ORDER BY n"))
            values = [r["n"] for r in records]
            assert 1 in values
            assert 2 in values

    def test_consume_result(self, driver):
        """Consuming results returns a valid summary."""
        with driver.session(database=self.DB) as s:
            result = s.run("RETURN 1 AS n")
            summary = result.consume()
            assert summary.result_available_after is not None
            assert summary.result_consumed_after is not None

    def test_result_summary_counters(self, driver):
        """Summary counters track mutations."""
        with driver.session(database="test_conn_counters") as s:
            result = s.run("CREATE (:Counter {x: 1})-[:R {w: 2}]->(:Counter)")
            summary = result.consume()
            c = summary.counters
            assert c.nodes_created == 2
            assert c.relationships_created == 1
            assert c.properties_set >= 2

    def test_execute_read(self, driver):
        """session.execute_read works with transaction functions."""
        with driver.session(database=self.DB) as s:
            def read_fn(tx):
                return list(tx.run("RETURN 42 AS n"))
            result = s.execute_read(read_fn)
            assert result[0]["n"] == 42

    def test_execute_write(self, driver):
        """session.execute_write works with transaction functions."""
        with driver.session(database="test_conn_txwrite") as s:
            def write_fn(tx):
                tx.run("CREATE (:TXNode {val: 1})")
                return list(tx.run("MATCH (n:TXNode) RETURN n.val AS v"))
            result = s.execute_write(write_fn)
            assert result[0]["v"] == 1


# ---------------------------------------------------------------------------
# Edge cases
# ---------------------------------------------------------------------------

class TestEdgeCases:
    """Test edge cases: empty strings, unicode, large numbers, null handling."""

    DB = "test_edge"

    @pytest.fixture(autouse=True)
    def session(self, driver):
        with _session(driver, self.DB) as s:
            self.s = s
            yield

    def test_empty_string_property(self):
        self.s.run("CREATE (:ES {val: ''})").consume()
        records = list(self.s.run("MATCH (n:ES) RETURN n.val AS v"))
        assert records[0]["v"] == ""

    def test_empty_string_literal(self):
        records = list(self.s.run("RETURN '' AS r"))
        assert records[0]["r"] == ""

    def test_unicode_accented_chars(self):
        records = list(self.s.run(u"RETURN '\u00e9\u00e0\u00fc\u00f1\u00e7' AS r"))
        assert records[0]["r"] == u"\u00e9\u00e0\u00fc\u00f1\u00e7"

    def test_unicode_cjk(self):
        records = list(self.s.run(u"RETURN '\u4f60\u597d\u4e16\u754c' AS r"))
        assert records[0]["r"] == u"\u4f60\u597d\u4e16\u754c"

    def test_unicode_in_property(self):
        self.s.run(u"CREATE (:UProp {name: '\u00fcber'})").consume()
        records = list(self.s.run(
            u"MATCH (n:UProp {name: '\u00fcber'}) RETURN n.name AS name"
        ))
        assert records[0]["name"] == u"\u00fcber"

    def test_large_integer_max(self):
        records = list(self.s.run("RETURN 9223372036854775807 AS r"))
        assert records[0]["r"] == 9223372036854775807

    def test_negative_integer(self):
        records = list(self.s.run("RETURN -999999 AS r"))
        assert records[0]["r"] == -999999

    def test_negative_float(self):
        records = list(self.s.run("RETURN -3.14 AS r"))
        assert abs(records[0]["r"] - (-3.14)) < 0.001

    def test_float_precision(self):
        records = list(self.s.run("RETURN 0.1 + 0.2 AS r"))
        # Float arithmetic imprecision is expected
        assert abs(records[0]["r"] - 0.3) < 0.0001

    def test_null_in_list(self):
        records = list(self.s.run("RETURN [1, null, 3] AS r"))
        assert records[0]["r"] == [1, None, 3]

    def test_null_in_map(self):
        records = list(self.s.run("RETURN {a: 1, b: null} AS r"))
        r = records[0]["r"]
        assert r["a"] == 1
        assert r["b"] is None

    def test_list_of_maps(self):
        records = list(self.s.run(
            "RETURN [{name: 'a', val: 1}, {name: 'b', val: 2}] AS r"
        ))
        r = records[0]["r"]
        assert len(r) == 2
        assert r[0]["name"] == "a"
        assert r[1]["val"] == 2

    def test_map_with_list_value(self):
        records = list(self.s.run("RETURN {items: [1, 2, 3]} AS r"))
        assert records[0]["r"]["items"] == [1, 2, 3]

    def test_zero_integer(self):
        records = list(self.s.run("RETURN 0 AS r"))
        assert records[0]["r"] == 0

    def test_zero_float(self):
        records = list(self.s.run("RETURN 0.0 AS r"))
        assert records[0]["r"] == 0.0

    def test_empty_list(self):
        records = list(self.s.run("RETURN [] AS r"))
        assert records[0]["r"] == []

    def test_empty_map(self):
        records = list(self.s.run("RETURN {} AS r"))
        assert records[0]["r"] == {}

    def test_nested_empty_structures(self):
        records = list(self.s.run("RETURN {a: [], b: {}} AS r"))
        r = records[0]["r"]
        assert r["a"] == []
        assert r["b"] == {}
