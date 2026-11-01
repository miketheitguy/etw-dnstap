# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it is tagged.

Dates are commit dates in America/Los_Angeles, reconstructed from `git log`. The
repository has no tags yet, so `0.1.0` lists the commits on `main` in order.
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
- Tests, fuzzing and CI: _pending_

### Changed

- Service identity: the service runs as the virtual account
  `NT SERVICE\EtwDnstap` instead of LocalSystem. It must be an enabled member of
  Performance Log Users and hold only `SeChangeNotifyPrivilege`. Existing
  installs must be reprovisioned. Commit `f0b51f4`.
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

### Fixed

- _pending_

### Security

- DNS process filter: an ETW `EVENT_FILTER_TYPE_PID` filter admits only the DNS
  Server process, and the callback checks the process ID again. Records from
  other processes count as `foreign_pid_rejected`. Commit `f0b51f4`.
- The service no longer runs as LocalSystem. It runs as the virtual account with
  only `SeChangeNotifyPrivilege`. Commit `f0b51f4`. This profile has not been
  validated on a live DNS Server.

## [0.1.0] - not tagged

Commits on `main`, oldest first. Entries reflect commit subjects. Per-commit
contents were not reviewed for this file.

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

### Removed

- The `--output`/`-o`, `--max-output-bytes`, and `--metrics` options and the
  service JSONL metrics writer. The removing commit is not identified in this
  file. Removed arguments now fail command-line parsing. See the
  [instrumentation compatibility note](docs/README.md#instrumentation-compatibility-note).
