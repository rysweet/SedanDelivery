"""Graph handle -- sends GQL commands and parses wire-format responses."""

from __future__ import annotations

import json
import re
from typing import Any, Dict

import redis

from .result import MutationStats, QueryResult, Record
from .types import decode_value


# Regex that matches a Cypher parameter reference ``$name``.
# Matches ``$`` followed by an identifier (letters, digits, underscore).
_PARAM_RE = re.compile(r"\$([A-Za-z_]\w*)")


def _cypher_escape(value: Any) -> str:
    """Escape a Python value into a Cypher literal string.

    Supports None, bool, int, float, str, list, and dict.

    Args:
        value: The Python value to escape.

    Returns:
        A string suitable for interpolation into a Cypher query.

    Raises:
        TypeError: If the value type is not supported.
    """
    if value is None:
        return "null"
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return repr(value)
    if isinstance(value, str):
        escaped = value.replace("\\", "\\\\").replace("'", "\\'")
        return f"'{escaped}'"
    if isinstance(value, (list, tuple)):
        items = ", ".join(_cypher_escape(v) for v in value)
        return f"[{items}]"
    if isinstance(value, dict):
        entries = ", ".join(
            f"{k}: {_cypher_escape(v)}" for k, v in value.items()
        )
        return "{" + entries + "}"
    raise TypeError(f"Cannot escape type {type(value).__name__} for Cypher")


def _substitute_params(cypher: str, params: Dict[str, Any]) -> str:
    """Replace ``$key`` placeholders in *cypher* with escaped parameter values.

    Only keys present in *params* are substituted; unknown ``$`` references
    are left as-is (so the server can report the error).

    Args:
        cypher: The Cypher query string containing ``$key`` placeholders.
        params: Mapping of parameter names to Python values.

    Returns:
        The query string with parameters substituted inline.
    """
    if not params:
        return cypher

    def _replacer(match: re.Match) -> str:
        name = match.group(1)
        if name in params:
            return _cypher_escape(params[name])
        return match.group(0)  # leave unknown params untouched

    return _PARAM_RE.sub(_replacer, cypher)


def _parse_query_response(raw_rows) -> QueryResult:
    """Parse the RESP Array returned by GQL.QUERY into a QueryResult.

    Each element of *raw_rows* is a JSON string representing one row.

    Args:
        raw_rows: The list of JSON-encoded row strings from Redis.

    Returns:
        A QueryResult with decoded records and zero-valued stats.
    """
    records = []
    for raw in raw_rows:
        row_dict = json.loads(raw)
        decoded = {k: decode_value(v) for k, v in row_dict.items()}
        records.append(Record(decoded))
    return QueryResult(records, MutationStats())


def _parse_mutate_response(raw) -> QueryResult:
    """Parse the RESP Array returned by GQL.MUTATE into a QueryResult.

    Wire format: ``[Array-of-JSON-rows, stats-string]``.

    Args:
        raw: The two-element list ``[rows_array, stats_string]`` from Redis.

    Returns:
        A QueryResult with decoded records and parsed mutation stats.
    """
    raw_rows, stats_str = raw
    records = []
    if raw_rows:
        for row_raw in raw_rows:
            row_dict = json.loads(row_raw)
            decoded = {k: decode_value(v) for k, v in row_dict.items()}
            records.append(Record(decoded))
    stats = MutationStats.parse(stats_str)
    return QueryResult(records, stats)


class Graph:
    """Handle to a single named graph on a SedanDelivery server.

    Obtained via ``SedanDB.select_graph(name)``.  All Cypher execution goes
    through :meth:`query` (reads) and :meth:`mutate` (writes).
    """

    def __init__(self, client: redis.Redis, name: str) -> None:
        self._redis = client
        self.name = name

    def query(self, cypher: str, **params) -> QueryResult:
        """Execute a read-only Cypher query.

        Parameters in the query string (``$name``) are replaced with
        Cypher-escaped values from *params*.

        Args:
            cypher: Cypher query string.
            **params: Named parameter values.

        Returns:
            A QueryResult containing decoded records.
        """
        cypher = _substitute_params(cypher, params)
        raw = self._redis.execute_command("GQL.QUERY", self.name, cypher)
        if raw is None:
            return QueryResult([], MutationStats())
        return _parse_query_response(raw)

    def mutate(self, cypher: str, **params) -> QueryResult:
        """Execute a mutating Cypher query.

        Args:
            cypher: Cypher query string (CREATE, SET, DELETE, etc.).
            **params: Named parameter values.

        Returns:
            A QueryResult containing decoded records and mutation stats.
        """
        cypher = _substitute_params(cypher, params)
        raw = self._redis.execute_command("GQL.MUTATE", self.name, cypher)
        if raw is None:
            return QueryResult([], MutationStats())
        return _parse_mutate_response(raw)

    def create(self) -> None:
        """Create this graph on the server.

        Raises:
            redis.ResponseError: If the graph already exists.
        """
        self._redis.execute_command("GQL.CREATE-GRAPH", self.name)

    def delete(self) -> None:
        """Delete this graph from the server.

        Raises:
            redis.ResponseError: If the graph does not exist.
        """
        self._redis.execute_command("GQL.DELETE-GRAPH", self.name)

    def explain(self, cypher: str) -> str:
        """Return the physical execution plan for a Cypher query.

        Args:
            cypher: Cypher query string.

        Returns:
            A human-readable plan string.
        """
        result = self._redis.execute_command("GQL.EXPLAIN", self.name, cypher)
        return str(result)
