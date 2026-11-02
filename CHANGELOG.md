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

- CI captures the release link output and fails when the linker's LNK2047 object
  set (ring's prebuilt NASM objects, the `/FORCE:GUARDEHCONT` exception) differs
  from `scripts/ci/link-exceptions.txt` in either direction. The allowlist header
  gives the update procedure, and `.cargo/config.toml` records the ring decision
  and the withdrawal of RUSTSEC-2025-0007.
- An advisory CI `miri` job runs the decoder, Frame Streams, dnstap encoder, and
  keyword-array parser tests under Miri with Stacked Borrows on and asserts that
  at least 50 tests ran. The test-only counting allocator is compiled out under
  Miri because the Windows system allocator's alignment header read is outside
  the borrow it is given.
- A criterion benchmark of the per-event hot path (`benches/hot_path.rs`,
  `cargo bench-hot-path`) behind the new `bench` feature, which is never enabled in
  a shipped build. Groups: `copy_record` (warm and cold pool), `decode`, `encode`,
  `parse_and_encode` (IPv4, IPv6, and event 258), a two-thread producer and
  consumer rate, and a decomposition into retained parse plus encode and recycle.
  CI runs Clippy and the tests with the feature.
- `datasets/baselines/code-review-2026-11-01/`, the Phase 0 baseline of the code
  review remediation plan: binary size and hash, `verify-pe` output, criterion
  numbers as the median of three quiet sessions with a drift bound and an A/B
  procedure, the manual benchmark results, and the pool's private bytes.
- A lab check, `scripts/performance/windows-startup-capacity.ps1`, that starts the
  service at the maximum `--capacity` and confirms the SCM reports Running. It
  reconfigures the installed service temporarily and restores the binary path,
  executable hash, and SDDL in `finally`; `-WhatIf` runs the preflight only.
- **Breaking.** Failed responses are captured. DNS Server event 258
  (`RESPONSE_FAILURE`, for example SERVFAIL) is sent to the receiver as a
  `CLIENT_RESPONSE` message, with the packet in `response_message`. The packet was observed to be the
  response sent to the client for SERVFAIL (Reason `System`) and for a policy
  REFUSED (Reason `Policy`). A 258 whose packet is not a response is rejected
  and counted in `rejected`.
  `MatchAnyKeyword` now includes the `RESPONSE_FAILURE` bit, resolved by name
  with the other two. The keyword lookup falls back to the full channel as before if
  `QUERY_RECEIVED` or `RESPONSE_SUCCESS` is missing or malformed, or if any two
  of the three share a bit. If only `RESPONSE_FAILURE` is missing or
  malformed, the mask is narrowed to the other two, the service logs one
  warning, and `capture started` shows `failed_responses_captured=false`; that
  field is new. The subscription
  level changes from 4 to `0xFF`, so there is no level filter. The ETW
  event-ID filter and the callback allowlist admit 256, 257, and 258. This is
  Breaking because the dnstap output is part of the contract, the receiver now
  gets messages it did not get before, and no option restores the old set. A
  receiver that treats every `CLIENT_RESPONSE` as a successful answer must now
  read the RCODE in the packet. The counters `received` and `encoded`, and the
  performance counters Events Received and Frames Sent, now include event 258,
  so their meaning changes. The message format is unchanged. Event 258 has
  level Error, which level 4 already passed, so the level change does not add
  captured events. NXDOMAIN is a successful response (257) and was already captured. The
  keywords also enable events that include 284, 285, 287, 288, and 597 to
  599, which the event-ID filter excludes. Event 259 (`IGNORED_QUERY`) stays
  excluded: an IGNORE policy produces it with no packet and no response on the
  wire, and the query itself is still captured as 256. They cost `dns.exe` CPU when they occur but never
  reach the service. See
  [the plan](docs/plans/response-failure-capture-plan.md) and
  [configuration.md](docs/guide/configuration.md#subscription).
- `--etw-min-buffers N` sets the requested minimum ETW buffer count, 0 to 4,096
  and at most `--etw-max-buffers`. ETW reserves the minimum when the session
  starts, so a known peak does not need on-demand buffer allocation. `0` lets
  ETW choose. A minimum above the maximum is a startup error.
  `capture started` gains `etw_min_buffers`.
  A new information event, `ETW buffers applied`, records the minimum, maximum,
  and allocated counts that ETW reported after the session started. See
  [the plan](docs/plans/etw-min-buffers-plan.md) and
  [configuration.md](docs/guide/configuration.md#etw-buffer-minimums).
- A Windows performance counter set, `EtwDnstap`, through a Performance
  Counters V2 provider. One instance per service, named after the ETW session.
  It shows throughput rates, loss totals, the channel fill level, and ETW buffer
  use. Rates and totals are read by reference from the existing counters. The
  other gauges are published once a second. Installation copies
  `packaging/perfcounters/etw-dnstap.man` next to the executable and registers
  it with `lodctr /m:`. Uninstall unregisters it with `unlodctr /m:`. Without a
  registration, the service captures normally but the counter set is not
  visible to consumers. See
  [configuration.md](docs/guide/configuration.md#performance-counters).
- TLS to the dnstap receiver, on by default. The connection uses TLS 1.2 or 1.3
  through rustls with the `ring` provider. The receiver certificate is verified
  with the Windows machine certificate store through `rustls-platform-verifier`.
  Revocation is checked for the receiver certificate only, and the connection
  proceeds when the status cannot be obtained. `--tls-server-name NAME` sets the name the
  certificate must match and the SNI. The default is the receiver IP as an
  `iPAddress` SAN. `--plaintext` disables TLS and cannot be combined with
  `--tls-server-name`. A certificate that does not verify is a protocol failure:
  fatal at the first connection, a counted failed attempt on a reconnect. No
  client certificate. `capture started` gains `tls` and `tls_server_name`.
  Build note: `ring` ships prebuilt assembly objects without EH-continuation
  metadata, so `.cargo/config.toml` now links with `/FORCE:GUARDEHCONT`. The
  rest of the image keeps its `/GUARD:EHCONT` metadata; the linker prints one
  `LNK2047` warning per object. The reason and the removal condition are in the
  config comment and in
  [the plan](docs/plans/tls-receiver-plan.md).
- The parser is woken once per delivered ETW buffer that enqueued events
  (`EVENT_TRACE_LOGFILEW.BufferCallback`), and after data it polls for up to
  200 µs before it parks (adaptive: off after an empty linger, on again after an
  idle park). A real-time session delivers each flush interval as one burst; the
  timed poll woke too late, so a 10,000-event burst overflowed the 8,192-slot
  queue. Lab, default settings: 5,000 QPS ring loss fell from about 19% to 0,
  and 10,000 QPS loss from 60.7% to 0.3%-1.1%. The per-event callback stays
  wake-free.
  New counters `etw_buffers_delivered` and `parser_wake_requests`. See
  [the validation record](docs/history/buffer-wake-validation.md). Commit
  `dde7188`.
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
- A property test for `copy_record` admission and copying. Commit `14d6e6a`.
- Release PE check in CI, `scripts/ci/verify-pe.ps1`. It fails when a hardening
  flag is missing, when the image has a PDB entry, or when it imports an ASan
  runtime or the dynamic C or Universal CRT. Commit `47206d4`.
- An advisory CI job runs `cargo test-release` and `cargo asan`. Commit `47206d4`.
- Dependabot checks each weekly for the root, `fuzz/`, and `tools/dns-stress/`
  Cargo workspaces, and for the GitHub Actions pins. Commit `47206d4`.
- `Grant-EtwDnstapEtwAccess.ps1 -BackupPath` saves the original bytes of each ETW
  descriptor value it changes. `-Restore` writes them back. Commit `700dd62`.
- The install guide has a snapshot procedure before a repair or upgrade. Commit
  `700dd62`.
- Internal: a `socket_write` criterion group (`benches/hot_path.rs`) that
  measures the Frame Streams write path, and the Phase 2 results of the code
  review remediation plan in
  `datasets/baselines/code-review-2026-11-01/` (host criterion tables and a lab
  comparison of the two release images: 11.5% fewer thread cycles per event,
  13% less private memory, same delivery and no drops at 10,000 QPS).

### Changed

- The network reconnect and deadline-clock tests wait on observable conditions
  with a 20 s bound instead of fixed sleeps, except for the one sleep that is the
  scenario (idle time before an operation's first I/O).
- The service reports `SERVICE_START_PENDING` progress to the SCM once per 16,384
  allocated pool slots, and the start and stop wait hints equal the startup
  budget (10 s plus 1 s per 16,384 slots) and the 10 s shutdown deadline instead
  of a fixed 15 s. The checkpoint sequence is unchanged in shape.
- The `--receiver` help text, README.md, SECURITY.md, and the configuration and
  install guides state the TLS revocation check precisely: `rustls-platform-verifier`
  checks revocation for the receiver (end-entity) certificate only; a status that
  cannot be obtained does not fail the connection; a certificate that Windows
  CryptoAPI reports as revoked (from a CRL, an OCSP responder, or a stapled OCSP
  response) is rejected; intermediates are not checked and hard-fail revocation is
  not provided. Time in the verifier counts against the 2 s handshake deadline. The
  `--help` output changes; the behavior does not.
- Internal cleanups with no behavior change: each `unsafe` block holds one
  operation with its own `SAFETY` comment; `ControlTrace(STOP)` uses the same
  `QueryProperties` buffer as the session query; the event-ID filter's `FilterIn`
  is a `u8` like the C `BOOLEAN`; the session query returns a typed `Win32Status`
  and a DNS Server stop or restart is a typed `TargetChange` with the old and new
  process IDs (log text unchanged); the protobuf tag bytes, the TDH property-flag
  mask, and the `EnableTraceEx2` timeout are named constants; `EventProducer` is
  implemented directly on `Capture`; eligible functions are `const fn`; shadowed
  FFI status bindings have distinct names; short error messages start lowercase.
- **Breaking.** The default ETW minimum buffer request changes from a fixed 4 to
  0, the default of the new `--etw-min-buffers`. ETW raises the request to at
  least 2 buffers per logical processor. On a host with 1 logical processor, ETW
  now reserves 2 buffers instead of 4. On hosts with 2 or more logical
  processors, the session does not change. Pass `--etw-min-buffers 4` for the
  old request.
- The rate warning (Event ID 3) uses the rate that the new defaults were
  validated for. It fires above 22,000 events/s instead of 1,500, only when the
  default ETW settings and the default `--capacity` are both in use, and at most
  once per run instead of once per measurement window. Its text no longer says
  that events may be dropped. It points to `ring_dropped` and `etw_lost` and
  gives the high-rate settings as an example. Lab basis: one 60-second run at
  10,000 queries per second with the new defaults on 2026-11-01 had 0 ring drops
  and 0 `etw_lost`.
- **Breaking.** Two command-line defaults change, and the command-line
  defaults are part of the public contract under the local policy, so this
  increases `y`. `--etw-max-buffers` goes from 12 to 64, and `--capacity` goes
  from 8,192 to 24,576 (3 times). Ranges, validation, and behavior are
  unchanged. A service that sets neither option now uses more memory:
  - Counted storage at the defaults rises from 28.22 MiB (29,595,984 bytes) to
    68.60 MiB (71,932,240 bytes).
  - The startup deadline rises from 11 s to 12 s, because it is 10 s plus 1 s
    per 16,384 channel slots, rounded up.
  - ETW allocates the extra buffers only on demand, so the higher maximum
    costs at most 64 x 16 KiB = 1 MiB.

  The lab has not tested the new defaults as a set. The low-rate lab results in
  [configuration.md](docs/guide/configuration.md#low-rate-defaults) were
  measured with the previous defaults. A service that wants the old values must
  pass `--etw-max-buffers 12 --capacity 8192`.
- **Breaking.** The receiver connection is TLS by default. An existing
  deployment that points at a plaintext receiver must add `--plaintext` to the
  service binary path. Without it, the TLS handshake against the plaintext
  receiver is a protocol failure, and the service stops at startup with
  Event ID 1. This changes the public contract under the local policy, so it
  increases `y`.
- The `EnableTraceEx2: access denied` message no longer tells the operator to
  grant `TRACELOG_GUID_ENABLE` on the DNS provider GUID to the service SID. The
  grant script removes such a grant on its next run, and the install guide says
  not to add one. The message now says to inspect that descriptor and points to
  the install guide.
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
- ETW access-denied errors name the missing right, the GUID, and
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
  `diag`), with `src/event.rs` and `src/sink.rs` as seams for the output sink.
  Commit `dc9d7e0`. Commit `f0b51f4` added `target` and `instance`; commit
  `9fce762` removed `instance` with the mutex.
- Lab performance scripts are hardened and consolidated under `scripts/`. Commit
  `6e62b2e`. This commit does not change runtime behavior.
- Documentation is reorganized into `docs/guide/`, `docs/design/`,
  `docs/performance/`, `docs/plans/`, and `docs/history/` by status. Commit
  `54eafae`. See the [documentation index](docs/README.md#layout).
- **Breaking.** The service starts with the delayed automatic start type and a
  dependency on `DNS` (`sc.exe config EtwDnstap start= delayed-auto depend= DNS`).
  Existing installs must be reprovisioned. A stop of `DNS` also stops `EtwDnstap`,
  and it stays stopped until it is started again. See the
  [start type notes](docs/guide/install.md#start-type). Commit `700dd62`.
- The session reclaim checks the session GUID as well as the name and shape. A
  leaked session with the same name and another GUID is left alone, and startup
  fails. Commit `9fce762`.
- ETW cleanup errors are logged through `tracing`. Commit `9fce762`.
- Release builds trim build-host paths from the executable with
  `trim-paths = "all"`. This needs nightly Cargo. Commit `77d7c47`.
- `windows-sys/Win32_Media` is enabled only by the `diag-timing` feature, so the
  default build does not link it. Commit `77d7c47`.
- CI pins `cargo-fuzz` to 0.13.2. Dependabot does not update that pin. Commit
  `47206d4`.
- CI pins `cargo-deny` 0.20.2 and `cargo-llvm-cov` 0.9.1 like `cargo-fuzz`, and
  checks the formatting of `fuzz/`. Commit `2fcc4a2`.
- Finished-campaign lab tools moved to `scripts/performance/archive/`: the
  service-stop config and pcap tools, the ring-fix and JSONL-metrics analyzers
  (with their tests), `count-parser-ramp-diagnostic.py` and
  `windows-control.ps1`. `LabCommon.psm1` no longer exports
  `Get-LabGuestPrelude`, `Get-LabCredential` or `Assert-LabId`. Commits
  `63dd477` and `015d385`.
- Documentation: the single-pass decoder plan, the live-load plan, the
  packet-flow diagram and the ArrayQueue capacity experiment moved to
  `docs/history/`. A current decoder contract is in
  [single-pass-decoder.md](docs/design/single-pass-decoder.md). Commit `40a6b65`.
- CI also runs the archived analyzer tests,
  `scripts/performance/archive/test_archived_analyzers.py`. Commit `21e31c0`.
- Documentation: the cleanup validation record is in
  [cleanup-validation.md](docs/history/cleanup-validation.md), and the cleanup
  plan moved to `docs/history/`. The decoder contract now states the `Port`
  byte order, cache, and epoch rules correctly. Commit `21e31c0`.
- The release profile uses fat LTO and one codegen unit. Parse and encode are
  about 10% faster, the release image is 7.7% smaller, and release builds take
  about 50% longer. In the lab, the service with all four Phase 2 changes used
  11.5% fewer thread cycles per event than the build before them (3,085 to
  2,729, three runs per image, ranges 2.8% and 0.9%, at 10,000 QPS).
- Event pool leases no longer count references. The pool is allocated once and
  lives for the process.
- The extended-data item tables moved from the slot header to the end of the
  slot bytes. Pool memory is 15% smaller: 59.59 MiB at defaults (was 68.60) and
  145.53 MiB at `--capacity 65536` (was 169.54). In the lab the service's
  private bytes at defaults went from 72.9 to 63.4 MiB. A record with extended
  data needs 16 bytes more room per item for a regular slot, and this provider
  sends none.
- The Frame Streams writer sets socket timeouts only when the time left moves
  by more than 5 ms. A blocked socket call can run up to 5 ms past its 2 s
  deadline.
- Internal: `src/pipeline.rs` is split into `src/pipeline/config.rs` (command
  line and validation), `watchdog.rs`, `supervisor.rs`, and `parser.rs`, with
  the tests beside each module. No behavior change.
- Internal: a `DiagTiming` type replaces the per-line `diag-timing` feature
  attributes in the parser loops. It is the timing collector with the feature
  on and a zero-sized type with empty inline methods with it off, and the
  argument-hiding `drain_queue` wrapper is gone. The feature-on log output is
  unchanged and the normal release image is the same size.

### Removed

- **Breaking.** The `--etw-flush-ms` option and the periodic ETW flush. The
  supervisor no longer sends `EVENT_TRACE_CONTROL_FLUSH`, and it no longer
  wakes the parser after a flush. A service binary path that still has
  `--etw-flush-ms` fails command-line parsing, and the service does not start.
  Remove the option from the binary path before you start this build. This
  changes the public contract under the local policy, so it increases `y`.
  The `capture stats` and `capture stopped` lines lose the `etw_flushes`,
  `etw_flush_errors`, and `etw_flush_max_us` counters. The `capture started`
  line loses the `etw_flush_ms` field. The `ETW session flush failed` and
  `ETW session flush took N ms` warnings are gone. The reason is the buffer
  wake in commit `dde7188`. The parser drains each ETW delivery while it
  arrives. With the buffer-wake build, all defaults, and no flush, the lab
  measured 0 ring loss at 1,000 and 5,000 QPS and 0.32% at 10,000 QPS. It
  measured 0 ring drops at 50,000 QPS with a small `etw_lost`. See
  [the validation record](docs/history/buffer-wake-validation.md).
  `--etw-flush-seconds` stays (default 1, range 1 to 60). The high-rate
  guidance changes from
  `--etw-flush-ms 50 --etw-max-buffers 256 --etw-buffer-kib 64` to
  `--etw-max-buffers 256 --etw-buffer-kib 64 --capacity 65536`. The low-rate
  warning names the new settings. In the lab, the new combination lost no
  events inside ETW in any run, and the channel dropped no events up to about
  44,000 QPS. At about 49,400 QPS, one of two runs filled the 65,536-event
  channel and dropped 0.28%. With this build and all defaults, the channel
  dropped 0.28 to 0.58% at about 10,000 QPS, 12 to 17% at about 30,000 QPS,
  and 0.34 to 4.39% at about 50,000 QPS, with `etw_lost` about 0.2%. See
  [the flush removal validation](docs/history/flush-removal-validation.md) and
  [High-rate settings](docs/guide/configuration.md#high-rate-settings).
- The per-event TDH fallback reader. Every captured event is now decoded by a
  plan compiled from the manifest schema. A DNS Server schema that does not
  compile is no longer decoded through `TdhGetPropertySize` and `TdhGetProperty`.
  Its events are rejected, counted in `schema_rejected` and `rejected`, and one
  warning per (event ID, version) names the reason: `DNS event schema is
  unsupported and its events are rejected`. The supported Windows Server 2025
  schema compiles, so its output does not change. Each parser also drops a
  64 KiB packet buffer and a 512-byte property buffer. See
  [the removal plan](docs/history/tdh-fallback-removal-plan.md). Commit
  `69db97d`.
- **Breaking.** The single-instance guard, a named mutex
  `Global\EtwDnstap-<session>`. It is removed, and no mutex is created. Only
  the SCM starts one process for the service, and the identity check refuses a
  process that does not run as this service. Existing installs must be
  reprovisioned. Commit `9fce762`.
- The `rust-version` field in `Cargo.toml`. The build needs the pinned nightly,
  so no minimum stable version applies. Commit `47206d4`.
- The `--output`/`-o`, `--max-output-bytes`, and `--metrics` options and the
  service JSONL metrics writer. Arguments that use them now fail command-line
  parsing. Remove them from the binary path before you start this build. See
  [install.md](docs/guide/install.md#repair-or-upgrade). The removing commit is
  not identified in this file. See the
  [instrumentation compatibility note](docs/README.md#instrumentation-compatibility-note).
- Five unreferenced archived lab scripts, `Test-LabAnswer`,
  `lab_config.receiver_endpoint()`, the unread `lab.json` key
  `windows_vm.install_dir`, and the PyYAML requirement (only an archived tool
  used it). Commits `63dd477` and `015d385`. Commit `21e31c0` deleted the
  unused `Assert-LabId` function.

### Fixed

- `Invoke-LabGuest` in `scripts/performance/LabCommon.psm1` passed a multi-element
  argument list to the guest as one nested array, so callers with several
  arguments received them all in the first parameter. Arguments now arrive as
  separate positional parameters; single-argument callers are unchanged.
- The identity check reported `truncated token groups` for an unrestricted
  token instead of `the process token is not restricted`. An unrestricted token
  returns `TokenRestrictedSids` as a bare zero count.
- The documented `sc.exe config ... password= ""` did nothing in Windows
  PowerShell 5.1, which drops the empty argument. The docs and the identity
  error now run it through `cmd.exe /c`.
- `Grant-EtwDnstapEtwAccess.ps1 -BackupPath` failed on Windows PowerShell 5.1
  with `Argument types do not match`, before it changed anything. It now writes
  the backup. Commit `d721569`.
- `ubuntu-parser-ramp-control.py`: recursive monitoring raised `NameError`
  because `monitor()` read a variable that only `finish()` defined. Commit
  `015d385`.

### Security

- DNS process filter: an ETW `EVENT_FILTER_TYPE_PID` filter admits only the DNS
  Server process, and the callback checks the process ID again. Records from
  other processes count as `foreign_pid_rejected`. Commit `f0b51f4`.
- The service no longer runs as LocalSystem. It runs as LocalService with a
  restricted service SID and only `SeChangeNotifyPrivilege`. The profile was
  validated on one lab DNS Server (Windows Server 2025, build 26100) on
  2026-11-01. Load and other Windows builds were not tested. A later lab run
  tested the SCM recovery policy and a DNS Server restart. See
  [the lab record](docs/history/skills-evaluation-lab-validation.md).
- **Breaking.** The service's SCM security descriptor is pinned with `sc.exe
  sdset`. SYSTEM and Administrators control the service. Interactive and
  service logons may query it, but they may not start, stop, or configure it.
  Network and batch logons get no access. Existing installs must be
  reprovisioned. Commits `700dd62` and `b8d0427`.
- Backup and restore folders under `ProgramData\EtwDnstap` must be owned by
  SYSTEM or Administrators, must not be junctions, and must not let anyone else
  write. The grant script refuses otherwise, never reuses a backup folder, and
  validates `changes.json` before `-Restore` applies it. `-Restore` skips values
  that changed after the run unless `-Force` is given. This closes a local
  privilege escalation found in review before release. Commit `b8d0427`.
- The DNS Server process ID is checked every 5 s. Windows can reuse that process
  ID after the DNS Server exits. A process that gets the same ID before the next
  check could inject records. The service cannot hold the DNS Server process
  open, because the restricted token is denied `OpenProcess` in the lab. See
  [SECURITY.md](SECURITY.md). Commit `5f610f5`.
- The ETW callback context requires the DNS Server process ID. The earlier
  default, which accepted any process, could not be reached from the service,
  and it is removed. Commit `0865e19`.

## Commit log through `dde7188`

This is not a release. It lists every commit on `main` from the first commit
through `dde7188`, from `git log`, grouped by category. Commits after `9172f2d`
are in the last subsection. Each entry is summarized
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

### After `9172f2d` (all 2026-11-01)

Added:

- `dc9d7e0`, Split ETW module and add pipeline seams for sinks and producers.
- `8588619`, Restructure documentation and add project files.
- `12cce7a`, Add property tests, fuzz targets, golden vectors and CI.
- `9ab6dad`, Add skills-driven evaluation plan.
- `14d6e6a`, Add a property test for copy_record admission and copying.
- `8c79f69`, Add the obsolete and legacy code cleanup plan.
- `dde7188`, Wake the parser once per delivered ETW buffer.

Changed:

- `6e62b2e`, Harden and consolidate lab performance scripts.
- `f0b51f4`, Reconnect receiver, filter DNS PID, and run as virtual account.
- `54eafae`, Reorganize docs into guide, design, performance, plans and history.
- `b7f23ba`, Document reconnect, PID filter, virtual account and runtime stats.
- `df6fd9c`, Reconcile CHANGELOG, guides and security notes with the code.
- `47206d4`, Verify the release PE in CI and widen supply-chain checks.
- `f3b4a6b`, Document the evaluation changes; move the executed plan to history.
- `efb47af`, Record the backup hardening and narrower service DACL in CHANGELOG
  and SECURITY.
- `2fcc4a2`, Remove manifest keys that repeat Cargo defaults; pin CI tools.
- `63dd477`, Remove unreferenced lab scripts and unused LabCommon exports.
- `015d385`, Archive finished-campaign Python tools; fix parser-ramp monitor.
- `0865e19`, Rename ring-era names, share test scaffolding, require the DNS PID.
- `40a6b65`, Move executed plans and retired designs to history; fix doc status.
- `21e31c0`, Record the cleanup validation; fix review findings; move the plan
  to history.

Removed:

- `69db97d`, Remove the per-event TDH fallback reader.

Fixed:

- `d721569`, Record lab validation of the evaluation fixes; fix grant script on
  PS 5.1.
- `987ec0f`, Make the log-file reclaim tests reach the log-file check.
- `78d1ca0`, Make verify-pe.ps1 fail when it parses nothing.

Security:

- `6718863`, Run as LocalService with a restricted service SID.
- `e8560ac`, Scope the ETW grant to the service's own session GUID.
- `9fce762`, Drop the global instance mutex, pin the DNS process, check session
  GUID.
- `700dd62`, Back up and restore ETW and service state; pin the service DACL.
- `77d7c47`, Trim build-host paths from the release binary; narrow windows-sys.
- `5f610f5`, Remove the DNS process handle; document the PID-reuse window.
- `b8d0427`, Close the backup-folder privilege escalation; guard restore.
