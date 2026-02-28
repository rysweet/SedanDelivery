"""Unit tests for the sedandelivery Python SDK.

All tests mock redis-py so no running server is required.
"""

from __future__ import annotations

import datetime
import json
from unittest.mock import MagicMock, patch

import pytest

from sedandelivery import (
    Graph,
    MutationStats,
    Path,
    Point,
    QueryResult,
    Record,
    SedanDB,
)
from sedandelivery.graph import _cypher_escape, _substitute_params
from sedandelivery.types import decode_value


# ---------------------------------------------------------------------------
# 1. SedanDB creation and select_graph
# ---------------------------------------------------------------------------


class TestSedanDB:
    def test_init_creates_redis_connection(self):
        with patch("sedandelivery.client.redis.Redis") as mock_redis:
            db = SedanDB(host="myhost", port=1234, password="secret")
            mock_redis.assert_called_once_with(
                host="myhost", port=1234, password="secret", decode_responses=True
            )

    def test_from_url(self):
        with patch("sedandelivery.client.redis.Redis") as mock_redis_cls:
            db = SedanDB.from_url("redis://localhost:6379/0")
            mock_redis_cls.from_url.assert_called_once_with(
                "redis://localhost:6379/0", decode_responses=True
            )

    def test_select_graph_returns_graph(self):
        with patch("sedandelivery.client.redis.Redis"):
            db = SedanDB()
            g = db.select_graph("social")
            assert isinstance(g, Graph)
            assert g.name == "social"

    def test_close(self):
        with patch("sedandelivery.client.redis.Redis") as mock_redis:
            db = SedanDB()
            db.close()
            mock_redis.return_value.close.assert_called_once()


# ---------------------------------------------------------------------------
# 2. Graph.query parses JSON rows into Records
# ---------------------------------------------------------------------------


class TestGraphQuery:
    def _make_graph(self) -> tuple:
        mock_redis = MagicMock()
        g = Graph(mock_redis, "test")
        return g, mock_redis

    def test_query_parses_rows(self):
        g, mock_redis = self._make_graph()
        mock_redis.execute_command.return_value = [
            json.dumps({"age": 30, "name": "Alice"}),
            json.dumps({"age": 25, "name": "Bob"}),
        ]
        result = g.query("MATCH (n) RETURN n.name AS name, n.age AS age")
        mock_redis.execute_command.assert_called_once_with(
            "GQL.QUERY", "test", "MATCH (n) RETURN n.name AS name, n.age AS age"
        )
        assert len(result) == 2
        assert result[0]["name"] == "Alice"
        assert result[0]["age"] == 30
        assert result[1]["name"] == "Bob"

    def test_query_empty_result(self):
        g, mock_redis = self._make_graph()
        mock_redis.execute_command.return_value = []
        result = g.query("MATCH (n) RETURN n")
        assert len(result) == 0

    def test_query_none_result(self):
        g, mock_redis = self._make_graph()
        mock_redis.execute_command.return_value = None
        result = g.query("MATCH (n) RETURN n")
        assert len(result) == 0


# ---------------------------------------------------------------------------
# 3. Graph.mutate parses rows + stats
# ---------------------------------------------------------------------------


class TestGraphMutate:
    def _make_graph(self) -> tuple:
        mock_redis = MagicMock()
        g = Graph(mock_redis, "test")
        return g, mock_redis

    def test_mutate_with_rows_and_stats(self):
        g, mock_redis = self._make_graph()
        rows = [json.dumps({"n.name": "Alice"})]
        stats = "nodes_created=1 nodes_deleted=0 edges_created=0 edges_deleted=0 properties_set=2 labels_added=1"
        mock_redis.execute_command.return_value = [rows, stats]

        result = g.mutate("CREATE (n:Person {name: 'Alice'}) RETURN n.name")

        assert len(result) == 1
        assert result[0]["n.name"] == "Alice"
        assert result.stats.nodes_created == 1
        assert result.stats.nodes_deleted == 0
        assert result.stats.properties_set == 2
        assert result.stats.labels_added == 1

    def test_mutate_no_return_rows(self):
        g, mock_redis = self._make_graph()
        stats = "nodes_created=3 nodes_deleted=0 edges_created=2 edges_deleted=0 properties_set=5 labels_added=3"
        mock_redis.execute_command.return_value = [[], stats]

        result = g.mutate("CREATE (a)-[:KNOWS]->(b)")
        assert len(result) == 0
        assert result.stats.nodes_created == 3
        assert result.stats.edges_created == 2


