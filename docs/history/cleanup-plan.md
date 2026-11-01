# Obsolete and legacy code cleanup plan

> **Status:** Historical (executed Nov 1, 2026; results in [cleanup-validation.md](cleanup-validation.md)) · **Evidence dated:** 2026-11-01 · **Commit:** written at `efb47af`; executed through the commit that records the validation · **Reviewed:** 2026-11-01

## 1. Summary

Five read-only discovery agents examined the repository: Rust, Python scripts,
PowerShell scripts, docs and datasets, and build and CI configuration. Nothing was
changed.

Overall the codebase has little dead code:

- With `warnings = deny`, the compiler and clippy report zero warnings for all four feature sets.
- No source file has a `dead_code` suppression, an unused dependency or a TODO.
- No code is left from the mutex or the process handle.

Most of the cleanup is in three areas:

1. **Legacy names and duplicated test scaffolding in Rust.** Names from the ring
   buffer remain, there are three prost or reference oracles, and handshake and
   template helpers are copied between test files.
2. **Scripts from finished campaigns.** These include 5 archive scripts that
   nothing references, single-campaign Python tools, and unused LabCommon exports.
   One latent bug was found: an undefined `start_marker` in the parser-ramp
   controller.
3. **Docs whose status no longer matches reality.** One executed plan is still
   in `design/`, three plans have incorrect status lines, an index is out of date,
   and the CHANGELOG commit log stops at `9172f2d`.

Raw reports were kept in the session scratch folder, outside the repository
(the agents returned them as hand-backs; finding IDs below are theirs: R = Rust,
P = Python, S = PowerShell/shell, D = docs/datasets, B = build/CI).

## 2. Rules for the cleanup

1. Do not remove a security control or a hardening flag, even if it is
   redundant. B-15 to B-25, the identity gate, the PID filter, the reclaim GUID
   check, the bounds constants and the layout assertions all stay.
2. History is a record. Move files with `git mv`. Do not merge or delete history
   docs (D-17). Code and a generator link to some of them by path.
3. If a doc tells a reader to run a script, keep the script runnable. If you move
   it, update the doc in the same commit.
4. Make one commit per wave, or per item group, so each one is easy to revert. No SemVer
   bump: all changes are internal. Record user-visible doc moves under
   CHANGELOG `[Unreleased]`.
5. Run the gate in section 6 after every wave.

## 3. Team and skills

| Wave | Agent | Skills |
|---|---|---|
| 0 Baseline | orchestrator | rust-check, build-rust |
| 1 Comments and status lines | voltagent-dev-exp:documentation-engineer | asd-ste100 |
| 2 Rust | voltagent-lang:rust-engineer (and voltagent-dev-exp:refactoring-specialist for R-9/R-12/R-13) | rust-skills, rust-check, rust-security-hardening |
| 3 Build/CI | voltagent-dev-exp:build-engineer | build-rust, rust-windows-hardening |
| 4a Python | voltagent-lang:python-pro | — |
| 4b PowerShell | voltagent-lang:powershell-7-expert (lab check: orchestrator) | hyperv-windows-vm |
| 5 Doc moves | voltagent-dev-exp:documentation-engineer | asd-ste100 |
| 6 Decision-gated items | as for the area | as above, semantic-versioning for R-18 |
| 7 Review | voltagent-qa-sec:code-reviewer, voltagent-qa-sec:security-auditor | rust-security-hardening, windows-service-security |

Waves 2, 3 and 4 touch disjoint files and can run in parallel. Waves 1 and 5 both
edit docs, so run them in order.

## 4. Decisions needed before execution

