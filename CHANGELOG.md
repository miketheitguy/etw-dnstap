# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). The project uses
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) and the policy below.

## Versioning policy

The version is the `version` field in [Cargo.toml](Cargo.toml). The service
reports it in the `version` field of its `capture started` event. It can also be
read offline, without running the service, from the executable's version resource
(`(Get-Item <exe>).VersionInfo`). The executable's version resource and the MSI
`ProductVersion` both come from `Cargo.toml`, so they agree. The MSI is never
built from a pre-release or build-metadata version, because an MSI version has
three numeric fields: major and minor up to 255, and patch up to 65535. The MSI
`UpgradeCode` never changes. It is what lets a newer MSI upgrade an older one.
The repository has no Git tag and no release yet.
Every change stays under `[Unreleased]` until a tag is made.

**SemVer rule.** A major version of `0` means initial development. Under
[SemVer 2.0.0](https://semver.org/spec/v2.0.0.html), anything may change at any
time, and the public API should not be treated as stable.

**Local policy while the version is `0.y.z`.** This policy states what the
project promises. It is not a SemVer rule. The public contract has these parts:

- The command-line options, and their defaults and ranges.
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

- Two counters that tell a late wake of the router thread from a blocking call in its
  loop: `router_wait_max_recent_us` (the longest single wait) and
  `router_pass_max_recent_us` (the longest pass outside the wait). They are peaks over the
  last 60 seconds, set once a second like `write_max_recent_us`. The `output stats` and
  `output stopped` lines carry them. New performance counters 67 and 68 are appended, and the
  updated manifest must be registered again on upgrade. Routing does not change.

- `--receiver-stall-seconds` (default 15, range 2 to 300): how long the receiver may
  accept no bytes, while the service has data to send, before the service ends the session.
  The MSI has no property for it, as for the other options. Pass it in `SERVICE_ARGS`.
  `capture started` records it.
- An MSI installer, `etw-dnstap-<version>-x64.msi`, built with WiX v7 by
  [scripts/build/Build-Msi.ps1](scripts/build/Build-Msi.ps1). It does the work of
  [install.md](docs/guide/install.md) steps 2 to 6 and creates the spool folder.
  The install needs one restart, and the service then starts with delayed
  automatic start. The uninstall reverses it, and it deletes the spool and
  the ETW snapshots. The MSI is unsigned. The lab validated install, rollback, repair, a receiver
  change, and uninstall on Windows Server 2025, build 26100.
- A version resource in the executable (`VERSIONINFO`). It holds the `Cargo.toml`
  version as `FileVersion` and `ProductVersion`, and the product name and company.
  The performance counter names do not change.
- Event ID 6, an Error event in the Application log, for a panic. Its text starts
  with `EtwDnstap panicked:` and has the panic message and location. It is written
  just before the process aborts.
- A `version` field on the `capture started` event, set from the package version in
  `Cargo.toml`. It is how an operator reads the running version now that the binary
  has no `--version` flag.
- A disk spool for DNS frames that the receiver has not accepted yet. It is on
  by default, in `C:\ProgramData\EtwDnstap\spool`. While the receiver is down or
  slow, frames are written to segment files there. After the receiver
  reconnects, the service sends them oldest first, before newer frames. Delivery is at most once while the service runs: a frame that the socket fully accepted is not
  sent again. A crash or kill can send again the frames that the connection had not yet taken, so
  delivery across a crash is at least once. The folder must exist and pass the service's trust check.
  Without it, the service logs one warning and runs without a spool. The install
  and upgrade guides have the folder step. The spool holds DNS data unencrypted,
  so see [SECURITY.md](SECURITY.md). New options are `--spool-dir`,
  `--no-spool`, `--spool-max-mib` (default 4096), and `--spool-min-free-mib`
  (default 2048). A `dnstap-sender` thread now owns the receiver connection and
  the spool. New performance counters 52 to 60 are appended, and the updated
  manifest must be registered again on upgrade. `disconnected_dropped` now
  counts only frames lost in the sink, and the help text of counters 2, 11, and
  18 changed to match. See [configuration.md](docs/guide/configuration.md#spool).
- Signals for a slow or saturated receiver. The `output stats` and `output stopped`
  lines gain `bytes_sent`, `send_queue_depth_max`, `write_max_recent_us`,
  `spool_oldest_age_ms`, and `link_saturated`. The two peaks cover the last 60 seconds and
  the sender thread sets them once a second, so no reader resets them. The existing
  `write_max_us` and `slow_writes` keep their meaning. The sender thread sets
  `link_saturated` when the spool backlog trends up over 10 samples (at least 10 seconds,
  with at least 7 of the 10 steps rising) with the receiver connected. It logs `dnstap link is saturated` (Event ID 3) and, at the end,
  `dnstap link is no longer saturated` (Event ID 2). Nothing about routing changes. New
  performance counters 61 to 66 are appended, and the updated manifest must be
  registered again on upgrade. See
  [monitoring.md](docs/guide/monitoring.md#the-saturated-state) and
  [troubleshooting.md](docs/guide/troubleshooting.md#the-receiver-is-slow).
- A data-path reference, [docs/design/data-path.md](docs/design/data-path.md), with an
  animation of the five stages from an ETW event to a TCP frame. The sources
  that build the GIF are in `scripts/docs/pipeline-animation/`.
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
  [the plan](docs/history/response-failure-capture-plan.md) and
  [configuration.md](docs/guide/configuration.md#subscription).
- `--etw-min-buffers N` sets the requested minimum ETW buffer count, 0 to 4,096
  and at most `--etw-max-buffers`. ETW reserves the minimum when the session
  starts, so a known peak does not need on-demand buffer allocation. `0` lets
  ETW choose. A minimum above the maximum is a startup error.
  `capture started` gains `etw_min_buffers`.
  A new information event, `ETW buffers applied`, records the minimum, maximum,
  and allocated counts that ETW reported after the session started. See
  [the plan](docs/history/etw-min-buffers-plan.md) and
  [tuning.md](docs/guide/tuning.md#etw-buffer-minimums).
- A Windows performance counter set, `EtwDnstap`, through a Performance
  Counters V2 provider. One instance per service, named after the ETW session.
  It shows throughput rates, loss totals, the channel fill level, and ETW buffer
  use. Rates and totals are read by reference from the existing counters. The
  other gauges are published once a second. Installation copies
  `packaging/perfcounters/etw-dnstap.man` next to the executable and registers
  it with `lodctr /m:`. Uninstall unregisters it with `unlodctr /m:`. Without a
  registration, the service captures normally but the counter set is not
  visible to consumers. See
  [monitoring.md](docs/guide/monitoring.md#performance-counters).
- TLS to the dnstap receiver, on by default. The connection uses TLS 1.2 or 1.3
  through rustls with the `ring` provider. The receiver certificate is verified
  with the Windows machine certificate store through `rustls-platform-verifier`.
  Revocation is checked for the receiver certificate only, and the connection
  proceeds when the status cannot be obtained. `--tls-server-name NAME` sets the name the
  certificate must match. A DNS name is also sent as the SNI, and an IP address
  is not, because rustls sends no SNI for an IP address. The default is the
  receiver IP as an `iPAddress` SAN, so the default sends no SNI. `--plaintext` disables TLS and cannot be combined with
  `--tls-server-name`. A certificate that does not verify is a protocol failure:
  fatal at the first connection, a counted failed attempt on a reconnect. No
  client certificate. `capture started` gains `tls` and `tls_server_name`.
  Build note: `ring` ships prebuilt assembly objects without EH-continuation
  metadata, so `.cargo/config.toml` now links with `/FORCE:GUARDEHCONT`. The
  rest of the image keeps its `/GUARD:EHCONT` metadata; the linker prints one
  `LNK2047` warning per object. The reason and the removal condition are in the
  config comment and in
  [the plan](docs/history/tls-receiver-plan.md).
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
  `7f91264`.
- Receiver reconnect with jittered backoff (250 ms to 30 s). Each new session
  repeats the Frame Streams handshake. Frames are never resent. Commit `b46da39`.
- Periodic counter logs, `capture stats` and `output stats`, every
  `--stats-interval-seconds` (default 60; 0 disables them), and final
  `capture stopped` and `output stopped` reports. Commit `b46da39`.
- `disconnected_dropped` counter for frames lost while the receiver is down.
- Low-rate warning when the received rate exceeds about 1,500 events/s under the
  default ETW settings. Commit `b46da39`.
- Watchdog Event ID 5, written before an abort. The startup deadline is 10 s plus
  1 s per 16,384 channel slots.
- Negative schema cache: an event whose schema fails to load is rejected without
  repeated TDH calls, and counted in `schema_rejected`.
- Tests, fuzzing, and CI. Commit `4603bc8` added them. Property tests
  (`proptest`) check Frame Streams framing, event decoding, and the dnstap
  encoder. Golden byte vectors compare the encoder with the upstream
  `dnstap.proto`. Three libFuzzer targets (`decoder`, `control_frame`, and
  `plan_compiler`) are in the separate `fuzz/` workspace. The CI workflow runs
  formatting, Clippy, tests, a release build, `cargo deny`, the Python tests,
  and a link check. The 60-second fuzz smoke run and coverage are report-only.
  The `lab.yml` workflow runs the ignored tests that need live ETW, on a
  self-hosted runner.
- A property test for `copy_record` admission and copying. Commit `abf6538`.
- Release PE check in CI, `scripts/ci/verify-pe.ps1`. It fails when a hardening
  flag is missing, when the image has a PDB entry, or when it imports an ASan
  runtime or the dynamic C or Universal CRT. Commit `f3f134b`.
- An advisory CI job runs `cargo test-release` and `cargo asan`. Commit `f3f134b`.
- Dependabot checks each weekly for the root, `fuzz/`, and `tools/dns-stress/`
  Cargo workspaces, and for the GitHub Actions pins. Commit `f3f134b`.
- `Grant-EtwDnstapEtwAccess.ps1 -BackupPath` saves the original bytes of each ETW
  descriptor value it changes. `-Restore` writes them back. Commit `39c4337`.
- The install guide has a snapshot procedure before a repair or upgrade. Commit
  `39c4337`.
- Internal: a `socket_write` criterion group (`benches/hot_path.rs`) that
  measures the Frame Streams write path, and the Phase 2 results of the code
  review remediation plan in
  `datasets/baselines/code-review-2026-11-01/` (host criterion tables and a lab
  comparison of the two release images: 11.5% fewer thread cycles per event,
  13% less private memory, same delivery and no drops at 10,000 QPS).

### Changed

- **Breaking.** The capacity defaults are sized for 200,000 queries per second (about
  400,000 events per second and 41.2 MB/s of dnstap frames). The command-line defaults
  are part of the public contract under the local policy, so this increases `y`. Ranges,
  validation, and behavior are unchanged. A service that sets none of these options uses
  more memory. The lab tested them at 200,000 queries per second on a 12-vCPU guest. With the
  131,072-event channel, 3 of 3 runs dropped nothing (two idle, one with 12 busy CPU threads).
  `etw_lost` was 0 except in one run with 24 busy threads. See
  [200,000 QPS sizing runs on 12 vCPUs](docs/history/slow-receiver-validation.md#200000-qps-sizing-runs-on-12-vcpus-2026-11-04).
  At 100,000 queries per second the guest's ETW delivered events late and the channel still
  overflowed. The larger channel halves that loss but does not remove it.
  - `--capacity` goes from 24,576 to 131,072. At 65,536 the channel peaked at 94 to 98% of
    its size at 200,000 queries per second.
  - `--etw-buffer-kib` goes from 16 to 128, `--etw-min-buffers` from 0 to 12, and
    `--etw-max-buffers` from 64 to 128. ETW still raises the minimum to at least two per
    logical processor.
  - The send queue between the parser and the router goes from 128 to 256 blocks (16 MiB),
    and the link queue from 128 to 256 blocks (16 MiB). The link queue holds 0.41 s at
    41.2 MB/s, which is two Windows Server scheduler quanta of 187.5 ms. A link thread that
    wakes late, as the lab saw (195 to 216 ms), no longer spills to the spool. No thread
    priority changes. The send queue also holds a burst of late ETW events. The replay
    credit stays 127 batches.
  - Counted storage at the default capacity rises from 59.59 MiB to 283.03 MiB. The block
    pool rises from 34.5 MiB to 68.5 MiB. The startup deadline rises from 12 s to 18 s. ETW
    buffer memory is at most 16 MiB (128 buffers of 128 KiB), against 1 MiB.
  - The rate warning (Event ID 3) fires above 440,000 events/s instead of 22,000 and names
    about 400,000 events/s (200,000 queries per second) as the validated rate. Its example
    is now `--etw-max-buffers 256 --capacity 262144`.
  - Pass `--etw-buffer-kib 16 --etw-min-buffers 0 --etw-max-buffers 64 --capacity 24576`
    for the old values. The queue sizes are not options. See
    [Defaults](docs/guide/tuning.md#defaults) and
    [Slow receiver](docs/design/slow-receiver.md).
- `--capacity` now accepts up to 524,288 events (it was 262,144). The default stays 131,072,
  so a service that does not set the option behaves as before. The lab saw ETW deliver events
  1 to 3 seconds late in one burst at 100,000 queries per second, and 262,144 still dropped
  0.03 to 0.16 million events in two runs. Counted storage at 524,288 with 64 jumbo slots is
  1,108.03 MiB (1,132.11 MiB with 256), and the startup deadline is 42 s. The working set is
  expected near 1.6 GB; it is not measured yet. A wider accepted range is backward compatible.
  See [Storage](docs/guide/configuration.md#storage).
- The service no longer sets `SO_SNDBUF` (it was 1 MiB). Windows dynamic send
  buffering is on unless an application sets that option, and a fixed buffer limits
  throughput to about the buffer size per round trip. The kernel send buffer now
  grows with the link, so more bytes can be in it, and be lost unseen, when a session
  ends. See [Slow receiver](docs/design/slow-receiver.md).
- **Breaking.** Four counters change meaning (`disconnected_dropped`, `bytes_sent`, and
  the write-time counters `slow_writes`, `write_max_us`, and `write_max_recent_us`, listed
  below). A slow or paused receiver no longer drops frames or costs a reconnect. A new
  `dnstap-link` thread owns the connection and writes with non-blocking sends, so the
  `dnstap-sender` thread (now the router) never waits on the socket. The link queue
  holds 128 blocks (8 MiB) in memory, and only a full link queue spills to the spool.
  A session ends only when the receiver accepts no bytes for `--receiver-stall-seconds`;
  it used to end when one write took 2 seconds. When a session ends, the live frames
  that the socket did not fully accept go to the spool (or to the front of the next
  session) and are sent again. With TLS, the frames of the one unconfirmed 16 KiB piece
  go back with them. With a spool, a failed connection therefore counts nothing in
  `disconnected_dropped` (the spool keeps the replay batches, see the next entry). `bytes_sent` counts bytes
  the socket accepted (with TLS, the record bytes). `slow_writes`, `write_max_us`, and
  `write_max_recent_us` measure the wait for the socket to become writable, one block at
  a time. The block pool grows from 17.3 MiB to 34.5 MiB, and the shutdown bound falls
  from about 5 to about 3.2 seconds. A stalled receiver ends the shutdown in about 1.1
  seconds, with no STOP. A STOP/FINISH exchange that does not complete at a requested
  stop no longer fails the stop: the service writes nothing about it and stops with
  success (it used to exit with a service-specific error, and SCM restarted it). Unsent
  frames stay in the spool. `output_errors` no longer counts it. See
  [Slow receiver](docs/design/slow-receiver.md).
- **Breaking.** The spool keeps data until the receiver connection has taken it. The
  router reads replay batches ahead (up to 127 of 64 KiB, about 8 MiB) and the spool deletes
  a segment only after the connection has fully taken it. When a session ends, the batches
  the connection did not take are read again from the frame after the last one it took, so a
  failed connection loses no replay frame and `disconnected_dropped` does not count one. This
  also lets a backlog replay without a router wake for each batch. `cursor.bin` now holds
  the position after the last frame the connection took, and the service saves it once per
  router pass. A normal stop repeats nothing. After a crash or kill, delivery is at least
  once: the next run sends again the replay data that was read and not taken, up to 127
  batches (about 8 MiB), and the receiver can see those frames twice. `replayed` counts a
  frame when the connection takes it, not when the service reads it. The spool holds up to
  about 9 MiB more on disk while it drains, and the size cap does not delete those
  segments. If `cursor.bin` cannot be written, replay goes on and a crash repeats more
  frames. See [Slow receiver](docs/design/slow-receiver.md#b2-spool-rewind).
- **Breaking.** The spool is on by default. An upgrade needs the spool folder
  and the re-registered counter manifest. See
  [Add the spool folder](docs/guide/upgrade.md#add-the-spool-folder). With a
  spool in use, frames from a receiver outage go to the spool and are sent
  later. They are no longer counted in `disconnected_dropped`. `disconnected_dropped`
  and counter 18 (Receiver Down Drops) now count only frames lost in the sink.
  The help text of counters 2, 11, and 18 changed. The two receiver warnings
  changed text. After a restart, the receiver can get frames that an earlier run
  left in the spool. To keep the earlier loss handling, run with `--no-spool`.
  The counter meaning changes still apply with `--no-spool`.
- Documentation restructure. `docs/guide/install.md` and
  `docs/guide/configuration.md` are split by task. The new guides are
  `build.md`, `receiver.md`, `upgrade.md`, `tuning.md`, and `monitoring.md`.
  The sections moved with their content. Links to the old anchors are
  updated. The root `README.md` and `docs/README.md` are shorter, and the index
  no longer repeats each document's status block. The four executed feature
  plans moved from `docs/plans/` to `docs/history/`. The Phase 2 results moved
  from the baseline record to `datasets/baselines/code-review-2026-11-01/phase-2-results.md`.
  The troubleshooting guide has a symptom table.
- Documentation audit against the code. The operator guides, the pooled intake
  reference, and the policy documents now match the source. Examples: the pool
  is leaked and has no reference count, the TLS handshake and the Frame Streams
  handshake share one 2-second deadline, and the Events Dropped counter does not
  add the jumbo drops twice.
- Internal: the Clippy lints `multiple_unsafe_ops_per_block`,
  `missing_const_for_fn`, and `shadow_unrelated` are enabled as warnings in
  `Cargo.toml`, so CI fails on them. Three functions became `const fn`, and the
  test code was renamed or split to comply. No behavior change.
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
  [tuning.md](docs/guide/tuning.md#low-rate-defaults) were
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
  fatal. Commit `b46da39`.
- The service exits for SCM restart when the DNS Server process ID changes or the
  DNS Server stops, checked every 5 s. Commit `b46da39`.
- Session reclaim no longer requires a matching buffer size. A mismatch logs a
  warning, and the session is reclaimed.
- Shutdown waits up to 5 s for `ProcessTrace` to return before `CloseTrace`. The
  socket send buffer is requested at 1 MiB. An idle parser flushes after 100 ms.
  The parser wakes after each successful periodic ETW flush.
- ETW code is split into `src/etw/` (`session`, `keywords`, `intake`, `schema`,
  `diag`), with `src/event.rs` and `src/sink.rs` as seams for the output sink.
  Commit `dc9d7e0`. Commit `b46da39` added `target` and `instance`; commit
  `cb0d290` removed `instance` with the mutex.
- Lab performance scripts are hardened and consolidated under `scripts/`. Commit
  `f1475d8`. This commit does not change runtime behavior.
- Documentation is reorganized into `docs/guide/`, `docs/design/`,
  `docs/performance/`, `docs/plans/`, and `docs/history/` by status. Commit
  `a2ae289`. See the [documentation index](docs/README.md#layout).
- **Breaking.** The service starts with the delayed automatic start type and a
  dependency on `DNS` (`sc.exe config EtwDnstap start= delayed-auto depend= DNS`).
  Existing installs must be reprovisioned. A stop of `DNS` also stops `EtwDnstap`,
  and it stays stopped until it is started again. See the
  [start type notes](docs/guide/install.md#start-type). Commit `39c4337`.
- The session reclaim checks the session GUID as well as the name and shape. A
  leaked session with the same name and another GUID is left alone, and startup
  fails. Commit `cb0d290`.
- ETW cleanup errors are logged through `tracing`. Commit `cb0d290`.
- Release builds trim build-host paths from the executable with
  `trim-paths = "all"`. This needs nightly Cargo. Commit `72cc41e`.
- `windows-sys/Win32_Media` is enabled only by the `diag-timing` feature, so the
  default build does not link it. Commit `72cc41e`.
- CI pins `cargo-fuzz` to 0.13.2. Dependabot does not update that pin. Commit
  `f3f134b`.
- CI pins `cargo-deny` 0.20.2 and `cargo-llvm-cov` 0.9.1 like `cargo-fuzz`, and
  checks the formatting of `fuzz/`. Commit `a691c10`.
- Finished-campaign lab tools moved to `scripts/performance/archive/`: the
  service-stop config and pcap tools, the ring-fix and JSONL-metrics analyzers
  (with their tests), `count-parser-ramp-diagnostic.py` and
  `windows-control.ps1`. `LabCommon.psm1` no longer exports
  `Get-LabGuestPrelude`, `Get-LabCredential` or `Assert-LabId`. Commits
  `18a29a9` and `aa7e199`.
- Documentation: the single-pass decoder plan, the live-load plan, the
  packet-flow diagram and the ArrayQueue capacity experiment moved to
  `docs/history/`. A current decoder contract is in
  [single-pass-decoder.md](docs/design/single-pass-decoder.md). Commit `6dac61f`.
- CI also runs the archived analyzer tests,
  `scripts/performance/archive/test_archived_analyzers.py`. Commit `d538e35`.
- Documentation: the cleanup validation record is in
  [cleanup-validation.md](docs/history/cleanup-validation.md), and the cleanup
  plan moved to `docs/history/`. The decoder contract now states the `Port`
  byte order, cache, and epoch rules correctly. Commit `d538e35`.
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
- **Breaking.** `--session-name` accepts only 1 to 128 ASCII letters, digits,
  underscore, dot, or hyphen, and it is case-sensitive. The earlier rule allowed
  1 to 200 UTF-16 units, so a name with any other character, such as a space or a
  non-ASCII letter, a name over 128 characters, or a name that starts with a
  hyphen, is now rejected at startup. The MSI passes the name as a separate
  argument, where a leading hyphen would read as an option. The MSI property
  `SESSION_NAME`, the provisioning script, and `Grant-EtwDnstapEtwAccess.ps1`
  use the same rule. The MSI also has a launch condition that refuses a
  `SESSION_NAME` that starts with `-`. A condition cannot test the length or the
  character set, so the scripts and the service enforce the rest. The session GUID is still
  derived from the exact name, so the ETW grant must spell the name as
  `--session-name` does. `Capture::start` keeps its 1,024-unit check as a
  backstop.
- A same-version rebuild of the MSI is now a major upgrade. `MajorUpgrade` sets
  `AllowSameVersionUpgrades="yes"`, and every build has a new `ProductCode`, so
  the rebuild replaces the installed build. Before, it installed beside it as a
  second product. The WiX build reports ICE61 as warning `WIX1076`.
- The MSI runs its install steps (folder checks, group membership, ETW grant,
  recovery policy) only in a pass that installs the executable component. A
  maintenance `msiexec /i` with a changed `SESSION_NAME` or `RECEIVER` and no
  `REINSTALL` now changes nothing. Before, it moved the ETW grant to the new
  name but left the service's binary path on the old name, so the service failed
  every start with access denied. Use `REINSTALL=ALL` or a repair to change a
  setting. A product installed from an older MSI keeps the old behavior until it
  is upgraded.
- The rate warning now reads `received rate is above 22,000 events/s; the default
  buffer settings were validated at about 20,000 events/s; ...`, so it states
  both the threshold and the validated rate. The text before named only the
  validated rate while it fired at 22,000.

### Removed

- **Breaking.** The service writes nothing to stdout or stderr. Tracing output goes
  only to the Application event log, and the panic text goes there too. The `--help`,
  `-h`, `--version`, and `-V` options are removed. They fail command-line parsing like
  any unknown argument. A console launch, with any arguments, exits with code 1 and
  prints nothing. Under SCM, an argument error is still Event ID 1 and an SCM failure
  status. Read the running version from the `version` field of `capture started`.
  This changes the public contract under the local policy, so the next release
  increases `y`.
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
  wake in commit `7f91264`. The parser drains each ETW delivery while it
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
  [High-rate settings](docs/guide/tuning.md#high-rate-settings).
- The per-event TDH fallback reader. Every captured event is now decoded by a
  plan compiled from the manifest schema. A DNS Server schema that does not
  compile is no longer decoded through `TdhGetPropertySize` and `TdhGetProperty`.
  Its events are rejected, counted in `schema_rejected` and `rejected`, and one
  warning per (event ID, version) names the reason: `DNS event schema is
  unsupported and its events are rejected`. The supported Windows Server 2025
  schema compiles, so its output does not change. Each parser also drops a
  64 KiB packet buffer and a 512-byte property buffer. See
  [the removal plan](docs/history/tdh-fallback-removal-plan.md). Commit
  `ee94de6`.
- **Breaking.** The single-instance guard, a named mutex
  `Global\EtwDnstap-<session>`. It is removed, and no mutex is created. Only
  the SCM starts one process for the service, and the identity check refuses a
  process that does not run as this service. Existing installs must be
  reprovisioned. Commit `cb0d290`.
- The `rust-version` field in `Cargo.toml`. The build needs the pinned nightly,
  so no minimum stable version applies. Commit `f3f134b`.
- The `--output`/`-o`, `--max-output-bytes`, and `--metrics` options and the
  service JSONL metrics writer. Arguments that use them now fail command-line
  parsing. Remove them from the binary path before you start this build. See
  [upgrade.md](docs/guide/upgrade.md). The removing commit is
  not identified in this file. See the
  [instrumentation compatibility note](docs/README.md#instrumentation-compatibility-note).
- Five unreferenced archived lab scripts, `Test-LabAnswer`,
  `lab_config.receiver_endpoint()`, the unread `lab.json` key
  `windows_vm.install_dir`, and the PyYAML requirement (only an archived tool
  used it). Commits `18a29a9` and `aa7e199`. Commit `d538e35` deleted the
  unused `Assert-LabId` function.

### Fixed

- A TLS 1.2 session with an AES-GCM cipher that sent 2^24 records silently dropped all
  later frames. At that limit rustls queues `close_notify` and refuses the record, yet its
  writer still reports the bytes as taken. The link counted every block as sent, the stall
  timer never fired, and the session stayed up, so frames were lost without a counter.
  The link now treats a TLS write that queues no record as a session error. It hands the
  data back, and the reconnect uses new keys. A session reaches 2^24 records after 256 GiB
  of frames in full 16 KiB records, and sooner when the records are smaller.
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
  the backup. Commit `50fb489`.
- `ubuntu-parser-ramp-control.py`: recursive monitoring raised `NameError`
  because `monitor()` read a variable that only `finish()` defined. Commit
  `aa7e199`.
- A transient failure to load an event schema no longer rejects that schema for
  the life of the process. A Win32 error from `TdhGetEventInformation` rejects
  the (event ID, version) key, and the parser loads the schema again after 65,536
  further rejected events of the key. A load that succeeds clears the key. A
  schema that cannot be decoded is still rejected for good. Before, one failed
  load could drop all queries or all responses until a restart. The new warning
  is `DNS event schema load failed; its events are rejected until the load is
  retried`, logged for the first four failures of a key and then every 60th.
  `schema_rejected` also counts each failed retry.
- A TLS certificate failure after a start while the receiver was down is now
  logged at once. The first rejection before any session logs one error (Event
  ID 4) that names the Windows trust store and `--tls-server-name`, or says that
  the receiver refused the Frame Streams handshake. `dnstap reconnect failed`
  gains the fields `cause` (`connect`, `timeout`, `tls`, or `protocol`) and
  `failures`, and it is also logged for the first failure of each `cause` since a
  session was last established. A session that is established clears that list,
  and a lost session does not, so alternating causes add at most one extra line
  per cause in an outage. A TLS alert after the handshake, such as
  `received fatal alert: CertificateRequired` from a receiver that requires a
  client certificate, counts as a `tls` failure, and the first-rejection error
  also tells you that the service sends no client certificate. Before, the
  reason appeared only at failure 61. A reconnect thread that cannot start, or
  that ends without a result, is now logged.
- The spool logs a warning when the free-space floor makes it refuse blocks for
  10 seconds: `dnstap spool is refusing blocks: the volume is below the
  free-space floor and no older segment is left to delete`, with `free_mib`,
  `needed_mib`, and `pause_s`. It logs the first four pauses and then every 60th.
  Before, the spool refused blocks with no log line.
- Watchdog and parser logs match their documentation. The deadline that guards
  the steps before the arguments are read is now named `startup`, not `shutdown`.
  The capture start has its own `CAPTURE_START_TIMEOUT` of 10 s, the same value
  as before. The `rejected ETW event` warning now logs the first four and then
  every 60th rejection, with the running `rejected` count. Before, it logged only
  the first four.
- The MSI uninstall no longer fails when the service was deleted by hand. The
  provisioning script reads and removes the Performance Log Users membership by
  the service SID, not by `NT SERVICE\EtwDnstap`. If that step fails, for
  example because application control blocks the small `Add-Type` class that it
  compiles, the uninstall prints a warning, continues with the grant and the
  spool, and tells you to remove the SID by hand. On install, the script still
  checks the membership by name first, and it reads the members by SID only if
  that check throws. The lab has not tested these cases.
- Documentation: the upgrade guide no longer says a same-version rebuild is
  probably refused, and it states that `/fvomus` repair needs the same
  `ProductCode` and replaces only missing or older files. `/fvamus` forces every
  file. The configuration and receiver guides say that the receiver certificate
  should carry an `iPAddress` SAN when the server name is the receiver IP. Whether
  the platform verifier also accepts the address in a `dNSName` SAN or falls back
  to the common name is not verified.

### Security

- DNS process filter: an ETW `EVENT_FILTER_TYPE_PID` filter admits only the DNS
  Server process, and the callback checks the process ID again. Records from
  other processes count as `foreign_pid_rejected`. Commit `b46da39`.
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
  reprovisioned. Commits `39c4337` and `3f15814`.
- Backup and restore folders under `ProgramData\EtwDnstap` must be owned by
  SYSTEM or Administrators, must not be junctions, and must not let anyone else
  write. The grant script refuses otherwise, never reuses a backup folder, and
  validates `changes.json` before `-Restore` applies it. `-Restore` skips values
  that changed after the run unless `-Force` is given. This closes a local
  privilege escalation found in review before release. Commit `3f15814`.
- The DNS Server process ID is checked every 5 s. Windows can reuse that process
  ID after the DNS Server exits. A process that gets the same ID before the next
  check could inject records. The service cannot hold the DNS Server process
  open, because the restricted token is denied `OpenProcess` in the lab. See
  [SECURITY.md](SECURITY.md). Commit `39a5ce4`.
- The ETW callback context requires the DNS Server process ID. The earlier
  default, which accepted any process, could not be reached from the service,
  and it is removed. Commit `9146740`.

## Commit log through `7f91264`

This is not a release. It lists every commit on `main` from the first commit
through `7f91264`, from `git log`, grouped by category. Commits after `9172f2d`
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
- `253df79`, Restructure documentation and add project files.
- `4603bc8`, Add property tests, fuzz targets, golden vectors and CI.
- `65b6e7f`, Add skills-driven evaluation plan.
- `abf6538`, Add a property test for copy_record admission and copying.
- `26cd08d`, Add the obsolete and legacy code cleanup plan.
- `7f91264`, Wake the parser once per delivered ETW buffer.

Changed:

- `f1475d8`, Harden and consolidate lab performance scripts.
- `b46da39`, Reconnect receiver, filter DNS PID, and run as virtual account.
- `a2ae289`, Reorganize docs into guide, design, performance, plans and history.
- `ca1fe2d`, Document reconnect, PID filter, virtual account and runtime stats.
- `b70702a`, Reconcile CHANGELOG, guides and security notes with the code.
- `f3f134b`, Verify the release PE in CI and widen supply-chain checks.
- `c988b32`, Document the evaluation changes; move the executed plan to history.
- `685f869`, Record the backup hardening and narrower service DACL in CHANGELOG
  and SECURITY.
- `a691c10`, Remove manifest keys that repeat Cargo defaults; pin CI tools.
- `18a29a9`, Remove unreferenced lab scripts and unused LabCommon exports.
- `aa7e199`, Archive finished-campaign Python tools; fix parser-ramp monitor.
- `9146740`, Rename ring-era names, share test scaffolding, require the DNS PID.
- `6dac61f`, Move executed plans and retired designs to history; fix doc status.
- `d538e35`, Record the cleanup validation; fix review findings; move the plan
  to history.

Removed:

- `ee94de6`, Remove the per-event TDH fallback reader.

Fixed:

- `50fb489`, Record lab validation of the evaluation fixes; fix grant script on
  PS 5.1.
- `fc4e33c`, Make the log-file reclaim tests reach the log-file check.
- `9837085`, Make verify-pe.ps1 fail when it parses nothing.

Security:

- `1a54272`, Run as LocalService with a restricted service SID.
- `87c8d6d`, Scope the ETW grant to the service's own session GUID.
- `cb0d290`, Drop the global instance mutex, pin the DNS process, check session
  GUID.
- `39c4337`, Back up and restore ETW and service state; pin the service DACL.
- `72cc41e`, Trim build-host paths from the release binary; narrow windows-sys.
- `39a5ce4`, Remove the DNS process handle; document the PID-reuse window.
- `3f15814`, Close the backup-folder privilege escalation; guard restore.
