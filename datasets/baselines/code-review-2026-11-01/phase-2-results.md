# Phase 2 results, code review baseline 2026-11-01

> **Status:** Historical · **Evidence dated:** 2026-11-02 · **Commit:** see the [baseline record](README.md#code-under-measurement) · **Reviewed:** 2026-11-02

This file holds the Phase 2 performance experiments and the Phase 2 lab
comparison. They compare against the Phase 0 numbers in the
[baseline record](README.md). Do not edit the results.

The Phase 0 numbers in the [baseline record](README.md) are unchanged. Phase 2 of
[docs/plans/code-review-remediation-plan.md](../../../docs/plans/code-review-remediation-plan.md)
ran four experiments, one commit each, in the plan's order: item 9 (fat LTO and
one codegen unit), item 10 (a leaked `&'static PoolManager` instead of an
`Arc`), item 11 (the extended-data item tables out of the slot header), and
item 12 (cache the socket timeouts). All four were kept. A preparatory commit
adds a `socket_write` benchmark group, because no existing group could see
item 12.

## Method

- Host and toolchain as in the Phase 0 sections of the [baseline record](README.md). Every experiment was
  measured inside one session with the A/B/A protocol: the criterion binary of the
  base commit (`before`), then the binary with the change (`after`), then the base
  binary again (`before again`). Each binary was built once, copied aside, and run
  with `--save-baseline`; the tables compare the saved means. A session takes
  about 20 minutes.
- Each run was started with `start /wait /high /affinity 50`, which requests
  High priority and logical processors 4 and 6 (mask `0x50`), as the Phase 0
  protocol does. The pin was requested; it was not read back in this session.
- The host was quiet. The processes `cargo`, `rustc`, `link`, `cl`, and `miri` were
  listed before and after each run, and none was present. No build ran during a
  measurement.
- `Change` is `after` against the mean of the two `before` runs. `Drift` is
  `before again` against `before`: what the host did between the two `before`
  runs. A change counts as a gain only if it exceeds that drift and the Phase 0
  bound for its group (about 5% warm, 20% cold and two-thread) in the expected
  direction.
- The criterion binary is built with `panic = "unwind"`; the shipped binary uses
  `panic = "abort"`. The numbers are close to, not identical to, what ships.

## Item 9: `lto = "fat"` and `codegen-units = 1` (kept)

Commit `e396871`. Two lines in `[profile.release]`, and a comment.

| Measure | Without LTO (this base commit) | With LTO | Change |
| --- | ---: | ---: | ---: |
| Release image, `cargo build --locked --release` | 2,396,672 bytes | 2,211,840 bytes | -7.7% |
| Release image at the Phase 0 commit | 2,455,552 bytes | 2,211,840 bytes | -9.9% |
| Cold release build (standard library and 61 crates) | 20 s | 31 s | +52% |
| `cargo test --release` build with `panic = "unwind"` | 26 s | 49 s | +88% |
| `LNK2047` lines in the link output | 8 | 8 | same set |

The `LNK2047` object set of the LTO link equals `scripts/ci/link-exceptions.txt`
(the eight ring NASM objects `aesni`, `chacha`, `p256-x86_64-asm`, `sha256`,
`sha512`, `vpaes`, `x86_64-mont`, and `x86_64-mont5`), so the allowlist needs no
change.

Criterion, before = no LTO, after = LTO, all groups:

| Benchmark | before (ns) | after (ns) | before again (ns) | Change | Drift |
| --- | ---: | ---: | ---: | ---: | ---: |
| `copy_record/jumbo_response_257` | 48.01 | 48.70 | 48.01 | +1.4% | +0.0% |
| `copy_record/regular_query_256` | 35.43 | 34.30 | 34.41 | -1.8% | -2.9% |
| `copy_record/regular_response_257` | 35.57 | 33.94 | 35.40 | -4.3% | -0.5% |
| `copy_record_cold/regular_query_256` | 62.92 | 59.45 | 61.15 | -4.2% | -2.8% |
| `copy_record_cold/regular_response_257` | 80.62 | 77.51 | 79.46 | -3.2% | -1.4% |
| `decode/failure_258` | 31.58 | 30.84 | 31.41 | -2.1% | -0.5% |
| `decode/query_256` | 22.86 | 24.45 | 22.78 | +7.2% | -0.3% |
| `decode/query_256_ipv6` | 23.56 | 24.72 | 23.59 | +4.9% | +0.1% |
| `decode/response_257` | 32.51 | 32.49 | 32.44 | +0.0% | -0.2% |
| `decompose/parse_encode_retained/failure_258` | 150.16 | 136.43 | 149.80 | -9.0% | -0.2% |
| `decompose/parse_encode_retained/query_256` | 142.40 | 128.41 | 141.75 | -9.6% | -0.5% |
| `decompose/parse_encode_retained/query_256_ipv6` | 190.50 | 169.09 | 191.00 | -11.4% | +0.3% |
| `decompose/parse_encode_retained/response_257` | 152.77 | 138.90 | 150.43 | -8.4% | -1.5% |
| `decompose/recycle/query_256` | 10.95 | 10.32 | 11.03 | -6.1% | +0.7% |
| `decompose/recycle/response_257` | 11.11 | 10.43 | 11.08 | -5.9% | -0.3% |
| `encode/failure_258` | 67.21 | 57.69 | 66.53 | -13.7% | -1.0% |
| `encode/query_256` | 66.69 | 57.21 | 66.60 | -14.1% | -0.1% |
| `encode/query_256_ipv6` | 67.70 | 57.63 | 67.55 | -14.8% | -0.2% |
| `encode/response_257` | 68.06 | 59.11 | 67.79 | -13.0% | -0.4% |
| `parse_and_encode/failure_258` | 159.77 | 144.02 | 159.88 | -9.9% | +0.1% |
| `parse_and_encode/query_256` | 152.79 | 137.08 | 152.81 | -10.3% | +0.0% |
| `parse_and_encode/query_256_ipv6` | 203.91 | 176.68 | 203.82 | -13.3% | -0.0% |
| `parse_and_encode/response_257` | 200.02 | 146.96 | 164.38 | -19.3% | -17.8% |
| `two_thread/query_256` | 248.30 | 223.24 | 251.91 | -10.7% | +1.5% |
| `two_thread/response_257` | 262.99 | 237.47 | 263.99 | -9.9% | +0.4% |

