"""Build report tables from preserved stage evidence; raw receipts stay unchanged."""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument("campaign", type=Path)
parser.add_argument("output", type=Path)
parser.add_argument("--complete", action="store_true",
                    help="Require 22 reconciled, classified stages, native trace integrity, and restoration proofs")
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


def strict_stage_pass(summary, receiver, stop, trace_gate):
    return (summary["all_dns_validity_pass"] and summary["client_delivery_clean"]
            and summary["capture_clean"] and receiver["capture_pipeline_zero_loss_verified"]
            and summary["achieved_fraction"] >= .98
            and summary["orchestrator_clean_after_explicit_assessment"]
            and stop["service"]["State"] == "Stopped"
            and stop["service"]["ProcessId"] == 0
            and stop["service"]["ExitCode"] == 0
            and trace_gate is not None and trace_gate["trace_gate_pass"])


audit_path = base / "build" / "management-overlap-audit-v1.json"
audit = read(audit_path) if audit_path.exists() else {"runs": {}}
trace_path = base / "wpa" / "trace-gates-v1.json"
trace_gates = {item["run_id"]: item for item in read(trace_path)["gates"]} if trace_path.exists() else {}
rows, deficit_rows, resources, thread_rows, io_rows, trace_rows, loss_rows = [], [], [], [], [], [], []
comparison_rows = []
lifecycle_rows = []
confirmation_rows = []
support_rows = []
strict_results = {}
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
        lifecycle = read(path.parent / "lifecycle-assessment.json")
        lifecycle_rows.append([
            f"`{run}`", lifecycle.get("pid", lifecycle.get("application_process_id")),
            lifecycle.get("event_record_id", lifecycle.get("application_event_record_id")),
            lifecycle.get("event_utc", lifecycle.get("application_event_utc")),
            f"{lifecycle.get('live_output_errors', 0)} / {lifecycle['final_output_errors']}",
            f"{lifecycle.get('final_output_time_increment_ns', lifecycle.get('finish_elapsed_ns'))/1e6:.4f}",
            f"{lifecycle['scm_exit_code']} / {lifecycle['service_specific_exit_code']}",
            f"{lifecycle['encoded_and_exact_receiver_records']:,}"])
    if (path.parent / "temporary-data-disk.json").exists():
        qualifiers.append("receiver data volume")
    label = f"`{run}`" + (" (" + "; ".join(qualifiers) + ")" if qualifiers else "")
    loss_rows.append([label, final['callback_errors'],
                      f"{final['ignored']} / {final['rejected']}",
                      f"{final['output_limit_dropped']} / {final['output_failed_events']}",
                      f"{final['etw_events_lost']} / {final['etw_realtime_buffers_lost']}",
                      f"{final['etw_stats_valid']} / {final['etw_stats_errors']}",
                      final['queue_backlog'],
                      f"{analysis['ledger']['capture_residual']} / {analysis['ledger']['parser_residual']}"])
    dns_errors = summary["client_sent"] - summary["client_successful"]
    joined = read(base / "ubuntu" / f"{run}-joined.json")
    receiver = next(item for item in joined["runs"] if item["run"] == run)
    if run in ("pooled-auth-48000-300s-1", "pooled-rec-28000-300s-1",
               "pooled-auth-1000-300s-2", "pooled-rec-1000-300s-2"):
        host = read(base / "client" / run / "host-analysis.json")
        host_cpu = host["cpu_raw_time_weighted_percent"]
        support_rows.append([
            f"`{run}`", f"{host_cpu:.3f} / {host_cpu/host['logical_processors']:.3f}",
            host["stats"]["cpu_raw_percent"]["n"],
            f"{host['stats']['private_bytes']['max']/1048576:.3f} / {host['stats']['working_set']['max']/1048576:.3f}",
            f"{host['stats']['host_cpu_percent']['mean']:.3f}",
            f"{receiver['collector_cpu_percent_one_core_estimated']:.3f}",
            f"{receiver['upstream_cpu_percent_one_core_estimated']:.3f}"])
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
    trace_gate = trace_gates.get(run)
    strict_pass = strict_stage_pass(summary, receiver, stop, trace_gate)
    strict_results[run] = strict_pass
    rows.append([label, summary["mode"], "PASS" if strict_pass else "FAIL",
                 int(summary["seconds"]),
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

historical = base.parent / "ring-fix-20261029"
for run in ("pooled-auth-48000-300s-1", "pooled-rec-28000-300s-1"):
    summary_path = base / "stages" / run / "stage-summary.json"
    if not summary_path.exists():
        continue
    summary = read(summary_path)
    cpu = summary["service_cpu_one_core_percent"]
    confirmation_rows.append([
        f"`{run}`", summary["mode"],
        f"{summary['requested_qps']:,} / {summary['achieved_qps']:,.2f}",
        f"{summary['client_sent']:,} / {summary['client_successful']:,}",
        f"{summary['upstream_queries']:,} / {summary['upstream_responses']:,} ({100*summary['upstream_queries']/summary['client_sent']:.2f}%)",
        f"{summary['received']:,} / {summary['encoded']:,}",
        f"{summary['encoded']/summary['seconds']:,.2f}",
        f"{cpu['mean']:.4f} / {cpu['mean']/2:.4f}",
        f"{cpu['valid_seconds']:.3f} / {summary['seconds']:.3f}",
        f"{summary['service_private_bytes']['max']/1048576:.3f}",
        f"{summary['latency_ns_upper']['p99_ns_upper']/1e6:.6f}"])
historical_receiver = read(historical / "ubuntu" / "receiver-results.json")["runs"]
for short in ("auth", "rec"):
    old_run = f"ringfix-capacity8192-{short}-300s-1"
    new_run = f"pooled-{short}-1000-300s-2"
    comparisons = [("capacity-only; partial memory-mode WPR", old_run,
                    historical / "windows" / old_run,
                    historical / "client" / old_run,
                    next(item for item in historical_receiver if item["run"] == old_run))]
    historical_p99 = read(historical / "client" / old_run / "summary.json")["latency"]["p99_ns_upper"]
    if (base / "stages" / new_run / "stage-summary.json").exists():
        comparisons.append(("pooled; full file-mode WPR; clean observer", new_run,
                            base / "capture" / new_run, base / "client" / new_run,
                            read(base / "ubuntu" / f"{new_run}-joined.json")["runs"][0]))
    for label, run, capture_path, client_path, receiver in comparisons:
        analysis = read(capture_path / "analysis.json")
        client = read(client_path / "summary.json")
        seconds = client["measured_send_duration_ns"] / 1e9
        sent = client["counts"]["sent"]
        process = analysis["processes"]["service"]
        cpu = process["cpu_raw_time_weighted_percent"]
        comparison_rows.append([
            f"`{run}` ({label})", client["options"]["mode"],
            f"{sent / seconds:,.2f}", f"{analysis['service_final']['encoded'] / seconds:,.2f}",
            f"{100 * receiver['upstream_queries_marker_interval'] / sent:.2f}%",
            f"{cpu['mean']:.4f} / {cpu['mean']/2:.4f}",
            f"{process['cpu_seconds_integrated']:.4f} / {cpu['valid_seconds']:.3f}",
            f"{process['counters']['private bytes']['max']/1048576:.3f}",
            f"{client['latency']['p99_ns_upper']/1e6:.6f}",
            f"{client['latency']['p99_ns_upper']/historical_p99:.3f}x",
            sent - client["counts"]["successful"]])

if args.complete:
    if not all(strict_results.get(run) for run in (
            "pooled-auth-48000-300s-1", "pooled-rec-28000-300s-1")):
        raise ValueError("Completion headline requires both high-rate confirmations to strictly pass")
    stage_runs = {path.parent.name for path in (base / "stages").glob("*/stage-summary.json")}
    if len(stage_runs) != 22 or any(not trace_gates.get(run, {}).get("trace_gate_pass") for run in stage_runs):
        raise ValueError("Completion requires 22 classified stage summaries and all native trace integrity gates")
    for run in stage_runs:
        summary = read(base / "stages" / run / "stage-summary.json")
        receiver = next(item for item in read(base / "ubuntu" / f"{run}-joined.json")["runs"]
                        if item["run"] == run)
        if (receiver["receiver_exact_delivery_verified"] is not True
                or receiver["event_denominator_domain_closure_verified"] is not True
                or summary["encoded"] != summary["durable_json"]
                or any(receiver["service_ledger"][key] != 0 for key in (
                    "intake_residual", "capture_residual", "parser_residual"))):
            raise ValueError(f"Unclosed stage evidence: {run}")
        stop = obj(base / "windows-campaign" / f"{run}-Stop.json")
        if stop["service"]["ExitCode"] and not (base / "stages" / run / "lifecycle-assessment.json").exists():
            raise ValueError(f"Unclassified failed lifecycle: {run}")
    windows_proof = read(base / "windows-campaign" / "final-policy-proof.json")
    if (windows_proof["security_matches_initial_inventory"] is not True
            or windows_proof["guest"]["service"]["State"] != "Running"
            or "--metrics" in windows_proof["guest"]["service"]["PathName"]):
        raise ValueError("Completion requires restored normal Windows policy")
    for name in ("restoration-final.json", "restoration-console-proof.json",
                 "temporary-data-disk-unmounted.json", "temporary-data-disk-detached.json"):
        if not (base / "ubuntu" / name).is_file():
            raise ValueError(f"Missing final Ubuntu proof: {name}")
    ubuntu_proof = read(base / "ubuntu" / "restoration-final.json")
    if (ubuntu_proof["config_sha256"] != ubuntu_proof["protected_backup_sha256"]
            or any(ubuntu_proof[key] is not True for key in (
                "config_matches_protected_baseline", "performance_dropin_absent",
                "temporary_telemetry_listener_absent", "temporary_upstream_listener_absent",
                "temporary_data_device_absent", "temporary_mount_absent", "raw_file_absent",
                "evidence_retained"))
            or ubuntu_proof["unit_states"] != {
                "dnscollector": "active", "dnstap-performance-monitor": "inactive",
                "dnstap-performance-upstream": "inactive"}):
        raise ValueError("Ubuntu normal receiver/settings were not restored")
    unmount = read(base / "ubuntu" / "temporary-data-disk-unmounted.json")
    detach = read(base / "ubuntu" / "temporary-data-disk-detached.json")
    if (unmount["unmounted"] is not True or unmount["no_remaining_mount_for_uuid"] is not True
            or unmount["umount_exit_code"] != 0 or unmount["open_fd_matches"]
            or detach["action"] != "Detach" or detach["vhdx_retained"] is not True
            or unmount["vhdx_path"] != detach["vhdx_path"]
            or unmount["vm_guid"] != detach["vm_guid"]):
        raise ValueError("Temporary receiver storage was not cleanly detached and retained")
    console = read(base / "ubuntu" / "restoration-console-proof.json")
    vm_proof = read(base / "ubuntu" / "restoration-vm-proof.json")
    if (console["normal_console_query_response_verified"] is not True
            or console["windows_service_pid"] != windows_proof["guest"]["service"]["ProcessId"]
            or console["collector_pid"] != ubuntu_proof["collector_pid"]
            or vm_proof["both_vms_running"] is not True
            or vm_proof["fixed_resources_unchanged"] is not True):
        raise ValueError("Missing verified normal console exchange or fixed running VMs")
    completion_status = f"""**Bounded campaign complete.** Both five-minute high-rate confirmations pass
all gates. The 22 measured stages retain **{sum(strict_results.values())} strict passes and
{len(strict_results) - sum(strict_results.values())} strict failures**, including overload and DNS/lifecycle qualifications.
All 22 native trace gates pass; that separate result does not clear a strict
stage failure. The two 1,000 QPS repeats exclude the earlier in-load management
inventory. At campaign restoration, normal settings were restored, the reviewed
candidate was retained, and both VMs were Running. These observations are not
an operational capacity guarantee."""
    restoration_text = """## Restoration and validation

`windows-campaign/final-policy-proof.json` verifies the running SCM service,
retained `98FD…1F5D7C` binary, original normal arguments without metrics/capacity
overrides, LocalSystem identity, service SID, only `SeChangeNotifyPrivilege`,
and exact executable/SCM ACL matches. DNS recursion, cache, and forwarder settings
match the protected baseline; all 5,000 authoritative A records remain and only
the owned temporary recursive forwarder is removed. No WPR recording or
campaign counter collector remains. `windows-campaign/restoration-final.json`
and `windows-campaign/final-query-proof.json` retain the restoration and normal
authoritative answer with TTL 300.

Ubuntu restoration precedes Windows normal SCM startup. Its protected collector
configuration, stdout destination, socket sysctls, temporary firewall/workload
fixtures, and normal services are restored. The verified temporary data volume
is cleanly unmounted and detached while its VHDX file is retained. See
`ubuntu/restoration-final.json`, `ubuntu/temporary-data-disk-unmounted.json`, and
`ubuntu/temporary-data-disk-detached.json`. Raw journal CQ/CR and the exact normal
query assessment remain in `ubuntu/restoration-console-proof.txt` and `.json`.
`ubuntu/restoration-vm-proof.json` verifies both fixed-resource running VMs.
Those receipts record both original VMs Running with unchanged resources; historical traces,
archives, configurations, and failed-stage evidence remain preserved.

The pre-load formatting, strict Clippy, locked release and PE checks remain in
`build`; source and binary stay unchanged during measurements. The focused
summary regression checks actual clean, RA-only, final-reset and ring-loss
receipts in fresh output files and verifies original summary hashes unchanged;
all four cases pass (`build/summary-gates-check-20261030-v2.json`). The report
predicate also rejects independent extra-client-error, upstream-loss, running
service, and nonzero-stopped-PID cases derived from a clean receipt. The diagnostic
helper's boolean argument array correction changes only post-load selection;
raw diagnostic outputs remain preserved. Report generation and local Markdown
links are checked after final integration. The optional receiver visibility-delay
distribution remains unavailable, and no receiver archive is rescanned for it.
"""
else:
    completion_status = """**Campaign incomplete.** This draft records completed native stages; not all
clean baseline repeats, restoration, and final analysis are finished. Do not
use an incomplete draft as an operational capacity guarantee."""
    restoration_text = """## Remaining validation and final restoration

Complete the two authorized 300-second 1,000 QPS repeats without management
inventory during sending, then restore the Ubuntu normal receiver before
restoring/starting the Windows normal SCM service. Verify policy, retained hash,
ordinary answer/TTL, console CQ/CR, both running VMs, and detached temporary data
disk. Preserve all failed and perturbed runs. The final report requires actual
restoration receipts and all 22 native trace gates.
"""

text = f"""# Pooled intake VM performance review

This report helps maintainers assess pooled Event Tracing for Windows (ETW)
capture under actual Windows DNS traffic.

This record describes historical binaries with service JSONL metrics
instrumentation. The TCP-only service has no durable metrics export; its
instrumentation and performance have not been measured by this campaign.
See the [instrumentation compatibility note](README.md). Preserved counters,
tables, hashes, and collector artifacts retain their historical meaning.

{completion_status}

See the [campaign plan](pooled-vm-performance-plan.md) for
delegation, approvals, exact commands, fixed workload, resource guards, and
restoration requirements. Evidence root: `{base.as_posix()}`. Table generation:
{datetime.now(timezone.utc).isoformat()}.

Use [Measurement terms and result gates](measurement-terms.md) for QPS, record
counts, CPU units, and the independent DNS/capture/lifecycle/trace outcomes.
For the four primary runs, start with [the graphs](#wpr-cpu-and-achieved-qps-over-time);
for every stage, see [the final record ledgers](#workload-and-final-record-ledgers).
Paths such as `build`, `wpa`, and `ubuntu` are relative to the evidence root.

This page is generated by
[`build-pooled-report.py`](../scripts/performance/build-pooled-report.py).
Update its narrative template and regenerate from the preserved campaign
receipts; keep raw evidence unchanged.

## Observed five-minute confirmations

Each confirmation sends for 300 seconds on the fixed two-vCPU/4 GiB Windows
guest. Both have zero event loss, zero final backlog and output errors, valid
DNS responses, clean Service Control Manager (SCM) exit, exact receiver ledgers,
and full zero-loss Windows Performance Recorder (WPR) coverage. CPU counters use
Windows Performance Data Helper (PDH). These are observed sustained results for the stated A-record and
recursive upstream mix, not universal capacity guarantees.

{table(['Run','Mode','Requested / achieved DNS QPS','Queries sent / valid','Actual upstream Q / R (% of sends)','Received / encoded ETW records','Encoded records / sending s','CPU one core / VM %','PDH valid / send s','Private peak MiB','DNS p99 upper ms'], confirmation_rows)}

The ETW rates divide complete final record counts, including drain, by sending
duration. PDH CPU uses only validated intervals inside the sending window.
These windows differ slightly; no partial-window CPU integral is divided by
whole-run records to claim an exact CPU cost per record. Recursive TTL zero
does not force a cache miss per repeated name.

The next higher short stages lose capture records: authoritative 56,000 requested
QPS drops 65,155 of 6,593,410 received events (0.9882%); recursive 32,000 drops
7,535 of 4,379,440 (0.1721%). Their direct DNS clients still complete all queries
validly. These fixed-order observations bracket this campaign's clean capture
results; they do not establish universal platform capacity or DNS packet-loss
limits. The perturbed original authoritative 48,000 QPS stage remains excluded
from the intrinsic bracket and is preserved separately.

## WPR CPU and achieved QPS over time

The four primary five-minute runs have matched one-second WPR process CPU and
client query-rate graphs for sending seconds 5–295. CPU uses percent of one core
on the two-vCPU, 4 GiB guest; the upper panels show native scheduled process CPU
excluding observed deferred procedure calls and interrupt service routines
(DPC/ISR), distinct from the PDH tables in this report. The
lower panels separate achieved client sends from the requested rate ceiling.
Approximate clock alignment and counter interpolation limit timing precision.
See the [graph methods, numeric CSVs, provenance, and reproduction command](pooled-vm-performance-graphs.md).
The recursive baseline retains one RA=false response and remains a strict
failure despite zero DNS delivery loss and capture loss.

![Authoritative 1,000 QPS baseline: WPR service and DNS CPU with achieved client QPS over sending seconds 5–295.](assets/performance/pooled-auth-1000-300s-2.svg)

![Recursive 1,000 QPS baseline: WPR process CPU and achieved client QPS; one RA=false response remains a strict failure.](assets/performance/pooled-rec-1000-300s-2.svg)

![Authoritative 48,000 requested QPS confirmation: WPR process CPU and achieved client QPS; full-sending mean 47,303.24 QPS.](assets/performance/pooled-auth-48000-300s-1.svg)

![Recursive 28,000 requested QPS confirmation: WPR process CPU and achieved client QPS; full-sending mean 27,758.24 QPS.](assets/performance/pooled-rec-28000-300s-1.svg)

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
show output operation errors / SCM exit code. HWM means observed queue high-water
mark in events. Exact receiver reconciliation
means every encoded record matches durable JSON and all four ingress/egress
counters, including correctly reconciled lossy stages. A strict stage pass
requires all DNS flags, achieved rate, loss, lifecycle, receiver, orchestration,
and native trace gates; the documented continuation qualifications do not turn
a failed stage into a strict pass.

{table(['Run / qualification','Mode','Strict stage','Seconds','Requested / achieved QPS','DNS validation failures (RA subset)','Timeouts / socket errors','Upstream Q / R (% Q/sent)','Received / encoded','Intake categories','Output / SCM','Receiver','HWM'], rows)}

For the lossy stages in this table, missing captured operations are the direct-client
or actual-upstream packet count minus the exact archived operation count.
Their sum equals native ring-admission loss. These are missing capture records;
the direct DNS client separately measures whether queries or responses were lost.

{table(['Run / qualification','Missing CLIENT_QUERY','Missing CLIENT_RESPONSE','Missing RESOLVER_QUERY','Missing RESOLVER_RESPONSE','Sum = ring drops'], deficit_rows)}

The remaining native loss categories and final closure are shown separately.
Ignored and rejected records are parser outcomes; output operation errors appear
in the workload table because a failed final exchange can occur after every
encoded record reaches the receiver. A valid final ETW statistics query with
zero cumulative loss establishes the final ETW loss gate; transient statistics
query errors remain visible. Both capture and parser ledger residuals must be zero.

{table(['Run / qualification','Callback errors','Ignored / rejected','Output limit / failed events','ETW events / buffers lost','Final stats valid / query errors','Final backlog','Capture / parser residual'], loss_rows)}

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

The generator and Ubuntu resource measurements for the primary 300-second runs
are shown separately. Host client CPU is weighted over sampled sending-window
intervals and normalized to one core and the 24-logical-processor host. Memory
columns are sampled peaks. Host total CPU is an arithmetic sample mean. Ubuntu
collector/upstream CPU estimates interpolate one-second Ubuntu samples at the
client sending boundaries. They remain distinct from the Windows PDH-valid
intervals and are not exact common-window or CPU-per-record measurements.

{table(['Run','Host client CPU one core / host %','CPU intervals','Client private / working peak MiB','Host total CPU mean %','Ubuntu collector one-core % estimated','Ubuntu upstream one-core % estimated'], support_rows)}

## Descriptive comparison with the capacity-only baseline

The historical 300-second capacity-only runs and the clean pooled 1,000 QPS
repeats use the same client settings and two-vCPU/4 GiB Windows VM. The table
shows actual event rates and upstream fractions because nominal QPS alone does
not establish equal recursive work. The historical WPR trace retains only part
of its workload in memory mode; the pooled runs record the full stage in file
mode with 128 MiB configured WPR buffers. Receiver storage also differs. These
single trials describe resource use under their stated methods and do not
establish a causal CPU saving from pooling, a run-to-run variance, or a reason
to shrink either pool or queue. The larger pooled private-memory footprint
includes its preallocated storage; process private bytes are distinct from
the counted 37.32 MiB pool/channel budget.

{table(['Run / instrumentation','Mode','Actual QPS','Encoded records/s','Upstream Q / client sent','Service CPU one core / VM %','CPU-s / valid s','Private peak MiB','DNS p99 upper ms','p99 / historical','DNS validity failures'], comparison_rows)}

The preserved [capacity-only report](ring-buffer-fix-validation.md) supplies
the historical method and limitations. Original pooled 1,000 QPS runs remain
in the full tables with their management-observer qualification; the clean
repeats provide the primary pooled baseline resource measurements.

## Thread and handle coverage

All per-thread PID/TID, CPU/user/kernel rates, context-switch rates, state, and
wait-reason samples remain in the raw BLG/CSV and per-run `analysis.json`.
Consecutive identity transitions invalidate rates and are counted in this table rather
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

The clean low-rate repeats' matched 20-second views retain median positive
parser Wait intervals of about 15.95 ms. These are scheduler waits, not DNS
latency or proof of the source's configured 2 ms park deadline. The continuously
dropping authoritative 64k late view has substantial parser Ready time and no
positive Wait observations; recursive 32k loss-centered views contain both
Ready and Wait. Those wider windows cannot identify the exact overflowing burst
or establish a TDH lock, timer, driver, or sole scheduler cause. See the measured
[primary scheduling comparisons](pooled-vm-trace-analysis.md#primary-five-minute-comparisons)
and [recursive loss windows](pooled-vm-trace-analysis.md#recursive-32k-loss-centered-windows).

Corrected 290-second I/O views retain one 4 KiB service read/hard fault in
`tdh.dll` during the authoritative repeat and 272 DNS pagefile reads/faults
totaling 10,006,528 bytes during the recursive confirmation. The other primary
service/DNS pairs have zero in-view disk events or hard faults. Explicit event
time filtering excludes WPA rows returned outside the requested view. Observed
service FileIo writes point to metrics JSONL; cached writes and TCP output have
different accounting. These observations do not establish an I/O bottleneck.
Exact sources, thread lifetimes and boundary qualifications remain in the
[trace tables](pooled-vm-trace-analysis.md#primary-five-minute-comparisons) and
`wpa/native-comparison-v1.json`; all ten comparison views and 44 native actions
complete successfully.

The initial 64,000 QPS stage lost 1,409,877 of 7,508,812 ETW records (18.7763%)
at channel admission while all 3,754,406 DNS requests completed valid. Loss
appears throughout all 60 sampled sending intervals. Focused native scheduling
shows substantial parser Ready time during persistent backlog; naming a TDH or
lock cause requires additional proof. Empty-queue waits in low-load traces
do not establish the cause of a full-queue high-rate failure.

## Measurement qualifications and lifecycle findings

An approximately twelve-hour idle gap follows the first 18 completed stages.
Read-only recovery at 2026-10-30 00:03 UTC verifies the exact VM/resources/hash,
stopped SCM service, no active WPR or campaign counter/load process, and no
Windows 28,000 QPS artifacts. The unused Ubuntu READY epoch has only 1,026 raw
bytes and no connection; its files remain under `ubuntu/stale-ready-20261029-1141`.
The pending midpoint receives fresh READY and clock bounds before load. Neither
VM is restarted, and no interrupted Windows load is presented as completed.
See `recovery-preflight-20261030.json` and the preserved fresh stage budgets.

Post-load runner review identifies a separate WPR cleanup limitation: native
start can succeed before the helper writes ownership and returns to the
controller's `traceStarted` assignment. A failure in that window can leave a
recording active. The generic marker is insufficient to identify a live
recording safely; no stale-marker cancellation or unvalidated cleanup change is
introduced. Future recovery must preserve state and verify current ownership
before stopping anything. This error path is not observed in these stages;
recovery/final checks show no recording left active. See
`build/wpr-cleanup-review-20261030.json` and the campaign plan.

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

The following stages have a distinct failed lifecycle: operation errors increase
only at final finish, while every encoded record reaches the exact receiver
archive. Each raw Application Event ID 1 matches the service PID and final time
and records the exact STOP/FINISH Winsock 10054 chain. SCM returns 1066 with
service-specific code 1. Final native/parser ledgers close with zero backlog;
these sub-millisecond final operations do not indicate the two-second deadline.
The coordinator permits continuation only under this exact documented exception;
each stage remains a strict lifecycle failure.

{table(['Run','PID','Application record','Event UTC','Live / final operation errors','Final operation ms','SCM / specific exit','Encoded = exact receiver'], lifecycle_rows)}

The receiver stays active through each stable end marker. Its pinned effective
configuration has `reset-conn=true`; source inspection shows `SetLinger(0)` after
FINISH flush, consistent with this final-only mechanism. A reset log appears on
clean runs too, and source/configuration evidence does not prove the exact wire
cause of any occurrence. The same error chain exists in the pre-deployment
baseline. No timeout, Frame Streams option, code, or FINISH validation is changed.
See each `stages/RUN/lifecycle-assessment.json`, raw event XML, and the independent
[trace and source analysis](pooled-vm-trace-analysis.md).

{restoration_text}
"""
args.output.write_text(text.rstrip() + "\n", encoding="utf-8")
print(args.output)
