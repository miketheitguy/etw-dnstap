# Ring capacity and shutdown

> **Status:** Current · **Describes:** `src/etw/` and the parser thread on branch `spool-encryption` as of 2026-11-05 · **Evidence dated:** 2026-11-01 for the buffer wake and flush validations · **Reviewed:** 2026-11-05

This note covers the event channel bound, the parser wake, the ETW delivery batches, and
the shutdown of the ETW threads. Pool sizes and the memory formula are in
[Memory budget and configuration](pooled-intake.md#memory-budget-and-configuration). The
shutdown order of all five stages is in
[Startup and shutdown order](data-path.md#startup-and-shutdown-order).

## Capacity

The event channel holds up to `--capacity` events. When it is full, the callback drops the
newest event and counts it in `ring_dropped` and `intake_dropped`. Read
[Defaults](../guide/tuning.md#defaults) before you change `--capacity`,
`--etw-flush-seconds`, or the ETW buffer options.

## Parser wake and polling

The parser takes events only with `try_recv`. It wakes in one of these ways:

- The buffer callback wakes it once for each delivered ETW buffer that enqueued events.
- After it receives data, it polls for up to 200 µs before it parks.
- Otherwise it parks with a timed backoff from 2 ms to 20 ms.

The per-event callback only sets an "enqueued" flag on its own thread. ETW calls the
buffer callback on the same thread after each delivered buffer. When the flag is set, the
buffer callback clears it, counts `parser_wake_requests`, and wakes the parser, so a burst
starts to drain within microseconds. If TLS is borrowed or missing, the buffer callback
does nothing, and the flag stays set for the next buffer. The parser keeps one wake token,
so no wake is lost. The channel has no blocking receiver, as
[Nonblocking admission](pooled-intake.md#nonblocking-admission-and-allocation-scope)
requires.

## Delivery batches

ETW delivers real-time events about once per `--etw-flush-seconds` (default 1), as one
batch. The parser drains the batch while it arrives, because the buffer callback wakes it
for each delivered buffer.

Size the channel for a batch. One batch holds up to about 2 × queries per second ×
`--etw-flush-seconds` events, because each query makes two events. Keep `--capacity` well
above that size. The peak queue is usually far lower, because the parser drains during
delivery.

## Completion and resource ownership

Orderly shutdown stops capture and joins the native `ProcessTrace` worker. Only then does
it publish `producer_done` with release ordering, so no callback can enqueue after that
point. The supervisor then wakes and joins the parser. A startup failure publishes
completion the same way.

After an empty `try_recv`, the parser reads completion with acquire ordering. If completion
is set, the parser calls `try_recv` again until the channel is empty. This final drain
catches an event sent between the empty receive and the completion read. Completion also
ends the post-data linger.

The 10-second fail-stop watchdog covers native shutdown and parser completion. Callback
state stays owned until the native worker ends. Native shutdown waits up to 5 seconds
(`WORKER_EXIT_WAIT`) for `ProcessTrace` to return after STOP, then forces the close with
`CloseTrace`.

Unit tests cover the capacity bounds, the full-channel loss, a final enqueue followed by a
joined producer, empty completion after a startup failure, and output errors. Lab evidence
is in the [ETW buffer wake validation](../history/buffer-wake-validation.md) and the
[delivery batch results](../history/flush-removal-validation.md).