Reading. LTO gains 9 to 11% on `parse_encode_retained`, 10 to 13% on
`parse_and_encode`, about 14% on `encode`, 10% on `two_thread`, 6% on the lease
recycle, and 3 to 4% on the cold copy. `decode` alone is up to 7% slower for
the query events (its
wrapper is `#[inline(never)]`, so LTO cannot inline it into the caller; the
composed path is faster). `parse_and_encode/response_257` shows an 18% drift
because its first `before` run was disturbed (200.0 ns against 164.4 ns in the
second); against the second `before` run its change is -10.6%, in line with its
siblings. The cold and two-thread drift stays inside the 20% bound.

Manual benchmarks (release, `panic = "unwind"`, pinned, alternating):

| Benchmark | Without LTO | With LTO |
| --- | ---: | ---: |
| `parse_encode_benchmark`, ns per event, four runs | 235.4, 230.4, 237.6, 230.2 | 126.0, 125.8, 126.3, 126.9 |
| `synthetic_pooled_intake_benchmark`, ns per offered event, three runs | 57.6, 61.0, 60.4 | 52.5, 51.4, 51.0 |

The synthetic figure moves with how many of the 200,000 offered events are
accepted (8,826 to 90,524 here), so it is a smoke test. Without LTO,
`parse_encode_benchmark` at this commit reads 230 to 238 ns, against 139.6 to
142.0 ns in the Phase 0 record, although criterion's `parse_encode_retained`
(142.4 ns here) still matches Phase 0 (141.9 ns). Why this test-profile binary
is 65% slower than Phase 0 without LTO, and 10% faster with it, was not
established; several Phase 1 commits changed `src/etw/schema.rs` and its tests
after Phase 0. Criterion is the comparison to trust.

## Item 10: `&'static PoolManager` instead of `Arc<PoolManager>` (kept)

Commit `8d1b172` (and `2ec1796`, which passes the now-`Copy` bench `Pool` by
value to satisfy clippy). `PoolManager::with_progress` leaks the pool once, after
every slot is allocated, and returns `&'static PoolManager`; the lease, the
callback context, the per-thread producer, and `Capture::start` hold the
reference, and the producer check compares addresses with `ptr::eq`. A failed
start frees the slots allocated so far. The recycle abort-on-full invariant is
untouched. A test that builds a pool leaks it (a few slots; the proptest leaks
about 130 KiB per case).

