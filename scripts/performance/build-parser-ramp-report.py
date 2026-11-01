#!/usr/bin/env python3
"""Build a receipt-gated Markdown and JSON report for the parser DNS ramp."""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
import re
import sys
from typing import Any
from perf_common import sha256_file as sha256


RUN_ID = re.compile(r"^(?:parser(?:-filtered)?|filtered)-(auth|rec)-(\d+)-(\d+)s-(\d+)$")
REQUIRED_FILES = (
    "client-exit.json",
    "completion.json",
    "service-stop.json",
    "client/summary.json",
    "receiver/canonical-final.json",
)
FLAG_KEYS = (
    "timeouts",
    "send_errors",
    "receive_errors",
    "malformed",
    "mismatched",
    "aa_missing",
    "ra_missing",
    "rd_mismatch",
    "truncated",
    "noerror_without_answers",
    "late",
    "duplicate",
    "unsolicited",
)


def read_json(path: Path) -> dict[str, Any]:
    try:
        value = json.loads(path.read_text(encoding="utf-8-sig"))
    except (OSError, json.JSONDecodeError) as exc:
        raise ValueError(f"Cannot read valid JSON from {path}: {exc}") from exc
    if not isinstance(value, dict):
        raise ValueError(f"Expected a JSON object in {path}")
    return value


def number(source: dict[str, Any], key: str, where: str) -> int:
    value = source.get(key)
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise ValueError(f"Missing or invalid integer {where}.{key}")
    return value


def pending(run_id: str, reasons: list[str]) -> dict[str, Any]:
    return {"run_id": run_id, "status": "pending", "reasons": reasons}


