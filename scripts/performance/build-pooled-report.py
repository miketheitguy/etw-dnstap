"""Build report tables from preserved stage evidence; raw receipts stay unchanged."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("campaign", type=Path)
parser.add_argument("output", type=Path)
args = parser.parse_args()
base = args.campaign


def read(path):
    return json.loads(path.read_text(encoding="utf-8-sig"))


def obj(path):
    value = read(path)
    return value[0] if isinstance(value, list) and len(value) == 1 else value


def table(headers, rows):
    return "\n".join(["| " + " | ".join(headers) + " |",
                       "| " + " | ".join("---" for _ in headers) + " |",
                       *("| " + " | ".join(map(str, row)) + " |" for row in rows)])


audit_path = base / "build" / "management-overlap-audit-v1.json"
audit = read(audit_path) if audit_path.exists() else {"runs": {}}
trace_path = base / "wpa" / "trace-gates-v1.json"
trace_gates = {item["run_id"]: item for item in read(trace_path)["gates"]} if trace_path.exists() else {}
rows, deficit_rows, resources, thread_rows, io_rows, trace_rows = [], [], [], [], [], []
for path in sorted((base / "stages").glob("*/stage-summary.json"),
                   key=lambda path: read(base / "client" / path.parent.name / "summary.json")["start_unix_ms"]):
    summary = read(path)
    run = summary["run"]
    client = read(base / "client" / run / "summary.json")
    analysis = read(base / "capture" / run / "analysis.json")
    service = analysis["processes"]["service"]
    stop = obj(base / "windows-campaign" / f"{run}-Stop.json")
    final = analysis["service_final"]
    qualifiers = []
    if audit["runs"].get(run, {}).get("in_load_dns_inventory_calls"):
        qualifiers.append("management observer")
    if (path.parent / "measurement-confound.json").exists():
        qualifiers.append("excluded threshold")
    if (path.parent / "lifecycle-assessment.json").exists():
        qualifiers.append("finish reset")
    if (path.parent / "temporary-data-disk.json").exists():
        qualifiers.append("receiver data volume")
    label = f"`{run}`" + (" (" + "; ".join(qualifiers) + ")" if qualifiers else "")
    dns_errors = summary["client_sent"] - summary["client_successful"]
    joined = read(base / "ubuntu" / f"{run}-joined.json")
    receiver = next(item for item in joined["runs"] if item["run"] == run)
    if summary["ring_dropped"]:
        operations = ("CLIENT_QUERY", "CLIENT_RESPONSE", "RESOLVER_QUERY", "RESOLVER_RESPONSE")
        expected = (summary["client_sent"], summary["client_sent"],
                    summary["upstream_queries"], summary["upstream_responses"])
        deficits = [count - receiver["captured_operations"].get(operation, 0)
                    for operation, count in zip(operations, expected)]
        if min(deficits) < 0 or sum(deficits) != summary["ring_dropped"]:
            raise ValueError(f"Operation deficits do not reconcile with ring loss: {run}")
        deficit_rows.append([label, *[f"{count:,}" for count in deficits],
                             f"{sum(deficits):,}"])
    rows.append([label, summary["mode"], int(summary["seconds"]),
                 f"{summary['requested_qps']:,} / {summary['achieved_qps']:,.2f}",
                 f"{dns_errors:,} (RA {summary['client_ra_missing']})",
                 f"{client['counts']['timeouts']} / {client['counts']['send_errors']+client['counts']['receive_errors']}",
                 f"{summary['upstream_queries']:,} / {summary['upstream_responses']:,} ({100*summary['upstream_queries']/summary['client_sent']:.2f}%)",
                 f"{summary['received']:,} / {summary['encoded']:,}",
                 f"{summary['ring_dropped']:,} / {summary['pool_exhausted']:,} / {summary['receiver_disconnected']:,}",
                 f"{final['output_errors']} / {stop['service']['ExitCode']}",
                 "exact" if receiver["receiver_exact_delivery_verified"] else "FAIL",
                 f"{summary['queue_high_water']:,}"])
    cpu = service["cpu_raw_time_weighted_percent"]
    private = service["counters"]["private bytes"]["max"]
    working = service["counters"]["working set"]["max"]
    resources.append([label, f"{cpu['mean']:.4f} / {cpu['mean']/2:.4f}",
                      f"{service['cpu_seconds_integrated']:.4f} / {cpu['valid_seconds']:.3f}",
                      analysis["window"]["rows"],
                      f"{summary['dns_cpu_one_core_percent']['mean']:.4f}",
                      f"{int(private):,} / {private/1048576:.3f}",
                      f"{int(working):,} / {working/1048576:.3f}",
                      f"{summary['latency_ns_upper']['p99_ns_upper']/1e6:.6f}"])
    counters = service["counters"]
    io_rows.append([label,
                    f"{int(counters['virtual bytes']['max']):,}",
                    f"{counters['page faults/sec']['mean']:.3f}",
                    f"{counters['io read bytes/sec']['mean']:.3f} / {counters['io write bytes/sec']['mean']:.3f}",
                    f"{counters['io read operations/sec']['mean']:.3f} / {counters['io write operations/sec']['mean']:.3f} / {counters['io other operations/sec']['mean']:.3f}",
                    f"{analysis['system']['memory\\available mbytes']['min']:.1f}",
                    f"{analysis['system']['system\\processor queue length']['max']:.0f}"])
    thread_rows.append([label,
                        f"{int(counters['thread count']['min'])}–{int(counters['thread count']['max'])}",
                        f"{int(counters['handle count']['min'])}–{int(counters['handle count']['max'])}",
                        service["thread_coverage"]["samples"],
                        service["thread_coverage"]["incomplete_samples"],
                        service["invalid_pid_samples"],
                        service["rejected_rate_values"].get("thread_identity_discontinuity", 0)])
    gate = trace_gates.get(run)
    if gate:
        exports = [item.parent.name for item in (base / "wpa" / run).glob("*/wpa-export-result.json")
                   if read(item).get("exit_code") == 0]
        trace_rows.append([label, f"{gate['canonical_etl_bytes']:,}",
                           f"{gate['retained_duration_s']:.6f}",
                           f"0–{gate['cswitch_coverage']['end_s']:.6f}; {'continuous' if gate['cswitch_coverage']['continuous'] else 'GAPS'}",
                           f"{gate['lost_events']} / {gate['lost_buffers']}",
                           "contained" if gate["client_interval_contained"] else "FAIL",
                           "pass" if gate["trace_gate_pass"] else "FAIL",
                           ", ".join(sorted(exports)) if exports else "none; ETL inspected"])

text = f"""# Pooled intake VM performance review

