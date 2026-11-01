"""Summarize one preserved native stage without changing its raw evidence."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from metrics_ledger import normalize_metrics
from perf_common import read_json as read

parser = argparse.ArgumentParser()
parser.add_argument("campaign", type=Path)
parser.add_argument("run")
parser.add_argument("--output", type=Path,
                    help="Write a derivative summary to a fresh path instead of the preserved stage path")
args = parser.parse_args()
base, run = args.campaign, args.run


client = read(base / "client" / run / "summary.json")
capture = read(base / "capture" / run / "analysis.json")
host = read(base / "client" / run / "host-analysis.json")
joined = read(base / "ubuntu" / f"{run}-joined.json")
joined_run = next(item for item in joined["runs"] if item["run"] == run)
final = normalize_metrics(joined_run["service_final_metrics"])
counts = client["counts"]
seconds = client["measured_send_duration_ns"] / 1e9
rate = counts["sent"] / seconds
requested = client["options"]["qps"]
non_ra_errors = {key: counts[key] for key in (
    "timeouts", "send_errors", "receive_errors", "malformed", "mismatched",
    "unsolicited", "late", "duplicate", "aa_missing", "rd_mismatch",
    "truncated", "noerror_without_answers") if counts[key]}
bad_rcodes = {str(index): value for index, value in enumerate(counts["rcode"])
              if index and value}
worker_errors = [item["error"] for item in client["workers"] if item["error"]]
client_delivery_clean = (not non_ra_errors and not bad_rcodes and not worker_errors
                         and counts["completed"] == counts["sent"]
                         and client["outstanding_final"] == 0)
capture_clean = (joined_run["capture_pipeline_zero_loss_verified"] is True
                 and joined_run["upstream_socket_drop_delta_marker_interval"] == 0
                 and joined_run["udp_receive_buffer_errors_marker_interval"] == 0)
capture_event_loss_clean = (
    joined_run["event_denominator_domain_closure_verified"] is True
    and joined_run["receiver_exact_delivery_verified"] is True
    and joined_run["upstream_socket_drop_delta_marker_interval"] == 0
    and joined_run["udp_receive_buffer_errors_marker_interval"] == 0
    and final["etw_stats_valid"] is True
    and all(final[key] == 0 for key in (
        "intake_dropped", "callback_errors", "queue_backlog", "ignored",
        "rejected", "output_limit_dropped", "output_failed_events",
        "etw_events_lost", "etw_realtime_buffers_lost"))
    and all(joined_run["service_ledger"][key] == 0 for key in (
        "intake_residual", "capture_residual", "parser_residual")))
stop = read(base / "windows-campaign" / f"{run}-Stop.json")
if isinstance(stop, list):
    stop = stop[0]
lifecycle_clean = (final["output_errors"] == 0
                   and stop["service"]["ExitCode"] == 0
                   and stop["service"]["State"] == "Stopped"
                   and stop["service"]["ProcessId"] == 0)
guards = read(base / "stages" / run / "completion.json")
orchestrator_assessment = base / "stages" / run / "completion-assessment.json"
correction = read(orchestrator_assessment) if orchestrator_assessment.exists() else None
orchestrator_clean = guards["failure"] is None or (
    correction is not None and correction["classification"].startswith("orchestrator metadata false failure")
    and correction["actual_client_exit_code"] == 0)
service = capture["processes"]["service"]
dns = capture["processes"]["dns"]
summary = {
    "run": run, "mode": client["options"]["mode"], "seconds": seconds,
    "requested_qps": requested, "achieved_qps": rate,
    "achieved_fraction": rate / requested,
    "client_sent": counts["sent"], "client_successful": counts["successful"],
    "client_delivery_clean": client_delivery_clean,
    "all_dns_validity_pass": counts["successful"] == counts["sent"],
    "client_ra_missing": counts["ra_missing"], "client_other_errors": non_ra_errors,
    "client_bad_rcodes": bad_rcodes, "client_worker_errors": worker_errors,
    "capture_clean": capture_clean,
    "capture_event_loss_clean": capture_event_loss_clean,
    "lifecycle_clean": lifecycle_clean,
    "strict_stage_pass_before_wpr_review": (
        capture_clean and lifecycle_clean and counts["successful"] == counts["sent"]
        and client_delivery_clean and rate >= requested * .98 and orchestrator_clean),
    "orchestrator_clean_after_explicit_assessment": orchestrator_clean,
    "ascent_clean_before_wpr_review": (capture_clean and client_delivery_clean
                                       and rate >= requested * .98 and orchestrator_clean),
    "latency_ns_upper": client["latency"],
    "received": final["received"], "encoded": final["encoded"],
    "durable_json": joined_run["durable_json_records"],
    "intake_dropped": final["intake_dropped"], "ring_dropped": final["ring_dropped"],
    "pool_exhausted": final["pool_exhausted"],
    "receiver_disconnected": final["receiver_disconnected"],
    "queue_high_water": final["queue_high_water_observed"],
    "etw_events_lost": final["etw_events_lost"],
    "upstream_queries": joined_run["upstream_queries_marker_interval"],
    "upstream_responses": joined_run["upstream_responses_marker_interval"],
    "service_cpu_one_core_percent": service["cpu_raw_time_weighted_percent"],
    "service_cpu_guest_capacity_percent": service["cpu_guest_time_weighted_percent"],
    "service_cpu_seconds_integrated": service["cpu_seconds_integrated"],
    "service_private_bytes": service["counters"]["private bytes"],
    "service_working_set": service["counters"]["working set"],
    "dns_cpu_one_core_percent": dns["cpu_raw_time_weighted_percent"],
    "host_client_cpu_one_core_percent": host["cpu_raw_time_weighted_percent"],
    "ubuntu_collector_cpu_one_core_percent": joined_run["collector_cpu_percent_one_core_estimated"],
    "wpr_review_required": True,
}
output = args.output or base / "stages" / run / "stage-summary.json"
if output.exists():
    raise SystemExit("Preserve existing stage summary")
output.write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps(summary, indent=2))
