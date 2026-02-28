"""Tagged-value decoding and graph-specific types (Point, Path)."""

from __future__ import annotations

import datetime
import re
from collections import namedtuple
from typing import Any, Dict, List, Union

Point = namedtuple("Point", ["x", "y", "crs"])
"""A geographic or Cartesian point with *x*, *y* coordinates and a *crs* string."""

Path = namedtuple("Path", ["nodes", "edges"])
"""A graph path consisting of node IDs and edge IDs (both as lists of ints)."""


def _parse_timezone_offset(s: str) -> datetime.timezone:
    """Parse ``+HH:MM`` or ``-HH:MM`` into a ``datetime.timezone``.

    Args:
        s: A string like ``+01:00`` or ``-05:30``.

    Returns:
        A ``datetime.timezone`` with the corresponding offset.
    """
    match = re.match(r"([+-])(\d{2}):(\d{2})", s)
    if not match:
        return datetime.timezone.utc
    sign = 1 if match.group(1) == "+" else -1
    hours = int(match.group(2))
    minutes = int(match.group(3))
    delta = datetime.timedelta(hours=hours, minutes=minutes) * sign
    return datetime.timezone(delta)


def _decode_tagged_string(s: str) -> Any:
    """Decode a tagged string value from the SedanDelivery wire format.

    Prefixes handled:
    - ``__str:``           -> plain string (un-escapes ``__``-prefixed strings)
    - ``__date:``          -> datetime.date
    - ``__localtime:``     -> datetime.time
    - ``__localdatetime:`` -> datetime.datetime (no timezone)
    - ``__datetime:``      -> datetime.datetime (with timezone)
    - ``__duration:``      -> raw string (Python has no ISO 8601 duration type)
    - ``__point:``         -> Point namedtuple
    - ``__node:``          -> int (node ID)
    - ``__edge:``          -> int (edge ID)

    Args:
        s: The tagged string.

    Returns:
        The decoded Python value, or the original string if no tag matches.
    """
    if s.startswith("__str:"):
        return s[6:]  # strip "__str:" prefix

    if s.startswith("__date:"):
        return datetime.date.fromisoformat(s[7:])

    if s.startswith("__localtime:"):
        return datetime.time.fromisoformat(s[12:])

    if s.startswith("__localdatetime:"):
        return datetime.datetime.fromisoformat(s[16:])

    if s.startswith("__datetime:"):
        raw = s[11:]
        # Strip optional IANA tz suffix like [Europe/Berlin]
        bracket = raw.find("[")
        if bracket != -1:
            raw = raw[:bracket]
        return datetime.datetime.fromisoformat(raw)

    if s.startswith("__duration:"):
        # Python stdlib has no ISO 8601 duration type; return the raw string.
        return s[11:]

    if s.startswith("__point:"):
        parts = s[8:].split(",", 2)
        if len(parts) == 3:
            x = float(parts[0])
            y = float(parts[1])
            crs = parts[2]
            return Point(x=x, y=y, crs=crs)
        return s  # malformed, return as-is

    if s.startswith("__node:"):
        return int(s[7:])

    if s.startswith("__edge:"):
        return int(s[7:])

    # Not a tagged string -- return as-is.
    return s


def decode_value(v: Any) -> Any:
    """Recursively decode a JSON-deserialized value from the wire format.

    Handles tagged strings, path objects, nested lists, and nested dicts.

    Args:
        v: A value from ``json.loads`` (None, bool, int, float, str, list, dict).

    Returns:
        The decoded Python value with tagged strings resolved.
    """
    if v is None:
        return None

    if isinstance(v, bool):
        return v

    if isinstance(v, (int, float)):
        return v

    if isinstance(v, str):
        if v.startswith("__"):
            return _decode_tagged_string(v)
        return v

    if isinstance(v, list):
        return [decode_value(item) for item in v]

    if isinstance(v, dict):
        # Check for encoded Path first.
        if "__path_nodes" in v and "__path_edges" in v:
            nodes = [int(n) for n in v["__path_nodes"]]
            edges = [int(e) for e in v["__path_edges"]]
            return Path(nodes=nodes, edges=edges)
        return {k: decode_value(val) for k, val in v.items()}

    return v