**Campaign incomplete.** This draft records completed native stages; not all
discovery, confirmation, clean baseline repeats, restoration, and final analysis
are finished. Do not
use an incomplete draft as an operational capacity guarantee.

This report helps maintainers assess the rebuilt pooled candidate under actual
Windows DNS traffic. See the [campaign plan](pooled-vm-performance-plan.md) for
delegation, approvals, exact commands, fixed workload, resource guards, and
restoration requirements. Evidence root: `{base.as_posix()}`. Table generation:
{datetime.now(timezone.utc).isoformat()}.

## Build and measured configuration

The locked release invocation, formatting, strict all-target Clippy, reviewed
source identities, actual PE checks, CLI defaults, and build receipt are in
`build`. Cargo reused the reviewed release artifacts; the resulting SHA-256 is
`98FDAB43AA24AE1BE31A7AABCCBDD67A2F64C87DC1C50C428B638B3C091F5D7C`.
The protected baseline `CA3342D8…55264347` remains preserved. Production source
and effective hardening settings stay unchanged during measurement.

The Windows target retains 2 vCPUs, fixed 4 GiB RAM, SCM-only LocalSystem,
enabled service SID, only `SeChangeNotifyPrivilege`, and protected executable/
SCM ACLs. Measured runs use default queue 8,192 and jumbo capacity 64, adding
only a unique metrics prefix. The host client uses two workers, 256 outstanding
requests each, a two-second deadline, and the same 5,000 A-name rotation. A
global requested rate is a ceiling; the achieved-rate gate is at least 98%.
These A workloads do not establish jumbo-pool or heavy DNSSEC capacity.

