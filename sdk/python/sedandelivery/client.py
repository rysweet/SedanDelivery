"""SedanDB client -- top-level connection wrapper around redis-py."""

from __future__ import annotations

from typing import List, Optional

import redis

from .graph import Graph


class SedanDB:
    """Connection to a SedanDelivery server (backed by redis-py).

    Usage::

        db = SedanDB(host="localhost", port=6379)
        g = db.select_graph("social")
        result = g.query("MATCH (n) RETURN n.name")
        db.close()

    Or as a context manager::

        with SedanDB() as db:
            g = db.select_graph("social")
            ...
    """

    def __init__(
        self,
        host: str = "localhost",
        port: int = 6379,
        password: Optional[str] = None,
        **redis_kwargs,
    ) -> None:
        self._redis = redis.Redis(
            host=host, port=port, password=password, decode_responses=True, **redis_kwargs
        )

    @classmethod
    def from_url(cls, url: str) -> "SedanDB":
        """Create a SedanDB instance from a Redis URL.

        Args:
            url: Redis connection URL, e.g. ``redis://localhost:6379/0``.

        Returns:
            A new SedanDB instance.
        """
        instance = cls.__new__(cls)
        instance._redis = redis.Redis.from_url(url, decode_responses=True)
        return instance

    def select_graph(self, name: str) -> Graph:
        """Return a Graph handle for the named graph.

        Args:
            name: Graph name (must already exist, or call ``graph.create()``).

        Returns:
            A Graph instance bound to *name*.
        """
        return Graph(self._redis, name)

    def list_graphs(self) -> List[str]:
        """Return the names of all graphs on the server.

        Returns:
            A list of graph name strings.
        """
        result = self._redis.execute_command("GQL.LIST-GRAPHS")
        if result is None:
            return []
        return list(result)

    def close(self) -> None:
        """Close the underlying Redis connection."""
        self._redis.close()

    # -- context manager --

    def __enter__(self) -> "SedanDB":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()