def diagnostic_lifecycle(stage: Path, ledger: dict[str, Any], pointer: dict[str, Any],
                         client_exit: dict[str, Any], completion: dict[str, Any],
                         service_stop: dict[str, Any], options: dict[str, Any],
                         client_counts: dict[str, Any]) -> tuple[list[str], dict[str, Any]]:
    run_id = stage.name
    receiver = stage / "receiver"
    proof_names = (
        "start-marker.json",
        "end-marker.json",
        "archive-manifest.json",
        "transfer-roundtrip.json",
        "raw-removal.json",
        "receiver-raw.jsonl.gz",
    )
    missing = [name for name in proof_names if not (receiver / name).is_file()]
    if not (stage / "end-marker.json").is_file():
        missing.append("../end-marker.json")
    if missing:
        reasons = [f"missing diagnostic proof {name}" for name in missing]
        operations = ledger.get("operation_counts")
        if not isinstance(operations, dict):
            reasons.append("diagnostic ledger has no operation counts")
        else:
            for key in ("CLIENT_QUERY", "CLIENT_RESPONSE", "RESOLVER_QUERY", "RESOLVER_RESPONSE"):
                value = operations.get(key)
                if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                    reasons.append(f"diagnostic operation count {key} is missing or invalid")
        client_counts = ledger.get("client_counts")
        if not isinstance(client_counts, dict):
            reasons.append("diagnostic ledger has no client counts")
        else:
            for key in ("sent", "completed", "successful", "timeouts"):
                value = client_counts.get(key)
                if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                    reasons.append(f"diagnostic client count {key} is missing or invalid")
        return reasons, {}

    reasons: list[str] = []
    if ledger.get("schema_version") != 1 or ledger.get("run_id") != run_id:
        reasons.append("diagnostic ledger schema or run ID is invalid")
    if ledger.get("strict_pass") is not False or pointer.get("strict_pass") is not False:
        reasons.append("diagnostic ledger is not explicitly marked strict_pass=false")
    if pointer.get("clean") is not False:
        reasons.append("diagnostic canonical pointer is not explicitly clean=false")
    if client_exit.get("exit_code") != 0:
        reasons.append("client process did not exit successfully")
    if completion.get("run") != run_id or completion.get("service_started") is not True:
        reasons.append("completion receipt does not confirm this started run")
    if not isinstance(completion.get("failure"), str) or not completion["failure"]:
        reasons.append("diagnostic lifecycle failure is not recorded in completion receipt")
    if (service_stop.get("State") != "Stopped" or service_stop.get("ProcessId") != 0
            or service_stop.get("ExitCode") == 0):
        reasons.append("diagnostic stage does not show a stopped service with a failed SCM result")

    for key in ("run", "mode", "duration_seconds", "qps"):
        expected = run_id if key == "run" else options.get({"mode": "mode", "duration_seconds": "duration", "qps": "qps"}[key])
        if completion.get(key) != expected:
            reasons.append(f"completion {key} does not match client summary")
    if options.get("mode") != ledger.get("mode"):
        reasons.append("diagnostic mode does not match client mode")
    nested_completion = ledger.get("completion_receipt")
    if not isinstance(nested_completion, dict):
        reasons.append("diagnostic ledger has no nested completion receipt")
    else:
        for key in ("run", "mode", "duration_seconds", "qps", "failure"):
            if nested_completion.get(key) != completion.get(key):
                reasons.append(f"diagnostic nested completion {key} does not match stage receipt")

    nested_stop = ledger.get("service_stop_receipt")
    if not isinstance(nested_stop, dict):
        reasons.append("diagnostic ledger has no nested service-stop receipt")
    else:
        for key in ("Name", "State", "ExitCode", "ServiceSpecificExitCode", "ProcessId"):
            if nested_stop.get(key) != service_stop.get(key):
                reasons.append(f"diagnostic nested service-stop {key} does not match stage receipt")

    counts = ledger.get("client_counts")
    operations = ledger.get("operation_counts")
    if not isinstance(counts, dict) or not isinstance(operations, dict):
        reasons.append("diagnostic ledger has no client or operation counts")
    else:
        for key in ("sent", "completed", "successful", "timeouts"):
            value = counts.get(key)
            if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                reasons.append(f"diagnostic client count {key} is missing or invalid")
            if client_counts.get(key) != value:
                reasons.append(f"diagnostic client {key} does not match client summary")
        for key in FLAG_KEYS:
            if key in client_counts and counts.get(key) != client_counts.get(key):
                reasons.append(f"diagnostic client flag {key} does not match client summary")
        for key in ("CLIENT_QUERY", "CLIENT_RESPONSE", "RESOLVER_QUERY", "RESOLVER_RESPONSE"):
            if isinstance(operations.get(key), bool) or not isinstance(operations.get(key), int) or operations.get(key) < 0:
                reasons.append(f"diagnostic operation count {key} is missing or invalid")
        if all(isinstance(operations.get(key), int) and not isinstance(operations.get(key), bool)
               and operations.get(key) >= 0
               for key in ("CLIENT_QUERY", "CLIENT_RESPONSE", "RESOLVER_QUERY", "RESOLVER_RESPONSE")) and all(
                   isinstance(counts.get(key), int) and not isinstance(counts.get(key), bool) and counts.get(key) >= 0
                   for key in ("sent", "completed")):
            query_residual = counts["sent"] - operations["CLIENT_QUERY"]
            response_residual = counts["completed"] - operations["CLIENT_RESPONSE"]
            recorded = ledger.get("client_directional_residuals")
            if not isinstance(recorded, dict) or recorded.get("sent_minus_CLIENT_QUERY") != query_residual or recorded.get("completed_minus_CLIENT_RESPONSE") != response_residual:
                reasons.append("diagnostic directional residuals do not match counts")
            expected_rows = sum(operations[key] for key in ("CLIENT_QUERY", "CLIENT_RESPONSE", "RESOLVER_QUERY", "RESOLVER_RESPONSE"))
            if ledger.get("json_rows") != expected_rows:
                reasons.append("diagnostic operation counts do not match JSON row count")

    raw_sha = ledger.get("raw_sha256")
    raw_bytes = ledger.get("raw_bytes")
    if not isinstance(raw_sha, str) or not re.fullmatch(r"[a-fA-F0-9]{64}", raw_sha):
        reasons.append("diagnostic raw SHA-256 is missing or invalid")
    if isinstance(raw_bytes, bool) or not isinstance(raw_bytes, int) or raw_bytes <= 0:
        reasons.append("diagnostic raw byte count is missing or invalid")

    start = read_json(receiver / "start-marker.json")
    end = read_json(receiver / "end-marker.json")
    stage_end = read_json(stage / "end-marker.json")
    manifest = read_json(receiver / "archive-manifest.json")
    transfer = read_json(receiver / "transfer-roundtrip.json")
    removal = read_json(receiver / "raw-removal.json")
    for marker_name, marker in (("receiver start", start), ("receiver end", end), ("stage end", stage_end),
                                ("archive manifest", manifest), ("transfer proof", transfer), ("raw removal", removal)):
        if marker.get("run") != run_id:
            reasons.append(f"{marker_name} run ID does not match stage")
    for marker_name, marker in (("receiver end", end), ("stage end", stage_end),
                                ("archive manifest", manifest), ("transfer proof", transfer), ("raw removal", removal)):
        if marker.get("raw_sha256") != raw_sha or marker.get("raw_bytes") != raw_bytes:
            reasons.append(f"{marker_name} raw hash or size does not match diagnostic ledger")
    if start.get("offset") != 0 or start.get("run") != run_id:
        reasons.append("receiver start marker does not begin this run at offset zero")
    if end.get("collector_final_pid") != 0 or end.get("collector_stop_exit_code") != 0 or end.get("final_line_terminated") is not True:
        reasons.append("receiver end marker does not confirm closed, complete output")
    if stage_end.get("collector_final_pid") != 0 or stage_end.get("collector_stop_exit_code") != 0:
        reasons.append("stage end marker does not confirm collector shutdown")
    if manifest.get("collector_stopped") is not True or manifest.get("roundtrip_sha256") != raw_sha or manifest.get("roundtrip_bytes") != raw_bytes:
        reasons.append("archive manifest does not verify the raw round trip")
    if transfer.get("source_hashes_match") is not True or transfer.get("gzip_integrity_verified") is not True:
        reasons.append("host transfer proof does not verify archive integrity")
    if removal.get("host_verification_hashes_match") is not True or removal.get("collector_writer_closed") is not True:
        reasons.append("raw removal receipt does not confirm verified closed-writer cleanup")

    archive_path = receiver / "receiver-raw.jsonl.gz"
    archive_sha = manifest.get("compressed_sha256")
    archive_bytes = manifest.get("compressed_bytes")
    if (not isinstance(archive_sha, str) or not isinstance(archive_bytes, int) or isinstance(archive_bytes, bool)
            or archive_sha != transfer.get("archive_sha256")
            or archive_bytes != transfer.get("archive_bytes")
            or archive_bytes != archive_path.stat().st_size
            or archive_sha != sha256(archive_path)
            or removal.get("compressed_sha256") != archive_sha):
        reasons.append("compressed receiver archive hash or size does not match its proofs")

    event_note = None
    event_path = stage / "service-error-events.json"
    if event_path.is_file():
        events = json.loads(event_path.read_text(encoding="utf-8-sig"))
        if isinstance(events, list):
            completion_time = dt.datetime.fromisoformat(str(completion.get("utc", "")).replace("Z", "+00:00"))
            candidates = []
            for event in events:
                if not isinstance(event, dict) or not isinstance(event.get("Message"), str):
                    continue
                match = re.fullmatch(r"/Date\((\d+)\)/", str(event.get("TimeCreated", "")))
                if not match or "10054" not in event["Message"] or "STOP/FINISH" not in event["Message"]:
                    continue
                event_time = dt.datetime.fromtimestamp(int(match.group(1)) / 1000, tz=dt.timezone.utc)
                if abs((completion_time - event_time).total_seconds()) <= 10:
                    candidates.append((abs((completion_time - event_time).total_seconds()), event_time))
            if candidates:
                event_time = min(candidates)[1]
                event_note = f"STOP/FINISH event 10054 at {event_time.isoformat()} (temporal correlation only)"

    lifecycle = (f"FAILED SCM stop (ExitCode={service_stop.get('ExitCode')}, "
                 f"ServiceSpecificExitCode={service_stop.get('ServiceSpecificExitCode')})")
    return reasons, {
        "evidence_class": "diagnostic-only counts",
        "lifecycle_status": lifecycle,
        "lifecycle_failure": completion.get("failure"),
        "service_error_event": event_note,
        "responses_received": counts.get("completed") if isinstance(counts, dict) else None,
        "client_dns_path_clean": None,
        "receiver_record_reconciliation_clean": None,
        "upstream_start": ledger.get("upstream_nft_counters_start"),
        "upstream_end": ledger.get("upstream_nft_counters_end"),
    }