The authoritative confirmation and subsequent stages use the explicitly
approved temporary Ubuntu data volume for raw receiver JSON, with archives on
the original root volume. CPU, RAM, network, collector YAML, and Frame Streams
behavior stay fixed; only the collector's stdout path changes. Its 64 GiB VHDX
is attached at SCSI 0:2, fully initialized before load, and has filesystem UUID
`3046058a-a6b1-4e74-97f5-a63905494a1f`. Rows identify this receiver-storage
boundary; do not imply matched storage against earlier root-volume stages.

## Workload and final record ledgers

DNS validation failures include flag checks; an RA=false response was received
and is not a lost DNS query/response. Timeouts and send/receive socket errors
are shown separately. Upstream query/reply counts and queries as a percentage
of client sends expose each recursive stage's actual workload intensity;
repeated names and TTL zero do not imply one upstream query per client query.
Received/encoded are ETW records, not DNS packet counts. Intake columns are
ring-full / pool-exhausted / receiver-disconnected. Output/lifecycle columns
show output operation errors / SCM exit code. Exact receiver reconciliation
means every encoded record matches durable JSON and all four ingress/egress
counters, including correctly reconciled lossy stages.

{table(['Run / qualification','Mode','Seconds','Requested / achieved QPS','DNS validation failures (RA subset)','Timeouts / socket errors','Upstream Q / R (% Q/sent)','Received / encoded','Intake categories','Output / SCM','Receiver','HWM'], rows)}

For the lossy stages below, missing captured operations are the direct-client
or actual-upstream packet count minus the exact archived operation count.
Their sum equals native ring-admission loss. These are missing capture records;
the direct DNS client separately measures whether queries or responses were lost.

{table(['Run / qualification','Missing CLIENT_QUERY','Missing CLIENT_RESPONSE','Missing RESOLVER_QUERY','Missing RESOLVER_RESPONSE','Sum = ring drops'], deficit_rows)}

## CPU, memory, and latency

Service CPU is the duration-weighted PID-validated PDH mean, in percent of one
CPU and percent of this two-vCPU VM. CPU work and valid interval coverage are
separate. Memory peaks show bytes / MiB. Latency percentiles are histogram upper
bounds for matched responses, exclude timeouts, and have no coordinated-omission
correction. WPR scheduled CPU includes interrupt attribution unless explicitly
excluded and must not be forced to equal PDH or sampled CPU.

{table(['Run / qualification','Service CPU one core / VM %','CPU-s / valid s','Rows','DNS one-core %','Private peak B / MiB','Working peak B / MiB','p99 upper ms'], resources)}

The optional receiver visibility-delay distribution is unavailable: these exact
archive scans did not enable the monitor/provider-time calculation, and no
archive is rescanned to manufacture it. Preserved one-second file-size samples
show coarse output visibility; their cadence is not a measured per-packet
network latency or fsync durability bound. Direct DNS latency remains separate.

## Thread and handle coverage

All per-thread PID/TID, CPU/user/kernel rates, context-switch rates, state, and
wait-reason samples remain in the raw BLG/CSV and per-run `analysis.json`.
Consecutive identity transitions invalidate rates and are counted below rather
than interpreted as CPU spikes. Actual WPR thread lifetimes and CPU Precise
data supplement interval-sampled thread state; they do not recreate missing
application loss counters.

{table(['Run / qualification','Threads','Handles','Thread samples','Incomplete','Invalid PID','Rejected thread rates'], thread_rows)}

The following service I/O and page-fault rates are arithmetic means of valid
one-second PDH samples. Page faults include soft faults; they are not hard-fault
counts. Virtual bytes measure reserved address space, not committed or resident
memory. Guest available memory is in MiB. Process I/O counters do not establish
network record delivery or disk durability; the exact receiver ledger remains
the delivery evidence.

{table(['Run / qualification','Virtual peak B','Faults/s','Read / write B/s','Read / write / other ops/s','Guest available MiB min','CPU queue max'], io_rows)}

## Actual WPR evidence and limitations

