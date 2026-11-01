# Filtered DNS ramp review

> **Status:** Historical (an in-progress snapshot of the October 30, 2026 filtered campaign; no final result is recorded here) · **Evidence dated:** October 30, 2026 · **Commit:** `3d401cc` (the pushed commit the deployment receipt cites) · **Reviewed:** 2026-11-01

This in-progress review is for maintainers evaluating the client-event filter. It reports only verified evidence from the filtered campaign and keeps client results separate from receiver reconciliation. See the [filtered ramp plan](../plans/filtered-ramp-plan.md) for the matrix, capture contract, and stop rules.

## Candidate and capture scope

The deployment receipt records pushed commit `3d401cc604ecc0314278c591073dd94985335464`, coordinator-verified push, and candidate executable SHA-256 `6065E8DFD185D3793BA34F8DD994F62F15992FD9066FD49BF85EDE700419C27F`. The deployed filter source hash is `2D601DF9EA49679FFCA021757E5733B849C49A3347513AF0AF45C09D8BEFF3FE`, matching `src/etw.rs` in the workspace. The campaign report uses the candidate hash and `client_only` policy.

The release binding records that all eight Rust source hashes match the committed build, and records a clean worktree for the commit. The executable hash matches the deployment receipt. The build receipt itself records source HEAD `2c62a9ffe789e15f95f77e3d429c4bd625ca480f` and a dirty working tree at build time; the binary was built before the later pushed commit. Describe this as the same Rust source verified against the commit, not as a clean-tree build performed from that commit.

The filter admits DNS provider events 256 and 257 at level 4 with keyword mask `0x8000000000000000`. It emits `CLIENT_QUERY` and `CLIENT_RESPONSE` records. Resolver events are excluded by design, including in recursive stages. Report controlled upstream query and response packet counters separately; do not expect them as collector resolver records.

## First client stage

The authoritative 48,000-QPS stage ran for 60 seconds with 5,000 rotating A names, two workers, 256 outstanding requests per worker, and a 2,000-millisecond response timeout. The client sent 2,824,483 queries; it completed and successfully validated 2,824,483 matched responses. Its measured send duration is 60 seconds, for 47,074.72 achieved QPS, or 98.07% of the requested rate. Client timeout, send/receive error, malformed, mismatch, late, duplicate, AA/RA/RD, and truncation counters are all zero. Peak outstanding requests were 512, and the final outstanding count was zero.

The client process exited with code 0. The service stop receipt reports `Stopped`, exit code 0, service-specific exit code 0, and process ID 0. The receiver READY receipt confirms the `client_only` policy and expected operations. These receipts establish the client load result and a clean Windows service stop.

The receiver has not yet published `receiver/canonical-final.json` for this stage. The report generator therefore keeps the run pending and does not show collector query or response counts, directional residuals, or a strict stage result. Do not describe this stage as a clean rate until receiver reconciliation completes.

## Regenerate the report snapshot

The receipt-derived report currently has zero canonical-complete stages and one pending stage. It writes Markdown and JSON under `target/performance/filtered-ramp-20261030/reports/generated/`; see the [generated Markdown report](../../target/performance/filtered-ramp-20261030/reports/generated/campaign-report.md) (local artifact) and [generated JSON report](../../target/performance/filtered-ramp-20261030/reports/generated/campaign-report.json) (local artifact).

To regenerate the in-progress snapshot with this campaign's candidate and filter policy, run:

```powershell
python scripts/performance/build-parser-ramp-report.py `
  --campaign-dir target/performance/filtered-ramp-20261030 `
  --candidate-sha256 6065E8DFD185D3793BA34F8DD994F62F15992FD9066FD49BF85EDE700419C27F `
  --status in_progress `
  --capture-policy client_only
```

The generator admits stages only after their canonical receiver ledger validates. Under `client_only`, nonzero resolver operation counts mark a stage as a filter-contract failure. Missing receiver receipts remain pending; missing counts are never filled with zero.

## Interpretation limits and next evidence

This snapshot does not establish collector record counts, an end-to-end loss boundary, a maximum rate, or a result for recursive traffic. Equal counts would not prove unique query-to-record identity, and aggregate differences would not localize loss to ring admission, Event Tracing for Windows (ETW), TCP, or collector processing.

The campaign does not include Windows Performance Recorder (WPR), CPU profiling, service durable metrics, or Prometheus receiver drop counters. Do not infer CPU behavior from Windows service guard samples.

After the receiver publishes a verified canonical ledger, regenerate the report and compare client sends with `CLIENT_QUERY` and matched responses with `CLIENT_RESPONSE`. Keep any protocol qualification, lifecycle result, upstream packet delta, and capture-policy violation separate. Keep this review marked in progress until the planned ramp, confirmation, and cleanup evidence is available.
