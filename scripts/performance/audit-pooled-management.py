"""Audit preserved management receipts against completed client windows."""
from __future__ import annotations

import argparse
from datetime import datetime, timedelta, timezone
import difflib
import hashlib
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("campaign", type=Path)
args = parser.parse_args()
base = args.campaign
output = base / "build" / "management-overlap-audit-v1.json"
if output.exists():
    raise SystemExit("Preserve existing management audit")


def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def epoch_ms(stamp):
    return datetime.fromisoformat(stamp.replace("Z", "+00:00")).timestamp() * 1000


windows = {}
for path in (base / "client").glob("*/summary.json"):
    if not path.stat().st_size:
        continue
    client = read(path)
    windows[path.parent.name] = {
        "client_start_unix_ms": client["start_unix_ms"],
        "client_end_unix_ms": client["sending_end_unix_ms"],
        "requested_qps": client["options"]["qps"],
        "sending_seconds": client["options"]["duration"],
    }
    metrics = list((base / "windows-campaign").glob(f"{path.parent.name}.*.jsonl"))
    if len(metrics) == 1:
        final = json.loads(metrics[0].read_text(encoding="utf-8-sig").splitlines()[-1])
        windows[path.parent.name].update(
            queue_high_water=final["queue_high_water_observed"],
            intake_dropped=final["intake_dropped"],
            final_metrics=final["final"],
        )

elapsed = {
    "pooled-auth-1000-300s-1": 4.2023452,
    "pooled-rec-1000-300s-1": 3.6369542,
    "pooled-auth-8000-60s-1": 3.3506515,
    "pooled-auth-16000-60s-1": 2.7700003,
    "pooled-auth-32000-60s-1": 2.7579024,
    "pooled-auth-48000-60s-1": 7.6801211,
}
calls = []
for path in (base / "windows-campaign").glob("*.json"):
    row = read(path)
    if isinstance(row, list) and len(row) == 1:
        row = row[0]
    if not isinstance(row, dict) or "action" not in row or "utc" not in row:
        continue
    stamp = epoch_ms(row["utc"])
    overlaps = [name for name, window in windows.items()
                if window["client_start_unix_ms"] <= stamp < window["client_end_unix_ms"]]
    call = {
        "receipt": str(path), "action": row["action"], "run": row["run_id"],
        "guest_receipt_utc": row["utc"],
        "dns_records_enumerated": row.get("authoritative_a_records") == 5000,
        "receipt_inside_sending_windows": overlaps,
        "exact_host_command_start_end_available": False,
    }
    if row["action"] == "Status" and row["run_id"] in elapsed:
        end = datetime.fromisoformat(row["utc"].replace("Z", "+00:00"))
        call.update(tool_elapsed_seconds=elapsed[row["run_id"]],
                    estimated_conservative_envelope_start_utc=(end-timedelta(seconds=elapsed[row["run_id"]]+1)).isoformat(),
                    estimated_conservative_envelope_end_utc=(end+timedelta(seconds=1)).isoformat())
    calls.append(call)
calls.sort(key=lambda item: item["guest_receipt_utc"])
for name, window in windows.items():
    window["in_load_dns_inventory_calls"] = [call for call in calls
        if call["dns_records_enumerated"] and name in call["receipt_inside_sending_windows"]]

before = base / "build" / "status-helper-before-fix.ps1"
after = Path("scripts/performance/windows-pooled-campaign.ps1")
diff = "".join(difflib.unified_diff(before.read_text(encoding="utf-8-sig").splitlines(True),
                                  after.read_text(encoding="utf-8-sig").splitlines(True),
                                  fromfile=str(before), tofile=str(after)))
(base / "build" / "status-helper-fix.diff").write_text(diff)
result = {
    "utc": datetime.now(timezone.utc).isoformat(),
    "scope": "All completed client windows and preserved action receipts present when Status inventory was corrected; no inferred exact host command timestamps.",
    "calls": calls, "runs": windows,
    "control_order": "Stage controller invokes Start before launching the client, and Stop/Fetch only after client completion/drain. Their action receipts lie outside completed sending windows.",
    "timing_limit": "Guest receipt UTC is exact; original host command start/end were not persisted. Status tool wall times are recorded from tool results and only bound an estimated envelope with one-second margins.",
    "correlation_limit": "In-load inventory overlaps four stages. Both 1k stages and auth8k had zero intake loss; auth48k-1 had loss in two coincident intervals. Auth4k reached HWM8032 without in-load Status, and auth64k sustained loss without in-load Status. Correlation does not prove management caused the 48k loss.",
    "before_sha256": hashlib.sha256(before.read_bytes()).hexdigest(),
    "after_sha256": hashlib.sha256(after.read_bytes()).hexdigest(),
    "fix_diff": str(base / "build" / "status-helper-fix.diff"),
}
output.write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps({"output": str(output), "overlaps": {
    name: len(window["in_load_dns_inventory_calls"]) for name, window in windows.items()
    if window["in_load_dns_inventory_calls"]}, "other_in_load_actions": [call for call in calls
    if call["action"] != "Status" and call["receipt_inside_sending_windows"]]}, indent=2))
