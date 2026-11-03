# No console output plan

> **Status:** Executed. Merged to `main` in `ef00c76` · **Evidence dated:** 2026-11-03 · **Commit:** written at `1be4f3e`, executed through `985bcd5` · **Reviewed:** 2026-11-03

This plan removes every write to stdout and stderr from `etw-dnstap.exe`. After
it, the Application event log (source `EtwDnstap`), the performance counters,
the process exit code, and the SCM service status are the only ways the service
reports. A console user sees no text.

## Decisions

The maintainer made these decisions on 2026-11-03:

1. **Remove `--help` and `--version`.** clap's help and version flags are
   disabled. `--help`, `-h`, `--version`, and `-V` become unknown arguments and
   are reported like every other argument error.
2. **A console launch exits with code 1 and records nothing.** This covers bad
   arguments and good arguments alike, because a console launch cannot connect
   to SCM. Under SCM, an argument error is still Event ID 1 and an SCM failure
   status, as today.
3. **Lab check after merge.** The release build is checked on the
   DNSTap-WS2025 lab guest after the branch merges.

## Writers at `1be4f3e`

| # | Location | Writes | Stream |
| --- | --- | --- | --- |
| W1 | `src/main.rs`, `main` | `error: {error:#}` when `run()` fails | stderr |
| W2 | `src/pipeline/supervisor.rs`, `run_capture` | every tracing line, through `std::io::stderr.and(EventLogWriter)` | stderr |
| W3 | `src/lib.rs`, `run` | clap help and version text, through `error.print()` | stdout |
| W4 | Rust default panic hook | the panic message and location. The release profile has `panic = "abort"`, but the hook runs before the abort | stderr |

rustls logs through the `log` crate. tracing-subscriber's default `tracing-log`
feature forwards those records to the tracing subscriber, so W2's fix covers
them.

Out of scope, because they are not part of the service binary's runtime:

- `println!` and `eprintln!` in `*_tests.rs` files. The test harness captures
  them.
- `build.rs`. Its `println!` lines are Cargo directives.
- The `fuzz/` crate, `benches/`, `tools/dns-stress`, and `scripts/`.

## Changes

### Code (C)

- **C1. Tracing goes only to the event log.** `run_capture` uses
  `.with_writer(crate::service::EventLogWriter)`. Remove the `MakeWriterExt`
  import.
- **C2. `main` prints nothing.** `main` maps `Err` to
  `ExitCode::FAILURE` and drops the text. Under SCM, `service_main` has already
  written the failure to the event log as Event ID 1 (`report_failure`). The
  errors that `service::run` returns before the dispatcher runs are internal
  state errors. The dispatcher failure is the console case, which decision 2
  covers.
- **C3. No help or version flags.** In `src/pipeline/config.rs`, remove
  `version` from `#[command(...)]` and add `disable_help_flag = true` and
  `disable_version_flag = true`. In `src/lib.rs`, remove `is_informational` and
  the `error.print()` branch. Every clap error goes to `service::run(Err(...))`.
  Update the doc comments on `run` and `parse_args`. Keep the argument doc
  comments; they document the options in the source.
- **C4. Panic hook writes to the event log.** `run()` installs a panic hook with
  `std::panic::set_hook` before it parses arguments. The hook writes one Error
  event, **Event ID 6**, to source `EtwDnstap`. The text starts with
  `EtwDnstap panicked:` and contains the panic message and location. The
  release build's `trim-paths` keeps the location free of host paths. The hook
  does not use tracing, so a panic inside the subscriber cannot recurse. Put the
  text formatting in a pure function that takes the message and an optional
  location, so it can be unit tested without a panic. Only `run()` installs the
  hook; unit tests keep the default hook.
- **C5. The version moves to `capture started`.** Without `--version`, the
  `capture started` event gets a `version` field set from
  `env!("CARGO_PKG_VERSION")`. This is the operator's way to read the running
  version.
