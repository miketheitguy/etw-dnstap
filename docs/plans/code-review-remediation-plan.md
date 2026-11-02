# Code review remediation plan

> **Status:** Approved. Phases 0 to 4 executed on the `plan/code-review-remediation` branch; the branch awaits the user's decision on merging to `main` · **Evidence dated:** 2026-11-01 · **Commit:** written at `fc983ab` · **Reviewed:** 2026-11-01

This plan records the findings of the 2026-11-01 Rust code review of `main` at
`fc983ab` and the steps that act on them. The review covered performance,
error handling, coding style, and security, using the installed Rust and
Windows service skills, Clippy with the nursery, cargo, and restriction lint
groups, cargo-deny, and three external sources (Microsoft Learn on
`SERVICE_STATUS`, RUSTSEC-2025-0007, and the `rustls-platform-verifier` Windows
verifier source).

## Baseline at `fc983ab`

| Check | Result |
| --- | --- |
| `cargo test --locked` | 259 passed, 0 failed, 7 ignored |
| `cargo clippy --locked --all-targets -- -D warnings` (project lints, pedantic on) | clean |
| `cargo fmt --check` | clean |
| `cargo deny --locked check` | advisories, bans, licenses, sources ok |
| Extended sweep: nursery, cargo, and selected restriction lints | 639 warnings. 391 are `missing_docs_in_private_items`. Zero hits for the nursery group, `cognitive_complexity`, `too_many_lines`, `large_stack_frames`, `significant_drop_tightening`, and `unreachable_pub` |

The extended sweep is not a gate. It was run once to find candidates and is not
added to CI by this plan.

## Findings

Severity is the reviewer's estimate of operational risk. Item numbers are used
by the phases below.

### Security and correctness

1. **Medium. The SCM wait hint can expire during a large startup.** The service
   reports `SERVICE_START_PENDING` once with a 15,000 ms hint
   (`src/service.rs`, `status_record`), then allocates the event pool. The
   startup budget (`src/pipeline.rs`, `startup_budget`) allows up to 26 s at the
   maximum `--capacity`. Microsoft Learn (`SERVICE_STATUS`, `dwWaitHint`): if the
   hint passes and neither `dwCheckPoint` nor `dwCurrentState` changed, "the
   service control manager or service control program can assume that an error
   has occurred and the service should be stopped." Fix: derive the hint from
   the startup budget, or report `SERVICE_START_PENDING` with an incremented
   checkpoint from inside pool allocation at a fixed slot interval. Verify in
   the lab with `--capacity 262144`.
2. **Medium. The revocation claim is stronger than the verifier's behavior.**
   README.md, SECURITY.md, the configuration guide, and the `--receiver` help
   text say the receiver certificate is verified "including revocation." The
   current `rustls-platform-verifier` Windows source sets only
   `CERT_CHAIN_REVOCATION_CHECK_END_CERT` and passes
   `CERT_CHAIN_POLICY_IGNORE_ALL_REV_UNKNOWN_FLAGS`: revocation is checked for
   the end-entity certificate only, and an unreachable OCSP responder or CRL
   endpoint is soft-fail. Fix: say "end-entity revocation, best effort; an
   unreachable revocation endpoint does not fail the connection." Hard-fail
   revocation would need rustls's own webpki verifier with operator-supplied
   CRLs and is out of scope.
3. **Low. The ring hardening exception has no detector.** `.cargo/config.toml`
   documents `/FORCE:GUARDEHCONT` for ring's eight prebuilt NASM objects, which
   the linker reports as LNK2047 warnings. Nothing fails if a ninth object
   appears. Fix: a CI step that captures the release link output and asserts the
   LNK2047 object set equals the known ring list. RUSTSEC-2025-0007 (ring
   unmaintained) was withdrawn after the rustls team took over maintenance;
   record that decision next to the exception.
