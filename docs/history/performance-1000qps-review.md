# Performance repeat at global 1,000 QPS, review

> **Status:** Historical · **Evidence dated:** October 29, 2026 UTC · **Commit:** not recorded · **Reviewed:** 2026-11-01

This historical review helps maintainers compare the original paced workloads
on the unchanged 1,024-entry ring. The runs and restoration occurred on
October 29, 2026 UTC. Binary, test, and installed-state descriptions apply at
that cutoff. Later results are in the [capacity validation](ring-buffer-fix-validation.md)
and [pooled VM review](../performance/pooled-vm-performance-review.md).

This record describes historical binaries with service JSONL metrics
instrumentation. The TCP-only service has no durable metrics export; its
instrumentation and performance have not been measured by this campaign.
See the [instrumentation compatibility note](../README.md#instrumentation-compatibility-note). Preserved counters,
tables, hashes, and collector artifacts retain their historical meaning.

Both 300-second runs achieved **999.69 QPS** with zero client errors. The
unchanged service nevertheless lost **24.78% of authoritative capture records**
and **37.27% of recursive-workload capture records**, entirely at its bounded
ring. ETW EventsLost was zero, service/guest CPU was low, and every record
accepted by the writer reached durable receiver output. This paced result
does not establish a zero-loss operating point.

## Configuration and method

The [run plan](performance-1000qps-plan.md) records ownership, models/efforts,
dependencies, exact commands, acceptance gates and restoration. Coding and
orchestration used GPT-6.1-Sol/high workers; coordination and independent
read-only review used GPT-6-Astra/high. Trace support performs heavy exports
only after both official sending windows finish.

- Same Windows VM/GUID/address: `DNSTap-WS2025`,
  `24bafedb-0998-4430-a0c6-7e90350cd9ac`, `172.27.215.163`, 2 vCPUs, 4 GiB RAM.
- Same service binary SHA-256:
  `0FCDD17E08A3DDD42F4F132F7F03F80A5513FEBC66E54F90A5EBBC8135BE2C1D`.
- Same client binary SHA-256:
  `7A5800CFC4252196E4C6162F7D2CCF81217FE95A339D7CFBB9CD84063F4B6960`.
- Existing 5,000 common names and authoritative A records, `192.0.2.53`, TTL
  300; controlled recursive upstream answers TTL 0. Twelve pre-run fixture
  probes passed. Source/license/hash provenance is in
  [`workload-provenance.json`](../../datasets/performance/workload-provenance.json).
- Host command uses `--threads 2 --duration 300 --inflight 256
  --timeout-ms 2000 --qps 1000` and explicit `--server 172.27.215.163:53`.
  The cap is **global**, not per worker: each worker uses a 2 ms interval and
  resets its next send from the actual send time. Paired microbursts are
  possible; the cap is not a single global 1 ms inter-packet scheduler.
- Same ring capacity 1,024, strict SCM-only identity/privileges, Windows cache
  policy, optional per-second metrics and timing instrumentation. No client or
  service rebuild/optimization was performed.
- New protected Windows campaign backups and unique run IDs preserve every
  earlier artifact. Ubuntu uses a new campaign/log path and the verified 8 MiB
  fixture receive buffer. The receiver JSON file sink and Prometheus/resource
  sampler are instrumented as in the prior experiment; console/journal output
  is restored afterward.

One-second BLG/CSV counters validate service/DNS PIDs and consecutive thread
PID/TID identity for rates. CPU means/p95 are duration-weighted interval
averages. WPR recording/rundown and metrics overhead are included. Resource
windows follow client sending timestamps; final ledgers and byte-offset
receiver markers also include the subsequent ETW/parser drain. Monotonic
client duration is authoritative: recorded wall ends were 2 ms and 5 ms earlier
than their computed nominal ends, whereas monotonic elapsed-with-drain was
300.0004281 and 300.0004343 seconds. The nominal UTC windows in
[Client and capture results](#client-and-capture-results) are useful
for counter alignment; these millisecond differences do not imply a negative
drain or a shortened sending interval.

## Client and capture results

Times are UTC on **2026-10-29**, corresponding to the evening of October 28
PDT. No optional sanity workload was run: independent pacing review and the
fixture probes passed before the two official measurements.

| Run | Nominal sending window UTC | Sent / matched / successful | Achieved QPS | Timeouts / other client errors |
| --- | --- | ---: | ---: | ---: |
| `auth-1000qps-1` | 06:39:49.469–06:44:49.469 | 299,907 / 299,907 / 299,907 | 999.69 | 0 / 0 |
| `rec-1000qps-1` | 06:47:39.300–06:52:39.300 | 299,907 / 299,907 / 299,907 | 999.69 | 0 / 0 |

| Run | p50 | p95 | p99 | Maximum matched latency |
| --- | ---: | ---: | ---: | ---: |
| `auth-1000qps-1` | 0.135167 ms | 0.217087 ms | 0.303103 ms | 2.4896 ms |
| `rec-1000qps-1` | 0.344063 ms | 0.573439 ms | 0.851967 ms | 2.7118 ms |

Quantiles are fixed-histogram bin upper bounds. They exclude timeouts and have no
coordinated-omission correction; they measure host application send-to-receive
latency, not dnstap delivery delay.

| Run | Supported records received | Ring drops | Encoded | ETW EventsLost | Real-time buffers lost |
| --- | ---: | ---: | ---: | ---: | ---: |
| `auth-1000qps-1` | 599,814 | 148,638 | 451,176 | 0 | 0 |
| `rec-1000qps-1` | 916,490 | 341,569 | 574,921 | 0 | 0 |

The controlled-workload final ledgers satisfy:

```text
2 × 299,907 client queries = 599,814 supported records received
599,814 = 148,638 ring drops + 451,176 dequeued/encoded records

2 × 299,907 client queries + 158,338 upstream queries + 158,338 upstream replies
        = 916,490 supported records received
916,490 = 341,569 ring drops + 574,921 dequeued/encoded records
```

Final backlog, callback-copy errors, parser rejections/ignored records,
output-limit drops, failed-frame events and output-operation errors were zero.
Final ETW statistics were valid with zero query failures in both runs. Observed
queue high water reached 1,024 in both. This is capture-record loss, not DNS
packet loss: every client request completed successfully.

| Run | Encoded | Exact durable JSON | Receiver ingress / egress | Worker discards |
| --- | ---: | ---: | ---: | ---: |
| `auth-1000qps-1` | 451,176 | 451,176 | 451,176 / 451,176 | 0 |
| `rec-1000qps-1` | 574,921 | 574,921 | 574,921 / 574,921 | 0 |

Both files contained zero malformed JSON/DNS records, and their global record
counts equaled the workload-zone counts with zero background records. The
authoritative upstream had no queries/replies. The recursive run recorded
158,338 upstream queries and exactly as many replies, a **52.7957% count ratio**
to client queries, with zero upstream UDP errors. This is not a pure cache-miss
workload or proof of a per-request cache/coalescing mechanism. Its denominator
uses actual wire counts, not an assumed four records per query. All retained
positive client/resolver responses had TTL 0. The client reported zero RA-check
failures, and all 186,660 retained client responses had RA set. The 100,880
authoritative upstream resolver replies had RA clear, as expected; that is not
a client-response flag error. The receiver's Boolean fields were present.

## Resource use

Process CPU in this section is percent of **one logical CPU**; divide by two for this
guest's capacity. Maxima/p95 describe interval-average samples, not
instantaneous peaks. MiB means 1,048,576 bytes.

| Run | Service CPU mean / p95 / max | DNS CPU mean | Guest CPU mean | Valid samples / CPU-rate interval coverage |
| --- | ---: | ---: | ---: | ---: |
| `auth-1000qps-1` | 0.431 / 1.580 / 4.750% | 3.193% | 4.440% | 300 / 299.614 s |
| `rec-1000qps-1` | 0.588 / 1.584 / 3.136% | 5.958% | 5.892% | 300 / 299.969 s |

| Run | Service private MiB start → end / peak | Working-set peak MiB | Threads | Handles |
| --- | ---: | ---: | ---: | ---: |
| `auth-1000qps-1` | 2.215 → 2.406 / 2.992 | 8.461 | 6–9 | 97–102 |
| `rec-1000qps-1` | 2.457 → 2.324 / 3.102 | 8.727 | 6–9 | 97–102 |

| Run | Guest ready-queue mean | Disk write latency mean | Disk queue mean | Service I/O read / write / other bytes/s |
| --- | ---: | ---: | ---: | ---: |
| `auth-1000qps-1` | 0.0206 | 1.782 ms | 0.0100 | 0 / 495.4 / 0 |
| `rec-1000qps-1` | 0.0166 | 1.761 ms | 0.0399 | 0 / 495.9 / 0 |

Queue and disk values are duration weighted; disk latency means are averages
of interval means rather than I/O-count-weighted latency. Process I/O values
are explicitly sample means and do not represent all Winsock traffic. Service
memory showed no sustained growth in these short windows; that is not proof
of absence of a leak. DNS private memory changed by −0.813 MiB and +9.246 MiB,
which is separate from the capture service's memory.

Host client CPU was approximately 198.9% and 198.7% of one CPU, or 8.29% and
8.28% of the 24-logical-CPU host. Busy polling/yielding keeps the
two workers occupied even when paced, so that occupancy alone is not evidence
that the client could not deliver 1,000 QPS.

All process PID samples were valid. Maximum sample intervals were 1.174 and
1.019 seconds. Consecutive identity checks excluded 34 and 35 service thread
rate values after slot changes; current identity gauges remain separately
valid. Full working/private-working/virtual memory, faults, network, DNS,
thread-state and I/O-operation series remain in the BLG/CSV and analysis files.

## Trace overhead and interpretation

Authoritative ring loss already appeared before WPR. A descriptive comparison
of fully enclosed one-second callback-counter intervals found:

| Phase | Counter interval seconds | Received delta | Ring-drop delta | Ring drops / received |
| --- | ---: | ---: | ---: | ---: |
| Before WPR | 121.670 | 243,416 | 54,193 | 22.26% |
| WPR recording and rundown | 45.305 | 91,372 | 30,568 | 33.45% |
| After WPR | 129.673 | 259,718 | 63,877 | 24.59% |

The same descriptive recursive comparison gave 42.50% before WPR (128.989 s),
37.13% during recording/rundown (47.290 s), and 32.39% afterward (120.805 s).
Its two WPR-boundary intervals total 2.025 s. The inconsistent direction of
these phase differences is another reason not to interpret them as causal
profiler-overhead estimates.

Two WPR-boundary intervals (2.019 s) are separate. Host/guest clock bounds,
one-second sampling and asynchronous counter updates make these phase
boundaries approximate. Deltas can contain queue carry-over across boundaries;
only the drained final ledger is an exact stage equality. This comparison is
descriptive, not a causal estimate of profiler overhead. It shows WPR cannot
be the sole explanation for the loss. The guest's 87.44% maximum CPU sample is
a transient instrumentation interval, not sustained utilization.

The authoritative service accumulated only 1.228 seconds of parse/encode wall
time and 0.0846 seconds of output time during the enclosing capture session.
Parsing timers begin after dequeue and exclude the empty-ring two-millisecond
park. Loss with this low average work/CPU does not support calling the parser
CPU saturated. ETW delivery batching, ring capacity and wake/scheduling policy
are investigation targets; no specific mechanism is claimed proven by source
inspection. Recursive parse/encode wall time was 1.569 seconds, with 0.0939
seconds of output. Zero lost-event counters alone do not prove that a circular
memory trace retained its entire header range; the bounded analysis uses
explicit retained trace views.

After both runs, independent WPA exports selected ETL seconds **5–25** of each
trace, a 20-second view during the corresponding load. Named parser results:

Trace headers span 46.2473 seconds (authoritative) and 46.2070 seconds
(recursive), with zero lost buffers/events. Context-switch coverage is present
through seconds 0–46 in these low-load recordings; the selected interior view
is retained. Its absolute UTC ranges are 06:41:56.4966–06:42:16.4966 and
06:49:54.2324–06:50:14.2324 respectively, avoiding startup and rundown.

| Run / parser TID | Scheduled CPU | Associated Ready total | Associated Wait total | Positive Wait median / p95 |
| --- | ---: | ---: | ---: | ---: |
| Authoritative / 2300 | 113.004 ms | 31.728 ms | 19.8543 s | 15.957 / 16.165 ms |
| Recursive / 3696 | 150.558 ms | 35.128 ms | 19.8143 s | 15.950 / 16.172 ms |

Ready medians were 19.2 and 18.8 microseconds. Same-window context-switch
accounting excluding DPC/ISR measured parser CPU at 109.277 and 144.051 ms.
These are separate CPU accounting definitions, not conflicting percentages.
Ready/Wait durations are associated with switch-ins in the view: a first wait
crosses its beginning and a tail without an in-view switch-in can be absent.
They are not asserted to be exactly clipped occupancy fractions or whole-run
totals.

The observed wait reason was `WrAlertByThreadId`, not evidence identifying a
contended application lock. Low Ready time does not support sustained CPU
scheduling pressure as the dominant limitation in these views. The source
requests a two-millisecond empty-ring park; observed waits near sixteen
milliseconds are consistent with a timeout/wakeup-cadence hypothesis.
ETW flush batching, queue capacity and producer wake policy should be tested
in a separate controlled change. This experiment did not alter timer
resolution, wake policy or capacity, and does not prove a particular fix.
Raw CSV, quantile aggregates, view boundaries and same-window exclusive CPU
logs are under `target/performance/review/1000qps/RUN_ID`.

## Evidence and restoration status

Raw client artifacts are under `target/performance/client/RUN_ID`; Windows
BLG/CSV/ETL, analysis, phases and clock bounds are under
`target/performance/windows/RUN_ID`. The new campaign baseline is
`target/performance/windows/campaign-1000qps-20261028`.
Receiver archives are under `target/performance/archive`:

| Archive | Compressed bytes | Original raw bytes |
| --- | ---: | ---: |
| `auth-1000qps-1-performance.jsonl.gz` | 13,282,310 | 437,740,879 |
| `rec-1000qps-1-performance.jsonl.gz` | 20,077,613 | 563,352,718 |

Compressed/decompressed SHA-256, gzip integrity and host-copy checks passed
before removing only each owned Ubuntu original. No heavy archive scans or
ETL exports ran during official sending windows.

Both measured services stopped cleanly with SCM exit code zero. The old
baseline connection, before measurements, reported a STOP/FINISH reset after
the receiver was reconfigured; it is not included in either measured ledger.
At 06:55:34 UTC, this campaign's original SCM arguments without metrics were
restored; the unchanged service was Running as LocalSystem, PID 1284, exit
zero. The authoritative 5,000-record zone and tested binary remain, and only
the owned recursive forwarder was removed. Original recursion settings,
service SID type and required-privilege allowlist were preserved. Ubuntu
console/journal output, original receive-buffer sysctls and baseline TCP filter
were restored; the owned upstream/monitor/firewall fixture are inactive or
removed. At 06:55:39 UTC, an ordinary authoritative query returned
`192.0.2.53`/TTL 300 and produced visible matching CQ/CR console records.

New receipts are under
`target/performance/windows/campaign-1000qps-20261028/{Restore,Proof}.json`
and `target/performance/ubuntu/1000qps`, including
`final-console-auth-proof.log` and `final-console-proof-receipt.json`.
Both VMs remain running; earlier backups and experiment artifacts are intact.

The unchanged binaries inherit the earlier 31-service-test/eight-client-test
validation; this repeat adds reviewed pacing, live fixture/hash/SCM checks and
independent final ledger/receiver verification rather than a rebuild.
