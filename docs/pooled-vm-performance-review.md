# Pooled intake VM performance review

**Campaign incomplete.** This draft records completed native stages; not all
discovery, confirmation, clean baseline repeats, restoration, and final analysis
are finished. Do not
use an incomplete draft as an operational capacity guarantee.

This report helps maintainers assess the rebuilt pooled candidate under actual
Windows DNS traffic. See the [campaign plan](pooled-vm-performance-plan.md) for
delegation, approvals, exact commands, fixed workload, resource guards, and
restoration requirements. Evidence root: `target/performance/pooled-vm-20261029-0942`. Table generation:
2026-10-29T11:41:13.395218+00:00.

## Build and measured configuration

The locked release invocation, formatting, strict all-target Clippy, reviewed
source identities, actual PE checks, CLI defaults, and build receipt are in
`build`. Cargo reused the reviewed release artifacts; the resulting SHA-256 is
`98FDAB43AA24AE1BE31A7AABCCBDD67A2F64C87DC1C50C428B638B3C091F5D7C`.
The protected baseline `CA3342D8…55264347` remains preserved. Production source
and effective hardening settings stay unchanged during measurement.

The Windows target retains 2 vCPUs, fixed 4 GiB RAM, SCM-only LocalSystem,
enabled service SID, only `SeChangeNotifyPrivilege`, and protected executable/
SCM ACLs. Measured runs use default queue 8,192 and jumbo capacity 64, adding
only a unique metrics prefix. The host client uses two workers, 256 outstanding
requests each, a two-second deadline, and the same 5,000 A-name rotation. A
global requested rate is a ceiling; the achieved-rate gate is at least 98%.
These A workloads do not establish jumbo-pool or heavy DNSSEC capacity.

The authoritative confirmation and subsequent stages use the explicitly
approved temporary Ubuntu data volume for raw receiver JSON, with archives on
the original root volume. CPU, RAM, network, collector YAML, and Frame Streams
behavior stay fixed; only the collector's stdout path changes. Its 64 GiB VHDX
is attached at SCSI 0:2, fully initialized before load, and has filesystem UUID
`3046058a-a6b1-4e74-97f5-a63905494a1f`. Rows identify this receiver-storage
boundary; do not imply matched storage against earlier root-volume stages.

## Workload and final record ledgers

DNS validation failures include flag checks; an RA=false response was received
and is not a lost DNS query/response. Timeouts and send/receive socket errors
are shown separately. Upstream query/reply counts and queries as a percentage
of client sends expose each recursive stage's actual workload intensity;
repeated names and TTL zero do not imply one upstream query per client query.
Received/encoded are ETW records, not DNS packet counts. Intake columns are
ring-full / pool-exhausted / receiver-disconnected. Output/lifecycle columns
show output operation errors / SCM exit code. Exact receiver reconciliation
means every encoded record matches durable JSON and all four ingress/egress
counters, including correctly reconciled lossy stages.