def load_stage(stage: Path, stage_root: Path) -> tuple[dict[str, Any] | None, dict[str, Any] | None]:
    run_id = stage.name
    if not RUN_ID.fullmatch(run_id):
        return None, None

    absent = [name for name in REQUIRED_FILES if not (stage / name).is_file()]
    if absent:
        return None, pending(run_id, [f"missing {name}" for name in absent])

    client_exit = read_json(stage / "client-exit.json")
    completion = read_json(stage / "completion.json")
    service_stop = read_json(stage / "service-stop.json")
    client = read_json(stage / "client" / "summary.json")
    pointer_path = stage / "receiver" / "canonical-final.json"
    pointer = read_json(pointer_path)

    reasons: list[str] = []
    if client_exit.get("exit_code") != 0:
        reasons.append("client exit code is not zero")
    if completion.get("run") != run_id:
        reasons.append("completion run ID does not match directory")
    if completion.get("service_started") is not True:
        reasons.append("completion does not confirm service start")
    if pointer.get("schema_version") != 1 or pointer.get("run") != run_id:
        reasons.append("canonical-final pointer schema or run ID is invalid")
    authoritative_ledger = pointer.get("authoritative_ledger")
    if not isinstance(authoritative_ledger, str) or not authoritative_ledger:
        return None, pending(run_id, ["canonical-final pointer has no relative ledger path"])

    receiver_dir = (stage / "receiver").resolve()
    ledger_path = (receiver_dir / str(authoritative_ledger or "")).resolve()
    try:
        ledger_path.relative_to(receiver_dir)
    except ValueError:
        return None, pending(run_id, ["canonical ledger resolves outside this receiver stage"])
    if not ledger_path.is_file():
        reasons.append("canonical ledger file is missing")
        ledger = None
    else:
        pointer_hash = pointer.get("sha256")
        if not isinstance(pointer_hash, str) or pointer_hash.lower() != sha256(ledger_path):
            reasons.append("canonical ledger SHA-256 does not match pointer")
        ledger = read_json(ledger_path)

    options = client.get("options")
    counts = client.get("counts")
    if not isinstance(options, dict) or not isinstance(counts, dict) or ledger is None:
        reasons.append("client summary or canonical ledger has no required object")
    ledger_name = Path(authoritative_ledger or "").name
    is_diagnostic = ledger_name == "diagnostic-counts-v1.json"
    diagnostic_info: dict[str, Any] = {}
    if ledger is not None:
        if not isinstance(pointer.get("verdict"), str) or not pointer.get("verdict"):
            reasons.append("canonical pointer verdict is missing")
        if options is not None and counts is not None:
            if completion.get("mode") != options.get("mode") or completion.get("qps") != options.get("qps") or completion.get("duration_seconds") != options.get("duration"):
                reasons.append("completion mode, rate, or duration does not match client summary")
            if options.get("mode") != ledger.get("mode"):
                reasons.append("client mode does not match receiver ledger")
        if is_diagnostic:
            if not isinstance(pointer.get("clean"), bool):
                reasons.append("diagnostic canonical pointer clean value is not boolean")
            diagnostic_reasons, diagnostic_info = diagnostic_lifecycle(
                stage, ledger, pointer, client_exit, completion, service_stop,
                options if isinstance(options, dict) else {}, counts if isinstance(counts, dict) else {},
            )
            reasons.extend(diagnostic_reasons)
        else:
            if ledger_name != "reconciliation-v2.json":
                reasons.append("canonical ledger format is unsupported")
            if completion.get("failure") not in (None, ""):
                reasons.append("completion reports failure")
            if service_stop.get("State") != "Stopped" or service_stop.get("ExitCode") != 0:
                reasons.append("SCM stop is not confirmed clean")
            if ledger.get("run_id") != run_id:
                reasons.append("ledger run ID does not match directory")
            if not isinstance(ledger.get("receiver_record_reconciliation_clean"), bool):
                reasons.append("ledger reconciliation verdict is not boolean")
            if not isinstance(pointer.get("clean"), bool):
                reasons.append("canonical pointer clean value is not boolean")
            if pointer.get("clean") is not ledger.get("receiver_record_reconciliation_clean"):
                reasons.append("canonical pointer clean value differs from ledger")

    ledger_client = ledger.get("client_counts") if ledger else None
    operations = ledger.get("operation_counts") if ledger else None
    if not isinstance(ledger_client, dict) or not isinstance(operations, dict):
        reasons.append("ledger has no client or operation count objects")
    elif isinstance(counts, dict):
        for key in ("sent", "completed", "successful", "timeouts"):
            if counts.get(key) != ledger_client.get(key):
                reasons.append(f"client {key} differs between client summary and ledger")
        for key in ("CLIENT_QUERY", "CLIENT_RESPONSE", "RESOLVER_QUERY", "RESOLVER_RESPONSE"):
            if isinstance(operations.get(key), bool) or not isinstance(operations.get(key), int) or operations.get(key) < 0:
                reasons.append(f"ledger operation count {key} is missing or invalid")
        if not is_diagnostic and ledger.get("client_completed_matched_responses") != counts.get("completed"):
            reasons.append("matched responses received do not match client completed count")

    if not isinstance(options, dict) or not isinstance(counts, dict):
        pass
    else:
        for key in ("sent", "completed", "successful", "timeouts"):
            value = counts.get(key)
            if isinstance(value, bool) or not isinstance(value, int) or value < 0:
                reasons.append(f"client summary count {key} is missing or invalid")
        measured_value = client.get("measured_send_duration_ns")
        if isinstance(measured_value, bool) or not isinstance(measured_value, int) or measured_value <= 0:
            reasons.append("client measured send duration is missing or invalid")
        for key in ("qps", "duration"):
            value = options.get(key)
            if isinstance(value, bool) or not isinstance(value, int) or value <= 0:
                reasons.append(f"client option {key} is missing or invalid")

    if reasons:
        return None, pending(run_id, reasons)

    assert ledger is not None and isinstance(options, dict) and isinstance(counts, dict)
    assert isinstance(ledger_client, dict) and isinstance(operations, dict)
    sent = number(counts, "sent", "client.counts")
    completed = number(counts, "completed", "client.counts")
    successful = number(counts, "successful", "client.counts")
    received = (number(ledger, "client_completed_matched_responses", "ledger")
                if not is_diagnostic else number(ledger_client, "completed", "diagnostic.client_counts"))
    measured_ns = number(client, "measured_send_duration_ns", "client")
    requested_qps = number(options, "qps", "client.options")
    duration_seconds = number(options, "duration", "client.options")
    query_count = number(operations, "CLIENT_QUERY", "ledger.operation_counts")
    response_count = number(operations, "CLIENT_RESPONSE", "ledger.operation_counts")
    sent_minus_queries = sent - query_count
    completed_minus_responses = completed - response_count
    ratio = (sent / (measured_ns / 1_000_000_000)) / requested_qps
    flags = {
        key: number(counts, key, "client.counts") if key in counts else None
        for key in FLAG_KEYS
    }
    known_flag_values = [value for value in flags.values() if value is not None]
    qualifications: list[str] = []
    if any(value > 0 for value in known_flag_values):
        qualifications.append("client validation or response anomalies")
    if len(known_flag_values) != len(FLAG_KEYS):
        qualifications.append("validation counters unavailable")
    if is_diagnostic:
        qualifications.append("diagnostic-only count ledger; protocol cleanliness not asserted")
    else:
        if ledger.get("client_dns_path_clean_for_record_reconciliation") is not True:
            qualifications.append("client DNS path is not clean")
        if ledger.get("receiver_record_reconciliation_clean") is not True:
            qualifications.append("receiver record reconciliation is not clean")
    protocol_status = "qualified: " + "; ".join(qualifications) if qualifications else "no client validation anomalies recorded"
    upstream_queries = ledger.get("actual_upstream_queries")
    upstream_responses = ledger.get("actual_upstream_responses")
    actual_upstream_valid = all(
        isinstance(value, int) and not isinstance(value, bool) and value >= 0
        for value in (upstream_queries, upstream_responses)
    )
    nft_deltas = ledger.get("upstream_nft_packet_deltas")
    if is_diagnostic:
        start_counters = ledger.get("upstream_nft_counters_start")
        end_counters = ledger.get("upstream_nft_counters_end")
        if isinstance(start_counters, dict) and isinstance(end_counters, dict):
            if all(isinstance(start_counters.get(key), int) and not isinstance(start_counters.get(key), bool)
                   and isinstance(end_counters.get(key), int) and not isinstance(end_counters.get(key), bool)
                   and end_counters[key] >= start_counters[key]
                   for key in ("upstream_queries", "upstream_responses")):
                nft_deltas = {
                    "upstream_queries": end_counters["upstream_queries"] - start_counters["upstream_queries"],
                    "upstream_responses": end_counters["upstream_responses"] - start_counters["upstream_responses"],
                }
                upstream_queries = nft_deltas["upstream_queries"]
                upstream_responses = nft_deltas["upstream_responses"]
                actual_upstream_valid = True
            else:
                nft_deltas = None
        else:
            nft_deltas = None
    nft_queries = nft_deltas.get("upstream_queries") if isinstance(nft_deltas, dict) else None
    nft_responses = nft_deltas.get("upstream_responses") if isinstance(nft_deltas, dict) else None
    nft_counts_valid = all(
        isinstance(value, int) and not isinstance(value, bool) and value >= 0
        for value in (nft_queries, nft_responses)
    )
    if actual_upstream_valid and nft_counts_valid:
        upstream_status = "nft counter delta" if is_diagnostic else ("nft counters match ledger" if (upstream_queries, upstream_responses) == (nft_queries, nft_responses) else "nft counters mismatch ledger")
    elif actual_upstream_valid and ledger.get("upstream_evidence_available") is True:
        upstream_status = "ledger marks upstream evidence available"
    elif actual_upstream_valid:
        upstream_status = "ledger counts; supporting counter evidence unavailable"
    else:
        upstream_queries = None
        upstream_responses = None
        upstream_status = "unavailable"

    lifecycle_status = (diagnostic_info.get("lifecycle_status") if is_diagnostic
                        else "clean SCM stop")
    record = {
        "run_id": run_id,
        "capture_policy": ledger.get('capture_policy', 'all_supported'),
        "evidence_class": diagnostic_info.get("evidence_class", "canonical reconciliation ledger"),
        "lifecycle_status": lifecycle_status,
        "lifecycle_failure": diagnostic_info.get("lifecycle_failure"),
        "service_error_event": diagnostic_info.get("service_error_event"),
        "mode": options["mode"],
        "requested_qps": requested_qps,
        "achieved_qps": sent / (measured_ns / 1_000_000_000),
        "requested_duration_seconds": duration_seconds,
        "measured_send_duration_seconds": measured_ns / 1_000_000_000,
        "sent": sent,
        "completed": completed,
        "responses_received": received,
        "successful": successful,
        "timeouts": number(counts, "timeouts", "client.counts"),
        "dns_delivery_deficit": sent - completed,
        "client_query_records": query_count,
        "client_response_records": response_count,
        "resolver_query_records": number(operations, "RESOLVER_QUERY", "ledger.operation_counts"),
        "resolver_response_records": number(operations, "RESOLVER_RESPONSE", "ledger.operation_counts"),
        "sent_minus_client_query_records": sent_minus_queries,
        "completed_minus_client_response_records": completed_minus_responses,
        "actual_upstream_queries": upstream_queries,
        "actual_upstream_responses": upstream_responses,
        "upstream_evidence_available": upstream_status != "unavailable",
        "upstream_evidence_status": upstream_status,
        "rate_ratio": ratio,
        "rate_qualification": "below 98% offered rate" if ratio < 0.98 else "at least 98% offered rate",
        "flags": flags,
        "protocol_status": protocol_status,
        "client_dns_path_clean": diagnostic_info.get("client_dns_path_clean", ledger.get("client_dns_path_clean_for_record_reconciliation")),
        "receiver_record_reconciliation_clean": diagnostic_info.get("receiver_record_reconciliation_clean", ledger.get("receiver_record_reconciliation_clean")),
        "canonical_clean": pointer.get("clean"),
        "canonical_ledger": str(ledger_path.relative_to(stage_root)).replace("\\", "/"),
        "canonical_ledger_sha256": sha256(ledger_path),
        "canonical_verdict": pointer.get("verdict"),
        "canonical_strict_pass": pointer.get("strict_pass") if isinstance(pointer.get("strict_pass"), bool) else None,
        "raw_sha256": ledger.get("raw_sha256"),
    }
    return record, None