4. **Low. Eight `unsafe` blocks contain more than one unsafe operation.** The
   largest is in `src/etw/schema.rs` (`normalize_fields`, five union reads).
   Split them so each precondition sits next to its operation. Consider
   `clippy::multiple_unsafe_ops_per_block = "warn"`.
5. **Nit.** `Capture::stop` (`src/etw/session.rs`) builds the STOP output buffer
   from `SessionProperties` with default tuning values. The query path uses
   `QueryProperties`. Use `QueryProperties` for STOP too.
6. **Nit.** `EventIdFilterPayload.filter_in` is a Rust `bool` in a `#[repr(C)]`
   record handed to ETW. It is layout-correct and size-asserted. `u8` is the
   conventional type for a C `BOOLEAN` field.

Checked and found sound: every `unsafe` block has a `SAFETY` comment; native
buffers use aligned `u64` storage with size and bounds validation; token, SID,
TDH, and keyword parsing are bounds-checked; callbacks are non-unwinding
`extern "system"` and cannot panic on the data path; shutdown is a documented
fail-stop under a watchdog; logs carry no payloads or names; rustls `logging`
reaches `tracing` through the `tracing-log` bridge.

### Error handling

7. **Low.** `Capture::query_statistics` returns `Result<(), u32>`. A
   `Win32Status(u32)` newtype with `Display` and `Error` keeps the raw status
   and reads as an error at call sites. `target_change` returns
   `Option<String>`; a small enum with `Display` carries the old and new
   process IDs as data.
8. **Low.** Short error messages have mixed casing. Keep the long remediation
   messages as they are; standardize only the short ones to lowercase.

### Performance

9. **Medium, measure first.** The release profile has no `lto` and default
   `codegen-units`. With `build-std` already on, `lto = "fat"` and
   `codegen-units = 1` usually give single-digit to low-double-digit gains on
   tight loops and a smaller image. Release builds get slower, and
   `scripts/ci/verify-pe.ps1` must be re-run because LTO changes the final link.
10. **Low to medium, measure first.** Every lease clones an `Arc<PoolManager>`
    on the ETW thread and drops it on the parser thread (`src/etw/pool.rs`,
    `acquire` and `Drop for PooledBuffer`): two contended atomic
    read-modify-writes per event. The pool lives for the process, so a leaked
    `&'static PoolManager` removes both.
11. **Low.** Each `Slot` carries 384 bytes of extended-data item and offset
    tables that this provider does not use in practice. At the maximum capacity
    that is about 107 MB resident. Move the tables to a separate small pool, or
    into the slot's byte tail.
12. **Low.** `DeadlineTcp::write` (`src/network.rs`) calls `set_write_timeout`
    before every socket write. Cache the last applied timeout and skip the call
    when the remaining time is within a few milliseconds.
13. **Low.** No committed benchmark. Add a criterion benchmark for decode,
    encode, and `copy_record`, with a baseline recorded under
    `datasets/baselines/`.

### Style and maintainability

14. **Medium.** `src/pipeline.rs` is 2,550 lines, about 1,100 of them inline
    tests. Split into `pipeline/config.rs`, `pipeline/watchdog.rs`,
    `pipeline/supervisor.rs`, and `pipeline/parser.rs`; move the tests to
    `pipeline/tests.rs` like the other modules.
15. **Medium.** The `diag-timing` feature is dozens of `#[cfg]` attributes in
    `drain_queue_inner` and `parse_loop`, plus a wrapper that only hides an
    argument. A zero-sized `DiagTiming` with no-op methods when the feature is
    off removes nearly all of them.
16. **Low.** Name the magic numbers: the protobuf tag bytes `0x72`, `0x52`,
    `0x78` in `src/dnstap.rs`; the `0x36` property-flag mask in
    `src/etw/schema.rs`; the 5,000 ms enable timeout in `src/etw/session.rs`;
    the 15,000 ms wait hint in `src/service.rs`.
17. **Low.** Lint-driven cleanups: 26 functions can be `const fn`; 11 `status`
    or `schema` bindings shadow an unrelated earlier binding in FFI sequences;
    the `EventProducer` impl for `Capture` delegates to inherent methods of the
    same name where the trait alone would do.
