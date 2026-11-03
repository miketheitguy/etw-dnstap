# Pooled ETW intake

> **Status:** Current · **Describes:** `main` at commit `7f91264` (pooled intake present by `2c62a9f`) · **Evidence dated:** not applicable (reference. Live results are in the historical campaign records.) · **Reviewed:** 2026-11-02

This reference helps maintainers review the ownership, memory budget, and loss
accounting of the pooled Event Tracing for Windows (ETW) intake. It describes the
local implementation and its checks. It does not establish production throughput
or replace validation of the installed service.

## Subscription and event selection

The session enables the DNS provider with these settings:

- Level `0xFF`. The level is a maximum, so this passes every level and there is
  no level filter.
- `MatchAnyKeyword` set to the resolved `QUERY_RECEIVED | RESPONSE_SUCCESS |
  RESPONSE_FAILURE` bits, or to `0x8000000000000000` when the keyword lookup
  fails or any of the three names is missing, malformed, or shares a bit.
- `MatchAllKeyword` set to zero.
- `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0`, which keeps keyword-zero events out.
- An ETW event-ID filter that admits only 256, 257, and 258.
- A second ETW filter that admits only events from the DNS Server service
  process ID that startup resolved.

The provider and event-ID checks in the callback are therefore defense in depth.
The keywords also enable events that include 284, 285, 287, 288, and 597 to 599.
The event-ID filter excludes them after the provider builds them, so they cost `dns.exe` CPU
but never reach the service.

The callback accepts only event 256 (client query), 257 (successful client
response), and 258 (failed client response, for example SERVFAIL). It checks
this right after the null-record check. It excludes all other event IDs, including 260
(recursive query) and 261 (recursive response). This check comes before context access, counters,
TLS, pool acquisition, payload copy, queue admission, and TDH parsing. Excluded records do not count
as received records, overload drops, or errors. After context access, the
callback also rejects a record whose `EventHeader.ProcessId` is not that DNS
Server process ID. It counts that record in `foreign_pid_rejected`, not in
`received`. The parser repeats the event allowlist before TDH access. This
intake produces no resolver records.

See the [subscription filter plan](../history/etw-subscription-filter-plan.md) for the
implementation scope and local validation record.

## Ownership and callback lifecycle

`PoolManager` allocates and initializes every slot before capture starts. The
`ProcessTrace` worker installs a producer in thread-local storage (TLS) before it
enters the native API. The native callback executes in user mode on that
worker. It is not kernel-mode application code. TLS contains these items:

- The bounded channel sender and a pool reference.
- The capture counters and the parser thread handle.
- A same-thread "enqueued" flag.
- The stable native callback-context identity.

The callback uses `try_with` and `try_borrow_mut`. Missing, wrong, or reentrant
TLS increments `callback_errors` without a panic.

`native_etw_callback` filters provider and event IDs, checks the process ID,
counts each supported record, validates lengths, leases storage, and calls
`ptr::copy_nonoverlapping` before returning. After a successful `try_send` it
sets the "enqueued" flag. It never wakes the parser.

`native_buffer_callback` is the `BufferCallback` of the trace. `ProcessTrace`
calls it on the same worker after it delivers each buffer. It reads no pointer
and always returns TRUE, because FALSE would end `ProcessTrace`. It increments
`etw_buffers_delivered`. If the "enqueued" flag is set, it clears the flag,
increments `parser_wake_requests`, and unparks the parser. If the TLS state is
borrowed, it does nothing, and the flag stays set for the next buffer. If the TLS
state is missing, it does nothing.

The event callback copies `UserData` and every nonempty extended-data payload.
It preserves the event header, buffer context, descriptor metadata, and lengths.
It clears native descriptor pointers and stores offsets instead. The callback
rejects a native record before the payload copy in these cases:

- It has more than 16 extended items.
- It has more than 65,535 aggregate extended bytes.
- It has a nonzero length with a null pointer.

The native API must still supply readable memory. These checks cannot validate
arbitrary non-null addresses.

`RawEtwEvent` owns a `PooledBuffer` lease. Each lease owns one boxed slot and a
`&'static` reference to the pool. The pool has no reference count, so a lease
adds no atomic operation. A move through the channel allocates no ownership
object. The parser uses the scoped `with_record` helper to build a temporary
typed descriptor array. The helper repairs the array's pointers into the leased
aligned storage and clears `UserContext`. The parser keeps the lease during
synchronous TDH calls. The parser and reconstruction tests use this same helper.
Each extended payload starts on an 8-byte boundary. No native callback pointer
survives in the queued event.

