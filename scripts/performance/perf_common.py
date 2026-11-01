"""Helpers shared by the summarize-*/build-*/export-* analysis scripts.

These were previously copy-pasted into each script: UTF-8(-BOM) JSON loading, streaming
SHA-256, WPA process-name normalization, finite number parsing and simple distribution /
top-N summaries.  The scripts import this module by name, so keep it next to them.
"""
from __future__ import annotations

import hashlib
import json
import math
import re
from collections import Counter
from pathlib import Path
from typing import Any, Iterable

_PROCESS_PID = re.compile(r"\(\s*(\d+)\)")


def read_json(path: Path) -> Any:
    """Load JSON written by PowerShell 5.1 or 7 (with or without a UTF-8 BOM)."""
    return json.loads(Path(path).read_text(encoding="utf-8-sig"))


def write_json(path: Path, value: Any) -> None:
    """Write strict (no NaN) indented JSON as UTF-8 with a trailing newline."""
    Path(path).write_text(json.dumps(value, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def sha256_file(path: Path, *, upper: bool = False, block_size: int = 1024 * 1024) -> str:
    """Streaming SHA-256 of a file (does not load it into memory)."""
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(block_size), b""):
            digest.update(block)
    return digest.hexdigest().upper() if upper else digest.hexdigest()


def normalize_process(value: str) -> str:
    """Normalize WPA process labels such as ``dns.exe ( 1234)`` to ``dns.exe (1234)``."""
    return _PROCESS_PID.sub(r"(\1)", value.strip())


def finite_number(value: str) -> float:
    """Parse an exported number (thousands separators allowed); reject NaN/inf."""
    result = float(value.replace(",", ""))
    if not math.isfinite(result):
        raise ValueError("nonfinite exported number")
    return result


def distribution(values: Iterable[float]) -> dict[str, Any]:
    """count/sum/max and p50/p95/p99 (nearest rank) of a sample."""
    ordered = sorted(values)
    return dict(count=len(ordered), sum=sum(ordered), max=max(ordered, default=None),
                **{f"p{p}": ordered[math.ceil(len(ordered) * p / 100) - 1] if ordered else None
                   for p in (50, 95, 99)})


def top(counter: Counter, limit: int = 12) -> list[dict[str, Any]]:
    return [dict(value=value, weight=weight) for value, weight in counter.most_common(limit)]
