# Tune capture for load

> **Status:** Current · **Describes:** the code at commit `13972d8` · **Reviewed:** 2026-11-04

Use this guide to read the defaults, change `--etw-*` and `--capacity` for your query rate, and
`--spool-max-mib` for the outage you must cover. Option details are in the
[configuration reference](configuration.md#options). Loss counters are in
[Log events and counters](monitoring.md).

Lab results are in [Guide lab results, 2026-11](../history/guide-lab-results-2026-11.md) and
the other records under [docs/history/](../history/). This guide keeps the conclusions and
the settings.

## ETW buffer minimums

`--etw-min-buffers` is the requested minimum. ETW reserves it at session start, as the
minimum times `--etw-buffer-kib`. It allocates more buffers on demand, up to
`--etw-max-buffers`. Raising the maximum allocates nothing up front.

The default is 12. `0` lets ETW choose. ETW raises the minimum to at least two buffers per
logical processor, so the default of 12 has an effect only on a host with fewer than six
logical processors. ETW may also lower a minimum above its own limit. The maximum rises to at
least the adjusted minimum.

The default maximum is 128. On a host with more than 64 logical processors, ETW's minimum is
above 128, so the maximum rises to match. Peak buffer memory can exceed the requested bound.

The minimum must not exceed `--etw-max-buffers`. Otherwise the service does not start and
reports `ETW minimum buffers (129) must not exceed ETW maximum buffers (128)`, with both values.
ETW cannot change the minimum while the session runs, so a new value takes effect at the next
start.

Microsoft advises setting the minimum and the maximum to the same value when you start tuning.
Raise the maximum if the session drops events during peaks.

ETW can apply other values than the requested ones. The service then writes
`ETW buffers applied` with `etw_min_buffers_applied`, `etw_max_buffers_applied`, and
`etw_buffers_allocated`. If the query fails, the event is not written. The
`ETW Buffers Allocated` counter shows the effective minimum just after start.

## Defaults

<a id="settings-for-200000-queries-per-second"></a>

This is the canonical statement of the default limits. Other documents link here.

The defaults are `--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128
--capacity 131072`: 128 KiB buffers, at least 12 of them and at most 128, and a channel of
131,072 events. They are sized for 200,000 queries per second, which is about 400,000 events
per second and 41.2 MB per second of dnstap frames.

> **Tested on one guest.** The lab ran these values at 200,000 queries per second on a 12-vCPU,
> 8 GiB guest. See [200,000 QPS sizing runs on 12 vCPUs](../history/slow-receiver-validation.md#200000-qps-sizing-runs-on-12-vcpus-2026-11-04).
> Other machines and loads can differ. Watch `ring_dropped` and `etw_lost`.

Each value is set from the largest value the lab observed at 200,000 queries per second. See
[200,000 QPS validation](../history/200k-qps-validation.md).

| Setting | Largest observed | Default |
| --- | --- | --- |
| ETW buffer size (`--etw-buffer-kib`) | 128 KiB, no loss, about 373 events per buffer | 128 |
| Maximum ETW buffers (`--etw-max-buffers`) | 29 allocated (3.6 MiB), at most 7 in use at once | 128 (16 MiB) |
| Minimum ETW buffers (`--etw-min-buffers`) | 7 in use, of ETW's minimum of 12 | 12 |
| Channel (`--capacity`) | Peak queue 28,129 idle, 61,239 with 12 busy CPU threads | 131,072 |

The lab ran this load twice, for 50 seconds each, with `--etw-max-buffers 256` and
`--capacity 262144`. Both runs had no ring drops, no `etw_lost`, and no lost ETW buffers.

Why these values:

- **Maximum buffers.** 128 is more than four times the peak of 29. ETW allocates buffers only
  on demand.
- **Minimum buffers.** ETW used one buffer per logical processor, plus at most one waiting for
  delivery. ETW raises the minimum to at least two per logical processor, so 12 has an effect
  only on a host with fewer than 6 logical processors.
- **Channel.** 131,072 is more than four times the idle peak queue of 28,129. ETW
  sometimes delivers events about 2 seconds late and then in one burst. A 24,576-event channel
  overflowed on such bursts, and `intake_dropped` counted 23,549 to 60,439 events. At 200,000
  queries per second the lab compared sizes on the 12-vCPU guest:

  | `--capacity` | Idle, peak queue | 12 busy CPU threads, peak queue | 24 busy CPU threads | Service working set (peak) |
  | --- | --- | --- | --- | --- |
  | 65,536 | 24,223 to 61,903 (94%), 0 dropped in 3 runs | 63,940 (98%), 0 dropped | Full. One run dropped 12,264 events | 241 MB |
  | 131,072 | 27,385 and 28,129, 0 dropped in 2 runs | 61,239 (47%), 0 dropped | Not run | 356 MB |
  | 262,144 | Not run at 200,000 queries per second | Not run | Not run | 812 MB |
  | 524,288 | 22,083 and 71,214, 0 dropped in 2 runs | Not run | Not run | 1,261 to 1,399 MB |

  At 65,536 the channel came close to full even on an idle guest. At 131,072 the busiest run
  used 47% of the channel and no run dropped an event. The step from 65,536 to 131,072 costs
  115 MB of working set. The step to 262,144 would cost another 456 MB. The step to 524,288 brings it to about 1.4 GB.

Costs:

- Counted storage is 283.03 MiB at the default capacity. See [Storage](configuration.md#storage).
- The startup deadline is 18 seconds (26 seconds at `--capacity 262144`, 42 seconds at `--capacity 524288`). See
  [Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines).
- ETW buffer memory is at most 16 MiB (128 buffers of 128 KiB), plus what ETW adds on a host
  with more than 64 logical processors. The minimum reserves 12 × 128 KiB = 1.5 MiB at start,
  or 3 MiB on a 12-vCPU guest, where ETW's own minimum is 24.
- The sender's block pool is 68.5 MiB. It does not depend on an option.

The sender queues are sized for the same rate. See [Spool sizing](#spool-sizing).

**Warning.** The service logs at most one warning per run (Event ID 3). It does so when the default
ETW buffer size, buffer counts, flush interval, and `--capacity` are in use. It logs at the end of
the first measurement window in which the received rate exceeds 440,000 events per second, just
above 200,000 queries per second. The window is `--stats-interval-seconds`, or 60 seconds when
that is `0`. The message reads `received rate is above 440,000 events/s; the default buffer
settings were validated at about 400,000 events/s (200,000 queries per second)`. The warning does
not mean events were lost. Watch `ring_dropped` and `etw_lost`. If they rise, raise `--capacity`
and `--etw-max-buffers`, for example to `--etw-max-buffers 256 --capacity 262144`. An earlier lab load of 200,000 queries per second ran without loss at that
value, but it uses 558.03 MiB of counted storage and about 812 MB of working set. The largest
`--capacity` is 524,288. It uses 1,108.03 MiB of counted storage at 64 jumbo slots, and its
working set is about 1.4 GB. The startup deadline is 42 seconds.

The lab also found a case that the channel size does not fix. At 100,000 queries per second on
the idle 12-vCPU guest, ETW delivered events late, in gaps of 1 to 3 seconds followed by a
burst. 65,536 dropped 0.89 to 1.40 million events, 131,072 dropped 0.31 to 0.46 million, and
262,144 dropped events in 4 of 5 runs (up to 0.16 million). 524,288 dropped nothing in 3 of 3
runs, with the channel at most 53% full, at a working set of about 1.4 GB. The gaps themselves
did not change with the channel size. The cause is on the ETW delivery side and is still open.
See [Event channel 262,144 and 524,288](../history/slow-receiver-validation.md#event-channel-262144-and-524288-at-100000-qps-2026-11-04). If `ring_dropped` rises at a rate well below 200,000 queries per second, check the
delivery gaps in the lab write-up before you raise `--capacity` further.
The 12-vCPU runs are in [200,000 QPS sizing runs on 12 vCPUs](../history/slow-receiver-validation.md#200000-qps-sizing-runs-on-12-vcpus-2026-11-04).

Choosing the value:

- One delivery holds up to about `2 × queries/s × --etw-flush-seconds` events (2 per query).
  Keep `--capacity` well above this.
- The measured peak was far lower, because the parser drains the channel during a delivery.
  Treat the rule as a safe upper limit.
- A longer `--etw-flush-seconds` makes each delivery larger. Raise `--capacity` by the same
  factor.
- Larger ETW buffers reduce `etw_lost`. In the lab, 64 KiB × 256 buffers removed the
  `etw_lost` that the previous defaults had at 30,000 queries per second and above.

Earlier lab runs with `--etw-max-buffers 32` and `--capacity 24576` lost events at 200,000
queries per second. Their buffer size is not recorded. See
[200,000 queries per second with 32 buffers and a 24,576-event channel](../history/guide-lab-results-2026-11.md#200000-queries-per-second-with-32-buffers-and-a-24576-event-channel).

## Lower rates

At a low rate, the defaults use more memory than the load needs. The settings below were
tested at 10,000 and 50,000 queries per second. The [rate warning](#defaults) is off when any
ETW buffer value, the flush interval, or `--capacity` differs from its default.

### Previous defaults, tested at 10,000 queries per second

<a id="low-rate-defaults"></a>

The previous defaults were `--etw-buffer-kib 16 --etw-max-buffers 64 --capacity 24576`, with
`--etw-min-buffers 0`: 16 KiB buffers, at most 64 of them, and a channel of 24,576 events.
They use 59.59 MiB of counted storage, a 12-second startup deadline, and at most 1 MiB of ETW
buffers.

At 10,000 queries per second (about 20,000 events per second), a 60-second run with these
settings had no ring drops and no `etw_lost`. See
[60-second run with the defaults](../history/guide-lab-results-2026-11.md#60-second-run-with-the-defaults-at-10000-queries-per-second).

ETW delivers real-time events in batches about `--etw-flush-seconds` apart. A batch larger
than the channel drops the excess, unless the parser drains it on arrival. The ETW buffer
callback wakes the parser for each delivered buffer, so it drains each batch as it arrives.

With the defaults before those (12 maximum buffers and an 8,192-event channel), the channel was
full from about 9,000 queries per second. Loss rose with rate. Two 60-second runs with that
channel dropped events after late deliveries. See [Two 60-second runs with the previous
channel](../history/guide-lab-results-2026-11.md#two-60-second-runs-at-10000-queries-per-second-with-the-previous-channel).
The per-rate loss tables are in [ETW flush removal
validation](../history/flush-removal-validation.md) and [ETW buffer wake
validation](../history/buffer-wake-validation.md).

Not tested: other machines and vCPU counts, recursive or mixed workloads, runs longer than
50 seconds, and TLS receivers. The lab used plaintext receivers.

A 10-minute run at 10,000 queries per second used at most 16 buffers, far below 64. See
[10-minute run](../history/guide-lab-results-2026-11.md#10-minute-run-at-10000-queries-per-second).
The 60-second run above is the only run of these settings.

### Settings tested at 50,000 queries per second

<a id="high-rate-settings"></a>

```text
--etw-max-buffers 256 --etw-buffer-kib 64 --capacity 65536
```

Tested on the 6-vCPU VM with an authoritative load, 50-second runs, and a plaintext receiver.
The per-rate table is in [ETW flush removal validation](../history/flush-removal-validation.md).

- ETW lost no events in any run. The channel dropped no events up to about 44,000 queries per
  second.
- At about 49,400 queries per second, one of two runs filled the channel and dropped 0.28%.
- At about 49,500 queries per second, the service used about 4.4 seconds of CPU per 50-second
  run. The settings before the previous defaults used 8.4 to 8.7 seconds.

**65,536 is enough up to about 44,000 queries per second on this VM. It is marginal at a
sustained 50,000.** The default is now 131,072, which this 6-vCPU VM did not run. The later
12-vCPU runs used `--capacity 65536` and the 256-block send queue and link FIFO, with a 320 Mbit
shaped link at 50,000 queries per second.
Four of four runs dropped no events and spilled nothing, and the channel peaked at 37,000 to
40,000. See
[200,000 QPS sizing runs on 12 vCPUs](../history/slow-receiver-validation.md#200000-qps-sizing-runs-on-12-vcpus-2026-11-04).

These results cover one VM, an authoritative load, 50-second runs, and
`--etw-flush-seconds 1`. Monitor `ring_dropped` and `etw_lost` under the real load after each
change.

Earlier periodic-flush results do not apply, because this build has no periodic flush. See
[Removed periodic flush](../design/ring-buffer.md#removed-periodic-flush). With no flush and an
8,192-event channel, the channel lost most events at 10,000 to 20,000 queries per second. That
result predates the buffer wake and has not been repeated. See
[no periodic flush](../history/guide-lab-results-2026-11.md#no-periodic-flush-channel-loss-of-59-to-79-at-10000-to-20000-queries-per-second) and
[periodic flush](../history/guide-lab-results-2026-11.md#periodic-flush-0-ring-drops-at-10000-and-20000-queries-per-second).

## Spool sizing

The spool holds frames while the receiver is down or slow. `--spool-max-mib` caps it, so it
sets how long an outage the spool can cover. The frame rate sets how fast the cap fills.

The figures use 103 bytes per frame, the average receiver size in the small-answer lab runs,
including the 4-byte length prefix. Each query makes two frames: event 256 and event 257 or
258. Larger answers make larger frames, so the times below can be shorter.

| Queries per second | Frames per second | Bytes per second | MiB per hour | Time to fill the default 4,096 MiB cap |
| --- | --- | --- | --- | --- |
| 1,000 | 2,000 | 206,000 | 707 | about 5.8 hours |
| 10,000 | 20,000 | 2,060,000 | 7,072 | about 35 minutes |
| 50,000 | 100,000 | 10,300,000 | 35,362 | about 7 minutes |
| 200,000 | 400,000 | 41,200,000 | 141,449 | about 104 seconds |

These are calculations from the 103-byte average. The lab has not run a spool outage, so no
figure here is a measured result.

While the receiver connection drains a backlog, the service holds up to about 8.7 MiB (127 batches
each of at most 71,684 bytes) that it has read and the connection has not yet taken. The segments that hold any of it
stay on disk whole and count in the cap and in `spool_bytes`, and the cap does not delete them.
Those segments can hold more than that: that data plus two segments, which is 24.7 MiB at the
smallest cap (64 MiB, with 8 MiB segments) and about 137 MiB with 64 MiB segments (caps of 512 MiB
and above). The smallest cap always has other segments to delete.

- Size the cap for the longest outage you must cover. Multiply MiB per hour by the hours, and
  add a margin. For 10,000 queries per second and a two-hour outage, the spool needs about
  14,144 MiB, so set `--spool-max-mib 16384`.
- `--spool-min-free-mib` (default 2,048) keeps that much free on the volume. The volume must
  also hold that floor and one 64 MiB segment: 16,384 + 2,048 + 64 = 18,496 MiB (about
  18.1 GiB).
- During an outage, the disk takes writes at the bytes-per-second rate. The backlog drains at
  the rate the receiver accepts. At 200,000 queries per second, that is about 41 MB per second
  of writes, then reads at the same rate.
- The send queue, between the parser and the router, is in memory: 256 blocks of about 64 KiB
  (about 16 MiB). It absorbs short router delays and a burst of late ETW events. The link FIFO
  adds another 256 blocks (about 16 MiB) for a slow receiver. Neither absorbs an outage: while
  the receiver is down, the router sends blocks to the spool. The send queue lasts about 8
  seconds at 10,000 queries per second, and about 0.4 seconds at 200,000. At 200,000 queries per
  second the link FIFO covers two Windows Server scheduler quanta (187.5 ms each) of a link
  thread that is not scheduled.

See [The receiver is slow](troubleshooting.md#the-receiver-is-slow) for the steps to take when
the spool grows.

## Next steps

- [Configuration reference](configuration.md)
- [Log events and counters](monitoring.md)
- [Ring capacity and shutdown](../design/ring-buffer.md)
