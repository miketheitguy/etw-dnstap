# Contributing

Thank you for helping. This project is a Windows-only Rust service. Read the
[security policy](SECURITY.md) before reporting a problem that involves data
exposure or privilege.

## Build and check

Set up the toolchain first. The repository pins it in
[rust-toolchain.toml](rust-toolchain.toml). See the
[prerequisites](docs/guide/install.md#prerequisites) for Visual Studio and
[Install the Rust toolchain](docs/guide/install.md#install-the-rust-toolchain)
for the rustup steps.

Run these from the repository root before opening a pull request:

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

Notes:

- `--locked` is required. The build uses the committed `Cargo.lock`.
- `Cargo.toml` denies all rustc warnings (`warnings = "deny"`) and the Clippy
  lints `unwrap_used` and `undocumented_unsafe_blocks`. Clippy `pedantic` lints
  are warnings, and CI runs Clippy with `-D warnings`, so they also fail CI. Do
  not add `#[allow]` to hide a warning without a comment that explains why.
- `unsafe` code is denied by default. Each `unsafe` block needs an
  `// SAFETY:` comment, which Clippy enforces (`undocumented_unsafe_blocks`).
- The first build rebuilds the standard library (`build-std`). Expect it to take
  noticeably longer than an ordinary Rust build.
- `cargo test --locked` reports 226 passed, 0 failed, and 7 ignored.
  `cargo test --locked --features diag-timing` reports 231 passed and 7
  ignored. Five ignored tests need an elevated session or the DNS Server role.
  The other two are manual benchmarks (`synthetic_pooled_intake_benchmark` and
  `parse_encode_benchmark`). The manual/scheduled `lab.yml` workflow runs all
  ignored tests on a self-hosted `etw-lab` runner. Do not un-ignore them in a
  pull request.
- Continuous integration (`.github/workflows/ci.yml`, `windows-latest`) runs
  these checks:
  - `cargo fmt --check`.
  - Clippy with `-D warnings`, for the default, `diag-timing`, and `fuzzing`
    feature sets.
  - Tests, for the default and `diag-timing` feature sets.
  - A release build.
  - `scripts/ci/verify-pe.ps1` on the release executable. It checks the
    hardening flags, the absence of a PDB entry, and the absence of ASan and
    dynamic CRT imports.
  - The `tools/dns-stress` checks (format, Clippy, and tests).
  - `cargo fmt --check` in `fuzz/`.
  - The Python tests in `scripts/performance`, including the archived
    analyzer tests in `scripts/performance/archive/`.
  - `cargo deny` on the root, `fuzz/`, and `tools/dns-stress/` lockfiles.
  - The markdown link check (`python -I scripts/ci/linkcheck.py`).
- An advisory `sanitizers` job runs `cargo test-release` and `cargo asan`. It
  does not block a merge until it passes on hosted runners.
- Coverage and a 60-second fuzz smoke run (`fuzz/`) are report-only. They do
  not block a merge. Local `cargo fuzz run` currently fails at build on MSVC
  with an LLVM COMDAT error, which is why the fuzz smoke run is advisory.
- Property tests use `proptest`. Golden byte vectors in
  `src/dnstap/golden_tests.rs` compare the encoder with the upstream
  `dnstap.proto`. The fuzz targets (`decoder`, `control_frame`, `plan_compiler`)
  live in the separate `fuzz/` workspace. They build with the `fuzzing` feature,
  which exposes internal entry points. That feature is never enabled in a
  shipped build. Install the runner with `cargo install cargo-fuzz --version 0.13.2 --locked`.
  CI pins cargo-fuzz 0.13.2, cargo-deny 0.20.2 and cargo-llvm-cov 0.9.1 by hand in
  `.github/workflows/ci.yml`. Dependabot does not update these pins.
- `cargo test-release` and `cargo asan` are alias commands in `.cargo/config.toml`.
  `cargo asan` writes its build output to `target\asan`. The ASan run needs the
  Visual Studio AddressSanitizer runtime. See the
  [install guide](docs/guide/install.md#optional-address-sanitizer-builds).

The `tools/dns-stress` load generator is a separate Cargo project. Build and
test it from its own directory:

```powershell
cd tools\dns-stress
cargo test --locked
```

## Documentation

The documentation index is [docs/README.md](docs/README.md). Every document in
`docs/` starts with a status line directly under its title:

```markdown
> **Status:** Current | Historical | Superseded | Proposed · **Evidence dated:** ... · **Commit:** ... · **Reviewed:** YYYY-MM-DD
```

A Current document can name the commit it was checked against with
**Describes** in place of **Commit**. It can keep **Evidence dated**:

```markdown
> **Status:** Current · **Describes:** `main` at commit `abc1234` · **Reviewed:** YYYY-MM-DD
```

For a Historical record, **Commit** is the commit the record describes. Write
"not recorded" when the record does not identify one.

Use the status values as follows.

| Status | Meaning |
| --- | --- |
| Current | Describes the code and procedures on `main`. Update it with the code. |
| Historical | A dated campaign, review, or plan. Its numbers and hashes apply to its recorded binaries. Do not edit its results. |
| Superseded | Replaced by a newer document or implementation. Link to the replacement. |
| Proposed | A design or plan that is not implemented or not executed. |

One exception applies. [docs/performance/pooled-vm-performance-review.md](docs/performance/pooled-vm-performance-review.md) stays Current while it is the latest completed native performance record. Its campaign binaries are historical, and its status line says so. When a later native campaign completes, mark that review Historical and update this exception.

The status decides the folder under `docs/`:

| Folder | Put here |
| --- | --- |
| `docs/guide/` | Current operator guides (install, configuration, troubleshooting, remediation, uninstall) |
| `docs/design/` | Current architecture and reference documents. Editable diagrams go in `docs/design/diagrams/`. |
| `docs/performance/` | The current performance record and the records the report generator links beside it |
| `docs/plans/` | Proposed plans and analyses that are not executed or implemented |
| `docs/history/` | Historical and Superseded records: finished campaigns, reviews, and completed plans |
| `docs/assets/performance/` | Generated graphs and data copies (the default output of `plot-pooled-wpr.py`) |

When a document's status changes, move it with `git mv`, update every relative
link to it, and update its row in [docs/README.md](docs/README.md). Do not rewrite
the commands or paths in a Historical record when a script or source file moves.
Add a short note that names the new path.

Rules for documentation changes:

- Do not edit numeric evidence in a Historical record. Add a clarifying note
  instead.
- Define an abbreviation at its first use, or link to its definition in
  [docs/guide/configuration.md](docs/guide/configuration.md) or
  [docs/design/measurement-terms.md](docs/design/measurement-terms.md).
- Link to symbols (function, type, or constant names), not to line numbers, when
  you cite source code. Line numbers drift.
- Relative links must resolve. Anchors must match a heading that exists. A link
  into the gitignored `target/` directory must say that it is a local artifact.
- Do not paste receiver addresses, query names, or capture data.

### Document types

Each Current document has one job. Put new text in the document whose job it
matches.

| Type | Job | Documents |
| --- | --- | --- |
| Procedure | Tells an operator what to do, in numbered steps, with a check after each step that can fail | [install.md](docs/guide/install.md), [uninstall.md](docs/guide/uninstall.md), [service-stop-remediation.md](docs/guide/service-stop-remediation.md) |
| Reference | Lists facts to look up: options, defaults, ranges, counters, log events | [configuration.md](docs/guide/configuration.md), [measurement-terms.md](docs/design/measurement-terms.md) |
| Diagnosis | Starts from a symptom and gives the cause and the corrective action | [troubleshooting.md](docs/guide/troubleshooting.md) |
| Design | Tells a maintainer how a part of the code works and why | [pooled-intake.md](docs/design/pooled-intake.md), [ring-buffer.md](docs/design/ring-buffer.md), [single-pass-decoder.md](docs/design/single-pass-decoder.md) |
| Record | Keeps the results of one campaign, review, or plan | `docs/performance/`, `docs/history/` |

A procedure links to the reference for option details. It does not copy them.

### Writing style

Current documents use a plain style based on Simplified Technical English
(ASD-STE100). The [`asd-ste100` skill](.agents/skills/asd-ste100/SKILL.md) has
the full rules. Use its *Strict* mode for procedure steps, error and log
messages, and script output. Use its *STE-flavored* mode for other prose.

- Write one instruction in each procedure sentence. Write the verb first.
- Keep a sentence to 20 words or fewer in a procedure step, and to 25 words or
  fewer in a description. A long command or path does not count against the
  limit.
- Do not use semicolons. Write two sentences, or use a list.
- Use a numbered list for three or more steps. Use a bulleted list for three or
  more conditions.
- Use the active voice when the actor is known.
- Keep each hedge ("may", "probably") that the evidence needs. Do not change a
  lab observation into a general claim.
- Use one word for one action. Use **check** for an operator action that
  examines a result. Keep **validate** and **validation** for a lab validation
  run and its record.

Run the structural linter on each Current document that you change:

```powershell
python -I .agents\skills\asd-ste100\scripts\ste-lint.py README.md docs\guide\install.md
```

The linter reports semicolons, long sentences, phrasal verbs, nominalizations,
and synonym rotation. Passive-voice and tense findings are advisory. A synonym
finding is a prompt to check the word, not a rule. Do not apply the style to
Historical records. Their text is part of the record.

## Commits and pull requests

The history uses short imperative subjects in sentence case, such as
`Add single-pass decoding, ETW flush control, and low-rate defaults`. Keep the
subject under about 72 characters and describe the behavior change in the body.
Do not commit build output, captures, ETL files, or anything under `target/`.

A pull request should state:

- The behavior that changed, and the configuration defaults that changed, if any.
- The commands you ran and their results.
- Whether the change affects the [CHANGELOG.md](CHANGELOG.md) `[Unreleased]`
  section. If it does, add an entry there.

Live ETW and DNS Server behavior is not covered by unit tests. Say so in the
pull request if your change needs a live check.

## Scope

Lab scripts under `scripts/` write to real VMs and can change DNS zones and
disks. Changes there need a dry-run path and a review from someone who can run
them against the lab.
