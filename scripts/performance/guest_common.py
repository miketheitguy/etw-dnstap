"""Shared helpers for the guest-side (Ubuntu) lab scripts that touch block devices.

Used by ``prepare-ubuntu-data-disk.py`` and ``unmount-ubuntu-data-disk.py``.  Everything
here is read-only except ``utc_now`` helpers; the destructive steps stay in the callers
so that the dry-run/``--apply`` split is visible in one place per script.
"""
from __future__ import annotations

import datetime
import json
import os
import re
import stat
import subprocess
import sys
from pathlib import Path
from typing import Any

HERE = Path(__file__).resolve().parent
if str(HERE) not in sys.path:  # allow ``python3 -I script.py`` as well as plain invocation
    sys.path.insert(0, str(HERE))

CAMPAIGN_ID = re.compile(r"[A-Za-z0-9_-]{1,64}")


def utc_now() -> str:
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def run(*args: str) -> str:
    """Run a command and return stripped stdout; raises CalledProcessError on failure."""
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout.strip()


def is_active(unit: str) -> str:
    """Return ``systemctl is-active`` output (the non-zero exit code is expected for inactive units)."""
    result = subprocess.run(["systemctl", "is-active", unit], capture_output=True, text=True, check=False)
    return result.stdout.strip()


def require_root() -> None:
    if os.geteuid() != 0:
        raise SystemExit("Run as root (sudo): block-device inspection needs root even in dry-run mode.")


def require_campaign_id(value: str) -> str:
    if not CAMPAIGN_ID.fullmatch(value):
        raise SystemExit("--campaign-id must be 1-64 letters, digits, '_' or '-'")
    return value


def lsblk(*paths: str) -> list[dict[str, Any]]:
    out = run("lsblk", "-J", "-b", "-o", "NAME,KNAME,PATH,SIZE,TYPE,FSTYPE,LABEL,MOUNTPOINTS,SERIAL,WWN,UUID,HCTL,MAJ:MIN,PKNAME", *paths)
    return json.loads(out)["blockdevices"]


def walk(devices: list[dict[str, Any]]):
    for device in devices:
        yield device
        yield from walk(device.get("children") or [])


def root_disk() -> str:
    """Return the /dev path of the whole disk that holds the root filesystem."""
    source = run("findmnt", "-n", "-o", "SOURCE", "--target", "/")
    resolved = str(Path(source).resolve())
    for disk in lsblk():
        if any(str(Path(node["path"]).resolve()) == resolved for node in walk([disk])):
            return str(Path(disk["path"]).resolve())
    raise SystemExit(f"Cannot identify the root disk for {source}; refusing to continue")


def require_block_device(path: Path) -> Path:
    resolved = path.resolve()
    if not resolved.exists() or not stat.S_ISBLK(resolved.stat().st_mode):
        raise SystemExit(f"{path} is not a block device")
    return resolved


def mounted_paths(device: dict[str, Any]) -> list[str]:
    return [m for node in walk([device]) for m in (node.get("mountpoints") or []) if m]
