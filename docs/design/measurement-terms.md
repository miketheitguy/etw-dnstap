# Measurement terms and result gates

> **Status:** Current · **Describes:** the live counter names in `src/pipeline/supervisor.rs` on branch `spool-encryption` as of 2026-11-05. · **Evidence dated:** not applicable (reference) · **Reviewed:** 2026-11-05

### Quick Start

```text
authoritative expected records = 2 * client_sent
recursive expected records     = 2 * client_sent + upstream_queries + upstream_replies
```

Check units before adding counters. Reports such as the [pooled VM review](../history/pooled-vm-performance-review.md)
keep their own binary, workload, and evidence cutoff; read the [instrumentation compatibility note](../README.md#instrumentation-compatibility-note)
before using a campaign harness.

### Steps / Configuration

#### DNS work and capture records

| Term | Meaning |
| --- | --- |
| Requested QPS | Global client send ceiling, shared by workers |
| Achieved QPS | Client `sent` / configured sending duration (drain excluded) |
| Completed query | Valid matching response by deadline, before answer checks |
| Successful query | Completed; passes RCODE, answer-presence, flag checks |
| ETW record | Supported event counted in `received` |
| Encoded record | Frame the writer accepted |
| Exact receiver delivery | Encoded = archived JSON count = worker ingress and egress |

The formulas reconcile controlled A-record workloads. Client events: `CLIENT_QUERY` (CQ),
`CLIENT_RESPONSE` (CR). Upstream: `RESOLVER_QUERY` (RQ), `RESOLVER_RESPONSE` (RR).

#### Loss counters (live service)

Logged to the Application event log. Fields: [monitoring guide](../guide/monitoring.md);
identities: [counter relationships](../guide/monitoring.md#counter-relationships); stages:
[Where events are lost](data-path.md#where-events-are-lost).

| Counter | Meaning |
| --- | --- |
| `ring_dropped` | Events rejected: event channel full |
| `pool_exhausted_regular`, `pool_exhausted_jumbo` | Events rejected: storage class has no slot |
| `receiver_disconnected` | Events rejected: no parser end (not the TCP collector) |
| `intake_dropped` | Sum of the three above |
| `callback_errors` | Malformed metadata, or callback TLS unusable or re-entered |
| `etw_lost` | Events the ETW session lost (wider ID domain) |
| `etw_buffers_lost` | Lost ETW buffers, not events |
| `received` | Supported records entering the callback, later rejects included |
| `dequeued` | Records taken from the channel, before parsing |
| `encoded` | Frames handed to the sink |
| `ignored` | Dequeued records ignored without an error |
| `rejected` | Decode or encode failures; `schema_rejected` subset |
| `output_failed_events` | Dequeued events whose frame write failed |
| `output_errors` | Failed parser output operations |
| `queue_depth_max` | Largest sampled event channel depth |
| `etw_buffers_delivered` | Buffer callback calls |
| `parser_wake_requests` | Delivered buffers that enqueued events, waking the parser |
| `etw_stats_errors` | Failed ETW statistics queries (not loss) |
| `queue_dropped` | Frames in blocks the full send queue refused |
| `disconnected_dropped` | No spool: frames in flight at failure, or sent while receiver down |
| `spool_dropped`, `spool_dropped_bytes` | Frames, file bytes refused, evicted, unreadable, unauthenticated |
| `spool_errors` | Spool I/O errors |
| `replayed`, `spooled` | Spool frames the socket took; frames appended |
| `bytes_sent` | Bytes the socket accepted |

Gauges: `spool_bytes`, `spool_segments`. Flags: `spool_available`, `link_saturated`.

#### Independent result gates

The pooled campaign records each separately:

- **DNS validity:** every query succeeds, every answer and flag check passes, no client errors; independent fixture probes check address and TTL.
- **Achieved rate:** at least 98% of the requested ceiling in this campaign.
- **Capture fidelity:** clean intake, callback, parser, output-event, ETW, and final-backlog checks; valid final statistics; exact ledgers.
- **Receiver delivery:** exact archived match; all four worker ingress and egress counters clean of discards and malformed records.
- **Lifecycle:** output finishes; Service Control Manager (SCM) stop with no operation or exit error.
- **Native trace fidelity:** Windows Performance Recorder (WPR) trace with zero event or buffer loss, continuous over the sending interval.
- **Strict stage:** every required DNS, rate, capture, receiver, lifecycle, orchestration, and native trace gate.

#### CPU, memory, and time units

State units and time windows with every comparison.

| Measurement | Meaning |
| --- | --- |
| One-core CPU percent | 100% = one logical CPU; can exceed 100% |
| VM-capacity CPU percent | One-core percent / vCPUs (/2 here) |
| PDH CPU | Performance Data Helper rates, PID-checked intervals |
| Native WPR CPU | Context-switch CPU; primary graphs, broad tables exclude observed deferred procedure calls and interrupt service routines (DPC/ISR) |
| Sampled CPU weight | Profile weight; can differ from PDH, switches |
| Private bytes / working set | Process memory, not pool storage or a capture-memory bound |
| MiB / GiB | 1,048,576 and 1,073,741,824 bytes; MB, GB decimal |
| DNS latency | Client send to response, with client scheduling |
| Ready / Wait | Scheduler switch-in observations in a trace view |

### Common Gotchas

- **Term limits:** a successful query leaves A addresses and TTL unchecked; an ETW record
  (including later rejects and drops) is no query count; an encoded record is no receiver
  ack or proof of durable storage.
- Never add `etw_buffers_lost` to event counts. `disconnected_dropped` sits outside
  `intake_dropped`; `spool_dropped_bytes` includes leftover segments deleted at startup.
- Use measured upstream counts: repeated names and time to live (TTL) zero break one lookup
  per request. The recursive formula needs RQ and RR; the current service writes only CQ and CR.
- With Recursion Available (RA) cleared, a response fails validation when the client requires RA=true.
- A client timeout hides the loss point; a dropped capture record says nothing about the DNS exchange.
- **Gates are independent.** A zero-loss WPR trace can describe a stage that drops capture
  records, and exact delivery a lossy capture stage. A requested stop passes with an
  incomplete STOP/FINISH. A continuation exception keeps a strict failure a failure.
- PDH coverage can be shorter than the sending duration. DNS latency excludes timeouts,
  capture, and receiver delay. Ready and Wait mean DNS latency or exact occupancy only when computed so.
- **Keep windows separate:** sending interval, client drain, service session, PDH-valid
  intervals, trace views. A partial-window CPU integral over whole-run records isn't an
  exact CPU cost; nominal UTC alignment can't establish subsecond causality. See
  [graph methods](../history/pooled-vm-performance-graphs.md#read-the-graphs),
  [trace interpretation](../history/pooled-vm-trace-analysis.md#interpret-scheduling-and-symbols).
