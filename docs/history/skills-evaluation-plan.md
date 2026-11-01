# etw-dnstap evaluation plan (skills-driven)

Status: Executed (Nov 1, 2026). Written at `e8560ac`. Execution ran through `5f610f5`. Every line number below was read from the file at plan time.

> **Correction:** `asd-ste100` is installed. `~/.claude/skills/asd-ste100` is a symlink to `~/.agents/skills/asd-ste100`, which the planning agent's file search did not follow. It applies to documentation and error-message wording. D-3 is resolved.

## 1. Task summary

Goal: evaluate `C:\Users\kn6py\Git\etw-dnstap` (a Rust Windows service that consumes Microsoft-Windows-DNSServer ETW events and emits dnstap Frame Streams) and its build/CI/provisioning process against the rules in the newly installed Claude Code skills, then report findings. Whether to fix anything is an open human decision (section 7).

Deliverables: relevance matrix, findings list (observed vs to verify), phased workflow with agents, risks/decision points.

Inventory facts (counted):
- 15 skills exist under `C:\Users\kn6py\.claude\skills\<name>\` (`asd-ste100` is a symlink; see the correction above).
- `skills\synced\` holds a bucket of general-purpose skills (docs, pdf, docx, deep-research, google-workspace, computer-use, skill-creator, browser, morning, import-memory), not part of the requested list. Only `deep-research`/`docs` could conceivably help, and neither is needed.
- Repo source: 37 `.rs` files under `src/` (about 17 contain `unsafe`/FFI tokens), one binary crate with a lib (`src/lib.rs`, `src/main.rs`), separate `fuzz/` workspace (3 targets) and `tools/dns-stress/` crate. **No `.c/.cpp/.h/.idl/.acf/.wxs/.lua` file and no `build.rs`** (Glob of repo root, `docs/`, `tools/`, `scripts/` found none; the repo `target/` tree contains Windows ISO media and a vendored DNS-collector Go tree, which are build artifacts, not project source, and `.gitignore:1` ignores `/target/`).
- No installer: service install is manual PowerShell (`docs/guide/install.md:192-235`, copy at `:205`). No tags (`CHANGELOG.md:8`). Workflows: only `.github/workflows/ci.yml` and `lab.yml`.

## 2. Relevance matrix

| Skill | Applies? | Evidence |
|---|---|---|
| rust-security-hardening | **Yes, primary** | Windows MSVC Rust service with FFI to ETW/SCM/token APIs: `src/service.rs`, `src/etw/*.rs`; hardened `.cargo/config.toml` and `Cargo.toml:31-55`. Repo already matches this skill's `assets/cargo-config.toml` (same flags) and adds `+crt-static` (`config.toml:14`). |
| rust-windows-hardening | **Yes (checklist)** | Older/shorter subset of the above. All its items are present: overflow-checks (`Cargo.toml:33,39`), `panic="abort"` (`:40`), CFG (`config.toml:15`), NXCOMPAT/DYNAMICBASE/HIGHENTROPYVA/GUARD:CF/CETCOMPAT (`config.toml:21-26`). Use only to confirm, not to change. |
| windows-service-security | **Yes, primary** | SCM service, LocalService + restricted service SID, privilege allowlist, token checks (`service.rs:565-620, 697-760`), ETW ACL script (`scripts/provision/Grant-EtwDnstapEtwAccess.ps1`), install ACLs (`install.md:189-222`). Its reference profile is BFD-specific (two services, ncalrpc, COM/MI); only the principles transfer. |
| windows-rpc-authentication | **N/A** | No RPC, ncalrpc, Authz or MIDL anywhere in `src/` (Cargo.toml has no rpc crate; windows-sys features at `Cargo.toml:19` omit `Win32_System_Rpc`). Single-service design with no broker. Do not apply. |
| c-security-hardening | **N/A** | No C source. State explicitly in the report. |
| cpp-ffi-safety | **N/A** | No C++ compiled; skill itself says not to infer C++ from installed Build Tools. |
| windows-msi-release-recovery | **N/A today** | No MSI/WiX/release workflow (only `ci.yml`, `lab.yml`). Becomes relevant only if an installer/release pipeline is added; its "validate on a disposable runner before tagging" guidance can be cited in a recommendation. |
| semantic-versioning | **Yes, light** | `Cargo.toml:3` is `0.1.0`; `CHANGELOG.md:5,8` says SemVer "once it is tagged", no tags; `CHANGELOG.md:119-124` records removed CLI options. Needs a 0.y.z policy statement and a baseline decision. |
| rust-check | **Partly** | Useful part: run `cargo fmt` + `cargo clippy` (already enforced, `ci.yml:56-66`). The rest is a template for another project ("Constitution II", async/await for I/O, `assert_cmd`, `crates/` dir, `mdnotes_lib`). The repo is thread-based with no tokio (`Cargo.toml:10-19`). Apply only fmt/clippy and `?`/iterator idioms. |
| rust-skills | **Yes, secondary** | 265 rules; relevant categories: `own-`, `err-`, `unsafe-`, `num-`, `type-`, `conc-atomic-ordering`, `test-`, `lint-`, `proj-msrv-declare`. `async-*` rules N/A (no async runtime). Its suggested `lto="fat"`, `codegen-units=1`, `strip=true` is a perf profile and is not in `Cargo.toml:35-40` (decision, see F-9). |
| build-rust | **Partly** | `references/cli.md` (clap, `Cargo.toml:12`), `ffi-unsafe.md`, `tooling.md` (CI/cargo-deny). Backend/frontend/embedded/data-plane references N/A. |
| hyperv-windows-vm | **Yes, lab phase only** | Lab guest `DNSTap-WS2025` via PowerShell Direct; `scripts/performance/LabCommon.psm1`, `docs/history/localservice-restricted-validation.md`. **The skill's `references/existing-lab.md` describes a different guest (`BFD-WS2025`, GUID `575400bf-...`, repo `bfd-winservice`). Do not reuse its VM name, GUID, credential path or IPs.** Use the repo's own helpers and the user's saved memory (`lab-guest-access.md`). |
| hyperv-ubuntu-vm | **Conditional** | The dnstap receiver is DNS-collector on `DNSTap-Ubuntu2604` (memory file; many `scripts/performance/*ubuntu*.py`). Needed only if a live end-to-end run requires reconnecting to or changing the receiver. Its `existing-lab.md` is likewise for the BIRD guest, not this receiver. |
| asd-ste100 | **Yes, docs** | Applies to CHANGELOG, guides and operator-facing error messages. |
| lua | **N/A** | No Lua in `src/`, `scripts/`, `tools/`, `fuzz/`; unrelated to the project (game-oriented guidance). |

## 3. Preliminary findings

Legend: **O** = observed in the file at the cited line; **V** = to verify (needs execution, a live host, or deeper reading). Severity is my first-pass judgement, not a measured result.

### Build and hardening configuration

- **F-1 (O, informational): the build profile conforms to the rust-security-hardening baseline.** `Cargo.toml:31-40` (dev `overflow-checks`, release `opt-level=3`, `debug=0`, `strip="symbols"`, `overflow-checks`, `panic="abort"`); `Cargo.toml:46-55` (`warnings=deny`, `unsafe_code=deny`, `unsafe_op_in_unsafe_fn=deny`, `clippy::unwrap_used`, `undocumented_unsafe_blocks`, plus correctness/pedantic); `.cargo/config.toml:12-28` (CFG, PIC, `-Z stack-protector=all`, `-Z ehcont-guard`, `/GUARD:CF`, `/GUARD:EHCONT`, `/CETCOMPAT`, `/HIGHENTROPYVA`, etc.), `:6-7` (`/DEBUG:NONE` release only), `:46-48` (`build-std`, `profile-rustflags`), `:31-41` (`test-release`, `asan` aliases), `rust-toolchain.toml:2` (pinned nightly). Explicit target at `config.toml:2` keeps build scripts on host flags.
- **F-2 (O, medium): CI never inspects the final production PE and never runs the `asan` or `test-release` aliases.** The skill requires verifying emitted CFG/EHCONT/CET/ASLR metadata with `dumpbin /headers /loadconfig` or a PE parser, and testing the production-profile PE separately from unwind-enabled tests. A search of `.github/` for `asan|dumpbin|loadconfig|test-release|sbom|persist-credentials|MSRV` returned zero matches. `ci.yml:74-75` builds release but checks nothing about the artifact. `install.md:79-83` itself says "inspect the final PE" but no script does it. **V:** whether the shipped exe actually carries the flags (the `-Z` flags and `build-std` are nightly-sensitive; `[unstable]` at `config.toml:46`).
- **F-3 (O, low-medium): MSRV claim is not testable.** `Cargo.toml:5` says `rust-version = "1.96"` but `rust-toolchain.toml:2` pins `nightly-2026-10-01` and the config needs nightly (`-Z`, `build-std`). No job builds on 1.96 stable (`rust-skills proj-msrv-declare`). Likely right answer: document "nightly only" rather than claim an MSRV. **V:** which nightly features are in use.
- **F-4 (O, low): no `lto`/`codegen-units` in release** (`Cargo.toml:35-40`). rust-skills recommends `lto="fat"`, `codegen-units=1`; rust-security-hardening does not require them. This is a performance and CFG-interaction decision, and the project has an extensive performance program (`docs/performance/`). Do not change without a perf run.
- **F-5 (O, medium): supply-chain coverage gaps.** `cargo deny` runs only at the root (`ci.yml:121`; `deny.toml:5-8`). The separate workspaces `fuzz/Cargo.lock` and `tools/dns-stress/Cargo.lock` (the latter carries serde/serde_json, `tools/dns-stress/Cargo.toml:12-13`) are outside it. `deny.toml:30` makes duplicate versions a warning only. No dependabot/renovate file exists (`.github/` holds only the two workflows). No SBOM, signing or provenance step anywhere. `publish = false` (`Cargo.toml:8`) limits crates.io exposure but the exe is deployed to servers (`install.md:205`).
- **F-6 (O, medium): fuzzing and coverage are advisory.** `fuzz-smoke` is `continue-on-error: true` (`ci.yml:168`), runs 60 s per target with `--sanitizer none` (`:187`), and installs `cargo-fuzz` without a version pin (`:183`). `coverage` is also non-blocking (`ci.yml:130`). Targets exist for `decoder`, `control_frame`, `plan_compiler` (`fuzz/Cargo.toml:22-40`). **V:** is there a committed corpus/regression set; do fuzz targets reach `copy_record` (`src/etw/intake.rs:143`), the code that handles raw native records, or only the decoder.
- **F-7 (O, medium): `lab.yml` runs repo code on a self-hosted runner that the file comment says runs as administrator.** `lab.yml:3-5` (admin runner), `:9-12` (`workflow_dispatch` plus weekly cron), `:28` (`self-hosted, windows, etw-lab`), `:40` (`cargo test -- --ignored`, which executes arbitrary test and build-script code from whatever ref is dispatched). The comment at `:5-6` correctly excludes `pull_request`, but any collaborator with write access can dispatch a branch. **V:** who has write access, runner isolation (VM is a lab guest), and whether the runner is persistent. Also, `actions/checkout` is used without `persist-credentials: false` (`ci.yml:38`, `lab.yml:34`), low risk because `contents: read`.

### Unsafe and FFI boundary

- **F-8 (O, good): panic containment matches the skill's fail-stop policy.** ETW callback is `unsafe extern "system"` (non-unwinding ABI), `src/etw/intake.rs:102`; no `catch_unwind` anywhere (grep for `catch_unwind` in `src/` found none); null checks (`:105-117`); `RefCell::try_borrow_mut` avoids a borrow panic (`:130`); context and pool identity re-checked (`:136-138`). SCM entry points are also non-unwinding (`service.rs:107,189`); poisoned lock aborts rather than unwinding (`service.rs:185`); pool exhaustion paths abort (`src/etw/pool.rs:105,120,127`). Release is `panic="abort"` (`Cargo.toml:40`). **V:** that `copy_record` (`intake.rs:143`, defined after `:209`) has no indexing/arith that can panic on hostile lengths (abort would take the service down on a malformed record; `CopyError::Malformed` exists at `:145`), and that the dev/test `unwind` builds are never shipped (install.md builds release only).
- **F-9 (O, good): token and SID parsing follow the skill's native-output rules.** Aligned `Vec<u64>` storage (`service.rs:489`, comment at `:490`), size cap (`:485-487`), `size <= capacity` re-check (`:504`), checked arithmetic (`:635-638`), pointer-range and alignment check in `bounded_sid` (`:522-541`), count cap 1024 (`:640`). Unit tests cover truncated and out-of-range cases (`:840-850`, `:1082-1093`). The two `unsafe impl Send/Sync` are documented (`service.rs:304-309`, `etw/instance.rs:14-16`). **V:** a Miri or ASan run over these tests (rust-skills `unsafe-miri-ci` expects Miri; nothing in CI, and Miri cannot run Win32 FFI, so ASan alias is the realistic option).
- **F-10 (O, low): `#[allow(unsafe_code)]` is applied per module** (`lib.rs:13`, `etw.rs:21-43` eleven attributes, `network.rs:31`), consistent with "narrow reviewed modules". **V:** that every module holding an `allow` genuinely needs it (for example `network.rs:31` is only socket options in `network/sockopt.rs`).

### Service identity, privileges, ACLs

- **F-11 (O, good): identity gate runs before configuration is read** (`service.rs:143-145`): token user must be LocalService (`:581-587`), service SID enabled and not deny-only (`:588-595`), restricted token with service SID and `S-1-5-33` (`:596-610`), Performance Log Users enabled (`:611-618`), and exactly one enabled privilege `SeChangeNotifyPrivilege` (`:701-712`). This is stricter than the skill's BFD allowlist (which keeps `SeCreateGlobalPrivilege`). Install steps match: `install.md:227-230`.
- **F-12 (O, medium, availability): the single-instance guard is a mutex in the global namespace with a default DACL**, `src/etw/instance.rs:21,36` (`Global\EtwDnstap-<session>`, `CreateMutexW(null,...)`). A local process can create that name first and cause a startup failure; the error path acknowledges this (`:40-48`). It does not escalate privilege but is a denial-of-service lever against a security-monitoring service. **V:** whether a standard user can create the mutex in the Global namespace on Server 2025, and whether the service token can still open it (lab test; also noted in `scripts/provision/README.md`).
- **F-13 (O, V for runtime): ETW grant is scoped to the session GUID** (`Grant-EtwDnstapEtwAccess.ps1:3-5,205-233`) and strips the service SID from other descriptors (`:173-203`), with `-WhatIf` support (`:79`). Lab-validated on build 26100 (`docs/history/localservice-restricted-validation.md:120-135`). Not covered per `:155-160`: SCM recovery with a DNS restart, load, other builds. **V:** the script edits `HKLM\...\WMI\Security` directly (`:97,191,199`), so a registry-snapshot rollback test is advisable; and the instruction that ETW applies descriptors only at boot (`install.md:264`).
- **F-14 (O, V): service DACL and executable ACL.** Install directory is SYSTEM/Administrators full control with service SID read/execute (`install.md:194-195,210`). `install.md:327` points to `sc.exe sdshow EtwDnstap` but no `sdset` is applied, so the service ACL is the Windows default. Skill expects an explicit SCM ACL where authenticated users can query but not start/stop/reconfigure. **V:** read the real `sdshow` output on the lab guest.
- **F-15 (O): install and repair are manual and unversioned.** No rollback snapshot of prior account, privileges or ACLs (skill: "preserve prior values in install/repair rollback"); the repair section is at `install.md:431`. **V:** read `install.md:431-end` to see whether it captures prior config.

### Versioning and docs

- **F-16 (O): CHANGELOG is internally inconsistent.** `CHANGELOG.md:31` still says `Tests, fuzzing and CI: _pending_` although commit `12cce7a` added them. `:43` says the LocalService profile is "Validated on build 26100" but `:93-94` under Security says it "has not been validated on a live DNS Server". `:8` states no tags exist; `Cargo.toml:3` is `0.1.0`, the same number as the untagged `[0.1.0]` heading (`:96`) that already lists removed CLI options (`:119-124`), so `0.1.0` is not a coherent release. Per semantic-versioning, a `0.y.z` policy must be stated; recommend the first tag only after the human picks one.

### Other

- **F-17 (V): `windows-sys` feature set** (`Cargo.toml:19`) includes `Win32_Security_Cryptography`, `Win32_Media`, `Win32_Networking_WinSock`. Cryptography is used (`src/etw/session_guid.rs:18`, BCryptHash for the session GUID) and `Win32_Media` is used by timer resolution (`src/etw/diag.rs:48,55`). **V:** whether `timeBeginPeriod` code is built in production without the `diag-timing` feature, i.e. whether the production binary carries it.
- **F-18 (V): Python tooling** (`scripts/performance/*.py`, `requirements.txt`) is lab-only; CI runs only two tests (`ci.yml:94-97`). Not security-critical, but dependencies in `requirements.txt` are outside cargo-deny. Low priority.

## 4. Agents and phase plan

Agent definitions were not provided to me (no agent glob was given), so the type names below are the ones the task suggested; the orchestrator must confirm these definition files exist and read their `tools` fields (a read-only reviewer must not be given Write/Edit on the repo). Total: 5 working agents, plus one optional consolidation by the orchestrator.

### Phase 0: preconditions (human + orchestrator, no agent)
- Answer the decision points in section 7.
- Confirm `asd-ste100` situation.
- Create a findings directory outside the repo, e.g. `...\scratchpad\eval\`, one file per agent.
- Note the shared-state hazard: the repo `target\` holds 39,000+ files (lab media, perf archives). Agents must use a dedicated `CARGO_TARGET_DIR` (memory note: scratchpad paths hit MAX_PATH at link time, so use a short path under repo `target\`, which is git-ignored).

### Phase 1: static review (parallel, no VM, read-only)

| Step | Agent | Skills to load | Inputs | Output |
|---|---|---|---|---|
| 1A | **security-auditor** | rust-security-hardening (+ `references/build-profile.md`), windows-service-security (principles only, ignore BFD COM/MI/RPC parts), rust-windows-hardening as a checklist | `src/service.rs`, `src/etw/*.rs` (intake, session, instance, pool, schema), `src/network/sockopt.rs`, `scripts/provision/*.ps1`, `docs/guide/install.md`, `docs/history/localservice-restricted-validation.md` | `eval/1A-security.md`: confirm or refute F-8 to F-15; add new unsafe/ownership findings with file:line; list items that need the lab (feeds Phase 3) |
| 1B | **build-engineer** (also covers CI/devops) | rust-security-hardening `assets/` + `build-profile.md`, build-rust `references/tooling.md`, rust-skills (`lint-`, `proj-`, `opt-`) | `Cargo.toml`, `.cargo/config.toml`, `rust-toolchain.toml`, `deny.toml`, `fuzz/Cargo.toml`, `tools/dns-stress/Cargo.toml`, both workflows, `Cargo.lock` | `eval/1B-build-ci.md`: F-1 to F-7 verdicts, effective-flags review (run `cargo rustc -- --print` style inspection or `cargo build -v` to see the real command line, in a separate target dir), proposed CI additions (PE check job, ASan job, blocking fuzz, deny for all lockfiles, SBOM/signing/release design) |
| 1C | **rust-engineer** (also owns test and fuzz execution; no separate test-automator) | rust-skills (`own-`, `err-`, `unsafe-`, `num-`, `test-`), rust-check (fmt/clippy only), build-rust `ffi-unsafe.md`/`cli.md` | `src/**` (focus `pipeline.rs`, `framestream.rs`, `dnstap.rs`, `etw/decode.rs`, `etw/schema.rs`), tests, `fuzz/fuzz_targets/*` | `eval/1C-rust.md`: idiom and API findings, whether fuzz targets reach native-record copy and control-frame paths, test gaps; results of `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, `cargo test --locked` run locally in an isolated target dir |
| 1D | **documentation-engineer** (small) | semantic-versioning | `CHANGELOG.md`, `Cargo.toml`, `git log`, `docs/README.md`, `CONTRIBUTING.md`, `SECURITY.md` | `eval/1D-versioning-docs.md`: F-16, proposed baseline and first-tag version with SemVer vs local-policy stated separately; stale-doc list; link check via `scripts/ci/linkcheck.py` (read-only) |

1A through 1D are independent and may run in parallel. They only read the repo. 1B and 1C both run cargo, so they must use different `CARGO_TARGET_DIR`s (or run 1B's build after 1C finishes) to avoid the cargo build lock and cache clobbering.

### Phase 2: dynamic local checks (needs host, not the VM)
Run by **build-engineer** (continuation of 1B, same agent) after Phase 1 completes:
- `cargo build --locked --release` into an isolated target dir, then inspect the PE (`dumpbin /headers /loadconfig` if Visual Studio tools are present, otherwise a reviewed PE parser) for ASLR/HIGHENTROPYVA/NX/CFG/EHCONT/CET flags and absence of debug output. Output: `eval/2-pe-evidence.md`. Report configuration intent separately from observed artifact evidence.
- `cargo asan` (needs the VS ASan runtime on PATH per `install.md:107-115`). Report instrumentation scope honestly (Rust and rebuilt std only).
- `cargo test-release` then rebuild plain release.
- Optional: `cargo fuzz` locally for longer than the CI 60 s.
- Dependency: Phase 1B's effective-flags note, and `rust-engineer`'s test results (to avoid duplicate runs).
These can start in parallel with Phase 3 preparation but the cargo steps must not share a target dir with other cargo runs.

### Phase 3: lab verification (needs the VM, gated by decision D-2)
Agent: **windows-infra-admin**. Skills: hyperv-windows-vm (generic parts only), windows-service-security (verification section), optionally hyperv-ubuntu-vm for the receiver.
Prerequisites: Phase 1A list of "needs lab" items and Phase 2's release exe.
Steps (each states whether it mutates the guest):
1. Inventory `Get-VM` and verify name `DNSTap-WS2025` and GUID before any action (do not trust the BFD profile). Use `powershell.exe` 5.1 with `scripts/performance/LabCommon.psm1` (`Get-LabVm`, `Get-LabSession`); import the DPAPI credential inside the 5.1 script (memory note). Read-only.
2. Take a checkpoint before any change (the validation doc notes an existing checkpoint restores the earlier LocalSystem profile; do not overwrite it). Mutates guest metadata only.
3. Read-only evidence: `sc.exe qc|qprivs|qsidtype|sdshow|qfailure|qfailureflag EtwDnstap`, `icacls` on `C:\Program Files\EtwDnstap`, `HKLM\...\WMI\Security` descriptors for the session GUID, token as seen by the running service (F-11, F-13, F-14). Read-only.
4. Mutating, only if authorized: start/stop service, mutex-squat test (F-12) as a non-admin user, SCM failure-recovery plus DNS restart (listed as uncovered at `localservice-restricted-validation.md:155-158`), reboot-to-apply ETW ACL test (reboot required). Restore state and report whether the VM remains running.
5. Optional: a short DNS load against the receiver to confirm F-8 behavior under malformed/hostile events is not feasible without a custom ETW injector; mark as not tested rather than claim.
Output: `eval/3-lab-evidence.md` with VM identity, build/provenance, per-check command and result, and an explicit list of unrun checks. Distinguish test fixtures from SCM-issued tokens.

### Phase 4: consolidation
**code-reviewer** (read-only, reads `eval/*.md`) de-duplicates, resolves conflicts between agents, ranks findings, and produces `eval/FINAL-findings.md` marking each as confirmed, refuted, or unverified. If the orchestrator prefers fewer agents, it can do this itself. If changes are authorized (D-1 = fix), a second pass uses **rust-engineer** for code fixes and **build-engineer** for CI/config edits, each on a branch, followed by a re-run of Phase 2.

### Dependency and parallelism summary
```
Phase 0 -> [1A | 1B | 1C | 1D]  (parallel, read-only)
              \     |     /        \
               Phase 2 (1B continues; isolated target dir)
                        \
                   Phase 3 (windows-infra-admin; gated by D-2; needs Phase 2 exe, Phase 1A lab list)
                        \
                   Phase 4 (code-reviewer / orchestrator)
```
Handoff files (all outside the repo): `eval/1A-security.md`, `1B-build-ci.md`, `1C-rust.md`, `1D-versioning-docs.md`, `2-pe-evidence.md`, `3-lab-evidence.md`, `FINAL-findings.md`. Each agent writes only its own file.

## 5. What static review can and cannot show

- Static only: F-1, F-3, F-4, F-5, F-6, F-7 (config part), F-10, F-16, matrix items.
- Needs local execution: F-2 (PE), ASan, test-release, clippy/test pass state, fuzz reach.
- Needs the lab VM: F-12, F-13 runtime, F-14, F-15 repair behavior, SCM recovery, DNS restart, anything about the real SCM-issued token.
- Not coverable by any available skill: release/installer recovery (no MSI), RPC/C/C++ (not present).

## 6. Risks

- **Lab mutation and reboot.** ETW descriptor changes take effect only on reboot; any grant test changes machine ETW security state. Mitigate with the existing checkpoint, `-WhatIf` first, and post-run state restoration.
- **Self-hosted runner.** Running or editing `lab.yml` in a way that adds triggers could expose the admin runner; the plan only reads it.
- **Wrong lab profile.** The `hyperv-windows-vm` skill's saved lab data is for BFD; using it would target the wrong VM/credential.
- **Nightly toolchain drift.** `nightly-2026-10-01` is pinned (`rust-toolchain.toml:2`); local runs may auto-download toolchains and `build-std` needs `rust-src`; first build is slow (`install.md:71`). Running 1B and 1C simultaneously can contend for the cargo lock.
- **Skill mismatch.** BFD-derived skills assume two services, RPC and COM/MI; applying their requirements literally (for example a broker, ncalrpc, or retaining `SeCreateGlobalPrivilege`) would be wrong for this single-service design. Agents must treat BFD-specific profiles as examples.
- **rust-check template.** Contains unrelated project rules; agents should not "fix" the repo toward async, tempfile or assert_cmd.
- **Unverified agent definitions.** Voltagent type names were suggested but not read; confirm tool grants.
- **Evidence honesty.** ASan covers only Rust-compiled code; CET compatibility bit is not enforcement; fixture tests do not prove SCM tokens. Reports must say so.
- **Large `target\` tree.** Greps and globs over the repo root without excluding `target\` are slow and noisy (the root has tens of thousands of files); agents should scope to tracked directories.

## 7. Decision points for the human (answer before execution)

- **D-1: report only, or allow changes?** If changes: on a branch or `main`? Which areas (CI, Cargo profile, docs/CHANGELOG, source)? Default recommended: report only, no repo edits.
- **D-2: may agents use the lab VM?** If yes: read-only inspection only, or also start/stop service, mutex-squat test, DNS-restart/recovery test, ETW ACL change plus guest reboot? May the existing checkpoint be consumed or must a new one be taken first? May the Ubuntu receiver `DNSTap-Ubuntu2604` be touched?
- **D-3: `asd-ste100`.** The skill is missing from disk. Reinstall, skip, or is it meant to govern documentation wording (then docs review scope grows)?
- **D-4: release scope.** Is a release pipeline (tags, signed artifacts, SBOM, MSI or zip) in scope as a recommendation only, or should the plan include designing one? Which versioning policy for `0.y.z`, and what is the baseline for `0.1.0`?
- **D-5: performance vs hardening trade-offs.** May the review recommend `lto`/`codegen-units` (F-4) given the performance program, or must such items be flagged without recommendation?
- **D-6: CI strictness.** Should recommendations make `fuzz-smoke` and `coverage` blocking, and add a PE-verification and ASan job (F-2, F-6)?
- **D-7: local execution.** May agents run `cargo build/test/asan/fuzz` on this workstation (long first build, writes under `target\`), and is Visual Studio with the ASan component installed?
- **D-8: self-hosted runner.** Is the `etw-lab` runner actually registered, and who has dispatch rights (F-7)?

## 8. Decisions (Nov 1, 2026)

**Execution status: Executed** on Nov 1, 2026, through commit `5f610f5`. The lab
results are in [the lab record](skills-evaluation-lab-validation.md).
Work that the lab record lists under "Not covered" was not executed. This plan
belongs in `docs/history/` under the status rules in CONTRIBUTING.md, and it should
move there with `git mv`. Links to it are in `docs/README.md`.

- **D-1:** Changes are allowed, committed directly on `main`.
- **D-2:** The lab VMs may be changed as needed, including service start/stop, the mutex-squat test, SCM recovery with a DNS restart, ETW ACL changes and guest reboots. Take a checkpoint first.
- **D-3:** Resolved. The skill is installed.
- **D-7:** Agents may run cargo build, test, asan and fuzz on the workstation.
- **D-4, D-5, D-6, D-8 (orchestrator defaults):**
  - D-4: a release pipeline, signing and SBOM are recommendations only. Signing needs a certificate the repo does not have.
  - D-5: `lto` and `codegen-units` are not changed without a performance run. They are recorded as a recommendation.
  - D-6: add a PE-verification step. ASan and fuzz stay advisory until they pass on hosted runners, but pin `cargo-fuzz`.
  - D-8: `gh` is not installed, so runner registration and dispatch rights cannot be checked from here.