| Run / qualification | Mode | Seconds | Requested / achieved QPS | DNS validation failures (RA subset) | Timeouts / socket errors | Upstream Q / R (% Q/sent) | Received / encoded | Intake categories | Output / SCM | Receiver | HWM |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `pooled-auth-1000-300s-1` (management observer) | authoritative | 300 | 1,000 / 999.67 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 599,802 / 599,802 | 0 / 0 / 0 | 0 / 0 | exact | 3,566 |
| `pooled-rec-1000-300s-1` (management observer) | recursive | 300 | 1,000 / 999.62 | 1 (RA 1) | 0 / 0 | 156,264 / 156,264 (52.11%) | 912,302 / 912,302 | 0 / 0 / 0 | 0 / 0 | exact | 3,790 |
| `pooled-auth-2000-60s-1` | authoritative | 60 | 2,000 / 1,998.75 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 239,850 / 239,850 | 0 / 0 / 0 | 0 / 0 | exact | 4,574 |
| `pooled-auth-4000-60s-1` | authoritative | 60 | 4,000 / 3,994.88 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 479,386 / 479,386 | 0 / 0 / 0 | 0 / 0 | exact | 8,032 |
| `pooled-auth-8000-60s-1` (management observer) | authoritative | 60 | 8,000 / 7,971.15 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 956,538 / 956,538 | 0 / 0 / 0 | 0 / 0 | exact | 3,934 |
| `pooled-auth-16000-60s-1` | authoritative | 60 | 16,000 / 15,908.67 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 1,909,040 / 1,909,040 | 0 / 0 / 0 | 0 / 0 | exact | 887 |
| `pooled-auth-32000-60s-1` | authoritative | 60 | 32,000 / 31,696.00 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 3,803,520 / 3,803,520 | 0 / 0 / 0 | 0 / 0 | exact | 2,093 |
| `pooled-auth-64000-60s-1` | authoritative | 60 | 64,000 / 62,573.43 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 7,508,812 / 6,098,935 | 1,409,877 / 0 / 0 | 0 / 0 | exact | 8,192 |
| `pooled-auth-48000-60s-1` (management observer; excluded threshold) | authoritative | 60 | 48,000 / 47,296.28 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 5,675,554 / 5,673,351 | 2,203 / 0 / 0 | 0 / 0 | exact | 8,192 |
| `pooled-auth-48000-60s-2` | authoritative | 60 | 48,000 / 47,288.98 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 5,674,678 / 5,674,678 | 0 / 0 / 0 | 0 / 0 | exact | 3,967 |
| `pooled-auth-56000-60s-1` (finish reset) | authoritative | 60 | 56,000 / 54,945.08 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 6,593,410 / 6,528,255 | 65,155 / 0 / 0 | 1 / 1066 | exact | 8,192 |
| `pooled-auth-48000-300s-1` (receiver data volume) | authoritative | 300 | 48,000 / 47,303.24 | 0 (RA 0) | 0 / 0 | 0 / 0 (0.00%) | 28,381,946 / 28,381,946 | 0 / 0 / 0 | 0 / 0 | exact | 6,174 |
| `pooled-rec-2000-60s-1` (finish reset; receiver data volume) | recursive | 60 | 2,000 / 1,998.75 | 2 (RA 2) | 0 / 0 | 63,035 / 63,035 (52.56%) | 365,920 / 365,920 | 0 / 0 / 0 | 1 / 1066 | exact | 6,080 |
| `pooled-rec-4000-60s-1` (finish reset; receiver data volume) | recursive | 60 | 4,000 / 3,989.62 | 0 (RA 0) | 0 / 0 | 121,655 / 121,655 (50.82%) | 722,064 / 722,064 | 0 / 0 / 0 | 1 / 1066 | exact | 2,967 |
| `pooled-rec-8000-60s-1` (receiver data volume) | recursive | 60 | 8,000 / 7,966.90 | 0 (RA 0) | 0 / 0 | 242,643 / 242,643 (50.76%) | 1,441,314 / 1,441,314 | 0 / 0 / 0 | 0 / 0 | exact | 967 |
| `pooled-rec-16000-60s-1` (receiver data volume) | recursive | 60 | 16,000 / 15,922.13 | 2 (RA 2) | 0 / 0 | 258,780 / 258,780 (27.09%) | 2,428,216 / 2,428,216 | 0 / 0 / 0 | 0 / 0 | exact | 1,239 |
| `pooled-rec-32000-60s-1` (receiver data volume) | recursive | 60 | 32,000 / 31,691.27 | 0 (RA 0) | 0 / 0 | 288,244 / 288,244 (15.16%) | 4,379,440 / 4,371,905 | 7,535 / 0 / 0 | 0 / 0 | exact | 8,192 |
| `pooled-rec-24000-60s-1` (receiver data volume) | recursive | 60 | 24,000 / 23,800.92 | 0 (RA 0) | 0 / 0 | 279,040 / 279,040 (19.54%) | 3,414,190 / 3,414,190 | 0 / 0 / 0 | 0 / 0 | exact | 2,291 |