- **C6. Lint guard.** Add `print_stdout = "deny"`, `print_stderr = "deny"`, and
  `dbg_macro = "deny"` to `[lints.clippy]` in `Cargo.toml`. Add a `clippy.toml`
  with `allow-print-in-tests = true` if test code needs it. `build.rs` gets a
  file-level `#![allow(clippy::print_stdout, clippy::print_stderr, reason = "...")]`
  because Cargo reads its directives from stdout. Fix or allow, with a reason,
  any other hit outside the service runtime (`benches/`, `#[cfg(test)]`
  helpers).

### Tests (T)

- **T1. Binary integration test.** A new `tests/console_output.rs` runs
  `env!("CARGO_BIN_EXE_etw-dnstap")` with each of these argument sets and
  asserts that stdout and stderr are empty and the exit code is 1:
  no arguments; `--help`; `-h`; `--version`; `-V`; an invalid value such as
  `--receiver not-an-address`; and valid arguments such as
  `--receiver 127.0.0.1:6000 --plaintext`. The last case reaches
  `StartServiceCtrlDispatcherW`, which fails at once from a console. Each child
  is killed after 30 seconds so that a regression cannot hang the test run.
- **T2. CLI unit tests.** In `src/pipeline/cli_tests.rs`, replace
  `help_and_version_win_when_they_come_first` with a test that each of the four
  flags is `ErrorKind::UnknownArgument`, alone and after other arguments.
- **T3. Panic text.** Unit tests for the C4 formatter: message with a location,
  message without a location, an empty message, and a message longer than the
  event log line limit (`MAX_LINE_BYTES` in `src/service.rs`).
- **T4. `capture started` version.** Extend the existing supervisor log test
  that captures tracing lines so that it checks the `version` field.

### Documentation (D)

Update the current guides. Leave `docs/history/` unchanged; it records what was
true when it was written.

- **D1. `README.md`.** The "Has no foreground mode" bullet: no `--help` or
  `--version`; a console launch exits with code 1 and prints nothing. The
  diagnostics bullet: drop stderr. Replace "Run it with `--help`" with a link to
  [configuration.md](../guide/configuration.md).
- **D2. `docs/guide/configuration.md`.** Remove the `--help` and `--version`
  rows. Say that the binary has no help output and that this page is the option
  reference.
- **D3. `docs/guide/build.md`.** Replace the "Check the binary" step. One way:
  start the service and read the `version` field of `capture started`.
- **D4. `docs/guide/monitoring.md`.** Drop stderr from the opening paragraph.
  Add an Event ID 6 row (Error, panic, text starts with `EtwDnstap panicked:`).
  Add `version` to the `capture started` field list.
- **D5. `docs/guide/troubleshooting.md`.** Remove the stderr bullet from "Read
  the diagnostics" and add Event ID 6 to the Application log bullet. Add a short
  item: a console launch, or any argument from a console, exits with code 1 and
  prints nothing; run the binary through SCM and read Event ID 1.
- **D6. `CHANGELOG.md`.** An `[Unreleased]` entry that starts with
  **Breaking.** for the removed flags and output, plus entries for Event ID 6
  and the `version` field. In the versioning policy, the version is read from
  `capture started`, not `--version`. In the contract list, remove the
  `--help` and `--version` output.
- **D7. Status headers.** Update the `Describes` and `Reviewed` fields of every
  guide that changes.

## Execution

| Step | Owner | Branch | Depends on |
| --- | --- | --- | --- |
| 1. This plan | orchestrator | `plan/no-console-output` | none |
| 2. C1 to C6, and the existing tests they break | Rust agent | `no-console-output/code` | 1 |
| 3. D1 to D7 | documentation agent | `no-console-output/docs` | 1 |
| 4. T1 to T4 | test agent | `no-console-output/tests`, cut from step 2 | 2 |
| 5. Merge, review, gates, PE checks | orchestrator | `plan/no-console-output` | 3, 4 |
| 6. Lab check | orchestrator | `plan/no-console-output` | 5 |

Steps 2 and 3 run at the same time. Each agent works in its own worktree and
commits to its own branch. The orchestrator merges.

## Gates

Every step that changes code passes these before it is handed back:

```powershell
cargo fmt --check
cargo test --locked
cargo test --locked --features diag-timing
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features diag-timing -- -D warnings
cargo clippy --locked --all-targets --features bench -- -D warnings
```