Dropping the lease returns its slot to the corresponding bounded free list. The
parser can return a slot from another thread. `PoolManager::with_progress`
leaks the pool once, after it allocates every slot. The pool therefore lives for
the rest of the process, and no callback can destroy it. The service exits when
the pipeline ends, and the operating system then frees the memory. A failed
startup frees the slots that it already allocated, because the leak comes last.

There are exactly N private, non-cloneable slots in a class and N free-list
entries. A returning lease owns one slot outside the list. Legitimate concurrent
returns therefore cannot fill the list before that slot returns. A full or
missing recycle list is an internal invariant failure and aborts the process.
Ordinary pool exhaustion, channel overflow, and receiver disconnection only
increment counters and return.

Capture keeps the SCM-only entry point, the native stop and join order, the
10-second shutdown watchdog, and the parser completion protocol. After native join,
the supervisor publishes `producer_done` with release ordering and unparks the
parser. The parser observes completion with acquire ordering and retries the
channel until all final queued records have drained. TLS is cleared explicitly
after `ProcessTrace` returns.

## Memory budget and configuration

The channel default is `--capacity 24576`. It was 8192 before 2026-11-01. The command line accepts 1,024
through 262,144. The pool itself accepts 1 through 262,144. Storage has two
independent classes:

| Class | Payload storage per slot | Slots | Admission |
| --- | ---: | ---: | --- |
| Regular | 2,048 bytes | Channel capacity + 2 | Complete padded payload, plus 16 bytes per extended item, fits 2 KiB |
| Jumbo | 131,448 bytes | `--jumbo-capacity`, default 64 | Complete padded payload, plus 16 bytes per extended item, exceeds 2 KiB |

`--jumbo-capacity` accepts 0 through 256. Zero disables jumbo capture. Regular
records do not borrow jumbo slots, and jumbo records cannot use regular slots.
No slot grows after startup. The regular reserve covers a full channel, one
parser-held event, and one callback event. The default jumbo budget allows only
64 jumbo records owned at the same time by the channel, callback, and parser.
The 65th can be dropped even when the channel has space. Adjust this budget only
with a measured workload and an explicit memory allowance.

The jumbo size covers these parts:

- The previous accepted maximum of 65,535 user bytes.
- 65,535 aggregate extended bytes.
- Up to 16 seven-byte alignment gaps, and one more gap before the item table.
- The item table, which has 16 cells of 16 bytes.

Slot selection includes alignment padding and the item table, not only the DNS
packet length. A record with extended data therefore has 16 fewer payload bytes
in a regular slot for each item it carries. If it does not fit, it uses a jumbo
slot.

On the configured Windows x64 target, each slot also has a 24-byte
allocation. It holds the payload pointer and length and the class flag. The
extended-data item table is not in it. A record's items are stored in the last
bytes of the slot's own payload storage. Each item is a packed 8-byte header
word and an 8-byte payload offset, 16 bytes in all. A slot therefore carries no
table when the record has no extended data, which is the case for this
provider. The allocation was
408 bytes while the table was in the header. `RawEtwEvent` occupies 104 bytes. With the locked queue versions,
free-list elements occupy 16 bytes and channel elements occupy 112 bytes. The
following table counts requested element and payload storage:

| Allocation contents | Default 24,576 / 64 | Maximum 262,144 / 256 |
| --- | ---: | ---: |
| Regular payloads | 50,335,744 bytes | 536,875,008 bytes |
| Jumbo payloads | 8,412,672 bytes | 33,650,688 bytes |
| Slot metadata allocations | 591,408 bytes | 6,297,648 bytes |
| Free-list elements | 394,272 bytes | 4,198,432 bytes |
| Channel elements | 2,752,512 bytes | 29,360,128 bytes |
| Total counted storage | 62,486,608 bytes (59.59 MiB) | 610,381,904 bytes (582.11 MiB) |

The maximum column uses the same per-slot sizes as the default column, with
262,144 channel slots and 256 jumbo slots:

- Regular payloads: (262,144 + 2) x 2,048 = 536,875,008 bytes.
- Jumbo payloads: 256 x 131,448 = 33,650,688 bytes.
- Slot metadata: (262,146 + 256) x 24 = 262,402 x 24 = 6,297,648 bytes.
- Free-list elements: 262,402 x 16 = 4,198,432 bytes.
- Channel elements: 262,144 x 112 = 29,360,128 bytes.
- Total: 536,875,008 + 33,650,688 + 6,297,648 + 4,198,432 + 29,360,128 = 610,381,904 bytes,
  which is 610,381,904 / 1,048,576 = 582.11 MiB.

