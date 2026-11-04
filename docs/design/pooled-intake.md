# Pooled ETW intake

> **Status:** Current · **Describes:** the code at commit `58fde72` (pooled intake present by `2c62a9f`) · **Evidence dated:** not applicable (reference. Live results are in the historical campaign records.) · **Reviewed:** 2026-11-03

This reference covers the ownership, memory budget, and loss accounting of the
pooled Event Tracing for Windows (ETW) intake. It describes the local
implementation. It does not establish production throughput or replace validation
of the installed service.

## Subscription and event selection

The session enables the DNS provider with these settings:

- Level `0xFF`. The level is a maximum, so every level passes.
- `MatchAnyKeyword` set to the resolved `QUERY_RECEIVED | RESPONSE_SUCCESS |
  RESPONSE_FAILURE` bits. The mask is `0x8000000000000000` when the keyword lookup
  fails. It is also that value when `QUERY_RECEIVED` or `RESPONSE_SUCCESS` is missing
  or malformed, or when any two of the three names share a bit. If only
  `RESPONSE_FAILURE` is missing or malformed, the mask keeps the other two bits, and
  the service does not capture event 258.
- `MatchAllKeyword` set to zero.
- `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0`, which keeps keyword-zero events out.
- An event-ID filter that admits only 256, 257, and 258.
- A process-ID filter that admits only the DNS Server process ID that startup resolved.

The keywords also enable events 284, 285, 287, 288, and 597 to 599. The event-ID filter
excludes them after the provider builds them. They cost `dns.exe` CPU but never reach
the service.

The callback accepts only event 256 (client query), 257 (successful client response),
and 258 (failed client response, for example SERVFAIL). It excludes all other IDs,
including 260 (recursive query) and 261 (recursive response). It checks the event ID
right after the null-record check, before any context access, counter, TLS, pool,
copy, queue, or TDH step. Excluded records are not counted. After context access, it
rejects a record whose `EventHeader.ProcessId` is not the DNS Server process ID, and it
counts that record in `foreign_pid_rejected`, not `received`. These provider and
event-ID checks are defense in depth. The parser repeats the event allowlist before TDH
access. This intake produces no resolver records.

See the [subscription filter plan](../history/etw-subscription-filter-plan.md) for the
implementation scope and local validation record.

## Ownership and callback lifecycle

`PoolManager` allocates and initializes every slot before capture starts. The
`ProcessTrace` worker installs a producer in thread-local storage (TLS) before it
enters the native API. The native callback runs in user mode on that worker, not in
kernel-mode application code. TLS holds the bounded channel sender, a pool reference, the capture counters, and the parser thread handle. It also holds a same-thread "enqueued" flag and the stable native callback-context identity. The callback uses `try_with` and
`try_borrow_mut`. Missing, wrong, or reentrant TLS increments `callback_errors` without
a panic.

`native_etw_callback` filters provider and event IDs, checks the process ID, counts
each supported record, checks lengths, leases storage, and calls
`ptr::copy_nonoverlapping` before it returns. After a successful `try_send`, it sets
the "enqueued" flag. It never wakes the parser.

