# Code review baseline, 2026-11-01

> **Status:** Historical · **Evidence dated:** 2026-11-01 · **Commit:** `fc983ab` (plan base; measured from the benchmark commit `4f9ef0a`, see below) · **Reviewed:** 2026-11-01

Phase 0 record (step 0.1) of
[docs/plans/code-review-remediation-plan.md](../../../docs/plans/code-review-remediation-plan.md).
The Phase 2 performance experiments compare against these numbers. Do not edit
the results. Record a later measurement in a new directory.

## Code under measurement

- Plan base: `fc983ab` (`main` at the time of the review).
- Measured from: `4f9ef0a` on `plan/code-review-remediation`. Between
  `fc983ab` and that commit the branch adds the plan document, the criterion
  benchmark (`benches/hot_path.rs`, the `bench` feature, and its feature-gated
  entry points), and a Cargo alias. None of that is compiled into the shipped
  build, so the release binary has the same inputs as at `fc983ab`.
- Toolchain: `nightly-2026-10-01` from `rust-toolchain.toml`.
  `rustc 1.101.0-nightly (21b707e3f 2026-09-30)`, LLVM 23.1.1,
  `x86_64-pc-windows-msvc`, standard library rebuilt with `build-std`.
  Linker and `dumpbin` from MSVC 14.51.36231 (Visual Studio 2026 Build Tools).

## Host

- CPU: AMD Ryzen 9 9900X 12-Core Processor, 12 cores, 24 logical processors,
  4.4 GHz base clock.
- Memory: 63.7 GiB.
- OS: Windows 11 Enterprise 10.0.26300.
- This is a developer desktop, not the lab guest. Compare these numbers only
  with later runs on the same host. The system-level reference for the service
  stays
  [docs/performance/pooled-vm-performance-review.md](../../../docs/performance/pooled-vm-performance-review.md).

## Release binary

`cargo build --locked --release` produced
`target/x86_64-pc-windows-msvc/release/etw-dnstap.exe`:

| Property | Value |
| --- | --- |
| Size | 2,455,552 bytes |
| SHA-256 | see [sha256.txt](sha256.txt) |
| Link warnings | 8 `LNK2047` lines, all from ring's NASM objects (the documented `/FORCE:GUARDEHCONT` exception) |

### `scripts/ci/verify-pe.ps1` output

