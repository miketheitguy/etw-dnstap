# Pooled ETW intake

> **Status:** Current · **Describes:** the code on branch `spool-encryption` as of 2026-11-05 · **Evidence dated:** not applicable (reference. Live results are in the historical campaign records.) · **Reviewed:** 2026-11-05

This reference covers how the Event Tracing for Windows (ETW) intake owns memory, how
large its memory budget is, and how it admits events. It describes this implementation.
It does not establish production throughput, and it does not replace validation of the
installed service.

For the stage order and the counter for each loss, read [Data path](data-path.md). For
the parser wake rules, read [Ring capacity and shutdown](ring-buffer.md).

## Subscription and event selection

The session enables the DNS Server provider with these settings:

- Level `0xFF`. The level is a maximum, so every level passes.
- `MatchAnyKeyword` set to the resolved `QUERY_RECEIVED | RESPONSE_SUCCESS |
  RESPONSE_FAILURE` bits. The mask is `0x8000000000000000` when the keyword lookup
  fails. It is also that value when `QUERY_RECEIVED` or `RESPONSE_SUCCESS` is missing
  or malformed, or when any two of the three names share a bit. If only
  `RESPONSE_FAILURE` is missing or malformed, the mask keeps the other two bits, and the
  service does not capture event 258.
- `MatchAllKeyword` set to zero.
- `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0`, which keeps keyword-zero events out.
- An event-ID filter that admits only 256, 257, and 258.

The keywords also enable events 284, 285, 287, 288, and 597 to 599. The event-ID filter
drops them after the provider builds them. They cost `dns.exe` CPU, but they never reach
the service.

The callback accepts only:

- 256, a client query.
- 257, a successful client response.
- 258, a failed client response, such as SERVFAIL.

It drops every other ID, including 260 (recursive query) and 261 (recursive response).
It checks the provider and the event ID right after the null-record check. This happens
before any counter, TLS, pool, copy, queue, or TDH step. Dropped records are not counted.
The callback does not check `EventHeader.ProcessId`, so a record from any process counts
in `received`. These provider and event-ID checks are defense in depth. The parser repeats
the event allowlist before any TDH access. This intake produces no resolver records.

The [subscription filter plan](../history/etw-subscription-filter-plan.md) gives the
implementation scope and the local validation record.

## Ownership and callback lifecycle

`PoolManager` allocates and initializes every slot before capture starts. The
`ProcessTrace` worker installs a producer state in thread-local storage (TLS) before it
enters the native API. The native callbacks run in user mode on that worker. The producer
state holds the channel sender, a pool reference, the capture counters, the parser thread
handle, an "enqueued" flag, and the native callback-context identity. Each callback uses
`try_with` and `try_borrow_mut`. If TLS is missing, wrong, or re-entered, the event counts
in `callback_errors`, and nothing panics.

**Event callback.** `native_etw_callback` checks the provider and event ID, counts
`received`, checks lengths, leases a slot, and copies the record with
`ptr::copy_nonoverlapping`. The copy covers `UserData` and each nonempty extended-data
payload. The callback keeps the event header, the buffer context, the descriptor
metadata, and the lengths. It clears the native descriptor pointers and stores offsets
instead. After a successful `try_send`, it sets the "enqueued" flag. It never wakes the
parser.

Before it copies, the callback rejects a native record that has more than 16 extended
items, more than 65,535 aggregate extended bytes, or a nonzero length with a null
pointer. The API must still supply readable memory. These checks cannot prove that an
arbitrary non-null address is safe.

