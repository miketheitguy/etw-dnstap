"""Check distinct summary outcomes against preserved campaign receipts."""
import argparse
import ast
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import subprocess
import sys
from uuid import uuid4

parser = argparse.ArgumentParser()
parser.add_argument("campaign", type=Path)
args = parser.parse_args()
base = args.campaign.resolve()
destination = base / "build" / f"summary-gates-{uuid4().hex}"
destination.mkdir()
report_path = Path(__file__).with_name("build-pooled-report.py")
# The report is a CLI script. Load only its pure predicate without running CLI IO.
predicate = next(node for node in ast.parse(report_path.read_text()).body
                 if isinstance(node, ast.FunctionDef) and node.name == "strict_stage_pass")
namespace = {}
exec(compile(ast.Module(body=[predicate], type_ignores=[]), str(report_path), "exec"), namespace)
report_gate = namespace["strict_stage_pass"]
cases = (
    ("clean", "pooled-rec-24000-60s-1", (True, True, True)),
    ("RA-only", "pooled-rec-16000-60s-1", (True, True, False)),
    ("finish-reset", "pooled-rec-4000-60s-1", (True, False, False)),
    ("ring-loss", "pooled-auth-64000-60s-1", (False, True, False)),
)
checks = []
for label, run, expected in cases:
    source = base / "stages" / run / "stage-summary.json"
    original_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    output = destination / f"{run}.json"
    subprocess.run([sys.executable, str(Path(__file__).with_name("summarize-pooled-stage.py")),
                    str(base), run, "--output", str(output)], check=True,
                   stdout=subprocess.DEVNULL)
    summary = json.loads(output.read_text())
    actual = tuple(summary[key] for key in (
        "capture_event_loss_clean", "lifecycle_clean", "strict_stage_pass_before_wpr_review"))
    if actual != expected:
        raise AssertionError(f"{label}: expected {expected}, got {actual}")
    if hashlib.sha256(source.read_bytes()).hexdigest() != original_hash:
        raise AssertionError(f"Preserved summary changed: {run}")
    receiver = next(item for item in json.loads(
        (base / "ubuntu" / f"{run}-joined.json").read_text(encoding="utf-8-sig"))["runs"]
        if item["run"] == run)
    stop = json.loads((base / "windows-campaign" / f"{run}-Stop.json").read_text(encoding="utf-8-sig"))
    if isinstance(stop, list):
        stop = stop[0]
    trace_gate = json.loads((base / "wpa" / run / "trace-gate-v1.json").read_text())
    if report_gate(summary, receiver, stop, trace_gate) != expected[2]:
        raise AssertionError(f"Report strict predicate disagrees for {label}")
    if label == "clean":
        for field in ("client_delivery_clean", "capture_clean"):
            changed = dict(summary, **{field: False})
            if report_gate(changed, receiver, stop, trace_gate):
                raise AssertionError(f"Report falsely passes {field}=False")
        for field, value in (("State", "Running"), ("ProcessId", 42)):
            changed_stop = dict(stop, service=dict(stop["service"], **{field: value}))
            if report_gate(summary, receiver, changed_stop, trace_gate):
                raise AssertionError(f"Report falsely passes service {field}={value}")
    checks.append({"case": label, "run": run, "expected": expected, "actual": actual,
                   "original_summary_sha256": original_hash, "original_unchanged": True,
                   "report_predicate_agrees": True,
                   "derived_output": str(output.relative_to(base))})
receipt = {"utc": datetime.now(timezone.utc).isoformat(), "checks": checks, "passed": True,
           "report_predicate_mutations_rejected": ["extra client errors", "upstream/UDP loss", "running service", "nonzero stopped PID"],
           "report_source_sha256": hashlib.sha256(report_path.read_bytes()).hexdigest(),
           "scope": "Actual preserved clean, RA-only, finish-reset and ring-loss receipts; no load or service changes"}
(destination / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
print(destination / "receipt.json")
