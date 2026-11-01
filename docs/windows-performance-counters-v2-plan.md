# Minimal Windows V2 performance counter plan

> **Status:** Proposed (future implementation tasks; none executed) · **Evidence dated:** not applicable · **Commit:** not applicable · **Reviewed:** 2026-11-01

This proposed design helps maintainers expose essential DNS pipeline progress
and errors through Windows Performance Counters V2, without adding per-event
instrumentation. It replaces the broader counter design with nine
counters. This is documentation only; no counters or provider are implemented.

## Scope and baseline

Reuse the in-memory counters in the TCP-only working tree. The
[`ETW callback`](../src/etw.rs) filters supported DNS events, copies records to
preallocated pools, and sends them through a bounded channel. The single
[`parser loop`](../src/pipeline.rs) performs TDH parsing, protobuf encoding,
and TCP writer calls. The [`network sink`](../src/network.rs) uses buffered
Frame Streams output with deadlines. These are sequential processing stages,
not three independent workers.

The proposed addition only reads existing atomics outside those paths. It adds
no callback/parser increments, loads, timing, branches, locks, allocation,
record fields, or native calls. It does not split existing parse/encode outcomes
or restore periodic ETW queries, queue high-water marks, or per-event timing.
Keep the required TCP receiver and existing service behavior. The service writes
no capture, metrics, or log files; an external PerfMon collector can persist its
own samples under its own identity and storage policy.

Read-only collection still creates cache traffic. The publisher, PERFLIB, and
consumers also consume CPU and memory. This design minimizes additional work;
it cannot promise zero performance impact. A proposed disabled mode skips all
provider startup and publication while preserving existing diagnostics. Select
the exact configuration mechanism during implementation; no flag is introduced
by this plan.

## Nine proposed counters