| Benchmark | before (ns) | after (ns) | before again (ns) | Change | Drift |
| --- | ---: | ---: | ---: | ---: | ---: |
| `copy_record/jumbo_response_257` | 44.27 | 44.26 | 45.11 | -1.0% | +1.9% |
| `copy_record/regular_query_256` | 34.77 | 25.08 | 35.10 | -28.2% | +0.9% |
| `copy_record/regular_response_257` | 34.09 | 26.39 | 34.37 | -22.9% | +0.8% |
| `copy_record_cold/regular_query_256` | 55.66 | 47.50 | 58.70 | -16.9% | +5.5% |
| `copy_record_cold/regular_response_257` | 74.18 | 71.61 | 80.39 | -7.3% | +8.4% |
| `decode/failure_258` | 31.01 | 30.61 | 31.11 | -1.4% | +0.3% |
| `decode/query_256` | 24.26 | 23.91 | 24.14 | -1.2% | -0.5% |
| `decode/query_256_ipv6` | 24.72 | 24.40 | 24.68 | -1.2% | -0.2% |
| `decode/response_257` | 33.02 | 32.20 | 33.21 | -2.8% | +0.6% |
| `decompose/parse_encode_retained/failure_258` | 136.48 | 138.30 | 136.72 | +1.2% | +0.2% |
| `decompose/parse_encode_retained/query_256` | 129.28 | 131.07 | 128.69 | +1.6% | -0.5% |
| `decompose/parse_encode_retained/query_256_ipv6` | 167.72 | 182.54 | 168.40 | +8.6% | +0.4% |
| `decompose/parse_encode_retained/response_257` | 140.08 | 139.95 | 138.17 | +0.6% | -1.4% |
| `decompose/recycle/query_256` | 10.32 | 6.09 | 10.34 | -41.0% | +0.2% |
| `decompose/recycle/response_257` | 10.40 | 6.18 | 10.46 | -40.7% | +0.5% |
| `encode/failure_258` | 57.28 | 57.05 | 56.91 | -0.1% | -0.6% |
| `encode/query_256` | 57.77 | 57.11 | 57.64 | -1.0% | -0.2% |
| `encode/query_256_ipv6` | 57.67 | 57.81 | 57.85 | +0.1% | +0.3% |
| `encode/response_257` | 58.50 | 59.41 | 58.53 | +1.5% | +0.1% |
| `parse_and_encode/failure_258` | 143.49 | 142.92 | 145.32 | -1.0% | +1.3% |
| `parse_and_encode/query_256` | 137.55 | 136.24 | 137.09 | -0.8% | -0.3% |
| `parse_and_encode/query_256_ipv6` | 176.13 | 185.50 | 177.58 | +4.9% | +0.8% |
| `parse_and_encode/response_257` | 146.60 | 145.91 | 146.88 | -0.6% | +0.2% |
| `two_thread/query_256` | 205.41 | 210.57 | 214.59 | +0.3% | +4.5% |
| `two_thread/response_257` | 217.64 | 222.44 | 225.30 | +0.4% | +3.5% |

Reading.

- Where the change can be seen it is large: `copy_record` -28% (query) and -23%
  (response), the lease recycle -41% (10.3 to 6.1 ns). That is about 14 ns of CPU
  per event: 9.7 ns on the ETW thread and 4.2 ns on the parser thread.
- `two_thread` shows no change (+0.3%, +0.4%) inside a drift of 4.5% and 3.5%.
  That group is limited by the parser thread, so the 4.2 ns saved there is 2% of
  210 ns, below what the group resolves. The contended-cache-line cost that the
  plan predicted for the ETW thread is hidden when the producer waits on a full
  channel; the lab run is the place to look for it.
- The cold copy gains 17% (query) and 7% (response), but the drift is 5.5% and
  8.4%, so only the query result counts, and only with that caveat.
- The IPv6 query parse is 5 to 9% slower in this binary. A/B/A/B on the four
  `ipv6` benchmarks reproduces it (`parse_and_encode` 178.5 and 175.7 ns before,
  187.8 and 189.3 ns after; `parse_encode_retained` 170.0 and 168.9 ns before,
  182.1 and 184.0 ns after), while the IPv4 parses are unchanged. No code on that
  path changed, and the effect disappears in the next commit's binary (-7.5%
  there), so it reads as a code-layout effect of this particular build. It costs
  about 10 to 14 ns on an IPv6 query only.

## Item 11: item tables at the end of the slot bytes (kept)

Commit `eecc74e`. The `Slot` header goes from 408 to 24 bytes. A record's item
table (16 bytes per item: a packed header word and the payload offset; `DataPtr`
is never stored) lives in the last `16 * count` bytes of the slot's own byte
buffer, item `i` being the `i`-th 16-byte cell counted from the end, so no count
is needed to find it. `copy_record` keeps its preflight (every offset and the
aggregate length are checked before any copy), then asks `acquire` for the
payload, its padding, and the cells, so the two regions cannot overlap. Cost: a
record with extended data that used to fill a regular slot to within `16 * count`
bytes now takes a jumbo slot, and `JUMBO_BYTES` grows from 131,184 to 131,448. This provider sends no
extended data. The budget in `docs/design/pooled-intake.md`,
`docs/design/ring-buffer.md`, and `docs/guide/configuration.md` is recomputed
(59.59 MiB at the defaults, was 68.60; 145.53 MiB at `--capacity 65536`, was
169.54). The `x64_storage_sizes_match_documented_budget` test now asserts a
24-byte `Slot` and the 131,448-byte jumbo size.

