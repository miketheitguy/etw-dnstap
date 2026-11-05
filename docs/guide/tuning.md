# Tune capture for load

> **Status:** Current · **Describes:** the code at commit `66c3de4` · **Reviewed:** 2026-11-05

Choose `--etw-*`, `--capacity`, and `--spool-max-mib` for your query rate. Option ranges are in the
[configuration reference](configuration.md#options), loss counters in
[Log events and counters](monitoring.md), and lab results in
[Guide lab results, 2026-11](../history/guide-lab-results-2026-11.md) and
[docs/history/](../history/).

## ETW buffer minimums

`--etw-min-buffers` is the requested minimum. ETW reserves it at session start, as the
minimum times `--etw-buffer-kib`, and allocates more buffers on demand up to
`--etw-max-buffers`. Raising the maximum allocates nothing up front.

- `0` lets ETW choose.
- ETW raises the minimum to at least two buffers per logical processor, so the default of 12
  has an effect only on a host with fewer than six logical processors. ETW may also lower a
  minimum above its own limit. The maximum rises to at least the adjusted minimum.
- On a host with more than 32 logical processors, ETW's minimum is above the default maximum of
  64, so the maximum rises to match. Peak buffer memory can exceed the requested bound.
- A minimum above `--etw-max-buffers` stops the service with
  `ETW minimum buffers (65) must not exceed ETW maximum buffers (64)`.
- A new value takes effect at the next start.

Microsoft advises setting the minimum and the maximum to the same value when you start tuning.
Raise the maximum if the session drops events during peaks.

When ETW applies other values than requested, the service writes `ETW buffers applied` with
`etw_min_buffers_applied`, `etw_max_buffers_applied`, and `etw_buffers_allocated`. The
`ETW Buffers Allocated` counter shows the effective minimum just after start.

## Defaults

<a id="settings-for-200000-queries-per-second"></a>

The defaults are `--etw-buffer-kib 32 --etw-min-buffers 12 --etw-max-buffers 64
--capacity 131072`: 32 KiB buffers, at least 12 of them and at most 64, and a channel of
131,072 events. They are sized for 10,000 queries per second, which is about 20,000 events
per second and 2.06 MB per second of dnstap frames.

Above about 10,000 queries per second, set the values for your rate. See
[Recommended settings for higher rates](#recommended-settings-for-higher-rates).

> **Tested on one guest.** The lab ran these values at 10,000 queries per second on a Windows
> Server 2025 guest with 12 vCPUs and 8 GiB, with no event lost in 5 runs (3 idle, 2 with 12
> CPU-busy threads). Other machines and loads can differ. Watch `ring_dropped` and `etw_lost`.
> The run record is in
> [Default and tier sizing runs on 12 vCPUs](../history/slow-receiver-validation.md#default-and-tier-sizing-runs-on-12-vcpus-2026-11-04).

| Setting | Largest observed at 10,000 queries per second | Default |
| --- | --- | --- |
| ETW buffer size (`--etw-buffer-kib`) | 32 KiB, no loss | 32 |
| Maximum ETW buffers (`--etw-max-buffers`) | 28 allocated (44 % of 64), at most 13 in use at once | 64 (2 MiB) |
| Minimum ETW buffers (`--etw-min-buffers`) | ETW applied 24 on 12 vCPUs, so an idle session allocated 24 | 12 |
| Channel (`--capacity`) | Peak queue 35,072 (27 % of 131,072), in a busy run | 131,072 |

![Graphs of the 10,000 queries-per-second runs: dns.exe CPU, the CPU of each EtwDnstap pipeline stage, and loss by failure point. No run lost events.](../assets/performance/tiers-2026-11-04/tier-10000.png)

The service used about 1 % of one CPU. dns.exe used 20 to 34 % on average.

Why: 64 buffers is more than twice the observed peak. ETW sometimes delivers events late in one
burst, so the default channel keeps a margin over the 27 % peak. The minimum is explained under
[ETW buffer minimums](#etw-buffer-minimums).

Costs: ETW buffer memory is at most 2 MiB (768 KiB reserved at start on a 12-vCPU guest). Counted
storage at the default capacity is 283.03 MiB ([Storage](configuration.md#storage)), and the working
set is about 356 to 400 MB. The startup deadline grows with `--capacity`
([Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines)).

**Warning.** With the default ETW buffer size, buffer counts, flush interval, and `--capacity`,
the service logs one warning per run (Event ID 3) at the end of the first measurement window
(`--stats-interval-seconds`, or 60 seconds when `0`) in which the received rate exceeds 22,000
events per second: `received rate is above 22,000 events/s; the default buffer settings were
validated at about 20,000 events/s (10,000 queries per second)`. It does not mean events were
lost. Watch `ring_dropped` and `etw_lost` (Event ID 11 and Event ID 10,
[Loss and risk warnings](monitoring.md#loss-and-risk-warnings)). If they rise, use
[Recommended settings for higher rates](#recommended-settings-for-higher-rates).

## Recommended settings for higher rates

**These values are not defaults.** Pass them in the MSI property `SERVICE_ARGS` (see
[Install](install.md)). Any ETW value or `--capacity` that differs from its default turns off the
[rate warning](#defaults).

Every tier uses `--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128`. Only
`--capacity` changes:

| Queries per second | `--capacity` | Largest peak channel depth | Service CPU (% of one CPU) | Peak working set |
| --- | --- | --- | --- | --- |
| 50,000 | 131,072 | 70,323 (54 %) | about 9 % | about 0.4 GB |
| 100,000 | 524,288 | 251,438 (48 %) | 13 to 17 % | 1.2 to 1.4 GB |
| 200,000 | 262,144 | 136,236 (52 %, busy run) | 33 to 39 % | about 0.7 GB |

No tier run with the listed `--capacity` dropped an event (4 runs each, on the 12-vCPU guest). The
record is in [Default and tier sizing runs on 12 vCPUs](../history/slow-receiver-validation.md#default-and-tier-sizing-runs-on-12-vcpus-2026-11-04).

### 50,000 queries per second

```text
--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128 --capacity 131072
```

![Graphs of the 50,000 queries-per-second runs: dns.exe CPU, the CPU of each EtwDnstap pipeline stage, and loss by failure point. No run lost events.](../assets/performance/tiers-2026-11-04/tier-50000.png)

### 100,000 queries per second

```text
--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128 --capacity 524288
```

ETW delivered events late, in gaps of 1 to 3 seconds followed by a burst. A channel of 524,288
absorbs the bursts. Smaller channels lost events: 65,536 dropped 0.89 to 1.40 million, 131,072
dropped 0.31 to 0.46 million, and 262,144 dropped up to 0.16 million (4 of 5 runs).

See [Event channel 262,144 and 524,288](../history/slow-receiver-validation.md#event-channel-262144-and-524288-at-100000-qps-2026-11-04).

![Graphs of the 100,000 queries-per-second runs: dns.exe CPU, the CPU of each EtwDnstap pipeline stage, and loss by failure point. No run lost events.](../assets/performance/tiers-2026-11-04/tier-100000.png)

### 200,000 queries per second

```text
--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128 --capacity 262144
```

With `--capacity 131072`, the busy run dropped 63,443 events (0.27 %) at the channel while ETW lost
nothing, so this tier uses 262,144. See
[200,000 QPS validation](../history/200k-qps-validation.md).

![Graphs of the 200,000 queries-per-second runs: dns.exe CPU, the CPU of each EtwDnstap pipeline stage, and loss by failure point. The runs with a 262,144-event channel lost no events.](../assets/performance/tiers-2026-11-04/tier-200000.png)

### Sizing for other rates

- One delivery holds up to about `2 × queries/s × --etw-flush-seconds` events. Keep `--capacity`
  well above this. The measured peak was far lower, so treat it as a safe upper limit.
- A longer `--etw-flush-seconds` makes each delivery larger. Raise `--capacity` by the same
  factor.
- Larger ETW buffers reduce `etw_lost`: 64 KiB × 256 buffers removed the `etw_lost` that smaller
  buffers had at 30,000 queries per second and above. With `--etw-max-buffers 32` and
  `--capacity 24576`, the lab lost events at 200,000 queries per second; see
  [the lab record](../history/guide-lab-results-2026-11.md#200000-queries-per-second-with-32-buffers-and-a-24576-event-channel).
- If `ring_dropped` rises at a rate well below the tier, check the delivery gaps in the lab
  write-up before you raise `--capacity` further.

## Lower rates

The defaults suit 10,000 queries per second and below. At a lower rate they use more memory than
the load needs, mostly in the channel. Changing any ETW buffer value, the flush interval, or
`--capacity` turns off the [rate warning](#defaults).

### Smaller settings, tested at 10,000 queries per second

<a id="low-rate-defaults"></a>

For a host with a low rate, use `--etw-buffer-kib 16 --etw-max-buffers 64 --capacity 24576`
with `--etw-min-buffers 0`: 16 KiB buffers, at most 64 of them, and a channel of 24,576 events.
They use 59.59 MiB of counted storage, a 12-second startup deadline, and at most 1 MiB of ETW
buffers.

A 60-second run at 10,000 queries per second had no ring drops and no `etw_lost`. See
[60-second run](../history/guide-lab-results-2026-11.md#60-second-run-with-the-defaults-at-10000-queries-per-second)
and [ETW buffer wake validation](../history/buffer-wake-validation.md).

Not tested: other machines and vCPU counts, recursive or mixed workloads, runs longer than
50 seconds, and TLS receivers. The lab used plaintext receivers.

## Spool sizing

`--spool-max-mib` sets how long an outage the spool can cover. The figures use 103 bytes per
frame (the small-answer average, with the 4-byte length prefix) and two frames per query. Larger
answers make larger frames and shorter times.

| Queries per second | Frames per second | Bytes per second | MiB per hour | Time to fill the default 4,096 MiB cap |
| --- | --- | --- | --- | --- |
| 1,000 | 2,000 | 206,000 | 707 | about 5.8 hours |
| 10,000 | 20,000 | 2,060,000 | 7,072 | about 35 minutes |
| 50,000 | 100,000 | 10,300,000 | 35,362 | about 7 minutes |
| 200,000 | 400,000 | 41,200,000 | 141,449 | about 104 seconds |

These are calculations, not measured spool outages.

While the receiver drains a backlog, the service holds up to about 8.7 MiB it has read but the
receiver has not accepted (127 batches of at most 71,684 bytes). Those segments stay whole, count
toward the cap and in `spool_bytes`, and can raise the total to 24.7 MiB at the smallest cap and
about 137 MiB at caps of 512 MiB and above.

- Size the cap for the longest outage you must cover. Multiply MiB per hour by the hours, and
  add a margin. For 10,000 queries per second and a two-hour outage, the spool needs about
  14,144 MiB, so set `--spool-max-mib 16384`.
- The volume must hold the cap, the `--spool-min-free-mib` floor, and one 64 MiB segment:
  16,384 + 2,048 + 64 = 18,496 MiB (about 18.1 GiB).
- During an outage, the disk takes writes at the bytes-per-second rate, then reads at the rate
  the receiver accepts. At 200,000 queries per second that is about 41 MB per second.
- The in-memory send queue (256 blocks, about 16 MiB) absorbs short router delays and late ETW
  bursts, for about 8 seconds at 10,000 queries per second and 0.4 seconds at 200,000. The link
  FIFO adds 256 blocks (about 16 MiB) for a slow receiver. Neither covers an outage: the router
  sends blocks to the spool while the receiver is down.

See [The receiver is slow](troubleshooting.md#the-receiver-is-slow) for the steps to take when
the spool grows.

## Next steps

- [Configuration reference](configuration.md)
- [Log events and counters](monitoring.md)
- [Ring capacity and shutdown](../design/ring-buffer.md)
