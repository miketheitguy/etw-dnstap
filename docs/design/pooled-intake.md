# Pooled ETW intake

> **Status:** Current · **Describes:** the code on branch `spool-encryption` as of 2026-11-05 · **Evidence dated:** not applicable (reference. Live results are in the historical campaign records.) · **Reviewed:** 2026-11-05

This reference covers how the Event Tracing for Windows (ETW) intake selects events, owns
memory, and admits events. The stage order and the loss map are in [Data path](data-path.md).

## Subscription and event selection

The session enables the DNS Server provider with these settings:

- Level `0xFF`. The level is a maximum, so every level passes.
- `MatchAnyKeyword` set to the resolved `QUERY_RECEIVED | RESPONSE_SUCCESS |
  RESPONSE_FAILURE` bits. The mask falls back to `0x8000000000000000` when the keyword
  lookup fails, when `QUERY_RECEIVED` or `RESPONSE_SUCCESS` is missing or malformed, or
  when any two of the three names share a bit. If only `RESPONSE_FAILURE` is missing or
  malformed, the mask keeps the other two bits, and the service does not capture event 258.
- `MatchAllKeyword` set to zero.
- `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0`, which keeps keyword-zero events out.
- An event-ID filter that admits only 256, 257, and 258.

The keywords also enable events 284, 285, 287, 288, and 597 to 599. The event-ID filter
drops them after the provider builds them, so they cost `dns.exe` CPU but never reach the
service.

The callback accepts events 256 (client query), 257 (successful client response), and 258
(failed client response, such as SERVFAIL) from any process. It drops every other ID,
including 260 (recursive query) and 261 (recursive response). It checks the provider and
the event ID right after the null-record check, before any counter, TLS, pool, copy,
queue, or TDH step, so a dropped record counts nothing. The parser repeats the event
allowlist before any TDH access.

## Ownership and callback lifecycle

`PoolManager` allocates and initializes every slot before capture starts. The
`ProcessTrace` worker installs a producer state in thread-local storage (TLS), and the
native callbacks run on that worker. The producer state holds the channel sender, a pool
reference, the capture counters, the parser thread handle, an "enqueued" flag, and the
native callback-context identity. Each callback uses `try_with` and `try_borrow_mut`. If
TLS is missing, wrong, or re-entered, the event counts in `callback_errors`, and nothing
panics.

**Event callback.** `native_etw_callback` checks the provider and event ID, counts
`received`, checks lengths, leases a slot, and copies `UserData` and each nonempty
extended-data payload with `ptr::copy_nonoverlapping`. It keeps the event header, the
buffer context, the descriptor metadata, and the lengths, and it replaces the native
descriptor pointers with offsets.
After a successful `try_send`, it sets the "enqueued" flag.

Before it copies, the callback rejects a record that has more than 16 extended items,
more than 65,535 aggregate extended bytes, or a nonzero length with a null pointer. These
checks cannot prove that a non-null address is readable, so the API must supply readable
memory.

