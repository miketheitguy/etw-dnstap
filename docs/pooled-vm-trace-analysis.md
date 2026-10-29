# Pooled VM trace analysis

Use this note to review native WPR evidence for the campaign at
`target/performance/pooled-vm-20261029-0942`. The campaign plan and final review
own workload decisions and capture results. This note records the trace probe,
export procedure, and limits on scheduling and stack attribution.

## Accepted recording probe

The Windows owner recorded `capture/wpr-probe-3/trace.etl` with actual WPR in
file mode. The trace reviewer inspected that file with the installed Xperf and
exported its data with WPA Exporter 11.8.423.12582.

| Check | Observed result |
| --- | --- |
| ETL bytes | 8,388,608 |
| Trace start UTC | 2026-10-29 09:10:50.6973172 |
| Trace end UTC | 2026-10-29 09:11:00.9026011 |
| Retained span | 10.2052839 seconds |
| Guest processors | 2 |
| Lost events / buffers | 0 / 0 |
| CSwitch events / associated stacks | 5,089 / 3,407 |
| ReadyThread events / associated stacks | 2,955 / 2,955 |
| Sampled profile events / associated stacks | 20,355 / 246 |
| Disk read / write initiation events and stacks | 4 / 21; every event has a stack |
| File I/O events | 3,558 |
| File read events / associated stacks | 21 / 20 |
| File write events / associated stacks | 175 / 175 |
| Hard faults / associated stacks | 5 / 5 |
| Lifetimes | Process, thread, and image start/end rundown present |

The idle probe has many idle samples; stack count is not expected to equal all
sample events. The recorded counts establish event presence and native export
support. They do not establish loss-free capture under load. Inspect every load
stage independently.

The recording profile uses 128 buffers of 1,024 KiB, or 128 MiB of configured
buffer storage. Report that instrumentation reservation alongside the 4 GiB
guest's memory results. The stage owner guards trace and temporary-file growth
and preserves complete sequential ETLs. The profile does not change the existing
1 ms sampling interval.

