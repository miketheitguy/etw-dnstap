# ETW flush removal validation

> **Status:** Historical · **Evidence dated:** 2026-11-01 (guest local time) · **Commit:** working tree on `ef46c85` (the flush removal) · **Reviewed:** 2026-11-01

This record covers the lab test of the change that removes the explicit
periodic ETW flush (`--etw-flush-ms`). It records what was tested, the client
limit that the runs had to work around, the results with the defaults and with
the new high-rate settings, and the open questions.

The raw run data is lab evidence. It is not in the repository.

## What was tested and why

The [buffer wake](buffer-wake-validation.md) lets the parser drain each ETW
delivery while it arrives. After that change, the explicit flush was not
necessary, so the maintainer removed it. The old high-rate guidance used the
flush:

```text
--etw-flush-ms 50 --etw-max-buffers 256 --etw-buffer-kib 64
```

The new high-rate guidance uses a larger channel instead:

```text
--etw-max-buffers 256 --etw-buffer-kib 64 --capacity 65536
```

Before this test, the new guidance was not lab-tested. The test had two
purposes:

1. Make sure that the build without the flush is stable and correct with the
   defaults, and compare it with the buffer-wake baseline.
2. Measure the new high-rate settings at rates from about 7,000 to about
   50,000 queries per second (QPS).

## Setup

| Item | Value |
| --- | --- |
| Guest | DNSTap-WS2025, 6 vCPU, 4 GiB fixed memory, Windows Server 2025 Standard Evaluation 10.0.26100 |
| Service build | Release build (`cargo build --release --locked`) of the working tree on HEAD `ef46c85`, with the flush removal |
| Service executable | SHA-256 `0C3673E0801FD5431B2F44BF16734A4351DEA88E46C1BF642F7105593E486DCC`. `verify-pe.ps1`: 21 checks, 0 failed |
| Service identity | LocalService, restricted SID |
| Receiver | dnscollector 3.2.3 on DNSTap-Ubuntu2604, plaintext. The service ran with `--plaintext` |
| Client | Host-side `dns-stress` release build, before the pacing fix. SHA-256 `4D545060FA0981DC280168546DFD2D5FEAD88A0B4A3618C6F9F40BC021D3CF3E`, the same in every run |
| Load | Authoritative, 50 s per run, 2,000 ms timeout |
| Checkpoint | `flushremoval-pre-2026-11-01`. It was not applied |

Each run used the same bracket:

1. Restart the service, so that every counter starts at zero.
2. Run `dns-stress`.
3. Wait 4 s, then sample the receiver socket with `ss -tin`.
4. Stop the service, and read the counters from the `capture stopped` and
   `output stopped` lines.

Every run had exactly one `capture stopped` line. Two smoke tests at 100 and
200 QPS came before the ramps. In both, the collector count was equal to
`received`, and `received` was 2 × the queries that the client sent.

Pass 1 used the defaults: `--capacity 8192`, 16 KiB × 12 ETW buffers, and
`--etw-flush-seconds 1`. Pass 2 used the high-rate settings above. The
`capture started` line of every run showed the configured values.

After the runs, the lab restored the binary path to the defaults with
`--plaintext` and left the service running. The receiver configuration did not
change. There were no reboots.

## Client pacing limit

The client did not reach the target rates with the first settings. Each
`dns-stress` worker sleeps until the time of its last send plus one interval.
It does not catch up after a late wake. On this host, the sleep ends on a grid
of about 0.53 ms. A send therefore takes 1, 2, or 3 grid steps. The per-worker
rate is about 1,850 sends/s at 1 step, about 925/s at 2 steps, and about 645/s
at 3 steps. The DNS server did not limit the rate. Every query succeeded,
and no query timed out.

**The achieved QPS is the rate that counts.** The requested `--qps` is only a
ceiling. Every table in this record gives the achieved rate.

The tests used two ramps:

- **First ramp.** 4 threads, and 8 threads for one run in each pass.
  `--inflight 1024`. The 4-thread runs achieved about 7,300 to 7,500 QPS for
  targets of 10,000 and 50,000. The 8-thread runs achieved about 14,700 QPS.
