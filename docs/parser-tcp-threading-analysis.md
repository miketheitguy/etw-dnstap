# Parser and TCP threading analysis

> **Status:** Proposed (design analysis; no change is implemented from it) · **Evidence dated:** not recorded · **Commit:** not recorded · **Reviewed:** 2026-11-01

This design analysis helps maintainers choose whether and how to parallelize
ETW parsing and dnstap TCP delivery. It compares ownership, ordering, resource
bounds, and failure handling against the inspected working tree and recorded
performance evidence. It makes no implementation or measured speedup claim.

## Recommendation

Multithreading is feasible, but adding parser threads is only a small part of
the work. The main decisions are the record-order contract, ownership of encoded
buffers, and coordinated failure and shutdown. Measure which stage limits the
service before selecting a topology or worker count.

Retain the single worker as the reference. If network waiting stalls otherwise
available parser capacity, compare one parser feeding one owning writer. If
parsing and encoding consume a core while other resources have headroom,
compare two parser workers. Two independent parser/encoder/TCP loops can require
fewer ownership changes than two parsers feeding one writer, because each loop
keeps its reusable output buffer. That option requires explicit acceptance of
multiple streams, reordered records, and additional collector resources.

Keep one TCP connection if preserving the existing accepted-record order is a
requirement. Start with one parser and one writer for that contract. Parallel
parsers need an additional bounded ordering design to preserve that order;
one writer alone does not preserve it. Do not choose a worker per logical CPU
or rewrite the service around async I/O without evidence for either decision.

## Evidence and its limits

The [four-CPU single-pass run](singlepass-four-core-test-results.md) achieved
94,639.07 queries per second (QPS). The client sent and matched 5,678,344 queries.
The collector contained 5,653,964 `CLIENT_QUERY` and 5,654,016
`CLIENT_RESPONSE` records, deficits of 0.42935% and 0.42843%. All client responses
succeeded, but collector reconciliation failed. These counts do not locate the
deficit in ETW admission, parsing, transport, or the collector.

Mean Windows guest CPU was 77.30% across four logical CPUs, equivalent to
approximately 3.092 busy cores and 0.908 idle cores averaged over the interval.
DNS, the service, background work, and measurement share those CPUs. The result
does not establish one free core for each additional parser, parser saturation,
or even that Windows CPU is the limiting resource. Ubuntu's 27.57% whole-guest
mean does not rule out one saturated collector stage. Inline gzip, the CPU
allocation, and different prior builds prevent a decoder-only comparison.

At two selected records per completed authoritative query, 94,639.07 QPS implies
about 189,278 records per second. An 8,192-entry queue represents approximately
43.3 ms of arrivals at that average rate if its consumer stops. This calculation
is a capacity illustration, not a measured arrival distribution, occupancy,
stall tolerance, or proof that the queue filled. Bursts and jumbo-pool admission
have separate limits.

The [historical native traces](pooled-vm-trace-analysis.md) demonstrate useful
analysis methods, but their binaries and workload differ. Their function,
scheduling, and metrics conclusions do not identify this build's bottleneck.

## Existing execution and ownership

The inspected implementation has one native `ProcessTrace` worker and one
`dnstap-parser` worker. The latter parses, encodes, writes TCP frames, flushes,
and performs the final STOP/FINISH exchange. The supervisor connects and
negotiates the receiver before reporting readiness. See
`run_capture` (`src/pipeline.rs`) and
`parse_loop` (`src/pipeline.rs`).

The callback copies an accepted event into a unique preallocated raw-event
lease and attempts a nonblocking enqueue. A full queue drops the newest event;
pool exhaustion is a separate admission failure. The parser polls with
`try_recv` and a two-millisecond timed park. The callback does not wake a
registered blocking receiver. Preserve that callback contract when adding
workers; replacing the intake receive path with blocking receivers or selectors
needs a separate callback-latency review. See
[ring capacity and shutdown](ring-buffer.md).

Each `Parser` (`src/etw.rs`) owns its schema cache and scratch storage.
The cache holds owned decode plans, clears at 32 entries, and loads TDH metadata
on a miss. Each `Encoder` (`src/dnstap.rs`) owns metadata and a reusable
output vector with a 70 KiB logical ceiling. Construct one parser and encoder
inside each worker. This avoids shared mutable cache locks and preserves
thread-local reuse; it duplicates cold schema work and memory. Do not add an
unsafe `Send` or `Sync` implementation to bypass an ownership error. Owned
`RawEtwEvent` leases already cross the existing channel; the scoped native
`EVENT_RECORD` view and its repaired pointers must stay inside the synchronous
parse operation.