| Benchmark | before (ns) | after (ns) | before again (ns) | Change | Drift |
| --- | ---: | ---: | ---: | ---: | ---: |
| `copy_record/jumbo_response_257` | 38.66 | 42.51 | 39.03 | +9.5% | +1.0% |
| `copy_record/regular_query_256` | 24.95 | 25.01 | 25.63 | -1.1% | +2.7% |
| `copy_record/regular_response_257` | 26.18 | 24.37 | 26.85 | -8.1% | +2.6% |
| `copy_record_cold/regular_query_256` | 53.84 | 35.07 | 57.34 | -36.9% | +6.5% |
| `copy_record_cold/regular_response_257` | 71.11 | 58.31 | 74.42 | -19.9% | +4.7% |
| `decode/failure_258` | 30.50 | 31.55 | 31.34 | +2.0% | +2.8% |
| `decode/query_256` | 23.76 | 24.26 | 23.93 | +1.7% | +0.8% |
| `decode/query_256_ipv6` | 24.22 | 24.77 | 24.81 | +1.0% | +2.5% |
| `decode/response_257` | 32.26 | 33.24 | 33.41 | +1.3% | +3.6% |
| `decompose/parse_encode_retained/failure_258` | 137.33 | 137.97 | 141.15 | -0.9% | +2.8% |
| `decompose/parse_encode_retained/query_256` | 131.14 | 128.97 | 136.55 | -3.6% | +4.1% |
| `decompose/parse_encode_retained/query_256_ipv6` | 182.05 | 170.90 | 183.57 | -6.5% | +0.8% |
| `decompose/parse_encode_retained/response_257` | 142.58 | 140.69 | 141.14 | -0.8% | -1.0% |
| `decompose/recycle/query_256` | 6.10 | 6.11 | 6.11 | +0.0% | +0.2% |
| `decompose/recycle/response_257` | 6.18 | 6.22 | 6.21 | +0.4% | +0.4% |
| `encode/failure_258` | 56.98 | 57.21 | 57.26 | +0.2% | +0.5% |
| `encode/query_256` | 56.75 | 57.32 | 58.07 | -0.2% | +2.3% |
| `encode/query_256_ipv6` | 58.23 | 57.73 | 57.65 | -0.4% | -1.0% |
| `encode/response_257` | 58.38 | 58.29 | 58.79 | -0.5% | +0.7% |
| `parse_and_encode/failure_258` | 142.47 | 142.29 | 142.77 | -0.2% | +0.2% |
| `parse_and_encode/query_256` | 134.61 | 132.55 | 138.39 | -2.9% | +2.8% |
| `parse_and_encode/query_256_ipv6` | 187.34 | 174.29 | 189.56 | -7.5% | +1.2% |
| `parse_and_encode/response_257` | 147.55 | 144.67 | 148.29 | -2.2% | +0.5% |
| `two_thread/query_256` | 213.75 | 199.42 | 211.72 | -6.3% | -1.0% |
| `two_thread/response_257` | 229.05 | 213.73 | 224.36 | -5.7% | -2.0% |

Pool private bytes (`copy_record_cold`, 65,538 regular and 64 jumbo slots):

| Binary | Private bytes added by the pool |
| --- | ---: |
| Before (two runs) | 176,439,296 and 175,861,760 |
| After | 149,278,720 |

That is -27.0 MB (-15.2%) against the 25.2 MB of header bytes the Phase 0 record
predicted; the rest is probably heap block rounding (408 to 416 bytes, 24 to 32). The counted storage at this size is now 152,598,608
bytes and the pool holds 98.0% of it as private bytes.

Reading.

- The cold copy gains 37% (query, 55.6 to 35.1 ns) and 20% (response, 72.8 to
  58.3 ns), against drifts of 6.5% and 4.7%. A slot header of 24 bytes shares a
  cache line with its neighbours instead of occupying a line of its own beside
  the payload, which the cold pool misses on every lease.
- `two_thread` gains 6.2% (query) and 5.7% (response), a drift of -1.0% and
  -2.0%.
- `copy_record/jumbo_response_257` is 9.5% slower (drift 1.0%). Only the jumbo
  slot size changed (131,184 to 131,448 bytes); it reads as an allocation
  alignment effect on a 4 KiB copy and affects jumbo records only.
- The IPv6 parse regression of item 10 is gone in this binary (-6.5% and -7.5%).

## Item 12: cache the socket timeouts (kept)

Commit `50e9298`; the `socket_write` group is in `05941c1`. `DeadlineTcp` keeps
the last read and write timeout it applied and calls `set_read_timeout` or
`set_write_timeout` again only when the time left is below the applied value, or
exceeds it by more than the slack (the smaller of 5 ms and `timeout / 32`). A
kept timeout is therefore never shorter than the time left, so no call is cut
short of the deadline, and at most the slack longer, so a blocked call can run
at most 5 ms past a 2 s deadline. `remaining()` still checks the deadline on
every call. This is not exact to the nanosecond: the plan asked for "within a few
milliseconds" and that is the tolerance. Four new tests cover the rule, the
slack, the once-per-window behavior, and a blocked write ending near its
deadline with a kept timeout.

The group writes to a loopback connection that a second thread drains, in a
fresh deadline window per write (as the frame writer does), three A/B pairs:

| Benchmark | Before, runs 1 to 3 (us) | After, runs 1 to 3 (us) | Change of the means |
| --- | --- | --- | ---: |
| `socket_write/frame_103` | 9.21 / 9.36 / 9.34 | 8.92 / 8.82 / 8.88 | -4.6% |
| `socket_write/flush_64k` | 87.18 / 86.56 / 86.07 | 82.14 / 155.24 / 74.02 | +19.9% |

Reading.

- `frame_103` is 4.6% faster (about 0.45 us per write) in all three pairs and
  outside every confidence interval (for example 9.34 us, 95% CI 9.30 to 9.39,
  against 8.88 us, CI 8.84 to 8.93). That is the cost of one `setsockopt` call.
- `flush_64k` cannot resolve it. Its 95% confidence intervals are 8% to 90% wide
  (one `after` run read 155 us, CI 94 to 237), because the receiving thread and
  the scheduler dominate a 64 KiB loopback send. The change in the means is not a
  result.
