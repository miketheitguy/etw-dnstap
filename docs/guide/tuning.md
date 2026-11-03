# Tune capture for load

> **Status:** Current · **Describes:** the code at commit `58fde72` · **Reviewed:** 2026-11-03

Use this guide to choose `--etw-*` and `--capacity` values for the query rate
you expect, and `--spool-max-mib` for the outage you must cover. The option list is in the
[configuration reference](configuration.md#options). The counters that show loss
are in [Log events and counters](monitoring.md).

The lab ran the defaults as a set once: one 60-second run at 10,000 queries per
second. See [Low-rate defaults](#low-rate-defaults). The other results in this
guide come from other settings. The sections below give the lab results for each
setting.

## ETW buffer minimums

`--etw-min-buffers` sets the requested minimum buffer count. ETW allocates the
minimum when the session starts and keeps those buffers for the whole session.
The reserved memory is the minimum times `--etw-buffer-kib`. ETW allocates more
buffers only on demand, up to the maximum, and that allocation happens during
the burst that needs the buffers. Raising `--etw-max-buffers` only allows more
buffers on demand. It does not allocate them up front.

The default is `0`, which lets ETW choose. The service holds no fixed minimum.
ETW raises the minimum to at least two buffers per logical processor. It may
reduce a minimum that exceeds its own limit. It raises the maximum to at least
the adjusted minimum.

The default maximum is 64. On a host with more than 32 logical processors,
ETW's minimum is above 64, and ETW raises the maximum to match. With the
previous default maximum of 12, the maximum was the effective floor on the
6-vCPU lab VM: ETW's minimum and maximum were both 12. Peak buffer memory can
exceed the requested bound. With the default minimum, a host reserves two
buffers for each logical processor.

The minimum must not exceed `--etw-max-buffers`. Otherwise the service does not
start and reports `ETW minimum buffers (65) must not exceed ETW maximum buffers
(64)`, with the two values given. ETW cannot change the minimum while the
session runs, so a new value takes effect at the next service start.

Microsoft advises starting to tune a session by setting the minimum and the
maximum to the same value. Raise the maximum if the session drops events during
peaks.

ETW can apply different values than the ones requested. After the session
starts, the service queries it once and writes the information event `ETW
buffers applied`. It has `etw_min_buffers_applied`, `etw_max_buffers_applied`,
and `etw_buffers_allocated`, as ETW reports them. If the query fails, the
service writes no such event. The `ETW Buffers Allocated` counter also shows the
effective minimum just after the session starts.

## Low-rate defaults

This is the canonical statement of the default limits. Other documents link here.

The defaults are `--etw-buffer-kib 16 --etw-max-buffers 64 --capacity 24576`:
16 KiB buffers, at most 64 of them, and a channel of 24,576 events.

**The lab validated the current defaults in one 60-second run at 10,000 queries
per second** (about 20,000 events per second) on 2026-11-01. Every event was
captured, with 0 ring drops and 0 `etw_lost`. ETW delivered late four times. Each
time about 35,000 to 41,000 events arrived in one second, and the channel peaked
at 8,800 events. ETW had at most 5 buffers in use and allocated at most 18. The
rate warning below uses a threshold of 22,000 events per second, just above
this validated rate.

The lab measured the tables below with the previous defaults: 16 KiB buffers, a
maximum of 12, and a channel of 8,192 events. Read them as results for those
settings.

ETW delivers real-time events in batches about `--etw-flush-seconds` (1 second
by default) apart. Delivery latency is therefore up to about 2 seconds, and
usually under 1 second. The service does not ask ETW for more flushes. A batch
that holds more events than the channel can take drops the excess unless the
parser drains it while it arrives. The ETW buffer callback wakes the parser for
each delivered buffer that enqueued an event. The parser therefore drains each
batch while it arrives.

The lab used one 6-vCPU, 4 GiB VM with Windows Server 2025, an authoritative
load, and 50-second runs. The rates are the rates that the client achieved. The
results with all of the previous defaults were:

| Queries per second | Ring drops | `etw_lost` | Build |
| --- | --- | --- | --- |
| 1,000 | 0 | 0 | Buffer wake, and this build (959 queries per second) |
| 5,000 | 0 | 0 | Buffer wake |
| 7,300 to 7,500 | 0 to 0.12%. 2 of 4 runs dropped events | 0 | This build |
| 9,000 to 10,000 | 0.28 to 0.58% | 0 | This build. The buffer-wake build lost 0.32% |
| 14,700 | 1.63% | 0 | This build |
| 30,000 | 12.3 to 16.6% | 0.10 to 0.16% | This build, two clients. See the note below |
| 44,000 | 2.23% | 0.135% | This build |
| 49,500 to 50,000 | 0.34 to 4.39% | 0.199 to 0.215% | This build, two clients. The buffer-wake diag build had 0 ring drops and 0.03 to 0.11% `etw_lost` |

- The 8,192-event channel was full in every run at about 9,000 queries per
  second and above.
- `etw_lost` is loss inside the previous default of 16 KiB × 12 ETW buffers.
- The runs at about 30,000 queries per second lost more than the runs at
  44,000 to 50,000. They also used less service CPU: 1.8 to 1.9 seconds against
  6.6 to 8.7 seconds. Two clients with different pacing gave the same loss, so
  the client is not the cause. The cause in the service or in ETW is not known.
- At about 50,000 queries per second, the loss depended on the client. It was
  0.34 to 1.24% with 54 client threads. It was 4.39% with 4 threads and steady
  pacing.
- Before the buffer wake, a 5,000 queries-per-second load lost about 19%.

See [ETW buffer wake validation](../history/buffer-wake-validation.md) and
[ETW flush removal validation](../history/flush-removal-validation.md).

The lab did not test these cases with the previous defaults:

- Other machines and vCPU counts.
- Recursive or mixed workloads.
- Runs longer than 50 seconds.
- A TLS receiver. The lab used a plaintext receiver.

Later lab runs on 2026-11-01 used other values:

- With `--etw-buffer-kib 16`, `--etw-max-buffers 32`, and `--capacity 8192` at
  10,000 queries per second, two 60-second runs dropped 10,751 and 12,747
  events at the channel. Each drop followed a late ETW real-time delivery: a
  second with about 1,500 events, then about 40,000 the next second. ETW lost
  nothing.
- In a 10-minute run at 10,000 queries per second with 16 KiB buffers, ETW used
  up to 16 buffers and allocated up to 21.
- With the previous default maximum of 12 buffers on the 6-vCPU lab VM, ETW's
  minimum and maximum were both 12.

The current defaults raise the channel to 24,576 events and the maximum to 64
buffers. The lab ran them as a set once, in the 60-second run at 10,000 queries
per second that this section describes first. The tables and runs above used
other settings. They do not show how the defaults behave at other rates or run
lengths.

### High-rate settings

For rates above about 1,000 queries per second, use larger ETW buffers and a
larger channel:

```text
--etw-max-buffers 256 --etw-buffer-kib 64 --capacity 65536
```

The lab tested this combination on the same VM as the previous defaults. The load was
authoritative, each run lasted 50 seconds, and the receiver was plaintext. The
rates are the rates that the client achieved:

| Queries per second | Runs | Ring drops | `etw_lost` | Peak queue |
| --- | --- | --- | --- | --- |
| 7,300 to 7,500 | 4 | 0 | 0 | 8,201 to 8,876 |
| 9,000 to 10,000 | 4 | 0 | 0 | 9,375 to 10,627 |
| 14,700 | 1 | 0 | 0 | 16,776 |
| 30,000 | 2 | 0 | 0 | 27,329 and 28,226 |
| 44,000 | 1 | 0 | 0 | 30,643 |
| 49,400 | 2 | 0 and 0.28% | 0 | 46,408, and 65,536 (full) |

The peak queue is `queue_depth_max`, a sampled lower bound.

- ETW lost no events in any run.
- The channel dropped no events at every rate up to about 44,000 queries per
  second. At 44,000 the peak queue was 30,643 of 65,536.
- At about 49,400 queries per second, one of two runs filled the 65,536-event
  channel and dropped 0.28% of events. The other run peaked at 46,408.
- At about 49,500 queries per second, the service used about 4.4 seconds of
  CPU in each 50-second run. With the previous defaults it used 8.4 to 8.7
  seconds.
- Private bytes rose from about 31 MiB with the previous defaults to about
  179 MiB.

**65,536 is enough up to about 44,000 queries per second on this VM. It is
marginal at a sustained 50,000.** For sustained rates near 50,000 queries per
second, raise `--capacity` further, for example to 131,072. The lab did not
test 131,072.

With the default `--jumbo-capacity 64`, the counted storage and the startup
deadline are:

| `--capacity` | Counted storage | Startup deadline | Lab-tested |
| --- | --- | --- | --- |
| 65,536 | 152,598,608 bytes (145.53 MiB) | 14 seconds | Yes |
| 131,072 | 296,777,808 bytes (283.03 MiB) | 18 seconds | No |

See [Storage](configuration.md#storage) and
[Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines).

Use these rules to choose the values:

- ETW delivers events about once per `--etw-flush-seconds`. Each query makes 2
  events. One delivery therefore holds up to about
  `2 × queries/s × --etw-flush-seconds` events. Keep `--capacity` well above
  this size.
- The measured peak queue was far below that size, because the parser drains
  the channel while a delivery arrives. At 44,000 queries per second the rule
  gives 88,000 events, and the peak queue was 30,643. Use the rule as a safe
  upper limit, not as a prediction.
- A longer `--etw-flush-seconds` makes each delivery larger. If you raise it,
  raise `--capacity` by the same factor.
- Larger ETW buffers reduce `etw_lost`. In the lab, 64 KiB × 256 buffers
  removed the `etw_lost` that the previous defaults had at 30,000 queries per
  second and above.

The lab results apply to one VM, an authoritative load only, and 50-second
runs. The lab did not test other machines, recursive or mixed workloads, longer
runs, a TLS receiver, or an `--etw-flush-seconds` value other than 1. See
[ETW flush removal validation](../history/flush-removal-validation.md).

After you change these settings, monitor `ring_dropped` and `etw_lost` under
the real load.

Two earlier lab results do not apply to this build:

- An explicit periodic flush gave 0 ring drops at 10,000 and 20,000 queries per
  second with 64 KiB × 256 buffers. The service no longer has this flush. See
  [Removed periodic flush](../design/ring-buffer.md#removed-periodic-flush).
- The lab ran 64 KiB × 256 buffers with the previous default 8,192-event channel
  and no flush. The channel dropped 59 to 79% of events at 10,000 to 20,000
  queries per second. That result predates the buffer wake. The lab did not
  repeat it with the buffer wake.

**Warning.** The service can log one warning (Event ID 3) per run. It does so
when the default buffer settings are in use: the default ETW settings (buffer
size, buffer counts, and flush interval) and the default `--capacity`. The
service logs it at the end of the first measurement window in which the received
rate is more than 22,000 events per second. The message reads `received rate is
above 22,000 events/s; the default buffer settings were validated at about
20,000 events/s`, so it states both figures. The window is `--stats-interval-seconds`, or 60
seconds when `--stats-interval-seconds` is `0`. The warning does not mean that
events were lost. It tells you to watch `ring_dropped` and `etw_lost`. If they
increase, raise `--capacity` and `--etw-max-buffers`, for example to
`--etw-buffer-kib 64 --etw-max-buffers 256 --capacity 65536`.

### Settings for 200,000 queries per second

For 200,000 queries per second, use:

```text
--etw-buffer-kib 128 --etw-max-buffers 32 --capacity 24576
```

Each value is a small margin above the largest value that the lab observed at
200,000 queries per second:

| Setting | Largest observed | Recommended |
| --- | --- | --- |
| ETW buffer size (`--etw-buffer-kib`) | 128 KiB, no loss. About 373 events in each buffer. | 128 |
| Maximum ETW buffers (`--etw-max-buffers`) | 29 allocated (3.6 MiB). At most 7 in use at the same time. | 32 (4 MiB) |
| Minimum ETW buffers (`--etw-min-buffers`) | 7 in use of ETW's minimum of 12 (2 per logical processor on the 6-vCPU VM) | `0`, the default. ETW chooses its minimum. |
| Channel (`--capacity`) | Peak queue 21,622 | 24,576 |

The lab ran this load twice for 50 seconds each, on the same VM as the other
results and with a plaintext receiver. The runs used `--etw-max-buffers 256`
and `--capacity 262144`. Both runs had 0 ring drops, 0 `etw_lost`, and 0 lost
ETW buffers. Every encoded frame reached the receiver. The service used about
0.34 of one CPU. The DNS server used about 2.9 CPUs.

Why these values:

- **Maximum buffers.** 32 is 3 buffers above the peak of 29. ETW allocates
  buffers only on demand, so the maximum limits how far ETW can grow. On a host
  with more logical processors, ETW's minimum is higher. ETW raises the maximum
  to at least that minimum.
- **Minimum buffers.** ETW gave one buffer in use per logical processor, plus
  at most one buffer that waited for delivery. Its minimum of 2 per logical
  processor was enough.
- **Channel.** 24,576 is 14% above the peak queue of 21,622. The parser samples
  the queue depth, so the true peak can be slightly higher. The margin covers
  that difference.

Costs:

- Counted storage with `--capacity 24576` and the default 64 jumbo slots is
  62,486,608 bytes (59.59 MiB). The service allocates it at startup. With
  `--capacity 262144`, the counted storage is 558.03 MiB. The lab measured about
  684 MiB of private bytes at this capacity with the earlier 408-byte slot
  header. The header is now 24 bytes, which is about 96 MiB less.
- The startup deadline is 12 seconds at `--capacity 24576` and 26 seconds at
  `--capacity 262144`.
- ETW buffer memory is at most 4 MiB with 32 buffers of 128 KiB.

The lab did not run these exact values. It also did not measure 128 KiB
buffers at 30,000 to 50,000 queries per second. In that range ETW delivered
events in batches in earlier runs. After you apply these settings, monitor
`ring_dropped` and `etw_lost`. If either is not 0, raise `--capacity` or
`--etw-max-buffers`.

**Later lab runs lost events with these values.** The default
`--etw-max-buffers` is now 64, which is above the 32 recommended here. On
2026-11-01 the lab also ran 32 buffers with `--capacity 24576` at 200,000
queries per second. One run had 37,133 ring drops and 351 `etw_lost`. Another
had 3,219 `etw_lost`. See the lab notes. Treat 32 and 24,576 as the earlier
recommendation. No run shows that this setting loses nothing. The recorded
results above stay as measured.

See [200,000 QPS validation](../history/200k-qps-validation.md).


## Spool sizing

The spool holds frames while the receiver is down or slow. Its cap,
`--spool-max-mib`, sets how long an outage it can cover. The rate of the
frames sets how fast the cap fills.

The figures use 103 bytes per frame. That is the average receiver bytes per
frame in the small-answer lab runs, with the 4-byte length prefix included. It is
not a measured minimum. Each query makes two frames: the query (event 256) and
the response (event 257 or 258). Larger answers make larger frames, so the times
below can be shorter.

| Queries per second | Frames per second | Bytes per second | MiB per hour | Time to fill the default 4,096 MiB cap |
| --- | --- | --- | --- | --- |
| 1,000 | 2,000 | 206,000 | 707 | about 5.8 hours |
| 10,000 | 20,000 | 2,060,000 | 7,072 | about 35 minutes |
| 50,000 | 100,000 | 10,300,000 | 35,362 | about 7 minutes |
| 200,000 | 400,000 | 41,200,000 | 141,449 | about 104 seconds |

These are calculations from the 103-byte average. The lab has not run an outage
with the spool, so no figure here is a measured result.

Use these rules to choose the values:

- Size the cap for the longest outage you must cover. Multiply the MiB per hour
  by the hours, and add a margin. For 10,000 queries per second and a two-hour
  outage, the spool needs about 14,144 MiB. Set `--spool-max-mib 16384`. The
  volume must then also hold the free-space floor and one segment: 16,384 MiB
  plus 2,048 MiB plus 64 MiB is 18,496 MiB, about 18.1 GiB, with the default
  floor.
- The free-space floor applies too. `--spool-min-free-mib` (default 2,048) keeps
  that much free on the volume. The spool deletes its oldest data to keep it.
- The disk must take the write rate in the bytes-per-second column while the
  receiver is down. After the outage, the backlog drains at the rate the
  receiver accepts. At 200,000 queries per second, that is about 41 MB per
  second of writes, and then reads at the same rate for replay.
- The send queue is in memory. It holds 128 blocks of about 64 KiB, which is
  about 8 MiB. It absorbs short delays in the sender thread, not an outage. When
  the receiver is down, the sender writes blocks to the spool at once. The queue
  therefore lasts seconds at high rates: about 4 seconds at 10,000 queries per
  second, and about 0.2 seconds at 200,000.

See [The receiver is slow](troubleshooting.md#the-receiver-is-slow) for the
checks when the spool grows.

## Next steps

- [Configuration reference](configuration.md)
- [Log events and counters](monitoring.md)
- [Ring capacity and shutdown](../design/ring-buffer.md)