All nine values use eight-byte unsigned `perf_counter_large_rawcount`, a legal
manifest type documented by the [counter schema](https://learn.microsoft.com/en-us/windows/win32/perfctrs/performance-counters-counter-complex-type).
They are cumulative run totals, including the publisher sequence. IDs and names
are proposed assignments, not shipped identifiers. Freeze their meanings after
release and never reset them when a consumer reads or connects.

The following catalog is the entire proposed counter set:

| ID | Name | Existing source and exact meaning |
| --- | --- | --- |
| 100 | Supported records | `CaptureCounters.received`: records passing the callback's provider and supported-ID filter, before admission decisions. Not client DNS queries. |
| 103 | Channel full drops | `CaptureCounters.dropped`: records rejected by bounded-channel `try_send` with Full. Preserves the existing `ring_dropped` meaning. |
| 108 | Intake drops | `CaptureCounters.intake_dropped`: existing total of channel-full, pool-exhausted, and in-process receiver-disconnected admission drops. Includes ID 103; never add them together as separate losses. |
| 101 | Callback errors | `CaptureCounters.callback_errors`: supported records rejected for malformed copy metadata or missing, wrong, or reentrant callback producer state. Separate from intake drops. |
| 300 | Records dequeued | `ParseCounters.dequeued`: records taken from the bounded channel before parsing. |
| 401 | Frames staged | `ParseCounters.encoded`: successful `NetworkSink::write_frame` returns. The writer can retain bytes in `BufWriter`; this is not collector delivery or a separate protobuf-success count. |
| 330 | Processing rejects | `ParseCounters.rejected`: combined TDH parse and encode rejection total, preserving the existing branch. This proposed ID does not reuse a parse-only counter meaning. |
| 417 | Output operation errors | `ParseCounters.output_errors`: failed data-frame writes, explicit flushes, and final STOP/FINISH operations. Counts operations, not lost records; a finish-only failure can occur after all frames are staged. |
| 5 | Publication sequence | Publisher-owned total, advanced only after all eight source values are copied successfully to native counters. No capture/parser update. |

The source fields and update points already exist in
[`CaptureCounters` and the callback](../src/etw.rs) and
[`ParseCounters` and `parse_loop`](../src/pipeline.rs). The implementation must
reuse those exact atomics and preserve their increment points. Only visibility
or shared read access for the publisher may change; do not create shadow
counters. The publication sequence is the only additional counter state.

Use one multi-instance counter set, proposed name `EtwDnstap Pipeline`, with
instance name `EtwDnstap-PID-RUN_TOKEN`. Generate the bounded per-start token
outside capture; use the PID as numeric instance ID. Active PIDs are unique,
and the token distinguishes restarts and PID reuse. Consumers enumerate and
retain the full instance name instead of adding identity counters. Assign
independent provider and counter-set GUIDs during implementation; never reuse
the DNS ETW provider GUID. Names reveal no receiver, DNS name, address, payload,
session name, or producer identity.

## Collection and interpretation

A single dedicated publisher reads the eight existing atomics with relaxed
loads once every five seconds, then copies values into PERFLIB-owned instance
storage using the documented [eight-byte setter](https://learn.microsoft.com/en-us/windows/win32/api/perflib/nf-perflib-perfsetulonglongcountervalue).
It advances ID 5 last after a successful batch. Do not publish native rate
aliases or add collection callbacks. Create native state on this thread and
keep its handles and pointers confined there.

Do not point `PerfSetCounterRefValue` at live Rust atomics. The
[reference-counter API](https://learn.microsoft.com/en-us/windows/win32/api/perflib/nf-perflib-perfsetcounterrefvalue)
does not establish that foreign dereferences obey
[Rust atomic memory rules](https://doc.rust-lang.org/std/sync/atomic/index.html).
Copied values avoid that ownership issue, but do not make a multi-counter
snapshot atomic. Producers progress during reads, and consumers can observe
partially updated native values. ID 5 is a heartbeat, not a transaction or
seqlock guarantee; live counters need not reconcile exactly.

Derive approximate interval rates externally as raw-total deltas divided by
host-monotonic elapsed time, across several publication periods. Five-second
publication creates staircase values; faster polling does not create finer
source samples. Require two observations from the same instance, positive elapsed
time, and advancing sequence. A stable sequence across multiple expected
publication periods signals stale or unavailable telemetry, not an idle service.
A progressing sequence with unchanged source totals means the publisher is
alive with no observed counter progress. It cannot distinguish idle capture
from a blocked pipeline with backlog and is not a pipeline-health claim.
Restart, disappearance, decreasing totals, or unsigned wrap invalidates the rate
baseline. Use raw integers rather than rounded display values.

These nine counters deliberately do not measure byte throughput, latency,
queue/pool occupancy, stage CPU, separate parse/encode failures, or per-query
results. They cannot prove zero ETW session loss, exact final drain, complete
record accounting, or end-to-end collector delivery. Dequeued minus staged can
include ignored, rejected, failed-write, and in-flight records; it is not a loss
counter. Existing final diagnostics
retain broader counters and stop-time ETW validity/loss information; they are
not a guaranteed durable export. Use independently captured receiver/client
ledgers and suitable final diagnostics when those claims matter. Do not add
native queries or hot-path counters to make this minimal set answer them.

Counter instances are volatile. An external collector can retain earlier
samples, but cannot assume it observes the last publication before deletion.
Do not add a shutdown grace sleep or durable service output to compensate.

## Native lifecycle, packaging, and access

Use the documented [V2 provider workflow](https://learn.microsoft.com/en-us/windows/win32/perfctrs/providing-counter-data-using-version-2-0)
and [CTRPP generation](https://learn.microsoft.com/en-us/windows/win32/perfctrs/ctrpp):
manifest, generated helper header, localized resource script, and resources
linked into the protected service executable. Compile one narrow maintained C
translation unit that includes the generated header and invokes its generated
initialization/cleanup helpers. Take exact symbols and signatures from pinned
SDK output, with a bounded status-based Rust/C boundary and no allocator
ownership crossing. Do not translate generated metadata into Rust.

Match the repository's static CRT policy with `/MT`, compile as C, preserve
strict warnings, static analysis, stack protection, SDL, CFG, and existing final
link mitigations. Verify effective flags and native test instrumentation rather
than assuming Rust ASan covers C. The wrapper must preserve only statuses the
helpers actually expose: a void `CounterCleanup` cannot prove successful
`PerfStopProvider`. Inspect generated cleanup/handle semantics and establish
safe ownership completion before integration; never fabricate success or free
possibly borrowed storage. Keep generated metadata alive as required and add no
reference-counter buffers or native request callbacks.

The proposed owner sequence is bounded and preserves existing supervision:

1. After service identity validation, start the publisher thread and initialize
   its provider/instance there; complete a bounded readiness handshake.
2. Start capture through its existing path. Provider/publication failure emits
   one bounded nonsensitive warning and continues capture without counters;
   missing or stale telemetry is unavailable, not healthy zero values.
3. On stop, preserve native capture join, parser drain/join, and existing final
   diagnostics. Request a final copy if possible within the shutdown budget.
4. On the publisher thread, delete its instance and invoke generated cleanup,
   then join it before completing service shutdown. Preserve the ten-second
   watchdog; do not detach workers or release uncertain native ownership.

Package registration after protected executable/resources placement and
unregistration before resource removal, with transactional upgrade/repair and
rollback. Use supported V2 manifest registration tooling and verify exact target
unregistration syntax; legacy registration APIs do not install V2 metadata.
Do not self-register on service startup. Verify localized discovery as well as
runtime publication; a started provider alone does not prove discoverability.

Keep executable/resource/manifest writes restricted by existing trusted
administrator/SYSTEM policy. Rates can disclose workload activity: verify actual
local reader permissions against [limited-user counter access](https://learn.microsoft.com/en-us/windows/win32/perfctrs/limited-user-access-support).
Do not claim administrator-only or service-SID isolation without evidence.
Preserve service identity/privileges and avoid enabling remote collection to
solve local discovery failures. Use existing diagnostics, adding a bounded
warning path if needed, without misreporting optional metrics failure as service
failure. Rollback disables publication and restores prior package registration
and resources without changing TCP configuration.

## Implementation tasks and verification

These are future implementation tasks; this documentation change executes none
of them. Preserve unrelated working-tree changes and assign one owner per file.

| Order | Smallest reasonable model / effort | Inputs and proposed owned files | Deliverable and completion check |
| --- | --- | --- | --- |
| 1 | GPT-6 Luna / low | This catalog; `src/etw.rs`, `src/pipeline.rs`; proposed `src/metrics.rs` | Read-only access to eight existing atomics and the five-second snapshot. Review diff for zero added capture/parser instrumentation, unchanged record layout, and exactly nine public counters. |
| 2, after 1 | GPT-6.1 Sol / medium | Pinned SDK generated output, native headers, Rust/C hardening policy; proposed native wrapper/header, manifest, resource/build integration, `src/metrics/windows.rs` | Generated-helper provider, copied publication, and bounded lifecycle. Prove exact ABI, cleanup semantics, partial-start failure safety, and no native pointers crossing threads. |
| 3, after 2 | GPT-6.1 Sol / medium | Existing service/watchdog and provisioning policy; `src/service.rs`, `src/pipeline.rs`, chosen package/provision files | Optional failure/disabled behavior, protected resources, install/repair/upgrade/uninstall rollback. Disposable Windows checks prove discovery, local access, stop safety, and unchanged capture behavior without counters. |
| 4, after 3 | GPT-6 Luna / medium for test orchestration; GPT-6.1 Sol / medium for mandatory native review | Settled diff, independent PDH/PerfMon consumers, compatible external workload/collector harness | Independent native ownership/ABI review, focused native tests, and paired overhead report; resolve review findings and verify concurrent consumers, consumer churn, partial publication, restart, stale sequence, missing resources, and shutdown. No historical JSONL harness compatibility is assumed. |
| 5, after 4 | GPT-6 Luna / low | Verified shipped behavior and validation report; operator/developer documentation | Exact supported configuration/discovery instructions, interpretation limits, rollback, and documentation audit. Remove proposal language only for behavior actually implemented and verified. |

Compare matched release runs with metrics disabled and enabled, first without
consumers and then with one/multiple consumers. Repeat at least three times at
matched achieved load and record binary/configuration/instance identity. Use
built-in Process/Thread counters and WPR for CPU, memory, I/O, scheduling, and
contention rather than adding application counters. Check trace fidelity.

Proposed acceptance requires zero new hot-path instrumentation, unchanged
capture/output behavior, bounded shutdown, no ownership leaks, and no
consumer-driven resets. At the declared accepted workload/rate/capacity, require
zero channel-full drops and independently verify other capture/ETW/delivery gates;
the nine-counter set alone cannot establish them. Report paired CPU and
throughput variation; a proposed regression budget is at most 5% median service
CPU-seconds per supported record and at least 98% of disabled-mode throughput,
with all independent correctness gates still passing. These are review targets,
not measured results or a zero-overhead promise.
