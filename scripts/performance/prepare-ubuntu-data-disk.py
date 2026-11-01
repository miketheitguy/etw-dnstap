"""Initialize the unique, blank temporary data disk attached for one campaign (DESTRUCTIVE).

Formats the disk with ext4, mounts it, and points the dnscollector log at it.  The default
is a DRY RUN that performs every identity check and prints the plan without changing
anything.  Add ``--apply`` to format.  Run as root on the Ubuntu guest.

Safety checks (all must pass before anything is written):
  * the guest hostname matches lab.json;
  * dnscollector and the performance monitor are stopped;
  * ``--device`` is a block device that is a whole disk, not the root disk, has no
    partitions, no filesystem, no mountpoints and is not mounted anywhere;
  * its size, serial and/or WWN match ``--expected-size-gib`` and ``--expected-serial`` /
    ``--expected-wwn`` (at least one identity is mandatory) and any existing label is
    absent or equal to ``--label``;
  * the host attachment receipt (temporary-data-disk-host.json) names the approved VM
    GUID, VHDX and SCSI slot;
  * ``wipefs --no-act`` finds no signature.  A disk that has signatures is refused unless
    ``--wipe-existing-signatures`` is given explicitly.

Example (dry run, then apply)::

    sudo python3 prepare-ubuntu-data-disk.py --campaign-id my-campaign \\
        --device /dev/disk/by-id/wwn-0x... --expected-wwn 0x... --mount /mnt/dnstap-my-campaign
    sudo python3 prepare-ubuntu-data-disk.py ... --apply
"""
from __future__ import annotations

import argparse
import difflib
import hashlib
import json
import os
import subprocess
from pathlib import Path
from typing import Any

import guest_common as common
import lab_config


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                     formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__.split("\n", 1)[1])
    parser.add_argument("--campaign-id", required=True, help="campaign identifier (names the archive base, VHDX and log)")
    parser.add_argument("--device", required=True, type=Path, help="explicit device path, e.g. /dev/disk/by-id/wwn-0x...")
    parser.add_argument("--mount", required=True, type=Path, help="explicit mountpoint to create (must not exist)")
    parser.add_argument("--base", type=Path, help="archive/receipt directory (default: <home>/performance-<campaign-id>)")
    parser.add_argument("--expected-size-gib", type=int, default=64)
    parser.add_argument("--expected-serial", help="expected device serial (SERIAL column of lsblk)")
    parser.add_argument("--expected-wwn", help="expected device WWN (WWN column of lsblk)")
    parser.add_argument("--label", default="dnstap-pooled-data", help="ext4 label to create")
    parser.add_argument("--wipe-existing-signatures", action="store_true",
                        help="DANGEROUS: allow wipefs --all on a disk that already has signatures")
    parser.add_argument("--apply", action="store_true", help="actually format and mount (default: dry run)")
    args = parser.parse_args(argv)
    common.require_campaign_id(args.campaign_id)
    if not (args.expected_serial or args.expected_wwn):
        parser.error("at least one of --expected-serial / --expected-wwn is required")
    return args


def check_disk(args: argparse.Namespace, base: Path) -> tuple[dict[str, Any], Path]:
    """Validate the target disk; returns (lsblk entry, resolved device path). Read-only."""
    device = common.require_block_device(args.device)
    entries = common.lsblk(str(device))
    if len(entries) != 1 or entries[0]["type"] != "disk":
        raise SystemExit("--device must be a whole disk (not a partition, LVM or loop device)")
    disk = entries[0]
    if str(device) == common.root_disk():
        raise SystemExit("Refusing: the device is the root disk")
    if disk.get("children") or disk.get("fstype") or common.mounted_paths(disk):
        raise SystemExit("Refusing: the device has partitions, a filesystem or a mountpoint")
    if str(device) in common.run("findmnt", "-rn", "-o", "SOURCE").split():
        raise SystemExit("Refusing: the device is mounted")
    if int(disk["size"]) != args.expected_size_gib * 1024**3:
        raise SystemExit(f"Size mismatch: {disk['size']} bytes, expected {args.expected_size_gib} GiB")
    if args.expected_serial and disk.get("serial") != args.expected_serial:
        raise SystemExit("Serial mismatch")
    if args.expected_wwn and disk.get("wwn") != args.expected_wwn:
        raise SystemExit("WWN mismatch")
    if disk.get("label") not in (None, "", args.label):
        raise SystemExit(f"Existing label {disk.get('label')!r} differs from --label {args.label!r}")
    before_path = base / "storage-block-before.json"
    if before_path.exists():
        before = json.loads(before_path.read_text(encoding="utf-8-sig"))
        known = {d["path"] for d in before["blockdevices"] if d["type"] == "disk"}
        if disk["path"] in known:
            raise SystemExit("The device was present before the new disk was attached; it is not the new disk")
    signature = common.run("wipefs", "--no-act", str(device))
    if signature and not args.wipe_existing_signatures:
        raise SystemExit("The disk has existing signatures; refusing to format (use --wipe-existing-signatures to override explicitly)")
    return disk, device


