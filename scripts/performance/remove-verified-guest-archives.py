"""Remove only this campaign's guest archive duplicates after host verification (DESTRUCTIVE).

Copy the existing host verification and exact record-count manifests into the archive base
as RUN-host-verification.json and RUN-archive-counts.json first.  The default is a
read-only DRY RUN that verifies everything and prints what would be deleted; add
``--apply`` to delete.  Historical campaigns and protected backups are never traversed.

Example::

    DNSTAP_PERF_BASE=/home/dnstaplab/performance-my-campaign \\
        python3 remove-verified-guest-archives.py --campaign-id my-campaign pooled-run-1
    ... --apply
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import re
from pathlib import Path
from typing import Any

import lab_config

RUN_ID = re.compile(r"pooled-[A-Za-z0-9_-]{1,64}")
CAMPAIGN_ID = re.compile(r"[A-Za-z0-9_-]{1,64}")


def sha256_file(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def plan_run(base: Path, run: str) -> tuple[Path, Path, dict[str, Any]]:
    if not RUN_ID.fullmatch(run):
        raise SystemExit("Unsafe or unrelated run ID")
    archive = base / f"{run}-performance.jsonl.gz"
    receipt_path = base / f"{run}-guest-archive-removal.json"
    if receipt_path.exists() or archive.is_symlink() or archive.resolve().parent != base:
        raise SystemExit("Prior removal receipt, symlink, or unexpected archive path")
    source = json.loads((base / f"{run}-archive-source.json").read_text())
    host = json.loads((base / f"{run}-host-verification.json").read_text())
    counts_path = base / f"{run}-archive-counts.json"
    counts = json.loads(counts_path.read_text())
    if host.get("gzip_integrity_verified") is not True or host.get("source_hashes_match") is not True:
        raise SystemExit("Host CRC/hash verification is not explicit")
    if any(host.get(key) != source.get(key) for key in ("raw_sha256", "compressed_sha256", "raw_bytes", "compressed_bytes")):
        raise SystemExit("Host and source archive proofs disagree")
    if (host["archive"].replace("\\", "/").rsplit("/", 1)[-1] != archive.name
            or counts["archive"].replace("\\", "/").rsplit("/", 1)[-1] != archive.name
            or type(counts.get("all_json_records")) is not int or counts["all_json_records"] < 0):
        raise SystemExit("Host archive/count manifest does not identify this run")
    digest = sha256_file(archive)
    if digest != host["compressed_sha256"] or archive.stat().st_size != host["compressed_bytes"]:
        raise SystemExit("Guest duplicate changed after host verification")
    return archive, receipt_path, {
        "run": run, "guest_archive_path": str(archive), "compressed_sha256": digest,
        "compressed_bytes": archive.stat().st_size, "retained_host_archive": host["archive"],
        "host_raw_sha256": host["raw_sha256"], "host_gzip_crc_and_hash_verified": True,
        "host_exact_json_records": counts["all_json_records"],
        "host_count_manifest_sha256": hashlib.sha256(counts_path.read_bytes()).hexdigest(),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                     formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__.split("\n", 1)[1])
    parser.add_argument("runs", nargs="+", help="run IDs (pooled-...) whose guest duplicate should be removed")
    parser.add_argument("--campaign-id", required=True, help="campaign whose archive base (performance-<id>) is authorized")
    parser.add_argument("--apply", action="store_true", help="actually delete the guest duplicates (default: dry run)")
    parser.add_argument("--delete", action="store_true", dest="legacy_delete", help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.legacy_delete:
        parser.error("--delete was renamed to --apply")
    if not CAMPAIGN_ID.fullmatch(args.campaign_id):
        parser.error("--campaign-id must be 1-64 letters, digits, '_' or '-'")
    base = Path(os.environ["DNSTAP_PERF_BASE"]).resolve()
    home = Path(lab_config.get("ubuntu_vm.home"))
    if base.parent != home or base.name != f"performance-{args.campaign_id}":
        raise SystemExit(f"Removal is restricted to {home}/performance-{args.campaign_id}")
    plans = [plan_run(base, run) for run in args.runs]
    for archive, receipt_path, receipt in plans:
        if args.apply:
            archive.unlink()
            receipt["removed_utc"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
            with receipt_path.open("x") as out:
                out.write(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps({"delete": args.apply, **receipt}))
    if not args.apply:
        print("DRY RUN: nothing was deleted. Re-run with --apply to remove the verified duplicates.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