The maximum capacity therefore costs about 522.5 MiB more than the default. The previous default of 8,192 counts 26,441,808 bytes (25.22 MiB). The previous maximum of
65,536 / 256 counts 177,844,304 bytes (169.61 MiB). (Before the item table left the slot header these four figures were 29,595,984, 203,041,616 bytes, and the totals above were 71,932,240 and 711,076,688.)

The high-rate setting `--capacity 65536` with the default 64 jumbo slots uses
the same sizes:

- Regular payloads: (65,536 + 2) x 2,048 = 134,221,824 bytes.
- Jumbo payloads: 64 x 131,448 = 8,412,672 bytes.
- Slot metadata: (65,538 + 64) x 24 = 65,602 x 24 = 1,574,448 bytes.
- Free-list elements: 65,602 x 16 = 1,049,632 bytes.
- Channel elements: 65,536 x 112 = 7,340,032 bytes.
- Total: 152,598,608 bytes, which is 145.53 MiB.

On the lab VM, the process private bytes were about 31 MiB with the previous
defaults (8,192 events). They were about 179 MiB with the high-rate settings. The
lab has not measured the current defaults. See the
[flush removal validation](../history/flush-removal-validation.md).

The tuning guide gives `--capacity 131072` as an example for
sustained rates near 50,000 queries per second. The lab did not test it. With
the default 64 jumbo slots it uses the same sizes:

- Regular payloads: (131,072 + 2) x 2,048 = 268,439,552 bytes.
- Jumbo payloads: 64 x 131,448 = 8,412,672 bytes.
- Slot metadata: (131,074 + 64) x 24 = 131,138 x 24 = 3,147,312 bytes.
- Free-list elements: 131,138 x 16 = 2,098,208 bytes.
- Channel elements: 131,072 x 112 = 14,680,064 bytes.
- Total: 296,777,808 bytes, which is 283.03 MiB.