**Buffer callback.** `native_buffer_callback` is the buffer callback of the trace.
`ProcessTrace` calls it on the same worker after each delivered buffer. It reads no
pointer and always returns TRUE, because FALSE would end `ProcessTrace`. It counts
`etw_buffers_delivered`. If the "enqueued" flag is set, it clears the flag, counts
`parser_wake_requests`, and wakes the parser. If TLS is borrowed or missing, it does
nothing, and the flag stays set for the next buffer. See
[Parser wake and polling](ring-buffer.md#parser-wake-and-polling).

**Leases.** A `RawEtwEvent` owns a `PooledBuffer` lease. The lease holds one boxed slot
and a `&'static` reference to the pool. The pool has no reference count, so a lease adds
no atomic operation, and a channel move allocates no ownership object. The parser uses the
scoped `with_record` helper to build a temporary typed descriptor array in the leased
storage. The helper repairs the array's pointers and clears `UserContext`. The parser
keeps the lease during the synchronous TDH calls. Each extended payload starts on an
8-byte boundary. No native callback pointer survives in the queued event.

Dropping a lease returns its slot to the matching bounded free list. The parser can return
a slot from another thread.

**Lifetime.** `PoolManager::with_progress` leaks the pool once, after it allocates every
slot. The pool therefore lives for the rest of the process, and no callback can destroy
it. If startup fails, the slots already allocated are freed, because the leak happens
last.

**Invariant.** Each class has exactly N private, non-cloneable slots and N free-list
entries. A returning lease holds its slot outside the list until the slot is pushed back,
so concurrent returns cannot fill the list early. A full or missing recycle list is an
internal invariant failure, and it aborts the process. Ordinary pool exhaustion, channel
overflow, and receiver disconnection only count and return.

Shutdown follows [Completion and resource ownership](ring-buffer.md#completion-and-resource-ownership).
TLS is cleared explicitly after `ProcessTrace` returns.

## Memory budget and configuration

The defaults are `--capacity 131072` and `--jumbo-capacity 64`. `--capacity` accepts
1,024 to 524,288. The pool accepts 1 to 524,288 slots. `--jumbo-capacity` accepts 0 to
256, and 0 disables jumbo capture. Storage has two classes with independent admission
limits:

- **Regular:** 2,048 bytes of payload per slot, in `--capacity` + 2 slots. A record uses a
  regular slot when its padded payload, plus 16 bytes for each extended item, fits in 2 KiB.
- **Jumbo:** 131,448 bytes per slot, in `--jumbo-capacity` slots. A record that does not
  fit a regular slot uses a jumbo slot.

Regular records never borrow jumbo slots, and jumbo records never use regular slots. No
slot grows after startup. The regular reserve covers a full channel, one event held by the
parser, and one event in the callback. The default jumbo budget allows 64 jumbo records to
be held at once across the channel, the callback, and the parser. A 65th jumbo record can
be dropped even when the channel has room. Change this budget only with a measured
workload and an explicit memory allowance.

The jumbo size covers the largest accepted record: 65,535 user bytes and 65,535 bytes of
extended data. It also covers up to 16 seven-byte alignment gaps, one more gap before the
item table, and the item table itself. The table has 16 cells of 16 bytes. Slot selection
counts this padding and table, so each extended item costs 16 bytes of a regular slot.

On the configured Windows x64 target, each slot has a 24-byte allocation for the payload
pointer, the payload length, and the class flag. The item table is not in that allocation.
It sits in the last bytes of the slot's payload storage, and each item takes 16 bytes: a
packed 8-byte header word and an 8-byte payload offset. A slot has no table when its
record has no extended data, which is the case for this provider. `RawEtwEvent` is 104
bytes, a free-list element is 16 bytes, and a channel element is 112 bytes.

Counted storage uses these formulas, where `C` is the channel capacity and `J` is the
jumbo capacity:

```text
regular payloads   = (C + 2) x 2,048
jumbo payloads     = J x 131,448
slot metadata      = (C + 2 + J) x 24
free-list elements = (C + 2 + J) x 16
channel elements   = C x 112
```

At the defaults (C = 131,072, J = 64):

| Allocation contents | Bytes |
| --- | ---: |
| Regular payloads | 268,439,552 |
| Jumbo payloads | 8,412,672 |
| Slot metadata | 3,147,312 |
| Free-list elements | 2,098,208 |
| Channel elements | 14,680,064 |
| Total counted storage | 296,777,808 (283.03 MiB) |

At the largest channel (C = 524,288), the same formulas give 1,161,853,008 bytes
(1,108.03 MiB) with 64 jumbo slots, and 1,187,098,704 bytes (1,132.11 MiB) with 256. The
regular payloads are 1,073,745,920 bytes, and the channel elements are 58,720,256 bytes.
The widest product is `(C + 2) x 2,048`, about 1.1e9, so the arithmetic fits a 32-bit
count as well as `usize`.

The totals exclude allocator rounding and metadata, queue headers, worker stacks, native
ETW buffers, TDH caches, protobuf data, and output buffers. They are not
process-private-memory measurements, and they are not total-memory limits. Measure process
memory under the intended workload. Payload pages are initialized before native delivery.
Startup still makes heap allocations, and a failure of the ordinary allocator can end the
process. The defaults are in the [tuning guide](../guide/tuning.md#defaults).

## Nonblocking admission and allocation scope

The callback leases with `ArrayQueue::pop` and submits with
`crossbeam_channel::Sender::try_send`. A full channel returns the event, and its lease is
recycled at once. The callback never waits, logs, grows a vector, or falls back to an
allocation. The parser takes events only with `try_recv`, and it reads the nonblocking
`len` and `is_empty`. It never registers a blocking receiver or selector. For the wake
rules, see [Parser wake and polling](ring-buffer.md#parser-wake-and-polling).

`crossbeam-channel` 0.5.15 checks an atomic waiter flag before it locks its notification
mutex. The production event path keeps the waiter list empty, so the check stays cheap. A
`recv`, `recv_timeout`, iteration, or selection on this channel would add a waiter and
remove that property. Neither the channel nor the pool gives a hard wait-free bound,
because atomic retries, spinning, yielding, scheduling, and preemption still apply.

The allocation guarantee covers admission and steady-state lease recycling while the pool
stays owned. The parser and encoder also reuse bounded storage. Cold schema loads, output
growth, error construction, and native TDH internals are outside the focused allocation
tests. When you measure a performance change, include the callback's bounded stack work
and the reference-count traffic.

Compiled schema plans are cached, as described in
[Supported schemas](single-pass-decoder.md#supported-schemas). An unsupported schema is
rejected and counted in `schema_rejected`. There is no per-event TDH fallback. A cache hit
makes no TDH call. A cold load makes two `TdhGetEventInformation` calls. Every decoded event
passes the shared checks on metadata, endpoint, timestamp, packet length, DNS direction, and
output size.

`Parser::parse` borrows the pooled event and borrows `PacketData` from its payload. The
private `Encoder::encode_with` in `src/dnstap.rs` copies the packet once, into the
initialized, exact packet range of the final protobuf output. Parsing releases the pool
lease before the TCP write, so the returned payload borrows only encoder output. Only a
cold metadata load reconstructs a native event view. The encoder keeps the identity, two
address vectors, Prost metadata, and the output storage. Prost encodes the metadata. The
nested message and the packet field are framed by hand, so the wire output has no
intermediate packet vector. Errors clear the output. Allocators can round up allocations,
and output storage can grow until it is warm. The
[single-pass decoder contract](single-pass-decoder.md) gives the interface and the
validation scope.

The copy from ETW callback storage into owned storage is required, so the design is not
zero-copy. Pooling removes repeated storage allocation. The design alone does not raise the
maximum throughput or allow a smaller safe queue. Warmed fixture tests do not show zero
allocation across the whole pipeline, higher throughput, or lower CPU use.

## Metrics schema 2

Schema 2 is the JSONL format of historical binaries. The TCP-only service has no JSONL
writer and no durable metrics export. See the
[instrumentation compatibility note](../README.md#instrumentation-compatibility-note).
The schema 2 counter meanings and identities are in
[Historical service loss and final ledgers](measurement-terms.md#historical-service-loss-and-final-ledgers).
The live log uses the names in [Log events and counters](../guide/monitoring.md).

Live samples read separate relaxed atomics, so they can briefly disagree with the
identities. Final samples follow native completion and the parser join. Historical metrics
and supervisor receiver references can keep the channel connected after a parser failure,
so `receiver_disconnected` does not detect every parser exit. The supervisor still detects
parser completion, ends capture, reports the error, and keeps the remaining backlog in the
final ledger. Treat every intake loss category as loss when you evaluate a zero-loss target.

The Python analyzers check the schema 1 and schema 2 fields, the schema 2 intake sum, and
final capture and parser closure. `etw_stats_errors` counts failed statistics queries
separately. Invalid final statistics cannot establish zero loss. The analysis checks are
in the [measurement review](../history/pooled-intake-review-plan.md).

## Validation scope

The local tests cover the ownership, wake, and allocation behavior described above. They do
not run real TDH extraction or a live ETW session. The audit findings and the remaining
checks are in the [audit and remaining validation plan](../history/pooled-intake-audit-plan.md).
Live DNS and ETW results are in the
[native VM performance review](../performance/pooled-vm-performance-review.md).

## Native and library contracts

The implementation follows the documented
[ETW callback context contract](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nc-evntrace-pevent_record_callback),
[EVENT_RECORD fields](https://learn.microsoft.com/en-us/windows/win32/api/evntcons/ns-evntcons-event_record),
[Rust copy preconditions](https://doc.rust-lang.org/std/ptr/fn.copy_nonoverlapping.html),
and [nonblocking channel submission](https://docs.rs/crossbeam-channel/0.5.15/crossbeam_channel/struct.Sender.html#method.try_send).
`Cargo.lock` selects `crossbeam-channel` 0.5.15 and `crossbeam-queue` 0.3.14. The
notification fast-path discussion depends on those implementations.
