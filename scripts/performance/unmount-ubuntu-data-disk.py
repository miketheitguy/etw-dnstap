"""Unmount the temporary campaign data filesystem after idle/open-descriptor checks.

The default is a DRY RUN: every check runs and the plan is printed, nothing is unmounted.
Add ``--apply`` to unmount.  Run as root on the Ubuntu guest.

Checks: guest hostname; dnscollector and the performance monitor are stopped; the raw log
named in the saved receipt is gone; ``--mount`` is an active mountpoint whose source is the
exact device (serial, WWN, filesystem UUID, label, major:minor) recorded by
prepare-ubuntu-data-disk.py, and that device is not the root disk; no process has a working
directory or open descriptor below the mountpoint.

Example::

    sudo python3 unmount-ubuntu-data-disk.py --campaign-id my-campaign --mount /mnt/dnstap-my-campaign
    sudo python3 unmount-ubuntu-data-disk.py --campaign-id my-campaign --mount /mnt/dnstap-my-campaign --apply
"""
from __future__ import annotations

import argparse
import json
import subprocess
from pathlib import Path
from typing import Any

import guest_common as common
import lab_config


def open_descriptors(mount: Path) -> list[dict[str, Any]]:
    opened: list[dict[str, Any]] = []
    for process in Path("/proc").glob("[0-9]*"):
        for link in [process / "cwd", *list((process / "fd").glob("*"))]:
            try:
                target = str(link.readlink())
            except FileNotFoundError:
                continue
            except PermissionError as exc:
                raise SystemExit(f"Cannot verify open descriptors: {link}") from exc
            if target == str(mount) or target.startswith(str(mount) + "/"):
                opened.append(dict(pid=int(process.name), descriptor=link.name, target=target))
    return opened


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0],
                                     formatter_class=argparse.RawDescriptionHelpFormatter, epilog=__doc__.split("\n", 1)[1])
    parser.add_argument("--campaign-id", required=True)
    parser.add_argument("--mount", required=True, type=Path, help="explicit mountpoint to unmount")
    parser.add_argument("--base", type=Path, help="archive/receipt directory (default: <home>/performance-<campaign-id>)")
    parser.add_argument("--apply", action="store_true", help="actually unmount (default: dry run)")
    args = parser.parse_args(argv)
    common.require_campaign_id(args.campaign_id)
    common.require_root()
    ubuntu = lab_config.get("ubuntu_vm")
    base = args.base or Path(ubuntu["home"]) / f"performance-{args.campaign_id}"
    mount = args.mount
    receipt = base / "temporary-data-disk-unmounted.json"
    saved = json.loads((base / "temporary-data-disk.json").read_text())
    if common.run("hostname") != ubuntu["hostname"] or receipt.exists():
        raise SystemExit("Wrong guest or prior unmount receipt")
    if str(mount) != saved["mountpoint"]:
        raise SystemExit(f"--mount does not match the recorded mountpoint {saved['mountpoint']}")
    states = {unit: common.is_active(unit) for unit in ("dnscollector", "dnstap-performance-monitor")}
    if any(state != "inactive" for state in states.values()) or Path(saved["raw_path"]).exists():
        raise SystemExit("Collector/monitor must be stopped and verified raw file removed")
    before = json.loads(common.run("findmnt", "-J", "-o", "SOURCE,TARGET,UUID,FSTYPE,OPTIONS,MAJ:MIN", "--target", str(mount)))["filesystems"][0]
    if (not mount.is_mount() or before["target"] != str(mount)
            or before["uuid"] != saved["filesystem_uuid"] or before["maj:min"] != saved["major_minor"]):
        raise SystemExit("Temporary mount identity changed")
    device = common.require_block_device(Path(saved["persistent_device_path"]))
    entry = common.lsblk(str(device))[0]
    if str(device) == common.root_disk():
        raise SystemExit("Refusing: the recorded device is the root disk")
    for key, field in (("serial", "device_serial"), ("wwn", "device_wwn")):
        if saved.get(field) and entry.get(key) != saved[field]:
            raise SystemExit(f"Device {key} differs from the preparation receipt")
    if saved.get("filesystem_label") and entry.get("label") != saved["filesystem_label"]:
        raise SystemExit("Filesystem label differs from the preparation receipt")
    opened = open_descriptors(mount)
    if opened:
        raise SystemExit("The temporary filesystem still has open descriptors: " + json.dumps(opened))
    if not args.apply:
        print(json.dumps({"mode": "dry-run", "would_unmount": str(mount), "device": str(device), "uuid": saved["filesystem_uuid"], "states": states}, indent=2))
        print("DRY RUN: all checks passed; nothing was unmounted. Re-run with --apply.")
        return 0
    started = common.utc_now()
    result = subprocess.run(["umount", str(mount)], capture_output=True, text=True, check=True)
    remaining = subprocess.run(["findmnt", "-r", "-n", "-S", "UUID=" + saved["filesystem_uuid"]], capture_output=True, text=True, check=False)
    if mount.is_mount() or remaining.stdout.strip():
        raise SystemExit("Temporary filesystem remains mounted")
    proof = dict(unmounted=True, unmounted_utc=common.utc_now(), unmount_started_utc=started, vm_guid=saved["vm_guid"],
                 vhdx_path=saved["vhdx_path"], filesystem_uuid=saved["filesystem_uuid"], mountpoint=str(mount),
                 mount_before=before, states=states, raw_file_absent=True, open_fd_matches=opened,
                 umount_exit_code=result.returncode, no_remaining_mount_for_uuid=True, vhdx_should_be_retained=True)
    with receipt.open("x") as out:
        out.write(json.dumps(proof, indent=2) + "\n")
    print(json.dumps(proof, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
