# Schema-cache lookup WPR comparison

> **Status:** Historical · **Evidence dated:** not recorded · **Commit:** not recorded · **Reviewed:** 2026-11-01

The parser shows **3.79% lower observed mean CPU usage** in the schema-cache
single-lookup trial: **15.1423% of one logical CPU**, compared with **15.7392%**
in the prior symbolized baseline. This report helps pipeline maintainers assess
that thread CPU comparison. It compares one profiled
trial per artifact using elapsed-time normalization and the prior symbolized
trial's accounting method. Achieved work is not measured, so these observations
do not establish per-query efficiency, throughput, or a causal improvement from
the code change.

## Observed CPU usage change

The following CPU seconds exclude deferred procedure calls and interrupt service
routines (DPC/ISR). Means use each trial's own interior duration: 57.9940597 s
for the baseline and 57.9952591 s for single lookup.

| Process or thread | Baseline CPU s | Lookup CPU s | Baseline mean, one-core % | Lookup mean, one-core % | Observed mean reduction |
| --- | ---: | ---: | ---: | ---: | ---: |
| Parser/writer | 9.127795 | 8.781835 | 15.7392% | 15.1423% | **3.7922%** |
| Native consumer | 4.800910 | 4.732730 | 8.2783% | 8.1605% | 1.4222% |
| Dnstap service | 13.956633 | 13.541294 | 24.0656% | 23.3490% | 2.9779% |
| DNS | 116.466780 | 116.930808 | 200.8254% | 201.6213% | −0.3963% (increase) |

The parser falls by 0.5969 percentage points of one-core utilization. Service
CPU includes both worker rows. DNS can exceed 100% of one core because its
process has multiple workers. Four-vCPU capacity uses a separate denominator:

| Process or thread | Baseline four-vCPU share | Lookup four-vCPU share |
| --- | ---: | ---: |
| Parser/writer | 3.9348% | 3.7856% |
| Native consumer | 2.0696% | 2.0401% |
| Dnstap service | 6.0164% | 5.8372% |
| DNS | 50.2063% | 50.4053% |

## Thread scheduling and waits

The loaded service is PID 4232, parser TID 3304, and consumer TID 3552; the
baseline uses PID 1576, parser TID 4268, and consumer TID 1852. Thread names,
lifetimes, and actual loaded Rust/`ProcessTrace` stacks identify the workers.
Both lookup workers span the interior and the service does not restart.

| Thread and trial | Scheduled Running | Ready | Wait |
| --- | ---: | ---: | ---: |
| Parser, baseline | 15.052997 s (25.96%) | 16.133125 s (27.82%) | 26.807938 s (46.23%) |
| Parser, lookup | 14.713942 s (25.37%) | 15.166985 s (26.15%) | 28.114332 s (48.48%) |
| Consumer, baseline | 8.442053 s (14.56%) | 11.836809 s (20.41%) | 37.716117 s (65.03%) |
| Consumer, lookup | 8.489033 s (14.64%) | 11.248038 s (19.39%) | 38.258200 s (65.97%) |

Scheduled Running includes interrupt occupancy; the parser's 14.714 s is not
14.714 s of Rust execution. Ready is eligible time awaiting scheduling. Neither
exclusive CPU nor scheduled occupancy shows sustained parser saturation.

All lookup parser Wait resolves to `parse_loop → park_timeout → WaitOnAddress`,
with reason `WrAlertByThreadId`. The parser retains timeout-based polling.
The wait calculation includes one completion after the interior. Complete
interior wait median is 3.665 ms and p95 is 15.363 ms, compared with baseline
6.877 ms and 15.738 ms. These blocked durations do not measure queue occupancy
or avoidable delay. The consumer's dominant resolved wait remains
`ProcessTrace → EtwpProcessRealTimeTraces → ZwWaitForMultipleObjects`.

Parser state coverage equals the entire interior, within floating-point noise.
The lookup consumer has 11.3 µs of overlap and 0.2 µs uncovered; its raw state
sum exceeds the window by 11.1 µs. Baseline consumer overlap is 932.1 µs and
uncovered time is 13.2 µs. These small export discrepancies remain explicit.

## Source and capture comparability

The retained source comparison changes only `src/etw.rs`: a cache hit returns
through `parse_schema` after one `get`, replacing `contains_key` followed by
`get`. A miss still loads the schema, preserves the 32-entry clearing policy,
and inserts before parsing. The extracted helper preserves fast-plan and TDH
fallback dispatch. The same file also adds tests; no tests were run during this
campaign, as instructed.

Both artifacts use the recorded working tree at revision
`5fa21521a915a342023af226f2399d6476ed066d`; the revision alone does not identify
either complete source snapshot. The retained build comparison records the same
profiling/compiler flags: optimization level 3, full debug information, panic
abort, overflow checks, 16 codegen units, no incremental compilation, and no LTO.
Linker flags retain `/DEBUG:FULL`, `/OPT:REF`, `/OPT:ICF`, and `/INCREMENTAL:NO`.
The build passes its effective-flag and native identity checks. No hashes,
unit tests, or Clippy checks were run for this campaign.