def relation(value: int) -> str:
    if value > 0:
        return f"+{value} missing"
    if value < 0:
        return f"{value} excess"
    return "0"


def count_or_unavailable(value: Any, evidence_available: bool) -> str:
    return str(value) if evidence_available and isinstance(value, int) and not isinstance(value, bool) else "unavailable"


def table_row(row: dict[str, Any]) -> str:
    flags = ", ".join(f"{key}={value}" for key, value in row["flags"].items() if value not in (None, 0))
    unavailable_flags = [key for key, value in row["flags"].items() if value is None]
    if unavailable_flags:
        flags = ", ".join(filter(None, [flags, "unavailable: " + ", ".join(unavailable_flags)]))
    elif not flags:
        flags = "none"
    values = dict(row)
    values.update(
        dns_deficit=relation(row["dns_delivery_deficit"]),
        cq=relation(row["sent_minus_client_query_records"]),
        cr=relation(row["completed_minus_client_response_records"]),
        upq=count_or_unavailable(row["actual_upstream_queries"], row["upstream_evidence_available"]),
        upr=count_or_unavailable(row["actual_upstream_responses"], row["upstream_evidence_available"]),
        flags=flags,
    )
    values["upstream_summary"] = (
        "unavailable" if row["upstream_evidence_status"] == "unavailable"
        else f"{values['upq']} / {values['upr']} ({row['upstream_evidence_status']})"
    )
    values["strict_pass"] = str(row["canonical_strict_pass"]).lower() if row["canonical_strict_pass"] is not None else "not stated"
    values["canonical_clean_label"] = str(row["canonical_clean"]).lower() if isinstance(row["canonical_clean"], bool) else "not stated"
    lifecycle = f"{row['lifecycle_status']}; {row['evidence_class']}"
    if row.get("lifecycle_failure"):
        lifecycle += f"; {row['lifecycle_failure'].strip(' ;')}"
    if row.get("service_error_event"):
        lifecycle += f"; {row['service_error_event']}"
    values["lifecycle_summary"] = lifecycle
    return "| {run_id} | {mode} | {requested_qps:,} / {achieved_qps:,.2f} | {requested_duration_seconds} / {measured_send_duration_seconds:.3f} | {sent:,} | {completed:,} | {responses_received:,} | {successful:,} | {dns_deficit} | {timeouts:,} | {client_query_records:,} | {client_response_records:,} | {resolver_query_records:,} / {resolver_response_records:,} | {cq} / {cr} | {upstream_summary} | {lifecycle_summary} | {canonical_verdict} / {canonical_clean_label} / {strict_pass}; {rate_qualification}; {protocol_status}; flags: {flags} |".format_map(values)


