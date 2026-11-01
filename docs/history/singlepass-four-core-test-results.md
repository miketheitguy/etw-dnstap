# Single-pass decoder: four-CPU test results

> **Status:** Historical · **Evidence dated:** October 30, 2026 UTC · **Commit:** not recorded. The run predates the commit that added single-pass decoding (`e59425f`, committed October 31 PDT), so its binary is not necessarily any commit in this history. · **Reviewed:** 2026-11-01
>
> **Measurement gap.** Per-counter intake attribution (`ring_dropped`, `pool_exhausted`,
> `receiver_disconnected`, `etw_lost`) and the service settings for this run
> (`--capacity`, `--jumbo-capacity`, `--etw-flush-ms`, `--etw-flush-seconds`,
> `--etw-buffer-kib`, `--etw-max-buffers`) were not recorded. The 0.43% deficit
> below therefore cannot be attributed to ETW, queue, or transport loss, and this
> run is not a flush-setting comparison.
>
> The test plan for this run is [Single-pass four-CPU test plan](singlepass-four-core-test-plan.md).

The client completed all 5,678,344 queries successfully, but collector receipt
reconciliation did not pass: query records were short by 0.42935%, and response
records were short by 0.42843%. Mean total guest CPU was 77.30% on Windows and
27.57% on Ubuntu, normalized across four logical CPUs per VM.

## Workload and counts

One authoritative run requested a global ceiling of 100,000 QPS for exactly
60 seconds, from October 30, 2026, 16:32:06.316 through 16:33:06.316 UTC.
The Windows host ran the unchanged client with 5,000 names, two threads,
256 outstanding requests per thread, and a 2,000 ms timeout. Actual throughput
was 94,639.07 QPS. There was no ramp, retry, or additional validation load.
The nominal 6,000,000 queries exceeded actual sends by 321,656.

| Client reference | Client count | Collector operation | Collector count | Deficit | Deficit percentage |
| --- | ---: | --- | ---: | ---: | ---: |
| Sent queries | 5,678,344 | CLIENT_QUERY | 5,653,964 | 24,380 | 0.42935% |
| Matched responses | 5,678,344 | CLIENT_RESPONSE | 5,654,016 | 24,328 | 0.42843% |
| Successful responses | 5,678,344 | CLIENT_RESPONSE | 5,654,016 | 24,328 | 0.42843% |

Client errors, timeouts, anomaly counters, and final outstanding requests were
zero. The isolated capture contained 11,307,980 selected-zone JSON records and
39 diagnostic lines. The ledger reported no malformed JSON/DNS, out-of-scope
JSON, unexpected operations, unterminated lines, or identical consecutive
records. The latter check does not establish complete message identity.
All captured response records had `NOERROR`.

## Total guest CPU during the sending interval

| VM and logical processor | Mean CPU | Maximum sampled CPU |
| --- | ---: | ---: |
| Windows 0 | 87.72% | 96.92% |
| Windows 1 | 67.46% | 87.60% |
| Windows 2 | 87.58% | 98.45% |
| Windows 3 | 66.44% | 81.35% |
| Windows four-CPU average | 77.30% | 87.60% |
| Ubuntu 0 | 34.49% | 46.53% |
| Ubuntu 1 | 25.98% | 33.33% |
| Ubuntu 2 | 25.59% | 38.78% |
| Ubuntu 3 | 24.23% | 34.00% |
| Ubuntu four-CPU average | 27.57% | 32.15% |

Both analyses cover all 60 seconds with no invalid overlapping samples or gaps.
Windows has 60 overlapping counter intervals, with a largest interval of
1.021194 seconds. Ubuntu has 61, with a largest interval of 1.014082 seconds.
Each sample represents its preceding interval; the calculation excludes the
initial sample and weights the remaining values by sending-window overlap.
Partial boundary intervals assume uniform utilization within each interval.
Peaks are interval averages, not instantaneous spikes. The VM peak is the
largest simultaneous four-core average, not the average of per-core peaks.

