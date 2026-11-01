# Ring capacity and shutdown

> **Status:** Current (the queue and shutdown reference; its capacity experiment is historical) · **Evidence dated:** October 28-29, 2026 for the experiment · **Commit:** not recorded · **Reviewed:** 2026-11-01

This note explains the queue bounds and shutdown contract for maintainers and
administrators choosing a capture capacity. It describes implementation limits
and the capacity experiment; it does not guarantee loss-free capture at every
traffic rate or event size.

## Capacity and memory

The defaults, their low-rate limits, and the recommended high-rate settings are
stated once, in the [configuration reference](guide/configuration.md#low-rate-defaults).
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
[configuration reference](guide/configuration.md#low-rate-defaults). They are
not repeated here.

Capacity guidance: each delivered batch holds roughly up to twice the flush
interval's worth of events (2 events per query). In the batching regime, lower
`--etw-flush-ms` or raise `--capacity` so that 2 x rate x interval stays well
below the channel capacity.

The callback does not signal the parser after enqueue. The channel has no
registered blocking receivers or selectors; atomic retries, spinning, yielding,
and preemption still apply.

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

The following results apply to the allocation-based `ArrayQueue` transport in
the installed capacity-only binary `CA3342…64347`. They do not validate the
pooled implementation or its separate jumbo limit. In that earlier design,
8192 queued records at the accepted maximum user and aggregate extended-data
sizes could contain 1,073,725,440 copied bytes, almost 1 GiB, before record,
vector, and extended-item overhead. That historical calculation does not
describe the preallocated pool budget.

Matched 30-second pilots at approximately 1000 queries per second produced ring
drops with a capacity of 1024. With the original polling implementation and a
capacity of 8192, the authoritative and recursive pilots both reported zero ring
drops. Their observed high-water counts were 3022 and 4972 events, respectively,
which exceed the old default. These observations motivated selecting 8192 as the default.

An enqueue-wakeup candidate with capacity 1024 still dropped events and was
rejected; see [Rejected wake-on-enqueue experiment](ring-wakeup.md). The capacity
candidate retained the original polling behavior. The subsequent full 300-second authoritative and recursive-request runs at
approximately global 1000 QPS had zero ring drops, with high water 3618/4984
and exact receiver delivery. The recursive run had one RA-check failure, so
the separate full DNS-validity gate did not pass. See the
[validation report](ring-buffer-fix-validation.md) for counts, CPU, memory,
WPA evidence and limitations. Measure loss,
queue high water, and memory on the intended DNS host before selecting capacity
for another workload.

The later [pooled VM campaign](pooled-vm-performance-review.md) measures a
historical pooled binary with a JSONL metrics writer. The TCP-only service has
no durable metrics export or queue high-water tracking; see the
[instrumentation compatibility note](README.md#instrumentation-compatibility-note). Keep its results separate from this historical
allocation-based capacity experiment, especially when comparing CPU or memory.

## Completion and resource ownership

Orderly shutdown stops capture and joins the native `ProcessTrace` worker before
publishing `producer_done` with release ordering. No callback can enqueue after
that publication. The supervisor then unparks and joins the parser. A startup
failure also publishes completion and unparks the parser.

After an empty pop, the parser reads completion with acquire ordering. If it
observes completion, it pops again until empty. This final drain retains a push
between the earlier empty pop and completion observation. The existing
10-second fail-stop watchdog covers native shutdown and parser completion;
callback state remains owned until the native worker ends.

Unit tests cover the default and configured capacity bounds, full-ring loss,
final enqueue followed by joined producer completion, empty completion after
startup failure, and output errors. Synthetic tests do not establish live DNS
throughput or an installed service's token configuration.

P3z (6-vCPU lab VM, quiet host only, 1,000 queries/s, authoritative, `--etw-max-buffers 12`, only the 1 s ETW flush timer): 4 KiB ETW buffers (the minimum) lost nothing. Delivery stayed at about one batch per second regardless of buffer size: about 1 s of events normally (about 2,000 events) and up to just over 2 s (at least about 4,063 events, inferred from one 300 s run with a 3,840 ring that dropped 223). A 4,096 ring passed 2 x 300 s release runs (private memory about 21.7 MB) with under 1% headroom; 3,840 passed both 60 s runs but dropped in a 300 s run, 3,584 passed one of two 60 s runs, and 3,072 and below dropped. This is one lab, one rate; keep the default 8,192 for headroom (about 4 s of inflow at 1,000 queries/s). The P3z results are not in the repository.