**Buffer callback.** `ProcessTrace` calls `native_buffer_callback` on the same worker
after each delivered buffer. It reads no pointer and always returns TRUE, because FALSE
would end `ProcessTrace`. It counts `etw_buffers_delivered` and applies the
[wake rule](ring-buffer.md#parser-wake-and-polling).

**Leases.** A `RawEtwEvent` owns a `PooledBuffer` lease. The lease holds one boxed slot
and a `&'static` reference to the pool. The pool has no reference count, so a lease adds
no atomic operation, and a channel move allocates no ownership object. The parser uses the
scoped `with_record` helper to build a temporary typed descriptor array in the leased
storage. The helper repairs the array's pointers and clears `UserContext`. The parser
keeps the lease during the synchronous TDH calls. Each extended payload starts on an
8-byte boundary. No native callback pointer survives in the queued event.

Dropping a lease returns its slot to the matching bounded free list, from any thread.

**Lifetime.** `PoolManager::with_progress` leaks the pool once, after it allocates every
slot. The pool therefore lives for the rest of the process, and no callback can outlive
it. If startup fails, the slots already allocated are freed, because the leak happens
last.

**Invariant.** Each class has exactly N private, non-cloneable slots and N free-list
entries. A returning lease holds its slot outside the list until the slot is pushed back,
so concurrent returns cannot fill the list early. A full or missing recycle list is an
internal invariant failure, and it aborts the process. Pool exhaustion, channel overflow,
and receiver disconnection only count and return.

Shutdown follows [Completion and resource ownership](ring-buffer.md#completion-and-resource-ownership).
TLS is cleared explicitly after `ProcessTrace` returns.

## Memory budget and configuration

The defaults are `--capacity 131072` and `--jumbo-capacity 64`. `--capacity` accepts
1,024 to 524,288, and the pool accepts 1 to 524,288 slots. `--jumbo-capacity` accepts 0 to
256, and 0 disables jumbo capture. Storage has two classes with independent admission
limits:

- **Regular:** 2,048 bytes of payload per slot, in `--capacity` + 2 slots. A record uses a
  regular slot when its padded payload, plus 16 bytes for each extended item, fits in 2 KiB.
- **Jumbo:** 131,448 bytes per slot, in `--jumbo-capacity` slots. A record that does not
  fit a regular slot uses a jumbo slot.

Each class serves only its own records, and no slot grows after startup. The two extra
regular slots cover one event held by the parser and one event in the callback while the
channel is full. The jumbo budget caps the jumbo records held at once across the channel,
the callback, and the parser. At the default, a 65th jumbo record can be dropped even when
the channel has room. Change this budget only with a measured workload and an explicit
memory allowance.

The jumbo size covers the largest accepted record: 65,535 user bytes and 65,535 bytes of
extended data. It also covers up to 16 seven-byte alignment gaps, one more gap before the
item table, and the item table itself (16 cells of 16 bytes). Slot selection counts this
padding and table, so each extended item costs 16 bytes of a regular slot.

On Windows x64, each slot has a 24-byte allocation for the payload pointer, the payload
length, and the class flag. The item table sits at the end of the payload storage, 16 bytes
per item (an 8-byte header word and an 8-byte payload offset). Records from this provider
have no extended data, so no table. `RawEtwEvent` is 104 bytes, a free-list element is 16
bytes, and a channel element is 112 bytes.

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

The totals exclude allocator overhead, queue headers, worker stacks, native ETW buffers,
TDH caches, protobuf data, and output buffers. They are neither a process-memory measurement nor
a memory limit, so measure process memory under the intended workload. Payload pages are initialized before
native delivery. A heap allocation failure at startup can end the process.

## Nonblocking admission and allocation scope

The callback leases with `ArrayQueue::pop` and submits with
`crossbeam_channel::Sender::try_send`. A full channel returns the event, and its lease is
recycled at once. The callback must not wait, log, grow a vector, or allocate. The parser
takes events only with `try_recv`, and it reads the nonblocking `len` and `is_empty`. It
registers no blocking receiver or selector.

`crossbeam-channel` 0.5.15 checks an atomic waiter flag before it locks its notification
mutex. The event path keeps the waiter list empty, so the check stays cheap. A `recv`,
`recv_timeout`, iteration, or selection on this channel would add a waiter and lose that
property. Atomic retries, spinning, yielding, scheduling, and preemption still apply, so
neither the channel nor the pool has a hard wait-free bound.

The allocation guarantee covers admission and steady-state lease recycling. The parser
and encoder also reuse bounded storage. Cold schema loads, output growth, error
construction, and native TDH internals fall outside the allocation tests. Schema caching
and rejection follow [Supported schemas](single-pass-decoder.md#supported-schemas).

`Parser::parse` borrows the pooled event and borrows `PacketData` from its payload. The
private `Encoder::encode_with` in `src/dnstap.rs` copies the packet once, into its exact
range of the final protobuf output, so parsing can release the lease before the TCP write.
Only a cold metadata load rebuilds a native event view. The encoder reuses the identity,
two address vectors, Prost metadata, and the output storage. Prost encodes the metadata,
and the nested message and the packet field are framed by hand. Errors clear the output.

The copy from ETW callback storage into owned storage is required. Pooling removes
repeated allocation; it alone neither raises the maximum throughput nor allows a smaller
safe queue.

## Loss accounting

The counter names and identities are in [Log events and counters](../guide/monitoring.md).

Live samples read separate relaxed atomics, so they can briefly disagree with the
identities. Final samples follow native completion and the parser join. Supervisor
receiver references can keep the channel connected after a parser failure, so
`receiver_disconnected` misses some parser exits. The supervisor still detects the parser
exit, ends capture, reports the error, and keeps the backlog in the final ledger. Treat
every intake loss category as loss when you evaluate a zero-loss target.

`etw_stats_errors` counts failed statistics queries. Invalid final statistics cannot
establish zero loss.

## Validation scope

The local tests cover the ownership, wake, and allocation behavior above, without real
TDH extraction or a live ETW session. They show neither zero allocation across the whole
pipeline nor a throughput or CPU gain. Live DNS and ETW results are in the
[native VM performance review](../performance/pooled-vm-performance-review.md).

## Native and library contracts

The implementation follows the documented
[ETW callback context contract](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nc-evntrace-pevent_record_callback),
[EVENT_RECORD fields](https://learn.microsoft.com/en-us/windows/win32/api/evntcons/ns-evntcons-event_record),
[Rust copy preconditions](https://doc.rust-lang.org/std/ptr/fn.copy_nonoverlapping.html),
and [nonblocking channel submission](https://docs.rs/crossbeam-channel/0.5.15/crossbeam_channel/struct.Sender.html#method.try_send).
`Cargo.lock` selects `crossbeam-channel` 0.5.15 and `crossbeam-queue` 0.3.14. The
waiter-flag behavior above depends on those versions.
