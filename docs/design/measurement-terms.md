# Measurement terms and result gates

> **Status:** Current · **Describes:** the live counter names in `src/pipeline/supervisor.rs` on branch `spool-encryption` as of 2026-11-05. · **Evidence dated:** not applicable (reference) · **Reviewed:** 2026-11-05

This glossary defines the units, the counters, and the gates that the performance reports
share, such as the [pooled VM review](../performance/pooled-vm-performance-review.md).
Each report keeps its own binary, workload, and evidence cutoff. Before you use a campaign harness, read the
[instrumentation compatibility note](../README.md#instrumentation-compatibility-note).

## DNS work and capture records

| Term | Meaning |
| --- | --- |
| Requested QPS | Global ceiling on client query sends per second, shared across workers. |
| Achieved QPS | Client `sent` divided by the configured sending duration. Draining happens after that window. |
| Completed query | A valid matching response received before its deadline, before the answer and flag checks. |
| Successful query | A completed response that passes the client's RCODE, answer-presence, and flag checks. The client does not check each A address or TTL. |
| ETW record | A supported provider event counted in `received`, including events later rejected or dropped. Not a DNS query count. |
| Encoded record | A dnstap frame that the writer accepted. Not a receiver acknowledgment or proof of durable storage. |
| Exact receiver delivery | Encoded records match the archived JSON record count and the receiver's worker ingress and egress counters. |

For the controlled A-record workloads, captured operations reconcile with these checked
domains:

```text
authoritative expected records = 2 * client_sent
recursive expected records = 2 * client_sent + upstream_queries + upstream_replies
```

`CLIENT_QUERY` (CQ) and `CLIENT_RESPONSE` (CR) are the client events. `RESOLVER_QUERY` (RQ)
and `RESOLVER_RESPONSE` (RR) are the upstream operations. Use measured upstream counts,
because repeated names and time to live (TTL) zero do not give one upstream lookup per
client request. The current service writes only CQ and CR records, so the recursive
equality applies only to a build that captured RQ and RR.

A response with the Recursion Available (RA) flag cleared fails DNS validation when the
recursive client requires RA=true, even though it arrived. A client timeout does not show
where a packet was lost, and a dropped capture record does not prove that the DNS exchange
failed.

## Loss counters (live service)

The service writes these counters to the Application event log. The
[monitoring guide](../guide/monitoring.md) lists every field, and its
[counter relationships](../guide/monitoring.md#counter-relationships) give the identities.
[Where events are lost](data-path.md#where-events-are-lost) maps each counter to a stage.
Read the unit before you add two counters.

| Counter | Unit and meaning |
| --- | --- |
| `ring_dropped` | Events rejected because the bounded event channel is full. |
| `pool_exhausted_regular`, `pool_exhausted_jumbo` | Events rejected because their storage class has no free slot. |
| `receiver_disconnected` | Events rejected because the event channel has no receiver (the parser end, not the TCP collector). |
| `intake_dropped` | Sum of the three admission categories above. |
| `callback_errors` | Supported records rejected for malformed metadata, or because callback thread-local storage was unavailable, wrong, or re-entered. |
| `etw_lost` | Events lost by the enabled ETW session. Its event-ID domain can be larger than the supported IDs. |
| `etw_buffers_lost` | Lost ETW buffers, not events. Never add this count to event counts. |
| `received` | Supported provider records that enter the callback, including records later rejected. |
| `dequeued` | Records the parser removes from the channel, before parsing or output. |
| `encoded` | Records whose dnstap frame the parser handed to the sink. |
| `ignored` | Dequeued records the parser ignored without an error. |
| `rejected` | Records that failed to decode or encode. `schema_rejected` counts the subset whose schema failed. |
| `output_failed_events` | Dequeued events whose frame write failed. |
| `output_errors` | Failed output operations in the parser. |
| `queue_depth_max` | Largest sampled depth of the event channel. |
| `etw_buffers_delivered` | ETW buffers whose delivery finished (buffer callback calls). |
| `parser_wake_requests` | Delivered buffers that enqueued events and so woke the parser. |
| `etw_stats_errors` | Failed ETW statistics queries, not loss. |
| `queue_dropped` | Frames in blocks that the full send queue refused. |
| `disconnected_dropped` | Frames lost in the sink with no spool: frames in flight when a connection failed, and frames received while the receiver was down. Not part of `intake_dropped`. |
| `spool_dropped`, `spool_dropped_bytes` | Frames, and their file bytes, that the spool refused, evicted, or could not read or authenticate. `spool_dropped_bytes` also counts leftover segments that the spool deletes at startup. |
| `spool_errors` | Spool I/O errors. |
| `replayed`, `spooled` | Frames read from the spool and accepted by the socket, and frames appended to the spool. |
| `bytes_sent` | Total bytes the socket accepted. |

`spool_bytes` and `spool_segments` are gauges of the current spool size. `spool_available`
and `link_saturated` are flags.

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

The gates are independent. A zero-loss WPR trace can describe a stage that drops capture
records, and exact receiver delivery can describe a lossy capture stage. An incomplete
STOP/FINISH exchange does not fail a requested stop. Record each outcome; a continuation
exception does not turn a strict failure into a pass.

## CPU, memory, and time units

Report units and time windows with every comparison:

| Measurement | Interpretation |
| --- | --- |
| One-core CPU percent | 100% is one logical CPU. A process can exceed 100% on a VM with several vCPUs. |
| VM-capacity CPU percent | One-core process percent divided by the VM's vCPU count. Divide by two for this campaign. |
| PDH CPU | Performance Data Helper counter rates over PID-checked sampling intervals. Coverage can be shorter than the sending duration. |
| Native WPR CPU | Scheduled CPU from context switches. Primary graphs and broad tables exclude observed deferred procedure calls and interrupt service routines (DPC/ISR). |
| Sampled CPU weight | Statistical profile weight. It need not equal PDH or context-switch CPU. |
| Private bytes / working set | Process memory, distinct from counted pool storage. Neither sets a total capture-memory bound. |
| MiB / GiB | Binary units: 1,048,576 and 1,073,741,824 bytes. MB and GB are decimal. |
| DNS latency | Client send-to-matching-response time, including client scheduling. Excludes timeouts and capture or receiver delay. |
| Ready / Wait | Scheduler observations for switch-ins in a stated trace view. Not DNS latency or exact occupancy unless computed that way. |

Keep the sending interval, client drain, service session, PDH-valid intervals, and trace
views separate. A partial-window CPU integral divided by whole-run records is not an exact
CPU cost. Nominal UTC alignment cannot establish subsecond causality. The [graph methods](../performance/pooled-vm-performance-graphs.md#read-the-graphs) and the
[trace interpretation](../performance/pooled-vm-trace-analysis.md#interpret-scheduling-and-symbols)
give their limits.
