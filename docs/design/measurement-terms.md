# Measurement terms and result gates

> **Status:** Current · **Describes:** `tcp-sink-spool` at commit `0569136` (live counter names. The JSONL schemas describe historical binaries.) · **Evidence dated:** not applicable (reference) · **Reviewed:** 2026-11-03

This glossary helps maintainers and administrators read performance reports. It
defines the units and gates that the
[completed pooled VM review](../performance/pooled-vm-performance-review.md) and
earlier reports share. It keeps DNS responses, captured records, trace fidelity, and
shutdown status apart. Each report keeps its own binary, workload, and evidence cutoff.

The JSONL schemas and final samples describe historical binaries. The TCP-only service
has no durable metrics export. See the
[instrumentation compatibility note](../README.md#instrumentation-compatibility-note)
before you use a campaign harness. Live log lines cannot replace a historical final
JSONL ledger.

## DNS work and capture records

| Term | Meaning |
| --- | --- |
| Requested QPS | Global ceiling on client query sends per second, shared across workers. It is not achieved throughput. |
| Achieved QPS | Client `sent` divided by the configured sending duration. Drain occurs after that window. |
| Completed query | A valid matching response received before its deadline. Completion alone does not pass every answer and flag check. |
| Successful query | A completed response that passes the client's RCODE, answer-presence, and flag checks. The client does not check each A address or TTL. |
| ETW record | A supported provider event counted in `received`, including events later rejected or dropped. It is not a DNS query count. |
| Encoded record | A dnstap frame that the writer accepted. It is not a per-record receiver acknowledgment or proof of durable storage. |
| Exact receiver delivery | Encoded records reconcile with the archived JSON record count and the receiver's worker ingress and egress counters. |

For the controlled A-record workloads, captured operations reconcile with these
checked domains:

```text
authoritative expected records = 2 * client_sent
recursive expected records = 2 * client_sent + upstream_queries + upstream_replies
```

`CLIENT_QUERY` (CQ) and `CLIENT_RESPONSE` (CR) are the client events.
`RESOLVER_QUERY` (RQ) and `RESOLVER_RESPONSE` (RR) are the upstream operations.
Repeated names and time to live (TTL) zero do not establish one upstream lookup per
client request, so use measured upstream counts. The equalities cover the checked lab
domains only. The current service captures only events 256, 257, and 258. It writes CQ
and CR records and no RQ or RR records, so the recursive equality applies only to a
build that captured them.

An RA=false response has its Recursion Available (RA) flag cleared. If the recursive
client requires RA=true, that response fails DNS validation even though it arrived. A
client timeout does not show where a packet was lost. A dropped capture record does
not prove that the DNS exchange failed.

## Historical service loss and final ledgers

The current service writes live counters to the Application event log, under the
names in [Log events and counters](../guide/monitoring.md). The identities below
apply to the historical JSONL binaries. The live identities are in
[Counter relationships](../guide/monitoring.md#counter-relationships).

The live log differs from schema 2. It reports `etw_lost` and `etw_buffers_lost`
instead of the schema 2 ETW loss names. It splits pool exhaustion into regular and
jumbo counters, and it has no `queue_backlog` or `output_limit_dropped` field. It adds
the send-queue and spool counters and `schema_rejected`.

The service distinguishes these loss units and the related counters:

| Counter | Unit and interpretation |
| --- | --- |
| `ring_dropped` | Records rejected because the bounded event channel is full. The legacy name remains in schema 2. |
| `pool_exhausted` | Records rejected because their regular or jumbo class has no free slot. |
| `receiver_disconnected` | Records rejected because the event channel has no receiver. It does not describe the remote TCP collector. |
| `intake_dropped` | Sum of the three admission categories in schema 2. |
| `callback_errors` | Supported records rejected for malformed metadata or for unavailable, wrong, or reentrant callback thread-local storage. |
| `etw_events_lost` | Events lost by the enabled ETW session. Its event-ID domain can exceed the supported IDs. |
| `etw_realtime_buffers_lost` | Lost ETW buffers, not lost events. Never add this count to event counts. |
| `output_failed_events` | Dequeued events whose frame-output operation failed. |
| `output_errors` | Failed output operations, including a final STOP/FINISH failure after all encoded records were delivered. |
| `received` | Supported provider records that enter the callback, including rejected records. |
| `dequeued` | Records the parser removes before parsing or output. |
| `queue_backlog` | Records that remain in the channel. The live log has no such field. It reports `queue_depth_max` instead. |
| `foreign_pid_rejected` | Live service only. Records from a process other than the DNS Server. Not counted in `received`. |
| `etw_buffers_delivered` | Live service only. ETW buffers whose delivery finished (buffer callback calls). |
| `parser_wake_requests` | Live service only. Delivered buffers that enqueued events and so sent the parser a wake request. |
| `disconnected_dropped` | Live service only. Encoded frames lost in the sink, in flight when a connection failed or received while the receiver was down with no spool in use. A failed replay batch also counts in `replayed`. These frames are never resent and are not part of `intake_dropped`. |

After native capture stops and the parser joins, schema 2 requires these exact
identities:

```text
intake_dropped = ring_dropped + pool_exhausted + receiver_disconnected
received = intake_dropped + callback_errors + dequeued + queue_backlog
dequeued = encoded + ignored + rejected + output_limit_dropped + output_failed_events
```

Schema 1 predates the pools and has ring-only intake accounting. Live samples read
separate relaxed atomics and can briefly fail to reconcile. A final sample must close
with valid final ETW statistics. Transient statistics-query errors stay separate from
cumulative loss. See [Pooled ETW intake](pooled-intake.md#metrics-schema-2).

## Independent result gates

The pooled campaign records these outcomes separately:

| Gate | What a pass establishes |
| --- | --- |
| DNS validity | Every sent query completes successfully, with no client errors, and every answer and flag check passes. Independent fixture probes check address and TTL. |
| Achieved rate | The client reaches at least 98% of the requested ceiling in this campaign. |
| Capture fidelity | All intake, callback, parser, output-event, ETW, and final-backlog checks are clean, with valid final statistics and exact ledgers. |
| Receiver delivery | Encoded counts match exact archived records. All four worker ingress and egress counters show no discards or malformed records. |
| Lifecycle | The service finishes output and stops through the Service Control Manager (SCM) with no operation or exit error. |
| Native trace fidelity | The Windows Performance Recorder (WPR) trace reports no event or buffer loss and covers the sending interval continuously. |
| Strict stage | Every required DNS, rate, capture, receiver, lifecycle, orchestration, and native trace gate passes. |

A zero-loss WPR trace can describe a stage that drops application capture records.
Exact receiver delivery can describe a correctly reconciled, lossy capture stage. A
final-only STOP/FINISH reset can fail lifecycle even when every encoded record
reached the receiver. Preserve each outcome. A continuation exception does not turn a
strict failure into a pass.

## CPU, memory, and time units

Report units and time windows with every comparison:

| Measurement | Interpretation |
| --- | --- |
| One-core CPU percent | 100% is one logical CPU. A process can exceed 100% on a VM with several vCPUs. |
| VM-capacity CPU percent | One-core process percent divided by the VM's vCPU count. Divide by two for this campaign. |
| PDH CPU | Performance Data Helper counter rates over PID-checked sampling intervals. Coverage can be shorter than the sending duration. |
| Native WPR CPU | Scheduled CPU from context switches. Primary graphs and broad tables exclude observed deferred procedure calls and interrupt service routines (DPC/ISR). |
| Sampled CPU weight | Statistical profile weight with different attribution. It need not equal PDH or context-switch CPU. |
| Private bytes / working set | Process memory, distinct from counted pool storage. Neither establishes a total capture-memory bound. |
| MiB / GiB | Binary units: 1,048,576 and 1,073,741,824 bytes. MB and GB are decimal when explicitly used. |
| DNS latency | Client application send-to-matching-response time, including client scheduling. Excludes timeouts and capture or receiver delay. |
| Ready / Wait | Scheduler observations for switch-ins in a stated trace view. Not DNS latency or exact clipped occupancy unless computed that way. |

Keep the sending interval, client drain, service session, PDH-valid intervals, and
trace views separate. Do not divide a partial-window CPU integral by whole-run records
and call the result an exact CPU cost. Nominal UTC alignment cannot establish
subsecond causality. See the
[graph methods](../performance/pooled-vm-performance-graphs.md#read-the-graphs) and
[trace interpretation](../performance/pooled-vm-trace-analysis.md#interpret-scheduling-and-symbols)
for their limits.