For the lossy stages below, missing captured operations are the direct-client
or actual-upstream packet count minus the exact archived operation count.
Their sum equals native ring-admission loss. These are missing capture records;
the direct DNS client separately measures whether queries or responses were lost.

| Run / qualification | Missing CLIENT_QUERY | Missing CLIENT_RESPONSE | Missing RESOLVER_QUERY | Missing RESOLVER_RESPONSE | Sum = ring drops |
| --- | --- | --- | --- | --- | --- |
| `pooled-auth-64000-60s-1` | 705,662 | 704,215 | 0 | 0 | 1,409,877 |
| `pooled-auth-48000-60s-1` (management observer; excluded threshold) | 1,103 | 1,100 | 0 | 0 | 2,203 |
| `pooled-auth-56000-60s-1` (finish reset) | 32,622 | 32,533 | 0 | 0 | 65,155 |
| `pooled-rec-32000-60s-1` (receiver data volume) | 2,499 | 2,506 | 1,264 | 1,266 | 7,535 |

## CPU, memory, and latency

Service CPU is the duration-weighted PID-validated PDH mean, in percent of one
CPU and percent of this two-vCPU VM. CPU work and valid interval coverage are
separate. Memory peaks show bytes / MiB. Latency percentiles are histogram upper
bounds for matched responses, exclude timeouts, and have no coordinated-omission
correction. WPR scheduled CPU includes interrupt attribution unless explicitly
excluded and must not be forced to equal PDH or sampled CPU.