Every stage retains an actual native ETL from the validated single full-stage
file-mode profile: CPU sampling, CSwitch/ReadyThread and stacks, process/thread
lifetimes, disk/file I/O, and hard faults, with 128 × 1,024 KiB buffers. Native
headers, event retention, lost-event/buffer counters, and continuous scheduling
coverage are inspected per run. See the separate
[trace analysis](pooled-vm-trace-analysis.md) and per-run `wpa` evidence.

An inspected zero-loss ETL is distinct from an optional WPA export. The initial
1,000 QPS stages have matched and broad exported views; the first 64,000 QPS
failure has focused early/late views. Other discovery stages have native
inspection gates and retained ETLs; their optional exports remain deferred.
The final coverage table records inspection and selected exports separately.

{table(['Run / qualification','ETL bytes','Header retained s','Actual scheduling coverage s','Lost events / buffers','Sending interval','Native trace gate','Successful optional WPA views'], trace_rows)}

The derivative `wpa/trace-gates-v1.json` records canonical hashes and exact
inspection source paths without changing any raw receipt. An exported view
samples its stated interval; it does not establish full-stage absence of waits,
locks, faults, or application loss.

The release has no matching PDB/CodeView record. Rust application function names
cannot be recovered reliably from unrelated symbols. Module/address and named
thread attribution remain available. File/hard-fault coverage with zero target
events is a measured zero, not a claim that the provider was disabled.

The initial 64,000 QPS stage lost 1,409,877 of 7,508,812 ETW records (18.7763%)
at channel admission while all 3,754,406 DNS requests completed valid. Loss
appears throughout all 60 sampled sending intervals. Focused native scheduling
shows substantial parser Ready time during persistent backlog; naming a TDH or
lock cause requires additional proof. Empty-queue waits in low-load traces
do not establish the cause of a full-queue high-rate failure.

## Measurement qualifications and lifecycle findings

The historical management audit, helper diff, and exact read-only fix proof are
preserved in `build/management-overlap-audit-v1.json` and
`build/status-helper-fix.diff`. Original 1,000 QPS runs and auth 8,000 QPS include
an in-load 5,000-record DNS management inventory. Their capture evidence remains
valid but CPU comparison is qualified. The original 48,000 QPS run is explicitly
perturbed and excluded from the intrinsic threshold bracket; its two drop
intervals overlap the management call envelope. Causation is not proven. Its
raw failure remains preserved, followed by one coordinator-authorized clean
replacement. Subsequent load uses only existing guards and native metric tails.

The first authoritative run's controller reported a null wrapper exit code;
the actual DNS client exit was zero and complete raw evidence reconciled. The
raw controller failure and `completion-assessment.json` remain preserved. The
retained-handle correction was checked with non-load child exit codes zero and
seven before later load.

The 56,000 QPS stage also has a distinct failed lifecycle: output operation
errors increased only at final finish, while every encoded record was delivered.
Application Event ID 1, Record 127, PID 4784, at 10:28:50.7720946 UTC records
STOP/FINISH exchange failure with Winsock error 10054. The measured final
operation lasted 0.4412 ms; SCM returned 1066 / service-specific 1. The receiver
stayed active and no operator teardown occurred. Its generic reset log also
appears on clean runs and alone proves no TCP reset. The reset's cause remains
unresolved; the same error chain exists in the pre-deployment baseline.
See `stages/pooled-auth-56000-60s-1/lifecycle-assessment.json` and raw event XML.

## Remaining validation and final restoration

The approved campaign sequence is authoritative confirmation, independent
recursive discovery/refinement/confirmation, the two clean 300-second 1,000 QPS
repeats, then restoration and final interpretation. Any temporary receiver data
disk and raw-path change must be documented and restored; do not imply matched
receiver storage across that boundary. Preserve all failed and perturbed runs.

Historical capacity-only CPU numbers are descriptive context only: the previous
partial memory-mode WPR recording and this full file-mode profile differ. No
causal CPU saving from pooling is established by these single trials. Final
findings require completed gates, exact achieved workload/upstream counts,
resource coverage, trace coverage, and restoration proof.
"""
args.output.write_text(text, encoding="utf-8")
print(args.output)
