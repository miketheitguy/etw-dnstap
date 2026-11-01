# Contributing

Thank you for helping. This project is a Windows-only Rust service. Read the
[security policy](SECURITY.md) before reporting a problem that involves data
exposure or privilege.

## Build and check

Set up the toolchain first. The repository pins it in
[rust-toolchain.toml](rust-toolchain.toml). See
[docs/guide/install.md](docs/guide/install.md#build-the-executable) for the
Visual Studio and rustup steps.

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
- `Cargo.toml` denies all rustc warnings (`warnings = "deny"`) and Clippy
  correctness lints. Do not add `#[allow]` to hide a warning without a comment
  that explains why.
- `unsafe` code is denied by default. Each `unsafe` block needs an
  `// SAFETY:` comment, which Clippy enforces (`undocumented_unsafe_blocks`).
- The first build rebuilds the standard library (`build-std`). Expect it to take
  noticeably longer than an ordinary Rust build.
- At commit `9172f2d`, `cargo test --locked` reported 124 passed, 0 failed, and
  6 ignored. The six ignored tests need live ETW or the DNS Server role. Do not un-ignore them in a
  pull request unless you run them on a suitable host.
- `cargo test-release` and `cargo asan` are alias commands in `.cargo/config.toml`.
  The ASan run needs the Visual Studio AddressSanitizer runtime; see the
  [install guide](docs/guide/install.md#optional-address-sanitizer-builds).

The `tools/dns-stress` load generator is a separate Cargo project. Build and
test it from its own directory:

```powershell
cd tools\dns-stress
cargo test --locked
```

## Documentation

The documentation index is [docs/README.md](docs/README.md). Every document
starts with a status line directly under its title:

```markdown
> **Status:** Current | Historical | Superseded | Proposed · **Evidence dated:** ... · **Commit:** ... · **Reviewed:** YYYY-MM-DD
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

Rules for documentation changes:

- Do not edit numeric evidence in a Historical record. Add a clarifying note
  instead.
- Define an abbreviation at its first use, or link to its definition in
  [docs/guide/configuration.md](docs/guide/configuration.md) or
  [docs/measurement-terms.md](docs/measurement-terms.md).
- Link to symbols (function, type, or constant names), not to line numbers, when
  you cite source code. Line numbers drift.
- Relative links must resolve. Anchors must match a heading that exists.
- Do not paste receiver addresses, query names, or capture data.

## Commits and pull requests

The history uses short imperative subjects in sentence case, such as
`Add single-pass decoding, ETW flush control, and low-rate defaults`. Keep the
subject under about 72 characters and describe the behavior change in the body.
Do not commit build output, captures, ETL files, or anything under `target/`.

A pull request should state:

- what behavior changed, and which configuration defaults (if any) changed;
- the commands you ran and their results;
- whether the change affects the [CHANGELOG.md](CHANGELOG.md) `[Unreleased]`
  section. Add an entry there.

Live ETW and DNS Server behavior is not covered by unit tests. Say so in the
pull request if your change needs a live check.

## Scope

Lab scripts under `scripts/` write to real VMs and can change DNS zones and
disks. Changes there need a dry-run path and a review from someone who can run
them against the lab.