| Run / qualification | Service CPU one core / VM % | CPU-s / valid s | Rows | DNS one-core % | Private peak B / MiB | Working peak B / MiB | p99 upper ms |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `pooled-auth-1000-300s-1` (management observer) | 0.6782 / 0.3391 | 2.0313 / 299.504 | 299 | 3.6518 | 43,630,592 / 41.609 | 49,152,000 / 46.875 | 0.466943 |
| `pooled-rec-1000-300s-1` (management observer) | 0.9600 / 0.4800 | 2.8749 / 299.479 | 300 | 6.8268 | 43,986,944 / 41.949 | 49,672,192 / 47.371 | 0.851967 |
| `pooled-auth-2000-60s-1` | 1.2723 / 0.6361 | 0.7568 / 59.481 | 60 | 7.6956 | 44,109,824 / 42.066 | 49,766,400 / 47.461 | 0.270335 |
| `pooled-auth-4000-60s-1` | 2.4029 / 1.2015 | 1.4293 / 59.483 | 60 | 13.8948 | 44,158,976 / 42.113 | 49,819,648 / 47.512 | 0.245759 |
| `pooled-auth-8000-60s-1` (management observer) | 4.8459 / 2.4230 | 2.8824 / 59.480 | 60 | 28.5643 | 43,298,816 / 41.293 | 48,947,200 / 46.680 | 0.249855 |
| `pooled-auth-16000-60s-1` | 10.1232 / 5.0616 | 6.0219 / 59.486 | 60 | 56.4275 | 43,024,384 / 41.031 | 48,664,576 / 46.410 | 0.221183 |
| `pooled-auth-32000-60s-1` | 21.3799 / 10.6900 | 12.7187 / 59.489 | 60 | 74.3343 | 43,032,576 / 41.039 | 48,697,344 / 46.441 | 0.212991 |
| `pooled-auth-64000-60s-1` | 42.3933 / 21.1966 | 25.2159 / 59.481 | 60 | 83.3843 | 43,294,720 / 41.289 | 48,971,776 / 46.703 | 0.983039 |
| `pooled-auth-48000-60s-1` (management observer; excluded threshold) | 35.1481 / 17.5740 | 20.9008 / 59.465 | 57 | 78.3096 | 43,061,248 / 41.066 | 48,775,168 / 46.516 | 0.270335 |
| `pooled-auth-48000-60s-2` | 35.3533 / 17.6766 | 21.0253 / 59.472 | 60 | 77.2043 | 43,040,768 / 41.047 | 48,750,592 / 46.492 | 0.294911 |
| `pooled-auth-56000-60s-1` (finish reset) | 45.0348 / 22.5174 | 26.7871 / 59.481 | 60 | 79.5895 | 43,024,384 / 41.031 | 48,754,688 / 46.496 | 0.688127 |
| `pooled-auth-48000-300s-1` (receiver data volume) | 36.0372 / 18.0186 | 107.9145 / 299.453 | 300 | 79.0827 | 43,180,032 / 41.180 | 48,893,952 / 46.629 | 0.270335 |
| `pooled-rec-2000-60s-1` (finish reset; receiver data volume) | 1.7990 / 0.8995 | 1.0707 / 59.517 | 60 | 13.3638 | 44,376,064 / 42.320 | 49,864,704 / 47.555 | 0.786431 |
| `pooled-rec-4000-60s-1` (finish reset; receiver data volume) | 3.1912 / 1.5956 | 1.8985 / 59.493 | 60 | 26.4488 | 43,634,688 / 41.613 | 49,025,024 / 46.754 | 0.655359 |
| `pooled-rec-8000-60s-1` (receiver data volume) | 6.8798 / 3.4399 | 4.0939 / 59.506 | 60 | 44.8148 | 43,028,480 / 41.035 | 48,398,336 / 46.156 | 0.540671 |
| `pooled-rec-16000-60s-1` (receiver data volume) | 14.0544 / 7.0272 | 8.3580 / 59.469 | 60 | 66.6752 | 43,024,384 / 41.031 | 48,418,816 / 46.176 | 0.499711 |
| `pooled-rec-32000-60s-1` (receiver data volume) | 27.9855 / 13.9928 | 16.6480 / 59.488 | 60 | 79.1570 | 43,032,576 / 41.039 | 48,488,448 / 46.242 | 0.466943 |
| `pooled-rec-24000-60s-1` (receiver data volume) | 20.3718 / 10.1859 | 12.1139 / 59.464 | 60 | 75.3023 | 43,069,440 / 41.074 | 48,476,160 / 46.230 | 0.458751 |

The optional receiver visibility-delay distribution is unavailable: these exact
archive scans did not enable the monitor/provider-time calculation, and no
archive is rescanned to manufacture it. Preserved one-second file-size samples
show coarse output visibility; their cadence is not a measured per-packet
network latency or fsync durability bound. Direct DNS latency remains separate.

## Thread and handle coverage

All per-thread PID/TID, CPU/user/kernel rates, context-switch rates, state, and
wait-reason samples remain in the raw BLG/CSV and per-run `analysis.json`.
Consecutive identity transitions invalidate rates and are counted below rather
than interpreted as CPU spikes. Actual WPR thread lifetimes and CPU Precise
data supplement interval-sampled thread state; they do not recreate missing
application loss counters.