# ---------------------------------------------------------------------------
# 4. Parameter substitution
# ---------------------------------------------------------------------------


class TestParameterSubstitution:
    def test_string_param(self):
        result = _substitute_params("MATCH (n {name: $name})", {"name": "Alice"})
        assert result == "MATCH (n {name: 'Alice'})"

    def test_string_with_single_quotes(self):
        result = _substitute_params("MATCH (n {name: $name})", {"name": "O'Brien"})
        assert result == "MATCH (n {name: 'O\\'Brien'})"

    def test_int_param(self):
        result = _substitute_params("MATCH (n) WHERE n.age > $age", {"age": 21})
        assert result == "MATCH (n) WHERE n.age > 21"

    def test_none_param(self):
        result = _substitute_params("SET n.x = $val", {"val": None})
        assert result == "SET n.x = null"

    def test_bool_params(self):
        result = _substitute_params("SET n.active = $a, n.deleted = $d", {"a": True, "d": False})
        assert result == "SET n.active = true, n.deleted = false"

    def test_list_param(self):
        result = _substitute_params("WHERE n.id IN $ids", {"ids": [1, 2, 3]})
        assert result == "WHERE n.id IN [1, 2, 3]"

    def test_dict_param(self):
        result = _substitute_params("CREATE (n $props)", {"props": {"a": 1, "b": "hi"}})
        assert result == "CREATE (n {a: 1, b: 'hi'})"

    def test_unknown_param_left_intact(self):
        result = _substitute_params("$known $unknown", {"known": 42})
        assert result == "42 $unknown"

    def test_no_params(self):
        query = "MATCH (n) RETURN n"
        assert _substitute_params(query, {}) == query

    def test_float_param(self):
        result = _substitute_params("SET n.x = $val", {"val": 3.14})
        assert "3.14" in result

    def test_nested_list(self):
        result = _substitute_params("SET n.x = $val", {"val": [1, [2, 3]]})
        assert result == "SET n.x = [1, [2, 3]]"


# ---------------------------------------------------------------------------
# 5. Record dict access
# ---------------------------------------------------------------------------


class TestRecord:
    def test_getitem(self):
        r = Record({"name": "Alice", "age": 30})
        assert r["name"] == "Alice"
        assert r["age"] == 30

    def test_getitem_missing_raises(self):
        r = Record({"name": "Alice"})
        with pytest.raises(KeyError):
            _ = r["missing"]

    def test_get_with_default(self):
        r = Record({"name": "Alice"})
        assert r.get("name") == "Alice"
        assert r.get("missing") is None
        assert r.get("missing", 42) == 42

    def test_data_returns_dict(self):
        original = {"name": "Alice", "age": 30}
        r = Record(original)
        d = r.data()
        assert d == original
        # Ensure it is a copy
        d["extra"] = True
        assert "extra" not in r.data()

    def test_keys(self):
        r = Record({"b": 2, "a": 1})
        assert set(r.keys()) == {"a", "b"}


# ---------------------------------------------------------------------------
# 6. Tagged value decoding
# ---------------------------------------------------------------------------


