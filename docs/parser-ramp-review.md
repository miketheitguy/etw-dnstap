# Parser optimization DNS ramp review

This aborted-campaign review is for maintainers evaluating the parser and encoder optimization. It includes strict stages with a successful client-process exit, completed run receipt, clean Service Control Manager (SCM) stop, and a final receiver ledger, plus diagnostic-only stages with verified client counts and raw-archive hash bindings when the service lifecycle fails. Diagnostic-only stages remain visibly qualified and never count as strict passes. See the [parser ramp plan](parser-ramp-plan.md) for the historical workload and stop rules.

**Aborted by user; do not resume this campaign.** The latest client stage,
authoritative 48k for 60 seconds, sent and completed 2,818,519 queries at
46,975.32 achieved QPS, with no client validation anomalies and a clean SCM
stop. Higher stages and high-rate 300-second confirmations were canceled;
this record establishes no final loss boundary or sustained high-rate result.
Cleanup restored normal receiver output and Windows DNS settings and retained
candidate `487FFD…A1AE` Running. Evidence remains under
`target/performance/parser-ramp-20261030/`; cleanup receipts are
`aborted-by-user.json`, `cleanup/normal-receiver-ready.json`, and
`windows/cleanup-Restore-20261030T032912664Z.json` relative to that root.

The candidate is `target/x86_64-pc-windows-msvc/release/etw-dnstap.exe`, SHA-256 `487FFD782B730B7E1A849626AF68BCB6C8A0EF2057ACB7D8E5D932B7EA49A1AE`. See the [build provenance receipt](../target/performance/parser-ramp-20261030/build/build-20261030T023020Z.json).

## Current evidence

The authoritative 1,000-QPS, 300-second baseline is complete. The receiver's canonical ledger is `target/performance/parser-ramp-20261030/stages/parser-auth-1000-300s-1/receiver/reconciliation-v2.json`, selected by `canonical-final.json`.

The validated stage records 299,906 client queries sent, 299,906 responses completed and received, and 299,906 successful responses over 300.000 seconds. The achieved rate is 999.69 QPS from the requested 1,000 QPS. The receiver records 299,906 `CLIENT_QUERY` and 299,906 `CLIENT_RESPONSE` operations. The client DNS path and receiver record reconciliation are clean; client validation counters show zero timeouts, send and receive errors, malformed or mismatched responses, late or duplicate responses, and AA/RA/RD or truncation issues. The client and receiver count residuals are both zero.

The generated report keeps delivery and capture-count alignment separate from protocol validity. It lists completed and received counts independently from successful responses and retains late, duplicate, malformed, and DNS-flag validation counters as qualifications. A completed stage with a nonzero validation flag is not described as a strict pass merely because its counts align.

The resolver operation counts are zero for this authoritative workload. Actual upstream packet counts are unavailable in this ledger; report them as unavailable rather than treating missing counters as zero.

The authoritative 8,000-QPS stage is a strict clean result: 478,555 queries were sent, completed, received, and successful over 60 seconds. The client achieved 7,975.92 QPS from 8,000 requested. The receiver records 478,555 client queries and responses, with zero directional residuals and no client validation flags.

The 16,000-QPS authoritative stage has a complete diagnostic-only ledger, not a strict pass. The client sent, completed, received, and successfully validated 955,300 queries over 60 seconds, or 15,921.67 QPS from 16,000 requested. The receiver records 955,300 `CLIENT_QUERY` and `CLIENT_RESPONSE` operations; the directional residuals are zero. The diagnostic ledger contains 1,910,600 operation rows, zero malformed DNS or JSON records, and zero unknown selected-zone operations. The scoped upstream nftables counters have zero query and response delta for this stage. The canonical pointer selects `diagnostic-counts-v1.json`, marks `strict_pass=false`, and uses the verdict `diagnostic_only_windows_scm_stop_failure`. The report includes this stage because the client receipt, raw counts, archive hash, transfer proof, and finalization receipts validate, while explicitly recording its failed service lifecycle.

The client process ended at `2026-10-30T03:08:30.060Z`. A Windows service error event at `03:08:33.098Z` records a connection reset (`10054`) during the dnstap STOP/FINISH exchange. The completion receipt at `03:08:33.394Z` records the SCM stop failure; the service stop receipt says `Stopped`, `ExitCode=1066`, `ServiceSpecificExitCode=1`, and `ProcessId=0`. The receiver subsequently recorded a final end marker and its archive and transfer receipts verified the raw stream hashes. These timestamps establish sequence only; they do not establish that the reset caused the SCM failure or identify a missing-record cause.