- The service makes one 64 KiB write per roughly 600 events, so the saving is
  about 0.7 ns per event, 0.5% of the 127 ns parse and encode. This is the
  smallest of the four changes; it is kept because the group designed to see it
  does, and TLS, which writes several records per window, skips more calls (not
  measured). A reviewer
  who prefers no timeout slack can drop this commit alone.

## All four together

Same A/B/A protocol: `before` is the Phase 2 base commit (`e61179d`, the plan
branch after Phase 1, no LTO), `after` is the final commit.

| Benchmark | before (ns) | after (ns) | before again (ns) | Change | Drift |
| --- | ---: | ---: | ---: | ---: | ---: |
| `copy_record/jumbo_response_257` | 50.28 | 42.27 | 47.86 | -13.9% | -4.8% |
| `copy_record/regular_query_256` | 34.74 | 25.12 | 33.83 | -26.8% | -2.6% |
| `copy_record/regular_response_257` | 34.77 | 25.27 | 35.31 | -27.9% | +1.6% |
| `copy_record_cold/regular_query_256` | 57.57 | 34.93 | 61.82 | -41.5% | +7.4% |
| `copy_record_cold/regular_response_257` | 78.77 | 58.81 | 79.84 | -25.8% | +1.3% |
| `decode/failure_258` | 31.72 | 31.51 | 31.45 | -0.2% | -0.8% |
| `decode/query_256` | 23.30 | 24.77 | 22.82 | +7.4% | -2.1% |
| `decode/query_256_ipv6` | 23.79 | 25.02 | 23.69 | +5.4% | -0.4% |
| `decode/response_257` | 32.69 | 33.76 | 32.17 | +4.1% | -1.6% |
| `decompose/parse_encode_retained/failure_258` | 152.76 | 133.86 | 151.20 | -11.9% | -1.0% |
| `decompose/parse_encode_retained/query_256` | 141.79 | 124.35 | 141.42 | -12.2% | -0.3% |
| `decompose/parse_encode_retained/query_256_ipv6` | 192.69 | 172.34 | 191.66 | -10.3% | -0.5% |
| `decompose/parse_encode_retained/response_257` | 154.29 | 134.99 | 152.75 | -12.1% | -1.0% |
| `decompose/recycle/query_256` | 10.96 | 6.07 | 10.94 | -44.6% | -0.2% |
| `decompose/recycle/response_257` | 11.08 | 6.20 | 11.02 | -43.8% | -0.6% |
| `encode/failure_258` | 67.10 | 57.70 | 66.41 | -13.6% | -1.0% |
| `encode/query_256` | 67.81 | 58.14 | 67.00 | -13.7% | -1.2% |
| `encode/query_256_ipv6` | 67.68 | 58.61 | 67.35 | -13.2% | -0.5% |
| `encode/response_257` | 68.14 | 59.09 | 67.37 | -12.8% | -1.1% |
| `parse_and_encode/failure_258` | 162.76 | 136.85 | 160.89 | -15.4% | -1.1% |
| `parse_and_encode/query_256` | 153.73 | 127.10 | 152.78 | -17.1% | -0.6% |
| `parse_and_encode/query_256_ipv6` | 205.14 | 176.55 | 200.87 | -13.0% | -2.1% |
| `parse_and_encode/response_257` | 167.18 | 139.82 | 164.36 | -15.7% | -1.7% |
| `two_thread/query_256` | 233.89 | 197.39 | 247.22 | -17.9% | +5.7% |
| `two_thread/response_257` | 242.91 | 205.39 | 259.48 | -18.2% | +6.8% |

| Measure | Phase 2 base | Final |
| --- | ---: | ---: |
| Release image | 2,396,672 bytes | 2,210,304 bytes |
| Pool private bytes (`copy_record_cold` pool) | 177,303,552 and 177,192,960 | 149,606,400 |
| `synthetic_pooled_intake_benchmark`, ns per offered event (three runs; with LTO, before items 10 and 11 versus after) | 53.2, 53.3, 53.7 | 40.4, 41.9, 42.2 |
| `parse_encode_benchmark`, ns per event (with LTO, items 10 to 12) | 125.7 to 126.3 | 125.2 to 128.8 |

Against the Phase 0 table: `two_thread/query_256` 231.5 to 197.4 ns (-14.7%),
`two_thread/response_257` 245.8 to 205.4 ns (-16.4%), `parse_and_encode/query_256`
153.0 to 127.1 ns (-16.9%). The `before` run of this session reproduces the
Phase 0 means within 1% (`two_thread/query_256` 233.9 ns against 231.5).

## Verification after the measurements

The tool that launched every command in the measurement session refused each
invocation of `pwsh` or `powershell.exe` (a worktree-isolated agent could not
show that a PowerShell command stays inside its worktree), so the two CI scripts
did not run there. They were run later on the build host, after the measurements,
on a fresh `cargo build --locked --release` of the branch at `ded1faa` (image
2,210,304 bytes):

- `scripts/ci/verify-link-exceptions.ps1`: 6 checks, 0 failed (8 `LNK2047` lines,
  8 distinct objects, all allowlisted, no `LNK2046`).