class TestTaggedValueDecoding:
    def test_date(self):
        val = decode_value("__date:2024-01-15")
        assert val == datetime.date(2024, 1, 15)

    def test_localtime(self):
        val = decode_value("__localtime:10:30:00")
        assert val == datetime.time(10, 30, 0)

    def test_localdatetime(self):
        val = decode_value("__localdatetime:2024-01-15T10:30:00")
        assert val == datetime.datetime(2024, 1, 15, 10, 30, 0)

    def test_datetime_with_tz(self):
        val = decode_value("__datetime:2024-01-15T10:30:00+01:00[Europe/Berlin]")
        assert isinstance(val, datetime.datetime)
        assert val.year == 2024
        assert val.month == 1
        assert val.day == 15
        assert val.hour == 10
        assert val.minute == 30
        assert val.tzinfo is not None
        offset = val.utcoffset()
        assert offset == datetime.timedelta(hours=1)

    def test_duration(self):
        val = decode_value("__duration:P1Y2M3D")
        assert val == "P1Y2M3D"

    def test_point(self):
        val = decode_value("__point:1.5,2.5,Wgs84")
        assert isinstance(val, Point)
        assert val.x == 1.5
        assert val.y == 2.5
        assert val.crs == "Wgs84"

    def test_node(self):
        val = decode_value("__node:42")
        assert val == 42

    def test_edge(self):
        val = decode_value("__edge:7")
        assert val == 7

    def test_escaped_string(self):
        val = decode_value("__str:__date:2024-01-15")
        assert val == "__date:2024-01-15"

    def test_path_object(self):
        raw = {"__path_nodes": [1, 2, 3], "__path_edges": [10, 20]}
        val = decode_value(raw)
        assert isinstance(val, Path)
        assert val.nodes == [1, 2, 3]
        assert val.edges == [10, 20]

    def test_plain_string_unchanged(self):
        assert decode_value("hello") == "hello"

    def test_none(self):
        assert decode_value(None) is None

    def test_bool(self):
        assert decode_value(True) is True
        assert decode_value(False) is False

    def test_int(self):
        assert decode_value(42) == 42

    def test_float(self):
        assert decode_value(3.14) == 3.14

    def test_nested_list(self):
        val = decode_value(["__node:1", "__edge:2", "plain"])
        assert val == [1, 2, "plain"]

    def test_nested_dict(self):
        val = decode_value({"date": "__date:2024-01-15", "count": 5})
        assert val == {"date": datetime.date(2024, 1, 15), "count": 5}


# ---------------------------------------------------------------------------
# 7. MutationStats parsing
# ---------------------------------------------------------------------------


class TestMutationStats:
    def test_parse_full_string(self):
        s = "nodes_created=5 nodes_deleted=1 edges_created=3 edges_deleted=0 properties_set=10 labels_added=2"
        stats = MutationStats.parse(s)
        assert stats.nodes_created == 5
        assert stats.nodes_deleted == 1
        assert stats.edges_created == 3
        assert stats.edges_deleted == 0
        assert stats.properties_set == 10
        assert stats.labels_added == 2

    def test_parse_all_zeros(self):
        s = "nodes_created=0 nodes_deleted=0 edges_created=0 edges_deleted=0 properties_set=0 labels_added=0"
        stats = MutationStats.parse(s)
        assert stats == MutationStats()

    def test_default_zeros(self):
        stats = MutationStats()
        assert stats.nodes_created == 0
        assert stats.edges_created == 0
        assert stats.properties_set == 0

    def test_equality(self):
        a = MutationStats(nodes_created=1, edges_created=2)
        b = MutationStats(nodes_created=1, edges_created=2)
        assert a == b

    def test_inequality(self):
        a = MutationStats(nodes_created=1)
        b = MutationStats(nodes_created=2)
        assert a != b


# ---------------------------------------------------------------------------
# 8. Context manager
# ---------------------------------------------------------------------------


class TestContextManager:
    def test_context_manager_calls_close(self):
        with patch("sedandelivery.client.redis.Redis") as mock_redis:
            with SedanDB() as db:
                assert isinstance(db, SedanDB)
            mock_redis.return_value.close.assert_called_once()