`Parser::parse` consumes the raw event and returns bytes that borrow only the
encoder. The raw lease returns before network I/O. The next encode reuses those
bytes. This lifetime is the decisive constraint on a separate writer queue.

One `NetworkSink` (`src/network.rs`) owns the Frame Streams writer,
`BufWriter`, socket, and deadline state. `BufWriter` already combines small
writes; the periodic one-second flush bounds idle-buffer delay approximately,
subject to scheduling and blocked operations. Buffer acceptance is not collector
receipt. The `encoded` counter increments after `write_frame` succeeds, which
can include bytes still buffered locally. See
[Rust `BufWriter`](https://doc.rust-lang.org/std/io/struct.BufWriter.html).

## Ordering and connection contract

The existing single consumer submits successful records in the event queue's
receive order on one TCP stream. Ignored, rejected, and admission-dropped events
are absent. This is not a promise of timestamp order, real-world DNS causality,
query/response pairing, or complete capture. The service emits individual
query and response observations; it does not join them into transactions.

Choose one of these contracts before changing concurrency:

| Contract | Design consequence |
| --- | --- |
| Preserve accepted queue order | Use one parser, or serialize dispatch and add a bounded sequence/reorder stage for parallel parsers. |
| Accept completion order | Parallel parsers can publish as they finish. A response can pass an earlier, slower query. Preserve original timestamps and packet metadata. |
| Preserve a defined flow's order | Define a stable query/response-compatible routing key and serialize each shard. A DNS ID alone is insufficient. Missing endpoint fields, retries, UDP/TCP, and reused IDs need explicit rules. |

Cloning a Crossbeam receiver distributes events among consumers; it does not
broadcast them or preserve their completion order. See
[Crossbeam channel sharing](https://docs.rs/crossbeam-channel/latest/crossbeam_channel/).
For a global reorder stage, assign sequence numbers at a serialized dispatch
boundary. An atomic increment after independent receives can itself run out of
receive order. Ignored and rejected inputs need completion markers; any
preassigned sequence lost at admission needs an explicit gap policy. Bound
pending work and prevent workers from consuming all output leases with later
records while the next required record waits for a lease. Waiting for an early
record restores head-of-line blocking and increases design complexity. Bound the
total dispatched-but-uncommitted window and reserve capacity for every in-flight
result. A full reorder map must not prevent the writer from consuming the
awaited result from behind later results in the ready queue; size the dispatch
window and storage so this cannot deadlock. These are liveness requirements,
not only memory limits.

The raw event envelope does not already provide parsed endpoint keys for cheap
flow sharding. Parsing once to discover a key, then parsing again in a worker,
can defeat the intended saving. Preserve one ordered path unless a measured
need justifies a dispatch redesign. Collector correlation behavior must be
verified for the actual configured transforms; retaining timestamps, addresses,
ports, type, and DNS payload does not establish a universal unique match key.

Serialize every connection's frame bytes and protocol state through one owner.
`FrameStreamWriter::write_frame` (`src/framestream.rs`) writes the length
and payload separately. Concurrent socket handles or separate `BufWriter`s can
interleave frame bytes, buffering, and control frames. `TcpStream::try_clone`
creates another handle to the same stream, not another connection; Microsoft
also cautions against concurrent sends on a stream socket. See
[Rust `TcpStream`](https://doc.rust-lang.org/std/net/struct.TcpStream.html#method.try_clone)
and [Microsoft `WSASend` remarks](https://learn.microsoft.com/en-us/windows/win32/api/winsock2/nf-winsock2-wsasend#remarks).
A mutex around the complete existing sink can make ownership safe, but serializes
network work and blocks parser threads behind it; it is not independent delivery.

## Option comparison

The following ranking separates source-change complexity from behavioral risk.
All performance benefits are hypotheses until measured:

| Option | Relative source complexity | Likely benefit when justified | Main cost or risk |
| --- | --- | --- | --- |
| Keep one combined worker | Lowest | Avoid overhead when another stage limits throughput | Parsing stops during TCP stalls. |
| One parser and one writer | Moderate | Overlap parsing with socket waits while retaining queue order | Add owned output transfer, a bound, and another completion/failure path. |
| Two or more private parser/encoder/TCP loops | Low to moderate ownership change; moderate lifecycle change | Parallelize CPU work and independent socket/collector connection work | Lose common stream order and affinity; multiply sessions and receiver resources. |
| Two or more parsers and one writer | Moderate to high | Parallelize CPU work while keeping one connection | Add output leases; writer remains serial; completion order differs unless reordered. |
| Parallel parsers with strict global reorder | High | Parallelize portions of CPU work while preserving dispatch order | Bounded reorder, gaps, lease starvation, and head-of-line blocking. |
| Async transport/runtime rewrite | Highest breadth for this service | Manage many independent I/O waits if that becomes a requirement | New runtime/cancellation model; CPU parsing still needs parallel execution. |

One parser plus one writer does not accelerate parsing itself. Its queue absorbs
short network stalls and lets parsing overlap writes until the bound fills.
It cannot exceed a sustained TCP/receiver bottleneck. Two parsers plus one writer
can improve CPU throughput, but cannot remove the single writer, callback,
memory-bandwidth, or collector bottleneck.

Independent loops retain the existing borrowed-output lifetime and require no
encoded-output queue. Each loop needs a fresh `NetworkSink::connect`, independent
READY/ACCEPT/START, buffering, deadlines, and STOP/FINISH. A blocked socket stalls
only its worker until the intended whole-service failure path acts. This is
temporary isolation, not permission to ignore a failed stream.

The inspected
[DNS-collector v3.2.3 source](https://github.com/dmachard/DNS-collector/blob/v3.2.3/workers/dnstapserver.go)
accepts connections concurrently and creates a processor per connection, with
configurable processor workers. Its frame/batch enqueue paths can drop when
busy, and processors forward to the configured downstream routes. Multiple
connections therefore have a plausible receiver-parallelism benefit, but also
multiply queues/workers and can converge on the same constrained output. This
source inspection does not establish the deployed configuration or capacity.

Async I/O changes how waits use threads. It does not make synchronous parsing
parallel or increase a single TCP stream's delivery capacity. The existing
service has one receiver rather than thousands of idle connections, so the
extra runtime, cancellation, and framing work lacks an established benefit.

## Owned output and resource bounds

For a writer queue, transfer a unique owned encoded-frame lease. A borrowed
`&[u8]` into the parser's encoder cannot outlive subsequent reuse. Calling
`payload.to_vec()` on every record adds allocation and a full protobuf copy,
undoing the [direct encoding design](direct-packet-encoding-plan.md).

Prefer encoding directly into a leased, reusable vector. Split encoder metadata
from its destination, or swap fully provisioned vectors between the encoder and
a bounded pool. Moving a vector without returning a replacement loses reusable
capacity. Return every lease on all surviving-process paths, including success,
ignore, rejection, failed enqueue, disconnect, and normal shutdown. Unwinding
must recycle leases where applicable; process abort does not run destructors
and relies on operating-system resource reclamation. Do not retain the raw-event
lease while waiting on TCP. Waiting for output capacity can retain a raw event if it
has already been dequeued; include that worker-held lease in the input budget.

Define the output pool's total slots, not only the ready queue length. For a
completion-order queue that can hold `Q` frames, `N` active encoders, and one
writer-held frame, with no separately held reorder records, `B = Q + N + 1`
is a straightforward worst-case lease allowance. An ordered design must budget
the entire outstanding dispatch window, including reorder storage. A smaller pool
is valid if deliberate earlier backpressure is part of the design. A single
pool cap includes free, queued, worker-held, and writer-held slots; do not count
each category as an independent reserve.

The following examples count payload capacity only at 70 KiB per output slot:

| Total output slots | Maximum provisioned payload capacity |
| ---: | ---: |
| 256 | 17.5 MiB |
| 1,024 | 70 MiB |
| 8,192 | 560 MiB |

Thus a second 8,192-entry queue backed by maximum-size buffers is expensive.
The existing default counted raw pool, metadata, free-list, and channel storage
is approximately 37.32 MiB before native ETW buffers, caches, output, stacks,
allocator overhead, and socket buffers. See
[pooled ETW intake](pooled-intake.md). Budget measured high-water process memory
as well as these structural bounds. A two-class output pool can reduce regular
traffic cost, but must size against the complete encoded envelope, including
identity and metadata, and define behavior when the required class is exhausted.

The regular raw pool has `capacity + 2` slots. To retain the existing allowance
for a full queue, active parsing, and one callback-held lease with `N` parsers,
budget `capacity + N + 1` regular slots. Keeping the old count is memory safe,
but extra concurrently held events can exhaust that pool sooner. Jumbo slots
remain independently limited and shared. Independent loops otherwise add
approximately one encoder's bounded output, metadata/cache, stack, socket, and
buffering per worker, rather than a large output queue.

For the initial decoupled design, backpressure parser workers when the output
pool/queue is full, using cancellation-aware waits. This preserves the existing
drop-newest policy at intake instead of silently adding a post-parse drop point.
Blocking must remain outside the native callback. If output dropping is chosen
later, define its counter and conservation equations explicitly. Never use an
unbounded channel to hide a slow receiver.

Keep buffering under one writer owner. Evaluate byte and maximum-delay bounds
before adding batch thresholds; small traffic must still flush while idle.
`write_vectored` is an optional measured optimization, not a framing or atomicity
guarantee, and partial progress requires correct handling. See
[Rust `Write`](https://doc.rust-lang.org/std/io/trait.Write.html).

## Failure and shutdown contract

Preserve fail-fast connection loss and the absence of automatic retry, reconnect,
or replay. A successful local write does not prove collector processing or
durable output. Frame Streams FINISH acknowledges the stream-close exchange,
not each record's durable storage. After a partial write, retry can duplicate
records or corrupt framing without an acknowledgement/deduplication protocol.
See the [upstream writer lifecycle](https://github.com/farsightsec/fstrm/blob/master/fstrm/writer.c).

The existing two-second deadline covers each whole frame/flush/finish operation;
partial I/O does not refresh it. The existing ten-second watchdog bounds the
entire shutdown and aborts on expiry. Adding workers must preserve a common
shutdown deadline, rather than granting ten seconds per worker. Two-second
per-operation bounds do not bound a large queue's total drain.

For a decoupled pipeline, use this completion order:

1. Stop and join the native producer before publishing input completion.
2. Drain and join every parser, then publish output completion or close the
   final output sender. Account for every retained sender clone.
3. Drain the writer, flush final data, and perform STOP/FINISH exactly once.
4. Join all workers and disarm the watchdog only after resources are quiescent.

A writer must not treat an empty output queue plus native-producer completion
as final: parsers can still hold unfinished events. On writer error, immediately
publish fatal cancellation, disconnect or wake output waiters, release queued
leases, and stop capture. On fatal parser-worker failure, panic, or unexpected
exit, apply the same explicit whole-service policy. Preserve nonfatal rejection
and counting of recoverable malformed events. Supervisor-owned receiver/sender
clones must not accidentally keep blocked peers connected forever.

For independent connections, supervise all workers, roll back partially
successful startup, and finish healthy streams under the same global deadline.
Avoid a design that serializes handshake/finish timeouts beyond that bound.
Joining already-running workers in sequence does not itself serialize their
I/O; the relevant limits are concurrent operations and the common absolute
deadline. Retain frame poisoning: a partially failed stream must not receive
a misleading STOP. Idle flush, completion rechecks, and final-enqueue races
need coverage for every worker, not only the first.

## Proof gates and implementation sequence

No load, VM action, source change, or permanent metrics implementation belongs
to this analysis. Use the following sequence for a separately authorized trial:

1. **Define the contract.** Record whether accepted queue order and common-stream
   query/response affinity are required. Select an explicit memory budget and
   whole-service failure policy. The result is a reviewable topology decision.
2. **Measure the reference.** Keep binary, receiver configuration/output mode,
   workload, CPUs, and memory fixed. Record achieved client QPS, directional
   collector counts, DNS validity, service/DNS/collector CPU, and process memory.
   Separate trace-free runs from bounded profiling probes so overhead is visible.
3. **Identify the limiting stage.** Use targeted WPR sampled CPU plus precise
   scheduling/Ready/Wait evidence for the parser, native consumer, and DNS.
   Attribute socket waits using stacks or supporting network evidence; a Wait
   state alone does not identify TCP. Inspect collector processor/output pressure
   too. Verify trace retention, interval coverage, and trace loss before drawing
   conclusions.
4. **Implement the smallest evidence-supported candidate.** Use one parser plus
   one writer for demonstrated overlap opportunity. For demonstrated CPU pressure,
   start with two private loops if the multiple-stream contract is acceptable,
   or two parsers plus one writer with explicit output ownership. Keep one-worker
   behavior available as the comparison. Do not change pool sizes, batching,
   worker counts, and collector configuration together without separate evidence.
5. **Verify correctness before throughput.** Complete the focused fault and
   lifecycle cases in the test matrix. Measure warmed allocation behavior and
   maximum output sizes if the output API changes. The result must retain intact
   protobuf/frame bytes, bounded memory, and bounded failure/shutdown behavior.
6. **Compare matched trials.** Repeat the same achieved-load targets and include
   a trace-free confirmation. Accept the candidate only for a repeatable useful
   throughput/CPU improvement without a worse loss, DNS, memory, or lifecycle
   result. Increase worker count only if the next measured bottleneck and shared
   DNS CPU budget justify it.

The release profile disables Rust debug information, strips symbols, and passes
`/DEBUG:NONE`; see the release linker configuration in `.cargo/config.toml` (`[profile.release]` `rustflags`). WPR can still attribute
thread CPU and scheduling without promising resolved Rust function stacks.
If symbol-level attribution needs an optimized profiling build, identify its
separate binary and preserve hardening/optimization settings; do not present it
as measurement of an identical production artifact. Microsoft's
[CPU analysis guidance](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/cpu-analysis)
describes sampled and precise CPU views.

Prefer external profiling and existing final counts over per-event clocks or
durable service files. If stage evidence remains insufficient, design only the
minimal temporary, opt-in counters needed to distinguish raw admission,
parse rejection, output acceptance, and queue/pool pressure. Keep counters local
to workers and aggregate outside the hot path where practical. Shared per-record
atomics, extra channels, cache duplication, and allocator/refcount traffic can
consume the expected concurrency saving. Specify how evidence is retrieved:
SCM stderr is not itself a durable, reliable performance export. The
[Windows V2 counter plan](windows-performance-counters-v2-plan.md) is a separate
proposal, not an implemented prerequisite or part of this work.

## Test matrix for a candidate

The following cases distinguish architectural correctness from a speed result:

| Area | Required cases and expected result |
| --- | --- |
| Compatibility | One-worker path retains supported/fallback decoding, malformed rejection, exact protobuf bytes, event filtering, identity bounds, and no replay. |
| Ownership | Concurrent unique raw/output leases, regular/jumbo exhaustion, all drop/disconnect/error returns, and no packet reference beyond its owner. |
| Ordering | Deliberately slow first event, query/response split across workers, equal timestamps, ignore/reject gaps, and the selected order/affinity policy. |
| Framing | Short writes, partial header/payload failure, buffered flush failure, and no interleaving or STOP after poisoning. |
| Pressure | Slow/stalled receiver, full output queue/pool, cancellation while waiting for a lease/send, and bounded memory without callback blocking. |
| Completion | Final enqueue racing with completion, last parser still encoding after input drain, idle flush, worker panic/unexpected exit, and all leases accounted for. |
| Connection lifecycle | Refused/incompatible receiver, failure on the second connection, one failed stream among healthy streams, missing/truncated FINISH, and common ten-second fail-stop coverage. |
| Performance | Identify/hash baseline and candidate builds, keep workload/environment/receiver settings fixed, and repeat each candidate consistently; compare achieved rate and directional receipts, CPU by process/thread, collector pressure, memory, and trace overhead. |

Existing tests cover raw-pool concurrency, parser/encoder reuse, final enqueue,
frame poisoning, and shutdown phases. Extend those invariants where concurrency
changes ownership; avoid tests that merely repeat worker-count arithmetic.
Counts alone remain insufficient to prove individual query/response identity
or exactly-once delivery. A later result should state which topology passed
these gates and which measured stage explains its benefit.

## Source anchors

Use these code locations to review the ownership and lifecycle assumptions:

| Concern | Source |
| --- | --- |
| Startup and all-worker supervision boundary | `run_capture` (`src/pipeline.rs`) |
| Combined parse, encode, write, and idle flush loop | `parse_loop` (`src/pipeline.rs`) |
| Completion publication and final receive recheck | `drain_ring` (`src/pipeline.rs`) |
| Raw lease counts and class-specific recycling | `PoolManager` (`src/etw/pool.rs`) |
| Parser cache, scratch, and borrowed output lifetime | `Parser` (`src/etw.rs`) |
| Reusable encoded-output ownership and size bound | `Encoder` (`src/dnstap.rs`) |
| Whole-operation deadlines and STOP/FINISH | `NetworkSink` (`src/network.rs`) |
| Frame-byte ownership and poisoned writes | `FrameStreamWriter` (`src/framestream.rs`) |
