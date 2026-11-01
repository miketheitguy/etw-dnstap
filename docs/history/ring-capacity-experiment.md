# Ring capacity experiment (historical)

> **Status:** Historical (the allocation-based `ArrayQueue` capacity experiment; superseded by [Pooled ETW intake](../design/pooled-intake.md), and its results do not validate the pooled implementation) · **Evidence dated:** October 28-29, 2026 · **Commit:** not recorded · **Reviewed:** 2026-11-01

This section was moved from [Ring capacity and shutdown](../design/ring-buffer.md),
which now gives the current queue and shutdown contract. The results below apply to
the recorded binaries only.

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

The later [pooled VM campaign](../performance/pooled-vm-performance-review.md) measures a
historical pooled binary with a JSONL metrics writer. The TCP-only service has
no durable metrics export. It logs its counters to the event log and samples the
deepest queue depth as a lower bound (`queue_depth_max`); see the
[instrumentation compatibility note](../README.md#instrumentation-compatibility-note). Keep its results separate from this historical
allocation-based capacity experiment, especially when comparing CPU or memory.