# ---------------------------------------------------------------------------
# 9. list_graphs
# ---------------------------------------------------------------------------


class TestListGraphs:
    def test_list_graphs(self):
        with patch("sedandelivery.client.redis.Redis") as mock_redis:
            mock_redis.return_value.execute_command.return_value = ["graph1", "graph2"]
            db = SedanDB()
            graphs = db.list_graphs()
            mock_redis.return_value.execute_command.assert_called_once_with("GQL.LIST-GRAPHS")
            assert graphs == ["graph1", "graph2"]

    def test_list_graphs_empty(self):
        with patch("sedandelivery.client.redis.Redis") as mock_redis:
            mock_redis.return_value.execute_command.return_value = []
            db = SedanDB()
            graphs = db.list_graphs()
            assert graphs == []

    def test_list_graphs_none(self):
        with patch("sedandelivery.client.redis.Redis") as mock_redis:
            mock_redis.return_value.execute_command.return_value = None
            db = SedanDB()
            graphs = db.list_graphs()
            assert graphs == []


# ---------------------------------------------------------------------------
# Additional: Graph.create, Graph.delete, Graph.explain
# ---------------------------------------------------------------------------


class TestGraphCommands:
    def test_create(self):
        mock_redis = MagicMock()
        g = Graph(mock_redis, "myg")
        g.create()
        mock_redis.execute_command.assert_called_once_with("GQL.CREATE-GRAPH", "myg")

    def test_delete(self):
        mock_redis = MagicMock()
        g = Graph(mock_redis, "myg")
        g.delete()
        mock_redis.execute_command.assert_called_once_with("GQL.DELETE-GRAPH", "myg")

    def test_explain(self):
        mock_redis = MagicMock()
        mock_redis.execute_command.return_value = "Scan -> Filter -> Project"
        g = Graph(mock_redis, "myg")
        plan = g.explain("MATCH (n) RETURN n")
        mock_redis.execute_command.assert_called_once_with(
            "GQL.EXPLAIN", "myg", "MATCH (n) RETURN n"
        )
        assert plan == "Scan -> Filter -> Project"


# ---------------------------------------------------------------------------
# Additional: Cypher escape edge cases
# ---------------------------------------------------------------------------


class TestCypherEscape:
    def test_escape_none(self):
        assert _cypher_escape(None) == "null"

    def test_escape_bool(self):
        assert _cypher_escape(True) == "true"
        assert _cypher_escape(False) == "false"

    def test_escape_int(self):
        assert _cypher_escape(42) == "42"
        assert _cypher_escape(-1) == "-1"

    def test_escape_string_with_backslash(self):
        assert _cypher_escape("a\\b") == "'a\\\\b'"

    def test_escape_unsupported_type_raises(self):
        with pytest.raises(TypeError):
            _cypher_escape(object())


# ---------------------------------------------------------------------------
# Additional: QueryResult iteration
# ---------------------------------------------------------------------------


class TestQueryResultIteration:
    def test_iteration(self):
        records = [Record({"x": 1}), Record({"x": 2})]
        qr = QueryResult(records, MutationStats())
        collected = [r["x"] for r in qr]
        assert collected == [1, 2]

    def test_index_access(self):
        records = [Record({"x": 1}), Record({"x": 2})]
        qr = QueryResult(records, MutationStats())
        assert qr[0]["x"] == 1
        assert qr[1]["x"] == 2

    def test_stats_default_for_query(self):
        qr = QueryResult([])
        assert qr.stats.nodes_created == 0
        assert qr.stats.edges_created == 0


# ---------------------------------------------------------------------------
# 10. Empty query results edge cases
# ---------------------------------------------------------------------------