- **Second ramp.** More threads and a calibrated `--qps`. `--inflight` is a
  limit per worker. The runs kept the total near the 4,096 of the first ramp:
  32 threads with 128, 48 threads with 85, and 54 threads with 76. The peak
  number of outstanding queries per worker stayed low, so this limit had no
  effect.

Short client-only probes calibrated `--qps`:

| Threads | `--qps` | Achieved QPS |
| --- | --- | --- |
| 32 | 10,000 | 8,952 to 9,008 |
| 32 | 10,600 | 10,003 to 10,008 |
| 32 | 50,000 | 30,045 to 30,072 |
| 48 | 50,000 | 43,895 to 44,255 |
| 54 | 70,000 | 49,354 to 49,826 |

The second ramp ran three loads as first planned (32 threads at 10,000 and
50,000, and 48 threads at 50,000). It then added two calibrated loads:
`--threads 32 --qps 10600` for about 10,000 QPS, and `--threads 54 --qps 70000`
for about 49,500 QPS.

## End-to-end count

The collector writes one journal line for each dnstap message. journald on the
receiver has a rate limit. It kept about 20,000 lines in each 30 s interval and
suppressed the remainder. The journald count is therefore not correct, and this
record does not use it.

The receiver's TCP byte count replaces it. In 28 runs, the TCP
`bytes_received` of the dnstap connection divided by `encoded` was 103.00 to
103.03 (about 103.01). The value was the same at every rate. Every encoded
frame therefore reached the receiver socket. The service reported 0
`disconnected_dropped` and 0 `output_errors`.

End-to-end loss is therefore `ring_dropped` + `etw_lost`.

## Results with the defaults

`--capacity 8192`, 16 KiB × 12 ETW buffers, `--etw-flush-seconds 1`.

| Achieved QPS | Threads / `--qps` | Received | Ring drops (%) | `etw_lost` (%) | Peak queue | Service CPU (s) | Private MiB |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 959 | 4 / 1,000 | 95,934 | 0 | 0 | 1,640 | 0.219 | 31.1 |
| 7,260 | 4 / 10,000 | 725,984 | 575 (0.079%) | 0 | 8,187 | 0.469 | 31.1 |
| 7,281 | 4 / 10,000 | 728,070 | 0 | 0 | 6,871 | 0.422 | 31.2 |
| 7,443 | 4 / 50,000 | 744,286 | 888 (0.119%) | 0 | 8,192 | 0.344 | 31.2 |
| 7,468 | 4 / 50,000 | 746,798 | 0 | 0 | 7,998 | 0.484 | 31.1 |
| 8,984 | 32 / 10,000 | 898,424 | 2,507 (0.279%) | 0 | 8,192 | 0.719 | 31.1 |
| 9,008 | 32 / 10,000 | 900,844 | 3,877 (0.430%) | 0 | 8,193 | 0.641 | 31.2 |
| 10,003 | 32 / 10,600 | 1,000,338 | 5,515 (0.551%) | 0 | 8,193 | 0.766 | 31.1 |
| 10,008 | 32 / 10,600 | 1,000,764 | 2,855 (0.285%) | 0 | 8,192 | 0.656 | 31.1 |
| 14,689 | 8 / 50,000 | 1,468,894 | 23,975 (1.632%) | 0 | 8,193 | 0.688 | 31.1 |
| 30,045 | 32 / 50,000 | 3,001,607 | 496,873 (16.55%) | 2,893 (0.096%) | 8,193 | 1.797 | 31.1 |
| 30,072 | 32 / 50,000 | 3,002,406 | 434,282 (14.46%) | 4,804 (0.160%) | 8,193 | 1.844 | 31.1 |
| 43,958 | 48 / 50,000 | 4,389,862 | 98,062 (2.234%) | 5,928 (0.135%) | 8,193 | 7.562 | 31.1 |
| 49,522 | 54 / 70,000 | 4,941,509 | 16,937 (0.343%) | 10,663 (0.215%) | 8,193 | 8.672 | 31.1 |
| 49,533 | 54 / 70,000 | 4,942,663 | 61,169 (1.238%) | 10,639 (0.215%) | 8,193 | 8.422 | 31.1 |

The percentages are of `received`. The peak queue is `queue_depth_max`, a
sampled lower bound. It counts the event that the parser is processing, so a
peak of 8,192 or 8,193 means that the channel was full.