The lookup executable is 1,092,096 bytes and its PDB is 21,368,832 bytes. PE
RSDS and native PDB identity agree at `B02FC5B4-6066-418C-AE5C-62FE000E75EA`,
Age 1. Actual idle captured Rust stacks pass the symbol gate before load.

The same retained four-vCPU, 4 GiB VM, service identity/configuration, capture
profile, and host workload fixture are preserved. The run makes no VM CPU,
affinity, priority, network, or storage configuration change.

## Timing and analysis method

The client sends from `2026-10-30T22:38:20.383Z` to
`22:39:20.383Z`, reports 60 seconds, and exits with code 0. Its deadline does
not expire and no owned workload process remains. The workload's configured
rate is a fixture setting; this report does not read query counts, drops,
collector counts, or achieved rate.

The ETL origin is `2026-10-30T22:38:10.9213849Z`, its end is
`22:39:29.6785047Z`, and its duration is 78.7571198 s. Its header reports four
CPUs, zero lost events, and zero lost buffers. Guest and host ETL lengths match
at 1,415,577,600 bytes and the host copy opens. This is not a cryptographic
transfer verification or a delivery-correctness claim.

| Mapping item | Lookup result |
| --- | --- |
| Pre-run guest-minus-host offset | +13.9373 to +16.2101 ms |
| Post-run guest-minus-host offset | +13.8178 to +16.5587 ms |
| Envelope | +13.8178 to +16.5587 ms |
| Endpoint midpoint change | +0.11455 ms; not an exact drift rate |
| Sending-start range in trace time | 9.4744329–9.4791738 s |
| Sending-end range in trace time | 69.4744329–69.4791738 s |
| Conservative common sending interval | 9.4791738–69.4744329 s |
| Analysis interior | **10.4791738–68.4744329 s**, **57.9952591 s** |

The analysis reuses the baseline's precise clock mapping: intersect
host-before/guest/host-after brackets within each pre/post set, use the envelope
of both sets, allow ±1 ms for client receipt precision, and trim one second
inside each edge of the conservative common sending interval. This assumes no
unobserved clock step or excursion outside the endpoint envelope. QPC anchors
and `GetSystemTimePreciseAsFileTime` receipts are retained.

Full context-switch exports supply clipped scheduled Running, Ready, and Wait
intervals, including completions after the interior. Coverage discrepancies
remain explicit. Scheduled Running includes interrupt occupancy and is distinct
from DPC/ISR-exclusive execution. Xperf `cpudisk -thread -exc_dpcisr` supplies
exclusive CPU seconds over the same exact interior. The comparison does not use unclassified sampled CPU to attribute application
hot paths. No new per-function sample comparison or interrupt-fragment budget
is needed to establish this DPC/ISR-exclusive CPU result.

Mean one-core utilization is `exclusive CPU seconds / analyzed wall seconds`.
Four-vCPU capacity is that fraction divided by four; thread rows are included
in the service row. The observed CPU usage reduction percentage is
`100 * (baseline mean utilization - lookup mean utilization) / baseline mean utilization`.
A negative reduction is an observed increase. Unequal interior durations are
normalized before comparison.

## Interpretation limits

One profiled trial per artifact supplies no estimate of run-to-run variation.
The same compiler profile and capture design improve comparability, while
profiler overhead, clock mapping assumptions, scheduling, DNS execution, and
virtual-network interrupt competition can affect CPU usage and state durations.
Sampled weights estimate instruction time; overlapping inclusive paths cannot
be added. Optimized inlining and missing stacks limit source attribution.

No achieved-work measurement or unprofiled control exists for this comparison.
A lower observed CPU mean therefore cannot be described as a measured
per-query saving, capacity gain, or isolated effect of the cache change. No
additional trial is inferred or requested by these results.

## Retained evidence

The lookup campaign is
[`target/analysis/symbol-wpr/single-lookup-100000`](../../target/analysis/symbol-wpr/single-lookup-100000/) (local artifact).
The [baseline report](symbolized-release-wpr-results.md) retains the earlier
trial's source, capture, and interpretation. The lookup campaign preserves
`build/READY.json`, `build/previous-source-diff.patch`, source snapshots,
effective compiler flags, native identity, deployment, actual symbol-gate
exports, clock brackets, client timing/exit receipts, guest ETL and host copy,
full WPA exports, and reproducible scripts in `analysis/`.

The principal derived receipts are `analysis/window.json`,
`analysis/cpu-exclusive.txt`, `analysis/exclusive-cpu.json`,
`analysis/interior.json`, and `analysis/comparison.json`. `map-window.py`,
`analyze.py`, `exclusive-cpu.py`, and `compare.py` retain their calculations.
The full scheduling export is under `exports/loaded-100000-stacks-all/`.
Unclassified samples retained in `interior.json` are explicitly labeled and
are unused in this report. Analysis invokes no workload or VM mutation.

## Final state and delegation

The updated symbol build remains running as PID 4232 with the existing service
configuration. WPR is idle, no workload remains, and the previous executable,
PDB, service configuration, and ACL backup are retained. The operator receipt
records completed exports and the final health checks.

Sol at low effort handled the build and workload. Sol at medium effort handled
deployment/WPR and the comparison. The coordinator independently reproduced the
parser's normalized CPU reduction from the retained precise CPU measurements.