The 24,000-QPS authoritative stage is another strict clean result: 1,429,752 queries were sent, completed, received, successful, and counted as both `CLIENT_QUERY` and `CLIENT_RESPONSE` over 60 seconds. The client achieved 23,829.20 QPS from 24,000 requested. Its canonical ledger reports no client validation flags and zero directional residuals.

The recursive 1,000-QPS, 300-second baseline is complete but qualified. It records 299,910 queries sent, 299,910 responses completed and received, and 299,909 successful responses at 999.70 achieved QPS from the requested 1,000 QPS. The collector records 299,910 `CLIENT_QUERY` and `CLIENT_RESPONSE` operations, so the observed count residuals are zero. It also records 152,264 `RESOLVER_QUERY` and `RESOLVER_RESPONSE` operations. The actual upstream counters report 152,264 queries and responses, matching the narrowly scoped nftables counter deltas.

One client response has `RA` missing. The canonical receiver verdict is `qualified_dns_flag_validation_failure`; `clean` and `strict_pass` are false. Treat this as a DNS flag-validation qualification with aligned counts, not a strict pass or a client delivery deficit. Because the client DNS path is marked not clean, count alignment does not localize capture loss.

The authoritative 32k stage completed with 1,895,652 matching client and
collector query/response counts and a clean lifecycle. The 40k stage completed
with 2,367,473 matching directional counts but a qualified postload STOP/FINISH
connection-reset failure. Those lifecycle qualifications remain independent
of count alignment. Use each preserved canonical ledger for its exact result.

## Rebuild the receipt-derived report

The report generator validates each stage's completion receipts and `receiver/canonical-final.json` pointer, resolves the selected ledger within that stage, and verifies the ledger SHA-256 before calculating counts. For diagnostic-only ledgers, it also checks client and SCM receipt identity, raw byte and SHA-256 matches across the end markers, archive manifest, transfer proof, and removal receipt, plus the compressed archive hash. It accepts a complete failed or qualified ledger without treating its verdict as a pass. It writes both Markdown and JSON output under `target/performance/parser-ramp-20261030/reports/generated/`; see the [generated Markdown report](../target/performance/parser-ramp-20261030/reports/generated/campaign-report.md) and [generated JSON report](../target/performance/parser-ramp-20261030/reports/generated/campaign-report.json).

To regenerate the report from the current stage receipts, run:

```powershell
python scripts/performance/build-parser-ramp-report.py
```

The generated report shows requested and achieved QPS separately, signed directional residuals, DNS delivery deficit, client validation flags, resolver records, and actual upstream evidence availability. Missing receipts keep a stage pending; malformed JSON or invalid counts fail report generation rather than creating zero-valued results.

## Interpretation and limits

This baseline supports a clean authoritative count reconciliation at the requested 1,000 QPS. It does not establish performance at higher rates, a loss boundary, a maximum capacity, or a universal QPS ceiling. Do not summarize a maximum rate until the planned ramp and required confirmation stages complete.

An achieved/requested rate below 98% is an offered-rate shortfall. Continue higher requested rates while achieved QPS increases, and stop offered-load search after two successive higher requested rates each increase achieved QPS by less than 2%. A shortfall can coexist with a client or collector delivery deficit, so assess those counts separately. Positive signed residuals indicate missing receiver records; negative residuals indicate excess records. Aggregate differences cannot localize loss to ring admission, Event Tracing for Windows (ETW), TCP, or collector processing, and equal totals do not prove unique query-to-record matching.

This campaign does not include Windows Performance Recorder (WPR), CPU profiling, service durable metrics, or Prometheus receiver drop counters. The Windows service's guard samples are not a CPU campaign and do not support CPU performance claims.

## Retain the aborted record

Do not run additional stages under this campaign. Preserve existing evidence
and keep missing or incomplete finalization receipts visible. Any derived
report must identify the status as aborted by user and must not imply that
the planned ramp or high-rate confirmations completed. The separately
authorized [filtered ramp](filtered-ramp-plan.md) uses a different artifact
and evidence root.