| ID | Question | Default if not answered |
|---|---|---|
| C-1 | Commit directly on main, as in the previous plan? | Yes, on main, no push |
| C-2 | Archive scripts that docs still cite (S-2, S-3, S-4, S-6, S-9): keep, or delete and replace the doc paths with "see commit `6e62b2e`"? | Keep (wave 6 not done) |
| C-3 | JSONL-metrics and ring-fix analyzers (P-6, P-7): move to `archive/` with their tests? | Move |
| C-4 | Remove the `diag-timing` feature (R-18)? | Keep |
| C-5 | The 1 GiB guard that cannot fail (R-2): keep and prove it with a const assert, or delete? | Keep, add const assert |
| C-6 | Make the DNS PID a required argument of `CallbackContext` (R-17)? This is hardening, not cleanup. | Do it as a separate commit in wave 6 |
| C-7 | Remove profile and lint keys that repeat Cargo defaults (B-4 to B-8)? | Remove B-1, B-5, B-6, B-8. Keep B-4 and B-7 because they show the release intent |
| C-8 | live-load plan (D-4): decide the pending L4 action, and redact the receiver IP and VM GUIDs? | Mark L4 "not done", redact, move to history |
| C-9 | CHANGELOG commit log (D-11): regenerate up to HEAD, or replace with a pointer to `git log`? | Regenerate |

## 5. Waves

### Wave 0: baseline (no change)
- Record the results of: `cargo test --locked` (and with `diag-timing`), clippy for
  all feature sets, `cargo deny` on all three lockfiles,
  `python -I scripts/ci/linkcheck.py .`, the two Python test files, and
  `verify-pe.ps1` on a release build.
- Record the SHA-256 of the release exe. This gives the before-state for the final
  diff.

### Wave 1: comments and status lines (no behaviour change)
- **Rust comments:**
  - R-1: repair the garbled `maximum_buffers` comment at `src/etw/session.rs:63`.
  - R-15: fix the module map at `src/etw.rs:1-10`.
  - R-16: fix the `PipelineConfig` doc at `src/pipeline.rs:268`.
- **Build comments:**
  - B-9: in `deny.toml`, say that `syn` is the only duplicate.
  - B-28: add a comment explaining why `Win32_System_Time` is needed.
- **Doc status lines:**
  - D-7: give `docs/history/skills-evaluation-plan.md` the standard block.
  - D-8, D-9: update the status lines in configuration.md and troubleshooting.md.
  - D-10: re-review README.md, docs/README.md, install.md and uninstall.md against
    HEAD and update their status commit.
  - D-5: correct the status of parallel-parser-workers-plan.md.
  - D-12: rerun the tests and update the CONTRIBUTING counts.
- D-11: regenerate the CHANGELOG commit log (see C-9).

### Wave 2: Rust legacy names, visibility and duplicated scaffolding
In order. Run `cargo test` after each step.
1. R-3: rename `drain_ring`/`drain_ring_inner` to `drain_queue*`, including the
   test name and the bail message. Optional R-4: collapse the cfg wrapper.
2. R-5, R-6: tighten visibility. Make `ProducerStart`, `EventProducer`,
   `run_pipeline`, `EventLogLine` and `MAX_JUMBO_CAPACITY` private.
3. R-7: use `MAX_JUMBO_CAPACITY` for the clap `--jumbo-capacity` range. Add the
   same bound to `PipelineConfig::validate`.
4. R-14: add a single `same_guid` helper and use it in `dns_provider` and
   `reclaimable`. The semantics must stay the same.
5. R-8 (narrow form only): make `Encoder::encode_with` private to `dnstap.rs`.
6. R-10, R-11: add one network test handshake helper and one retry-policy helper.
   Move the inline `network.rs` tests into the `*_tests.rs` files and delete exact
   duplicates.
7. R-12: add `etw/test_support.rs` for `offset_u32` and the UTF-16 helpers.
   Extract the production parse-then-encode step so that `ParseEncode` calls it.
8. R-13: add one `saved_template()` for fuzzing and the decoder tests, under
   `cfg(any(test, feature = "fuzzing"))`.
9. R-9: merge the two prost oracles into `dnstap/test_oracle.rs`. Keep the
   hand-encoded `golden()` oracle. Delete the redundant reference encoder,
   varint, address and warm-up duplicates. Move the `#[cfg(test)]` fields out of
   the production structs.
   - Gate: no boundary case may be lost. Keep packet length 12 and 65535,
     varint 127/128, and IPv6. The test count may drop only by the number of
     exact duplicates deleted, and that number goes in the commit message.

