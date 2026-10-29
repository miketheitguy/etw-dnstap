"""Descriptive matched-run CPU comparison; normalized work values are estimates."""
import argparse
import json
from pathlib import Path
from metrics_ledger import normalize_metrics


parser = argparse.ArgumentParser()
parser.add_argument("campaign", type=Path)
args = parser.parse_args()
rows = []
for path in sorted((args.campaign / "windows").glob("*/analysis.json")):
    run = path.parent.name
    client_path = args.campaign / "client" / run / "summary.json"
    if not client_path.exists():
        continue
    analysis = json.loads(path.read_text())
    client = json.loads(client_path.read_text())
    duration = client["measured_send_duration_ns"] / 1e9
    if duration != 30:
        continue
    process = analysis["processes"]["service"]
    cpu = process["cpu_raw_time_weighted_percent"]
    final = normalize_metrics(analysis["service_final"])
    qps = client["counts"]["sent"] / duration
    encoded_rate = final["encoded"] / duration
    mean_fraction = cpu["mean"] / 100
    rows.append({
        "run": run, "mode": client["options"]["mode"],
        "binary_sha256": analysis["metadata"]["service_sha256"],
        "service_command": analysis["metadata"]["service_command"],
        "sending_seconds": duration, "qps": qps,
        "sent": client["counts"]["sent"], "successful": client["counts"]["successful"],
        "received": final["received"], "encoded": final["encoded"],
        "ring_dropped": final["ring_dropped"],
        "intake_dropped": final["intake_dropped"],
        "pool_exhausted": final["pool_exhausted"],
        "receiver_disconnected": final["receiver_disconnected"],
        "ring_drop_percent": 100 * final["ring_dropped"] / final["received"] if final["received"] else None,
        "intake_drop_percent": 100 * final["intake_dropped"] / final["received"] if final["received"] else None,
        "service_cpu_one_core_percent": cpu["mean"],
        "service_cpu_guest_capacity_percent": cpu["mean"] / analysis["metadata"]["logical_processors"],
        "cpu_seconds_integrated_valid": process["cpu_seconds_integrated"],
        "valid_cpu_seconds": cpu["valid_seconds"], "cpu_samples": cpu["n"],
        "estimated_cpu_ms_per_offered_query": mean_fraction * 1000 / qps,
        "estimated_cpu_us_per_encoded_record": mean_fraction * 1e6 / encoded_rate if encoded_rate else None,
        "dns_cpu_one_core_percent": analysis["processes"]["dns"]["cpu_raw_time_weighted_percent"]["mean"],
        "guest_cpu_percent": analysis["system_time_weighted"]["processor(_total)\\% processor time"]["mean"],
        "service_private_peak_mib": process["counters"]["private bytes"]["max"] / 1048576,
        "service_private_delta_mib": process["private_bytes_delta"] / 1048576,
    })
comparisons = []
for mode in ("authoritative", "recursive"):
    selected = [row for row in rows if row["mode"] == mode]
    baseline = next((row for row in selected if "baseline1024-" in row["run"]), None)
    if not baseline:
        continue
    for row in selected:
        if row is baseline:
            continue
        delta = row["service_cpu_one_core_percent"] - baseline["service_cpu_one_core_percent"]
        comparisons.append({"mode": mode, "reference": baseline["run"], "run": row["run"],
                            "cpu_change_percentage_points_one_core": delta,
                            "cpu_relative_change_percent": 100 * delta / baseline["service_cpu_one_core_percent"] if baseline["service_cpu_one_core_percent"] else None,
                            "encoded_change_percent": 100 * (row["encoded"] / baseline["encoded"] - 1) if baseline["encoded"] else None})
result = {"scope": "Single-shot 30-second, no-WPR comparison arms. Whole-run counts include final drain; CPU uses valid sending-window interval rates.",
          "normalization": "Estimated CPU/work = mean CPU fraction / full-run achieved work rate. Assumes valid coverage represents sending load; final drain and interval boundaries are approximate. Integrated partial-coverage CPU seconds are not divided directly by whole-run counts. Process overhead and callback work are included, not parser-only cost.",
          "limits": "No variance estimate or automatic winner. Quantization and single-short-run variation can dominate small differences. Loss changes the amount of encoded work. Compare actual upstream fractions separately from the receiver's wire-counter receipts.",
          "runs": rows, "changes_from_original_1024": comparisons}
output = args.campaign / "cpu-comparison.json"
output.write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result))