- At about 7,300 to 7,500 QPS, 2 of 4 runs dropped events (0.08% and 0.12%).
- At about 9,000 QPS and above, every run dropped events, and the channel was
  full in every run.
- `etw_lost` was 0 up to about 14,700 QPS. It was 0.10% to 0.22% from about
  30,000 QPS.
- The 30,000 QPS runs lost the most. See
  [Open question: loss at about 30,000 QPS](#open-question-loss-at-about-30000-qps).

## Results with the high-rate settings

`--etw-max-buffers 256 --etw-buffer-kib 64 --capacity 65536`.

| Achieved QPS | Threads / `--qps` | Received | Ring drops (%) | `etw_lost` (%) | Peak queue | Service CPU (s) | Private MiB |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 7,325 | 4 / 10,000 | 732,452 | 0 | 0 | 8,201 | 0.516 | 178.6 |
| 7,326 | 4 / 10,000 | 732,598 | 0 | 0 | 8,876 | 0.406 | 178.7 |
| 7,455 | 4 / 50,000 | 745,486 | 0 | 0 | 8,616 | 0.375 | 178.7 |
| 7,456 | 4 / 50,000 | 745,630 | 0 | 0 | 8,540 | 0.344 | 178.7 |
| 8,986 | 32 / 10,000 | 898,556 | 0 | 0 | 9,375 | 0.594 | 178.7 |
| 8,987 | 32 / 10,000 | 898,724 | 0 | 0 | 9,787 | 0.328 | 178.7 |
| 10,000 | 32 / 10,600 | 1,000,036 | 0 | 0 | 10,627 | 0.328 | 178.7 |
| 10,005 | 32 / 10,600 | 1,000,452 | 0 | 0 | 10,295 | 0.750 | 178.7 |
| 14,681 | 8 / 50,000 | 1,468,120 | 0 | 0 | 16,776 | 0.812 | 178.6 |
| 29,988 | 32 / 50,000 | 2,998,756 | 0 | 0 | 27,329 | 1.984 | 178.7 |
| 30,017 | 32 / 50,000 | 3,001,740 | 0 | 0 | 28,226 | 1.703 | 178.7 |
| 43,895 | 48 / 50,000 | 4,389,462 | 0 | 0 | 30,643 | 3.953 | 178.7 |
| 49,354 | 54 / 70,000 | 4,935,414 | 0 | 0 | 46,408 | 4.406 | 178.7 |
| 49,416 | 54 / 70,000 | 4,941,602 | 13,698 (0.277%) | 0 | 65,536 (full) | 4.406 | 178.7 |

- `etw_lost` was 0 in all 14 runs.
- The channel dropped no events at every rate up to about 44,000 QPS. The peak
  queue at about 44,000 QPS was 30,643, which is 47% of 65,536.
- At about 49,400 QPS, one of two runs filled the channel and dropped 0.28%.
  The other run peaked at 46,408 (71%).
- At the same load, the defaults dropped 1.63% at about 14,700 QPS and 14% to
  17% at about 30,000 QPS. The high-rate settings dropped nothing at those
  rates.
- Service CPU was the same as or lower than with the defaults. At about
  49,500 QPS it was about 4.4 s for each 50 s run, against 8.4 to 8.7 s with
  the defaults. ETW delivered about one quarter as many buffers (about 26,600
  against about 107,500), because each 64 KiB buffer holds about 4 times as
  many events.
- Private bytes rose from about 31 MiB to about 179 MiB. The peak working set
  was 191 to 255 MiB. Most of the increase is the pool for the 65,536-slot
  channel.
- Startup with `--capacity 65536` reached `capture started` in under 0.1 s.
  The startup deadline at that capacity is 14 s.

The sizing rule in the configuration reference gives one delivery as up to
`2 × queries/s × --etw-flush-seconds` events. At about 44,000 QPS that is about
88,000 events. The measured peak was 30,643, because the parser drains the
channel while a delivery arrives. The rule is a safe upper bound, not a
prediction.

## Counters that stayed at zero

In all runs of both passes:

- `pool_exhausted_regular`, `pool_exhausted_jumbo`, `etw_buffers_lost`, and
  `receiver_disconnected` were 0.
- `disconnected_dropped`, `receiver_disconnects`, `output_errors`, and
  `slow_writes` were 0.
- Client timeouts and send errors were 0.
- `intake_dropped` was equal to `ring_dropped`.
- `encoded` and `dequeued` were equal to `received` − `ring_dropped`.
- `received` was exactly 2 × the queries that the client sent when `etw_lost`
  was 0. It was 0.998 to 0.999 × that number when ETW lost events.
- `parser_wake_requests` was within 0.01% to 3% of `etw_buffers_delivered`.

The largest socket write (`write_max_us`) was 78 to 397 µs. Two default runs
had larger values: 993 µs at about 49,500 QPS and 3,170 µs at about 30,000 QPS.
Both are below the slow-write threshold.

## Comparison with the buffer-wake baseline

The baseline is the [buffer wake validation](buffer-wake-validation.md), on
the same VM with the defaults and 50 s runs. The baseline also ran without the
flush (`--etw-flush-ms` was not set).

| Load | Baseline | This build, defaults | This build, high-rate settings |
| --- | --- | --- | --- |
| About 10,000 QPS | 9,985 QPS, release: 0.32% ring drops, `etw_lost` 0, CPU 0.719 s | 10,003 and 10,008 QPS: 0.55% and 0.29% ring drops, `etw_lost` 0, CPU 0.77 and 0.66 s | 0 ring drops, `etw_lost` 0, peak queue 10,295 and 10,627 |
| About 30,000 QPS | Not run | 14.5% and 16.6% ring drops, `etw_lost` 0.10% to 0.16%, CPU 1.8 s | 0 ring drops, `etw_lost` 0, peak queue 27,329 and 28,226 |
| About 44,000 QPS | Not run | 2.23% ring drops, `etw_lost` 0.135%, CPU 7.6 s | 0 ring drops, `etw_lost` 0, peak queue 30,643, CPU 3.95 s |
| About 49,500 QPS | 49,400 QPS, diag build: 0 ring drops, `etw_lost` 0.03% to 0.11%, CPU 8.25 and 8.33 s | 0.34% and 1.24% ring drops, `etw_lost` 0.215%, CPU 8.42 and 8.67 s | 0 and 0.28% ring drops, `etw_lost` 0, CPU 4.41 s |

- At about 10,000 QPS, the defaults gave the same result as the baseline.
- At about 49,500 QPS, the service CPU with the defaults was equal to the
  baseline. The ring drops and the `etw_lost` were higher than in the baseline.
  The baseline used a diag build and 4 client threads. This test used a release
  build and 54 client threads with a different pacing grid. The burst shape is
  therefore not the same. This record does not show which difference causes
  the higher loss.

## Open question: loss at about 30,000 QPS

With the defaults, the runs at about 30,000 QPS dropped 14.5% and 16.6% at the
channel. The runs at about 44,000 and 49,500 QPS dropped only 0.34% to 2.23%.
Both 30,000 QPS runs gave the same result. The client was steady: each worker
sent about 944 queries/s in every 1 s sample. The service CPU was 1.8 s at
about 30,000 QPS, against 7.6 to 8.7 s at 44,000 to 49,500 QPS.

**Hypothesis (not confirmed).** At about 30,000 QPS, the parser parks between
ETW buffers, and the 8,192-slot channel overflows while the parser does not
run. At 44,000 QPS and above, the parser stays busy and drains continuously.
The lab did not investigate this. [Delivery batches](../design/ring-buffer.md#delivery-batches)
records a related earlier observation. ETW delivered one batch per second at
10,000 and 20,000 QPS. It delivered buffers continuously at 50,000 QPS. The
threshold between the two modes is not known.

The high-rate settings removed this loss: 0 drops, with a peak queue of 27,329
and 28,226.

The [fixed client check](#fixed-client-check) gave the same loss at about
30,000 QPS with a different pacing method and 4 threads. The client is
therefore not the cause. Service CPU per million events was about 0.6 s at
10,000 and 30,000 QPS, and about 1.3 to 1.7 s at 50,000 QPS. This agrees with
batch delivery at 30,000 QPS and continuous delivery at 50,000 QPS. It does not
show the parser's park and wake latency. A `diag-timing` build, or per-second
`ring_dropped` samples at 30,000 QPS, can settle the cause.

## Fixed client check

After the runs above, `dns-stress` was changed to send on an absolute schedule
and to catch up after a late wake, with a backlog limit of 100 ms. The fixed
client was run against the same service build with the defaults. The client
change is the commit before the flush removal. The client exe SHA-256 was
`82CFB758153DB8BF2E00E0FB25E932B84154DB2737B8D52CBA05A3D4E61D38A6`.
`--inflight` is per worker.

| Achieved QPS | Threads × in-flight / `--qps` | Received | Ring drops (%) | `etw_lost` (%) | Peak queue | Service CPU (s) |
| --- | --- | --- | --- | --- | --- | --- |
| 10,000 | 4 × 1,024 / 10,000 | 1,000,000 | 5,793 (0.58%) | 0 | 8,193 | 0.594 |
| 30,000 | 4 × 1,024 / 30,000 | 2,996,567 | 369,256 (12.32%) | 3,427 (0.114%) | 8,193 | 1.891 |
| 30,000 | 32 × 128 / 30,000 | 2,996,351 | 498,321 (16.63%) | 3,649 (0.122%) | 8,193 | 1.781 |
| 50,000 | 4 × 1,024 / 50,000 | 4,989,999 | 219,008 (4.39%) | 9,961 (0.199%) | 8,193 | 6.609 |

- The fixed client reached every target rate with 4 threads.
  `pacing_slots_skipped`, timeouts, and send errors were 0 in every run.
- Every encoded frame reached the receiver socket. The receiver bytes per
  encoded frame were 103.01 to 103.03.
- At 1 s resolution, the fixed client was steadier than the old client. At
  30,000 QPS the aggregate rate was 29,988 to 30,012 per second with 4 threads.
  The old 32-thread client gave 29,831 to 30,275 per second.
- The loss at about 30,000 QPS occurred again: 12.32% with 4 threads and 16.63%
  with 32 threads. The old client lost 14.46% and 16.55%. The ETW buffer counts,
  wake requests, and service CPU were also the same as with the old client. The
  old client's pacing is therefore not the cause.
- At 50,000 QPS with 4 threads, the defaults dropped 4.39%. The old client with
  54 threads at about 49,500 QPS dropped 0.34% to 1.24%. The result with the
  defaults at 50,000 QPS therefore depends on the client's burst shape.
- The high-rate settings were not run with the fixed client.

## Conclusions

1. **Stability.** The build without the explicit flush is stable and correct.
   No run had a disconnect, an output error, a timeout, or a crash. Every
   encoded frame reached the receiver socket.
2. **Defaults.** The 8,192-event channel drops events at about 9,000 QPS and
   above. It dropped 0.28% to 0.58% at about 10,000 QPS and 12% to 17% at
   about 30,000 QPS. At about 49,500 QPS it dropped 0.34% to 1.24%, and
   `etw_lost` was 0.215%. With the fixed client at 50,000 QPS, it dropped
   4.39%, and `etw_lost` was 0.199%.
   Two of four runs at about 7,300 to 7,500 QPS also dropped 0.08% to 0.12%.
   The defaults are for about 1,000 QPS or less.
3. **High-rate settings.** `--etw-max-buffers 256 --etw-buffer-kib 64
   --capacity 65536` gave 0 `etw_lost` in all runs. It gave 0 ring drops at
   every rate up to about 44,000 QPS. At a sustained 49,400 QPS, one of two runs
   filled the channel and dropped 0.28%. A capacity of 65,536 is therefore
   enough up to about 44,000 QPS on this VM, and marginal at a sustained
   50,000 QPS.
4. **Larger capacity.** For sustained rates near 50,000 QPS, a larger
   `--capacity`, for example 131,072, is the next step. The lab did not test
   it.
5. **Cost.** The high-rate settings raise private bytes from about 31 MiB to
   about 179 MiB. They use the same or less service CPU than the defaults.

The lab did not test these cases:

- Other machines and vCPU counts.
- Recursive or mixed workloads.
- Runs longer than 50 s.
- A TLS receiver. All runs used a plaintext receiver.
- An `--etw-flush-seconds` value other than 1.
- `--capacity 131072`.
