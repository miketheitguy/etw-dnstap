# DNS capture performance review — 2026-10-28

> **Status:** Historical · **Evidence dated:** October 28, 2026 PDT · **Commit:** not recorded · **Reviewed:** 2026-11-01

This historical report helps maintainers interpret the original unpaced
experiment on the 1,024-entry allocation-based ring. All binary identities,
test counts, and installed-state descriptions apply at its recorded restoration
cutoff. For the later bounded pooled campaign, see the
[pooled VM review](../performance/pooled-vm-performance-review.md).

This record describes historical binaries with service JSONL metrics
instrumentation. The TCP-only service has no durable metrics export; its
instrumentation and performance have not been measured by this campaign.
See the [instrumentation compatibility note](../README.md#instrumentation-compatibility-note). Preserved counters,
tables, hashes, and collector artifacts retain their historical meaning.

The instrumented service remained small and completed all three 300-second
captures cleanly, but it did not retain full-fidelity DNS telemetry at the
offered load. In the authoritative run, DNS completed 53.95 million queries
without client timeouts while 89.42% of the corresponding capture records were
lost before durable receiver output. The repaired recursive-request run had a
similar 89.93% capture-record shortfall. DNS response correctness and dnstap
capture completeness are separate results.

The first recursive run exposed an upstream UDP receive-buffer limitation. It
is retained as a complete, fixture-limited observation. A temporary buffer
repair passed a zero-drop burst test before the final five-minute comparison.
No service throughput optimization, capacity change, privilege expansion, or
Windows DNS cache-policy change was made between measured runs.

## Configuration and reproducibility

The measured configuration is:

- Windows: `DNSTap-WS2025`, GUID
  `24bafedb-0998-4430-a0c6-7e90350cd9ac`, Windows Server 2025 Core,
  `172.27.215.163`, 2 vCPUs and 4 GiB RAM.
- Host load generator: compiled Rust executable, explicit VM destination
  `172.27.215.163:53`, two independent workers, 256 outstanding requests per
  worker, 2,000 ms response deadline, unpaced bounded closed loop.
- Names: 5,000 common names from pinned SecLists provenance in
  [`datasets/performance/workload-provenance.json`](../../datasets/performance/workload-provenance.json).
  `wpad` and `isatap` are excluded while the Windows global query block list
  remains enabled. The owned authoritative zone has exactly 5,000 A records.
- Authoritative requests use RD=0 and TTL 300. Recursive requests use RD=1 and
  the same repeated 5,000-name working set through the controlled Ubuntu
  upstream, whose answers have TTL 0. Repeated `www` and `bills` fixture probes
  verified answers and TTLs before the measurements. The repaired run recorded
  1,496,947 upstream queries for 51,448,594 client requests (a 2.91% count ratio).
  This repeated recursive-request workload does not measure pure cache-miss
  capacity or establish one upstream lookup per client request. Cache and
  coalescing mechanisms were not independently isolated.
- Service: SCM-only `EtwDnstap`, LocalSystem, enabled service SID,
  `SeChangeNotifyPrivilege` allowlist, ring capacity 1,024. Optional one-second
  protected JSONL metrics, one extra metrics thread, and per-event timing were
  enabled consistently. The benchmark includes this instrumentation overhead.
- Receiver: the existing dnscollector JSON output was redirected to a durable
  append-only file during measurement. This is a file-output configuration;
  the results do not establish console/journal throughput. Original console
  output is restored afterward.

Measured service SHA-256:
`0FCDD17E08A3DDD42F4F132F7F03F80A5513FEBC66E54F90A5EBBC8135BE2C1D`.
Original installed backup SHA-256:
`2487FFEFA54D4F6FC2665695E564F6F2B0F254A6C2FA67D8443F4B97AC04E792`.
Client SHA-256:
`7A5800CFC4252196E4C6162F7D2CCF81217FE95A339D7CFBB9CD84063F4B6960`.

The [test plan](performance-test-plan.md) records the delegated chunks,
models, reasoning efforts, acceptance gates, commands, and restoration policy.
All coding used GPT-6.1-Sol/high workers; coordination and independent
read-only review used GPT-6-Astra/high.

## Client results

All timestamps in this section are UTC on **2026-10-29** (the evening of October 28 PDT).
The sending windows are exactly 300 seconds; response drain and service/ETW
shutdown occur afterward. Per-run `invocation.json`, `summary.json`,
`metrics.jsonl`, host samples, and exit records are under
`target/performance/client/RUN_ID/`.

| Run | Sending window UTC | Sent / matched | Sent QPS | Successful | Timeouts | SERVFAIL | RA missing |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |
| `auth-1` | 04:58:44.117–05:03:44.117 | 53,952,700 / 53,952,700 | 179,842.33 | 53,952,700 | 0 | 0 | 0 |
| `rec-1`, fixture limited | 05:08:19.061–05:13:19.061 | 14,177,516 / 14,138,580 | 47,258.39 | 14,106,151 | 38,936 | 32,429 | 0 |
| `rec-2`, repaired upstream | 05:21:01.140–05:26:01.140 | 51,448,594 / 51,448,594 | 171,495.31 | 51,448,394 | 0 | 0 | 200 |

| Run | p50 | p95 | p99 | Maximum matched latency |
| --- | ---: | ---: | ---: | ---: |
| `auth-1` | 2.884 ms | 3.342 ms | 3.604 ms | 69.803 ms |
| `rec-1` | 2.818 ms | 3.670 ms | 6.160 ms | 1,999.360 ms |
| `rec-2` | 2.884 ms | 3.932 ms | 7.078 ms | 55.232 ms |

Quantiles are fixed-histogram bin upper bounds for matched responses. They
exclude timeouts and have no coordinated-omission correction. “Successful”
requires NOERROR, an answer, and the requested flag checks; it does not validate
each response's A address/TTL. Independent fixture probes cover those values.
The 200 RA-bit omissions in `rec-2` are reported separately from transport
timeouts and SERVFAIL responses. The independent receiver retained five
explicit RA=false, AA=true, RD=true, NOERROR client responses with answers at
05:23:00.5223283–05:23:00.5225168 UTC. Their fields were present, not missing
JSON defaults. Other retained client responses had RA=true. Heavy capture loss
prevents matching those five records to all 200 direct-client observations;
no cause is attributed to the capture service.

One-, two-, four-, and eight-thread pilots achieved 116.5k, 180.1k, 188.0k,
and 152.9k QPS. Two workers gave approximately 96% of the four-thread peak.
Eight workers had 4,312 timeouts and spent approximately 31.6% of worker sending
time waiting for safe endpoint/transaction-ID rotation. Pilot tracing/rundown
also limits comparisons between these short runs. The two-worker choice is a
reproducible offered-load setting, not proof of the DNS server's absolute
maximum capacity. The first host monitoring wrapper attempt lacked permission
for host CIM counters, and two early wrappers did not preserve the process
handle needed for an exit code. Their completed client summaries were retained;
the corrected wrapper and exit-code checks were used for all measured runs.

## Loss by stage

These are stable final counters after the native consumer stopped and the
parser joined. Live relaxed-atomic samples can straddle updates; ledger
equalities are checked only on final samples.

| Run | Supported callback records received | Ring drops | Encoded / accepted by writer | ETW EventsLost | Real-time buffers lost |
| --- | ---: | ---: | ---: | ---: | ---: |
| `auth-1` | 102,723,800 | 91,309,674 | 11,414,126 | 5,181,600 | 0 |
| `rec-1` | 29,892,473 | 26,207,487 | 3,684,986 | 293,365 | 0 |
| `rec-2` | 101,271,058 | 90,606,333 | 10,664,725 | 4,620,024 | 0 |

Every row satisfies `received = ring drops + dequeued` and
`dequeued = encoded`. All three runs have zero final backlog, callback-copy
errors, parser rejections, ignored records, output-limit drops, failed-frame
events, output-operation errors, and ETW statistics-query failures. Final ETW
statistics are valid. Observed queue high water reached the configured 1,024
slots in every run; one-second backlog samples do not measure how long bursts
spent full.

The following controlled-workload equalities also hold, using actual upstream
packet counts rather than assuming four events per recursive client query:

```text
auth-1: 2 × 53,952,700
        = 107,905,400
        = 102,723,800 received + 5,181,600 ETW EventsLost

rec-1:  2 × 14,177,516 + 919,446 upstream queries + 911,360 upstream replies
        = 30,185,838
        = 29,892,473 received + 293,365 ETW EventsLost

rec-2:  2 × 51,448,594 + 1,496,947 upstream queries + 1,496,947 upstream replies
        = 105,891,082
        = 101,271,058 received + 4,620,024 ETW EventsLost
```

For these checked domains/event counts, the capture-record shortfalls are
96,491,274 (89.42%), 26,500,852 (87.79%), and 95,226,357 (89.93%). These are
not DNS packet-loss percentages: authoritative client queries all completed,
and timed-out recursive client queries can receive later server responses.
The equalities were established from this workload's drained counters; they
are not a general rule that every timed-out query produces a response.

ETW `EventsLost` applies to all enabled session event IDs, whereas `received`
counts the five supported IDs. Their domains must be checked before combining
them. `RealTimeBuffersLost` is a buffer count and is never added to event counts.
“Encoded” means the writer accepted a frame, not a per-record receiver ACK.
The independent receiver file/ingress/egress reconciliation establishes the
next stage separately.

| Run | Encoded | Durable JSON records | Receiver ingress / egress | Worker discards | Malformed JSON / DNS |
| --- | ---: | ---: | ---: | ---: | ---: |
| `auth-1` | 11,414,126 | 11,414,126 | 11,414,126 / 11,414,126 | 0 | 0 / 0 |
| `rec-1` | 3,684,986 | 3,684,986 | 3,684,986 / 3,684,986 | 0 | 0 / 0 |
| `rec-2` | 10,664,725 | 10,664,725 | 10,664,725 / 10,664,725 | 0 | 0 / 0 |

There was no observed encoded-to-durable-output record loss. The recursive
fixture's first run had 8,086 upstream receive-buffer drops, exactly matching
919,446 queries minus 911,360 replies. The repaired socket had an observed
8 MiB receive buffer and zero new UDP errors throughout the burst pilot and
`rec-2`. All 161,946 retained `rec-2` resolver replies and all 5,176,460 retained
positive client responses had TTL 0.

## Resource consumption

Process CPU is summed across processors: **100% means one logical CPU**.
Divide by two for percentage of this guest's capacity. Means and p95 values
in this section are duration weighted over valid interval-average rates; maxima are
observed interval averages, not instantaneous peaks.

| Run | Service CPU mean / p95 / max, one-CPU % | Service mean, guest % | DNS CPU mean, one-CPU % | Guest total CPU mean |
| --- | ---: | ---: | ---: | ---: |
| `auth-1` | 18.04 / 29.33 / 40.73 | 9.02 | 127.53 | 96.24% |
| `rec-1` | 5.71 / 20.29 / 30.95 | 2.85 | 40.05 | 31.03% |
| `rec-2` | 17.42 / 27.93 / 35.73 | 8.71 | 130.51 | 96.80% |

| Run | Service private MiB, start → end | Private peak MiB | Working-set peak MiB | Service threads | Service handles |
| --- | ---: | ---: | ---: | ---: | ---: |
| `auth-1` | 2.617 → 2.551 | 4.988 | 10.594 | 6–9 | 97–102 |
| `rec-1` | 2.504 → 2.629 | 5.641 | 11.055 | 6–9 | 97–102 |
| `rec-2` | 2.496 → 2.840 | 7.402 | 13.016 | 7–9 | 98–102 |

The service shows no sustained unbounded growth over these short observations;
that does not prove absence of a leak. DNS process private memory increased by
0.539 MiB, 128.289 MiB, and 123.703 MiB respectively. That recursive DNS heap
growth belongs to the DNS process and has not been diagnosed as a leak.

Service PerfMon write throughput averaged approximately 507–509 bytes/s,
principally the small metrics stream. Those process I/O counters do not
represent full Winsock traffic. Receiver JSON output was approximately 11.08 GB
for `auth-1`, 3.57 GB for `rec-1`, and 10.33 GB for `rec-2`; network and receiver
resource counters are preserved separately. Host client CPU was approximately
198.3% of one CPU, or 8.3% of the 24-logical-CPU host. The workers busy poll;
their CPU occupancy alone does not establish a client computation bottleneck.

| Run | Service I/O read / write / other, bytes/s sample means | DNS I/O other, KiB/s sample mean | Guest ready-queue mean / p95 | Disk read / write latency mean, ms | Disk queue mean |
| --- | ---: | ---: | ---: | ---: | ---: |
| `auth-1` | 0 / 509.1 / 0 | 752.1 | 2.77 / 4 | 0.068 / 1.593 | 0.035 |
| `rec-1` | 0 / 507.2 / 0 | 530.9 | 0.88 / 3 | 0.020 / 1.561 | 0.030 |
| `rec-2` | 0 / 507.4 / 30.0 | 1,236.7 | 2.78 / 4 | 0.089 / 1.532 | 0.025 |

Queue and disk statistics use duration weighting; the explicitly labeled
process I/O sample means retain PerfMon accounting. Disk latency means are
duration-weighted interval averages, rather than I/O-count-weighted averages.
The BLG/CSV and analysis
also retain operation rates, page faults, working/private working sets, virtual
bytes, available memory, paging, and network/UDP/TCP counters.

The corrected recursive run kept the upstream and receiver lightly loaded:
approximately 5.69% and 13.65% of one CPU respectively, with approximately
6.05 MB and 74.73 MB RSS. Receiver disk busy time was approximately 0.96%.
The receiver reported zero worker discards. Byte values labeled MB/GB in this section are
decimal; service memory in the table is MiB.

## Counter, trace, and interpretation limits

`logman` collected BLG counters at a requested one-second interval; `relog`
exported CSV. Actual service/DNS PIDs were validated before attribution. The
client sending windows contained 280, 300, and 279 valid process samples, with
299.735, 299.622, and 299.549 seconds of CPU-rate coverage. Maximum observed
sample gaps were 7.271, 1.572, and 8.819 seconds. Weighted calculations account
for interval lengths; the first clipped interval is an approximation of its
underlying PDH average. Exact durations and raw samples remain in each
`target/performance/windows/RUN_ID/analysis.json`.

Thread instance slots can be reused. The analyzer requires the same instance,
PID, and TID in consecutive CSV samples for rates; it excludes impossible CPU
rates instead of clamping them. Review caught this before final thread results
were used. It rejected 32, 38, and 41 service thread-rate values respectively
for identity changes. Frozen thread instances also miss later-created slots;
the report records coverage, including incomplete DNS coverage in 114 of the
280 authoritative samples. Current thread-state snapshots are not evidence
of lock contention.

WPR General-profile memory recordings requested 45 seconds during each load.
The authoritative ETL header spans 54.334 seconds including rundown, but its
circular buffers retain only the last approximately 17 seconds of busy
CPU/context-switch data. Zero ETL lost-event counters do not undo that circular
overwrite. Trace claims use retained interior intervals, not the whole header
duration. Recording and rundown overhead are included in the measured resource
windows.

In a retained five-second authoritative interval, context-switch accounting
showed approximately 16.4% of one CPU for the service and 134.6% for DNS;
`vmbus.sys` and `vmbkmcl.sys` DPC activity contributed approximately 37.9% of one
CPU. The full authoritative run used approximately 54 service CPU-seconds,
while the parser's measured parse/encode wall time was 251.96 seconds plus
12.95 seconds for output. Parse timing begins after dequeue and excludes the
empty-ring two-millisecond park. The wall/CPU difference therefore includes
descheduling or waits during active parse/encode; it does not establish a
fully CPU-bound parser or a particular lock bottleneck.

An independent WPA CPU Usage (Precise) export over ETL seconds **37–53**
(a separate 16-second view) identifies the named service threads:

| Thread / TID | CPU usage, ms | Associated Ready, ms | Associated Wait, ms |
| --- | ---: | ---: | ---: |
| `dnstap-parser` / 2184 | 3,293.14 | 12,204.74 | 503.30 |
| `etw-consumer` / 3624 | 1,284.39 | 9,307.45 | 5,405.26 |

Maximum associated parser Ready and Wait durations were 49.725 and 15.709 ms.
These Ready/Wait sums are associated with switch-ins in the
selected view: a first interval can cross its beginning and a tail without an
in-view switch-in can be absent. They are not asserted to be exactly clipped
occupancy fractions or directly compared with the different five-second xperf
view described in this section. The large Ready durations provide scheduling-pressure evidence;
they do not identify a contended application lock. Raw CSV and the aggregate
are under `target/performance/review/wpa-auth-precise` and
`target/performance/review/auth-cpu-ready-wait-threads.json`.

The two-vCPU guest's high total CPU, ready queue, DNS workers, virtualization
DPC activity, and burst-full ring support scheduler/burst handling as important
follow-up targets. Native property extraction also merits profiling: schemas
are already cached by event ID/version, but five fields still invoke
`TdhGetPropertySize` and `TdhGetProperty` per record. No optimization or lock
attribution is claimed from source inspection alone.

## Receiver evidence and restoration

Receiver start/end byte-offset markers enclose each service session, including
final ETW/parser drain after the 300-second sending window. All measured
services stopped with SCM exit code zero. The pilot-only STOP/FINISH reset
error is preserved as a calibration finding; none of the three measured runs
had an output or shutdown error.

Owned raw JSON logs are archived losslessly under `target/performance/archive`.
Every archive has gzip integrity, compressed SHA-256, decompressed SHA-256, and
host-copy verification before its generated Ubuntu original is removed.
Compression and JSON scans occurred outside measurement. Counter/JSON
reconciliation and final restoration receipts are retained under
`target/performance/ubuntu` and `target/performance/windows`.

The tested metrics-capable binary and reusable authoritative zone were
retained. Original SCM arguments were restored without periodic metrics, and
only the temporary `rec.stress.test` forwarder was removed. Ubuntu console
output and original receive-buffer sysctls were restored, the owned
upstream/monitor were stopped, and the owned test firewall fixture was removed.
At 05:32:28 UTC the Windows service was Running with LocalSystem, its original
arguments, exit code zero, 5,000 authoritative A records, and no recursive
forwarder. `www.auth.stress.test` returned `192.0.2.53` with TTL 300, followed by
visible CQ/CR output in the restored receiver console/journal. Original
recursion settings, service SID type, and privilege allowlist were verified;
the receiver's original TCP 6000 filter remained in place. Both VMs remain
running. Receipts are `target/performance/windows/final-query-proof.json` and
`target/performance/ubuntu/final-console-proof.log`.

Validation completed: 31 service unit tests and eight client tests passed;
formatting and strict Clippy passed; release builds used the existing Windows
hardening profile without changing compiler/linker security settings or the
abort-on-panic strategy. A metrics write-error/JSON-escaping test and live SCM
smoke run verified failure propagation and durable final ledger behavior.

## Follow-up measurements proposed at this cutoff

The original review proposed these investigations. The
[1,000 QPS repeat](performance-1000qps-review.md),
[capacity experiment](ring-buffer-fix-validation.md), and
[pooled campaign](../performance/pooled-vm-performance-review.md) record subsequent work;
their results do not change this report's measurements or establish a causal
explanation for the original loss:

1. Find the zero-capture-loss offered-rate ceiling with a paced load ramp and
   the same stage ledger. These overload results do not establish that ceiling.
2. Compare producer wakeups/idle park policy and bounded queue sizing under
   measured ETW bursts. Larger memory alone is not established as a cure.
3. Profile native property extraction and allocation costs while preserving
   the existing schema cache, validation, ownership, and shutdown invariants.
4. Separate scheduler/DPC pressure from service CPU with a controlled capacity
   comparison and retained Ready/Wait traces; do not infer lock contention from
   waiting threads alone.
5. For a pure cache-miss recursion experiment, define and verify the actual
   upstream lookup ratio. This repeated-name workload's repaired upstream
   lookup fraction is approximately 2.91%, not 100%.

These are follow-up experiments, not measured fixes. The complete plan,
independent client, telemetry, raw counters, and lossless receiver evidence
make those comparisons repeatable.