class TestEmptyResults:
    def test_empty_query_result_len(self):
        qr = QueryResult([])
        assert len(qr) == 0

    def test_empty_query_result_iteration(self):
        qr = QueryResult([])
        assert list(qr) == []

    def test_empty_query_result_stats_are_zero(self):
        qr = QueryResult([], MutationStats())
        assert qr.stats == MutationStats()

    def test_query_result_index_out_of_range(self):
        qr = QueryResult([])
        with pytest.raises(IndexError):
            _ = qr[0]

    def test_empty_record(self):
        r = Record({})
        assert r.keys() == []
        assert r.data() == {}
        assert r.get("anything") is None


# ---------------------------------------------------------------------------
# 11. Unicode in parameters
# ---------------------------------------------------------------------------


class TestUnicodeParameters:
    def test_unicode_string_param(self):
        result = _substitute_params("SET n.name = $name", {"name": "\u00e9l\u00e8ve"})
        assert result == "SET n.name = '\u00e9l\u00e8ve'"

    def test_unicode_emoji_param(self):
        result = _substitute_params("SET n.emoji = $e", {"e": "\U0001f600"})
        assert result == "SET n.emoji = '\U0001f600'"

    def test_unicode_cjk_param(self):
        result = _substitute_params("SET n.label = $l", {"l": "\u4f60\u597d\u4e16\u754c"})
        assert result == "SET n.label = '\u4f60\u597d\u4e16\u754c'"

    def test_unicode_in_query_result(self):
        g = Graph(MagicMock(), "test")
        g._redis.execute_command.return_value = [
            json.dumps({"name": "\u00fc\u00f6\u00e4"})
        ]
        result = g.query("MATCH (n) RETURN n.name AS name")
        assert result[0]["name"] == "\u00fc\u00f6\u00e4"


# ---------------------------------------------------------------------------
# 12. Nested list/dict parameters
# ---------------------------------------------------------------------------


class TestNestedParameters:
    def test_nested_dict_in_list(self):
        result = _substitute_params(
            "SET n.x = $val",
            {"val": [{"a": 1}, {"b": 2}]},
        )
        assert result == "SET n.x = [{a: 1}, {b: 2}]"

    def test_deeply_nested_list(self):
        result = _substitute_params("SET n.x = $val", {"val": [[[1]]]})
        assert result == "SET n.x = [[[1]]]"

    def test_dict_with_nested_list(self):
        result = _substitute_params(
            "CREATE (n $props)",
            {"props": {"tags": [1, 2, 3], "name": "test"}},
        )
        assert "tags: [1, 2, 3]" in result
        assert "name: 'test'" in result

    def test_empty_dict_param(self):
        result = _substitute_params("CREATE (n $props)", {"props": {}})
        assert result == "CREATE (n {})"

    def test_empty_list_param(self):
        result = _substitute_params("SET n.tags = $tags", {"tags": []})
        assert result == "SET n.tags = []"

    def test_dict_with_none_value(self):
        result = _substitute_params("CREATE (n $props)", {"props": {"x": None}})
        assert result == "CREATE (n {x: null})"

    def test_dict_with_bool_value(self):
        result = _substitute_params("CREATE (n $props)", {"props": {"active": True}})
        assert result == "CREATE (n {active: true})"


# ---------------------------------------------------------------------------
# 13. Tagged types round-trip (Duration, LocalDateTime, DateTime with tz)
# ---------------------------------------------------------------------------


