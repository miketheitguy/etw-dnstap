# Code review baseline, 2026-11-01 (re-recorded 2026-11-02)

> **Status:** Historical · **Evidence dated:** 2026-11-02 · **Commit:** `fc983ab` (plan base; measured from the benchmark commit `8ee43a4`, see below) · **Reviewed:** 2026-11-02

Phase 0 record (step 0.1) of
[docs/plans/code-review-remediation-plan.md](../../../docs/plans/code-review-remediation-plan.md).
The Phase 2 performance experiments compare against this record. Do not edit
the results. Record a later measurement in a new directory.

This record replaces its first version (commit `777f21b`), which published one
unattended session. A rerun on the same host came out 5 to 19% faster, outside
every published confidence interval, and the explanation it gave for the gap to
the manual benchmark did not hold (see [Reading](#reading)). This version
measures eight sessions, controls priority and affinity, logs what else ran on
the host, and states how far a session can drift.

## Code under measurement

- Plan base: `fc983ab` (`main` at the time of the review).
- Measured from: `8ee43a4` (the benchmark branch of plan step 1.5).
  Between `fc983ab` and that commit the branch adds the plan document, the
  criterion benchmark (`benches/hot_path.rs`, the `bench` feature, and its
  feature-gated entry points), and a Cargo alias. The benchmark commits change
  no shipped code path, but they do shift line numbers in `src/lib.rs` and
  `src/etw.rs`, which appear in panic locations, so the release binary is not
  claimed to be byte-identical to a build of `fc983ab`. Release builds do not
  reproduce anyway: two builds of the same tree differ in SHA-256 (checked
  here: two consecutive `cargo build --locked --release` runs of one tree
  produced different hashes and the same size, 2,455,552 bytes).
- Toolchain: `nightly-2026-10-01` from `rust-toolchain.toml`.
  `rustc 1.101.0-nightly (21b707e3f 2026-09-30)`, LLVM 23.1.1,
  `x86_64-pc-windows-msvc`, standard library rebuilt with `build-std`.
  Linker and `dumpbin` from MSVC 14.51.36231 (Visual Studio 2026 Build Tools).
- The criterion binary and the manual benchmarks are built with
  `panic = "unwind"`: Cargo builds bench and test targets for unwinding, and
  the rebuilt standard library must match. The shipped build uses
  `panic = "abort"`. The hot path has no panic, but unwinding adds cleanup
  paths around values with destructors (the pool lease), so this code is close
  to, not identical to, what ships. How much that matters has not been
  measured. The lab run that Phase 2 requires is the check that counts.

## Host

- CPU: AMD Ryzen 9 9900X 12-Core Processor, 12 cores, 24 logical processors
  (SMT on), 4.4 GHz base clock. The boost state was not captured; the OS
  reported 4,400 MHz.
- Memory: 63.7 GiB.
- OS: Windows 11 Enterprise 10.0.26300, Balanced power plan.
- This is a developer desktop, not the lab guest. Compare these numbers only
  with later runs on the same host. The system-level reference for the service
  stays
  [docs/performance/pooled-vm-performance-review.md](../../../docs/performance/pooled-vm-performance-review.md).
- Conditions of every session: the benchmark process ran at High priority,
  pinned to logical processors 4 and 6 (two physical cores, no SMT siblings;
  mask `0x50`). The two cores serve the single-thread groups and, in
  `two_thread`, one thread each. Desktop software (a game launcher, fan and
  lighting controllers) ran throughout.
- Other work on the host. The host is shared with other build and test jobs.
  A sampler counted `cargo`, `rustc`, `link`, `cl`, `miri`, and `cargo-miri`
  processes every 4 seconds (the benchmark's own two `cargo` processes make a
  count of 2 a quiet host). The share of samples with anything else running
  during each criterion run:

| Session | Started (local, 2026-11-02) | Samples with other processes | Recorded |
| --- | --- | --- | --- |
| s1 | 02:25 | not sampled; 7 other processes counted at start and end | no |
| s2 | 02:31 | 100% (sampled from 02:35:38) | no |
| s3 | 02:38 | 84% | no |
| s4 | 02:44 | 88% | no |
| s5 | 02:50 | 68% | no |
| s6 | 02:56 | 3% | yes |
| s7 | 03:03 | 11% | yes |
| s8 | 03:09 | 0% | yes |

The other processes were repeated single-threaded `miri` test runs from a
parallel worktree. They made the sessions that overlapped them 10 to 38%
slower (compare the two spread columns of the results table) even with the
benchmark pinned.
Sessions s1 to s5 are kept in `criterion-sessions.csv` to show that effect
and are not used for the baseline numbers. s7 and s8 waited for 15 seconds of
quiet before starting; s7 still overlapped a short job.

## Release binary

`cargo build --locked --release` produced
`target/x86_64-pc-windows-msvc/release/etw-dnstap.exe`:

| Property | Value |
| --- | --- |
| Size | 2,455,552 bytes |
| SHA-256 | see [sha256.txt](sha256.txt) |
| Link warnings | 8 `LNK2047` lines, all from ring's NASM objects (the documented `/FORCE:GUARDEHCONT` exception) |

A `cargo test --release` or `cargo bench` run with the unwind override replaces
that file in `target` with a larger unwind build (3,024,384 bytes here), so
rebuild with `cargo build --locked --release` before hashing or measuring size.

### `scripts/ci/verify-pe.ps1` output

Paths in the first two lines are replaced by placeholders, and the last check
is abbreviated (its label names the profile-path pattern that the script
searches for; the result is 0 matches).

```text
dumpbin: <vs-install-dir>\VC\Tools\MSVC\14.51.36231\bin\Hostx64\x64\dumpbin.exe
image:   <repo>\target\x86_64-pc-windows-msvc\release\etw-dnstap.exe
[PASS] x64 machine (0x8664): machine = 0x8664
[PASS] large-address aware: FILE HEADER characteristics = 0x22
[PASS] high-entropy ASLR (HIGH_ENTROPY_VA): DLL characteristics = 0xC160
[PASS] dynamic base (ASLR): DLL characteristics = 0xC160
[PASS] NX compatible (DEP): DLL characteristics = 0xC160
[PASS] CFG image flag (GUARD_CF): DLL characteristics = 0xC160
[PASS] CET compatible marker: extended DLL characteristics = 0x1
[PASS] base relocations present: Base Relocation size = 0x2E3C
[PASS] load configuration present: Guard Flags line found
[PASS] CFG instrumented code (IMAGE_GUARD_CF_INSTRUMENTED): Guard Flags = 0x10417500
[PASS] CFG function table present: Guard Flags = 0x10417500
[PASS] CFG function count > 0: Guard CF function count = 1488
[PASS] EH continuation table present: Guard Flags = 0x10417500
[PASS] EH continuation count > 0: Guard EH continuation count = 33
[PASS] security cookie present: Security Cookie = 0x140248A40
[PASS] debug directory parsed (known entry type present): debug entry types: coffgrp, dllchar
[PASS] no CodeView (PDB) debug entry: debug entry types: coffgrp, dllchar
[PASS] imports parsed (kernel32.dll present): 12 import(s) parsed
[PASS] no AddressSanitizer runtime import: asan imports:
[PASS] no dynamic VC/UCRT runtime import (static CRT): CRT imports:
[PASS] no absolute profile path strings (trim-paths): 0 match(es)
PE verification: 21 checks, 0 failed.
```

## Criterion benchmark

Command: `cargo bench-hot-path` (the alias runs
`cargo bench --locked --features bench --bench hot_path` with
`profile.release.panic="unwind"`). In a nested worktree, run the expansion
instead, because Cargo merges the parent configuration and doubles the alias
arrays. In PowerShell the TOML string needs single quotes inside:

```powershell
cargo bench --locked --features bench --bench hot_path --config "profile.release.panic='unwind'" -- --save-baseline s1
```

Settings (in `benches/hot_path.rs`): 200 samples, 3 s warm-up, 10 s
measurement, 95% confidence intervals, noise threshold 2%. Times are per
iteration.

Inputs (`src/etw/bench.rs`): event 256 with a 60-byte A query (217-byte ETW
payload), the same query over IPv6 addresses, event 257 with a 100-byte
response (307-byte payload), event 258 with a 37-byte SERVFAIL response, and
event 257 with a 4,096-byte TXT response (4,303-byte payload, a jumbo slot).
Every parse is a schema-cache hit. The groups:

| Group | What one iteration is |
| --- | --- |
| `copy_record` | Acquire a slot, copy the record, drop the lease (recycle). One thread, a pool of 2,048 regular slots that stays in cache. |
| `copy_record_cold` | The same on a pool of 65,538 regular slots (about 169 MiB), larger than the last-level cache. The free list is FIFO, so each iteration takes a slot last touched 65,000 events ago, as the service does at its tested capacity. |
| `two_thread` | One event through the production shape: a producer thread copies it into the cold pool and sends it over a bounded channel (1,024 entries); the benchmark thread parses, encodes, and recycles it. Timed with `iter_custom` over `iters` events from the first copy to the last recycle, so the time per iteration is the reciprocal of the sustained events per second. The producer blocks on a full channel (the service drops instead) so every run moves exactly `iters` events. |
| `decode` | `DecodePlan::decode` on the saved template. |
| `encode` | `Encoder::encode` of a view decoded once outside the measurement. |
| `parse_and_encode` | `pipeline::parse_and_encode`; the copy that makes each input runs in the batch setup (1,024 per batch); the lease recycle inside it is measured. The inputs were written just before on the same core, so they are warm. |
| `decompose` | `parse_encode_retained`: parse and encode on one record that stays leased (the shape of the manual `parse_encode_benchmark`); `recycle`: the lease drop alone. |

The wrappers in `src/etw/bench.rs` are `#[inline(never)]` and the `EVENT_RECORD`
is built once per record, so the measured boundary does not move when a profile
change (item 9's fat LTO) inlines across crates.

### Results

Sessions s6, s7, and s8 are the recorded sessions. The table gives the median
of their three means (so one disturbed session does not set the number), the
95% confidence interval of the median session, and the spread
`(max − min) / median` of the three session means, and of all eight.

| Group | Benchmark | Mean, median of 3 sessions (ns) | 95% CI of the median session (ns) | Median (ns) | Spread over the 3 sessions | Spread over all 8 |
| --- | --- | --- | --- | --- | --- | --- |
| `copy_record` | `jumbo_response_257` | 50.88 | 50.49 – 51.43 | 50.01 | 7.5% | 18.9% |
| `copy_record` | `regular_query_256` | 34.65 | 34.49 – 34.85 | 34.23 | 2.7% | 10.3% |
| `copy_record` | `regular_response_257` | 35.43 | 35.18 – 35.79 | 34.95 | 2.0% | 14.1% |
| `copy_record_cold` | `regular_query_256` | 59.24 | 57.73 – 60.85 | 55.84 | 17.9% | 37.9% |
| `copy_record_cold` | `regular_response_257` | 78.16 | 76.92 – 79.55 | 74.96 | 8.5% | 33.0% |
| `two_thread` | `query_256` | 231.46 | 230.31 – 232.75 | 228.92 | 14.0% | 20.8% |
| `two_thread` | `response_257` | 245.75 | 244.22 – 247.57 | 242.29 | 9.3% | 21.3% |
| `decode` | `failure_258` | 31.53 | 31.20 – 31.90 | 30.83 | 0.3% | 23.9% |
| `decode` | `query_256` | 22.81 | 22.57 – 23.09 | 22.33 | 0.2% | 26.6% |
| `decode` | `query_256_ipv6` | 23.43 | 23.20 – 23.70 | 22.98 | 0.4% | 29.9% |
| `decode` | `response_257` | 32.73 | 32.41 – 33.11 | 32.06 | 1.7% | 23.6% |
| `encode` | `failure_258` | 63.32 | 63.05 – 63.62 | 62.39 | 1.0% | 22.5% |
| `encode` | `query_256` | 63.68 | 63.03 – 64.55 | 62.16 | 1.0% | 37.9% |
| `encode` | `query_256_ipv6` | 64.49 | 63.95 – 65.22 | 62.92 | 1.2% | 24.0% |
| `encode` | `response_257` | 64.31 | 63.97 – 64.70 | 63.15 | 0.5% | 22.5% |
| `parse_and_encode` | `failure_258` | 162.11 | 160.40 – 164.23 | 157.97 | 0.8% | 36.9% |
| `parse_and_encode` | `query_256` | 153.00 | 151.91 – 154.23 | 149.79 | 0.5% | 28.5% |
| `parse_and_encode` | `query_256_ipv6` | 204.47 | 202.00 – 207.48 | 198.40 | 3.4% | 27.5% |
| `parse_and_encode` | `response_257` | 166.02 | 164.30 – 168.20 | 162.27 | 0.8% | 32.4% |
| `decompose` | `parse_encode_retained/failure_258` | 148.54 | 147.48 – 149.74 | 145.44 | 0.6% | 25.4% |
| `decompose` | `parse_encode_retained/query_256` | 141.88 | 140.54 – 143.54 | 137.13 | 2.4% | 25.2% |
| `decompose` | `parse_encode_retained/query_256_ipv6` | 187.08 | 186.02 – 188.24 | 184.05 | 2.3% | 29.4% |
| `decompose` | `parse_encode_retained/response_257` | 148.84 | 147.89 – 149.89 | 146.20 | 4.2% | 24.9% |
| `decompose` | `recycle/query_256` | 11.08 | 10.99 – 11.22 | 10.88 | 1.2% | 9.6% |
| `decompose` | `recycle/response_257` | 11.10 | 11.06 – 11.14 | 10.98 | 0.8% | 11.2% |

`criterion.csv` has one row per benchmark (also the min and max, the standard
deviation, and the throughput unit); `criterion-sessions.csv` has the mean,
median, confidence bounds, and standard deviation of every benchmark in every
session, s1 to s8.

### How far a session drifts

- Within one session the 95% confidence interval of the mean is about ±0.5 to
  1.5% for the single-thread groups, ±2.6% for `copy_record_cold`, and ±0.5%
  for `two_thread`. It describes sampling error in that session only.
- Between clean sessions (s6, s7, s8, 0 to 11% overlap with other jobs) the
  single-thread groups differ by at most 3.4% for `decode`, `encode`,
  `copy_record` (regular), and `parse_and_encode`, by 4.2% for
  `parse_encode_retained`, and by 7.5% for the jumbo copy. The cold pool and
  the two-thread run differ by up to 17.9%: they depend on memory latency and
  on where the scheduler puts the two threads.
- A session that overlaps other jobs (s1 to s5) is up to 38% slower than a
  clean one. The first version of this record may have been such a
  session.
- So a comparison against these numbers is trustworthy only to about 5% for
  the warm groups and about 20% for `copy_record_cold` and `two_thread`, and
  only if the host was quiet. A change worth less than that must be judged
  with the A/B procedure below, never against this table.

### Comparing a change with the baseline

Phase 2 changes are a few percent (items 10 and 12). Judge each one inside a
single session:

1. Check that the host is quiet. This must print nothing, and a loaded host is
   noted in the result:
   `Get-Process cargo, miri, rustc, cargo-miri, link, cl -ErrorAction SilentlyContinue`.
2. Pin the shell: `$p = Get-Process -Id $PID; $p.PriorityClass = 'High'; $p.ProcessorAffinity = [IntPtr]0x50`.
3. On the base commit, save the reference: `... -- --save-baseline pre`.
4. Still on the base commit, run it again against itself:
   `... -- --baseline pre`. Whatever criterion reports as a change is the drift
   of that session. Repeat the reference if it exceeds the noise threshold.
5. Apply the change and run `... -- --baseline pre`.
6. Accept a gain only if it is larger than the drift from step 4 and the
   confidence intervals do not overlap. For a result near that limit, run the
   order again as base, change, change, base (ABBA).
7. Record the same-session before and after in the plan. Use this record only
   to check that a number is the right order of magnitude.

`copy_record` isolates the single-thread cost of item 10's two atomic
operations, but the atomics are uncontended there. Item 10 is about the
contended case across the ETW and parser threads, which only `two_thread`
shows. The single-thread `copy_record` result is a lower bound on what item 10
can win. Item 11 (slot header size) should show up in `copy_record_cold` and in the
private bytes below, not in `copy_record`, which never touches the header tables
or leaves the cache.

### Pool private bytes (Phase 0 metric)

`copy_record_cold` allocates `Pool::with_capacity(65_536, 64)` (65,538 regular
and 64 jumbo slots, as `PoolManager` adds two) and prints the process private
bytes before and after, with the Win32 memory counters:

| Session | Private bytes added by the pool |
| --- | --- |
| s6 | 176,926,720 |
| s7 | 177,090,560 |
| s8 | 176,869,376 |

The median, 176,926,720 bytes (168.7 MiB), is within 0.5% of the documented
counted storage for this size, 177,772,880 bytes
([configuration guide](../../../docs/guide/configuration.md)). The eight
sessions range from 176,603,136 to 177,254,400. Item 11 would remove up to 384 bytes
from each of the 65,538 slots, up to about 25.2 MB of this figure (14%), and the
same metric measures the result.

### Reading

- Throughput figures: the benchmark prints bytes per second for the copy
  groups, but they come from warm caches and mean little for capacity. The
  CSV keeps the per-iteration byte count only so that nobody has to guess the
  input size. Trust the per-event times.
- A warm, single-thread `parse_and_encode` is a best case. The two-thread run
  shows what the same work costs when the record was written by another core:
  231 ns per event for a 256 query (4.32 million events per second sustained,
  4.30 to 4.34 million by the confidence interval of the median session) and
  246 ns for a 257 response (4.07 million per second), against 153 and 166 ns on one
  thread. The 78 to 80 ns difference is what crossing cores adds: cache-to-cache
  transfer of the record, of the pool's `Arc` count, and of the free-list ends,
  plus the channel hand-off. This benchmark does not separate them. In the session with 11% overlap
  (s7) the same run took 263.6 ns (3.79 million per second).
- `parse_and_encode` splits cleanly: for the 256 query,
  `parse_encode_retained` (141.9 ns) plus `recycle` (11.1 ns) is 153.0 ns, the
  measured `parse_and_encode`. For the 257 response the parts sum to 159.9 ns
  against 166.0 ns measured. `decode` plus `encode` (86.5 ns for the query) is
  61% of `parse_encode_retained`; the other 55 ns is the rest of `parse` (by subtraction:
  the schema lookup and the integer, address, and timestamp conversions).
  The IPv6 query costs about 51 ns more than the IPv4 query in
  `parse_and_encode` (204.5 against 153.0), almost all of it in `parse`
  (`decode` differs by 0.6 ns).
- The manual `parse_encode_benchmark` (139.6 to 142.0 ns over eight sessions) agrees
  with `parse_encode_retained` (141.9, 148.8, and 148.5 ns for events 256, 257,
  and 258; their mean is 146.4 ns). The first version of this record said the
  manual figure was lower than criterion's 226 to 260 ns because of packet size
  and missing lease recycling. That does not hold: the packet is copied once
  (17 against 60 bytes is a couple of nanoseconds) and the recycle is 11 ns.
  Measured like for like, the two benchmarks agree. Why the first version read
  226 to 260 ns is not established; an uncontrolled session on a shared host is
  a possible cause. Its numbers are superseded.
- `copy_record` on the small pool is 34.7 ns for a 256 query; 11.1 ns of that is
  the recycle. On the cold pool it is 59.2 ns for the query and 78.2 ns for the
  response: the first touch of a slot costs 25 to 43 ns more than the warm
  case. The jumbo copy (50.9 ns for 4.3 KiB) is a single `memcpy` from a
  slot-sized working set that stays in cache.

## Manual benchmarks

Both ignored manual benchmarks ran in release with `cargo test --release`
and the unwind override (the same caveat as above), once per session, from
the same pinned shell. Neither needs an elevated session or the DNS Server
role. The raw output of every run is in
[manual-benchmarks.txt](manual-benchmarks.txt).

Commands:

```powershell
cargo test --locked --release --config "profile.release.panic='unwind'" -- --ignored --nocapture synthetic_pooled_intake_benchmark
cargo test --locked --release --config "profile.release.panic='unwind'" -- --ignored --nocapture parse_encode_benchmark
```

(`cargo test-release` is the alias for the same command with the override
spelled `'profile.release.panic="unwind"'` for Cargo's own parser.)

| Session | `parse_encode_benchmark` (ns per event) | `synthetic_pooled_intake_benchmark` (ns per offered event) | Accepted of 200,000 | Dropped |
| --- | --- | --- | --- | --- |
| s1 | 141.99 | 67.86 | 8,532 | 191,468 |
| s2 | 141.91 | 60.76 | 25,432 | 174,568 |
| s3 | 140.63 | 56.52 | 28,166 | 171,834 |
| s4 | 140.19 | 60.19 | 29,060 | 170,940 |
| s5 | 139.81 | 54.73 | 55,993 | 144,007 |
| s6 | 139.71 | 53.79 | 9,623 | 190,377 |
| s7 | 141.79 | 50.54 | 137,294 | 62,706 |
| s8 | 139.62 | 50.67 | 111,124 | 88,876 |

- `parse_encode_benchmark` (default 2,000,000 iterations; three cached-schema
  fixture events 256, 257, and 258 with 17-byte packets, reused and never
  recycled) is stable: 139.62 to 141.99 ns over eight sessions, a
  spread of 1.7%. It showed no effect of the overlapping jobs,
  but it runs for under 0.3 s and may simply have fallen between them. The median of the recorded sessions is 139.71 ns.
- `synthetic_pooled_intake_benchmark` (defaults `ETW_BENCH_CAPACITY=8192`,
  `ETW_BENCH_EVENTS=200000`, `ETW_BENCH_MODE=steady`) drops most of its
  offered events, because the consumer drains between 2 ms parks, so it
  measures the callback with a full channel more than the copy. How many of
  the events are accepted is not deterministic (from 8,532 to 137,294 here), so
  its ns per offered event (50.5 to 67.9) varies with that mix. The median of the recorded
  sessions is 50.67 ns. It is a smoke test of the intake path, not a
  baseline for a few-percent change.

## Files

| File | Content |
| --- | --- |
| `README.md` | This record |
| `sha256.txt` | SHA-256 of the release `etw-dnstap.exe`, in the format of `datasets/baselines/arrayqueue-v2/sha256.txt`. The binary itself is not in the clone (`*.exe` is ignored). |
| `criterion.csv` | The criterion numbers, one row per benchmark: median of the three recorded sessions, spreads, and the median session's confidence interval |
| `criterion-sessions.csv` | The criterion numbers of every benchmark in every session, s1 to s8 |
| `manual-benchmarks.txt` | Raw output of the two manual benchmarks, one run per session |
