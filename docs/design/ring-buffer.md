# Ring capacity and shutdown

> **Status:** Current (the queue and shutdown reference; its capacity experiment is historical) · **Evidence dated:** October 28-29, 2026 for the experiment · **Commit:** not recorded · **Reviewed:** 2026-11-01

This note explains the queue bounds and shutdown contract for maintainers and
administrators choosing a capture capacity. It describes implementation limits
and the capacity experiment; it does not guarantee loss-free capture at every
traffic rate or event size.

## Capacity and memory

The defaults, their low-rate limits, and the recommended high-rate settings are
stated once, in the [configuration reference](../guide/configuration.md#low-rate-defaults).
Read that section before you choose `--capacity`, `--etw-flush-ms`, or the ETW
buffer options.

The user-mode ETW callback leases preallocated storage, copies each accepted
event, and calls `crossbeam_channel::Sender::try_send` on a bounded event channel.
It does not wait for capacity or allocate callback storage. A full channel drops
the newest event and increments `ring_dropped`. The default capacity is 8192
events; `--capacity` accepts 1,024–262,144. The parser uses only `try_recv` and polls
an empty channel with a timed park that backs off from 2 ms to 20 ms while idle.
When `--etw-flush-ms` is set (opt-in; default 0, off), the supervisor thread also asks ETW to flush the session at that interval, because ETW otherwise delivers about once per `--etw-flush-seconds`, even with full buffers, and delivers them as one batch, which can overflow the
channel. On the 6-vCPU lab VM, ETW delivered one batch per second at 10k and
20k queries/s (about 20k and 39k events/s) and delivered promptly at 50k
queries/s and above. The threshold is not known and may depend on the machine.
At 10,000 queries/s with the flush disabled, about 59-61% of events were
dropped there.

The lab results for the in-process flush (0 ring drops at 10k and 20k queries/s
with the flush, against 59-79% loss without it; the 50k comparison and the
settings it used; the batch sizes; and the untested cases) are in the
[configuration reference](../guide/configuration.md#low-rate-defaults). They are
not repeated here.

Capacity guidance: each delivered batch holds roughly up to twice the flush
interval's worth of events (2 events per query). In the batching regime, lower
`--etw-flush-ms` or raise `--capacity` so that 2 x rate x interval stays well
below the channel capacity.

The callback does not signal the parser after enqueue. The channel has no
registered blocking receivers or selectors; atomic retries, spinning, yielding,
and preemption still apply. The supervisor thread, not the callback, unparks the
parser after each successful periodic ETW flush, so a flushed batch does not wait
for the parser's idle park.

The regular pool has `capacity + 2` slots with 2048 payload bytes each. The jumbo
pool has 131,184-byte slots, with `--jumbo-capacity` accepting 0–256 and defaulting
to 64. The classes have independent admission limits. Only 64 simultaneous jumbo
leases fit at the defaults, even if the channel has spare entries. Slot return
uses class-specific `ArrayQueue` free lists. No pool grows during capture.

Counted payload, slot metadata, free-list, and channel element storage totals
29,595,984 bytes (28.22 MiB) at the defaults and 711,076,688 bytes (678.14 MiB) at
the maximum (262,144 events, 256 jumbo slots). These calculations exclude allocator overhead, native
ETW buffers, stacks, TDH caches, protobuf data, and output buffers. See
[Pooled ETW intake](pooled-intake.md) for the detailed budget and ownership
contract. Measure actual process memory and every intake loss category under
the intended workload.

## Historical capacity experiment

The results of the earlier allocation-based `ArrayQueue` capacity experiment, which explain the 8192 default, are in [Ring capacity experiment (historical)](../history/ring-capacity-experiment.md).
They do not validate the pooled implementation or its jumbo limit.

## Completion and resource ownership

Orderly shutdown stops capture and joins the native `ProcessTrace` worker before
publishing `producer_done` with release ordering. No callback can enqueue after
that publication. The supervisor then unparks and joins the parser. A startup
failure also publishes completion and unparks the parser.

After an empty pop, the parser reads completion with acquire ordering. If it
observes completion, it pops again until empty. This final drain retains a push
between the earlier empty pop and completion observation. The existing
10-second fail-stop watchdog covers native shutdown and parser completion;
callback state remains owned until the native worker ends. Within that window,
native shutdown waits up to 5 seconds for `ProcessTrace` to return after STOP
before `CloseTrace` forces it.

Unit tests cover the default and configured capacity bounds, full-ring loss,
final enqueue followed by joined producer completion, empty completion after
startup failure, and output errors. Synthetic tests do not establish live DNS
throughput or an installed service's token configuration.

Unverified lab notes (P3z; results not in the repository, so they are not evidence here): on a quiet host with one 6-vCPU lab VM, at 1,000 queries/s, with `--etw-max-buffers 12` and only the 1 s ETW flush timer, the smallest ETW buffer size lost no events, and smaller rings dropped events in some runs. Delivery stayed at about one batch per second for every buffer size tested. This is one lab and one rate. Keep the default 8,192 for headroom (about 4 s of inflow at 1,000 queries/s).
