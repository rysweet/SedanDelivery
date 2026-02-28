"""Query result types -- QueryResult, Record, and MutationStats."""

from __future__ import annotations

import re
from typing import Any, Dict, Iterator, List, Optional, Sequence


class MutationStats:
    """Statistics returned by a GQL.MUTATE command.

    Attributes:
        nodes_created: Number of nodes created.
        nodes_deleted: Number of nodes deleted.
        edges_created: Number of edges created.
        edges_deleted: Number of edges deleted.
        properties_set: Number of properties set.
        labels_added: Number of labels added.
    """

    __slots__ = (
        "nodes_created",
        "nodes_deleted",
        "edges_created",
        "edges_deleted",
        "properties_set",
        "labels_added",
    )

    def __init__(
        self,
        nodes_created: int = 0,
        nodes_deleted: int = 0,
        edges_created: int = 0,
        edges_deleted: int = 0,
        properties_set: int = 0,
        labels_added: int = 0,
    ) -> None:
        self.nodes_created = nodes_created
        self.nodes_deleted = nodes_deleted
        self.edges_created = edges_created
        self.edges_deleted = edges_deleted
        self.properties_set = properties_set
        self.labels_added = labels_added

    @classmethod
    def parse(cls, s: str) -> "MutationStats":
        """Parse a stats string from the GQL.MUTATE wire format.

        Expected format::

            nodes_created=1 nodes_deleted=0 edges_created=2 ...

        Args:
            s: The raw stats string.

        Returns:
            A populated MutationStats instance.
        """
        kwargs: Dict[str, int] = {}
        for match in re.finditer(r"(\w+)=(\d+)", s):
            key = match.group(1)
            val = int(match.group(2))
            if key in cls.__slots__:
                kwargs[key] = val
        return cls(**kwargs)

    def __repr__(self) -> str:
        parts = [f"{k}={getattr(self, k)}" for k in self.__slots__]
        return f"MutationStats({', '.join(parts)})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, MutationStats):
            return NotImplemented
        return all(
            getattr(self, k) == getattr(other, k) for k in self.__slots__
        )


class Record:
    """A single result row, providing dict-like access to column values.

    Usage::

        record["name"]          # KeyError if missing
        record.get("name")      # None if missing
        record.data()           # full dict
        record.keys()           # column names
    """

    __slots__ = ("_data",)

    def __init__(self, data: Dict[str, Any]) -> None:
        self._data = data

    def __getitem__(self, key: str) -> Any:
        return self._data[key]

    def get(self, key: str, default: Any = None) -> Any:
        """Return the value for *key*, or *default* if the key is absent."""
        return self._data.get(key, default)

    def data(self) -> Dict[str, Any]:
        """Return the underlying dict for this record."""
        return dict(self._data)

    def keys(self) -> List[str]:
        """Return the column names in this record."""
        return list(self._data.keys())

    def __repr__(self) -> str:
        return f"Record({self._data!r})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Record):
            return NotImplemented
        return self._data == other._data


class QueryResult:
    """Result of a GQL.QUERY or GQL.MUTATE command.

    Iterable over :class:`Record` objects and supports index access and
    ``len()``.  The :attr:`stats` property provides mutation statistics.
    """

    __slots__ = ("_records", "_stats")

    def __init__(
        self,
        records: Sequence[Record],
        stats: Optional[MutationStats] = None,
    ) -> None:
        self._records = list(records)
        self._stats = stats if stats is not None else MutationStats()

    @property
    def stats(self) -> MutationStats:
        """Mutation statistics for this result."""
        return self._stats

    def __len__(self) -> int:
        return len(self._records)

    def __getitem__(self, index: int) -> Record:
        return self._records[index]

    def __iter__(self) -> Iterator[Record]:
        return iter(self._records)

    def __repr__(self) -> str:
        return f"QueryResult(rows={len(self._records)}, stats={self._stats!r})"
