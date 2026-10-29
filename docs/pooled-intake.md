# Pooled ETW intake

This reference helps maintainers review the ownership, memory budget, and loss
accounting of the pooled Event Tracing for Windows (ETW) intake. It describes the
local implementation and its checks. It does not establish production throughput
or replace validation of the installed service.

## Ownership and callback lifecycle

`PoolManager` allocates and initializes every slot before capture starts. The
`ProcessTrace` worker installs a producer in thread-local storage (TLS) before it
enters the native API. The native callback executes in user mode on that worker;
it is not kernel-mode application code. TLS contains the bounded channel sender, a pool reference,
and the stable native callback-context identity. The callback uses `try_with` and
`try_borrow_mut`; missing, wrong, or reentrant TLS increments `callback_errors`
without panicking.

`native_etw_callback` filters provider and event IDs, counts each supported
record, validates lengths, leases storage, and calls `ptr::copy_nonoverlapping`
before returning. It copies `UserData` and every nonempty extended-data payload.
It preserves the event header, buffer context, descriptor metadata, and lengths.
It clears native descriptor pointers and stores offsets instead. A native record
with more than 64 extended items, more than 65,535 aggregate extended bytes, or a
nonzero length with a null pointer is rejected before payload copying. The native
API must still supply readable memory; these checks cannot validate arbitrary
non-null addresses.

`RawEtwEvent` owns a `PooledBuffer` lease. Each lease owns one boxed slot and a
strong pool reference; moving it through the channel allocates no ownership
object. Cloning the existing `Arc` changes its reference count. The parser builds
a temporary typed descriptor array through the scoped `with_record` helper,
repairs its pointers into the leased aligned storage, clears `UserContext`, and
retains the lease throughout synchronous TDH calls. The parser and reconstruction
tests use this same helper. Each extended payload starts on an 8-byte boundary. No native callback
pointer survives in the queued event.

Dropping the lease returns its slot to the corresponding bounded free list. The
parser can return a slot from another thread, and queued leases keep the pool
alive after the native worker exits. The native callback context independently
retains the pool throughout `ProcessTrace`, so rejection or channel failure
cannot destroy the last pool owner inside a callback. Final pool destruction can
free memory during shutdown after callbacks end.

There are exactly N private, non-cloneable slots in a class and N free-list
entries. A returning lease owns one slot outside the list; legitimate concurrent
returns therefore cannot fill the list before that slot returns. A full or
missing recycle list is an internal invariant failure and aborts the process.
Ordinary pool exhaustion, channel overflow, and receiver disconnection only
increment counters and return.

Capture retains the existing SCM-only entry point, native stop and join order,
10-second shutdown watchdog, and parser completion protocol. After native join,
the supervisor publishes `producer_done` with release ordering and unparks the
parser. The parser observes completion with acquire ordering and retries the
channel until all final queued records have drained. TLS is cleared explicitly
after `ProcessTrace` returns.

## Memory budget and configuration

The channel default stays at `--capacity 8192`, with the existing range of
1 through 16,384. Storage has two independent classes:

| Class | Payload storage per slot | Slots | Admission |
| --- | ---: | ---: | --- |
| Regular | 2,048 bytes | Channel capacity + 2 | Complete padded payload fits 2 KiB |
| Jumbo | 131,584 bytes | `--jumbo-capacity`, default 64 | Complete padded payload exceeds 2 KiB |

`--jumbo-capacity` accepts 0 through 256. Zero disables jumbo capture. Regular
records do not borrow jumbo slots, and jumbo records cannot use regular slots.
No slot grows after startup. The regular reserve covers a full channel, one
parser-held event, and one callback event. The default jumbo budget allows only
64 simultaneously owned jumbo records across the channel, callback, and parser;
the 65th can be dropped even when the channel has space. Adjust this budget only
with a measured workload and an explicit memory allowance.

The jumbo size covers the previous accepted maximum of 65,535 user bytes plus
65,535 aggregate extended bytes and up to 64 seven-byte alignment gaps. Slot
selection includes alignment padding, rather than only DNS packet length.

On the configured Windows x64 target, each slot also has a 1,560-byte allocation
containing its payload pointer, 64 typed extended descriptors, 64 offsets, and
class metadata. `RawEtwEvent` occupies 104 bytes. With the locked queue versions,
free-list elements occupy 16 bytes and channel elements occupy 112 bytes. The
following table counts requested element and payload storage:

| Allocation contents | Default 8,192 / 64 | Maximum 16,384 / 256 |
| --- | ---: | ---: |
| Regular payloads | 16,781,312 bytes | 33,558,528 bytes |
| Jumbo payloads | 8,421,376 bytes | 33,685,504 bytes |
| Slot metadata allocations | 12,882,480 bytes | 25,961,520 bytes |
| Free-list elements | 132,128 bytes | 266,272 bytes |
| Channel elements | 917,504 bytes | 1,835,008 bytes |
| Total counted storage | 39,134,800 bytes (37.32 MiB) | 95,306,832 bytes (90.89 MiB) |

These totals exclude allocator rounding, allocator metadata, the small queue and
`Arc` headers, worker stacks, native ETW buffers, TDH caches, protobuf data, and
output buffers. They are not process-private-memory measurements or total-memory
limits. Payload pages are initialized before native delivery. Startup still
performs heap allocations; failure of the ordinary allocation machinery can
terminate the process.

## Nonblocking admission and allocation scope

The callback leases with `ArrayQueue::pop` and submits with
`crossbeam_channel::Sender::try_send`. A full channel returns ownership of the
event, which immediately recycles its lease. There is no capacity wait, callback
logging, vector growth, or fallback allocation.

