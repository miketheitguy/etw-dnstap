# ETW buffer wake validation

> **Status:** Historical · **Evidence dated:** 2026-11-01 (guest local time) · **Commit:** working tree on `69db97d` (the buffer-wake change) · **Reviewed:** 2026-11-01

This record covers the change that wakes the parser once for each delivered ETW
buffer. Before the change, the parser waited for its next timed poll. It
records the cause of the loss, the options that were considered, and the lab
results.

## Cause of the loss

The test used the defaults on DNSTap-WS2025 (6 vCPUs):

- 16 KiB × 12 ETW buffers
- `--etw-flush-seconds 1`
- no `--etw-flush-ms`
- `--capacity 8192`

A `diag-timing` build at 5,000 authoritative QPS recorded these values:

| Counter | Value |
| --- | --- |
| `received` | 499,076 |
| `ring_dropped` | 97,616 (19.6%) |
| `etw_lost` | 0 |
| `wake_len_0` | 1,816 |
| `wake_len_ge_8k_or_full` | 49 |
| `max_run` | 8,192 |

The parser found data on about 49 wakes in 55 s. Each of those wakes found the
queue full.

Conclusion: the real-time session delivers events in one burst for each flush
interval. At 5,000 QPS that is about 10,000 events in each burst. The burst
fills the 8,192-slot queue in 1 to 3 ms. The parser is then in its 20 ms
backoff, which lasts about 31 ms in real time. Microsoft Learn states the
delivery rule: "Events are delivered when the buffers are flushed, not at the
time the provider writes the event" (Logging Mode Constants).

## Options considered

A burst simulation reproduced the lab loss: it gave 18.1%, and the lab showed
19.6%. The simulation then compared the wake options:

| Option | Time to start draining | Loss, 10k-event burst | Loss, 20k-event burst |
| --- | --- | --- | --- |
| Timed park (the old behaviour) | about 16 ms | 18% | 52%-59% |
| High-resolution waitable timer | about 10 ms | 15% | 45%-48% |
| Wake for each ETW buffer, with linger | about 0.04 ms | 0% | 0% |
| Threshold wake from the event callback | about 0.1 ms | 0% | 0% |
| Blocking `recv` | about 0.02 ms | 0% | 0% |

The blocking `recv` option has a cost. It wakes the parser 8,600 to 17,000
times per second, and the ETW thread pays for each wake. That is the
wake-on-enqueue design that [ring-wakeup.md](ring-wakeup.md) rejected.

`SetWaitableTimerEx` gave no benefit. With a high-resolution timer, every
non-zero tolerance fails with error 87. With tolerance 0 it behaves the same as
`SetWaitableTimer`. On the guest, default-resolution timers with a tolerance of
32 ms or more made the waits later.

## Change

- **Buffer callback.** `EVENT_TRACE_LOGFILEW.BufferCallback` is set to
  `intake::native_buffer_callback`. `ProcessTrace` calls it on the consumer
  thread after each delivered buffer. When that buffer enqueued at least one
  event, the callback unparks the parser. The per-event callback still never
  wakes a thread. It only sets a same-thread flag.
- **Linger.** After a drain that found data, the parser polls the queue for up
  to 200 µs before it parks. Inside a burst, the next buffer usually arrives
  within that time.
- **New counters.** `etw_buffers_delivered` and `parser_wake_requests` are reported on
  the `capture stats` and `capture stopped` lines.

## Lab results

All runs used DNSTap-WS2025 with the defaults listed above. The load was
authoritative `dns-stress` for 50 s, and the checkpoint was
`bufwake-pre-2026-11-01`.

| Build | QPS | Received | Dropped | `etw_lost` | Peak queue | Buffers / wakes | Service CPU (s) |
| --- | --- | --- | --- | --- | --- | --- | --- |
| buffer wake, diag | 1,000 | 99,972 | 0 | 0 | 1,814 | 2,210 / 2,210 | 0.125 |
| buffer wake, diag | 5,000 | 499,288 | 0 | 0 | 6,069 | 10,894 / 10,894 | 0.312 |
| buffer wake, diag | 10,000 | 997,045 | 10,728 (1.08%) | 105 | 8,192 | 21,739 / 21,720 | 0.656 |
| before the change, diag | 10,000 | 996,898 | 604,902 (60.7%) | 0 | 8,192 | n/a | 0.375 |
| buffer wake, release | 5,000 | 499,262 / 499,258 | 0 / 0 | 0 | 5,516 / 5,888 | 10,901 / 10,900 | 0.391 / 0.281 |
| `0865e19` release | 5,000 | 499,274 / 499,238 | 93,378 / 90,208 | 0 | 8,192 | n/a | 0.359 / 0.422 |

Notes:

- **Release runs.** The two release builds alternated. The CPU time is within
  noise. At 10,000 QPS the new build encodes 2.5 times as many events, so its
  CPU time is higher.
- **Idle.** An idle 50 s run had 2,046 parks (about 41/s). This is unchanged.
- **Events per buffer.** A 16 KiB buffer held about 46 events.
- **ETW loss.** The buffer-wake run at 10,000 QPS also lost 105 events inside
  ETW (`etw_lost`); the run before the change lost none. The cause is not
  established. Two candidates are the parser's extra CPU use on the busy VM while
  it drains during the burst, and the cost of waking a parked parser on the
  consumer thread. The 1.1% figure counts `ring_dropped` only.
- **Linger.** The linger in the table above was unconditional. The committed
  version turns it off after a linger that found nothing and back on after an
  idle park, so that steady high rates with buffers further apart than 200 µs do
  not spin. See "Adaptive linger" below.
- **10,000 QPS.** The bursts are about 20,000 events. Even with prompt wakes,
  the queue still reaches 8,192, and a small loss remains. The configuration
  fixes still apply: `--etw-flush-ms 50`, or a larger `--capacity`.

## Adaptive linger (after code review)

The code review found a cost in the unconditional linger. At high, steady rates,
ETW delivers buffers one at a time, and the gap between buffers is longer than
200 µs. The parser then spins for nothing after each buffer. The committed
linger is therefore adaptive.

The second lab round used the same VM and defaults. The load was authoritative
`dns-stress` for 50 s, with 4 client threads and 1,024 in flight. Every client
query succeeded, and no query timed out.

| Build | QPS | Received | Ring drops | `etw_lost` | Peak queue | Service CPU (s) |
| --- | --- | --- | --- | --- | --- | --- |
| adaptive, release | 5,000 | 499,596 / 499,654 | 0 / 0 | 0 | 6,095 / 5,877 | 0.312 / 0.281 |
| adaptive, release | 10,000 | 998,514 | 3,175 (0.32%) | 0 | 8,192 | 0.719 |
| adaptive, diag | 50,000 | 4,941,058 / 4,938,757 | 0 / 0 | 1,640 / 5,245 | 5,769 / 4,465 | 8.250 / 8.328 |
| unconditional, diag | 50,000 | 4,941,100 / 4,937,327 | 0 / 0 | 4,798 / 7,829 | 7,693 / 3,676 | 9.750 / 9.359 |

- **CPU.** At 50,000 QPS the adaptive linger used about 13% less service CPU
  than the unconditional one. That is about 1.3 s less in 50 s, or about 2.6% of
  one core.
- **Ring drops.** Neither build dropped events at the ring at 50,000 QPS. At
  that rate ETW delivers buffers continuously.
- **ETW loss.** The remaining loss at 50,000 QPS is inside ETW (`etw_lost`,
  about 0.03%-0.16%). It comes from the default 16 KiB × 12 buffers. The
  documented fix is the high-rate buffer settings.