- `scripts/ci/verify-pe.ps1`: 21 checks, 0 failed.

During the measurement session, a throwaway port of the 21 `verify-pe.ps1`
checks (same dumpbin switches, patterns, and masks) and a comparison of the
`LNK2047` object set in the captured build log with
`scripts/ci/link-exceptions.txt` (equal, 8 and 8) had already given the same
answers.

The lab comparison that Phase 2 requires (Windows PowerShell 5.1 and
PowerShell Direct to the guest) was run afterwards and is recorded in
[Phase 2 lab comparison](#phase-2-lab-comparison) below. It found 11.5% fewer
cycles per event in the shipped service and 13% less private memory, with no
change in what the receiver gets.

## Phase 2 lab comparison

Run on 2026-11-02 on the lab guest `DNSTap-WS2025`. It compares the two shipped
release images under the same load: the integration branch before Phase 2
(`e61179d`) against the Phase 2 merge (`25742af`), all four changes together.
It does not isolate one change. It is the check that the criterion sections
above could not make: real thread contention, the real `panic = "abort"` build,
and the real ETW, socket, and receiver path.

| | Base (before Phase 2) | Phase 2 merge |
| --- | --- | --- |
| Commit | `e61179d` | `25742af` |
| Image | 2,458,112 bytes | 2,210,816 bytes (-10.1%) |
| SHA-256 | `40e63f9ddd2b089b21b046eafe0f21129b74dfcde0a3dbe464d106c77775ed8e` | `ef31e6593fc91534493e9bfc2e0d75618c1176b28c140dea3a81d91fa393b3d0` |

Both were built with `cargo build --locked --release` and passed
`scripts/ci/verify-pe.ps1` (21 of 21). The hashes are in [sha256.txt](sha256.txt).

### Method

- Setup. The guest is a Hyper-V VM with 6 vCPUs and 4 GiB (`lab.json` still
  says 2 vCPUs). The service runs as `NT AUTHORITY\LocalService` with the lab's
  installed command line (`--receiver ... --session-name EtwDnstap-Lab --plaintext --stats-interval-seconds 5`)
  and every other option at its default: `capacity=24576`, `etw_buffer_kib=16`,
  `etw_max_buffers=64`. Both images logged the same `capture started` line.
- Load. The paced-schedule build of `tools/dns-stress` (commit `6d49577`; not
  rebuilt for this run; client SHA-256 `82cfb758153db8bf2e00e0fb25e932b84154db2737b8d52cba05a3d4e61d38a6`),
  on the Hyper-V host, authoritative mode, 4 threads, `--inflight 1024`,
  `--qps 10000`, 120 s, so 1,200,000 queries and 2,400,000 events per run. Two
  60 s pilots (base at 5,000 QPS, Phase 2 at 10,000 QPS; kept as `run-pilot-*.json`)
  showed the pipeline working and no drops at either rate, so the matrix uses
  10,000 QPS. The earlier lab runs that
  dropped events at this rate used `capacity=8192`; the default is now 24,576.
- Order. Base, Phase 2, Base, Phase 2, Base, Phase 2 (`r1` to `r6`), 25 s idle
  between runs. Every run stopped the service, copied the image over the
  installed executable when it differed (hash and executable SDDL checked), started
  the service, waited 8 s, and then took the start snapshot. The service was
  therefore restarted for every run, and all counters start at zero.
- Receiver. DNS-collector 3.2.3 on `DNSTap-Ubuntu2604`. Its pipeline ends in the
  `devnull` logger (`routing-policy: forward: [discard]`), trace logging is off,
  and the unit sets `StandardOutput=null` and `StandardError=null`. It was
  already in that state, so nothing was changed on the receiver. Delivery is
  counted from `ss -tin` `bytes_received` on the receiver socket, before the load
  and 6 s after it, divided by 103.01 bytes per frame.
- Per run, on the guest, at the start and 6 s after the client ended: the
  service process CPU time, the private bytes and working set, the cycle time of
  every thread (`QueryThreadCycleTime`; threads are named with
  `GetThreadDescription`: `dnstap-parser`, `etw-consumer`, and everything else
  as `other`), and the EtwDnstap performance counters (Events Received, Frames
  Sent, Events Dropped, and the drop counters). The deltas cover the 120 s load
  and about 14 s of idle at the edges. Private bytes were also polled every 10 s
  during the load. After the stop, the `capture stopped` event log line was read.
- Quiet host. Before each run `tasklist` showed no `cargo`, `rustc`, `link`,
  `cl`, `lld`, or `miri` process, and the run script checked again every 10 s
  and at the end. The count was 0 in all six runs.
- Restore. The installed executable (`a37729cb...`), the service command line,
  the service state (Stopped), the executable SDDL, and the SCM SDDL were
  compared with the state saved before the first run and matched after the last
  ([guest-state.json](lab/guest-state.json)). The staging directory on the guest was removed.

Scripts and raw data are in [lab/](lab/): `run-<run>.json` (one per run, with
the snapshots, the polls, and the event log lines), `runs.csv` (one row per
run), `summary.csv` (medians and spreads), `guest-state.json`, and `tools/` (the
scripts that ran the lab, and the analyzer; `tools/common.ps1` has the user
module directory removed from the `PSModulePath` line).

### Per run

Delivery, at 10,000 QPS requested for 120 s:

| Run | Image | QPS achieved | Events received (counter) | Frames delivered (receiver bytes / 103.01) | Service drops | Peak queue depth |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| r1 | base | 9,999.97 | 2,399,992 | 2,400,113 | 0 | 5,663 |
| r2 | Phase 2 | 10,000.00 | 2,400,000 | 2,400,121 | 0 | 6,367 |
| r3 | base | 9,999.97 | 2,399,992 | 2,400,113 | 0 | 5,777 |
| r4 | Phase 2 | 10,000.00 | 2,400,000 | 2,400,121 | 0 | 6,043 |
| r5 | base | 10,000.00 | 2,400,000 | 2,400,121 | 0 | 5,701 |
| r6 | Phase 2 | 10,000.00 | 2,400,000 | 2,400,121 | 0 | 7,263 |

"Service drops" is the Events Dropped counter (drops at admission, which
includes Channel Full Drops and Pool Exhausted Drops). All six runs also had `etw_lost=0`,
`pool_exhausted=0`, zero client timeouts and send errors, one receiver
connection, and a clean stop (exit code 0). The counter `Frames Sent` (frames encoded) equals
`Events Received` in every run.

Cost. Cycles are thread cycle times in billions. Service CPU is
`Process.TotalProcessorTime`, which advances in 15.6 ms ticks.

| Run | Image | Service CPU (s) | `dnstap-parser` Gcycles | `etw-consumer` Gcycles | Total Gcycles | Cycles per event | Frames per CPU second | Frames per Gcycle | Private MiB at start / end |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| r1 | base | 1.656 | 3.564 | 3.664 | 7.343 | 3,060 | 1,449,125 | 326,844 | 72.86 / 73.16 |
| r2 | Phase 2 | 1.297 | 3.099 | 3.302 | 6.514 | 2,714 | 1,850,696 | 368,457 | 63.38 / 63.75 |
| r3 | base | 1.453 | 3.668 | 3.771 | 7.551 | 3,146 | 1,651,691 | 317,870 | 72.91 / 73.23 |
| r4 | Phase 2 | 1.469 | 3.144 | 3.319 | 6.574 | 2,739 | 1,634,125 | 365,113 | 63.36 / 63.75 |
| r5 | base | 1.813 | 3.578 | 3.711 | 7.403 | 3,085 | 1,324,205 | 324,213 | 72.86 / 73.17 |
| r6 | Phase 2 | 1.641 | 3.123 | 3.315 | 6.550 | 2,729 | 1,462,931 | 366,442 | 63.38 / 63.75 |

The remaining threads (`main` and the unnamed ones) used 0.11 Gcycles in every
run, 1.5% of the total, with no difference between the images.

### Comparison

Median of three runs per image, Phase 2 against base. "Range" is (largest minus
smallest) over the median. "Pairs" are the changes of the adjacent runs (r2
against r1, r4 against r3, r6 against r5).

| Metric | Base | Phase 2 | Change | Range base / Phase 2 | Pairs | Reading |
| --- | ---: | ---: | ---: | ---: | --- | --- |
| Cycles per event, `dnstap-parser` | 1,491 | 1,301 | -12.7% | 2.9% / 1.4% | -13.1, -14.3, -12.7% | Real: ranges do not overlap |
| Cycles per event, `etw-consumer` | 1,546 | 1,381 | -10.7% | 2.9% / 0.5% | -9.9, -12.0, -10.7% | Real: ranges do not overlap |
| Cycles per event, all threads | 3,085 | 2,729 | -11.5% | 2.8% / 0.9% | -11.3, -12.9, -11.5% | Real: ranges do not overlap |
| Frames per Gcycle | 324,213 | 366,442 | +13.0% | 2.8% / 0.9% | +12.7, +14.9, +13.0% | The same result, inverted |
| Service CPU seconds (tick accounting) | 1.656 | 1.469 | -11.3% | 21.7% / 23.4% | -21.7, +1.1, -9.5% | Inside noise: the ranges overlap and one pair has the wrong sign |
| Frames per CPU second (tick accounting) | 1,449,125 | 1,634,125 | +12.8% | 22.6% / 23.7% | +27.7, -1.1, +10.5% | Inside noise, same reason |
| Frames delivered | 2,400,113 | 2,400,121 | 0.0% | 0.0% / 0.0% | | Nothing: both deliver everything offered |
| Service drops | 0 | 0 | | | | Nothing |
| Private bytes at start (idle, pool allocated) | 72.86 MiB | 63.38 MiB | -13.0% | 0.1% / 0.03% | -13.0, -13.1, -13.0% | Real |
| Private bytes at end (idle after load) | 73.17 MiB | 63.75 MiB | -12.9% | 0.1% / 0.0% | -12.9, -12.9, -12.9% | Real |
| Private bytes during load (mean of 10 s polls) | 84.42 MiB | 74.68 MiB | -11.5% | 1.5% / 2.5% | -9.3, -11.5, -12.8% | Real |
| Peak working set | 98.55 MiB | 89.76 MiB | -8.9% | 1.9% / 0.5% | -8.9, -10.8, -8.7% | Real |
| Peak channel depth | 5,701 | 6,367 | +11.7% | 2.0% / 19.2% | +12.4, +4.6, +27.4% | Higher in all three pairs; see below |
| `dns.exe` CPU seconds (not the service) | 26.12 | 27.66 | +5.9% | 9.5% / 3.9% | +5.9, -5.5, +7.7% | Inside noise, as expected |

What the lab shows:

- The service spends 11.5% fewer cycles per event, split over both threads:
  about 190 fewer on `dnstap-parser` and 165 fewer on `etw-consumer`, 355 in
  all. The gain is the same size in each of the three adjacent pairs, and no
  base run is as cheap as any Phase 2 run, so it is not drift. The `etw-consumer`
  thread is the one that leases and fills the pool slot, and the
  `dnstap-parser` thread is the one that encodes and recycles it, so the gain
  appears on both sides of the handoff.
- Size against the host benchmark. The criterion groups above saved, per event, about
  26.6 ns in parse and encode (`parse_and_encode/query_256`), 9.6 ns in the
  warm copy (`copy_record/regular_query_256`), and 4.9 ns in recycle
  (`decompose/recycle/query_256`), about 41 ns in total on one core. If the guest runs near the
  host's 4.4 GHz base clock (not measured), 355 cycles is about 80 ns, twice
  that. It fits the expectation that Phase 2 removes contention cost that the
  single-thread groups cannot see (the `two_thread` group saved 34 ns). This
  experiment compares whole images. It does not show how much of the extra
  comes from the leaked pool (item 10), the smaller slot header (item 11), or
  LTO (item 9).
- The pool memory saving is confirmed in the shipped service: 9.5 MiB less
  private memory at defaults (the counted storage of the pool is 9.01 MiB
  smaller, 59.59 against 68.60 MiB; the rest is the smaller image and its
  data). The cost during load, measured by 10 s polls, is also about 10 MiB lower.
- Nothing changed in what reaches the receiver: both images delivered all
  events offered, with zero drops, and the receiver byte counts match the
  counters.
- Peak channel depth was higher with Phase 2 in all three pairs (6,043 to 7,263
  against 5,663 to 5,777), still about 30% of the 24,576 slots and with no drops.
  The peak is a single maximum over a bursty arrival pattern. With three runs I
  cannot say whether it reflects a faster `etw-consumer` thread draining ETW
  batches into the queue faster than `dnstap-parser` takes them out, or is chance. Watch it
  at higher rates, where a smaller headroom would show as drops. This
  comparison did not test those rates.

### Caveats

- One session, one load level (10,000 QPS, authoritative names), one
  configuration (defaults), and three runs per image. A different rate,
  `--capacity 65536`, the recursive workload, or a drop-inducing rate may
  behave differently. The `--capacity 65536` memory figures in the sections
  above (145.53 against 169.54 MiB of storage) were not measured in the lab.
- The cycle counts are thread cycle times from `QueryThreadCycleTime` inside a
  VM. They are proportional to CPU time on that VM and good for comparing two
  images on it, but they are not converted to time, and they would not compare across
  hosts. The process CPU time uses the 15.6 ms scheduler tick, and its
  run-to-run range (22%) is much larger than the tick size, so it is sampled
  accounting, not a measurement of the difference. Use the cycle counts.
- The window of each delta is the 120 s load plus about 14 s of idle at its
  edges, the same for both images. Threads are matched by thread ID between the
  two snapshots; a thread that existed at the start and had exited by the end
  (start-up threads, under 0.1 Gcycles) is not counted.
- Two base runs (r1, r3) received 8 events fewer than 2 x 1,200,000, and the
  receiver bytes match 8 fewer frames. The service reports `etw_lost=0`, no
  drops, and every client query succeeded, so the 8 events were not delivered
  to the service. The cause was not investigated. It is 0.0003% of the events.
  The 103.01 bytes per frame is an average, which is why the frames
  delivered by bytes are 121 above the counted events (0.005%).
- The base image here is 2,458,112 bytes, but item 9 above lists 2,396,672
  bytes for the Phase 2 base commit. I did not find the reason for the
  61,440-byte difference. The size
  line of the CHANGELOG uses the item 9 figure (-7.7%, same tree before and after
  the profile change). The lab pair is -10.1%.
- The runs alternate base and Phase 2, so a steady drift of the guest would
  look like a trend across `r1`, `r3`, `r5`. The base runs show none (cycles
  per event 3,060, 3,146, 3,085). The guest also runs the DNS Server, which used
  26 to 28 CPU seconds per run with no difference between the images, and the Hyper-V
  host ran at 7% to 9% CPU, so noise from sharing is limited but not controlled.
- The client is the paced-schedule `dns-stress` build named above, not the build
  that `scripts/performance/lab.json` points to (SHA-256 `4d545060...`), which the
  2026-11-01 lab notes record as not holding the target rate at 30,000 QPS and above.
