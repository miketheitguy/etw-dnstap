# Pooled VM trace analysis

Use this note to review native Windows Performance Recorder (WPR) evidence for
the completed campaign at
`target/performance/pooled-vm-20261029-0942`. The campaign plan and final review
own workload decisions and capture results. This note helps maintainers inspect
the trace probe,
export procedure, and limits on scheduling and stack attribution.

All 22 completed load ETLs pass the final native trace gate: zero lost events
or buffers, continuous one-second CSwitch coverage, and containment of each
complete sending interval. Per-run `wpa/RUN_ID/trace-gate-v1.json` receipts and
the derivative `wpa/trace-gates-v1.json` retain canonical identities and source
paths. This result describes WPR recording fidelity; the campaign review reports
application admission loss, DNS flag failures, and lifecycle failures separately.
All file paths without a repository prefix are relative to that campaign root.
For units and independent pass criteria, see
[Measurement terms and result gates](measurement-terms.md).

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

## Primary five-minute comparisons

Use the management-clean `pooled-auth-1000-300s-2` and
`pooled-rec-1000-300s-2` repeats as the primary low-rate resource evidence.
The recursive repeat retains one RA=false response; its complete delivery and
native trace pass do not clear that DNS-validity failure. The sustained
confirmations are `pooled-auth-48000-300s-1` and `pooled-rec-28000-300s-1`.
All four use the same production executable and full file-mode recording profile.

The broad native views cover sending seconds 5–295. CPU in this table excludes observed
DPC/ISR according to the recorded Xperf action; percentages use one core.
Private bytes are the maximum from the separate full-sending PDH series.

| Stage | Actual QPS | Broad trace-relative view, s | Service CPU, s / % | DNS CPU, s / % | Service maximum private MiB |
| --- | ---: | --- | ---: | ---: | ---: |
| Authoritative 1k repeat | 999.70 | 16.004688–306.004688 | 2.078489 / 0.716720 | 10.975200 / 3.784552 | 41.594 |
| Recursive 1k repeat, one RA=false | 999.68 | 15.651842–305.651842 | 2.955109 / 1.019003 | 19.786422 / 6.822904 | 41.797 |
| Authoritative 48k confirmation | 47,303.24 | 15.914268–305.914268 | 107.064749 / 36.918879 | 237.642071 / 81.945542 | 41.180 |
| Recursive 28k confirmation | 27,758.24 | 15.541131–305.541131 | 72.981012 / 25.165866 | 236.019012 / 81.385866 | 41.035 |

Service sampled weights in these broad windows are respectively 1,788.5764,
2,607.8676, 135,510.9296, and 91,575.5919 ms. These statistical weights include
different attribution from exclusive CPU and must remain separate. PDH CPU also
uses its own full-sending valid intervals; neither its integrated CPU nor the
sample weights must equal the native 290-second CPU totals. The main campaign
review retains PDH coverage and memory/thread series.

The broad I/O observations are:

| Stage | Service FileIo metrics writes / bytes | Service disk events / hard faults | DNS disk events / hard faults |
| --- | ---: | ---: | ---: |
| Authoritative 1k repeat | 292 / 163,162 | 1 / 1 | 0 / 0 |
| Recursive 1k repeat | 293 / 163,916 | 0 / 0 | 0 / 0 |
| Authoritative 48k confirmation | 292 / 166,581 | 0 / 0 | 0 / 0 |
| Recursive 28k confirmation | 291 / 165,691 | 0 / 0 | 272 / 272 |

The authoritative repeat's single disk read and hard fault concern 4,096 bytes
of `tdh.dll` at trace-relative approximately 196.135 seconds. Disk I/O elapsed
time is 0.144 ms; the hard-fault I/O measure is 0.1782 ms. Recursive confirmation
DNS reads concern `pagefile.sys`: 10,006,528 bytes, 46.9789 ms of summed disk I/O
time, and 59.8153 ms of summed hard-fault I/O time. These measures use different
event boundaries. They do not establish a throughput bottleneck. Every observed
service FileIo write in these broad views points to its metrics JSONL. Cached
metrics writes need not appear as service-attributed physical disk writes;
System can issue their later disk I/O. TCP output remains separate from FileIo.

The matched precise views cover sending seconds 5–25. The parser observations
for the primary low-rate repeats and recursive confirmation are:

