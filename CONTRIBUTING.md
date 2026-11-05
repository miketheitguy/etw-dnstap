# Contributing

This project is a Windows-only Rust service. Report data exposure or privilege problems under the [security policy](SECURITY.md).

## Build and check

Install the toolchain that [rust-toolchain.toml](rust-toolchain.toml) pins. See the [prerequisites](docs/guide/install.md#prerequisites) and [Install the Rust toolchain](docs/guide/build.md#install-the-rust-toolchain).

Run these from the repository root before you open a pull request:

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

- `--locked` is required. The build uses the committed `Cargo.lock`.
- `Cargo.toml` denies rustc warnings and the Clippy lints `unwrap_used`, `undocumented_unsafe_blocks`, `print_stdout`, `print_stderr`, and `dbg_macro`. CI fails on every Clippy warning, including `pedantic`. Add `#[allow]` only with a reason comment.
- `multiple_unsafe_ops_per_block`, `missing_const_for_fn`, and `shadow_unrelated` also fail CI. Use one unsafe operation per `unsafe` block, with a `// SAFETY:` comment. A test module can carry one `#![expect(clippy::shadow_unrelated, reason = "...")]` at the top of its file.
- `cargo test --locked` reports 790 passed, 0 failed, 9 ignored. `--features diag-timing` reports 795 passed, and `--features bench` 794. Counts change as tests are added, so check the failed count.
- Five ignored tests need an elevated session, the DNS Server role, or the production performance-counter provider. Four are manual benchmarks (`synthetic_pooled_intake_benchmark`, `parse_encode_benchmark`, `bench_seal_and_open_64_kib_records`, `with_aead_cost_benchmark`). The `lab.yml` workflow runs them on the `etw-lab` runner. Keep them ignored.

### Continuous integration

CI (`.github/workflows/ci.yml`, `windows-latest`) runs:

- `cargo fmt --check` in the root, `fuzz/`, and `tools/dns-stress/`.
- Clippy with `-D warnings` and tests, for the default, `diag-timing`, `fuzzing` (Clippy only), and `bench` feature sets.
- The release build, with output in `release-build.log`.
- `scripts/ci/verify-link-exceptions.ps1`: fails when an LNK2047 object (ring's prebuilt NASM objects) is missing from `scripts/ci/link-exceptions.txt`, or a listed object is no longer reported.
- `scripts/ci/verify-pe.ps1`: checks the hardening flags and fails on a PDB entry or an ASan or dynamic CRT import.
- `tools/dns-stress` checks, and the Python tests `test_metrics_ledger.py`, `test_pooled_intake_run.py`, and `archive/test_archived_analyzers.py` in `scripts/performance`.
- `cargo deny` on the root, `fuzz/`, and `tools/dns-stress/` lockfiles.
- `python -I scripts/ci/linkcheck.py .`.

Four jobs are advisory:

- `sanitizers` runs `cargo test-release` and `cargo asan`.
- `miri` runs `cargo miri test` on `etw::decode`, `framestream`, `dnstap`, and `etw::keywords`. To run it locally, run `rustup component add miri`, then `cargo miri setup` from a directory outside the repository that holds a copy of `rust-toolchain.toml`. Then run the job's command with its `MIRIFLAGS`, `PROPTEST_CASES`, and `PROPTEST_DISABLE_FAILURE_PERSISTENCE` settings.
- `coverage` reports only, with no threshold.
- `fuzz-smoke` runs `decoder`, `control_frame`, and `plan_compiler` for 60 seconds each. A local `cargo fuzz run` fails on MSVC with an LLVM COMDAT error.

Property tests use `proptest`. Golden byte vectors in `src/dnstap/golden_tests.rs` check the encoder against `dnstap.proto`. The fuzz targets (`decoder`, `control_frame`, `plan_compiler`, `spool_segment`) live in `fuzz/` and build with the `fuzzing` feature. Install the runner with `cargo install cargo-fuzz --version 0.13.2 --locked`. CI pins cargo-deny 0.20.2 and cargo-llvm-cov 0.9.1 in `.github/workflows/ci.yml`.

`cargo test-release` and `cargo asan`. It stays advisory until it passes on hosted runners.
- `miri` runs `cargo miri test` on `etw::decode`, `framestream`, `dnstap`, and `etw::keywords`. Its name filters skip every test that calls a `windows-sys` function, and three slow tests. To run it locally, run `rustup component add miri`, then `cargo miri setup` from a directory outside the repository that holds a copy of `rust-toolchain.toml` (the sysroot build fails inside the repository). Then run the job's command with its `MIRIFLAGS`, `PROPTEST_CASES`, and `PROPTEST_DISABLE_FAILURE_PERSISTENCE` settings.
- `coverage` reports only, with no threshold.
- `fuzz-smoke` runs each of `decoder`, `control_frame`, and `plan_compiler` for 60 seconds. A local `cargo fuzz run` fails at build on MSVC with an LLVM COMDAT error, so `fuzz-smoke` stays advisory.

Property tests use `proptest`. Golden byte vectors in `src/dnstap/golden_tests.rs` check the encoder against `dnstap.proto`. The fuzz targets (`decoder`, `control_frame`, `plan_compiler`, and `spool_segment`) live in the separate `fuzz/` workspace. They build with the `fuzzing` feature, which exposes internal entry points and is enabled only for fuzzing. Install the runner with `cargo install cargo-fuzz --version 0.13.2 --locked`. CI pins cargo-deny 0.20.2 and cargo-llvm-cov 0.9.1 in `.github/workflows/ci.yml`. Dependabot does not update these pins.

`cargo test-release` and `cargo asan` are aliases in `.cargo/config.toml`. `cargo asan` writes to `target\asan` and needs the Visual Studio AddressSanitizer runtime. See the [build guide](docs/guide/build.md#optional-address-sanitizer-builds).

### Benchmarks

The criterion benchmark in `benches/hot_path.rs` measures the per-event hot path: slot copy, loopback socket write, decode, encode, `parse_and_encode`, and a two-thread producer and consumer. It needs the `bench` feature. Run the alias, which adds the feature and builds the standard library for unwinding:

```powershell
cargo bench-hot-path
```

Reports go to `target\criterion\`. Judge a change with an A/B run in one session, as in [the baseline record](datasets/baselines/code-review-2026-11-01/README.md#comparing-a-change-with-the-baseline):

```powershell
cargo bench-hot-path -- --save-baseline pre
cargo bench-hot-path -- --baseline pre
```

The `tools/dns-stress` load generator is a separate Cargo project. Build and test it from its own directory:

```powershell
cd tools\dns-stress
cargo test --locked
```

## Documentation

The documentation index is [docs/README.md](docs/README.md). Every document in `docs/` starts with a status line under its title:

```markdown
> **Status:** Current | Historical | Superseded | Proposed · **Evidence dated:** ... · **Commit:** ... · **Reviewed:** YYYY-MM-DD
```

A Current document uses **Describes** (for example `main` at commit `abc1234`) in place of **Commit**. A Historical record gives the commit it describes, or "not recorded". See [Status values](docs/README.md#status-values).

[docs/performance/pooled-vm-performance-review.md](docs/performance/pooled-vm-performance-review.md) stays Current while it is the latest native performance record. When a later native campaign completes, mark it Historical and update this paragraph.

The status decides the folder under `docs/`: `guide/` (Current operator guides), `design/` (Current architecture and reference, diagrams in `design/diagrams/`), `performance/` (the current performance record and the records the report generator links), `plans/` (Proposed plans, moved to `history/` when executed), `history/` (Historical and Superseded records), `assets/performance/` (generated graphs, default output of `plot-pooled-wpr.py`), `assets/pipeline/` (the data-path animation, built by `scripts/docs/pipeline-animation/Build-Animation.ps1`), and `assets/loss-solutioning/` (Historical animations).

When a status changes, move the document with `git mv`, update every relative link to it, and update its row in [docs/README.md](docs/README.md). In a Historical record, add a note that names the new path when a file moves.

Rules for documentation changes:

- Add a note in place of editing numeric evidence in a Historical record.
- Define an abbreviation at first use, or link to [configuration.md](docs/guide/configuration.md) or [measurement-terms.md](docs/design/measurement-terms.md).
- Link to source symbols (function, type, or constant names), not line numbers.
- Relative links and anchors must resolve. Label a link into the gitignored `target/` as a local artifact.
- Leave receiver addresses, query names, and capture data out of documents.

### Document types

Each Current document has one job. Put new text where its job matches.

- **Procedure** (`build.md`, `install.md`, `receiver.md`, `upgrade.md`, `uninstall.md`, `service-stop-remediation.md`): numbered steps, with a check after each step that can fail.
- **Reference** (`configuration.md`, `tuning.md`, `monitoring.md`, `measurement-terms.md`): options, defaults, ranges, counters, log events.
- **Diagnosis** (`troubleshooting.md`): symptom, cause, corrective action.
- **Design** (`data-path.md`, `pooled-intake.md`, `ring-buffer.md`, `single-pass-decoder.md`): how code works, and why.
- **Record** (`docs/performance/`, `docs/history/`): results of one campaign, review, or plan.

A procedure links to the reference for option details.

### Writing style

Current documents use Simplified Technical English (ASD-STE100). The [`asd-ste100` skill](.agents/skills/asd-ste100/SKILL.md) has the rules. Use *Strict* mode for procedure steps, messages, and script output. Use *STE-flavored* mode for other prose.

- Write one instruction in each procedure sentence, verb first.
- Keep a procedure sentence to 20 words or fewer, and a description sentence to 25 or fewer. A long command or path does not count.
- Write two sentences or a list in place of a semicolon.
- Use a numbered list for three or more steps, and a bulleted list for three or more conditions.
- Use the active voice when the actor is known.
- Keep each hedge the evidence needs.
- Use **check** for an operator action that examines a result. Keep **validate** and **validation** for a lab validation run and its record.

Lint each Current document you change:

```powershell
python -I .agents\skills\asd-ste100\scripts\ste-lint.py README.md docs\guide\install.md
```

Passive-voice and tense findings are advisory. Historical records keep their style.

## Commits and pull requests

Use short imperative subjects in sentence case, under about 72 characters, such as `Add single-pass decoding, ETW flush control, and low-rate defaults`. Describe the behavior change in the body. Keep build output, captures, ETL files, and `target/` out of commits.

A pull request states the behavior and defaults that changed, lists the commands you ran with results, and adds a [CHANGELOG.md](CHANGELOG.md) `[Unreleased]` entry when it applies. Say so if your change needs a live ETW or DNS Server check, which unit tests cannot give.

## Scope

Lab scripts under `scripts/` write to real VMs and can change DNS zones and disks. They need a dry-run path and a review from someone who can run them against the lab.