def generate(root: Path, output: Path, candidate_sha256: str =
             '487FFD782B730B7E1A849626AF68BCB6C8A0EF2057ACB7D8E5D932B7EA49A1AE',
             status: str = 'in_progress', capture_policy: str = 'all_supported') -> dict[str, Any]:
    root = root.resolve()
    stage_root = root / "stages"
    if not stage_root.is_dir():
        raise FileNotFoundError(f"Stage directory not found: {stage_root}")
    complete: list[dict[str, Any]] = []
    pending_rows: list[dict[str, Any]] = []
    for stage in sorted(stage_root.iterdir()):
        if not stage.is_dir() or not RUN_ID.fullmatch(stage.name):
            continue
        record, not_ready = load_stage(stage, root)
        if record is not None:
            if capture_policy == 'client_only' and record['capture_policy'] != 'client_only':
                raise ValueError(f"Client-only report has a mismatched capture policy: {stage.name}")
            if capture_policy == 'client_only' and (record['resolver_query_records'] != 0
                                                    or record['resolver_response_records'] != 0):
                record['filter_contract_qualification'] = 'FAILED filter contract: unexpected resolver records'
                record['source_canonical_clean'] = record['canonical_clean']
                record['source_canonical_strict_pass'] = record['canonical_strict_pass']
                record['canonical_clean'] = False
                record['canonical_strict_pass'] = False
                record['receiver_record_reconciliation_clean'] = False
                record['protocol_status'] += '; ' + record['filter_contract_qualification']
            complete.append(record)
        elif not_ready is not None:
            pending_rows.append(not_ready)
    complete.sort(key=lambda item: (item["mode"], item["requested_qps"], item["run_id"]))

    generated = dt.datetime.now(dt.timezone.utc).isoformat()
    data = {
        "schema_version": 1,
        "generated_utc": generated,
        "status": status.replace('_', ' '),
        "build_sha256": candidate_sha256.upper(),
        "capture_policy": capture_policy,
        "completed_stage_count": len(complete),
        "pending_stage_count": len(pending_rows),
        "complete_stages": complete,
        "pending_stages": pending_rows,
        "limits": [
            "Only a verified final canonical ledger with matching SHA-256 is included.",
            "An offered-rate shortfall does not establish a delivery deficit or a loss boundary.",
            "Aggregate count differences do not localize loss to ring admission, ETW, TCP, or collector processing.",
            "This campaign has no WPR/CPU collection and does not use service durable metrics or Prometheus drop counters.",
        ],
    }
    output.mkdir(parents=True, exist_ok=True)
    (output / "campaign-report.json").write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")

    md = [
        "# Parser ramp campaign report",
        "",
        f"**Status:** {status.replace('_', ' ').capitalize()}  ",
        f"**Generated:** {generated}  ",
        f"**Completed stages:** {len(complete)}  ",
        f"**Pending stages:** {len(pending_rows)}",
        f"**Candidate SHA-256:** `{candidate_sha256.upper()}`",
        f"**Capture policy:** `{capture_policy}`",
        "",
        "The following table includes cleanly stopped stages with verified canonical receiver ledgers and diagnostic-only stages with verified client counts and raw-archive hash bindings. Each row states its lifecycle and evidence class. A diagnostic-only row with a failed stop is not a strict pass. Requested and achieved QPS are shown separately. A positive sent-minus-completed value means unmatched client queries; a positive collector residual means fewer matching receiver records. Negative differences mean the second count exceeds the first.",
        "",
        "| Run | Mode | Requested / achieved QPS | Requested / measured seconds | Sent | Completed | Received | Successful | Sent−completed | Timeouts | `CLIENT_QUERY` | `CLIENT_RESPONSE` | Resolver Q / R | Sent−CQ / completed−CR | Actual upstream Q / R (evidence) | Lifecycle / evidence class | Ledger verdict / clean / strict pass; rate and protocol qualification; flags |",
        "|---|---|---:|---:|---:|---:|---:|---:|---|---:|---:|---:|---:|---|---|---|---|",
    ]
    if capture_policy == 'client_only':
        md.insert(9, 'Only provider events 256/257 are admitted (level 4, keyword 0x8000000000000000). Resolver records are excluded by design, including recursive workloads; upstream packet counts are separate observations.')
    md.extend(table_row(row) for row in complete)
    if not complete:
        md.append("| No complete stages | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — | — |")
    md.extend(["", "The following stages remain pending because required evidence is missing or does not validate:", ""])
    if pending_rows:
        md.extend(["| Run | Pending reason |", "|---|---|"])
        for item in pending_rows:
            md.append(f"| {item['run_id']} | {', '.join(item['reasons'])} |")
    else:
        md.append("No named campaign stages are pending in the stage directory.")
    md.extend([
        "",
        "## Interpretation limits",
        "",
        "- DNS delivery deficit is `sent - completed`; collector query and response residuals are `sent - CLIENT_QUERY` and `completed - CLIENT_RESPONSE`. A positive value means the first count exceeds the second; a negative value means the second exceeds the first.",
        "- When the client DNS path is not clean, report client-to-receiver residuals as ambiguous; do not assign them to collector capture loss.",
        "- Resolver operation counts are separate from actual upstream packet counts. The report says `unavailable` when validated upstream evidence is absent; it does not substitute zero.",
        "- Client flag-validation counts appear in each row. A DNS validation issue and a delivery deficit must be assessed separately.",
        "- An offered-rate shortfall does not by itself prove loss. Continue higher requested rates while achieved QPS increases; stop offered-load search after the planned two-step plateau or the 128k QPS cap.",
        "- Aggregate differences cannot assign loss to ring admission, Event Tracing for Windows (ETW), TCP, or collector processing. Equal counts do not prove unique query-to-record matching.",
        "- This campaign does not include Windows Performance Recorder (WPR), CPU profiling, service durable metrics, or Prometheus receiver drop counters.",
        "",
        "The JSON output preserves the full receipt-derived values and pending reasons. Do not describe the highest completed rate as a capacity boundary until all planned stages and confirmations finish.",
        "",
    ])
    (output / "campaign-report.md").write_text("\n".join(md), encoding="utf-8")
    return data


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--campaign-dir",
        type=Path,
        default=Path(__file__).resolve().parents[2] / "target/performance/parser-ramp-20261030",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        help="Defaults to CAMPAIGN_DIR/reports/generated",
    )
    parser.add_argument('--candidate-sha256', default='487FFD782B730B7E1A849626AF68BCB6C8A0EF2057ACB7D8E5D932B7EA49A1AE')
    parser.add_argument('--status', choices=('in_progress', 'complete', 'aborted_by_user'), default='in_progress')
    parser.add_argument('--capture-policy', choices=('all_supported', 'client_only'), default='all_supported')
    args = parser.parse_args()
    if not re.fullmatch(r'[A-Fa-f0-9]{64}', args.candidate_sha256):
        parser.error('--candidate-sha256 must contain 64 hexadecimal characters')
    output = args.output_dir or args.campaign_dir / "reports/generated"
    try:
        data = generate(args.campaign_dir, output, args.candidate_sha256, args.status, args.capture_policy)
    except (OSError, ValueError) as exc:
        print(f"report generation failed: {exc}", file=sys.stderr)
        return 2
    print(f"complete={data['completed_stage_count']} pending={data['pending_stage_count']}")
    print(f"markdown={output / 'campaign-report.md'}")
    print(f"json={output / 'campaign-report.json'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
