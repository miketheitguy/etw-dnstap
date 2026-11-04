# Ring capacity and shutdown

> **Status:** Current · **Describes:** `src/etw/` on the `plan/msi-installer` branch (the schema retry was added after `7ec888d`) · **Evidence dated:** October 28-29, 2026 for the experiment, which is historical · **Reviewed:** 2026-11-03

This note explains the queue bounds and the shutdown contract. It does not
guarantee loss-free capture at every traffic rate or event size.

## Capacity and memory

The defaults and the settings tested at lower rates are in the
[tuning guide](../guide/tuning.md#defaults). Read that section before you
choose `--capacity`, `--etw-flush-seconds`, or the ETW buffer options.

### Enqueue

The user-mode ETW callback leases preallocated storage and copies each accepted event.
It calls `crossbeam_channel::Sender::try_send` on a bounded event channel. It never
waits or allocates. A full channel drops the newest event and increments `ring_dropped`
and `intake_dropped`. The default capacity is 131,072 events. It was 65,536, and 24,576 before that, until the
defaults moved to 200,000 queries per second, and 8192 until 2026-11-01.
`--capacity` accepts 1,024 to 262,144.

### Parser wake and polling

The parser takes events only with `try_recv`. It wakes in these ways:

- The ETW buffer callback wakes it once for each delivered buffer that enqueued
  events.
- After data, it polls for up to 200 µs before it parks.
- Otherwise it parks with a timed backoff from 2 ms to 20 ms.

The per-event callback does not signal the parser. It sets a same-thread flag.
`ProcessTrace` calls the ETW buffer callback on the consumer thread after each
delivered buffer. That callback unparks the parser when the flag is set, so a burst
starts to drain within microseconds. See
[ETW buffer wake validation](../history/buffer-wake-validation.md). The channel has no
blocking receiver, as [Pooled ETW intake](pooled-intake.md#nonblocking-admission-and-allocation-scope)
explains.

### Delivery batches

The service does not ask ETW to flush the session. ETW delivers real-time events about
once per `--etw-flush-seconds`, even with full buffers, as one batch. The buffer
callback wakes the parser for each delivered buffer, so the parser drains the batch
while it arrives.

Capacity guidance: one batch holds up to about 2 x queries/s x `--etw-flush-seconds`
events, because each query makes 2 events. Keep `--capacity` well above that size. The
peak queue is usually far below that size, because the parser drains during delivery.
Lab results for high rates are in the
[flush removal validation](../history/flush-removal-validation.md) record.

### Removed periodic flush

Earlier builds had an opt-in periodic flush. The supervisor thread asked ETW to flush
the session at a set interval and woke the parser after each flush. The buffer wake in
commit `7f91264` made the flush unnecessary, so the maintainer removed it. The current
build does not flush. The measured results for the old flush are in
[flush removal validation](../history/flush-removal-validation.md). They do not apply to
the current build.

### Pools and memory budget

The regular pool has `capacity + 2` slots. The jumbo pool has `--jumbo-capacity` slots
(0 to 256, default 64). The two classes have independent admission limits, so only 64
simultaneous jumbo leases fit at the defaults, even if the channel has spare entries.
Slot return uses class-specific `ArrayQueue` free lists. No pool grows during capture.

At the defaults (131,072 events, 64 jumbo slots), counted storage is 296,777,808 bytes
(283.03 MiB). The count includes payload, slot metadata, free lists, and channel
elements. It excludes allocator overhead, native ETW buffers, stacks, TDH caches,
protobuf data, and output buffers. The
[slot sizes and formulas](pooled-intake.md#memory-budget-and-configuration) are in
Pooled ETW intake. Measure the process memory and every intake loss category under the
intended workload.

## Historical capacity experiment

The earlier experiment used an allocation-based `ArrayQueue`. It explains the former
8192 default. Its results are in
[Ring capacity experiment (historical)](../history/ring-capacity-experiment.md). They do
not validate the pooled implementation or its jumbo limit.

## Completion and resource ownership

Orderly shutdown stops capture and joins the native `ProcessTrace` worker before it
publishes `producer_done` with release ordering. No callback can enqueue after that
publication. The supervisor then unparks and joins the parser. A startup failure also
publishes completion and unparks the parser.

After an empty `try_recv`, the parser reads completion with acquire ordering. If it
observes completion, it calls `try_recv` again until the channel is empty. This final
drain catches an event sent between the earlier empty receive and the completion read.
The post-data linger also ends when it reads completion.

The 10-second fail-stop watchdog covers native shutdown and parser completion. Callback
state stays owned until the native worker ends. In that window, native shutdown waits up
to 5 seconds for `ProcessTrace` to return after STOP before `CloseTrace` forces it.

Unit tests cover the capacity bounds and loss when the ring is full. They also cover a
final enqueue followed by joined producer completion, empty completion after a startup
failure, and output errors. Synthetic tests do not establish live DNS throughput or an
installed service's token configuration.