Step 5 also runs `scripts/ci/verify-pe.ps1` and
`scripts/ci/verify-link-exceptions.ps1` on a release build, and the
`CLAUDE.md` host-path check on the staged changes.

## Lab check (step 6)

On DNSTap-WS2025, with the DNS-collector output on the receiver sent to null:

1. Deploy the release build and start `EtwDnstap` through SCM. The
   `capture started` event has `version` equal to `Cargo.toml`'s version.
2. Stop the service. It reaches `STOPPED` with exit code 0, and `capture
   stopped` and `output stopped` are in the event log.
3. From a console in the guest, run the binary with no arguments, `--help`,
   `--version`, a bad `--receiver`, and valid arguments. Each prints nothing and
   exits with code 1.
4. Set an invalid argument in the service's `ImagePath`, start the service, and
   confirm Event ID 1 with the clap text and an SCM failure status. Restore the
   `ImagePath`.

## Risks

- **An operator loses the version check.** `capture started` carries it now.
  An offline check of an unstarted binary is not possible. A `VERSIONINFO`
  resource would give one, but it is not in this plan.
- **A console user gets no hint.** This is decision 2. The troubleshooting
  guide says to use SCM and the event log.
- **A panic before the event source opens.** The hook opens the source itself
  through the existing lazy `event_source()`. If registration fails, the panic
  text is lost and the process still aborts, as it does for any log line.

## Results

| Step | Commit | Result |
| --- | --- | --- |
| 1. Plan | `fbd34d2` | Written. |
| 2. Code, C1 to C6 | `0eb9cde` | As planned. The panic text formatter takes `Option<(&str, u32, u32)>` in place of `Option<&Location>`, so tests can give exact values. `src/pipeline/config_tests.rs` also expected `--help` to display help and was updated. `clippy.toml` sets `allow-print-in-tests = true` for the existing test `println!` calls. `benches/hot_path.rs` has one scoped `print_stderr` allow. |
| 3. Documentation, D1 to D7 | `ec438e2`, merged in `c65951d` | As planned. The troubleshooting guide also gets a "A panic aborts the process" section. |
| 4. Tests, T1 to T4 | `179be84`, merged in `985bcd5` | As planned. T1 was shown to fail with an `eprintln!` added to `main`. |
| 5. Gates | `985bcd5` | All six gates pass: 417 + 1 tests (default) and 422 + 1 tests (`diag-timing`) passed, 7 ignored in each. `verify-link-exceptions.ps1`: 6 checks, 0 failed. `verify-pe.ps1`: 21 checks, 0 failed. |
| 6. Lab check | `985bcd5` | Passed. See below. |

### Lab check

The release build is from the source of `0eb9cde`; the later commits change
only tests and documentation. Guest checkpoint
`no-console-output-pre-2026-11-03` was taken first. The receiver routed to its
`devnull` logger, with its stdout and stderr sent to null. The service was
`STOPPED` at the start and at the end, with no leftover ETW session.

1. The new executable replaced the installed one (the guest keeps the old one as
   `etw-dnstap.exe.nco-bak`). `NT SERVICE\EtwDnstap:(I)(RX)` was kept. The
   service reached `RUNNING`. `capture started` had `version="0.1.0"`.
2. `sc.exe stop` reached `STOPPED` with exit code 0. `capture stopped` and
   `output stopped` were written.
3. Run from a console in the guest with no arguments, `--help`, `-h`,
   `--version`, `-V`, `--receiver bad`, and valid arguments: each exited with
   code 1 with 0 bytes on stdout and 0 bytes on stderr.
4. With `--help` added to `ImagePath`, the start failed with Event ID 1:
   `EtwDnstap failed: invalid service arguments: unexpected argument '--help'
   found`, followed by the usage line. The guest's SCM recovery actions
   (restart after 5 s, 5 s, and 60 s, with failure actions on non-crash
   failures) restarted it. The first restart hit the same Event ID 1. The
   `ImagePath` was then restored, and the 60 s restart ran normally. The
   service was stopped again to leave the guest as found.