The accepted recording profile SHA-256 is
`9AA81B45BDED6232A1B7306CD41031A38E54552FF34E030270073A81C6CC8418`.
The actual guest schema accepts `DiskIOInit`; it rejected the public
documentation's `DiskIOInitialization` spelling. Probe 2 retains that rejection.
Probe 3 proves the corrected profile works on this guest. See Microsoft's
[system provider keywords](https://learn.microsoft.com/windows-hardware/test/wpt/keyword--in-systemprovider-)
and [stack definitions](https://learn.microsoft.com/windows-hardware/test/wpt/stack-wpa).

Probe evidence is in `wpa/probe3-flat`: `trace-detail-stack.txt`,
`cswitch-coverage.txt`, four raw CSVs, and `export.log`. The earlier `wpa/probe3`
exports retain collapsed group rows and are preserved for provenance. Use the
flat exports for stack and per-switch analysis. The flat profile sets
`KeyColumnCount="0"` so a collapsed process group cannot hide leaf events.

## Authoritative five-minute observations

This original baseline includes a later in-load management enumeration. Its
capture ledger remains valid; its broad resource measurements carry that
qualification. The separately authorized clean repeat supplies the primary
five-minute resource comparison when complete.

Stage `pooled-auth-1000-300s-1` uses service PID 3188 and DNS PID 4176. Its
776,994,816-byte trace spans UTC 09:15:49.2986125 through 09:21:09.4973807,
or 320.1987682 seconds. Actual first/reliable events begin at the trace start;
the last observed event is at 320.2049616 seconds. Full-file inspection reports
zero lost events and buffers, with continuous one-second CSwitch coverage across
the sending window. The trace contains 1,400,914 CSwitch events with 1,059,575
associated stacks and 839,013 ReadyThread events with 839,013 stacks.

The client's sending interval begins at trace-relative 11.5303875 seconds.
The matched window is 16.5303875–36.5303875 seconds, corresponding to sending
seconds 5–25. The broad interior is 16.5303875–306.5303875 seconds, corresponding
to sending seconds 5–295. Evidence is in
`wpa/pooled-auth-1000-300s-1/{inspect,matched,broad}`.

| Measurement | Observed result | Scope |
| --- | --- | --- |
| Service CPU excluding observed DPC/ISR | 2.160047 CPU seconds; 0.74484% of one core; 0.37242% of two-vCPU capacity | Broad 290 seconds |
| DNS CPU excluding observed DPC/ISR | 11.181446 CPU seconds; 3.85567% of one core; 1.92784% of two-vCPU capacity | Broad 290 seconds |
| Service sampled CPU | 1,894 samples; 1,877.640 ms of sample weight | Broad 290 seconds |
| Service sampled instruction modules | `tdh.dll` 558.144 ms; `ntdll.dll` 540.660 ms; application 387.683 ms; kernel 227.556 ms | Broad sample weights; unresolved functions |
| Service/DNS attributed disk I/O | Zero filtered rows | Broad 290 seconds; cached writes can reach disk through System |
| Service/DNS hard faults | Zero filtered rows | Broad 290 seconds |
| Service FileIo writes | 292 writes, 163,143 bytes, with 292 operation-end events; all point to the metrics JSONL | Broad 290 seconds |
| DNS FileIo events | Zero observed attributed events | Broad 290 seconds; does not include every form of I/O |
| Parser identity | `dnstap-parser`, TID 3764 | Matched 20 seconds |
| Parser scheduled CPU / switch-ins | 127.3897 ms / 1,352 | Matched 20 seconds; scheduled CPU includes observed interrupt overlap |
| Parser positive Wait count | 1,286 | Matched 20 seconds |
| Parser positive Wait p50 / p95 / p99 / max | 15.9581 / 16.0895 / 16.4497 / 16.5317 ms | Matched 20 seconds; one wait crosses the left boundary |
| Parser Ready total / p50 / p95 / p99 / max | 28.4368 ms / 12.7 / 63.8 / 119.1 / 509.1 µs | Matched 20 seconds |
| Parser dispatcher wait reason | 1,286 `WrAlertByThreadId` observations | Matched 20 seconds; does not prove a lock |
| Parser stack coverage | 1,352 new-thread and 1,352 readying stacks | Matched 20 seconds; symbols disabled in initial export |
| ETW consumer identity / scheduled CPU | `etw-consumer`, TID 1612 / 14.1406 ms | Matched 20 seconds |
| ETW consumer switch-ins / positive Wait median | 218 / 93.9675 ms | Matched 20 seconds |

The initial exports retain real module stacks but disable symbol decoding.
Matching OS symbol decoding is deferred to a cleared analysis window. The
parser's roughly 16 ms waits persist despite the configured 2 ms source timeout;
the trace establishes the measured scheduling distribution, not its source-level
cause. This 20-second precise window cannot establish the absence of lock or
scheduling problems across the full five-minute stage. Broad disk, file, and
fault observations do not rule out TCP or other I/O. PerfMon, application, and
receiver counters supply those separate measurements.

## Recursive five-minute observations

This original baseline also includes a later in-load management enumeration.
Preserve its capture result and use the authorized clean repeat as the primary
resource comparison when complete.

Stage `pooled-rec-1000-300s-1` uses service PID 2492 and DNS PID 4176. Its
663,748,608-byte ETL spans UTC 09:27:46.7979906 through 09:33:07.4870098,
or 320.6890192 seconds. Actual first/reliable events begin at the trace start;
the last observed event is at 320.6976566 seconds. Full-file inspection reports
zero lost events and buffers, with continuous one-second CSwitch coverage across
sending. It contains 1,418,199 CSwitch events with 876,396 stacks and 809,783
ReadyThread events with 809,783 stacks.

Sending begins at trace-relative 10.8930094 seconds. The matched window is
15.8930094–35.8930094 seconds; the broad interior is
15.8930094–305.8930094 seconds. These correspond to the same sending-relative
5–25 and 5–295 second windows used for the authoritative baseline. Evidence is
in `wpa/pooled-rec-1000-300s-1/{inspect,matched,broad}`.

| Measurement | Observed result | Scope |
| --- | --- | --- |
| Service CPU excluding observed DPC/ISR | 3.038240 CPU seconds; 1.04767% of one core; 0.52383% of guest capacity | Broad 290 seconds |
| DNS CPU excluding observed DPC/ISR | 20.532513 CPU seconds; 7.08018% of one core; 3.54009% of guest capacity | Broad 290 seconds |
| Service sampled CPU | 2,609 samples; 2,590.544 ms of sample weight | Broad 290 seconds |
| Service sampled instruction modules | `tdh.dll` 828.881 ms; `ntdll.dll` 775.237 ms; application 540.487 ms; kernel 247.116 ms | Broad sample weights; unresolved functions |
| Service/DNS attributed disk I/O / hard faults | Zero filtered rows in both tables | Broad 290 seconds; cached file and TCP I/O remain separate |
| Service FileIo writes | 292 metrics JSONL writes, 163,376 bytes, with 292 operation-end events | Broad 290 seconds |
| DNS FileIo events | Zero observed attributed events | Broad 290 seconds |
| Parser identity | `dnstap-parser`, TID 4900 | Matched 20 seconds |
| Parser scheduled CPU / switch-ins | 189.6797 ms / 1,432 | Matched 20 seconds |
| Parser positive Wait count | 1,278 | Matched 20 seconds |
| Parser positive Wait p50 / p95 / p99 / max | 15.9475 / 16.2715 / 16.4648 / 16.6478 ms | Matched 20 seconds; one wait crosses the left boundary |
| Parser Ready total / p50 / p95 / p99 / max | 33.2810 ms / 14.2 / 66.3 / 146.1 / 255.8 µs | Matched 20 seconds |
| Parser dispatcher wait reason | 1,278 `WrAlertByThreadId` observations | Matched 20 seconds; does not prove a lock |
| Parser stack coverage | 1,432 new-thread and 1,432 readying stacks | Matched 20 seconds; symbols disabled in initial export |
| ETW consumer identity / scheduled CPU | `etw-consumer`, TID 3064 / 20.9860 ms | Matched 20 seconds |
| ETW consumer switch-ins / positive Wait median | 327 / 65.7568 ms | Matched 20 seconds |

Both baselines show roughly 16 ms median positive parser waits. The recursive
baseline has more captured events and a shorter ETW consumer wait median. These
are observations from the stated windows. They do not establish timer causality
or a whole-stage absence of locking. Keep PerfMon's time-weighted CPU results
separate from these native CPU totals because their windows and accounting differ.

## First authoritative limiting stage

At requested 64,000 QPS, stage `pooled-auth-64000-60s-1` achieves approximately
62,573.43 QPS, or 97.77% of the request. Final native counters contain 7,508,812
received events, 1,409,877 ring/intake drops (18.78%), 6,098,935 encoded events,
and an observed queue high-water mark of 8,192. Other final loss categories are
zero. This is a failed capture stage and a requested-rate miss; preserve both
results independently.

The 1,785,724,928-byte WPR trace spans UTC 10:00:46.2142537 through
10:02:10.2833205, or 84.0690668 seconds. Actual reliable events begin at the
trace start and extend to 84.0692608 seconds. Full-file inspection reports zero
lost events/buffers and continuous CSwitch coverage across sending. Service PID
is 3928; DNS PID is 4176; the parser is `dnstap-parser`, TID 2176, and the consumer
is `etw-consumer`, TID 360. Evidence is in
`wpa/pooled-auth-64000-60s-1/{inspect,early,late}`.

Drops occur in every one of the 60 sampled traffic intervals. The first interval
crosses sending start and ends at sending +0.744 seconds: 90,430 newly received
events, 1,342 drops, backlog 7,821, and observed high water 8,192. Drops continue
through the final boundary interval. Examples include 25,330 drops in the
interval ending at +5.780 seconds and 38,175 ending at +59.274 seconds. These
observations rule out an explanation confined to startup. One-second cumulative
metrics bound each increment's time; they do not identify the exact instant of
each drop.

| Measurement | Early sending 0–10 seconds | Late sending 53–60 seconds |
| --- | ---: | ---: |
| Trace-relative window | 10.6087463–20.6087463 s | 63.6087463–70.6087463 s |
| Parser switch-ins | 163,112 | 117,816 |
| Parser scheduled CPU, including observed interrupt overlap | 5.589543 s | 3.768181 s |
| Parser CPU excluding observed DPC/ISR | 4.122607 s | 2.760970 s |
| Parser Ready associated with switch-ins | 4.376618 s | 3.231837 s |
| Parser positive Wait count | 3 | 0 |
| Parser fully interior positive Wait count / total | 2 / 31.6972 ms | 0 / 0 ms |
| Parser Ready p50 / p95 / p99 / max | 18.1 / 64.3 / 103.1 / 4,569.1 µs | 16.3 / 67.6 / 134.3 / 6,466.6 µs |
| Parser previous `WrDispatchInt` observations | 160,912 | 116,299 |
| Parser previous `WrQuantumEnd` observations | 1,346 | 931 |
| Parser previous `WrPreempted` observations | 851 | 586 |
| Consumer scheduled CPU | 0.468716 s | 0.331528 s |
| Consumer CPU excluding observed DPC/ISR | 0.350882 s | 0.244780 s |
| Consumer Ready associated with switch-ins | 1.100882 s | 1.002025 s |
| Service CPU excluding observed DPC/ISR | 4.478310 s | 3.009393 s |
| DNS CPU excluding observed DPC/ISR | 8.765776 s | 6.380274 s |

The late parser has substantial Ready time and no positive dispatcher Wait
observations while the queue stays near full. The observed off-CPU delay in this
window is chiefly runnable scheduling delay. This does not establish the absence
of spinning, user-mode contention, or locking elsewhere in the stage. It does
not attribute individual dispatcher interruptions to a specific driver or cause.

The two CPU measurements use identical windows. WPA's scheduled CPU includes
observed interrupt overlap; Xperf's recorded `-exc_dpcisr` action excludes it.
Their differences, approximately 1.466936 seconds early and 1.007211 seconds
late for the parser, are interrupt-overlap estimates pending corroboration with
explicit DPC/ISR tables. Installed `xperf -help cswitch` defines
`-exc_dpcisr` as excluding observed DPC/ISR time and requires those events to be
enabled; this profile records both. Per-vCPU usage and explicit DPC/ISR/module
attribution remain follow-up analysis after load.

A bounded full-sending-window Context pass adds the following native summaries
for trace-relative 10.6087463–70.6087463 seconds:

| Measurement | vCPU 0 | vCPU 1 |
| --- | ---: | ---: |
| Mean scheduled-inclusive utilization | 77.7957% | 90.0480% |
| Mean utilization excluding observed DPC/ISR | 48.0928% | 90.0052% |
| Largest one-second scheduled-inclusive utilization | 91.90% | 96.06% |
| DPC module elapsed total | 24.949337 s | 0.027124 s |
| ISR module elapsed total | 0.282219 s | 0 s |
| `vmbus.sys` DPC elapsed | 23.074506 s | 0 s |
| `vmbkmcl.sys` DPC elapsed | 1.856044 s | 0 s |

These summaries demonstrate asymmetric interrupt activity alongside a heavily
used second vCPU and substantial parser Ready time. DPC/ISR routine elapsed
totals and scheduled-thread interrupt-overlap estimates have different
accounting; do not force them to sum or treat the routine totals as unique
exclusive occupancy. This is measured competition context, not proof that a
specific driver causes every admission drop. The completed native reports are
in `context-full-send`; each uses the same 60-second sending range.

The optional explicit DPC interval request produces unexpectedly fine-grained
output with the installed tool despite the public reference's interval wording.
The reviewer stops only the owned export and diagnostic tail processes and
preserves a cancellation receipt. The per-vCPU and DPC/ISR summary actions
already complete successfully. The helper omits that optional interval request
thereafter; no unfinished interval result enters these findings.

Source timing inspection confirms that `parse_ns` surrounds ETW parsing plus
encoding and ends before `write_frame`; `output_ns` covers frame writes,
periodic flush, and finish. Both measure elapsed time, including scheduling
delays inside the calls. Typical limiting-stage intervals accumulate about
0.90–0.92 seconds of parse time and 0.045–0.053 seconds of output time per
roughly 1.01 seconds. They do not independently establish CPU saturation or
blocking. The measured Ready time helps explain why parse elapsed time exceeds
actual parser CPU work.

A symbol-enabled late export exceeded the brief analysis window and was stopped
by terminating only its task-owned exporter. Its nonzero result, command, and
partial logs remain in `late-symbols`. Raw early and late exports complete with
exit code zero. Matching OS-symbol attribution is deferred; no symbol-derived
TDH, lock, driver, or Rust function claim is made from the partial pass.

## Management-perturbed 48k stage

Stage `pooled-auth-48000-60s-1` records 2,203 ring/intake drops out of 5,675,554
received events. Its only positive drop intervals end at sending +39.048 and
+40.089 seconds. The Windows owner identified an overlapping management
`Status` action that enumerates 5,000 authoritative DNS records. The command
takes about 7.68 seconds and overlaps those intervals. This is a material
workload perturbation; temporal overlap alone does not prove that the action
causes the drops. Preserve the trial and do not use it as an intrinsic 48k
capacity boundary. The coordinator authorizes a separately retained replacement
after the status helper becomes lightweight.

The trace itself passes inspection: canonical decompressed ETL 1,577,058,304
bytes; retained 84.2391210 seconds; reliable events 0–84.2400504 seconds;
zero lost events/buffers and continuous CSwitch coverage. Original guest and
decompressed host SHA-256 identities match. Lossless transfer compression occurs
after WPR stops and does not change the recording profile. Trace validity and
the management perturbation are separate findings. The Windows owner audits
other in-load management invocations for the final comparison.

The preserved audit at `build/management-overlap-audit-v1.json` identifies
enumeration during the original authoritative and recursive 1k baselines,
the 8k authoritative trial, and the perturbed 48k trial. The baseline matched
5–25 second precise windows precede these management envelopes; the broad
290-second measurements and full-stage PerfMon include them. Exact host command
start/end times are unavailable, so the audit records conservative envelopes
around guest receipt times and measured tool durations. Do not present these
envelopes as exact command boundaries. The coordinator authorizes clean 300-second
1k repeats in both modes after throughput work, before restoration.

## Finish-only lifecycle failure at 56k

Stage `pooled-auth-56000-60s-1` has a separate final lifecycle failure as well as
its recorded intake loss. All 6,528,255 encoded records reach the durable
receiver, with zero `output_failed_events`. After delivery, `output_errors`
increments once during `writer.finish()`. The recovered Windows event at
10:28:50.7720946 UTC identifies a STOP/FINISH exchange failure caused by the
remote host resetting the connection, Windows socket error 10054. The final
output elapsed increment is about 0.4412 ms; this is not the two-second deadline
expiring. A matching reset also appears before candidate deployment, so this
observation does not establish a pooling-specific regression.

Local `src/network.rs` sends STOP, flushes the buffered stream, and then reads
FINISH under one two-second deadline. `src/pipeline.rs` increments
`output_errors` for final finish failures independently of per-record
`output_failed_events`. Keep failed lifecycle and exact delivered-record counts
as separate findings; do not relabel this stage as wholly clean.

The installed DNS-collector release is 3.2.3, revision
`3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03`. Its
[pinned go.mod](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/go.mod)
selects `github.com/dmachard/go-framestream v1.8.0`. That dependency's
[`ResetReceiver` implementation](https://github.com/dmachard/go-framestream/blob/0326694be1a73a70d127d8c1530beb677d101fa2/framestream.go#L428-L447)
handles STOP, sends FINISH in bidirectional mode, and intentionally returns
`io.EOF`. The collector's
[`DnstapServer.HandleConn` implementation](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/workers/dnstapserver.go)
logs that EOF as “framestream reset by sender,” then exits the connection loop
and closes the connection. This log wording denotes normal STOP handling;
the same message on clean runs is not independent evidence of a TCP reset.
Preserved source extracts are in `build/shutdown-source`.

The pinned collector also selects `go-netutils v1.9.1`, commit
`1ac8345ffa557073c4cc53a84050afddac4fd20b`. Its
[`Close(conn, reset)` implementation](https://github.com/dmachard/go-netutils/blob/1ac8345ffa557073c4cc53a84050afddac4fd20b/conn.go#L90-L106)
calls `SetLinger(0)` when reset is enabled, then `CloseRead` and `Close`.
Collector
[`ResetConn` defaults to true](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/pkg/config/collectors.go#L38),
and the connection handler's deferred close passes this setting.

The Ubuntu owner verifies that active YAML omits `reset-conn`, the CLI supplies
only the config path, and the service drop-in redirects output without a
configuration override. Pinned
[`LoadConfig`](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/pkg/config/config.go#L180-L203)
applies defaults before YAML decoding. Pipeline
[`GetStanzaConfig`](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/pkg/pipeline/pipelines.go#L90-L129)
also applies defaults before decoding supplied fields with `ZeroFields: false`.
Together these establish the active pipeline's reset default through the
recorded configuration and pinned loading path; no setting is changed.

FrameStreams
[`SendFrame`](https://github.com/dmachard/go-framestream/blob/0326694be1a73a70d127d8c1530beb677d101fa2/framestream.go#L71-L75)
flushes FINISH before the collector returns from STOP handling. That buffered
flush does not guarantee peer receipt before a subsequent aggressive close.
Go's [TCP `SetLinger` documentation](https://pkg.go.dev/net#TCPConn.SetLinger)
states that zero linger discards unsent or unacknowledged data. This supplies a
concrete shutdown mechanism consistent with intermittent Windows error 10054,
which also recurs at recursive 2k with exact delivered-record reconciliation.
It does not prove the failed connection's wire ordering or which pending bytes
were affected.

A later controlled interoperability check should compare a graceful receiver
close while retaining the sender's strict FINISH validation. No source,
receiver configuration, retry, or protocol behavior changes during this campaign.

## Inspect and export a stage

Wait for the Windows owner to clear the interstage analysis window. Do not hash
large ETLs, export tables, resolve symbols, scan receiver files, or compress
artifacts during sending. Run full-file inspection before selecting a detailed
window:

```powershell
./scripts/performance/export-pooled-evidence.ps1 `
    -Action Inspect -TracePath TRACE_ETL -OutputDirectory FRESH_INSPECTION_DIR
```

Replace `TRACE_ETL` with the stage's retained ETL and
`FRESH_INSPECTION_DIR` with a directory that does not exist. The helper records
exact tool arguments, exit codes, trace hash, tool versions, full trace bounds,
event/stack counts, scheduling coverage, and process/thread lifetimes. Require
zero event and buffer loss. Check CSwitch coverage across the entire sending
window, including the first and last interval; a header duration alone does not
prove event coverage.

Export a finite matched interior window after identifying exact service and
DNS PIDs from that stage's receipts and lifetime data:

```powershell
./scripts/performance/export-pooled-evidence.ps1 `
    -Action Export -TracePath TRACE_ETL -OutputDirectory FRESH_EXPORT_DIR `
    -ViewStartSeconds START_SECONDS -ViewEndSeconds END_SECONDS `
    -TargetProcess 'etw-dnstap.exe (SERVICE_PID)','dns.exe (DNS_PID)' `
    -FileEvents
```

Replace the uppercase placeholders with actual trace-relative times, process
IDs, and paths. The helper exports raw sampled CPU, CPU Precise with new-thread
and readying stacks, disk I/O with initiation stacks, and hard faults. It also
produces Xperf scheduled CPU excluding observed DPC/ISR time, detailed disk I/O,
and hard-fault reports for the same window. Exact process filters bound stack
CSV growth. Use additional nonoverlapping windows if a short interior needs
checking against the rest of the load. Preserve the raw full-stage ETL.

Choose the interior from actual trace and client UTC timestamps, for example
five through twenty-five seconds after sending begins. Trace-relative 5–25
seconds is valid only if sending already began before that interval. Keep the
same sending-relative interval for comparisons. Record the stage's host/guest
clock bounds alongside this mapping; UTC subtraction inherits that clock
alignment uncertainty.

Summarize the raw scheduling and sampled CPU tables with the matching times:

```powershell
python scripts/performance/summarize-pooled-stacks.py `
    --directory FRESH_EXPORT_DIR --start-s START_SECONDS --end-s END_SECONDS `
    --process 'etw-dnstap.exe (SERVICE_PID)' --process 'dns.exe (DNS_PID)' `
    --output FRESH_SUMMARY_JSON
```

Replace the uppercase placeholders with the same values used for export and a
summary file that does not exist. The helper rejects collapsed scheduling rows
and reports positive-duration quantiles, boundary counts, stack coverage,
readying identities, sampled modules, and missing target processes. Preserve
raw CSVs so reviewers can inspect each contributing switch.

For a broad window exported with `-Tables Sampled,Disk,HardFault -FileEvents`,
summarize native process CPU, sample modules, disk/fault counts, and file events:

```powershell
python scripts/performance/summarize-pooled-broad.py `
    --directory FRESH_EXPORT_DIR --process 'etw-dnstap.exe (SERVICE_PID)' `
    --process 'dns.exe (DNS_PID)' --output FRESH_SUMMARY_JSON
```

Use the same process names and a fresh summary path. The helper reads the export
receipt's exact range and streams the file-event dump to bound memory use.

`-FileEvents` exports the selected window's classic FileIo events, including
other processes, to retain file-object/name and operation-end joins. It uses the
official [FileIo class GUID](https://learn.microsoft.com/windows/win32/etw/fileio)
`90cbdc39-4a3e-11d1-84f4-0000f80464e3`; no unsupported Xperf `fileio` action or
invented WPA table columns are required. Confirm the native action succeeds and
contains file events before citing this optional report.

The exporter can require execution outside the filesystem sandbox because its
plugin discovery traverses installed WPT components. Probe export succeeded
with that execution permission. It did not require a guest configuration change.
The helper refuses to replace an existing evidence directory. Microsoft's
[WPA Exporter reference](https://learn.microsoft.com/windows-hardware/test/wpt/exporter)
documents profile, range, and symbol-loading parameters.

Use `-Action Context` with a fresh output directory and the same explicit start
and end seconds to export per-vCPU scheduling utilization, the corresponding
exclusive view, DPC/ISR module summaries, and sampled process/module data. This
action performs no WPA stack export or symbol loading. Run it only in a cleared
analysis window. Microsoft's [CSwitch reference](https://learn.microsoft.com/windows-hardware/test/wpt/cswitch)
defines exclusive interrupt accounting, and its
[DPC/ISR reference](https://learn.microsoft.com/windows-hardware/test/wpt/dpcisr)
describes the summary action.

`summarize-pooled-trace-gates.py` refreshes the derivative
`wpa/trace-gates-v1.json` from small existing inspection receipts and logs. It
does not reopen or hash ETLs. The index records canonical identities, retained
and actual bounds, event/buffer loss, continuous CSwitch coverage, UTC client
containment, management-perturbation flags, and source paths. A passing trace
gate remains separate from the stage's application and receiver gates.

## Interpret scheduling and symbols

Identify `dnstap-parser` by the stage's actual thread name and TID, and preserve its
lifetime. Report scheduled CPU, switch count, positive Ready and Wait duration
distributions, wait reasons, readying process/TID, stack coverage, and switch-in
gaps. Keep raw positive-duration quantiles distinct from occupancy clipped to a
window. A wait that starts before the view can extend into it; a final wait
without an in-view switch-in can be absent. Thread-seconds can exceed wall time
when several threads are combined.

Ready time is scheduling delay after a thread becomes runnable. Wait time ends
when the thread becomes runnable. Dispatcher wait reason alone does not identify
a lock. A parser wait duration is neither DNS response latency nor per-record
end-to-end capture latency. Correlate both stacks with readying identity and
the disk, file, and hard-fault timelines before attributing an observed delay to a lock, timer,
output, I/O, or CPU contention. Microsoft's
[CPU analysis guide](https://learn.microsoft.com/windows-hardware/test/wpt/cpu-analysis)
defines these columns and describes readying-stack analysis.

The audited production executable has no CodeView/PDB reference and is stripped.
Preserve that exact binary. Public matching OS symbols can resolve OS frames;
they cannot recover Rust source function names in this executable. `-Symbols`
requests symbol decoding using the recorded symbol-path environment. Report
symbol failures and unresolved application frames. A later symbol-enabled
rebuild would be a different executable and cannot supply matching application
symbols for this trace.

Treat source-level timer and allocation explanations as hypotheses until native
observations support them. A configured 2 ms park timeout is not proof of a
2 ms wake deadline. WPR loss checks cover the trace session; application intake,
parser, output, DNS-client, and durable receiver ledgers remain separate gates.