Paths in the first two lines are replaced by placeholders.

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
[PASS] no absolute C:\Users\ path strings (trim-paths): 0 match(es)
PE verification: 21 checks, 0 failed.
```

## Criterion benchmark

Command: `cargo bench-hot-path` (the alias runs
`cargo bench --locked --features bench --bench hot_path` with
`profile.release.panic="unwind"`, because Cargo builds bench targets for
unwinding and the rebuilt standard library must match). The hot path has no
panic, so the unwind tables do not change the measured code, but the binary is
not byte-identical to the shipped one.

Inputs (`src/etw/bench.rs`): event 256 with a 60-byte A query (217-byte ETW
payload), event 257 with a 100-byte response (307-byte payload), and event 257
with a 4,096-byte TXT response (4,303-byte payload, a jumbo slot). Every parse
is a schema-cache hit. One iteration of `copy_record` includes the lease
recycle (drop), which production pays on the parser thread. `parse_and_encode`
builds its input (`copy_record`) in the batch setup, outside the measurement.

Criterion defaults: 100 samples, 3 s warm-up, 5 s measurement, 95% confidence
intervals. Times are per iteration.

| Group | Benchmark | Mean (95% CI) | Median (95% CI) | Throughput (mean) |
| --- | --- | --- | --- | --- |
| `copy_record` | `regular_query_256` | 45.94 ns (45.64 – 46.18) | 46.14 ns (45.97 – 46.33) | 4.40 GiB/s |
| `copy_record` | `regular_response_257` | 48.80 ns (48.39 – 49.17) | 49.45 ns (49.24 – 49.66) | 5.86 GiB/s |
| `copy_record` | `jumbo_response_257` | 72.35 ns (71.99 – 72.67) | 72.90 ns (72.63 – 73.12) | 55.4 GiB/s |
| `decode` | `query_256` | 43.48 ns (42.73 – 44.14) | 44.39 ns (43.89 – 44.69) | 4.65 GiB/s |
| `decode` | `response_257` | 59.01 ns (58.13 – 59.80) | 59.94 ns (59.17 – 60.58) | 4.85 GiB/s |
| `encode` | `query_256` | 89.67 ns (88.68 – 90.50) | 90.78 ns (90.23 – 91.65) | 11.2 M events/s |
| `encode` | `response_257` | 91.32 ns (90.86 – 91.75) | 91.71 ns (91.09 – 92.52) | 11.0 M events/s |
| `parse_and_encode` | `query_256` | 225.73 ns (219.65 – 231.41) | 231.82 ns (226.75 – 240.47) | 4.43 M events/s |
| `parse_and_encode` | `response_257` | 259.66 ns (254.32 – 264.45) | 268.26 ns (265.57 – 270.96) | 3.85 M events/s |

Files: [criterion.csv](criterion.csv) (one row per benchmark with the mean,
median, confidence bounds, standard deviation, and throughput unit) and
`criterion/<group>/<benchmark>/estimates.json` (criterion's own estimates,
copied unchanged).

Reading: on this host one event costs about 46 ns to copy into the pool and
about 230 to 260 ns to parse and encode on a cache hit. Decode plus encode
(about 135 to 150 ns) is roughly 60% of `parse_and_encode`; the rest is the
parser's address and timestamp conversion, the frame validation, and the
lease recycle. The jumbo copy is a single `memcpy` of 4.3 KiB.

## Manual benchmarks

Both ignored manual benchmarks ran locally, in release with
`cargo test-release` (the same unwind caveat as above). Neither needs an
elevated session or the DNS Server role.

Commands (the expansion of the `cargo test-release` alias):

```powershell
cargo test --locked --release --config 'profile.release.panic="unwind"' -- --ignored --nocapture synthetic_pooled_intake_benchmark
cargo test --locked --release --config 'profile.release.panic="unwind"' -- --ignored --nocapture parse_encode_benchmark
```

### `synthetic_pooled_intake_benchmark`

Defaults: `ETW_BENCH_CAPACITY=8192`, `ETW_BENCH_EVENTS=200000`,
`ETW_BENCH_MODE=steady`. The consumer thread drains between 2 ms parks, so at
this offered rate most events find the channel full and take the drop path.
The elapsed time therefore measures the callback with a full channel more than
the copy. 200,000 callbacks in 10,709,700 ns is 53.5 ns per offered event.

```text
ETW_BENCH_JSON={"schema":1,"capacity":8192,"regular_capacity":8194,"regular_bytes":2048,"jumbo_capacity":64,"jumbo_bytes":131184,"mode":"steady","offered":200000,"accepted":8878,"dequeued":8878,"intake_dropped":191122,"ring_dropped":191122,"pool_exhausted":0,"receiver_disconnected":0,"callback_errors":0,"final_backlog":0,"elapsed_ns":10709700,"allocator_fixtures":{"success":{"allocations":0,"reallocations":0,"deallocations":0},"channel_full":{"allocations":0,"reallocations":0,"deallocations":0},"recycle":{"allocations":0,"reallocations":0,"deallocations":0},"pool_exhausted":{"allocations":0,"reallocations":0,"deallocations":0},"jumbo_success":{"allocations":0,"reallocations":0,"deallocations":0},"jumbo_exhausted":{"allocations":0,"reallocations":0,"deallocations":0},"disconnected":{"allocations":0,"reallocations":0,"deallocations":0},"malformed":{"allocations":0,"reallocations":0,"deallocations":0},"reentrant":{"allocations":0,"reallocations":0,"deallocations":0},"tls_wrong":{"allocations":0,"reallocations":0,"deallocations":0},"tls_missing":{"allocations":0,"reallocations":0,"deallocations":0}},"recycle_verified":true}
```

### `parse_encode_benchmark`

Default `ETW_PARSE_BENCH_EVENTS=2000000`. Each iteration parses one of three
cached-schema fixture events (256, 257, 258, each with a 17-byte packet) and
encodes it. Unlike the criterion `parse_and_encode` group, the three events
are reused and never recycled, so no pool or `Arc` work is included, and the
packets are smaller. That is why it reports about 139 ns per event against
226 to 260 ns in criterion.

```text
parse_encode_benchmark iterations=2000000 elapsed_ns=278067600 ns_per_event=139.03 bytes=126000000 parser_size=824
```

## Files

| File | Content |
| --- | --- |
| `README.md` | This record |
| `sha256.txt` | SHA-256 of the release `etw-dnstap.exe`, in the format of `datasets/baselines/arrayqueue-v2/sha256.txt`. The binary itself is not in the clone (`*.exe` is ignored). |
| `criterion.csv` | The criterion numbers, one row per benchmark |
| `criterion/<group>/<benchmark>/estimates.json` | Criterion's estimates for each benchmark |
