# Measurement terms and result gates

> **Status:** Current · **Describes:** `main` at commit `dde7188` (live counter names. The JSONL schemas describe historical binaries.) · **Evidence dated:** not applicable (reference) · **Reviewed:** 2026-11-01

This reference helps maintainers and administrators read the service's
performance reports. It keeps four things separate: DNS responses, captured
records, trace fidelity, and shutdown status. It defines the units and result
gates that the
[completed pooled VM review](../performance/pooled-vm-performance-review.md)
and earlier reports share. Each report keeps its own binary, workload, and
evidence cutoff.

The JSONL schemas and final samples in this reference describe historical
binaries. The TCP-only service retains diagnostic counters but has no durable
metrics export. See the [instrumentation compatibility note](../README.md#instrumentation-compatibility-note) before
using a campaign harness. Stderr diagnostics cannot replace a historical final
JSONL ledger.

## DNS work and capture records

The workload and capture counters measure different things:

| Term | Meaning |
| --- | --- |
| Requested QPS | Global ceiling on client query sends per second, shared across workers. It is not achieved throughput. |
| Achieved QPS | Client `sent` divided by the configured sending duration. Drain occurs after that window. |
| Completed query | A valid matching response received before its deadline. Completion alone does not satisfy every answer and flag check. |
| Successful query | A completed response passing the client's RCODE, answer-presence, and flag checks. The client does not validate each A address or TTL. |
| ETW record | A supported provider event admitted to the callback's `received` count, including events later rejected or dropped. It is not a count of DNS queries. |
| Encoded record | A dnstap frame accepted by the writer. It is not a per-record receiver acknowledgment or proof of durable storage. |
| Exact receiver delivery | Encoded records reconcile with the archived JSON record count and the receiver's worker ingress/egress counters. |

For the controlled A-record workloads, expected captured operations reconcile
with these checked domains:

```text
authoritative expected records = 2 * client_sent
recursive expected records = 2 * client_sent + upstream_queries + upstream_replies
```

The two client events are `CLIENT_QUERY` (CQ) and `CLIENT_RESPONSE` (CR).
Actual upstream operations are `RESOLVER_QUERY` (RQ) and `RESOLVER_RESPONSE`
(RR). Repeated names and time to live (TTL) zero do not establish one upstream
lookup per client request. Use measured upstream counts rather than assuming
four records per recursive query. These equalities describe the checked lab
domains, not arbitrary traffic or every timeout.

An RA=false response has its Recursion Available (RA) flag cleared. If the recursive
client requires RA=true, that response fails DNS validation even though it
arrived. A client timeout does not identify where a packet was lost, and a
dropped capture record does not prove that the DNS exchange failed.

## Historical service loss and final ledgers

The current service writes live counters to the Application event log under the
names listed in [Log events and counters](../guide/configuration.md#log-events-and-counters).
Several names match this section. The live fields are not the schema 2 JSONL
ledger, however, and some schema 2 fields do not exist in the live log. The identities below
apply to the historical JSONL binaries. The live identities are in
[Counter relationships](../guide/configuration.md#counter-relationships).

The live log differs from schema 2 in these names. It reports ETW loss as
`etw_lost` and `etw_buffers_lost`, not `etw_events_lost` and
`etw_realtime_buffers_lost`. It splits pool exhaustion into
`pool_exhausted_regular` and `pool_exhausted_jumbo`, with no `pool_exhausted`
field. It has no `queue_backlog` or `output_limit_dropped` field. It adds
`foreign_pid_rejected`, `schema_rejected`, `disconnected_dropped`, and the ETW
flush and buffer-delivery counters.

The service distinguishes these loss units:

| Counter | Unit and interpretation |
| --- | --- |
| `ring_dropped` | Records rejected because the bounded event channel is full. The legacy name remains in schema 2. |
| `pool_exhausted` | Records rejected because their regular or jumbo class has no free slot. |
| `receiver_disconnected` | Records rejected because the event channel has no receiver. This counter does not describe the remote TCP collector. |
| `intake_dropped` | Sum of the three admission categories in schema 2. |
| `callback_errors` | Supported records rejected for malformed metadata or unavailable, wrong, or reentrant callback thread-local storage. |
| `etw_events_lost` | Events lost by the enabled ETW session, whose event-ID domain can exceed the service's supported IDs. |
| `etw_realtime_buffers_lost` | Lost ETW buffers, not lost events. Never add this count to event counts. |
| `output_failed_events` | Dequeued events whose frame-output operation failed. |
| `output_errors` | Failed output operations, including a flush or final STOP/FINISH failure after all encoded records were delivered. |
| `disconnected_dropped` | Live service only (no schema 2 equivalent). Frames discarded because the TCP receiver was down, or frames written since the last successful flush when a session failed. Those may have reached the receiver. Never resent. Not part of `intake_dropped`. Frames it counts were already encoded. |

After native capture stops and the parser joins, schema 2 requires these exact
identities:

```text
intake_dropped = ring_dropped + pool_exhausted + receiver_disconnected
received = intake_dropped + callback_errors + dequeued + queue_backlog
dequeued = encoded + ignored + rejected + output_limit_dropped + output_failed_events
```

Schema 1 predates the pools and has ring-only intake accounting. Live samples
read separate relaxed atomics and can briefly fail to reconcile. A final sample
must close. Final ETW statistics must be valid. Transient statistics-query errors
remain separate from cumulative event or buffer loss. See
[Pooled ETW intake](pooled-intake.md#metrics-schema-2) for ownership and analysis
details.

## Independent result gates

The pooled campaign records these outcomes separately:

| Gate | What a pass establishes |
| --- | --- |
| DNS validity | Every sent query completes successfully, with no client errors and all answer and flag checks satisfied. Independent fixture probes verify address and TTL. |
| Achieved rate | The client reaches at least 98% of the requested ceiling in this campaign. |
| Capture fidelity | All intake, callback, parser, output-event, ETW, and final-backlog checks are clean, with valid final statistics and exact ledgers. |
| Receiver delivery | Encoded counts match exact archived records and all four worker ingress/egress counters without discards or malformed records. |
| Lifecycle | The service finishes output and stops through the Service Control Manager (SCM) without an operation or exit error. |
| Native trace fidelity | The Windows Performance Recorder (WPR) trace has no reported event/buffer loss and retains continuous coverage of the sending interval. |
| Strict stage | All required DNS, rate, capture, receiver, lifecycle, orchestration, and native trace gates pass. |

A zero-loss WPR trace can describe a stage that drops application capture
records. Exact receiver delivery can describe a correctly reconciled lossy
capture stage. A final-only STOP/FINISH reset can fail lifecycle even when every
encoded record reached the receiver. Preserve each outcome. A continuation
exception does not convert a strict failure into a pass.

## CPU, memory, and time units

Report units and time windows with every comparison:

| Measurement | Interpretation |
| --- | --- |
| One-core CPU percent | 100% is one logical CPU. A process can exceed 100% on a VM with multiple vCPUs. |
| VM-capacity CPU percent | One-core process percent divided by the VM's vCPU count. Divide by two for this campaign. |
| PDH CPU | Performance Data Helper counter rates over PID-validated sampling intervals. Coverage can be shorter than the sending duration. |
| Native WPR CPU | Scheduled CPU derived from context switches. The primary graphs and broad tables exclude observed deferred procedure calls and interrupt service routines (DPC/ISR). |
| Sampled CPU weight | Statistical profile weight with different attribution. It need not equal PDH or context-switch CPU. |
| Private bytes / working set | Process memory measures distinct from counted pool storage. Neither establishes a total capture-memory bound. |
| MiB / GiB | Binary units: 1,048,576 and 1,073,741,824 bytes. MB and GB are decimal when explicitly used. |
| DNS latency | Client application send-to-matching-response time, including client scheduling. It excludes timeouts and capture/receiver delay. |
| Ready / Wait | Scheduler observations associated with switch-ins in a stated trace view. They are not DNS latency or exact clipped occupancy unless explicitly computed that way. |

Keep the configured sending interval, later client drain, enclosing service
session, PDH-valid intervals, and selected trace views separate. Do not divide a
partial-window CPU integral by whole-run records and call it an exact CPU cost.
Nominal UTC alignment inherits clock uncertainty. It cannot establish subsecond
causality. See the [graph methods](../performance/pooled-vm-performance-graphs.md#read-the-graphs)
and [trace interpretation](../performance/pooled-vm-trace-analysis.md#interpret-scheduling-and-symbols)
for their specific limits.
