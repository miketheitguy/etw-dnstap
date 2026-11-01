"""Compare fully enclosed one-second counter intervals around WPR; not causal."""
import argparse
from collections import defaultdict
from datetime import datetime
import json
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))  # metrics_ledger lives in the parent directory
from metrics_ledger import nonnegative_integer, normalize_metrics, validate_sample_pair


def epoch_ms(path):
    value = path.read_text(encoding="utf-8-sig").strip()
    return datetime.fromisoformat(value.replace("Z", "+00:00")).timestamp() * 1000


parser = argparse.ArgumentParser()
parser.add_argument("capture", type=Path)
parser.add_argument("client", type=Path)
parser.add_argument("metrics", type=Path)
args = parser.parse_args()
client = json.loads(args.client.read_text())
start, end = client["start_unix_ms"], client["sending_end_unix_ms"]
trace_start = epoch_ms(args.capture / "wpr-start-utc.txt")
trace_end = epoch_ms(args.capture / "wpr-stop-end-utc.txt")
samples = [normalize_metrics(json.loads(line), require_final=False)
           for line in args.metrics.read_text(encoding="utf-8-sig").splitlines() if line]
keys = ("received", "ring_dropped", "intake_dropped", "pool_exhausted",
        "receiver_disconnected", "encoded", "etw_events_lost", "parse_ns", "output_ns")
for sample in samples:
    for key in (*keys, "epoch_ms"):
        nonnegative_integer(sample, key)
groups = defaultdict(lambda: {"intervals": 0, "seconds": 0, **{key: 0 for key in keys}})
for previous, current in zip(samples, samples[1:]):
    validate_sample_pair(previous, current, keys)
    left, right = previous["epoch_ms"], current["epoch_ms"]
    if right <= left:
        raise ValueError("Non-increasing metrics timestamps")
    if left < start or right > end:
        label = "outside_or_crosses_send_boundary"
    elif right <= trace_start:
        label = "pre_wpr"
    elif left >= trace_end:
        label = "post_wpr"
    elif left >= trace_start and right <= trace_end:
        label = "wpr_and_rundown"
    else:
        label = "crosses_wpr_boundary"
    group = groups[label]
    group["intervals"] += 1
    group["seconds"] += (right - left) / 1000
    for key in keys:
        delta = current[key] - previous[key]
        if delta < 0:
            raise ValueError(f"Counter reset: {key}")
        group[key] += delta
for group in groups.values():
    group["ring_drop_percent_of_received"] = 100 * group["ring_dropped"] / group["received"] if group["received"] else None
    group["intake_drop_percent_of_received"] = 100 * group["intake_dropped"] / group["received"] if group["received"] else None
    group["received_per_second"] = group["received"] / group["seconds"]
    group["encoded_per_second"] = group["encoded"] / group["seconds"]
result = {"method": "Guest metrics timestamps versus WPR helper timestamps; fully enclosed counter intervals, boundary intervals separate. Host/guest clock bounds and one-second sampling make phase boundaries approximate. Comparison is descriptive, not causal; final drained ledger remains authoritative.", "client_window_ms": [start, end], "wpr_helper_window_ms": [trace_start, trace_end], "groups": dict(groups)}
output = args.capture / "metrics-phases.json"
output.write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result))
