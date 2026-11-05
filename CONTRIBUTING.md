# Contributing

Thank you for helping. This project is a Windows-only Rust service. Read the [security policy](SECURITY.md) before you report a problem that involves data exposure or privilege.

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
- `Cargo.toml` denies rustc warnings and the Clippy lints `unwrap_used`, `undocumented_unsafe_blocks`, `print_stdout`, `print_stderr`, and `dbg_macro`. CI treats every Clippy warning, including `pedantic`, as an error. Do not add `#[allow]` without a comment that says why.
- `multiple_unsafe_ops_per_block`, `missing_const_for_fn`, and `shadow_unrelated` also fail CI. Put one unsafe operation in each `unsafe` block, and give each one a `// SAFETY:` comment. A test module that rebinds a name in many sequential cases can carry one `#![expect(clippy::shadow_unrelated, reason = "...")]` at the top of its file. Do not use `allow`.
- The first build rebuilds the standard library (`build-std`), so it is slow.
- Expected results at commit `66c3de4` (2026-11-05). `cargo test --locked` reports 790 passed, 0 failed, and 9 ignored. That total is 788 library unit tests, plus the `console_output` and `version_resource` integration tests. `--features diag-timing` reports 795 passed, and `--features bench` reports 794 passed. Both report 9 ignored. The counts change as tests are added, so check the failed count first.
- Five ignored tests need an elevated session, the DNS Server role, or the production performance-counter provider. Four are manual benchmarks: `synthetic_pooled_intake_benchmark`, `parse_encode_benchmark`, `bench_seal_and_open_64_kib_records`, and `with_aead_cost_benchmark`. The `lab.yml` workflow runs all ignored tests on the self-hosted `etw-lab` runner. Do not un-ignore them in a pull request.

### Continuous integration

CI (`.github/workflows/ci.yml`, `windows-latest`) runs these checks:

| Check | Command or script |
| --- | --- |
| Format | `cargo fmt --check`, in the root, `fuzz/`, and `tools/dns-stress/` |
| Clippy | `-D warnings`, for the default, `diag-timing`, `fuzzing`, and `bench` feature sets |
| Tests | The default, `diag-timing`, and `bench` feature sets |
| Release build | Output in `release-build.log` |
| Link exceptions | `scripts/ci/verify-link-exceptions.ps1` checks that log. It fails when an LNK2047 object is missing from `scripts/ci/link-exceptions.txt`. LNK2047 objects are ring's prebuilt NASM objects, which lack EH-continuation metadata. The script also fails when a listed object is no longer reported. |
| PE hardening | `scripts/ci/verify-pe.ps1` on the release executable. It checks the hardening flags, and it fails on a PDB entry or an ASan or dynamic CRT import. |
| Other | `tools/dns-stress` checks. Three Python test files in `scripts/performance` run: `test_metrics_ledger.py`, `test_pooled_intake_run.py`, and `archive/test_archived_analyzers.py`. `cargo deny` runs on the root, `fuzz/`, and `tools/dns-stress/` lockfiles. The Markdown link check runs with `python -I scripts/ci/linkcheck.py .` |

Four jobs are advisory. They do not block a merge:

- `sanitizers` runs `cargo test-release` and `cargo asan`. It stays advisory until it passes on hosted runners.
- `miri` runs `cargo miri test` on `etw::decode`, `framestream`, `dnstap`, and `etw::keywords`. Its name filters skip every test that calls a `windows-sys` function, and three slow tests. To run it locally, run `rustup component add miri`, then `cargo miri setup` from a directory outside the repository, which holds a copy of `rust-toolchain.toml` (the sysroot build fails inside the repository). Then run the job's command with its `MIRIFLAGS`, `PROPTEST_CASES`, and `PROPTEST_DISABLE_FAILURE_PERSISTENCE` settings.

- `coverage` is report only. It has no threshold.
- `fuzz-smoke` runs each of `decoder`, `control_frame`, and `plan_compiler` for 60 seconds. It does not run `spool_segment`. A local `cargo fuzz run` fails at build on MSVC with an LLVM COMDAT error, so `fuzz-smoke` stays advisory.

Property tests use `proptest`. Golden byte vectors in `src/dnstap/golden_tests.rs` compare the encoder with the upstream `dnstap.proto`. The fuzz targets (`decoder`, `control_frame`, `plan_compiler`, and `spool_segment`) live in the separate `fuzz/` workspace. They build with the `fuzzing` feature, which exposes internal entry points and is never enabled in a shipped build. Install the runner with `cargo install cargo-fuzz --version 0.13.2 --locked`. CI pins cargo-fuzz 0.13.2, cargo-deny 0.20.2, and cargo-llvm-cov 0.9.1 by hand in `.github/workflows/ci.yml`. Dependabot does not update these pins.