| Run / qualification | Threads | Handles | Thread samples | Incomplete | Invalid PID | Rejected thread rates |
| --- | --- | --- | --- | --- | --- | --- |
| `pooled-auth-1000-300s-1` (management observer) | 7–9 | 99–102 | 299 | 0 | 0 | 34 |
| `pooled-rec-1000-300s-1` (management observer) | 7–9 | 99–102 | 300 | 0 | 0 | 37 |
| `pooled-auth-2000-60s-1` | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-auth-4000-60s-1` | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-auth-8000-60s-1` (management observer) | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-auth-16000-60s-1` | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-auth-32000-60s-1` | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-auth-64000-60s-1` | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-auth-48000-60s-1` (management observer; excluded threshold) | 7–9 | 102–102 | 57 | 0 | 0 | 19 |
| `pooled-auth-48000-60s-2` | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-auth-56000-60s-1` (finish reset) | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-auth-48000-300s-1` (receiver data volume) | 7–9 | 99–102 | 300 | 0 | 0 | 36 |
| `pooled-rec-2000-60s-1` (finish reset; receiver data volume) | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-rec-4000-60s-1` (finish reset; receiver data volume) | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-rec-8000-60s-1` (receiver data volume) | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-rec-16000-60s-1` (receiver data volume) | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-rec-32000-60s-1` (receiver data volume) | 7–9 | 102–102 | 60 | 0 | 0 | 19 |
| `pooled-rec-24000-60s-1` (receiver data volume) | 7–10 | 102–102 | 60 | 0 | 0 | 24 |

The following service I/O and page-fault rates are arithmetic means of valid
one-second PDH samples. Page faults include soft faults; they are not hard-fault
counts. Virtual bytes measure reserved address space, not committed or resident
memory. Guest available memory is in MiB. Process I/O counters do not establish
network record delivery or disk durability; the exact receiver ledger remains
the delivery evidence.

| Run / qualification | Virtual peak B | Faults/s | Read / write B/s | Read / write / other ops/s | Guest available MiB min | CPU queue max |
| --- | --- | --- | --- | --- | --- | --- |
| `pooled-auth-1000-300s-1` (management observer) | 2,203,448,729,600 | 71.398 | 0.000 / 553.785 | 0.000 / 0.992 / 0.027 | 2095.0 | 4 |
| `pooled-rec-1000-300s-1` (management observer) | 2,203,448,766,464 | 134.594 | 0.000 / 555.681 | 0.000 / 0.993 / 0.026 | 2022.0 | 4 |
| `pooled-auth-2000-60s-1` | 2,203,448,881,152 | 160.699 | 0.000 / 556.897 | 0.000 / 1.000 / 0.132 | 2238.0 | 1 |
| `pooled-auth-4000-60s-1` | 2,203,449,061,376 | 287.700 | 0.000 / 558.157 | 0.000 / 1.000 / 0.133 | 2247.0 | 1 |
| `pooled-auth-8000-60s-1` (management observer) | 2,203,448,537,088 | 192.374 | 0.000 / 560.053 | 0.000 / 1.000 / 0.131 | 2074.0 | 1 |
| `pooled-auth-16000-60s-1` | 2,203,448,274,944 | 3.836 | 0.000 / 562.146 | 0.000 / 1.000 / 0.131 | 2254.0 | 2 |
| `pooled-auth-32000-60s-1` | 2,203,448,274,944 | 8.527 | 0.000 / 564.991 | 0.000 / 1.000 / 0.133 | 2246.0 | 3 |
| `pooled-auth-64000-60s-1` | 2,203,448,537,088 | 821.568 | 0.000 / 578.024 | 0.000 / 1.001 / 0.149 | 2257.0 | 3 |
| `pooled-auth-48000-60s-1` (management observer; excluded threshold) | 2,203,448,274,944 | 387.928 | 0.000 / 568.523 | 0.000 / 1.000 / 0.140 | 2257.0 | 4 |
| `pooled-auth-48000-60s-2` | 2,203,448,274,944 | 136.886 | 0.000 / 566.204 | 0.000 / 1.000 / 0.135 | 2495.0 | 3 |
| `pooled-auth-56000-60s-1` (finish reset) | 2,203,448,274,944 | 549.150 | 0.000 / 575.734 | 0.000 / 1.004 / 0.152 | 2500.0 | 3 |
| `pooled-auth-48000-300s-1` (receiver data volume) | 2,203,448,340,480 | 163.031 | 0.000 / 565.182 | 0.000 / 0.991 / 0.030 | 2587.0 | 4 |
| `pooled-rec-2000-60s-1` (finish reset; receiver data volume) | 2,203,448,848,384 | 184.820 | 0.000 / 548.709 | 0.000 / 0.983 / 0.133 | 2775.0 | 1 |
| `pooled-rec-4000-60s-1` (finish reset; receiver data volume) | 2,203,448,864,768 | 197.609 | 0.000 / 559.380 | 0.000 / 1.000 / 0.134 | 2758.0 | 2 |
| `pooled-rec-8000-60s-1` (receiver data volume) | 2,203,448,274,944 | 4.623 | 0.000 / 561.482 | 0.000 / 1.000 / 0.132 | 2722.0 | 2 |
| `pooled-rec-16000-60s-1` (receiver data volume) | 2,203,448,274,944 | 4.155 | 0.000 / 563.677 | 0.000 / 1.000 / 0.131 | 2726.0 | 2 |
| `pooled-rec-32000-60s-1` (receiver data volume) | 2,203,448,274,944 | 62.795 | 0.000 / 572.451 | 0.000 / 1.001 / 0.131 | 2704.0 | 2 |
| `pooled-rec-24000-60s-1` (receiver data volume) | 2,203,450,372,096 | 10.622 | 0.000 / 564.745 | 0.000 / 1.000 / 0.131 | 2717.0 | 3 |

