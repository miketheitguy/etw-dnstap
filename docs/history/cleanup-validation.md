# Obsolete and legacy code cleanup validation

> **Status:** Historical · **Evidence dated:** 2026-11-01 (guest local time, America/Los_Angeles; Application log lines are UTC) · **Commit:** `0865e19` for the deployed build; all checks repeated at the final cleanup commit · **Reviewed:** 2026-11-01

This record covers the execution of the
[cleanup plan](cleanup-plan.md). It lists the gate results for each wave, the
lab run, the two reviews, and the places where the execution differs from the
plan.

## Baseline (wave 0, at `efb47af`)

| Check | Result |
| --- | --- |
| `cargo fmt --check` | clean |
| clippy `-D warnings`: default, `diag-timing`, `fuzzing`, `--all-features` | clean |
| `cargo test --locked` | 217 passed, 0 failed, 6 ignored |
| `cargo test --locked --features diag-timing` | 222 passed, 0 failed, 6 ignored |
| `cargo deny --locked check`: root, `fuzz/`, `tools/dns-stress/` | all ok |
| linkcheck | 62 files, 0 broken |
| Python CI tests | both files OK |
| `verify-pe.ps1` on the release exe | 21 checks, 0 failed |

## Gates after each wave

| Wave | Commit | Result |
| --- | --- | --- |
| 3 Build and CI | `2fcc4a2` | Gate passed. The release exe differs from the baseline in 6 bytes: the PE header TimeDateStamp and two debug-directory timestamps. The code is identical. `verify-pe.ps1`: 21 checks, 0 failed. |
| 4b PowerShell | `63dd477` | Parser check: 0 errors in PowerShell 7 and 5.1. `LabCommon` loads in both and exports 14 functions. |
| 4a Python | `015d385` | `py_compile` passed on every script. pyflakes is clean except `hashlib` in the three self-hashing scripts, which were left byte-identical on purpose. `test_metrics_ledger.py` (7 tests), `test_pooled_intake_run.py` (5) and `archive/test_archived_analyzers.py` (9) pass. |
| 2 Rust | `0865e19` | fmt and clippy are clean for all four feature sets. `cargo test` gives 217/0/6, and 222/0/6 with `diag-timing`. `cargo check` of `fuzz/` passes. `verify-pe.ps1`: 21 checks, 0 failed. |
| 1 and 5 Docs | `40a6b65` | linkcheck: 65 files, 0 broken. |
| Review fixes and records | final commit | linkcheck: 0 broken. Archived analyzer tests pass, and CI now runs them. |

Test count changes in wave 2: one test was deleted and one added.

- **Deleted:** `warmed_encoder_reuses_storage_without_heap_operations`. It was a
  strict subset of
  `warmed_direct_encoding_reuses_all_storage_without_heap_operations`.
- **Replaced:** `with_expected_pid_sets_the_filter` is now
  `new_stores_the_required_expected_pid`.
- **Added:** a CLI test for the jumbo capacity bound.
- **Renamed:** `startup_failure_completion_finishes_an_empty_ring` is now
  `startup_failure_completion_finishes_an_empty_queue`.

## Lab run (DNSTap-WS2025, build 26100)

Checkpoint `cleanup-pre-2026-11-01` was taken first. The three earlier
checkpoints are kept.

| # | Check | Observed |
| --- | --- | --- |
| L1 | Elevated ignored tests, unit-test exe SHA-256 `B649934595BCB6F95967C41F889A5A9488F87F89B1A23ADE9011A604A64A61CD` | `unit.exe --ignored --test-threads=1`: 6 passed, 0 failed |
| L2 | Deploy the release exe from `0865e19` (SHA-256 `BCDA9D3CD9F7AD2B645BC67EAEE5A4B68E4B02C1ADDB34ED039221199D976907`; previous `8FAD752D...`, kept on the guest as `C:\Lab\etw-dnstap-previous.exe`) | Hash verified after the copy. The ACL is inherited: `NT SERVICE\EtwDnstap:(I)(RX)`. |
| L3 | Service identity after start | `NT AUTHORITY\LocalService`, `RESTRICTED`, `SeChangeNotifyPrivilege` only, start mode Auto (delayed) |
| L4 | Capture with the required DNS PID | 100 local queries. The next stats line shows `received=400 encoded=400 ring_dropped=0 foreign_pid_rejected=0 callback_errors=0`. Output shows `receiver_connected=true output_errors=0`. |
| L5 | Receiver | The DNS-collector journal on DNSTap-Ubuntu2604 shows the queries at 15:46:29 UTC. |
| L6 | Lab scripts with the changed `LabCommon` (Windows PowerShell 5.1) | The module loads and opens a PowerShell Direct session. `windows-parser-campaign.ps1 -Action Inventory` stops at the guard `VM resources changed`. `archive/windows-control.ps1 -Action Status` fails with a null `LiteralPath`. Both failures predate the cleanup; see "Problems found". |

## Reviews

- **Code review** of `efb47af..40a6b65` found no unintended behavior change in
  `src/` and no lost Rust test coverage. It found four small problems. All four
  are fixed in the final commit:
  - **A wrong fact in the new decoder contract.** `Port` is read big-endian only
    when its output type is `PORT`. The contract also stated the cache rule and
    the epoch bound too strictly.
  - **Archived analyzer tests no longer ran in CI.** CI runs them again.
  - **A broken sentence** in `service-stop-remediation.md`.
  - **Dead code.** `Assert-LabId` (and its pattern variable) is deleted.
- **Security review** confirmed the following:
  - Every control in plan section 2, rule 1, is unchanged.
  - The required DNS PID is complete. The only production caller passes the PID
    of the DNS Server service, and that PID cannot be 0.
  - No test helper or fuzzing helper is in a default-feature build.
  - `git diff efb47af..HEAD` is empty for `scripts/provision`, `scripts/ci`,
    `.cargo/config.toml`, `src/service.rs` and `src/framestream*`.
  - The only change in `deny.toml` is a comment.

## Differences from the plan

- **B-1 kept.** The explicit `tracing-subscriber` `fmt` feature stays. The
  dependency is named, and does not rely on the crate defaults.
- **R-4 skipped.** Collapsing the `drain_queue` cfg wrapper is not simpler in
  both feature configurations.
- **`EventLogLine` stays `pub(crate)`.** `EventLogWriter` exposes it as its
  `MakeWriter` writer type.
- **One Rust commit.** The R-17 PID change shares `src/etw/session.rs` with
  cleanup items, so it is in the same commit as the cleanup (`0865e19`). That
  commit message names it separately.
- **D-15 not moved.** The ignored dataset files stay in `datasets/`. All three
  exe hashes match their receipts. In `target/`, `cargo clean` would delete them.
- **P-1 kept a guard.** `finish()` still requires `start.json`.

## Problems found (not fixed)

- **vCPU mismatch.** `scripts/performance/lab.json` says 2 vCPUs for
  DNSTap-WS2025, but the VM has 6. The `-CheckResources` guard in `Get-LabVm`
  therefore stops campaign scripts. The owner decides which value is correct.
- **`archive/windows-control.ps1 -Action Status` fails at `efb47af` too.** The
  historical harness passes arguments that its guest script block does not
  expect.
- **Lab addresses and the VM ID remain elsewhere.** Only the live-load plan was
  redacted. The same values are in other docs, in `lab.json`, in
  `tools/dns-stress/README.md` and in one test, and they remain in git history.
- **Low, already present before the cleanup:** `network.rs` writes the control
  payload bound `4..=512` as a literal next to `MAX_CONTROL_LENGTH`. A shared
  constant or a const assertion would keep the two equal.
