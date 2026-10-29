"""Summarize PID-validated PerfMon CSV within the client's sending interval."""
from __future__ import annotations
import argparse
import csv
from datetime import datetime, timedelta, timezone
import json
import math
from pathlib import Path
import re
from collections import Counter, defaultdict
from metrics_ledger import ledger_summary, normalize_metrics


def number(value):
    try:
        result = float(value)
        return result if math.isfinite(result) and result >= 0 else None
    except (ValueError, TypeError):
        return None


def describe(values):
    values = sorted(values)
    return None if not values else {"n": len(values), "min": values[0], "mean": sum(values) / len(values), "p95": values[math.ceil(len(values) * .95) - 1], "max": values[-1]}


def weighted(values, weights):
    pairs = sorted(zip(values, weights))
    total = sum(weight for _, weight in pairs)
    if not total:
        return None
    threshold, seen, p95 = total * .95, 0, None
    for value, weight in pairs:
        seen += weight
        if seen >= threshold:
            p95 = value
            break
    return {"n": len(pairs), "valid_seconds": total, "mean": sum(value * weight for value, weight in pairs) / total, "p95": p95, "max": max(values)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("capture", type=Path)
    parser.add_argument("--client", type=Path)
    parser.add_argument("--metrics", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    metadata = json.loads((args.capture / "metadata.json").read_text(encoding="utf-8-sig"))
    client = json.loads(args.client.read_text()) if args.client else None
    start = client["start_unix_ms"] if client else None
    end = client["sending_end_unix_ms"] if client else None
    reader = csv.DictReader((args.capture / "counters.csv").open(encoding="utf-8-sig", newline=""))
    clock = reader.fieldnames[0]
    bias_match = re.search(r"\((-?\d+)\)$", clock)
    if not bias_match:
        raise ValueError("PerfMon timezone bias missing")
    bias = int(bias_match.group(1))
    process_columns, thread_columns, system_columns = {}, {}, {}
    for column in reader.fieldnames[1:]:
        lower = column.lower()
        match = re.search(r"\\process\(([^)]+)\)\\(.+)$", lower)
        thread = re.search(r"\\thread\(([^)]+)\)\\(.+)$", lower)
        if match:
            process_columns[column] = match.groups()
        elif thread:
            thread_columns[column] = thread.groups()
        else:
            system_columns[column] = lower.split("\\", 3)[-1]
    series = {"service": defaultdict(list), "dns": defaultdict(list)}
    weights = {"service": defaultdict(list), "dns": defaultdict(list)}
    timelines = {"service": defaultdict(list), "dns": defaultdict(list)}
    thread_stats = {"service": defaultdict(lambda: defaultdict(list)), "dns": defaultdict(lambda: defaultdict(list))}
    thread_weights = {"service": defaultdict(lambda: defaultdict(list)), "dns": defaultdict(lambda: defaultdict(list))}
    states = {"service": Counter(), "dns": Counter()}
    coverage = {"service": [], "dns": []}
    invalid_pid = Counter()
    rejected_rates = {"service": Counter(), "dns": Counter()}
    system = defaultdict(list)
    system_weights = defaultdict(list)
    stamps = []
    intervals = []
    previous_epoch = None
    previous_process_ids, previous_thread_ids = {}, {}
    for row in reader:
        stamp = datetime.strptime(row[clock], "%m/%d/%Y %H:%M:%S.%f").replace(tzinfo=timezone.utc) + timedelta(minutes=bias)
        epoch = stamp.timestamp() * 1000
        previous = previous_epoch
        previous_epoch = epoch
        processes, threads = defaultdict(dict), defaultdict(dict)
        for column, (instance, counter) in process_columns.items():
            processes[instance][counter] = number(row[column])
        for column, (instance, counter) in thread_columns.items():
            threads[instance][counter] = number(row[column])
        prior_process_ids, prior_thread_ids = previous_process_ids, previous_thread_ids
        previous_process_ids = {instance: values.get("id process") for instance, values in processes.items()}
        previous_thread_ids = {instance: (values.get("id process"), values.get("id thread")) for instance, values in threads.items()}
        if start is not None and not (start <= epoch < end):
            continue
        seconds = max(0, (epoch - max(previous if previous is not None else epoch, start if start is not None else 0)) / 1000)
        if previous is not None:
            intervals.append((epoch - previous) / 1000)
        stamps.append(epoch)
        for role in ("service", "dns"):
            expected = metadata[f"{role}_pid"]
            valid = [(instance, values) for instance, values in processes.items() if values.get("id process") == expected]
            if len(valid) != 1:
                invalid_pid[role] += 1
                continue
            process_instance, process = valid[0]
            for counter, value in process.items():
                if value is not None:
                    rate = counter.startswith("%") or counter.endswith("/sec")
                    if rate and prior_process_ids.get(process_instance) != expected:
                        rejected_rates[role]["process_identity_discontinuity"] += 1
                        continue
                    if counter.startswith("%") and value > metadata["logical_processors"] * 100.5:
                        rejected_rates[role]["process_impossible_cpu"] += 1
                        continue
                    series[role][counter].append(value)
                    weights[role][counter].append(seconds)
                    timelines[role][counter].append((epoch, value))
            current = {}
            for instance, values in threads.items():
                tid = values.get("id thread")
                if values.get("id process") != expected or not tid:
                    continue
                current[int(tid)] = values
                for counter, value in values.items():
                    if value is not None:
                        rate = counter.startswith("%") or counter.endswith("/sec")
                        if rate and prior_thread_ids.get(instance) != (expected, tid):
                            rejected_rates[role]["thread_identity_discontinuity"] += 1
                            continue
                        if counter.startswith("%") and value > 100.5:
                            rejected_rates[role]["thread_impossible_cpu"] += 1
                            continue
                        thread_stats[role][int(tid)][counter].append(value)
                        thread_weights[role][int(tid)][counter].append(seconds)
                state = values.get("thread state")
                reason = values.get("thread wait reason")
                if state is not None:
                    states[role][f"state={int(state)},wait_reason={int(reason) if reason is not None else 'unavailable'}"] += 1
            expected_threads = process.get("thread count")
            coverage[role].append({"utc_ms": epoch, "observed_thread_ids": len(current), "process_thread_count": expected_threads})
        for column, label in system_columns.items():
            value = number(row[column])
            if value is not None:
                system[label].append(value)
                system_weights[label].append(seconds)
    result = {"metadata": metadata, "window": {"source": "client sending window, excludes drain; PDH rates average preceding intervals" if client else "all capture rows", "requested_start_ms": start, "requested_end_ms": end, "first_sample_ms": min(stamps) if stamps else None, "last_sample_ms": max(stamps) if stamps else None, "rows": len(stamps), "csv_timezone_bias_minutes": bias, "actual_interval_seconds": describe(intervals)}, "processes": {}, "system": {name: describe(values) for name, values in system.items()}}
    result["system_time_weighted"] = {name: weighted(values, system_weights[name]) for name, values in system.items()}
    for role in ("service", "dns"):
        values = series[role]
        raw_cpu = values.get("% processor time", [])
        private = timelines[role].get("private bytes", [])
        result["processes"][role] = {"invalid_pid_samples": invalid_pid[role], "counters": {name: describe(items) for name, items in values.items()}, "cpu_guest_capacity_percent": describe([item / metadata["logical_processors"] for item in raw_cpu]), "private_bytes_start": private[0][1] if private else None, "private_bytes_end": private[-1][1] if private else None, "private_bytes_delta": private[-1][1] - private[0][1] if private else None, "threads": {str(tid): {name: describe(items) for name, items in counters.items()} for tid, counters in thread_stats[role].items()}, "thread_state_samples": dict(states[role]), "thread_coverage": {"samples": len(coverage[role]), "incomplete_samples": sum(item["process_thread_count"] is not None and item["observed_thread_ids"] < item["process_thread_count"] for item in coverage[role]), "observed": describe([item["observed_thread_ids"] for item in coverage[role]])}}
        result["processes"][role]["cpu_raw_time_weighted_percent"] = weighted(raw_cpu, weights[role]["% processor time"])
        result["processes"][role]["cpu_guest_time_weighted_percent"] = weighted([value / metadata["logical_processors"] for value in raw_cpu], weights[role]["% processor time"])
        result["processes"][role]["rejected_rate_values"] = dict(rejected_rates[role])
        result["processes"][role]["cpu_seconds_integrated"] = sum(value * weight for value, weight in zip(raw_cpu, weights[role]["% processor time"])) / 100
        for tid, counters in thread_stats[role].items():
            values = counters.get("% processor time", [])
            time_weights = thread_weights[role][tid]["% processor time"]
            result["processes"][role]["threads"][str(tid)]["cpu_time_weighted_percent"] = weighted(values, time_weights)
            result["processes"][role]["threads"][str(tid)]["cpu_seconds_integrated"] = sum(value * weight for value, weight in zip(values, time_weights)) / 100
    if args.metrics:
        samples = [json.loads(line) for line in args.metrics.read_text().splitlines() if line]
        final = normalize_metrics(samples[-1])
        result["service_final"] = samples[-1]
        result["ledger"] = ledger_summary(final)
    if client:
        result["client"] = {key: value for key, value in client.items() if key != "latency_histogram_bins"}
        result["client"]["sent_qps"] = client["counts"]["sent"] / (client["measured_send_duration_ns"] / 1e9)
        result["client"]["successful_qps"] = client["counts"]["successful"] / (client["measured_send_duration_ns"] / 1e9)
        result["client"]["rotation_wait_worker_fraction"] = client["counts"]["rotation_wait_us"] / (client["options"]["threads"] * client["options"]["duration"] * 1e6)
    output = args.output or args.capture / "analysis.json"
    output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"output": str(output), "rows": len(stamps), "pids": {role: {"invalid": invalid_pid[role], "cpu": describe(series[role].get("% processor time", []))} for role in ("service", "dns")}}))


if __name__ == "__main__":
    main()
