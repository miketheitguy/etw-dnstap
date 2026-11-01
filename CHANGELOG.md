# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) and the policy below.

## Versioning policy

The version is the `version` field in [Cargo.toml](Cargo.toml). The binary
prints it with `--version`. The repository has no Git tag and no release yet.
Every change stays under `[Unreleased]` until a tag is made.

**SemVer rule.** A major version of `0` means initial development. Under
[SemVer 2.0.0](https://semver.org/spec/v2.0.0.html), anything may change at any
time, and the public API should not be treated as stable.

**Local policy while the version is `0.y.z`.** This policy states what the
project promises. It is not a SemVer rule. The public contract has these parts:

- The command-line options, their defaults and ranges, and the `--help` and
  `--version` output.
- The Application event log messages and counters, and the dnstap output sent
  to the receiver.
- The ETW session name, and the session GUID derived from it.
- The provisioning steps in [install.md](docs/guide/install.md): identity,
  privileges, ETW grant, and event source.

Under this policy:

- A change that breaks the contract, or that makes an operator change the
  configuration or reprovision the service, increases the minor number `y`. It
  sets the patch number `z` to `0`. Its entry starts with **Breaking.**
- Any other change increases the patch number `z`. This includes fixes,
  documentation, new options that keep the old behavior, and new counters or
  log events.
- A published version is never replaced. A faulty release gets a new version.
- The project moves to `1.0.0` when the maintainer decides that the contract is
  stable and that lab validation covers the Windows builds operators use.

Dates are commit dates in America/Los_Angeles, reconstructed from `git log`.
Each entry is summarized from its commit subject. See the commit for details.

## [Unreleased]

Entries are summarized from commit subjects and from the source on `main`.
Per-commit diffs were not reviewed for this section. Do not remove the headings.

### Added

- Receiver reconnect with jittered backoff (250 ms to 30 s). Each new session
  repeats the Frame Streams handshake. Frames are never resent. Commit `f0b51f4`.
- Periodic counter logs, `capture stats` and `output stats`, every
  `--stats-interval-seconds` (default 60; 0 disables them), and final
  `capture stopped` and `output stopped` reports. Commit `f0b51f4`.
- `disconnected_dropped` counter for frames lost while the receiver is down.
- Low-rate warning when the received rate exceeds about 1,500 events/s under the
  default ETW settings. Commit `f0b51f4`.
- Watchdog Event ID 5, written before an abort. The startup deadline is 10 s plus
  1 s per 16,384 channel slots.
- Single-instance guard: a named mutex, `Global\EtwDnstap-<session>`.
- Negative schema cache: an event whose schema fails to load is rejected without
  repeated TDH calls, and counted in `schema_rejected`.
- Tests, fuzzing, and CI. Commit `12cce7a` added them. Property tests
  (`proptest`) check Frame Streams framing, event decoding, and the dnstap
  encoder. Golden byte vectors compare the encoder with the upstream
  `dnstap.proto`. Three libFuzzer targets (`decoder`, `control_frame`, and
  `plan_compiler`) are in the separate `fuzz/` workspace. The CI workflow runs
  formatting, Clippy, tests, a release build, `cargo deny`, the Python tests,
  and a link check. The 60-second fuzz smoke run and coverage are report-only.
  The `lab.yml` workflow runs the ignored tests that need live ETW, on a
  self-hosted runner.

### Changed

- Service identity: the service runs as `NT AUTHORITY\LocalService` with a
  restricted service SID type (`sc.exe sidtype EtwDnstap restricted`, a
  write-restricted token) instead of LocalSystem. The token must be restricted with
  the service SID and `S-1-5-33` as restricting SIDs, the service SID must be an
  enabled group and a member of Performance Log Users, and only
  `SeChangeNotifyPrivilege` may be held. `scripts/provision/Grant-EtwDnstapEtwAccess.ps1`
  grants the ETW right to the service SID. Existing installs must be
  reprovisioned. Validated on one lab DNS Server (Windows Server 2025, build
  26100) on 2026-11-01. See the
  [validation record](docs/history/localservice-restricted-validation.md).
- ETW and instance-lock access-denied errors name the missing right, the GUID, and
  the provisioning script.
- The ETW session now has a stable GUID derived from `--session-name`
  (SHA-1, RFC 9562 version 5; `{05e1db3c-1322-58c7-8235-0463fccba32b}` for the default name), logged as
  `session_guid` in `capture started`. The grant script grants only
  `TRACELOG_CREATE_REALTIME` (0x20), on that session GUID, and no longer edits
  the shared default descriptor. It no longer grants `0x423` on the default, or
  `0x80` on the DNS provider GUID. Every run strips the service SID from every
  other ETW descriptor, which removes the first version's grants and the copies
  that other components made of the default. A lab run on build 26100 showed
  that this is the only right the restricted token needs, and that ETW applies
  descriptor changes at boot. See
  [the validation record](docs/history/localservice-restricted-validation.md).
- Connection loss no longer ends capture. A failed session is reconnected, and
  events are dropped and counted until it is. A receiver that is unreachable at
  startup no longer stops the service; only a protocol failure at startup is
  fatal. Commit `f0b51f4`.
- The service exits for SCM restart when the DNS Server process ID changes or the
  DNS Server stops, checked every 5 s. Commit `f0b51f4`.
- Session reclaim no longer requires a matching buffer size. A mismatch logs a
  warning, and the session is reclaimed.
- Shutdown waits up to 5 s for `ProcessTrace` to return before `CloseTrace`. The
  socket send buffer is requested at 1 MiB. An idle parser flushes after 100 ms.
  The parser wakes after each successful periodic ETW flush.
- ETW code is split into `src/etw/` (`session`, `keywords`, `intake`, `schema`,
  `target`, `instance`, `diag`), with `src/event.rs` and `src/sink.rs` as seams
  for the output sink. Commit `dc9d7e0`.
- Lab performance scripts are hardened and consolidated under `scripts/`. Commit
  `6e62b2e`. This commit does not change runtime behavior.
- Documentation is reorganized into `docs/guide/`, `docs/design/`,
  `docs/performance/`, `docs/plans/`, and `docs/history/` by status. Commit
  `54eafae`. See the [documentation index](docs/README.md#layout).

### Removed

- The `--output`/`-o`, `--max-output-bytes`, and `--metrics` options and the
  service JSONL metrics writer. Arguments that use them now fail command-line
  parsing. Remove them from the binary path before you start this build. See
  [install.md](docs/guide/install.md#repair-or-upgrade). The removing commit is
  not identified in this file. See the
  [instrumentation compatibility note](docs/README.md#instrumentation-compatibility-note).

### Fixed

- The identity check reported `truncated token groups` for an unrestricted
  token instead of `the process token is not restricted`. An unrestricted token
  returns `TokenRestrictedSids` as a bare zero count.
- The documented `sc.exe config ... password= ""` did nothing in Windows
  PowerShell 5.1, which drops the empty argument. The docs and the identity
  error now run it through `cmd.exe /c`.

### Security

- DNS process filter: an ETW `EVENT_FILTER_TYPE_PID` filter admits only the DNS
  Server process, and the callback checks the process ID again. Records from
  other processes count as `foreign_pid_rejected`. Commit `f0b51f4`.
- The service no longer runs as LocalSystem. It runs as LocalService with a
  restricted service SID and only `SeChangeNotifyPrivilege`. The profile was
  validated on one lab DNS Server (Windows Server 2025, build 26100) on
  2026-11-01. A DNS Server restart, SCM recovery, load, and other Windows builds
  were not tested.

## Commit log through `9172f2d`

This is not a release. It lists every commit on `main` from the first commit
through `9172f2d`, from `git log`, grouped by category. Each entry is summarized
from its commit subject. Per-commit contents were not reviewed for this file.
The removal of the CLI options is recorded under `[Unreleased]`.

### Added

- `08254d4` (2026-10-29), Initial Buildout.
- `2c62a9f` (2026-10-29), Document pooled ETW performance results and add WPR
  graphs. Adds the pooled-intake performance records and generated graphs.
- `e59425f` (2026-10-31), Add single-pass decoding, ETW flush control, and
  low-rate defaults. Adds the `--etw-flush-ms` option and single-pass decoding.
- `9172f2d` (2026-11-01), Clarify low-rate default warnings and high-rate
  guidance. Documentation only.

### Changed

- `3d401cc` (2026-10-29), Optimize DNS ETW intake and restrict capture to client
  events. The service accepts only DNS Server event IDs 256 (query received) and
  257 (response success).
- `5fa2152` (2026-10-29), Preserve capture filter metadata in receiver end
  markers.

