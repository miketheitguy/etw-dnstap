# Ring capacity and shutdown

> **Status:** Current (the queue and shutdown reference. Its capacity experiment is historical.) · **Describes:** `main` at commit `dde7188` · **Evidence dated:** October 28-29, 2026 for the experiment · **Reviewed:** 2026-11-01

This note is for maintainers, and for administrators who choose a capture
capacity. It explains the queue bounds and the shutdown contract. It describes
the implementation limits and the capacity experiment. It does not guarantee
loss-free capture at every traffic rate or event size.

## Capacity and memory

The defaults, their low-rate limits, and the recommended high-rate settings are
stated once, in the [configuration reference](../guide/configuration.md#low-rate-defaults).
Read that section before you choose `--capacity`, `--etw-flush-ms`, or the ETW
buffer options.

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
- When the periodic ETW flush is on, the supervisor thread unparks it after each
  successful flush, as a backstop.

The per-event callback does not signal the parser after enqueue. It sets a
same-thread flag. `ProcessTrace` calls the ETW buffer callback on the consumer
thread after each delivered buffer. That callback unparks the parser when the
flag is set. A burst therefore starts to drain within microseconds, not after
the parser's backed-off park. See
[ETW buffer wake validation](../history/buffer-wake-validation.md).

The channel has no registered blocking receivers or selectors. Atomic retries,
spinning, yielding, and preemption still apply.

### Periodic flush

`--etw-flush-ms` is opt-in. Its default is 0, which is off. When it is set, the
supervisor thread asks ETW to flush the session at that interval. Without it,
ETW delivers about once per `--etw-flush-seconds`, even with full buffers. It
delivers those buffers as one batch, which can overflow the channel.

On the 6-vCPU lab VM, ETW delivered one batch per second at 10k and 20k
queries/s, which is about 20k and 39k events/s. It delivered promptly at 50k
queries/s and above. The threshold is not known and may depend on the machine.
At 10,000 queries/s with the flush off, about 59-61% of events were dropped
there.

The [configuration reference](../guide/configuration.md#low-rate-defaults) has
the lab results for the in-process flush. They are not repeated here. The
results include these items:

- 0 ring drops at 10k and 20k queries/s with the flush, against 59-79% loss
  without it.
- The 50k comparison and the settings that it used.
- The batch sizes and the untested cases.

Capacity guidance: each delivered batch holds up to about twice the flush
interval's worth of events, because there are 2 events per query. In the
batching regime, lower `--etw-flush-ms` or raise `--capacity`. Keep
2 x rate x interval well below the channel capacity.

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