`cargo test-release` and `cargo asan` are aliases in `.cargo/config.toml`. `cargo asan` writes to `target\asan` and needs the Visual Studio AddressSanitizer runtime. See the [build guide](docs/guide/build.md#optional-address-sanitizer-builds).

### Benchmarks

The criterion benchmark in `benches/hot_path.rs` measures the per-event hot path on deterministic DNS records. It covers slot copy, loopback socket write, decode, encode, `parse_and_encode`, and a two-thread producer and consumer. It needs the `bench` feature, which is never enabled in a shipped build. Without that feature, `cargo bench` runs nothing and reports no error, because the target declares `required-features`. Run the alias instead. It adds the feature and builds the standard library for unwinding:

```powershell
cargo bench-hot-path
```

The bench binary uses `panic = "unwind"`, so it is close to, but not identical to, the shipped build. Reports go to `target\criterion\`. Host speed drifts by several percent between sessions, so judge a change with an A/B run in one session, as described in [the baseline record](datasets/baselines/code-review-2026-11-01/README.md#comparing-a-change-with-the-baseline):

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

A Current document can name the commit it was checked against with **Describes** in place of **Commit**, and it can keep **Evidence dated**:

```markdown
> **Status:** Current · **Describes:** `main` at commit `abc1234` · **Reviewed:** YYYY-MM-DD
```

For a Historical record, **Commit** is the commit the record describes. Write "not recorded" when the record does not identify one. The status values are defined in [Status values](docs/README.md#status-values).

One exception applies. [docs/performance/pooled-vm-performance-review.md](docs/performance/pooled-vm-performance-review.md) stays Current while it is the latest completed native performance record. Its status line says that its campaign binaries are historical. When a later native campaign completes, mark that review Historical and update this exception.

The status decides the folder under `docs/`:

| Folder | Put here |
| --- | --- |
| `docs/guide/` | Current operator guides |
| `docs/design/` | Current architecture and reference documents. Editable diagrams go in `docs/design/diagrams/`. |
| `docs/performance/` | The current performance record, and the records that the report generator links beside it |
| `docs/plans/` | Proposed plans that are not executed. Move a plan to `docs/history/` when it is executed. |
| `docs/history/` | Historical and Superseded records: finished campaigns, reviews, and completed plans |
| `docs/assets/performance/` | Generated graphs and data copies (the default output of `plot-pooled-wpr.py`) |
| `docs/assets/pipeline/` | The data-path animation. Build it with `scripts/docs/pipeline-animation/Build-Animation.ps1`. |
| `docs/assets/loss-solutioning/` | Historical animations of the event-loss causes and fixes |

When a status changes, move the document with `git mv`, update every relative link to it, and update its row in [docs/README.md](docs/README.md). In a Historical record, do not rewrite commands or paths when a script or source file moves. Add a note that names the new path.

Rules for documentation changes:

- Do not edit numeric evidence in a Historical record. Add a clarifying note instead.
- Define an abbreviation at its first use, or link to its definition in [docs/guide/configuration.md](docs/guide/configuration.md) or [docs/design/measurement-terms.md](docs/design/measurement-terms.md).
- Link to source symbols (function, type, or constant names), not line numbers. Line numbers drift.
- Relative links must resolve, and anchors must match a heading. A link into the gitignored `target/` directory must say that it is a local artifact.
- Do not paste receiver addresses, query names, or capture data.

### Document types

Each Current document has one job. Put new text in the document whose job it matches.

| Type | Job | Documents |
| --- | --- | --- |
| Procedure | Numbered steps for an operator, with a check after each step that can fail | [build.md](docs/guide/build.md), [install.md](docs/guide/install.md), [receiver.md](docs/guide/receiver.md), [upgrade.md](docs/guide/upgrade.md), [uninstall.md](docs/guide/uninstall.md), [service-stop-remediation.md](docs/guide/service-stop-remediation.md) |
| Reference | Facts to look up: options, defaults, ranges, tuned settings, counters, log events | [configuration.md](docs/guide/configuration.md), [tuning.md](docs/guide/tuning.md), [monitoring.md](docs/guide/monitoring.md), [measurement-terms.md](docs/design/measurement-terms.md) |
| Diagnosis | Starts from a symptom, and gives the cause and the corrective action | [troubleshooting.md](docs/guide/troubleshooting.md) |
| Design | How a part of the code works, and why | [data-path.md](docs/design/data-path.md), [pooled-intake.md](docs/design/pooled-intake.md), [ring-buffer.md](docs/design/ring-buffer.md), [single-pass-decoder.md](docs/design/single-pass-decoder.md) |
| Record | Results of one campaign, review, or plan | `docs/performance/`, `docs/history/` |

A procedure links to the reference for option details. It does not copy them.

### Writing style

Current documents use a plain style based on Simplified Technical English (ASD-STE100). The [`asd-ste100` skill](.agents/skills/asd-ste100/SKILL.md) has the full rules. Use its *Strict* mode for procedure steps, error and log messages, and script output. Use its *STE-flavored* mode for other prose.

- Write one instruction in each procedure sentence. Put the verb first.
- Keep a procedure sentence to 20 words or fewer, and a description sentence to 25 or fewer. A long command or path does not count.
- Do not use semicolons. Write two sentences, or use a list.
- Use a numbered list for three or more steps, and a bulleted list for three or more conditions.
- Use the active voice when the actor is known.
- Keep each hedge ("may", "probably") that the evidence needs. Do not turn a lab observation into a general claim.
- Use one word for one action. Use **check** for an operator action that examines a result. Keep **validate** and **validation** for a lab validation run and its record.

Example: write "Run `cargo test --locked`. Check that it reports 0 failed." Do not write "Running the tests, which should be checked for failures; the output then reviewed."

Run the structural linter on each Current document that you change:

```powershell
python -I .agents\skills\asd-ste100\scripts\ste-lint.py README.md docs\guide\install.md
```

Passive-voice and tense findings from the linter are advisory. Do not apply this style to Historical records.

## Commits and pull requests

Use short imperative subjects in sentence case, such as `Add single-pass decoding, ETW flush control, and low-rate defaults`. Keep the subject under about 72 characters, and describe the behavior change in the body. Do not commit build output, captures, ETL files, or anything under `target/`.

A pull request should state the behavior that changed and any default that changed. It should list the commands you ran and their results. Say whether it affects `[Unreleased]` in [CHANGELOG.md](CHANGELOG.md), and if it does, add an entry there. Unit tests do not cover live ETW and DNS Server behavior. Say so if your change needs a live check.

## Scope

Lab scripts under `scripts/` write to real VMs and can change DNS zones and disks. Changes there need a dry-run path and a review from someone who can run them against the lab.