18. **Low.** Two reconnect tests (`src/network/reconnect_tests.rs`) sleep 80 ms
    to wait for a thread. Wait on a condition with a timeout instead.
19. **Low.** Miri cannot run the FFI modules, but `decode`, `framestream`,
    `dnstap`, and `keywords::parse_keyword_array` are pure. A filtered
    `cargo miri test` job (advisory) is a cheap second check on slice
    arithmetic.

## Gate

Every phase ends with this gate, run from the repository root:

```powershell
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features diag-timing -- -D warnings
cargo clippy --locked --all-targets --features fuzzing -- -D warnings
cargo test --locked
cargo test --locked --features diag-timing
cargo build --locked --release
pwsh -NoProfile -File scripts/ci/verify-pe.ps1 -Path target/x86_64-pc-windows-msvc/release/etw-dnstap.exe
```

Then run the staged-diff host-path check from CLAUDE.md; it must print
nothing. Phases 2 and 3 also need a
lab run on the WS2025 guest, following the receiver rules in
`scripts/performance/README.md` (collector output to a null logger; count
delivery with `ss -tin` on the receiver).

## Phases

Each step names the files it owns. Steps in the same wave run in separate
worktrees and must not edit each other's files. `CHANGELOG.md`,
`docs/README.md`, and this plan are edited only at integration.

### Phase 0: baseline

| Step | Owns | Output |
| --- | --- | --- |
| 0.1 Record the release binary size, the `verify-pe` output, and the two ignored benchmarks (`synthetic_pooled_intake_benchmark`, `parse_encode_benchmark`) at the plan's base commit | `datasets/baselines/code-review-2026-11-01/` (new) | A baseline record with the commit hash and the host CPU model, no host paths |

### Phase 1: low-risk fixes (wave 1, parallel)

| Step | Items | Owns |
| --- | --- | --- |
| 1.1 Startup wait hint and checkpointed pool allocation, plus a lab test that starts the service at the maximum capacity and confirms SCM reports Running | 1, and the wait-hint constant from 16 | `src/service.rs`, `src/pipeline.rs` (startup budget, `run_pipeline`, `on_running`), `src/etw/pool.rs` (progress callback), `docs/guide/troubleshooting.md` if the behavior is documented there |
| 1.2 Revocation wording | 2 | `README.md`, `SECURITY.md`, `docs/guide/configuration.md`, the `--receiver` doc comment in `src/pipeline.rs` (`CaptureArgs`) |
| 1.3 LNK2047 assertion in CI and the ring decision note | 3 | `.github/workflows/ci.yml`, a new script under `scripts/ci/`, the comment block in `.cargo/config.toml`, `CONTRIBUTING.md` |
| 1.4 Code cleanups | 4, 5, 6, 7, 8, 16 (except the wait hint), 17 | `src/dnstap.rs`, `src/etw/schema.rs`, `src/etw/session.rs`, `src/etw/keywords.rs`, `src/etw/intake.rs`, `src/etw/decode.rs`, `src/network.rs` (shadowed `stream` only), `src/perfcounters.rs`, `src/framestream.rs`, `src/lib.rs`, and in `src/pipeline.rs` only the `EventProducer for Capture` impl and the `target_change` call site |
| 1.5 Criterion benchmark and the Phase 0 record | 13, 0.1 | `benches/` (new), the `[dev-dependencies]` and `[[bench]]` sections of `Cargo.toml`, `Cargo.lock`, `datasets/baselines/code-review-2026-11-01/` (new) |
| 1.6 Condition-based waits in the reconnect tests and an advisory Miri job | 18, 19 | `src/network/reconnect_tests.rs`, `src/network/test_support.rs`, the new job in `.github/workflows/ci.yml` (appended; step 1.3 adds a step to the existing `rust` job) |

Integration: merge each step's branch into `plan/code-review-remediation`,
run the gate, add the CHANGELOG lines, and update the status block above.