The event-channel receiver uses only `try_recv` and retains the previous 2 ms
empty-channel park. It never registers a blocking receiver or selector.
`crossbeam-channel` 0.5.15 checks an atomic empty-waiter flag before locking its
notification mutex; the production event path keeps that waiter list empty.
Adding `recv`, `recv_timeout`, iteration, or selection on this channel would
invalidate that property. The channel API is nonblocking, but neither it nor the
pool promises a hard wait-free execution bound: atomic retries, spinning,
yielding, scheduling, and preemption still apply. Metrics receivers only inspect
channel length.

The allocation guarantee covers callback admission and steady-state lease
recycling while the pool remains owned. Parser TDH extraction, schema caching,
protobuf encoding, and output retain their existing allocations. The callback
also has bounded stack work for up to 64 descriptors and offsets, and the parser
uses a fixed descriptor array. This fixed work and reference-count traffic must
be included when measuring any performance change.

The five parser property reads use six fixed, nul-terminated UTF-16 property
names, selecting either `Source` or `Destination` for the peer. Static names
remove five per-event name-vector constructions. This does not change TDH
extraction: a schema-cache hit still uses five `TdhGetPropertySize` calls and
five `TdhGetProperty` calls, with property result buffers allocated by the parser.

The copy from native ETW callback storage into owned storage remains required.
Pooling removes repeated storage allocation; it is not literal zero-copy.
Neither a higher maximum throughput nor a smaller safe queue follows from this
design alone. The preserved ArrayQueue production baseline passed its stated
1,000 QPS, 300-second authoritative and recursive runs with zero ring loss. That
evidence applies to the baseline binary, not to this candidate or its separate
jumbo budget. No installed VM or capacity default changes as part of this work.

## Metrics schema 2

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

At quiescence, the accounting identities are:

```text
intake_dropped = ring_dropped + pool_exhausted + receiver_disconnected
received = intake_dropped + callback_errors + dequeued + queue_backlog
dequeued = encoded + ignored + rejected + output_limit_dropped + output_failed_events
```

Intermediate samples read separate relaxed atomics and can briefly differ from
these identities. Final samples follow native completion and parser join.
Metrics and supervisor receiver references can keep the channel connected after
a parser failure; `receiver_disconnected` therefore does not detect every parser
exit. The supervisor still detects parser completion, stops capture, reports the
error, and retains the remaining backlog in the final ledger. Treat every intake
loss category as loss when evaluating a zero-loss target. The Python analyzers
validate schema 1 and 2 fields, the schema 2 intake sum, and final capture/parser
closure through one shared normalizer. Schema 1 retains its ring-only intake
meaning. Intermediate phase samples need not close, but final samples must.
`etw_stats_errors` counts failed statistics queries separately; a valid final
query with zero cumulative ETW loss can pass despite transient query failures.
Invalid final statistics cannot establish zero loss. See the
[measurement review](pooled-intake-review-plan.md) for analysis checks and
preserved-data comparisons.

## Validation scope

Focused tests exercise nonempty user and extended-data copying, source-buffer
mutation and destruction, maximum padded extended layouts, disabled jumbo
capture, pool exhaustion, channel overflow, receiver disconnection, wrong and
reentrant TLS, cross-thread recycling, queued ownership after native-owner exit,
and final queue drain. Concurrent pool workers check unique active addresses,
independent sentinel data, and recovery of every slot.

The parser's shared native-view helper is exercised with zero and 64 extended
items after native source storage ends. Tests inspect copied bytes, header and
buffer context, descriptor metadata, repaired pointer alignment and bounds,
null context, lease exclusivity, and a dirty slot reused for an empty record.
Static-name descriptor checks cover all six terminated spellings and measure
their allocation scope. These tests do not execute real TDH extraction.

A test-only Rust global allocator counts allocations, reallocations, and
deallocations separately during callback success, channel-full, regular and jumbo
exhaustion, jumbo success, disconnection, malformed metadata, and TLS failures.
It also measures an isolated parser-thread lease return while the pool remains
owned. Setup, TLS installation, and final pool destruction occur outside these
measurement scopes. This does not instrument arbitrary direct allocations in
Windows DLLs or prove the behavior of a real installed ETW session.

The ignored `synthetic_pooled_intake_benchmark` test is an optional local
callback/channel/lease harness. It excludes native ETW delivery, TDH parsing,
protobuf encoding, and output. The original `perf_windows` benchmark matrix and
`perf_review` security review remain stopped at the user's request. Their draft
artifacts are preserved; that matrix has not run. A separate, fresh read-only
Astra audit reviewed the source and analysis after implementation. See the
[audit and remaining validation plan](pooled-intake-audit-plan.md) for its
findings, follow-up checks, exact artifact identities, and final review status.
The follow-up local [receipt](../datasets/pooled-intake-local/audit-fixes-v1/receipt.json)
records 45 passing tests and one ignored manual benchmark in development,
release-unwind, and ASan configurations. It preserves the initial 42-test
candidate separately and does not claim native TDH or VM validation.

## Native and library contracts

The implementation follows the documented
[ETW callback context contract](https://learn.microsoft.com/en-us/windows/win32/api/evntrace/nc-evntrace-pevent_record_callback),
[EVENT_RECORD fields](https://learn.microsoft.com/en-us/windows/win32/api/evntcons/ns-evntcons-event_record),
[Rust copy preconditions](https://doc.rust-lang.org/std/ptr/fn.copy_nonoverlapping.html),
and [nonblocking channel submission](https://docs.rs/crossbeam-channel/0.5.15/crossbeam_channel/struct.Sender.html#method.try_send).
`Cargo.lock` selects `crossbeam-channel` 0.5.15 and `crossbeam-queue` 0.3.14;
the notification fast-path discussion depends on those implementations.