def check_host_receipt(args: argparse.Namespace, base: Path, disk: dict[str, Any]) -> dict[str, Any]:
    ubuntu = lab_config.get("ubuntu_vm")
    host = json.loads((base / "temporary-data-disk-host.json").read_text(encoding="utf-8-sig"))
    vhdx = f"{ubuntu['vhdx_dir']}/{args.campaign_id}-data.vhdx".replace("/", "\\")
    if (host["vm_guid"] != ubuntu["id"] or host["virtual_size_bytes"] != args.expected_size_gib * 1024**3
            or host["controller_location"] != 2 or host["controller_number"] != 0 or host["vhdx_path"] != vhdx):
        raise SystemExit("Host attachment receipt does not match the approved disk")
    return host


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    common.require_root()
    ubuntu = lab_config.get("ubuntu_vm")
    if common.run("hostname") != ubuntu["hostname"]:
        raise SystemExit("Wrong guest")
    for unit in ("dnscollector", "dnstap-performance-monitor"):
        if common.is_active(unit) != "inactive":
            raise SystemExit(f"{unit} must be stopped")
    base = args.base or Path(ubuntu["home"]) / f"performance-{args.campaign_id}"
    mount = args.mount
    receipt_path = base / "temporary-data-disk.json"
    if receipt_path.exists() or mount.exists():
        raise SystemExit("Storage receipt or mount directory already exists")
    disk, device = check_disk(args, base)
    host = check_host_receipt(args, base, disk)
    old_log = f"/var/log/dnscollector/performance-{args.campaign_id}.jsonl"
    new_log = str(mount / f"performance-{args.campaign_id}.jsonl")
    dropin = Path("/etc/systemd/system/dnscollector.service.d/performance.conf")
    expected = f"StandardOutput=append:{old_log}"
    old = dropin.read_text()
    if old.splitlines().count(expected) != 1:
        raise SystemExit("Collector output override does not match the owned root-disk fixture")
    command = ["mkfs.ext4", "-m", "0", "-L", args.label, "-E", "lazy_itable_init=0,lazy_journal_init=0", str(device)]
    plan = {"mode": "apply" if args.apply else "dry-run", "device": str(device), "serial": disk.get("serial"), "wwn": disk.get("wwn"),
            "size_bytes": int(disk["size"]), "wipe_existing_signatures": args.wipe_existing_signatures,
            "mkfs": command, "mount": str(mount), "collector_log_old": old_log, "collector_log_new": new_log}
    if not args.apply:
        print(json.dumps(plan, indent=2))
        print("DRY RUN: all identity checks passed; nothing was changed. Re-run with --apply to format this disk.")
        return 0

    started = common.utc_now()
    base.mkdir(parents=True, exist_ok=True)
    after = {"blockdevices": common.lsblk()}
    (base / "storage-block-after-attach.json").write_text(json.dumps(after, indent=2) + "\n")
    if args.wipe_existing_signatures:
        subprocess.run(["wipefs", "--all", str(device)], check=True, stdin=subprocess.DEVNULL)
    formatted = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, text=True, check=True)
    (base / "storage-mkfs.txt").write_text(formatted.stdout + formatted.stderr)
    checked = subprocess.run(["e2fsck", "-f", "-n", str(device)], stdin=subprocess.DEVNULL, capture_output=True, text=True, check=True)
    (base / "storage-fsck.txt").write_text(checked.stdout + checked.stderr)
    filesystem_uuid = common.run("blkid", "-s", "UUID", "-o", "value", str(device))
    mount.mkdir(mode=0o755)
    options = "nodev,nosuid,noexec,relatime,discard,commit=30"
    subprocess.run(["mount", "-o", options, str(device), str(mount)], check=True)
    mounted = json.loads(common.run("findmnt", "-J", "-o", "SOURCE,TARGET,UUID,OPTIONS,FSTYPE,MAJ:MIN", "--target", str(mount)))["filesystems"][0]
    if mounted["target"] != str(mount) or mounted["uuid"] != filesystem_uuid or mounted["fstype"] != "ext4":
        raise SystemExit("Mounted filesystem identity differs from the freshly initialized disk")
    config = Path("/etc/dnscollector/config.yml")
    before_hash = hashlib.sha256(config.read_bytes()).hexdigest()
    (base / "pre-data-disk-config.yml").write_bytes(config.read_bytes())
    (base / "pre-data-disk-performance.conf").write_text(old)
    new = old.replace(expected + "\n", f"StandardOutput=append:{new_log}\n", 1)
    diff = "".join(difflib.unified_diff(old.splitlines(True), new.splitlines(True), fromfile="before/performance.conf", tofile="after/performance.conf"))
    (base / "collector-data-path.diff").write_text(diff)
    dropin.write_text(new)
    subprocess.run(["systemctl", "daemon-reload"], check=True)
    after_hash = hashlib.sha256(config.read_bytes()).hexdigest()
    if before_hash != after_hash:
        raise SystemExit("Collector YAML changed unexpectedly")
    receipt = dict(created_utc=common.utc_now(), initialization_started_utc=started, vm_guid=ubuntu["id"], host_attachment=host,
                   vhdx_path=host["vhdx_path"], device_path=disk["path"], persistent_device_path=str(args.device),
                   device_serial=disk.get("serial"), device_wwn=disk.get("wwn"), device_hctl=disk.get("hctl"),
                   major_minor=disk["maj:min"], filesystem_uuid=filesystem_uuid, filesystem_label=args.label,
                   mountpoint=str(mount), mount=mounted, mkfs_command=command, mkfs_exit_code=formatted.returncode,
                   fsck_exit_code=checked.returncode, lazy_initialization_disabled=True, raw_path=new_log,
                   prior_raw_path=old_log, archive_base=str(base), collector_yaml_sha256_before=before_hash,
                   collector_yaml_sha256_after=after_hash, collector_output_diff="collector-data-path.diff",
                   cpu_ram_network_changed=False, raw_cap_bytes=48 * 1024**3, min_free_bytes=3 * 1024**3,
                   raw_volume_available_bytes=os.statvfs(mount).f_bavail * os.statvfs(mount).f_frsize,
                   archive_volume_available_bytes=os.statvfs(base).f_bavail * os.statvfs(base).f_frsize)
    with receipt_path.open("x") as out:
        out.write(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps(receipt, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