## Actual WPR evidence and limitations

Every stage retains an actual native ETL from the validated single full-stage
file-mode profile: CPU sampling, CSwitch/ReadyThread and stacks, process/thread
lifetimes, disk/file I/O, and hard faults, with 128 × 1,024 KiB buffers. Native
headers, event retention, lost-event/buffer counters, and continuous scheduling
coverage are inspected per run. See the separate
[trace analysis](pooled-vm-trace-analysis.md) and per-run `wpa` evidence.

An inspected zero-loss ETL is distinct from an optional WPA export. The initial
1,000 QPS stages have matched and broad exported views; the first 64,000 QPS
failure has focused early/late views. Other discovery stages have native
inspection gates and retained ETLs; their optional exports remain deferred.
The final coverage table records inspection and selected exports separately.

| Run / qualification | ETL bytes | Header retained s | Actual scheduling coverage s | Lost events / buffers | Sending interval | Native trace gate | Successful optional WPA views |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `pooled-auth-1000-300s-1` (management observer) | 776,994,816 | 320.198768 | 0–320.204962; continuous | 0 / 0 | contained | pass | broad, matched |
| `pooled-rec-1000-300s-1` (management observer) | 663,748,608 | 320.689019 | 0–320.697657; continuous | 0 / 0 | contained | pass | broad, matched |
| `pooled-auth-2000-60s-1` | 193,986,560 | 79.297179 | 0–79.297719; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-auth-4000-60s-1` | 285,212,672 | 79.318923 | 0–79.319147; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-auth-8000-60s-1` (management observer) | 483,393,536 | 79.309317 | 0–79.309405; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-auth-16000-60s-1` | 806,354,944 | 79.797580 | 0–79.799534; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-auth-32000-60s-1` | 1,112,539,136 | 80.099556 | 0–80.100141; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-auth-64000-60s-1` | 1,785,724,928 | 84.069067 | 0–84.069261; continuous | 0 / 0 | contained | pass | early, late |
| `pooled-auth-48000-60s-1` (management observer; excluded threshold) | 1,577,058,304 | 84.239121 | 0–84.240050; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-auth-48000-60s-2` | 1,408,237,568 | 82.061765 | 0–82.061981; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-auth-56000-60s-1` (finish reset) | 1,666,187,264 | 82.432998 | 0–82.433183; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-auth-48000-300s-1` (receiver data volume) | 6,859,784,192 | 320.178539 | 0–320.181360; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-rec-2000-60s-1` (finish reset; receiver data volume) | 263,192,576 | 80.758128 | 0–80.758244; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-rec-4000-60s-1` (finish reset; receiver data volume) | 390,070,272 | 80.744253 | 0–80.750248; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-rec-8000-60s-1` (receiver data volume) | 564,133,888 | 81.668708 | 0–81.670128; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-rec-16000-60s-1` (receiver data volume) | 838,860,800 | 81.589317 | 0–81.589760; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-rec-32000-60s-1` (receiver data volume) | 1,168,113,664 | 82.152163 | 0–82.154389; continuous | 0 / 0 | contained | pass | none; ETL inspected |
| `pooled-rec-24000-60s-1` (receiver data volume) | 1,003,487,232 | 81.680043 | 0–81.680789; continuous | 0 / 0 | contained | pass | none; ETL inspected |

The derivative `wpa/trace-gates-v1.json` records canonical hashes and exact
inspection source paths without changing any raw receipt. An exported view
samples its stated interval; it does not establish full-stage absence of waits,
locks, faults, or application loss.

The release has no matching PDB/CodeView record. Rust application function names
cannot be recovered reliably from unrelated symbols. Module/address and named
thread attribution remain available. File/hard-fault coverage with zero target
events is a measured zero, not a claim that the provider was disabled.

The initial 64,000 QPS stage lost 1,409,877 of 7,508,812 ETW records (18.7763%)
at channel admission while all 3,754,406 DNS requests completed valid. Loss
appears throughout all 60 sampled sending intervals. Focused native scheduling
shows substantial parser Ready time during persistent backlog; naming a TDH or
lock cause requires additional proof. Empty-queue waits in low-load traces
do not establish the cause of a full-queue high-rate failure.

## Measurement qualifications and lifecycle findings

The historical management audit, helper diff, and exact read-only fix proof are
preserved in `build/management-overlap-audit-v1.json` and
`build/status-helper-fix.diff`. Original 1,000 QPS runs and auth 8,000 QPS include
an in-load 5,000-record DNS management inventory. Their capture evidence remains
valid but CPU comparison is qualified. The original 48,000 QPS run is explicitly
perturbed and excluded from the intrinsic threshold bracket; its two drop
intervals overlap the management call envelope. Causation is not proven. Its
raw failure remains preserved, followed by one coordinator-authorized clean
replacement. Subsequent load uses only existing guards and native metric tails.

The first authoritative run's controller reported a null wrapper exit code;
the actual DNS client exit was zero and complete raw evidence reconciled. The
raw controller failure and `completion-assessment.json` remain preserved. The
retained-handle correction was checked with non-load child exit codes zero and
seven before later load.

The 56,000 QPS stage also has a distinct failed lifecycle: output operation
errors increased only at final finish, while every encoded record was delivered.
Application Event ID 1, Record 127, PID 4784, at 10:28:50.7720946 UTC records
STOP/FINISH exchange failure with Winsock error 10054. The measured final
operation lasted 0.4412 ms; SCM returned 1066 / service-specific 1. The receiver
stayed active and no operator teardown occurred. Its generic reset log also
appears on clean runs and alone proves no TCP reset. The reset's cause remains
unresolved; the same error chain exists in the pre-deployment baseline.
See `stages/pooled-auth-56000-60s-1/lifecycle-assessment.json` and raw event XML.

## Remaining validation and final restoration

The approved campaign sequence is authoritative confirmation, independent
recursive discovery/refinement/confirmation, the two clean 300-second 1,000 QPS
repeats, then restoration and final interpretation. Any temporary receiver data
disk and raw-path change must be documented and restored; do not imply matched
receiver storage across that boundary. Preserve all failed and perturbed runs.

Historical capacity-only CPU numbers are descriptive context only: the previous
partial memory-mode WPR recording and this full file-mode profile differ. No
causal CPU saving from pooling is established by these single trials. Final
findings require completed gates, exact achieved workload/upstream counts,
resource coverage, trace coverage, and restoration proof.