| Stage / parser TID | Scheduled / exclusive CPU, ms | Raw positive Ready total, ms | Raw positive Wait count / total, s | Wait p50 / p95 / p99 / max, ms |
| --- | ---: | ---: | ---: | ---: |
| Authoritative 1k / 1360 | 127.8208 / 123.687 | 32.6040 | 1,298 / 19.8443181 | 15.9583 / 16.3368 / 16.4800 / 16.5595 |
| Recursive 1k / 4848 | 179.0088 / 172.443 | 35.6196 | 1,282 / 19.7938566 | 15.9474 / 16.2403 / 16.4483 / 16.5308 |
| Recursive 28k / 504 | 5,850.6328 / 4,614.718 | 3,353.1895 | 1,166 / 10.8028726 | 10.5484 / 13.7119 / 15.3933 / 17.6319 |

Each parser row has one positive Wait crossing the left boundary. Fully interior
positive Wait totals are respectively 19.8286280, 19.7778919, and 10.7907006 s.
Ready p50/p95/p99/max values are 13.7/77.9/153.3/892.5 µs for authoritative 1k,
14.4/87.6/168.5/584.8 µs for recursive 1k, and
17.8/75.4/114.5/1,968.9 µs for recursive 28k. The roughly 16 ms low-rate waits
persist in the management-clean repeats; the measurement does not establish
timer or lock causality.

The matching ETW consumers are TID 1344, 1988, and 1624. Their scheduled /
exclusive CPU totals are 13.6874/13.387, 20.5645/19.923, and
573.8534/448.459 ms. Raw positive Ready totals are 6.0268, 8.9538, and
613.1907 ms; positive Wait medians are 94.0091, 63.9811, and 2.8466 ms.
Each consumer also has one left-boundary positive Wait. The parser and consumer
lifetimes begin before sending and end after drain; exact trace-relative
lifetimes, raw/interior distributions, clipped occupancy, and source paths are
in `wpa/native-comparison-v1.json` and each `matched/scheduling-summary.json`.
Scheduling/sample CSVs filter only the service; Xperf process/thread CPU logs
retain DNS and other processes. No full-stage absence of locks is inferred.

Broad summary sources are `broad/summary-v2.json` for the authoritative 1k repeat
and recursive 28k confirmation, and `broad/summary.json` for the recursive 1k
repeat and authoritative 48k confirmation. All reside under the corresponding
`wpa/RUN_ID` directory. All native actions used for these comparison views
complete with exit code zero.
The original baselines in the following sections remain preserved with their management-observer
qualification. The earlier capacity-only baseline has a different partial WPR
profile, so these observations do not establish a causal CPU improvement from
pooling.

## Original authoritative five-minute observations

