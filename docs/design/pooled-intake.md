# Pooled ETW intake

> **Status:** Current · **Describes:** the code on branch `spool-encryption` as of 2026-11-05 · **Evidence dated:** not applicable (reference. Live results are in the historical campaign records.) · **Reviewed:** 2026-11-05

### Quick Start

```text
ETW record -> provider/ID check -> received -> lease slot -> copy -> try_send
           -> parser try_recv -> lease dropped, slot recycled
```

The callback waits, logs, and allocates nothing; every slot exists before capture. Stage
order and loss map: [Data path](data-path.md).

### Steps / Configuration

#### Subscription and event selection

- Level `0xFF` (a maximum: every level passes).
- `MatchAnyKeyword`: resolved `QUERY_RECEIVED | RESPONSE_SUCCESS | RESPONSE_FAILURE` bits; `MatchAllKeyword` zero.
- `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0` keeps keyword-zero events out.
- Event-ID filter: 256 (client query), 257 (successful response), 258 (failed response, such as SERVFAIL), from any process.

Every other ID, including 260 and 261 (recursive query and response), drops right after the
null-record check, before any counter, TLS, pool, copy, queue, or TDH step. The parser
rechecks the allowlist before TDH.

#### Ownership and callback lifecycle

- `PoolManager::with_progress` allocates and initializes every slot, then leaks the pool last, so it outlives every callback; a startup failure frees the slots.
- The `ProcessTrace` worker keeps producer state in thread-local storage (TLS): channel sender, pool, counters, parser handle, "enqueued" flag, callback-context identity; callbacks use `try_with` and `try_borrow_mut`.
- `native_etw_callback` copies `UserData` and each nonempty extended payload with `ptr::copy_nonoverlapping`, swapping descriptor pointers for offsets; extended payloads are 8-byte aligned.
- Before copying it rejects more than 16 extended items, more than 65,535 aggregate extended bytes, or a nonzero length with a null pointer.
- `native_buffer_callback` reads no pointer and always returns TRUE (FALSE ends `ProcessTrace`).
- A `RawEtwEvent` owns a `PooledBuffer` lease (one boxed slot, a `&'static` pool reference, no reference count). Dropping it on any thread returns the slot to its bounded free list; each class has exactly N private, non-cloneable slots and N list entries, and a returning lease holds its slot until pushed back.
- `with_record` builds a temporary typed descriptor array in the leased storage, repairs its pointers, and clears `UserContext`; the lease lives through the TDH calls.
- TLS is cleared after `ProcessTrace` returns ([shutdown](ring-buffer.md#completion-and-resource-ownership)).

#### Memory budget and configuration

| Option | Default | Range |
| --- | --- | --- |
| `--capacity` | 131072 | 1,024 to 524,288 |
| `--jumbo-capacity` | 64 | 0 to 256; 0 disables jumbo capture |

The pool accepts 1 to 524,288 slots. Each class has its own admission limit:

- **Regular:** 2,048 payload bytes, `--capacity` + 2 slots (one for the parser, one for the callback while the channel is full). Fits a padded payload plus 16 bytes per extended item.
- **Jumbo:** 131,448 bytes, `--jumbo-capacity` slots, capping jumbo records held across channel, callback, and parser. It fits 65,535 user bytes, 65,535 extended bytes, up to 16 seven-byte alignment gaps plus one before the item table, and the table (16 cells of 16 bytes: 8-byte header word, 8-byte offset).
- Per slot on Windows x64: 24 bytes (pointer, length, class flag). `RawEtwEvent` 104 bytes, free-list element 16, channel element 112. This provider sends no extended data.

```text
C = channel capacity, J = jumbo capacity
regular payloads   = (C + 2) x 2,048
jumbo payloads     = J x 131,448
slot metadata      = (C + 2 + J) x 24
free-list elements = (C + 2 + J) x 16
channel elements   = C x 112
```

| At defaults (C = 131,072, J = 64) | Bytes |
| --- | ---: |
| Regular payloads | 268,439,552 |
| Jumbo payloads | 8,412,672 |
| Slot metadata | 3,147,312 |
| Free-list elements | 2,098,208 |
| Channel elements | 14,680,064 |
| Total counted storage | 296,777,808 (283.03 MiB) |

At C = 524,288: 1,161,853,008 bytes (1,108.03 MiB) with J = 64, 1,187,098,704 (1,132.11 MiB)
with 256; regular payloads 1,073,745,920, channel elements 58,720,256. Totals leave out
allocator overhead, queue headers, worker stacks, native ETW buffers, TDH caches, protobuf
data, and output buffers. Payload pages are initialized before native delivery.

#### Nonblocking admission and allocation scope

- `ArrayQueue::pop` leases, `crossbeam_channel::Sender::try_send` submits; a full channel returns the event and recycles the lease.
- The parser uses only `try_recv`, `len`, and `is_empty`, so the waiter list stays empty and `crossbeam-channel` 0.5.15's atomic waiter-flag check stays cheap.
- Allocation-free: admission, steady-state lease recycling, and parser and encoder storage ([schema caching](single-pass-decoder.md#supported-schemas)).
- `Parser::parse` borrows the event and its `PacketData`; the private `Encoder::encode_with` (`src/dnstap.rs`) copies the packet once into its final protobuf range, so the lease goes before the TCP write. Only a cold metadata load rebuilds a native event view.
- The encoder reuses the identity, two address vectors, Prost metadata, and output; the nested message and packet field are framed by hand. Errors clear the output.

#### Native and library contracts

[ETW callback context](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nc-evntrace-pevent_record_callback),
[EVENT_RECORD fields](https://learn.microsoft.com/en-us/windows/win32/api/evntcons/ns-evntcons-event_record),
[Rust copy preconditions](https://doc.rust-lang.org/std/ptr/fn.copy_nonoverlapping.html),
[nonblocking `try_send`](https://docs.rs/crossbeam-channel/0.5.15/crossbeam_channel/struct.Sender.html#method.try_send).
`Cargo.lock` pins `crossbeam-channel` 0.5.15 and `crossbeam-queue` 0.3.14; the waiter-flag behavior depends on them.

### Common Gotchas

- **Keyword fallback:** mask `0x8000000000000000` if lookup fails, `QUERY_RECEIVED` or `RESPONSE_SUCCESS` is missing or malformed, or two names share a bit. A bad `RESPONSE_FAILURE` alone loses event 258.
- **Filtered events cost `dns.exe` CPU:** the keywords also enable 284, 285, 287, 288, and 597 to 599, built before the ID filter drops them.
- Bad or re-entered TLS counts in `callback_errors`; nothing panics.
- The API must supply readable memory; the checks can't prove a non-null pointer readable.
- A full or missing recycle list aborts. Exhaustion, overflow, and disconnection only count.
- At the default, a 65th jumbo record can drop with channel room. Raise it only with a measured workload and memory allowance.
- Counted storage is neither process memory nor a limit; measure under load. A startup heap failure can end the process.
- A `recv`, `recv_timeout`, iteration, or selection adds a waiter. Spinning, yielding, and preemption mean no hard wait-free bound.
- Allocation tests leave out cold schema loads, output growth, error construction, and TDH internals.
- The callback copy is required. Pooling alone raises neither throughput nor the safe minimum queue.
- Live samples read separate relaxed atomics and can briefly break the [identities](../guide/monitoring.md); final samples follow native completion and the parser join.
- Supervisor receiver references can hide a parser exit from `receiver_disconnected`; the supervisor still ends capture, reports it, and keeps the backlog in the final ledger.
- Zero loss means every intake loss category is zero; invalid final statistics (see `etw_stats_errors`) can't establish it.
- Local tests (no real TDH, no live ETW) cover ownership, wake, and allocation, and claim no throughput or CPU gain. See the [native VM performance review](../performance/pooled-vm-performance-review.md).