`native_buffer_callback` is the `BufferCallback` of the trace. `ProcessTrace` calls it on
the same worker after each delivered buffer. It reads no pointer and always returns
TRUE, because FALSE would end `ProcessTrace`. It increments `etw_buffers_delivered`. If
the flag is set, it clears the flag, increments `parser_wake_requests`, and unparks the
parser. If TLS is borrowed, it does nothing, and the flag stays set for the next buffer.
If TLS is missing, it does nothing. See
[Parser wake and polling](ring-buffer.md#parser-wake-and-polling).

The event callback copies `UserData` and every nonempty extended-data payload. It keeps
the event header, buffer context, descriptor metadata, and lengths. It clears native
descriptor pointers and stores offsets instead. Before the payload copy, it rejects a native record with more than 16 extended items or more than 65,535 aggregate extended bytes. It also rejects a nonzero length with a null pointer. The native API must still supply
readable memory. These checks cannot prove that arbitrary non-null addresses are safe.

`RawEtwEvent` owns a `PooledBuffer` lease. A lease owns one boxed slot and a `&'static`
reference to the pool. The pool has no reference count, so a lease adds no atomic
operation, and a channel move allocates no ownership object. The parser uses the scoped
`with_record` helper to build a temporary typed descriptor array. The helper repairs
the array's pointers into the leased aligned storage and clears `UserContext`. The
parser keeps the lease during synchronous TDH calls. Each extended payload starts on an
8-byte boundary. No native callback pointer survives in the queued event.

Dropping a lease returns its slot to the matching bounded free list. The parser can
return a slot from another thread. `PoolManager::with_progress` leaks the pool once,
after it allocates every slot. The pool therefore lives for the rest of the process,
and no callback can destroy it. A failed startup frees the slots it already allocated,
because the leak comes last.

Each class has exactly N private, non-cloneable slots and N free-list entries. A
returning lease owns one slot outside the list, so concurrent returns cannot fill the
list before that slot returns. A full or missing recycle list is an internal invariant
failure and aborts the process. Ordinary pool exhaustion, channel overflow, and receiver
disconnection only increment counters and return.

Capture keeps the SCM-only entry point. Shutdown follows the protocol in
[Completion and resource ownership](ring-buffer.md#completion-and-resource-ownership).
TLS is cleared explicitly after `ProcessTrace` returns.

## Memory budget and configuration

The channel default is `--capacity 65536`. `--capacity` accepts 1,024 to 262,144, and
the pool accepts 1 to 262,144 slots. `--jumbo-capacity` accepts 0 to 256, and zero
disables jumbo capture. Storage has two independent classes:

- Regular: 2,048 bytes of payload per slot, in `--capacity` + 2 slots. A record uses a
  regular slot when its complete padded payload, plus 16 bytes per extended item, fits
  in 2 KiB.
- Jumbo: 131,448 bytes per slot, in `--jumbo-capacity` slots (default 64). A record that
  does not fit a regular slot uses a jumbo slot.

Regular records do not borrow jumbo slots, and jumbo records cannot use regular slots.
No slot grows after startup. The regular reserve covers a full channel, one
parser-held event, and one callback event. The default jumbo budget allows only 64
jumbo records owned at once by the channel, callback, and parser. The 65th can be
dropped even when the channel has space. Change this budget only with a measured
workload and an explicit memory allowance.

The jumbo size covers the accepted maximum of 65,535 user bytes and 65,535 aggregate extended bytes. It also covers up to 16 seven-byte alignment gaps, one more gap before the item table, and the item table itself. The table has 16 cells of 16 bytes. Slot selection counts
this padding and table, so each extended item costs 16 bytes of a regular slot.

On the configured Windows x64 target, each slot has a 24-byte allocation for the payload
pointer, the payload length, and the class flag. The item table is not in that
allocation. Items sit in the last bytes of the slot's payload storage. Each item is a
packed 8-byte header word and an 8-byte payload offset, 16 bytes in all. A slot has no
table when its record has no extended data, which is the case for this provider.
`RawEtwEvent` occupies 104 bytes. Free-list elements occupy 16 bytes, and channel
elements occupy 112 bytes.

Counted storage uses these formulas, where `C` is the channel capacity and `J` is the
jumbo capacity:

```text
regular payloads   = (C + 2) x 2,048
jumbo payloads     = J x 131,448
slot metadata      = (C + 2 + J) x 24
free-list elements = (C + 2 + J) x 16
channel elements   = C x 112
```

At the defaults (C 65,536, J 64):

| Allocation contents | Bytes |
| --- | ---: |
| Regular payloads | 134,221,824 |
| Jumbo payloads | 8,412,672 |
| Slot metadata | 1,574,448 |
| Free-list elements | 1,049,632 |
| Channel elements | 7,340,032 |
| Total counted storage | 152,598,608 (145.53 MiB) |

These totals exclude allocator rounding and metadata, queue headers, worker stacks,
native ETW buffers, TDH caches, protobuf data, and output buffers. They are not
process-private-memory measurements or total-memory limits. Payload pages are
initialized before native delivery. Startup still makes heap allocations, and a failure
of the ordinary allocation machinery can terminate the process. The defaults
are in the [tuning guide](../guide/tuning.md#defaults).

## Nonblocking admission and allocation scope

The callback leases with `ArrayQueue::pop` and submits with
`crossbeam_channel::Sender::try_send`. A full channel returns ownership of the event,
which recycles its lease at once. There is no capacity wait, callback logging, vector
growth, or fallback allocation. The receiver takes events only with `try_recv`, and it
reads the nonblocking `len` and `is_empty`. It never registers a blocking receiver or
selector. For the wake and polling rules, see
[Parser wake and polling](ring-buffer.md#parser-wake-and-polling).

`crossbeam-channel` 0.5.15 checks an atomic empty-waiter flag before it locks its
notification mutex. The production event path keeps that waiter list empty. A `recv`,
`recv_timeout`, iteration, or selection on this channel would remove that property.
Neither the channel nor the pool promises a hard wait-free execution bound, because
atomic retries, spinning, yielding, scheduling, and preemption still apply.

The callback allocation guarantee covers admission and steady-state lease recycling
while the pool stays owned. The parser and encoder also reuse bounded storage. Cold
schema loads, output growth, error construction, and native TDH internals stay outside
the focused allocation tests. Include the callback's bounded stack work and the
reference-count traffic when you measure a performance change.

Schema plans are compiled and cached as described in
[Single-pass decoder](single-pass-decoder.md#supported-schemas). An unsupported schema
is rejected and counted in `schema_rejected`. There is no per-event TDH fallback. A
schema-cache hit makes no TDH call, and a native cold load makes two
`TdhGetEventInformation` calls. Every decoded event passes the shared validation of
metadata, endpoint, timestamp, packet length, DNS direction, and output size.

`Parser::parse` borrows the pooled event and borrows `PacketData` from its payload.
`Encoder::encode_with`, which is private to `src/dnstap.rs`, copies it once into the
initialized, exact packet range of the final protobuf output. Parsing releases the pool
lease before the TCP write, so the returned payload borrows only encoder output. Only
cold metadata loads reconstruct a native event view. The encoder keeps identity, two
address vectors, Prost metadata, and requested output storage. Prost encodes the
metadata. Manual framing of the nested message and the packet field keeps the wire
output, with no intermediate packet vector. Errors clear the output. Allocators can
round up allocations, and output storage can grow until warmed. See the
[single-pass decoder contract](single-pass-decoder.md) for the interface and validation
scope.

The copy from ETW callback storage into owned storage is required, so the design is not
zero-copy. Pooling removes repeated storage allocation. The design alone does not raise
maximum throughput or allow a smaller safe queue. Warmed fixture tests do not establish
zero allocation across the full pipeline, higher throughput, or a CPU improvement.

## Metrics schema 2

This section covers the preserved JSONL evidence from historical binaries. The TCP-only
service has no JSONL metrics writer or durable metrics export. See the
[instrumentation compatibility note](../README.md#instrumentation-compatibility-note).

Schema 2 keeps `ring_dropped` as channel-full loss, adds explicit intake categories,
and adds `received`, `dequeued`, and `queue_backlog`. The counter meanings and identities
are in [Historical service loss and final ledgers](measurement-terms.md#historical-service-loss-and-final-ledgers).
The live log also has `foreign_pid_rejected`, `etw_buffers_delivered`, and
`parser_wake_requests`. The full live list is in [Log events and counters](../guide/monitoring.md).

Intermediate samples read separate relaxed atomics, so they can briefly differ from the
identities. Final samples follow native completion and parser join. Historical metrics
and supervisor receiver references can keep the channel connected after a parser
failure, so `receiver_disconnected` does not detect every parser exit. The supervisor
still detects parser completion, ends capture, reports the error, and keeps the
remaining backlog in the final ledger. Treat every intake loss category as loss when you
evaluate a zero-loss target.

The Python analyzers check schema 1 and 2 fields, the schema 2 intake sum, and final
capture and parser closure. `etw_stats_errors` counts failed statistics queries
separately. Invalid final statistics cannot establish zero loss. The analysis checks are
in the [measurement review](../history/pooled-intake-review-plan.md).

## Validation scope

The local tests cover the ownership, wake, and allocation behavior described above.
They do not run real TDH extraction or a live ETW session. The audit findings and
remaining checks are in the
[audit and remaining validation plan](../history/pooled-intake-audit-plan.md). Live DNS
and ETW results are in the
[native VM performance review](../performance/pooled-vm-performance-review.md).

## Native and library contracts

The implementation follows the documented
[ETW callback context contract](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nc-evntrace-pevent_record_callback),
[EVENT_RECORD fields](https://learn.microsoft.com/en-us/windows/win32/api/evntcons/ns-evntcons-event_record),
[Rust copy preconditions](https://doc.rust-lang.org/std/ptr/fn.copy_nonoverlapping.html),
and [nonblocking channel submission](https://docs.rs/crossbeam-channel/0.5.15/crossbeam_channel/struct.Sender.html#method.try_send).
`Cargo.lock` selects `crossbeam-channel` 0.5.15 and `crossbeam-queue` 0.3.14. The
notification fast-path discussion depends on those implementations.