### Phase 2: performance experiments (sequential, one change per commit)

Order: item 9 (configuration only), then 10, then 11, then 12. Each change is
measured with the Phase 1.5 benchmark and a lab run against the Phase 0
record. A change that shows no gain is reverted and the result is recorded in
the plan. `verify-pe` is mandatory after item 9.

### Phase 3: structure (sequential, no behavior change)

Items 14 and 15. Verified by the unchanged test count and the `diag-timing`
build. Runs after Phase 2 so that the split does not conflict with the
performance commits.

### Phase 4: lint tightening

After Phase 1 has cleared them, add `clippy::multiple_unsafe_ops_per_block`,
`clippy::missing_const_for_fn`, and `clippy::shadow_unrelated` to
`[lints.clippy]` in `Cargo.toml` as warnings, and run the gate.

## Execution record

Filled in at integration. Each row names the step, the merge commit, the gate
result, and any deviation from the plan.

| Step | Commit | Gate | Notes |
| --- | --- | --- | --- |
| 1.3 LNK2047 detector | `1fabf32` | script 6/6 on a fresh release link; full gate pass | build-engineer review: ship after four comment-level edits (update procedure, failure-message pointer, unwind wording, object basename normalization), applied and re-verified under both PowerShell editions. CONTRIBUTING conflict with 1.5 resolved by keeping both bullet sets. |
| 1.6 Test waits and Miri | `a442727` | full gate pass (268 tests); Miri 56 tests in 555 s locally | code-reviewer: no-ship until three fixes (a scenario sleep restored, a reconnect-test race, a positive test-count floor) plus a 60-minute timeout, runner-temp handling, and a 20 s wait bound; all applied. The job is advisory until it has run on a hosted runner. |
| Phase 1 integration | `a442727` | fmt; clippy default, diag-timing, fuzzing, bench; tests 268, 273, 272; cargo-deny; fresh release link with the link-exception script; verify-pe 21/21; linkcheck; Python tests | Complete on 2026-11-02. Every step was implemented and separately reviewed by a specialist agent before merge. |
| 1.5 Benchmark and Phase 0 | `da06e4e`, fix-up `1236fe7` | full gate pass incl. `--features bench` (272 tests) and cargo-deny | performance-engineer review: conditional ship; required fixes applied (host-path line, unsupported 139 ns explanation withdrawn, A/B protocol and drift bound, cold-pool group) plus two-thread and decomposition groups, CI steps for the feature, IPv6 and event 258 records. Baseline re-recorded as the median of three quiet sessions. Key numbers: `parse_and_encode` 153 ns (IPv4 query), two-thread 231 ns per event, so crossing cores adds about 80 ns. The merge needed one fix-up: the bench pool now uses `with_progress`. |
| 1.2 Revocation wording | `b7c0614` | fmt, clippy, linkcheck pass | security-auditor review: no-ship until five wording fixes; applied by technical-writer, committed at integration. Also corrected the install guide, `src/network.rs` doc comment, `Cargo.toml` comment, and the existing CHANGELOG entry. |
| 1.1 SCM start progress | `dbaf472` | full gate pass, 267 tests | surgeon-reviewer: Rust change sound; lab script restore-flag ordering fixed and re-run on the WS2025 guest (RUNNING at 417 ms, configuration restored). Pre-existing `Invoke-LabGuest` argument bug fixed (not in the plan). `PoolManager::new` is now test-only; `with_progress` is the production constructor. |
| 1.4 Code cleanups | `ee40d02` | full gate pass, 268 tests; production hits for the three Phase 4 lints: 0 | surgeon-reviewer: ship. `EventProducer` moved to `src/etw/session.rs` (one conflict with 1.1 near `run_capture`, resolved by keeping `Lifecycle` and dropping the old trait block). Item 8 touched four messages. |
| 9 Fat LTO and one codegen unit | `e396871`, merged at `25742af` | full pass, 276 tests | surgeon-reviewer: ship (the review covered items 9 to 12 together; one finding, applied in `29f821d`: the pool owns the item-cell reservation invariant, see item 11). Host: image 2,396,672 to 2,211,840 bytes (-7.7%), cold release build +52%, parse and encode about 10% faster. Lab: not separable; all four items together in the "Phase 2 lab comparison" row. |
| 10 Leaked `&'static PoolManager` | `8d1b172`, `2ec1796`, merged at `25742af` | full pass, 276 tests | surgeon-reviewer: ship. Host: `copy_record` -28% (query), lease recycle -41%; `two_thread` +0.3%, no change on the host. Lab: the contended-atomic saving cannot be separated from the other items; the two threads together use 11.5% fewer cycles per event after all four items. |
| 11 Item tables at the end of the slot bytes | `eecc74e`, merged at `25742af`; reviewer finding in `29f821d` | full pass, 276 tests | surgeon-reviewer: ship, one finding applied in `29f821d` (the pool owns the item-cell reservation invariant). Host: pool private bytes 177 to 150 MB at `--capacity 65536` (-15%), cold copy -37%, `two_thread` -6%. Lab: service private bytes at defaults 72.9 to 63.4 MiB (-13.0%, ranges 0.1%). |
| 12 Cache the socket timeouts | `50e9298`, `socket_write` group in `05941c1`, merged at `25742af` | full pass, 276 tests | surgeon-reviewer: ship. Host: about 0.7 ns per event (0.5% of parse and encode), the smallest of the four; kept because the group built to see it does. Lab: no separate effect visible; `slow_writes=0` and the write path is inside the 11.5% above. |
| Phase 2 lab comparison | `25742af` against `e61179d` | 3 runs per image at 10,000 QPS for 120 s, ABABAB, 0 drops, guest restored | Thread cycles per event 3,085 to 2,729 (-11.5%; `dnstap-parser` -12.7%, `etw-consumer` -10.7%; ranges 2.8% and 0.9%, no overlap); private bytes -13.0%; delivery equal (2.4 million events per run, zero drops). Service CPU seconds at tick resolution are inside noise (22% range). Peak queue depth higher in all three pairs (5,701 to 6,367), no drops. Record: `datasets/baselines/code-review-2026-11-01/` (section "Phase 2 lab comparison"). |
| Phase 3 pipeline split and `DiagTiming` | merge `6d9f474` | full gate pass; 276, 281 and 280 tests; release image 2,210,816 bytes, unchanged | surgeon-reviewer: ship, no findings. The split is a pure move, verified by line set and contiguity. `DiagTiming`, `Stamp` and `DrainTiming` live in `src/pipeline/parser/timing_{on,off}.rs`, and the `drain_queue` wrapper is removed. |
| Phase 4 lint tightening | `909b0b8` | fmt; clippy `-D warnings` for default, `diag-timing`, `fuzzing` and `bench`; tests 276, 281 and 280 with 7 ignored each; release build; image 2,210,816 bytes, unchanged; the separate `fuzz/` crate passes its own Clippy run | Hits before the change, as `multiple_unsafe_ops_per_block` / `missing_const_for_fn` / `shadow_unrelated`: default 4 / 1 / 67, `diag-timing` 4 / 2 / 68, `fuzzing` 4 / 1 / 67, `bench` 5 / 2 / 68. After: 0 / 0 / 0 in every set. Production hits were the const functions `pack_item`, the `diag-timing` event counter and the bench `event_id`, plus one bench unsafe block (`private_bytes`). Test code: four unsafe blocks split and about 24 shadow sites renamed. Five test modules (`keywords_tests`, `schema_tests`, `session_tests`, `handshake_tests`, `cli_tests`) carry one `#![expect(clippy::shadow_unrelated)]` each, covering 43 sites where the same name is rebound across many sequential cases of one test and a rename would only number the cases. The lint lines and every fix land in one commit so that each commit on the branch passes the gate. `fuzz/` has no `[lints]` table and does not inherit these lints. |
