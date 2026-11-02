# Ring capacity and shutdown

> **Status:** Current (the queue and shutdown reference. Its capacity experiment is historical.) · **Describes:** `main` at commit `dde7188` with the removal of the explicit ETW flush · **Evidence dated:** October 28-29, 2026 for the experiment · **Reviewed:** 2026-11-01

This note is for maintainers, and for administrators who choose a capture
capacity. It explains the queue bounds and the shutdown contract. It describes
the implementation limits and the capacity experiment. It does not guarantee
loss-free capture at every traffic rate or event size.

## Capacity and memory

The defaults, their low-rate limits, and the recommended high-rate settings are
stated once, in the [configuration reference](../guide/configuration.md#low-rate-defaults).
Read that section before you choose `--capacity`, `--etw-flush-seconds`, or the
ETW buffer options.

### Enqueue

The user-mode ETW callback leases preallocated storage and copies each accepted
event. It then calls `crossbeam_channel::Sender::try_send` on a bounded event
channel. It does not wait for capacity or allocate callback storage. A full
channel drops the newest event and increments `ring_dropped`. The default
capacity is 8192 events. `--capacity` accepts 1,024–262,144.

### Parser wake and polling

The parser takes events only with `try_recv`. It wakes in these ways:

- The ETW buffer callback wakes it once for each delivered buffer that enqueued
  events.
- After data, it polls for up to 200 µs before it parks.
- Otherwise it parks with a timed backoff from 2 ms to 20 ms.

The per-event callback does not signal the parser after enqueue. It sets a
same-thread flag. `ProcessTrace` calls the ETW buffer callback on the consumer
thread after each delivered buffer. That callback unparks the parser when the
flag is set. A burst therefore starts to drain within microseconds, not after
the parser's backed-off park. See
[ETW buffer wake validation](../history/buffer-wake-validation.md).

The channel has no registered blocking receivers or selectors. Atomic retries,
spinning, yielding, and preemption still apply.

### Delivery batches

The service does not ask ETW to flush the session. ETW delivers real-time
events about once per `--etw-flush-seconds`, even with full buffers. It
delivers those buffers as one batch. The buffer callback wakes the parser for
each delivered buffer, so the parser drains the batch while it arrives.

On the 6-vCPU lab VM, ETW delivered one batch per second at 10k and 20k
queries/s, which is about 20k and 39k events/s. It delivered buffers
continuously at 50k queries/s and above. The threshold is not known and can
depend on the machine.

Capacity guidance: one batch holds up to about
2 x queries/s x `--etw-flush-seconds` events, because there are 2 events per
query. Keep `--capacity` well above that size. In the lab, the measured peak
queue was far below that size, because the parser drains during delivery. At
about 44,000 queries/s the rule gives 88,000 events, and the peak queue was
30,643. The recommended high-rate settings and their lab results are in the
[configuration reference](../guide/configuration.md#high-rate-settings). With
them, the channel dropped no events up to about 44,000 queries/s. At about
49,400 queries/s, one of two runs filled the 65,536-event channel. See the
[flush removal validation](../history/flush-removal-validation.md).

### Removed periodic flush

Earlier builds had an opt-in periodic flush. The supervisor thread asked ETW to
flush the session at a set interval, and it woke the parser after each flush.
The buffer wake in commit `dde7188` made it unnecessary, so the maintainer
removed it.
The lab results below used that flush with 64 KiB × 256 buffers. They are
history. They do not apply to the current build:

| Queries/s | Flush interval | Result |
| --- | --- | --- |
| 20,000 | 50 ms | 0 ring drops |
| 10,000 | 100 ms | 0 ring drops |
| 50,000 | 100 ms | 0.04 to 0.91% loss. `etw_lost` was 0. |

With the same buffers and no flush, before the buffer wake, the channel dropped
59 to 79% of events at 10,000 to 20,000 queries/s. At 20,000 queries/s with the flush,
the largest batch was about 6,800 events with a 100 ms interval and about 3,500
with 50 ms. The flush wrote about 5 to 16% more ETW buffers and cost under 0.1%
of a core when idle.

### Pools and memory budget

The regular pool has `capacity + 2` slots with 2048 payload bytes each. The jumbo
pool has 131,184-byte slots. `--jumbo-capacity` accepts 0–256, and its default is
64. The two classes have independent admission limits. Only 64 simultaneous jumbo
leases fit at the defaults, even if the channel has spare entries. Slot return
uses class-specific `ArrayQueue` free lists. No pool grows during capture.

The counted storage includes the payload, the slot metadata, the free lists, and
the channel elements. Its totals are:

| Configuration | Bytes | MiB |
| --- | --- | --- |
| Defaults | 29,595,984 | 28.22 |
| Maximum (262,144 events, 256 jumbo slots) | 711,076,688 | 678.14 |

These totals exclude allocator overhead, native ETW buffers, stacks, TDH caches,
protobuf data, and output buffers. See [Pooled ETW intake](pooled-intake.md) for
the detailed budget and the ownership contract. Measure the actual process
memory and every intake loss category under the intended workload.

## Historical capacity experiment

The earlier capacity experiment used an allocation-based `ArrayQueue`. Its results explain the 8192 default. They are in [Ring capacity experiment (historical)](../history/ring-capacity-experiment.md).
They do not validate the pooled implementation or its jumbo limit.

## Completion and resource ownership

Orderly shutdown stops capture and joins the native `ProcessTrace` worker before
publishing `producer_done` with release ordering. No callback can enqueue after
that publication. The supervisor then unparks and joins the parser. A startup
failure also publishes completion and unparks the parser.

After an empty `try_recv`, the parser reads completion with acquire ordering. If
it observes completion, it calls `try_recv` again until the channel is empty.
This final drain keeps an event sent between the earlier empty receive and the
completion read. The post-data linger also ends when it reads completion. The
existing 10-second fail-stop watchdog covers native shutdown and parser
completion. Callback state stays owned until the native worker ends. Within
that window, native shutdown waits up to 5 seconds for `ProcessTrace` to return
after STOP before `CloseTrace` forces it.

Unit tests cover these cases:

- The default and configured capacity bounds.
- Loss when the ring is full.
- A final enqueue followed by joined producer completion.
- Empty completion after a startup failure.
- Output errors.

Synthetic tests do not establish live DNS
throughput or an installed service's token configuration.

**Unverified lab notes (P3z).** The results are not in the repository, so they are not evidence here. The run used a quiet host with one 6-vCPU lab VM, 1,000 queries/s, `--etw-max-buffers 12`, and only the 1 s ETW flush timer. The smallest ETW buffer size lost no events. Smaller rings dropped events in some runs. Delivery stayed at about one batch per second for every buffer size tested. This is one lab and one rate. Keep the default 8,192 for headroom (about 4 s of inflow at 1,000 queries/s).