The high-rate setting and its lab results are in the
[tuning guide](../guide/tuning.md#high-rate-settings).

These totals exclude allocator rounding, allocator metadata, the small queue headers,
worker stacks, native ETW buffers, TDH caches, protobuf data, and
output buffers. They are not process-private-memory measurements or total-memory
limits. Payload pages are initialized before native delivery. Startup still
does heap allocations. A failure of the ordinary allocation machinery can
terminate the process.

## Nonblocking admission and allocation scope

The callback leases with `ArrayQueue::pop` and submits with
`crossbeam_channel::Sender::try_send`. A full channel returns ownership of the
event, which immediately recycles its lease. There is no capacity wait, callback
logging, vector growth, or fallback allocation.

The event-channel receiver takes events only with `try_recv`. It also reads
the nonblocking `len` and `is_empty`. It never registers a blocking receiver or
selector. The parser wakes and polls in this way:

- The ETW buffer callback wakes it once for each delivered buffer that enqueued
  events.
- After data, it polls for up to 200 µs before it parks. The linger turns off
  after a linger that finds nothing. It turns back on after an idle park.
- Between empty polls it parks. The park backs off from 2 ms to 20 ms and
  resets after each event.

`crossbeam-channel` 0.5.15 checks an atomic empty-waiter flag before it locks its
notification mutex. The production event path keeps that waiter list empty.
A `recv`, `recv_timeout`, iteration, or selection on this channel would remove
that property. The channel API is nonblocking. Neither the channel nor the pool
promises a hard wait-free execution bound, because atomic retries, spinning,
yielding, scheduling, and preemption still apply.

The callback allocation guarantee covers admission and steady-state lease
recycling while the pool remains owned. The parser and encoder also reuse
bounded storage on the parser thread. Cold schema loads, output growth,
error construction, native TDH internals, and output remain outside their
focused steady-state allocation tests. The callback also has bounded stack
work for up to 16 descriptors and offsets. The parser uses a fixed descriptor
array. This fixed work and reference-count traffic must
be included when measuring any performance change.

The parser owns a cache of at most 8 schemas keyed by event ID and version.
It compiles validated TDH metadata into plans with at most 256 fields and
property names of at most 256 UTF-16 units. Each plan walks only the prefix
through the last required property. The supported layouts are:

- 8-, 16-, and 32-bit integer scalars.
- Boolean scalars.
- Terminated ANSI and UTF-16 strings.
- Binary fields with a fixed length or a preceding unsigned scalar length.

An array, a structure, a custom schema, or an unknown layout in that prefix
makes the schema unsupported. Its events are rejected and counted in
`schema_rejected`. Binary fields with zero length and no explicit fixed-length
flag are also unsupported. There is no per-event TDH fallback. Unsupported
trailing layouts can stay as names-only sentinels. The decoder does not inspect
their bytes. Duplicate required property
names prevent plan compilation, including duplicates after the required prefix.

A schema-cache hit makes no TDH calls. A native cold load makes two
`TdhGetEventInformation` calls to obtain metadata. The parser reuses 768 bytes
of address text scratch. Malformed payloads under a plan are rejected without
a retry through TDH. Every decoded event passes the shared validation of
metadata, endpoint, timestamp, packet length, DNS direction, and output size.

`Parser::parse` borrows the pooled event. The parser borrows `PacketData` from
its payload. `Encoder::encode_with`, which is private to `src/dnstap.rs`, copies
it once into the initialized, exact packet range in the final protobuf output.
Only cold metadata loads reconstruct a native event view. Parsing releases the
pool lease before the TCP write. The returned payload borrows only encoder
output.

The encoder retains identity, two address vectors, Prost metadata, and at most
70 KiB of requested output storage. It accepts packet lengths from 12 through
65,535 bytes. Allocators may round allocations beyond the requested size, and
output storage can grow until warmed. Prost encodes the metadata. Manual
framing of the nested message and the packet field preserves the wire output. Neither path
uses an intermediate packet vector. The callback copy and initialized-output
zeroing remain. Query/response fields and optional endpoint presence reset for
each event, and errors clear the output. See the
[single-pass decoder contract](single-pass-decoder.md) for the maintained
interface, supported grammar, and local validation scope. Focused warmed native
fixture tests do not establish zero allocation across the full pipeline,
higher throughput, or a live CPU improvement.

The copy from native ETW callback storage into owned storage remains required.
Pooling removes repeated storage allocation. It is not literal zero-copy.
Neither a higher maximum throughput nor a smaller safe queue follows from this
design alone. The preserved ArrayQueue baseline had zero ring loss in its stated
1,000 QPS, 300-second authoritative and recursive runs. The recursive run retained
one RA-check failure. Those results apply to the baseline binary and do not
validate the pooled candidate or its separate jumbo budget. The implementation
follow-up itself made no VM or capacity-default change. The later native
campaign is recorded in [Validation scope](#validation-scope).

## Metrics schema 2

This section documents the preserved JSONL evidence from historical binaries.
The TCP-only service has no JSONL metrics writer or durable metrics export.
See the [instrumentation compatibility note](../README.md#instrumentation-compatibility-note). Its diagnostic intake
counters retain the admission meanings listed here. The historical
`output_limit_dropped` field has no counterpart in the TCP-only parser ledger.

Schema 2 preserves `ring_dropped` as channel-full loss and adds explicit intake
loss categories:

| Counter | Meaning |
| --- | --- |
| `received` | Supported provider records entering the callback, including rejected records |
| `ring_dropped` | Records rejected by `try_send` because the channel is full |
| `pool_exhausted` | Records without an available slot in the required class |
| `receiver_disconnected` | Records rejected by `try_send` after all receivers disconnect |
| `intake_dropped` | Sum of those three admission loss categories |
| `callback_errors` | Malformed native metadata or unavailable, wrong, or reentrant TLS |
| `dequeued` | Records removed by the parser before parsing or output |
| `queue_backlog` | Records remaining in the channel |

The following counters exist only in the live log of the TCP-only service. They
are not schema 2 fields and are not part of the identities below. The full live
list is in
[Log events and counters](../guide/monitoring.md).

| Live counter | Meaning |
| --- | --- |
| `foreign_pid_rejected` | Records from a process other than the DNS Server. Not counted in `received`. |
| `etw_buffers_delivered` | ETW buffers whose delivery finished (buffer callback calls) |
| `parser_wake_requests` | Delivered buffers that enqueued events and so sent the parser a wake request |

At quiescence, the accounting identities are:

```text
intake_dropped = ring_dropped + pool_exhausted + receiver_disconnected
received = intake_dropped + callback_errors + dequeued + queue_backlog
dequeued = encoded + ignored + rejected + output_limit_dropped + output_failed_events
```

Intermediate samples read separate relaxed atomics and can briefly differ from
these identities. Final samples follow native completion and parser join.
Historical metrics and supervisor receiver references can keep the channel
connected after a parser failure. `receiver_disconnected` therefore does not
detect every parser exit. The supervisor still detects parser completion, stops capture, reports the
error, and retains the remaining backlog in the final ledger. Treat every intake
loss category as loss when evaluating a zero-loss target. The Python analyzers
validate schema 1 and 2 fields, the schema 2 intake sum, and final capture/parser
closure through one shared normalizer. Schema 1 retains its ring-only intake
meaning. Intermediate phase samples need not close, but final samples must.
`etw_stats_errors` counts failed statistics queries separately. A valid final
query with zero cumulative ETW loss can pass despite transient query failures.
Invalid final statistics cannot establish zero loss. See the
[measurement review](../history/pooled-intake-review-plan.md) for analysis checks and
preserved-data comparisons.

## Validation scope

Focused tests exercise these cases:

- Copy of nonempty user data and extended data.
- Mutation and destruction of the source buffer.
- Maximum padded extended layouts.
- Disabled jumbo capture, pool exhaustion, channel overflow, and receiver
  disconnection.
- Wrong and reentrant TLS.
- Cross-thread recycling, and queued ownership after the native owner exits.
- Final queue drain.

Concurrent pool workers check unique active addresses,
independent sentinel data, and recovery of every slot.

The parser's shared native-view helper is exercised with zero and 16 extended
items after native source storage ends. The tests inspect these items:

- Copied bytes, the header, and the buffer context.
- Descriptor metadata, and the alignment and bounds of repaired pointers.
- Null context and lease exclusivity.
- A dirty slot reused for an empty record.

These tests do not execute real TDH extraction. Further tests call the buffer
callback directly. They check one wake request per buffer that enqueued events,
and that a buffer callback during a TLS borrow keeps the pending wake.

A test-only Rust global allocator counts allocations, reallocations, and
deallocations separately. It counts them during callback success, a full
channel, regular and jumbo exhaustion, jumbo success, disconnection, malformed
metadata, and TLS failures.
It also measures an isolated parser-thread lease return while the pool remains
owned. Setup and TLS installation occur outside these
measurement scopes. This does not instrument arbitrary direct allocations in
Windows DLLs or prove the behavior of a real installed ETW session.

The ignored `synthetic_pooled_intake_benchmark` test is an optional local
callback/channel/lease harness. It excludes native ETW delivery, TDH parsing,
protobuf encoding, and output. The original `perf_windows` benchmark matrix and
`perf_review` security review stay stopped at the project owner's request.
Their draft artifacts are preserved. That matrix has not run. A separate, fresh read-only
Astra audit reviewed the source and analysis after implementation. See the
[audit and remaining validation plan](../history/pooled-intake-audit-plan.md) for its
findings, follow-up checks, exact artifact identities, and final review status.
The follow-up local [receipt](../../datasets/pooled-intake-local/audit-fixes-v1/receipt.json)
records 45 passing tests and one ignored manual benchmark in development,
release-unwind, and ASan configurations. It preserves the initial 42-test
candidate separately and does not claim native TDH or VM validation.

The separate [native VM performance review](../performance/pooled-vm-performance-review.md)
records the reviewed candidate's actual DNS/ETW runs, bounded overload discovery,
and full five-minute authoritative/recursive confirmations. Its all-category
record ledgers, actual upstream mix, strict DNS/lifecycle outcomes, resource
coverage, and native WPR gates determine those claims. This later campaign does
not change the preserved local receipts or restart the stopped original matrix.
Instrumentation and receiver storage differ from the historical capacity-only
baseline. Those single trials do not establish a causal CPU saving from pooling.

## Native and library contracts

The implementation follows the documented
[ETW callback context contract](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nc-evntrace-pevent_record_callback),
[EVENT_RECORD fields](https://learn.microsoft.com/en-us/windows/win32/api/evntcons/ns-evntcons-event_record),
[Rust copy preconditions](https://doc.rust-lang.org/std/ptr/fn.copy_nonoverlapping.html),
and [nonblocking channel submission](https://docs.rs/crossbeam-channel/0.5.15/crossbeam_channel/struct.Sender.html#method.try_send).
`Cargo.lock` selects `crossbeam-channel` 0.5.15 and `crossbeam-queue` 0.3.14.
The notification fast-path discussion depends on those implementations.