This original baseline includes a later in-load management enumeration. Its
capture ledger remains valid; its broad resource measurements carry that
qualification. The separately authorized clean repeat supplies the primary
five-minute resource comparison; it is complete and included in
[Primary five-minute comparisons](#primary-five-minute-comparisons).

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

## Original recursive five-minute observations

This original baseline also includes a later in-load management enumeration.
Preserve its capture result and use the authorized clean repeat as the primary
resource comparison. That repeat is complete and included in
[Primary five-minute comparisons](#primary-five-minute-comparisons).

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

## Authoritative sustained confirmation window

The clean `pooled-auth-48000-300s-1` confirmation has a 6,859,784,192-byte ETL,
zero trace event/buffer loss, and continuous full-sending coverage. Its matched
view covers trace-relative 15.914268–35.914268 seconds, or sending seconds 5–25.
The scheduling and sampled CSVs include only service PID 3796 to bound stack
output. The matching Xperf process/thread CPU report also contains DNS PID 4176.
All native export actions complete with exit code zero. Evidence is in
`wpa/pooled-auth-48000-300s-1/{inspect,matched}`.

| Measurement | Parser | ETW consumer |
| --- | ---: | ---: |
| Named thread TID | 2272 | 4524 |
| Trace-relative lifetime | 3.349710–316.319311 s | 3.350094–316.318826 s |
| Switch-ins in matched view | 207,654 | 33,688 |
| Scheduled CPU, including observed interrupt overlap | 8.2692395 s | 0.9245142 s |
| CPU excluding observed DPC/ISR | 6.584242 s | 0.672077 s |
| Raw positive Ready total | 4.6165969 s | 1.2861393 s |
| Ready p50 / p95 / p99 / max | 16.4 / 51.8 / 81.3 / 6,058.0 µs | 13.6 / 59.2 / 203.1 / 14,913.8 µs |
| Raw positive Wait count / total | 753 / 7.1141733 s | 9,535 / 17.7889024 s |
| Fully interior positive Wait count / total | 753 / 7.1141733 s | 9,534 / 17.7862788 s |
| Wait p50 / p95 / p99 / max | 9.2746 / 16.5409 / 17.4809 / 17.6916 ms | 2.0754 / 3.0044 / 3.2820 / 14.1332 ms |
| Left-boundary Ready / Wait observations | 1 / 1 | 0 / 1 |

The left-boundary parser observations have zero positive Wait; all 753 positive
parser waits are fully interior. The consumer's raw Wait total includes one
positive interval crossing the left boundary. Raw intervals, clipped occupancy,
and fully interior distributions remain separate in `scheduling-summary.json`;
they are not a required decomposition of the view's 20-second wall time.

The service uses 7.265114 CPU seconds excluding observed DPC/ISR, and DNS uses
16.244476 CPU seconds in the same view. Service sampled weight is 9,239.0827 ms,
with unresolved instruction modules including `ntoskrnl.exe` (3,060.9622 ms),
`tdh.dll` (1,968.4421 ms), `ntdll.dll` (1,811.7872 ms), and the application
(1,410.9157 ms). Sample weights and scheduled/exclusive CPU use different
accounting. No matching Rust functions are recovered.

This successful high-rate window contains substantial parser dispatcher Wait
and Ready time. The limiting 64k windows contain very little or no positive
parser Wait while the queue stays near full. Those observations distinguish
the measured windows; they do not identify a lock, timer mechanism, or sole
cause of capacity loss.

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
late for the parser, reflect those actions' different interrupt accounting.
They are not interchangeable with elapsed DPC/ISR routine totals. Installed
`xperf -help cswitch` defines
`-exc_dpcisr` as excluding observed DPC/ISR time and requires those events to be
enabled; this profile records both. The per-vCPU and explicit DPC/ISR summaries
provide additional context in the following table.

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

## Recursive 32k loss-centered windows

`pooled-rec-32000-60s-1` delivers 31,691.27 client QPS but loses 7,535 of
4,379,440 received events at ring admission. Its three positive one-second loss
observations end at sending +0.628 s (6,265 drops), +46.088 s (684), and
+47.106 s (586). The latter two establish that loss is not confined to startup.
The queue has fallen to 889, 899, and 292 respectively by those observations;
these endpoints do not reconstruct the queue within each interval.

The 1,168,113,664-byte native trace passes full-file inspection with zero lost
events/buffers and continuous sending coverage. Service PID is 4884. Parser
TID 2904 lives at trace-relative 3.334621–78.320317 s; consumer TID 1872 lives
at 3.335007–78.319765 s. Detailed views cover sending 0–3 s and 44–49 s,
corresponding to trace-relative 10.756463–13.756463 s and
54.756463–59.756463 s. Evidence is in
`wpa/pooled-rec-32000-60s-1/{early-loss,late-loss}`.

| Measurement | Early 3-second view | Later 5-second view |
| --- | ---: | ---: |
| Parser scheduled / exclusive CPU | 1.0372728 / 0.806395 s | 1.7567959 / 1.377934 s |
| Parser switch-ins | 25,458 | 39,046 |
| Parser raw positive Ready total | 0.7069856 s | 1.3030591 s |
| Parser Ready p50 / p95 / p99 / max | 16.7 / 82.4 / 133.8 / 769.3 µs | 19.2 / 94.5 / 205.2 / 2,390.9 µs |
| Parser raw positive Wait count / total | 128 / 1.2526110 s | 225 / 1.9371606 s |
| Parser fully interior positive Wait count / total | 127 / 1.2366850 s | 224 / 1.9281816 s |
| Parser clipped Wait occupancy associated with in-view switch-ins | 1.2487066 s | 1.9304249 s |
| Consumer scheduled / exclusive CPU | 0.0977210 / 0.075913 s | 0.1725564 / 0.134359 s |
| Consumer raw positive Ready total | 0.1390153 s | 0.3278697 s |
| Consumer fully interior positive Wait count / total | 1,100 / 2.7491352 s | 1,789 / 4.4971653 s |
| Consumer clipped Wait occupancy associated with in-view switch-ins | 2.7632443 s | 4.4986810 s |

Both threads have one positive Wait crossing the left boundary in each view.
The early consumer's raw Wait total is 10.1839291 s because it includes a
7.4347939-second pre-load wait. That raw total is not occupancy inside the
three-second view. Clipped values remain subject to the missing final-tail
limitation when no further switch-in occurs inside the view. Do not force
scheduled, Ready, and Wait totals to sum to wall time.

Both loss-centered windows contain substantial positive parser Wait as well
as Ready time. This differs from the continuously dropping authoritative 64k
late window, which has no positive parser Wait observations. The recursive
windows do not support a claim of uninterrupted runnable-only parser delay
throughout either view. These three- and five-second views include time before
and after the one-second loss observations. Positive Wait elsewhere in a view
does not establish that the parser waits at an exact drop instant. Their
averages cannot identify the exact short burst
that overflows the ring or establish a TDH lock, timer, driver, or sole scheduler
cause. The retained one-second counters bound drop timing; native events and
stack module offsets retain evidence for a later focused diagnosis.

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

## Auxiliary receiver counter discrepancy

The authoritative 48k five-minute receiver window reports 28,381,946 records
in all four tap/console worker ingress/egress counters and durable JSON. Its
`dnscollector_exporter_policy_forwarded_total{worker="tap"}` delta is
28,379,834, a deficit of 2,112 (33 times 64). The resource receipt is
`ubuntu/pooled-auth-48000-300s-1-start--pooled-auth-48000-300s-1-end-resources.json`.
The worker and durable-record reconciliation remains the delivery gate; this
auxiliary counter does not establish missing records.

Read-only inspection of the pinned collector identifies a potential counter
accounting data race. In
[`sendBatch`](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/workers/worker.go#L370-L395),
the producer sends a batch pointer to the route before reading
`len(b.Messages)` to increment the forwarded counter. The
[`StdOut` consumer](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/workers/stdout.go#L125-L150)
processes and releases that input batch. The batch's
[`Release`](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/dnsutils/dnsmessage_batch.go#L52-L63)
resets `Messages` to length zero before returning it to the pool. No additional
producer reference or synchronization protects the producer's post-send length
read on this single-route path. A consumer release before that read can therefore
undercount the metric even after successful delivery. This mechanism is
consistent with the observed batch-sized deficit; no runtime race reproduction
or per-deficit attribution was performed.

The cached pinned source bytes and hashes are preserved in
`build/shutdown-source/forwarded-metric-source-receipt.json`. No collector patch,
receiver option change, or additional workload is part of this source check.

## Inspect and export a stage

Use Windows PowerShell with the Windows Performance Toolkit (WPT), the installed
Xperf and WPA Exporter versions recorded with the campaign, and access to its
preserved native ETLs. Run the following examples from the repository root.
These are offline evidence exports, not new load tests.

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

The installed WPA Hard Faults Summary table can return events outside the
requested export range. The broad summarizer explicitly filters its `Log Time`
to the receipt's half-open time interval and reports excluded event counts.
Disk events use initiation/completion overlap with that interval. In the clean
authoritative 1k broad view, this excludes 13 earlier service faults and two
earlier DNS faults, leaving one service fault, consistent with the independent
Xperf range report. Preserve the unfiltered native CSV. The corrected derivative
for that view is `broad/summary-v2.json`; its initial summary remains preserved.

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

After the comparison views and summaries exist, create their derivative index:

```powershell
python scripts/performance/summarize-pooled-native.py `
    --campaign CAMPAIGN_DIR --output FRESH_COMPARISON_JSON
```

Replace `CAMPAIGN_DIR` with the campaign root and `FRESH_COMPARISON_JSON` with a
file that does not exist. The helper joins the four primary five-minute views
and two recursive 32k loss views using saved summaries, exact process/thread CPU
rows, and lifetime logs. It prefers corrected `summary-v2.json` derivatives when
present, requires explicit I/O filtering metadata, and does not reopen ETLs.
The campaign's completed index is `wpa/native-comparison-v1.json`.

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

After an export, confirm that its receipt records successful native exit codes,
the intended process IDs and time range, and complete source paths. Preserve
failed or partial exports separately. Use the
[campaign review](pooled-vm-performance-review.md) to assess strict stage results;
trace fidelity alone does not establish capture or DNS success.