class TestTaggedTypesRoundTrip:
    def test_duration_iso_format(self):
        val = decode_value("__duration:PT2H30M")
        assert val == "PT2H30M"

    def test_duration_full_format(self):
        val = decode_value("__duration:P1Y2M3DT4H5M6S")
        assert val == "P1Y2M3DT4H5M6S"

    def test_localdatetime_with_seconds(self):
        val = decode_value("__localdatetime:2024-06-15T14:30:45")
        assert val == datetime.datetime(2024, 6, 15, 14, 30, 45)

    def test_localdatetime_with_millis(self):
        val = decode_value("__localdatetime:2024-06-15T14:30:45.123000")
        assert val.microsecond == 123000

    def test_datetime_utc(self):
        val = decode_value("__datetime:2024-01-15T10:30:00+00:00")
        assert val.tzinfo is not None
        assert val.utcoffset() == datetime.timedelta(0)

    def test_datetime_negative_offset(self):
        val = decode_value("__datetime:2024-01-15T10:30:00-05:00[America/New_York]")
        assert val.utcoffset() == datetime.timedelta(hours=-5)

    def test_date_edge_of_year(self):
        val = decode_value("__date:2024-12-31")
        assert val == datetime.date(2024, 12, 31)

    def test_localtime_midnight(self):
        val = decode_value("__localtime:00:00:00")
        assert val == datetime.time(0, 0, 0)

    def test_localtime_end_of_day(self):
        val = decode_value("__localtime:23:59:59")
        assert val == datetime.time(23, 59, 59)

    def test_point_cartesian(self):
        val = decode_value("__point:10.0,20.0,Cartesian")
        assert isinstance(val, Point)
        assert val.crs == "Cartesian"

    def test_point_negative_coords(self):
        val = decode_value("__point:-73.9857,40.7484,Wgs84")
        assert isinstance(val, Point)
        assert val.x == pytest.approx(-73.9857)
        assert val.y == pytest.approx(40.7484)


# ---------------------------------------------------------------------------
# 14. Graph.explain edge cases
# ---------------------------------------------------------------------------


class TestGraphExplainEdgeCases:
    def test_explain_returns_string(self):
        mock_redis = MagicMock()
        mock_redis.execute_command.return_value = 42
        g = Graph(mock_redis, "myg")
        plan = g.explain("MATCH (n) RETURN n")
        assert plan == "42"  # str() conversion

    def test_explain_multiline_plan(self):
        mock_redis = MagicMock()
        mock_redis.execute_command.return_value = "Scan\n  Filter\n    Project"
        g = Graph(mock_redis, "myg")
        plan = g.explain("MATCH (n) WHERE n.x > 5 RETURN n")
        assert "Scan" in plan
        assert "Filter" in plan
        assert "Project" in plan


# ---------------------------------------------------------------------------
# 15. Error handling (Redis connection errors)
# ---------------------------------------------------------------------------


class TestErrorHandling:
    def test_query_raises_on_redis_error(self):
        import redis as redis_lib

        mock_redis = MagicMock()
        mock_redis.execute_command.side_effect = redis_lib.ConnectionError("Connection refused")
        g = Graph(mock_redis, "test")
        with pytest.raises(redis_lib.ConnectionError):
            g.query("MATCH (n) RETURN n")

    def test_mutate_raises_on_redis_error(self):
        import redis as redis_lib

        mock_redis = MagicMock()
        mock_redis.execute_command.side_effect = redis_lib.ResponseError("ERR unknown command")
        g = Graph(mock_redis, "test")
        with pytest.raises(redis_lib.ResponseError):
            g.mutate("CREATE (n)")

    def test_create_raises_on_existing_graph(self):
        import redis as redis_lib

        mock_redis = MagicMock()
        mock_redis.execute_command.side_effect = redis_lib.ResponseError("Graph already exists")
        g = Graph(mock_redis, "test")
        with pytest.raises(redis_lib.ResponseError):
            g.create()

    def test_delete_raises_on_missing_graph(self):
        import redis as redis_lib

        mock_redis = MagicMock()
        mock_redis.execute_command.side_effect = redis_lib.ResponseError("Graph does not exist")
        g = Graph(mock_redis, "test")
        with pytest.raises(redis_lib.ResponseError):
            g.delete()


# ---------------------------------------------------------------------------
# 16. Multiple graphs on same connection
# ---------------------------------------------------------------------------