Not in this wave: the full R-8 `&[u8]` API change, which needs a design doc
update; R-2 and R-17 (wave 6).

### Wave 3: build and CI
- B-1, B-5, B-6, B-8: remove keys that repeat defaults (see C-7).
- B-11: pin the `cargo-deny` and `cargo-llvm-cov` versions the same way as
  `cargo-fuzz`, and give the pin source in CONTRIBUTING.
- B-12: add `cargo fmt --check` for `fuzz/`.
- B-13: list `target/`, `fuzz/target/` and `tools/dns-stress/target/` in
  `.claudeignore`.
- B-14: add `lcov.info` and `coverage-summary.txt` to `.gitignore`.
- Keep B-2 and B-3. `Win32_Security` and `Win32_Foundation` are used directly,
  and the Rust agent advises an explicit declaration. Keep B-10, B-15 to B-27,
  B-29 to B-33.
- Gate: release build plus `verify-pe.ps1`, which must pass all 21 checks. Also
  compare the exe hash with wave 0: B-1, B-5 and B-8 must not change a release
  byte. With trim-paths, an identical hash is expected. If the hash differs,
  explain why before you continue.

### Wave 4a: Python scripts
- **P-1 (bug):** in `ubuntu-parser-ramp-control.py`, `monitor()` reads
  `start_marker` (line 465), but only `finish()` defines it (line 346). Read
  `start.json` inside `monitor()`. P-2: drop `import socket` in the same edit.
- P-3: remove unused imports.
  - Do not edit `plot-pooled-wpr.py`, `export-pooled-graph-data.py` or
    `pooled_intake_run.py`. They hash their own source into provenance records.
- P-4: remove unused locals and parameters.
- P-11: delete `lab_config.receiver_endpoint()`.
- P-12: delete the `lab.json` key `windows_vm.install_dir`.
- **Move to `scripts/performance/archive/`:**
  - P-5: the service-stop config and pcap tools. Fix their `sys.path` to
    `parents[1]`, and remove PyYAML from `requirements.txt`.
  - P-8: `count-parser-ramp-diagnostic.py`.
  - P-6, P-7: the ring-fix and JSONL-metrics analyzers, with their tests (C-3).
    Update `test_metrics_ledger.py` so CI still runs the remaining tests.
- P-19: list the scripts by name in the README. Name P-9 explicitly as well.
- Keep P-10, P-14, P-15, P-16 as they are. Shared-helper refactors across guest
  and host scripts are not worth the risk.
- Gate: run `py_compile` on all files, pyflakes must be clean, and both CI test
  files must pass.

### Wave 4b: PowerShell scripts
- S-1: delete the 5 archive scripts that nothing references, and their README row.
- S-7: delete the private `ConvertTo-LabCommandLine`.
- S-8: delete `Test-LabAnswer`. Remove `Get-LabGuestPrelude`, `Get-LabCredential`,
  `Assert-LabId` and `Add-LabOwnedZone` from `Export-ModuleMember`.
- S-5: move `windows-control.ps1` to `archive/`. Fix its LabCommon import path,
  the README and two history command examples.
- S-10: add comments that explain the `.Handle` reads. Leave everything else.
- Keep S-11: `Grant-EtwDnstapEtwAccess.ps1` and `verify-pe.ps1` are not changed.
- Gate: parse check, then `Import-Module -Force` and `Get-Command -Module LabCommon`.
  Then run a read-only lab check on DNSTap-WS2025: `windows-parser-campaign.ps1
  -Action Inventory` and `archive/windows-control.ps1 -Action Status`. Take a
  checkpoint first.

### Wave 5: doc moves and index
- D-3: `git mv` `design/diagrams/packet-flow.drawio` to `history/diagrams/`.
- D-1: `git mv` `design/single-pass-decoder-plan.md` to `history/`. Write a short
  current-contract page, `design/single-pass-decoder.md`, and fix the 7 link sites.
