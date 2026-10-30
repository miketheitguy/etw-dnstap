"""Export bounded native process CPU bins and align cumulative client counts."""

import argparse
import bisect
import csv
from datetime import datetime, timezone
import hashlib
import json
import math
from pathlib import Path
import re
import subprocess


RUNS = (
    "pooled-auth-1000-300s-2", "pooled-rec-1000-300s-2",
    "pooled-auth-48000-300s-1", "pooled-rec-28000-300s-1",
)
XPERF = Path("C:/Program Files (x86)/Windows Kits/10/Windows Performance Toolkit/xperf.exe")
ROW = re.compile(r"^\s*(\d+),\s*(.+?)\s+\(\s*(\d+)\)\s*$")


def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def write(path, value):
    path.write_text(json.dumps(value, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest().upper()


def interpolate(points, at):
    times = [p[0] for p in points]
    i = bisect.bisect_left(times, at)
    if i < len(points) and times[i] == at:
        return points[i][1]
    assert 0 < i < len(points), (at, points[0], points[-1])
    a, b = points[i - 1], points[i]
    return a[1] + (b[1] - a[1]) * (at - a[0]) / (b[0] - a[0])


def client_series(campaign, run):
    summary = read(campaign / "client" / run / "summary.json")
    metrics = [json.loads(line) for line in (campaign / "client" / run / "metrics.jsonl").read_text().splitlines()]
    workers = {}
    checks = []
    for worker in summary["workers"]:
        wid = worker["worker"]
        rows = [r for r in metrics if r["worker"] == wid]
        assert rows and sum(r["final_sample"] for r in rows) == 1
        assert rows[-1]["final_sample"]
        assert all(b["elapsed_ns"] > a["elapsed_ns"] for a, b in zip(rows, rows[1:]))
        assert all(b["counts"]["sent"] >= a["counts"]["sent"] for a, b in zip(rows, rows[1:]))
        assert rows[-1]["counts"]["sent"] == worker["counts"]["sent"]
        assert worker["error"] is None
        regular = [(r["elapsed_ns"] / 1e9, r["counts"]["sent"]) for r in rows if not r["final_sample"]]
        assert regular[0][0] <= 5 and regular[-1][0] >= 295
        workers[wid] = regular
        checks.append({"worker": wid, "regular_samples": len(regular),
                       "first_regular_s": regular[0][0], "last_regular_s": regular[-1][0],
                       "maximum_regular_gap_s": max(b[0] - a[0] for a, b in zip(regular, regular[1:])),
                       "final_sent": rows[-1]["counts"]["sent"], "final_sample_elapsed_s": rows[-1]["elapsed_ns"] / 1e9,
                       "monotone_elapsed_and_sent": True, "final_matches_worker_summary": True})
    assert sum(w["counts"]["sent"] for w in summary["workers"]) == summary["counts"]["sent"]
    cumulative = [sum(interpolate(p, at) for p in workers.values()) for at in range(5, 296)]
    rates = [b - a for a, b in zip(cumulative, cumulative[1:])]
    assert all(math.isfinite(v) and v >= 0 for v in rates)
    assert math.isclose(sum(rates), cumulative[-1] - cumulative[0], abs_tol=1e-7)
    return summary, rates, {"workers": checks, "cumulative_at_5s": cumulative[0],
                            "cumulative_at_295s": cumulative[-1], "integrated_interior_queries": sum(rates),
                            "full_sending_sent": summary["counts"]["sent"], "regular_only_rate_interpolation": True}


def process_reports(path):
    reports = []
    current = None
    for line in path.read_text(encoding="utf-8-sig", errors="strict").splitlines():
        if "Time (us)" in line and "Process Name" in line:
            current = {}
            reports.append(current)
        elif match := ROW.match(line):
            assert current is not None
            us, name, pid = match.groups()
            identity = f"{name} ({int(pid)})"
            assert identity not in current
            current[identity] = int(us)
        elif line.strip():
            raise ValueError(f"Unexpected native output: {line!r}")
    return reports


def clock_summary(path):
    rows = read(path)
    lower = max(r["offset_lower_ms"] for r in rows)
    upper = min(r["offset_upper_ms"] for r in rows)
    assert lower <= upper
    return {"source": str(path), "guest_minus_host_intersection_ms": [lower, upper], "measurements": rows}


def export_run(campaign, output, run, native_only=False):
    destination = output / run
    destination.mkdir()
    gate_path = campaign / "wpa" / run / "trace-gate-v1.json"
    gate = read(gate_path)
    assert gate["trace_gate_pass"] and gate["client_interval_contained"]
    native = next(s for s in read(campaign / "wpa" / "native-comparison-v1.json")["stages"] if s["run_id"] == run)
    broad = native["broad"]
    processes = broad["requested_processes"]
    service = next(p for p in processes if p.startswith("etw-dnstap.exe"))
    dns = next(p for p in processes if p.startswith("dns.exe"))
    start_us = round(broad["start_s"] * 1e6)
    bounds = [(start_us + i * 1000000, start_us + (i + 1) * 1000000) for i in range(290)]
    command = [str(XPERF), "-i", gate["canonical_etl"]]
    for a, b in bounds:
        command.extend(["-a", "cswitch", "-process", "-exc_dpcisr", "-range", str(a), str(b)])
    assert len(subprocess.list2cmdline(command)) < 32760
    source_trace = Path(gate["canonical_etl"])
    assert source_trace.stat().st_size == gate["canonical_etl_bytes"]
    write(destination / "command.json", {"executable": str(XPERF), "arguments": command[1:],
                                         "ranges_us": bounds, "canonical_etl_sha256_from_gate": gate["canonical_sha256"],
                                         "etl_rehashed": False, "symbols": False,
                                         "started_utc": datetime.now(timezone.utc).isoformat()})
    with (destination / "native-cpu.log").open("wb") as stdout, (destination / "native-cpu.stderr.log").open("wb") as stderr:
        result = subprocess.run(command, stdout=stdout, stderr=stderr, check=False)
    write(destination / "result.json", {"exit_code": result.returncode, "ended_utc": datetime.now(timezone.utc).isoformat()})
    assert result.returncode == 0
    reports = process_reports(destination / "native-cpu.log")
    assert len(reports) == 290
    assert all(service in r and dns in r for r in reports)
    validation = {}
    for process in (service, dns):
        total = sum(r[process] for r in reports)
        expected = broad["exclusive_cpu"][process]["cpu_us"]
        delta = total - expected
        # Native reports round each total to integer microseconds; the saved
        # independent broad report has a single integer rounding operation.
        assert abs(delta) <= 291, (process, total, expected, delta)
        validation[process] = {"binned_cpu_us": total, "broad_cpu_us": expected, "difference_us": delta,
                               "mean_percent_one_core": total / 2900000,
                               "maximum_percent_one_core": max(r[process] / 10000 for r in reports)}
    summary, rates, client_checks = client_series(campaign, run)
    csv_path = destination / "series.csv"
    fields = ["run_id", "elapsed_s", "interval_start_s", "interval_end_s", "trace_start_s", "trace_end_s",
              "service_cpu_percent_one_core", "dns_cpu_percent_one_core", "achieved_qps", "requested_qps"]
    with csv_path.open("w", newline="", encoding="utf-8") as stream:
        writer = csv.DictWriter(stream, fieldnames=fields)
        writer.writeheader()
        for i, ((a, b), report, qps) in enumerate(zip(bounds, reports, rates)):
            writer.writerow(dict(zip(fields, [run, i + 5.5, i + 5, i + 6, a / 1e6, b / 1e6,
                                               report[service] / 10000, report[dns] / 10000, qps, summary["options"]["qps"]])))
    sources = [gate_path, campaign / "wpa" / "native-comparison-v1.json",
               campaign / "client" / run / "summary.json", campaign / "client" / run / "metrics.jsonl",
               campaign / "client" / run / "invocation.json", campaign / "capture" / run / "clock-bounds.json",
               campaign / "capture" / run / "clock-bounds-end.json", Path("tools/dns-stress/src/main.rs")]
    qualification = "One RA=false response; zero packet/capture loss and clean SCM exit" if run == "pooled-rec-1000-300s-2" else "All workload and native trace gates pass"
    receipt = {"run_id": run, "mode": summary["options"]["mode"], "requested_qps": summary["options"]["qps"],
               "strict_pass": run != "pooled-rec-1000-300s-2", "qualification": qualification,
               "series_csv": str(csv_path), "series_sha256": digest(csv_path), "bins": 290,
               "service_process": service, "dns_process": dns, "native_validation": validation,
               "client_validation": client_checks, "full_sending_achieved_qps": summary["counts"]["sent"] / 300,
               "interior_achieved_qps": sum(rates) / 290,
               "clock_bounds_before": clock_summary(sources[5]), "clock_bounds_after": clock_summary(sources[6]),
               "nominal_trace_sending_start_s": gate["client_trace_start_s"],
               "trace_grid_rounding_us": start_us - broad["start_s"] * 1e6,
               "canonical_etl_sha256": gate["canonical_sha256"], "canonical_etl_bytes": gate["canonical_etl_bytes"],
               "sources_sha256": {str(p): digest(p) for p in sources},
               "native_log_sha256": digest(destination / "native-cpu.log")}
    write(destination / "validation.json", receipt)
    print(json.dumps({"run_id": run, "native_validation": validation, "interior_qps": sum(rates) / 290}), flush=True)
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--campaign", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--run", action="append", choices=RUNS)
    args = parser.parse_args()
    # A fresh directory protects original evidence and previous derivatives.
    args.output.mkdir(parents=True, exist_ok=False)
    for topic in ("processing", "cswitch"):
        result = subprocess.run([str(XPERF), "-help", topic], capture_output=True, check=True)
        (args.output / f"xperf-help-{topic}.txt").write_bytes(result.stdout)
    manifest = {"schema": 1, "generated_utc": datetime.now(timezone.utc).isoformat(),
                "derivative": True, "raw_evidence_unchanged": True, "etl_rehashed": False,
                "documentation": "https://learn.microsoft.com/windows-hardware/test/wpt/cswitch",
                "native_method": "290 Xperf cswitch -process -exc_dpcisr -range T1 T2 actions per ETL pass; T1/T2 in us",
                "cpu_units": "CPU percent of one core; cpu_us / interval_us * 100; guest has two vCPUs; no clipping",
                "cpu_semantics": "CSwitch scheduled process CPU excluding observed DPC/ISR; same native action as independent broad totals",
                "qps_method": "Interpolate each worker's monotone cumulative sent counts separately at each integer sending second, then sum differences; only regular samples, never final drain sample",
                "worker_clock": "dns-stress source main.rs gives every worker the same start Instant; elapsed_ns is relative to that shared start, not worker creation time",
                "time_alignment": "Nominal UTC subtraction from trace-gate, identical sending [5,295) mapping to saved broad view; rounded to nearest us for Xperf. Guest-host offset is not corrected. Before/after measured bounds and 1ms client Unix timestamp quantization limit cross-clock alignment; interpolation cannot recover sub-sample send timing",
                "source_code_sha256": digest(Path(__file__)), "runs": []}
    write(args.output / "manifest.json", manifest)
    for run in args.run or RUNS:
        manifest["runs"].append(export_run(args.campaign, args.output, run))
        write(args.output / "manifest.json", manifest)


if __name__ == "__main__":
    main()
