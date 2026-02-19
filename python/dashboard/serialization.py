"""Shared serialization helpers for dashboard API responses."""

from __future__ import annotations

import datetime
import json
from decimal import Decimal


def serialize_row(row: dict, *, parse_json: bool = False) -> dict:
    """Convert a DB row dict to JSON-safe types."""
    out = {}
    for k, v in row.items():
        if isinstance(v, Decimal):
            out[k] = str(v)
        elif isinstance(v, (datetime.datetime, datetime.date, datetime.time)):
            out[k] = v.isoformat()
        elif parse_json and isinstance(v, str):
            try:
                parsed = json.loads(v)
                if isinstance(parsed, (dict, list)):
                    out[k] = parsed
                    continue
            except (json.JSONDecodeError, TypeError):
                pass
            out[k] = v
        else:
            out[k] = v
    return out