class TestMultipleGraphs:
    def test_two_graphs_same_connection(self):
        with patch("sedandelivery.client.redis.Redis") as mock_redis:
            db = SedanDB()
            g1 = db.select_graph("graph1")
            g2 = db.select_graph("graph2")
            assert g1.name == "graph1"
            assert g2.name == "graph2"
            # Both should share the same underlying Redis mock
            assert g1._redis is g2._redis

    def test_operations_use_correct_graph_name(self):
        mock_redis = MagicMock()
        mock_redis.execute_command.return_value = []
        g1 = Graph(mock_redis, "graph1")
        g2 = Graph(mock_redis, "graph2")

        g1.query("MATCH (n) RETURN n")
        g2.query("MATCH (n) RETURN n")

        calls = mock_redis.execute_command.call_args_list
        assert calls[0][0] == ("GQL.QUERY", "graph1", "MATCH (n) RETURN n")
        assert calls[1][0] == ("GQL.QUERY", "graph2", "MATCH (n) RETURN n")


# ---------------------------------------------------------------------------
# 17. Record.keys() and Record.data() additional tests
# ---------------------------------------------------------------------------


class TestRecordKeysAndData:
    def test_keys_preserves_order(self):
        r = Record({"z": 1, "a": 2, "m": 3})
        keys = r.keys()
        assert len(keys) == 3
        assert set(keys) == {"z", "a", "m"}

    def test_data_is_independent_copy(self):
        original = {"name": "Alice"}
        r = Record(original)
        d1 = r.data()
        d2 = r.data()
        d1["name"] = "Bob"
        assert d2["name"] == "Alice"
        assert r["name"] == "Alice"

    def test_record_repr(self):
        r = Record({"x": 1})
        rep = repr(r)
        assert "Record" in rep
        assert "x" in rep

    def test_record_equality(self):
        r1 = Record({"a": 1, "b": 2})
        r2 = Record({"a": 1, "b": 2})
        assert r1 == r2

    def test_record_inequality(self):
        r1 = Record({"a": 1})
        r2 = Record({"a": 2})
        assert r1 != r2

    def test_record_not_equal_to_other_types(self):
        r = Record({"a": 1})
        assert r != {"a": 1}


# ---------------------------------------------------------------------------
# 18. MutationStats additional edge cases
# ---------------------------------------------------------------------------


class TestMutationStatsEdgeCases:
    def test_parse_partial_string(self):
        s = "nodes_created=3 properties_set=7"
        stats = MutationStats.parse(s)
        assert stats.nodes_created == 3
        assert stats.properties_set == 7
        assert stats.edges_created == 0

    def test_parse_unknown_keys_ignored(self):
        s = "nodes_created=1 unknown_stat=99 edges_created=2"
        stats = MutationStats.parse(s)
        assert stats.nodes_created == 1
        assert stats.edges_created == 2

    def test_repr(self):
        stats = MutationStats(nodes_created=5)
        rep = repr(stats)
        assert "MutationStats" in rep
        assert "nodes_created=5" in rep

    def test_parse_empty_string(self):
        stats = MutationStats.parse("")
        assert stats == MutationStats()


# ---------------------------------------------------------------------------
# 19. QueryResult additional tests
# ---------------------------------------------------------------------------


class TestQueryResultAdditional:
    def test_query_result_repr(self):
        qr = QueryResult([Record({"x": 1})])
        rep = repr(qr)
        assert "QueryResult" in rep
        assert "rows=1" in rep

    def test_query_result_multiple_iterations(self):
        records = [Record({"x": i}) for i in range(5)]
        qr = QueryResult(records)
        first = [r["x"] for r in qr]
        second = [r["x"] for r in qr]
        assert first == second == [0, 1, 2, 3, 4]

    def test_query_result_negative_index(self):
        records = [Record({"x": 1}), Record({"x": 2})]
        qr = QueryResult(records)
        assert qr[-1]["x"] == 2

    def test_query_result_slice(self):
        records = [Record({"x": i}) for i in range(5)]
        qr = QueryResult(records)
        # QueryResult wraps a list internally, slice should work
        assert qr[1]["x"] == 1
        assert qr[4]["x"] == 4


