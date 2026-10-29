"""Summarize host load-generator resources for its active sending window."""
import argparse
import csv
from datetime import datetime
import json
from pathlib import Path
from statistics import mean
import math


def stats(values):
    values = sorted(values)
    return {"n": len(values), "mean": mean(values), "p95": values[math.ceil(len(values) * .95) - 1], "max": max(values), "min": min(values)} if values else None


parser = argparse.ArgumentParser()
parser.add_argument("directory", type=Path)
args = parser.parse_args()
summary = json.loads((args.directory / "summary.json").read_text())
invocation = json.loads((args.directory / "invocation.json").read_text(encoding="utf-8-sig"))
start, end = summary["start_unix_ms"], summary["sending_end_unix_ms"]
rows = list(csv.DictReader((args.directory / "host-process.csv").open(encoding="utf-8-sig", newline="")))
series = {key: [] for key in ("cpu_raw_percent", "cpu_host_capacity_percent", "private_bytes", "working_set", "threads", "handles", "host_cpu_percent", "interval_seconds")}
previous = None
for row in rows:
    stamp = datetime.fromisoformat(row["utc"].replace("Z", "+00:00")).timestamp() * 1000
    if start <= stamp < end:
        for key in ("private_bytes", "working_set", "threads", "handles", "host_cpu_percent"):
            series[key].append(float(row[key]))
        if previous:
            seconds = (stamp - previous[0]) / 1000
            cpu = 100 * (float(row["cpu_seconds"]) - previous[1]) / seconds
            if cpu >= 0:
                series["cpu_raw_percent"].append(cpu)
                series["cpu_host_capacity_percent"].append(cpu / invocation["logical_processors"])
                series["interval_seconds"].append(seconds)
    previous = (stamp, float(row["cpu_seconds"]))
result = {"run": args.directory.name, "logical_processors": invocation["logical_processors"], "window_start_ms": start, "window_end_ms": end, "stats": {key: stats(values) for key, values in series.items()}, "cpu_raw_time_weighted_percent": sum(value * weight for value, weight in zip(series["cpu_raw_percent"], series["interval_seconds"])) / sum(series["interval_seconds"]) if series["interval_seconds"] else None}
(args.directory / "host-analysis.json").write_text(json.dumps(result, indent=2) + "\n")
print(json.dumps(result))
