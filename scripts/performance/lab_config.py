"""Read the shared lab constants from ``lab.json``.

The same file is consumed by ``LabCommon.psm1`` (PowerShell) and by shell
scripts through the command line interface of this module::

    python3 lab_config.py get network.receiver_ip

Resolution order for the file: the ``ETW_DNSTAP_LAB_CONFIG`` environment
variable, then ``lab.json`` next to this module.  Paths such as
``target/lab-ubuntu/ssh_config`` are relative to the repository root, which is
two levels above this directory (see ``repo_root``).
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from functools import lru_cache
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent


def repo_root() -> Path:
    """Return the repository root (``scripts/performance/../..``)."""
    return HERE.parents[1]


def config_path() -> Path:
    override = os.environ.get("ETW_DNSTAP_LAB_CONFIG")
    return Path(override).resolve() if override else HERE / "lab.json"


@lru_cache(maxsize=None)
def load(path: str | None = None) -> dict[str, Any]:
    """Load and return the parsed lab configuration."""
    target = Path(path) if path else config_path()
    if not target.is_file():
        raise SystemExit(f"Lab configuration not found: {target}")
    return json.loads(target.read_text(encoding="utf-8-sig"))


def get(dotted: str, default: Any = ...) -> Any:
    """Return ``section.key`` from the configuration."""
    node: Any = load()
    for part in dotted.split("."):
        if not isinstance(node, dict) or part not in node:
            if default is ...:
                raise KeyError(f"Unknown lab setting: {dotted}")
            return default
        node = node[part]
    return node


def repo_path(dotted: str) -> Path:
    """Return a repository-relative setting as an absolute ``Path``."""
    return (repo_root() / str(get(dotted))).resolve()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    getter = sub.add_parser("get", help="print one setting (e.g. network.receiver_ip)")
    getter.add_argument("key")
    args = parser.parse_args(argv)
    try:
        value = get(args.key)
    except KeyError as error:
        print(error, file=sys.stderr)
        return 2
    print(value if isinstance(value, str) else json.dumps(value))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