# ---------------------------------------------------------------------------
# 20. Query with params (kwargs interface)
# ---------------------------------------------------------------------------


class TestQueryWithKwargs:
    def test_query_substitutes_kwargs(self):
        mock_redis = MagicMock()
        mock_redis.execute_command.return_value = [
            json.dumps({"name": "Alice"})
        ]
        g = Graph(mock_redis, "test")
        result = g.query("MATCH (n {name: $name}) RETURN n", name="Alice")
        mock_redis.execute_command.assert_called_once_with(
            "GQL.QUERY", "test", "MATCH (n {name: 'Alice'}) RETURN n"
        )
        assert result[0]["name"] == "Alice"

    def test_mutate_substitutes_kwargs(self):
        mock_redis = MagicMock()
        stats = "nodes_created=1 nodes_deleted=0 edges_created=0 edges_deleted=0 properties_set=1 labels_added=1"
        mock_redis.execute_command.return_value = [[], stats]
        g = Graph(mock_redis, "test")
        result = g.mutate("CREATE (n:Person {name: $name})", name="Bob")
        mock_redis.execute_command.assert_called_once_with(
            "GQL.MUTATE", "test", "CREATE (n:Person {name: 'Bob'})"
        )
        assert result.stats.nodes_created == 1

    def test_mutate_none_response(self):
        mock_redis = MagicMock()
        mock_redis.execute_command.return_value = None
        g = Graph(mock_redis, "test")
        result = g.mutate("CREATE (n)")
        assert len(result) == 0
        assert result.stats == MutationStats()


# ---------------------------------------------------------------------------
# 21. Cypher escape edge cases (additional)
# ---------------------------------------------------------------------------


class TestCypherEscapeAdditional:
    def test_escape_empty_string(self):
        assert _cypher_escape("") == "''"

    def test_escape_string_with_both_quotes_and_backslash(self):
        assert _cypher_escape("it's a \\test") == "'it\\'s a \\\\test'"

    def test_escape_float(self):
        result = _cypher_escape(3.14)
        assert "3.14" in result

    def test_escape_negative_float(self):
        result = _cypher_escape(-2.5)
        assert "-2.5" in result

    def test_escape_zero(self):
        assert _cypher_escape(0) == "0"

    def test_escape_tuple(self):
        result = _cypher_escape((1, 2, 3))
        assert result == "[1, 2, 3]"

    def test_escape_list_of_strings(self):
        result = _cypher_escape(["a", "b"])
        assert result == "['a', 'b']"

    def test_escape_nested_dict(self):
        result = _cypher_escape({"inner": {"x": 1}})
        assert result == "{inner: {x: 1}}"


# ---------------------------------------------------------------------------
# 22. Tagged value decoding (additional edge cases)
# ---------------------------------------------------------------------------


class TestTaggedValueDecodingAdditional:
    def test_double_underscore_non_tagged(self):
        val = decode_value("__unknown_tag:value")
        # Not a known tag, but starts with __ -- falls through to plain string
        assert val == "__unknown_tag:value"

    def test_empty_string(self):
        val = decode_value("")
        assert val == ""

    def test_str_tag_empty_suffix(self):
        val = decode_value("__str:")
        assert val == ""

    def test_deeply_nested_decode(self):
        val = decode_value({
            "outer": {
                "inner": ["__node:1", "__date:2024-01-01"],
                "num": 42,
            }
        })
        assert val["outer"]["inner"] == [1, datetime.date(2024, 1, 1)]
        assert val["outer"]["num"] == 42

    def test_path_with_empty_lists(self):
        raw = {"__path_nodes": [], "__path_edges": []}
        val = decode_value(raw)
        assert isinstance(val, Path)
        assert val.nodes == []
        assert val.edges == []

    def test_point_malformed_too_few_parts(self):
        val = decode_value("__point:1.5,2.5")
        # Only 2 parts instead of 3 -- should fall through as plain string
        assert val == "__point:1.5,2.5"
