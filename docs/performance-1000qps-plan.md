# Performance repeat at global 1,000 QPS

This historical plan helps maintainers reproduce the method and review the
acceptance gates for the original paced comparison. The two runs are complete;
their [review](performance-1000qps-review.md) records ring loss in both modes.
The fixed binary hashes and campaign paths belong to that experiment, before
the capacity-only and pooled changes. See the
[documentation index](README.md#review-earlier-decisions-and-validation) for the sequence.

This plan depends on the historical service JSONL metrics writer. It cannot
measure the TCP-only service; new campaigns through its helpers are disabled.
See the [instrumentation compatibility note](README.md). Recorded evidence and
collector file destinations remain unchanged.

Run authoritative and recursive-request workloads for 300 seconds each at a
global offered-rate ceiling of 1,000 queries/s. Keep the existing binaries,
two client workers, 256 outstanding requests per worker, 2,000 ms deadlines,
Windows DNS cache policy, service ring capacity, and security profile unchanged.
Report the achieved rate and losses without assuming exactly 300,000 sends.
Do not rebuild or optimize the client/service for this comparison.

## Work chunks and dependencies

The original campaign assigned these work chunks:

| Chunk | Model / effort | Owner and outputs | Acceptance gate |
| --- | --- | --- | --- |
| Coordination | GPT-6-Astra / high | Coordinator; scope, decisions and final review | Two official 300-second windows; no unsupported capacity claim |
| Windows / execution | GPT-6.1-Sol / high | `perf_windows`; this plan, protected new campaign backup, Windows/host counters, service ledgers, report | Correct GUID/hash/SCM identity; exact CLI; PID-validated samples and final ledgers |
| Ubuntu fixture / receiver | GPT-6.1-Sol / high | `lab_workload`; sole owner of temporary upstream/receiver configuration, metrics, offsets, archives and restoration | TTL probes, no upstream socket loss, exact receiver reconciliation and restore receipt |
| Trace support | GPT-6.1-Sol / high | `stress_client`; new capped-run export/aggregation helpers and bounded existing-trace analysis | Metadata-only during load; heavy exports only after both official windows and explicit clearance; no production edits |
| Independent pacing / evidence review | GPT-6-Astra / high, read-only | Reviewer; pacing, arithmetic, counter/trace validity and report review | Confirm `--qps 1000` is global across both workers before first load |

Dependencies are sequential: plan → inventory/hashes/pacing review → Ubuntu
READY → current Windows backup and owned forwarder → fixture probes → optional
10-second sanity → authoritative 300 seconds → drain/final ledger/receiver
reconciliation → recursive 300 seconds → drain/reconciliation → restoration and
final query proof → independent final report review. Independent read-only
analysis can run outside measurement; avoid heavy archive scans or trace
exports during the official windows.

## Fixed lab and evidence

Windows VM `DNSTap-WS2025`, GUID
`24bafedb-0998-4430-a0c6-7e90350cd9ac`, is expected at
`172.27.215.163`, with 2 vCPUs and 4 GiB RAM. Verify live identity and readiness.
Use Windows PowerShell 5.1 and PowerShell Direct with the existing protected
DPAPI credential. Never print or copy its contents into evidence.

Required service SHA-256:
`0FCDD17E08A3DDD42F4F132F7F03F80A5513FEBC66E54F90A5EBBC8135BE2C1D`.
Required client SHA-256:
`7A5800CFC4252196E4C6162F7D2CCF81217FE95A339D7CFBB9CD84063F4B6960`.

The existing `auth.stress.test` zone retains exactly 5,000 A records pointing
to `192.0.2.53`, TTL 300. Source/license/hash provenance is in
[`datasets/performance/workload-provenance.json`](../datasets/performance/workload-provenance.json).
Run-name files are the generated `target/performance/workload/auth-names.txt`
and `rec-names.txt`; preserve their exact hashes in each invocation manifest.
Recreate only the owned temporary `rec.stress.test` conditional forwarder to
the ready Ubuntu upstream `172.27.215.60`, whose answers have TTL 0.
Do not alter the enabled Windows global query block list or cache policy.

Save current SCM arguments/state, binary hash, recursion/forwarder settings,
service SID type and required privileges in a **new** protected campaign
directory. Do not rerun the original `Prepare` workflow, recreate the existing
authoritative zone, overwrite earlier backups, or replace prior run artifacts.

Use the new campaign helper after Ubuntu READY:

```powershell
powershell.exe -NoProfile -File scripts/performance/windows-1000qps-campaign.ps1 -Action Inventory
powershell.exe -NoProfile -File scripts/performance/windows-1000qps-campaign.ps1 -Action Prepare
```

It saves the current baseline in the protected guest directory
`C:\ProgramData\EtwDnstap\performance\campaign-1000qps-20261028` and new host
receipts in `target/performance/windows/campaign-1000qps-20261028`.

## Exact workload and capture commands

Run the compiled Rust client on the host, targeting the VM explicitly. The
PowerShell wrapper writes the full expanded executable/argument vector,
binary/name hashes, UTC timestamps, host resource samples, and exit code:

```powershell
powershell.exe -NoProfile -File scripts/performance/windows-run-client.ps1 -Mode authoritative -RunId auth-1000qps-1 -Threads 2 -Duration 300 -Qps 1000
powershell.exe -NoProfile -File scripts/performance/windows-run-client.ps1 -Mode recursive -RunId rec-1000qps-1 -Threads 2 -Duration 300 -Qps 1000
```

The wrapper appends `--qps 1000` once to this fixed argument vector:

```text
--server 172.27.215.163:53 --names NAMES --mode MODE --threads 2 --duration 300 --inflight 256 --timeout-ms 2000 --output-dir RUN_DIRECTORY --qps 1000
```

`NAMES` is the selected generated names-file path, `MODE` is `authoritative`
or `recursive`, and `RUN_DIRECTORY` is the fresh output directory in the
expanded invocation manifest. This block is an argument vector, not a command
to execute by itself.

For each official run, use its unique run ID with the existing helpers:

```powershell
powershell.exe -NoProfile -File scripts/performance/windows-control.ps1 -Action Start -RunId auth-1000qps-1
powershell.exe -NoProfile -File scripts/performance/windows-capture.ps1 -Action Start -RunId auth-1000qps-1
powershell.exe -NoProfile -File scripts/performance/windows-capture.ps1 -Action Trace -RunId auth-1000qps-1
powershell.exe -NoProfile -File scripts/performance/windows-control.ps1 -Action Stop -RunId auth-1000qps-1
powershell.exe -NoProfile -File scripts/performance/windows-capture.ps1 -Action Stop -RunId auth-1000qps-1
powershell.exe -NoProfile -File scripts/performance/windows-control.ps1 -Action Fetch -RunId auth-1000qps-1
```

Create receiver markers `auth-1000qps-1-start/end` and
`rec-1000qps-1-start/end` using the receiver helper. Start markers precede the
service/client session. After the client sending window and bounded response
drain, allow at least three seconds for receiver counter publication, stop the
service to drain ETW/parser output, then create the end marker with settlement.
Byte offsets must include the final drain. Keep the 300-second resource window
separate from the enclosing service/receiver ledger window.

A single optional sanity run uses a distinct `sanity-1000qps-1` ID, duration
10 seconds and the same two-worker global QPS setting. It is not an official
result. Require client exit zero and inspect achieved sends, timeout/error
counts, host contention, service final ledger and receiver counts.

## Metrics, analysis and validity gates

Collect one-second PerfMon BLG/CSV process CPU (user/kernel), private/working/
private-working/virtual memory, page faults, thread/handle counts and I/O rates
for the exact service and DNS PIDs. Collect thread PID/TID, CPU, context
switches/state/wait reason, guest CPU/ready queue/memory/disk/network/DNS
counters, host client resources, and Ubuntu upstream/receiver resources.
Preserve discovered paths, exclusions, actual intervals and valid counts.

Use a short WPR memory recording during each load. Retention can be shorter
than the ETL header range even with zero lost-event counters; inspect retained
coverage before drawing Ready/Wait or stack conclusions. Include recording/
rundown and metrics overhead in limitations. Do not cancel another trace.

Use the existing `analyze-windows.py` and `analyze-host.py` on the exact client
sending windows. Validate consecutive process/thread identity for rate
attribution. Report duration-weighted process/system CPU with valid coverage,
CPU units (100% = one logical CPU; divide process CPU by 2 for guest capacity),
memory start/end/delta/peak, thread/handle ranges, I/O/queue/wait evidence,
achieved QPS, and matched-response latency histogram upper-bound quantiles.
Timeouts are not automatically network loss, and latency excludes timeouts.

Require valid final ETW statistics and these stable schema 1 ledgers:

```text
received = ring_dropped + callback_errors + dequeued + queue_backlog
dequeued = encoded + ignored + rejected + output_limit_dropped + output_failed_events
```

Keep ETW `EventsLost` separate from `RealTimeBuffersLost` (events versus
buffers). Check event domains before any combined loss denominator. The
receiver file record count and ingress/egress/discard counters independently
test encoded-to-durable output loss. Use actual upstream query/reply counts
for recursion; TTL 0 and a repeated name list do not establish four events per
client query or one cache miss per request. A nonzero loss at 1,000 QPS is a
measurement finding, not a reason to silently tune this run.

Expected artifacts are under `target/performance/client/RUN_ID`,
`target/performance/windows/RUN_ID`, new campaign backups, Ubuntu marker/
resource/reconciliation receipts, and verified lossless receiver archives.
Preserve every earlier run and backup. Final conclusions go in
[`performance-1000qps-review.md`](performance-1000qps-review.md).

## Restore and verify

Stop/delete only new task collectors and finish owned WPR recordings. Restore
this campaign's saved SCM command line without the historical metrics writer, retaining the
tested binary and authoritative 5,000-record zone. Remove only the newly owned
recursive forwarder. The Ubuntu owner restores its original console/journal
output, prior sysctls and task fixtures. Verify both baseline services healthy,
the original strict SID/privilege profile, and an authoritative query returning
`192.0.2.53`/TTL 300 with visible CQ/CR receiver console output. Record final
hashes, settings, service status and proof in new campaign receipts.

After Ubuntu console output is restored, execute:

```powershell
powershell.exe -NoProfile -File scripts/performance/windows-1000qps-campaign.ps1 -Action Restore
powershell.exe -NoProfile -File scripts/performance/windows-1000qps-campaign.ps1 -Action Proof
```
