# Ring-buffer capacity fix: validation

> **Status:** Superseded by [Pooled ETW intake](../design/pooled-intake.md). This report validates the allocation-based `ArrayQueue` candidate, which is no longer the transport. · **Evidence dated:** October 29, 2026 UTC · **Commit:** not recorded · **Reviewed:** 2026-11-01

This historical report helps maintainers assess the allocation-based
capacity-only candidate measured on October 29, 2026 UTC. Its binary, test
counts, and installed-state descriptions apply at the recorded restoration
cutoff. The [pooled VM review](../performance/pooled-vm-performance-review.md) records later
validation of a different implementation; these results do not validate its
pool or jumbo admission limits.

This record describes historical binaries with service JSONL metrics
instrumentation. The TCP-only service has no durable metrics export; its
instrumentation and performance have not been measured by this campaign.
See the [instrumentation compatibility note](../README.md#instrumentation-compatibility-note). Preserved counters,
tables, hashes, and collector artifacts retain their historical meaning.

**Capture target passed:** both 300-second runs at approximately global 1,000
QPS had zero ring drops and exact end-to-end record counts. **The full DNS-client
validity gate was not met:** the recursive run had one RA-check failure. Its
cause remains unknown; no unqualified all-gates pass is claimed.

## Final candidate and rejected experiment

The capacity-only v2 installed for this campaign changes the default ring
capacity from 1,024 to 8,192 and
retains the original enqueue/polling behavior. The wake-only experiment at
1,024 did not eliminate ring drops and was reverted. It remains preserved as
a failed experiment, not hidden or accepted as the fix.

| Artifact | SHA-256 |
| --- | --- |
| Original service | `0FCDD17E08A3DDD42F4F132F7F03F80A5513FEBC66E54F90A5EBBC8135BE2C1D` |
| Rejected wake v1 | `75DD2336BCED33F115679D3D9296AB9984A84604EE8EACDD5AD40A3144A824DE` |
| Capacity-only v2 | `CA3342D848BD06B7B8E1E6CE9ADC05E2157C3E86A18980E69E74B16555264347` |
| Unchanged Rust client | `7A5800CFC4252196E4C6162F7D2CCF81217FE95A339D7CFBB9CD84063F4B6960` |

The full SHA-256 of the capacity-only v2 binary is also in [`sha256.txt`](../../datasets/baselines/arrayqueue-v2/sha256.txt). The `etw-dnstap.exe` beside that file is local only: git ignores `*.exe`, so it is not in a clone. Use the hash file to check a local copy.

The [plan](ring-buffer-fix-plan.md) records the delegated models/efforts,
approved matrix, revision after the failed gate and restoration requirements.
Sol/high workers authored code/scripts/docs; Astra/high performed coordination
and independent read-only source/security/evidence review. V2 passed 33 normal
and 33 ASan tests, formatting, strict Clippy, locked release, actual PE hardening
and static-CRT checks. The executable ACL is unchanged after deployment.
The default 8,192 is confirmed by CLI help, unit tests and explicit measured-run
arguments. No foreground mode, privilege expansion, timer-resolution change,
cache-policy change, VM resize or unrelated network change was made.

The Windows VM remains `DNSTap-WS2025`, GUID
`24bafedb-0998-4430-a0c6-7e90350cd9ac`, `172.27.215.163`, 2 vCPUs and 4 GiB RAM.
All runs use the same 5,000 common names, two client workers, 256 maximum
outstanding requests each, 2,000 ms deadline and **global** `--qps 1000` cap.
Actual sends/rate are measured, not assumed to equal 30,000/300,000. Recursion
uses the controlled TTL-zero upstream and existing Windows cache policy;
actual upstream counts define the event denominator.

## Matched 30-second comparisons

All six arms use PerfMon and metrics with **no WPR**, the same client/settings,
and fresh receiver counters/logs. These fixed-order, single-shot short runs
are exploratory rather than a reliable estimate of run-to-run variance.

| Mode / arm | Client sent | Received | Encoded | Ring drops | High water |
| --- | ---: | ---: | ---: | ---: | ---: |
| Authoritative, original / 1,024 | 29,957 | 59,914 | 36,264 | 23,650 | 1,024 |
| Authoritative, original / 8,192 | 29,990 | 59,980 | 59,980 | 0 | 3,022 |
| Authoritative, wake v1 / 1,024 | 29,969 | 59,938 | 35,596 | 24,342 | 1,024 |
| Recursive, original / 1,024 | 29,965 | 90,040 | 42,393 | 47,647 | 1,024 |
| Recursive, original / 8,192 | 29,987 | 90,080 | 90,080 | 0 | 4,972 |
| Recursive, wake v1 / 1,024 | 29,938 | 90,230 | 45,289 | 44,941 | 1,024 |

ETW EventsLost, real-time buffers lost, callback errors, parser rejections,
output errors and final backlog were zero in all six. Every encoded count
matched exact receiver JSON and ingress/egress, with zero downstream discards,
background records or malformed data. Original/8,192 observed occupancies
above 1,024 show that the headroom was used; they do not prove all 8,192 entries
were necessary or guarantee the later five-minute result.

The wake recursive diagnostic had one direct-client RA-check failure:
29,938 matched NOERROR responses but 29,937 classified successful. The receiver
also retained one explicit RA=false/AA=true answer. Its cause is
unknown; it is separate from capture loss and was not hidden by extra queries.
That run was diagnostic only, never a passing acceptance result.

Actual recursive upstream queries/replies were 15,055/15,055 for original
1,024, 15,053/15,053 for original 8,192 and 15,177/15,177 for wake v1.
The count ratios to client requests were approximately 50.24%, 50.20% and
50.69%; this repeated-name workload is not one upstream lookup per request.

### Observed CPU impact

The capacity increase had the more consistent **observed increase in total
service CPU**, approximately +0.42 percentage points of one CPU in each mode.
It also encoded 65% more authoritative records and 112% more recursive records
than the lossy original-1,024 arms. This is not evidence that larger capacity
is intrinsically less efficient per processed record. Wake-only CPU changes
had inconsistent signs, so these trials do not establish a reliable causal
CPU-efficiency ranking or a general CPU saving from notification.

| Mode / arm | Mean CPU, one-core % | CPU-seconds / valid interval seconds | Estimated CPU ms / offered query | Estimated CPU µs / encoded record |
| --- | ---: | ---: | ---: | ---: |
| Authoritative, original / 1,024 | 0.5298 | 0.1563 / 29.497 | 0.00531 | 4.383 |
| Authoritative, original / 8,192 | 0.9546 | 0.2813 / 29.462 | 0.00955 | 4.775 |
| Authoritative, wake v1 / 1,024 | 0.5903 | 0.1719 / 29.115 | 0.00591 | 4.975 |
| Recursive, original / 1,024 | 0.9443 | 0.2812 / 29.781 | 0.00945 | 6.682 |
| Recursive, original / 8,192 | 1.3652 | 0.4001 / 29.307 | 0.01366 | 4.547 |
| Recursive, wake v1 / 1,024 | 0.4269 | 0.1250 / 29.282 | 0.00428 | 2.828 |

Compared with original/1,024, capacity-only changes were +0.4249/+0.4209
percentage points (authoritative/recursive); wake-only changes were
+0.0606/−0.5174 points. Divide one-core percentages by two for this VM's CPU
capacity. Relative changes from small baselines are less useful than these
absolute values.

Primary CPU means are duration weighted over valid sending-window intervals;
the integral and coverage are shown separately. Work-normalized values are
**rate-based estimates**, calculated from mean CPU fraction divided by full-run
achieved query or encoded-record rate. They assume representative coverage
and approximate final-drain/interval boundaries. Partial-coverage CPU integrals
are not divided directly by full-run record totals. The process includes
callback/output/metrics overhead, not just parser CPU. Quantization and only
0.125–0.400 CPU-seconds of work per arm limit inference from single 30-second
runs. No additional repeats were required for fix selection: wake failed the
loss target, while capacity passed its short diagnostics.

`target/performance/ring-fix-20261029/cpu-comparison.json` preserves the full
arithmetic, actual service command/hash, DNS/guest CPU, encoded work and memory.

## Official capacity-only v2 acceptance

Run IDs are `ringfix-capacity8192-auth-300s-1` and
`ringfix-capacity8192-rec-300s-1`. Both require full 300-second sending windows
near global 1,000 QPS, zero ring/callback/parser/output/ETW loss, valid final
statistics, zero backlog and exact receiver reconciliation. Resources use the
client sending window; final ledgers include the later native/parser drain.

| Sending window (UTC, 2026-10-29) | Sent / matched / successful | Actual QPS | p95 / p99 upper bound, ms | Timeouts / RA failures |
| --- | ---: | ---: | ---: | ---: |
| Authoritative 07:51:45.853–07:56:45.853 | 299,639 / 299,639 / 299,639 | 998.80 | 0.225279 / 0.393215 | 0 / 0 |
| Recursive 07:59:48.783–08:04:48.783 | 299,867 / 299,867 / 299,866 | 999.56 | 0.557055 / 0.770047 | 0 / 1 |

Both sending windows were exactly 300 seconds by the client's monotonic clock;
wall-clock drain-end timestamps can be a few milliseconds earlier than nominal
UTC end because of clock adjustment/rounding. Maximum matched latency was
9.0633/8.3806 ms. These histogram percentile values are upper bin bounds for
matched responses, exclude timeouts, and have no coordinated-omission correction.
All responses were NOERROR with answers; the single recursive RA failure is
still a validation failure. There were no send/receive/malformed/mismatched,
AA/RD/truncation or empty-answer errors. Client pacing resets from actual send
time, so it does not catch up missed sends; two workers can produce two-packet
microbursts. The global cap is not an exact 300,000-send promise.

### Final capture and delivery ledger

The final runs produced the following capture and delivery ledger:

| Mode | Received = dequeued = encoded | Ring drops | Final backlog | Observed high water | Exact receiver JSON | Tap/console ingress and egress, each |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Authoritative | 599,278 | 0 | 0 | 3,618 | 599,278 | 599,278 |
| Recursive | 905,486 | 0 | 0 | 4,984 | 905,486 | 905,486 |

Both final snapshots had valid ETW statistics, zero stats-query errors,
ETW EventsLost=0 and RealTimeBuffersLost=0. Callback errors, ignored/rejected
records, output-limit drops, failed output events and output errors were all
zero. Worker discards, policy drops, malformed JSON/DNS and unrelated-domain
records were zero. Final SCM stops exited successfully at 07:57:58.913 and
08:06:00.637 UTC; wider receiver markers ended at 07:58:02.508 and 08:06:04.435.
Resource windows exclude that later stop/drain interval. Host/Windows clock
bounds are preserved with each capture; Ubuntu start/between/end clock bounds
are under `ubuntu`. One-second boundary interpolation remains approximate.

Domain closure was verified: authoritative JSON has 299,639 CQ and CR each;
recursive JSON has 299,867 CQ/CR and 152,876 RQ/RR each. Actual upstream query
and reply counts both equal 152,876, with zero new Ubuntu UDP errors. Thus this
controlled run reconciles 2 × 299,867 + 152,876 + 152,876 = 905,486; it does not
assume four events per client query. The upstream/client count ratio was 50.98%,
so this repeated 5,000-name workload does not establish pure cache-miss capacity.
ETW lost-events counters cover the enabled session and lost-buffer units are
never added to event counts.

An auxiliary collector `tap policy_forwarded` counter was 905,422, 64 below
recursive delivery, despite exact file/worker ingress/egress and zero drops.
This unresolved telemetry-accounting discrepancy is retained separately; it
is not evidence of 64 missing records. Do not claim every collector counter
agreed. The authoritative policy-forwarded count agreed with delivery.

All retained positive recursive client/resolver responses had TTL 0. One
retained client response had RA=false/AA=true, NOERROR and one answer:
`www.rec.stress.test`, DNS ID 0, timestamp 07:59:48.7991864 UTC. The other
299,866 client responses had RA=true. Upstream authoritative resolver replies
had RA=false as expected. JSON flag fields were present; field absence and a
clear RA bit are different. The direct-client summary and captured example
both expose the anomaly, but the summary does not retain a failing tuple for
explicit correlation. Its cause is unknown and is not attributed to capture.

### CPU, memory, threads and I/O

The following table records process and guest CPU, coverage, and sample counts:

| Mode | Service CPU mean / p95 / max, one-core % | CPU-seconds / valid seconds | Valid samples | DNS CPU one-core % | Guest total CPU % |
| --- | ---: | ---: | ---: | ---: | ---: |
| Authoritative | 0.9243 / 3.1077 / 5.8702 | 2.7657 / 299.220 | 300 | 3.4361 | 5.3673 |
| Recursive | 1.0799 / 3.1232 / 4.6584 | 3.2344 / 299.505 | 300 | 6.0074 | 6.9968 |

CPU means and percentiles are duration weighted interval-average PDH rates;
maxima are observed interval averages, not instantaneous peaks. Divide service
and DNS one-core CPU by two for VM capacity. No paired capture-off baseline was
run, so these are observed resource use, not isolated incremental service cost.
Previous original-1,024 capped runs lost 24.78%/37.27% of received records; these
v2 runs lost zero. Their five-minute resource/WPR windows are not the matched
CPU controls: use the six-arm no-WPR comparison for the CPU-change question.

| Mode | Private MiB start → end / peak | Working-set peak MiB | Threads / handles range | Mean process read / write bytes/s | Mean write ops/s |
| --- | ---: | ---: | ---: | ---: | ---: |
| Authoritative | 2.797 → 3.637 / 4.633 | 10.188 | 7–9 / 99–102 | 0 / 491.89 | 0.994 |
| Recursive | 2.922 → 4.066 / 4.582 | 10.098 | 7–9 / 99–102 | 0 / 490.22 | 0.990 |

Process I/O columns are sample means and do not represent all network traffic.
One-second metrics writes are included. Thread instance identity transitions
rejected 44/45 rate values; current PID/TID and preceding identity were checked.
Observed thread coverage matched process thread counts in all 300 samples per
run. The private-byte increases are +0.840/+1.145 MiB; these short observations
do not establish absence of a leak. No growing queue remained after drain.

| Mode | Guest run queue mean / max | Disk queue mean / max | Disk write latency mean, ms | NIC total mean bytes/s | Ubuntu collector / upstream CPU one-core % |
| --- | ---: | ---: | ---: | ---: | ---: |
| Authoritative | 0.0179 / 1 | 0.0435 / 5 | 1.819 | 387,841 | 0.8975 / 0 |
| Recursive | 0.0100 / 1 | 0.0499 / 5 | 1.810 | 640,324 | 1.1704 / 2.2421 |

Guest disk metrics cover all Windows disk activity, including WPR; mean read
latency was 0.0262/0.0273 ms. NIC error counters and UDP receive errors stayed
zero. Recursive TCP retransmissions averaged 0.00333 segments/s (authoritative
zero), without output failure or missing delivery. Ubuntu disk busy time was
0.1313%/0.1443%, with collector RSS peaks 73,007,104/74,919,936 bytes.
The busy-poll host client used approximately two cores despite pacing (sample
means 196.33%/197.66% of one core, 24 logical host CPUs); it maintained the offered
rate. Raw host counters and exact sampling coverage remain in each client folder.

### Post-fix WPA evidence

WPR GeneralProfile memory recordings ran inside the official windows:
authoritative 07:54:08.594–07:54:55.160 UTC (46.566 s), recursive
08:02:43.487–08:03:29.795 UTC (46.308 s). Both trace headers reported zero lost
events/buffers and CSwitch coverage retained the beginning through end; this
was checked rather than inferred from zero-loss headers. Instrumentation,
trace start/rundown and one-second metrics add overhead to these resource
windows. The [matched comparison arms](#matched-30-second-comparisons) had no WPR.

A matched interior view at trace-relative 5–25 seconds was exported after both
loads. Actual thread names and service PIDs identify these rows:

| Mode / thread (PID:TID) | Scheduled CPU ms | Same-window exclusive CPU ms | Ready ms | Associated Wait s | Positive Wait p50 / p95, ms |
| --- | ---: | ---: | ---: | ---: | ---: |
| Auth parser 1140:3256 | 195.822 | 187.519 | 36.159 | 19.7656 | 15.9324 / 16.2871 |
| Auth consumer 1140:3304 | 14.118 | 13.905 | 8.625 | 19.9949 | 94.0012 / 106.0626 |
| Rec parser 4364:4520 | 220.741 | 212.145 | 37.581 | 19.7501 | 15.9243 / 16.1463 |
| Rec consumer 4364:4496 | 18.918 | 18.404 | 8.997 | 19.9546 | 64.1823 / 79.8063 |

Whole-service exclusive CPU was 211.878/239.596 ms in these same 20-second views.
Parser waits retain `WrAlertByThreadId` and approximately 16 ms cadence, while
Ready time remains small. Final capacity-only v2 did not improve wake policy;
more queue headroom retained bursts despite the observed polling cadence.
This supports the measured capacity mitigation, not proof that timer resolution
caused the old drops or that locks are absent. Dispatcher wait reasons are not
lock attribution. Scheduled CPU includes a different interrupt treatment from
xperf exclusive CPU; they are deliberately labeled separately.

Ready/Wait sums are associated with switch-ins, not exact clipped occupancy:
the first wait can cross the view boundary and tails without a switch-in can
be absent. These views do not represent all 300 seconds. Raw CSV, exports,
positive quantiles and same-window exclusive logs are under `review/elevated`;
initial sandbox access-denied export logs were preserved in the parent review
folders and successful privileged-tool exports use separate paths.

## Memory and scope

The ring has eight times the entry slots. Maximum copied payload retained in
the ring could grow from approximately 128 MiB to 1 GiB, excluding cell/header/
descriptor/allocator overhead and in-flight records. That theoretical bound
is not predicted RSS. The original 8,192-slot short controls peaked at roughly
3.76 MiB and 4.44 MiB private memory for these small packets; the original
1,024-slot controls peaked at 2.68 MiB and 2.80 MiB. Official private peaks were
4.633/4.582 MiB; this experiment does not prove absence of leaks or zero loss for arbitrary workloads.

## Evidence and restoration

All evidence is under `target/performance/ring-fix-20261029`: original backup,
both preserved candidate/source snapshots, code gates, `client`, `windows`,
`windows-campaign`, `ubuntu`, `archive` and WPA exports. Earlier campaigns
remain intact. Receiver archives are CRC/SHA/host-copy verified before removing
only owned guest raw logs; no heavy archive scans/exports occur during load.

Restoration completed at 08:09:27 UTC. The service is Running (PID 3528) with
verified v2 hash and exactly the original normal SCM arguments, without metrics
or capacity override; the new default selects 8,192. LocalSystem, unrestricted
service SID, `SeChangeNotifyPrivilege` alone and executable ACL were preserved.
The authoritative 5,000 records remain; only the owned recursive forwarder was
removed. WPR is not recording and no task data collector remains active.
Ubuntu restored the exact original console/journal configuration and receive
buffers (212,992/4,194,304), stopped owned upstream/monitor and removed the owned
fixture firewall table while retaining the original TCP 6000 source filter.

A new ordinary authoritative query at 08:09:32 UTC returned 192.0.2.53 TTL 300.
The restored console journal independently retained its CQ/CR pair (ID 10873),
verified in `ubuntu/final-console-proof.{txt,json}`. Current-state backup,
`windows-campaign/restoration-final.json`, final privilege/SID/WPR/logman files
and `ubuntu/campaign-receipt.json` preserve restoration evidence. All eight
archives have verified compressed/raw SHA-256 and gzip integrity; the official
compressed SHA-256 values are:

- Authoritative: `97cfd39def075d67551dfb6dbcc0a4d29e498684b5b8b700adc285ae3675d505`.
- Recursive: `cac2cc12e3959c33da56bf1d0e652aab37ff44faf71810f814aac6342cb4cf84`.

Any subsequent pooled-intake source candidate is separate from this measured
and installed CA3342…64347 artifact; these results do not validate later source.

The zero-ring/capture target passed for the two observed five-minute workloads.
The separate all-DNS-validity gate remains **not met** because of one recursive
RA-check failure; its cause and the auxiliary policy-counter discrepancy remain
unresolved. No automatic rerun, hidden tuning or universal zero-loss claim is made.