- D-2: move the ArrayQueue experiment section of `ring-buffer.md` to
  `history/ring-capacity-experiment.md`, and label or remove the unsourced P3z
  paragraph. The rename to `channel-capacity.md` is optional. If you rename it,
  update the 4 link sites.
- D-4: apply C-8, then move the live-load plan to `history/`.
- D-6: in the filtered-ramp plan, merge the two status blocks and fix the identity
  sentence, so it says LocalService with a restricted SID. Change the guide link
  at `service-stop-remediation.md:61`.
- D-16: add reverse links between plans and their results.
- D-13, D-14: link the baseline `sha256.txt` and the PE text files from their
  history docs.
- D-18: state in CONTRIBUTING the exception that keeps
  `pooled-vm-performance-review.md` Current.
- D-19: refresh the `docs/README.md` index last.
- D-15 is local only, not committed:
  - Hash the ignored exe files in `datasets/` and compare them with the receipts.
  - Then move the logs and candidate exe files to `target/`. Keep the baseline exe.
- Gate: linkcheck must report 0 broken links. A grep for each moved file name may
  find only updated links.

### Wave 6: decision-gated (only on a "yes")
- C-2: delete the remaining archive scripts and `pooled-vm.wprp`, and retire
  `audit-pooled-management.py`. Then remove `Invoke-LabNative` and
  `Get-LabServiceCommandLine`.
- C-4: remove diag-timing. This removes `etw/diag.rs`, about 50 cfg sites, the two
  CLI flags, the CI steps and the doc sections. Do it as one commit, then run the
  full gate.
- C-5: add the const assert for the 1 GiB guard (R-2).
- C-6: make the PID required (R-17). Elevated lab tests are needed: run
  `cargo test-release -- --ignored` on the guest.
- Optional S-12: read the zone names in `New-DnsWorkload.ps1` from `lab.json`.

### Wave 7: review and lab smoke
- code-reviewer and security-auditor review the full diff from `efb47af`. The
  security-auditor confirms that no control in the section 2 rule 1 list has
  changed.
- Deploy the release build to DNSTap-WS2025 and run the 6 elevated tests. Check
  that the service starts and that dnstap frames arrive at DNSTap-Ubuntu2604.
- Record the result in `docs/history/cleanup-validation.md`, and move this plan
  to `docs/history/`.

## 6. Gate after each wave

- `cargo fmt --check`. Run it for the root, `fuzz/` and `tools/dns-stress/`.
- `cargo clippy --locked --all-targets -- -D warnings`. Run it with default
  features, `diag-timing`, `fuzzing` and `--all-features`.
- `cargo test --locked`, also with `--features diag-timing`. The counts must match
  wave 0, minus any documented duplicates that were deleted.
- `cargo deny --locked check`. Run it for all three lockfiles.
- Python CI tests and `python -I scripts/ci/linkcheck.py .`.
- After waves 2, 3 and 6: release build and `verify-pe.ps1`.

## 7. Checked and kept (no action)

- All `allow(unsafe_code)` attributes. They match `unsafe` blocks in their modules.
- The TDH fallback path, which is still used for unsupported schemas.
- The `fuzzing` feature and `fuzz/`.
- The asan profile and its aliases.
- The `EventProducer` test seam.
- `PipelineConfig::validate`, which is defence in depth.
- All windows-sys features except those noted above. Each one is needed to compile.
- `trim-paths` and `profile-rustflags`. Both are load-bearing.
- `allow-wildcard-paths`.
- The `syn` 2 and 3 duplicate, which needs upstream changes to remove.
- The active campaign, workload, trace-export and Ubuntu guest scripts.
- `lab.json`.
- All of `docs/assets`.
- The datasets receipts and `workload-provenance.json`.
- The history plan and result pairs, which stay separate.
- The CHANGELOG `[Unreleased]` body.
- SECURITY.md.
- The bodies of install.md and uninstall.md.
- `__pycache__`, which is untracked and ignored. Nothing to commit.