Windows uses `Processor Information` processor-time counters. Its `_Total`
agrees with the mean of the four individual CPUs; the processor-group subtotal
is excluded. Ubuntu uses retained `/proc/stat` snapshots. Busy time excludes
idle and iowait; its denominator sums user, nice, system, idle, iowait, irq,
softirq, and steal. Guest and guest_nice are not added again. Steal is included
in busy time and was zero in this window. Mean per-core iowait was 0.02033%.
The Linux kernel documents the fields and cautions that iowait is not a reliable
measure of CPU waiting. See [kernel proc documentation](https://www.kernel.org/doc/html/v6.9/filesystems/proc.html)
and [the Linux proc_stat manual](https://www.man7.org/linux/man-pages/man5/proc_stat.5.html).

The Ubuntu headline averages exactly four individual CPU percentages. The
separate aggregate `/proc/stat` ratio is 27.68895% mean, reflecting a different
tick-weighted calculation and coarse accounting; it is retained in the JSON
analysis rather than substituted for the four-core normalization.

Clock brackets place Windows guest time 0.435–2.957 ms ahead of the host
workload clock; the central offset is 1.696 ms. Ubuntu's six brackets intersect
at 0.419–150.648 ms, with central offset 75.534 ms. Assuming a constant offset,
recomputing at the wider pre/post union endpoints gives Windows means
77.29802–77.30693% and Ubuntu means 27.55656–27.61453%. These are alignment
sensitivity checks, not statistical confidence intervals.

Total guest CPU includes DNS, collector/ETW work, background activity, and
measurement overhead. Ubuntu also compresses the collector console stream
inline with gzip level 1. These values do not measure host physical CPUs or
isolate service CPU. Four-CPU allocation and compressed capture differ from the
preceding two-CPU, uncompressed campaign, so this comparison cannot establish
a decoder-only throughput or CPU improvement. The aggregate deficits do not
identify ETW, queue, transport, or collector loss stages.

## Capture validation and final state

After the entire sending window and client drain, the Windows service stopped
cleanly. The collector received a bounded five-second pre-stop drain, then
closed at 16:33:28.303 UTC; stop took 0.022 seconds. The collector exited with
status zero and closed gzip successfully. The Linux operator streamed the
closed capture once, validating gzip CRC, line termination, counts, decompressed
bytes and SHA-256, and unchanged compressed size/mtime. Independent review
recomputed the host compressed copy's size/hash and directional residuals;
it did not decode the full capture a second time.

The compressed capture contains 345,907,746 bytes with SHA-256
`6b2365f238ab99c7adc69333b0f3f4d0fa8d55e8604ee1a52ab501d109cf967d`.
Its decompressed stream contains 10,971,466,769 bytes with SHA-256
`314b82860a2bfeb37ccad795e1f3dd6e4ead820274163471e084e6d90198fbf5`.
The capture is retained on Ubuntu at
`/var/lib/dnscollector/singlepass-4cpu-100000/console.jsonl.gz`, with a verified
host copy under the campaign's `linux/` directory. Prior captures remain intact.

The independent reconciliation manifest binds the client summary hash and
capture-ledger hash to `singlepass-auth-100000-60s-1`. Scope comes from the
isolated capture and `auth.stress.test` fixture; names were reused, so counts do
not establish individual packet identity.

Both VMs remain running with four CPUs and unchanged memory. The Windows service
runs executable SHA-256
`EB70865D5D36A6E19868E87F2F7A4CAEE2D260B588DD39983097EA51E52803ED`
with an established receiver connection. The collector's normal
`journal+console` output is restored and its temporary capture override removed.
Owned workload and sampling helpers have stopped.

## Evidence

Evidence resides under `target/performance/singlepass-4cpu-100000/`:

- `stages/singlepass-auth-100000-60s-1/client/summary.json`
- `linux/capture-ledger.json`, `linux/end-marker.json`, and `linux/receiver-ready.json`
- `windows/cpu-raw.csv`, `linux/cpu-raw.jsonl`, and `linux/cpu-normalized.csv`
- `independent-cpu-analysis.json` and `independent-reconciliation.json`
- `analyze-evidence.py` and `normalize-linux-cpu.py`
- `windows/final-readback.json` and `linux/normal-output-restored.json`

Sol at medium effort handled Windows deployment/workload and Linux capture;
Sol at low effort wrote the plan and independently calculated both CPU tables
and count residuals. The coordinator verified readiness and final evidence.
