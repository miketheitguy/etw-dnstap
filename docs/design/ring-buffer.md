# Ring capacity and shutdown

> **Status:** Current · **Describes:** `src/etw/` and the parser thread on branch `spool-encryption` as of 2026-11-05 · **Evidence dated:** October 28-29, 2026 for the capacity experiment, which is historical · **Reviewed:** 2026-11-05

This note covers the bounds of the event channel, how the parser wakes, how ETW delivers
events in bursts, and how the ETW threads shut down. It does not promise loss-free
capture at every traffic rate or event size.

For pool sizes and the memory formula, read [Pooled ETW intake](pooled-intake.md). For the
shutdown order of all five stages, read [Startup and shutdown order](data-path.md#startup-and-shutdown-order).

## Capacity and memory

The event channel holds up to `--capacity` events. The default is 131,072, and the flag
accepts 1,024 to 524,288. When the channel is full, the newest event is dropped and counted
in `ring_dropped` and `intake_dropped`. The callback never waits or allocates.

The default buffer settings were validated at about 20,000 events per second (10,000
queries per second). The defaults and the settings tested at lower rates are in the
[tuning guide](../guide/tuning.md#defaults). Read that section before you choose
`--capacity`, `--etw-flush-seconds`, or the ETW buffer options.

Earlier builds used smaller channel defaults. The
[ring capacity experiment](../history/ring-capacity-experiment.md) records the 8,192 default
and its allocation-based queue. Its results do not validate the pooled implementation.

The pool sizes, the jumbo limit, and the counted storage (296,777,808 bytes at the defaults)
are in [Memory budget and configuration](pooled-intake.md#memory-budget-and-configuration).
Those totals exclude allocator overhead, native ETW buffers, stacks, and output buffers.

## Parser wake and polling

The parser takes events only with `try_recv`. It wakes in one of these ways:

- The buffer callback wakes it once for each delivered ETW buffer that enqueued events.
- After it receives data, it polls for up to 200 µs before it parks.
- Otherwise it parks with a timed backoff from 2 ms to 20 ms.

The per-event callback does not signal the parser. It sets a flag on its own thread. ETW
calls the buffer callback on the consumer thread after each delivered buffer. That callback
wakes the parser when the flag is set, so a burst starts to drain within microseconds. The
parser keeps one wake token, so no wake is lost. See the
[ETW buffer wake validation](../history/buffer-wake-validation.md). The channel has no
blocking receiver, as [Nonblocking admission](pooled-intake.md#nonblocking-admission-and-allocation-scope)
explains.

## Delivery batches

The service does not ask ETW to flush the session. ETW delivers real-time events about once
per `--etw-flush-seconds` (default 1), as one batch. The buffer callback wakes the parser
for each delivered buffer, so the parser drains the batch while it arrives.

Size the channel for a batch. One batch holds up to about 2 × queries per second ×
`--etw-flush-seconds` events, because each query makes two events. Keep `--capacity` well
above that size. The peak queue is usually far below that size, because the parser drains
during delivery. Lab results at high rates are in the
[flush removal validation](../history/flush-removal-validation.md) record.

## Removed periodic flush

Earlier builds had an opt-in periodic flush. The supervisor asked ETW to flush the session
at a set interval, and it woke the parser after each flush. The buffer wake in commit
`7f91264` made the flush unnecessary, so the maintainer removed it. The current build does
not flush. The results for the old flush are in the
[flush removal validation](../history/flush-removal-validation.md) record, and they do not
apply to the current build.

## Completion and resource ownership

Orderly shutdown stops capture and joins the native `ProcessTrace` worker. Only then does
it publish `producer_done` with release ordering. No callback can enqueue after that point.
The supervisor then unparks and joins the parser. A startup failure publishes completion
the same way.

After an empty `try_recv`, the parser reads completion with acquire ordering. If completion
is set, the parser calls `try_recv` again until the channel is empty. This final drain
catches an event sent between the empty receive and the completion read. The post-data
linger also ends when the parser sees completion.

The 10-second fail-stop watchdog covers native shutdown and parser completion. Callback
state stays owned until the native worker ends. During that window, native shutdown waits up
to 5 seconds (`WORKER_EXIT_WAIT`) for `ProcessTrace` to return after STOP. If it does not
return, `CloseTrace` forces the close.

Unit tests cover the capacity bounds, the loss when the channel is full, a final enqueue
followed by a joined producer, empty completion after a startup failure, and output errors.
Synthetic tests do not establish live DNS throughput or the token configuration of an
installed service.
