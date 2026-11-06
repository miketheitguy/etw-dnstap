# Ring capacity and shutdown

> **Status:** Current · **Describes:** `src/etw/` and the parser thread on `main` at commit `592a040` as of 2026-11-05 · **Evidence dated:** 2026-11-01 for the buffer wake and flush validations · **Reviewed:** 2026-11-05

### Quick Start

```text
--capacity  well above  2 x queries per second x --etw-flush-seconds
```

Sizes: [Memory budget and configuration](pooled-intake.md#memory-budget-and-configuration).
Full shutdown: [Startup and shutdown order](data-path.md#startup-and-shutdown-order).
Buffer analogy, with an animation: [Buffers as a watershed](buffer-watershed.md).

### Steps / Configuration

#### Parser wake and polling

The parser takes events only with `try_recv`. It wakes:

- once per delivered ETW buffer that enqueued events, from the buffer callback;
- by polling up to 200 µs after it receives data, then parking;
- from a timed park, backing off from 2 ms to 20 ms.

The event callback only sets an "enqueued" flag. ETW calls the buffer callback on the same
thread after each buffer; if the flag is set, it clears it, counts `parser_wake_requests`,
and wakes the parser, so a burst drains within microseconds. One wake token means no lost
wake. No blocking receiver: [Nonblocking admission](pooled-intake.md#nonblocking-admission-and-allocation-scope).

#### Delivery batches

ETW delivers one batch about once per `--etw-flush-seconds` (default 1). Each query makes
two events, so a batch holds up to about 2 × queries per second × `--etw-flush-seconds`
events. The peak queue is usually far lower, because the parser drains during delivery.

#### Completion and resource ownership

1. Shutdown stops capture and joins the native `ProcessTrace` worker.
2. Then it publishes `producer_done` (release ordering), so no callback can enqueue after.
   A startup failure publishes completion the same way.
3. The supervisor wakes and joins the parser.
4. After an empty `try_recv`, the parser reads completion (acquire ordering). If set, it
   drains with `try_recv` until empty, catching an event sent between the two reads.
   Completion also ends the post-data linger.

Callback state stays owned until the native worker ends.

### Common Gotchas

- A full channel drops the newest event (`ring_dropped`, `intake_dropped`). Read
  [Defaults](../guide/tuning.md#defaults) before changing `--capacity`, `--etw-flush-seconds`,
  or the ETW buffer options.
- Borrowed or missing TLS makes the buffer callback do nothing; the flag waits for the next buffer.
- After STOP, native shutdown waits up to 5 seconds (`WORKER_EXIT_WAIT`), then forces
  `CloseTrace`. A 10-second fail-stop watchdog covers native shutdown and parser completion.
- Unit tests cover capacity bounds, full-channel loss, a final enqueue then a joined producer,
  empty completion after startup failure, and output errors. Lab evidence: [buffer wake](../history/buffer-wake-validation.md),
  [delivery batches](../history/flush-removal-validation.md).
