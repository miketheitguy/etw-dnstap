# Contributing

### Quick Start

Install the toolchain [rust-toolchain.toml](rust-toolchain.toml) pins ([prerequisites](docs/guide/install.md#prerequisites), [Install the Rust toolchain](docs/guide/build.md#install-the-rust-toolchain)). Before a pull request, run from the repository root:

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

Data exposure or privilege bugs: [security policy](SECURITY.md).

### Steps / Configuration

#### Build and check

- `--locked` is required (committed `Cargo.lock`).
- `Cargo.toml` denies rustc warnings and `unwrap_used`, `undocumented_unsafe_blocks`, `print_stdout`, `print_stderr`, `dbg_macro`. CI fails on every Clippy warning, `pedantic` included. `#[allow]` needs a reason comment.
- `multiple_unsafe_ops_per_block`, `missing_const_for_fn`, `shadow_unrelated` also fail CI. One unsafe operation per `unsafe` block, with a `// SAFETY:` comment. A test module can carry one `#![expect(clippy::shadow_unrelated, reason = "...")]` at the top.
- Expected: default 790 passed, 0 failed, 9 ignored. `--features diag-timing` 795, `--features bench` 794. Watch the failed count.
- Ignored: five need elevation, the DNS Server role, or the production performance-counter provider. Four are manual benchmarks (`synthetic_pooled_intake_benchmark`, `parse_encode_benchmark`, `bench_seal_and_open_64_kib_records`, `with_aead_cost_benchmark`). `lab.yml` runs them on `etw-lab`.

#### Continuous integration

`.github/workflows/ci.yml` (`windows-latest`) gates on:

- `cargo fmt --check` in the root, `fuzz/`, `tools/dns-stress/`.
- Clippy and tests for default, `diag-timing`, `fuzzing` (Clippy only), `bench`.
- Release build (`release-build.log`).
- `scripts/ci/verify-link-exceptions.ps1`: LNK2047 objects (ring's NASM objects) must match `scripts/ci/link-exceptions.txt` both ways.
- `scripts/ci/verify-pe.ps1`: hardening flags; no PDB entry, ASan, or dynamic CRT import.
- `tools/dns-stress` checks; `scripts/performance` tests `test_metrics_ledger.py`, `test_pooled_intake_run.py`, `archive/test_archived_analyzers.py`.
- `cargo deny` on the root, `fuzz/`, `tools/dns-stress/` lockfiles.
- `python -I scripts/ci/linkcheck.py .`
- A `git grep` for host directory paths (see `CLAUDE.md`).

Advisory:

- `sanitizers`: `cargo test-release`, `cargo asan`, until they pass on hosted runners.
- `miri`: `cargo miri test` on `etw::decode`, `framestream`, `dnstap`, `etw::keywords`, skipping `windows-sys` callers and three slow tests.
- `coverage`: no threshold.
- `fuzz-smoke`: `decoder`, `control_frame`, `plan_compiler`, `spool_segment`, 60 s each.

Miri locally: `rustup component add miri`, then `cargo miri setup` outside the repository, next to a copy of `rust-toolchain.toml`. Run the job's command with its `MIRIFLAGS`, `PROPTEST_CASES`, `PROPTEST_DISABLE_FAILURE_PERSISTENCE`.

#### Tests, fuzzing, benchmarks

- `proptest` property tests. `src/dnstap/golden_tests.rs` checks the encoder against `dnstap.proto`.
- Fuzz targets `decoder`, `control_frame`, `plan_compiler`, `spool_segment` live in the `fuzz/` workspace, built with the fuzzing-only `fuzzing` feature. Runner: `cargo install cargo-fuzz --version 0.13.2 --locked`.
- `ci.yml` pins cargo-deny 0.20.2 and cargo-llvm-cov 0.9.1.
- `cargo test-release` and `cargo asan` are `.cargo/config.toml` aliases. `cargo asan` writes `target\asan` and needs the Visual Studio AddressSanitizer runtime ([build guide](docs/guide/build.md#optional-address-sanitizer-builds)).
- `cargo bench-hot-path` runs `benches/hot_path.rs` (criterion, `bench` feature): slot copy, loopback write, decode, encode, `parse_and_encode`, two-thread producer and consumer. Reports: `target\criterion\`.

A/B a change in one session ([baseline record](datasets/baselines/code-review-2026-11-01/README.md#comparing-a-change-with-the-baseline)):

```powershell
cargo bench-hot-path -- --save-baseline pre
cargo bench-hot-path -- --baseline pre
```

`tools/dns-stress` is its own Cargo project:

```powershell
cd tools\dns-stress
cargo test --locked
```

#### Documentation

Index: [docs/README.md](docs/README.md). Every `docs/` file starts with:

```markdown
> **Status:** Current | Historical | Superseded | Proposed · **Evidence dated:** ... · **Commit:** ... · **Reviewed:** YYYY-MM-DD
```

Current uses **Describes** (`main` at commit `abc1234`) for **Commit**. Historical gives the commit or "not recorded" ([Status values](docs/README.md#status-values)). Status picks the folder:

- `guide/`, `design/`: Current.
- `plans/`: Proposed. Move a plan to `history/` when it's executed.
- `history/`: Historical and Superseded, with their diagrams (`history/diagrams/`) and assets (`history/assets/`, including the `plot-pooled-wpr.py` graphs).
- `assets/pipeline/`: the Current data-path GIF (`scripts/docs/pipeline-animation/Build-Animation.ps1`).

Rules:

- Moving a file: `git mv`, fix relative links, update its [docs/README.md](docs/README.md) row, and note the new path in Historical records.
- Historical numbers: add a note, don't edit.
- Define abbreviations at first use, or link [configuration.md](docs/guide/configuration.md) or [measurement-terms.md](docs/design/measurement-terms.md).
- Link symbols (function, type, constant), not line numbers. Label links into gitignored `target/` as local artifacts.
- No receiver addresses, query names, or capture data.

##### Document types

Current documents: the H1, the status line, then exactly three H3s. The index, `docs/README.md`, is the exception: it uses task headings.


- **Quick Start:** one code block or three commands.
- **Steps / Configuration:** imperative steps, reference tables.
- **Common Gotchas:** the document's own limits and warnings, or "None known."
- Demote a linked heading to `####` and keep its exact text. `git grep -n "<anchor>"` before you drop one.

One job per document:

- **Procedure** (`build.md`, `install.md`, `receiver.md`, `upgrade.md`, `uninstall.md`, `service-stop-remediation.md`): numbered steps, a check after each step that can fail, links to the reference.
- **Reference** (`configuration.md`, `tuning.md`, `monitoring.md`, `measurement-terms.md`): options, defaults, ranges, counters, log events.
- **Diagnosis** (`troubleshooting.md`): symptom, cause, fix.
- **Design** (`data-path.md`, `pooled-intake.md`, `ring-buffer.md`, `single-pass-decoder.md`): how code works, and why.
- **Record** (`docs/history/`): one campaign, review, or plan.

##### Writing style

Use contractions and the imperative, skip intros, outros, and filler.

- "Run this command", not "The command should be executed".
- Start at step one.
- No "robust", "seamlessly", "leverage", "utilize", "in order to", "note that". Say "make sure", not "ensure that".
- Three-line paragraphs, max.
- Bold: file names, keys, critical warnings.
- Cut words, never facts: options, defaults, units, event IDs, counters, messages, and numbers stay exact.
- **check** is an operator action. **validate** means a lab validation run.
- Historical records keep their style.

#### Commits and pull requests

- Short imperative sentence-case subject, under about 72 characters: `Add single-pass decoding, ETW flush control, and low-rate defaults`. Body: the behavior change.
- PR: changed behavior and defaults, commands run with results, a [CHANGELOG.md](CHANGELOG.md) `[Unreleased]` entry when it applies.

### Common Gotchas

- Run `cargo fuzz` from outside the repository with `+<toolchain>`, `--fuzz-dir <repo>/fuzz`, and `--sanitizer none`, as `fuzz-smoke` does. Inside it, the `build-std` setting fails the build with an LLVM COMDAT error.
- `cargo miri setup` fails inside the repository.
- Dependabot doesn't update the cargo-deny and cargo-llvm-cov pins.
- Keep the nine ignored tests ignored.
- Unit tests can't check live ETW or DNS Server behavior. Say so in the PR when that matters.
- Keep build output, captures, ETL files, and `target/` out of commits.
- Lab scripts under `scripts/` change real VMs, DNS zones, and disks. They need a dry-run path and review by someone who can run them in the lab.
