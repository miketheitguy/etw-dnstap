# Source review remediation plan

This plan fixes selected findings from the October 30, 2026 `src` review. It
follows the [ETW session recovery and event-ID filter plan](etw-session-recovery-and-event-filter-plan.md),
and also fixes three findings from that plan's Stage 3 review (Stage R6).

| Review item | Stage |
| --- | --- |
| 4. Invalid service arguments exit before the SCM connection | R2 |
| 6. Pending service status never advances `dwCheckPoint` | R2 |
| 7. `ControlTraceW` stop `ERROR_MORE_DATA` treated as failure | Already fixed (see below) |
| 9. Magic token-group attribute constants | R2 |
| ETW session settings; configurable buffer size and flush timer | R3 |
| Extended-item metadata per pool slot | R4 |
| Schema lookup hashing on every event | R4 |
| Parser thread wakes 500 times per second when idle | R5 |
| Network writer uses an 8 KiB `BufWriter` | R1 |
| `read_finish` duplicates `read_control` | R1 |
| Session query buffer lacks log-file-name storage (Stage 3 finding 1) | R6 |
| Session query failures reported as shape mismatches (Stage 3 finding 2) | R6 |
| Event-ID filter described as provider-side (Stage 3 finding 3) | R6 |

Item 7 is already done: `stop_outcome` in `src/etw.rs` classifies
`ERROR_MORE_DATA` as stopped, so `Capture::stop` neither errors nor retains the
handle. The Stage 3 review verified this behavior. No further work is planned.

## Rules for every stage

- Stages run sequentially in the main worktree. Uncommitted work in the tree
  must be preserved: never commit, stash, reset, checkout, restore, or clean.
- Touch only the files a stage lists. Do not deploy, run elevated commands,
  start real ETW sessions, or run `#[ignore]`d tests.
- Match surrounding style: comment density, naming, `anyhow` `ensure!`/`bail!`,
  `#[expect(..., reason = "...")]` for intentional casts, `// SAFETY:` on every
  `unsafe` block. Lints are strict: warnings denied, clippy pedantic,
  `unwrap_used` denied.
- Each stage ends with these checks passing:
  `cargo fmt`, `cargo fmt --check`,
  `cargo clippy --locked --all-targets -- -D warnings`,
  `cargo test --locked --lib`, and `git diff --check` on the touched files.
  The starting test count is 88 passed, 1 ignored.
- Each stage appends its own subsection to this plan's completion record.

## Stage R1: Network writer

Files: `src/network.rs`, `src/network/shutdown_tests.rs` (only if a test
helper must change).

1. Add `const WRITE_BUFFER_BYTES: usize = 64 * 1024;` with a one-line comment:
   it holds many typical frames per send, and larger frames bypass it. Use
   `BufWriter::with_capacity(WRITE_BUFFER_BYTES, transport)` in
   `NetworkSink::connect`. If `shutdown_tests.rs` builds its own
   `BufWriter`, leave it unchanged unless a test depends on capacity.
2. Remove the duplication between `read_finish` and `read_control`. Both read
   an 8-byte header, check the zero escape, check length `4..=512`, read the
   body, then call `validate_control`. Extract one helper that reads a control
   frame into caller storage and reports which phase failed:

   ```rust
   enum ControlPhase { HeaderRead, BodyRead, Validation }
   fn read_control_frame<'a>(
       reader: &mut impl Read,
       storage: &'a mut [u8; MAX_CONTROL_LENGTH],
   ) -> Result<&'a [u8], (ControlPhase, io::Error)>
   ```

   - Header `read_exact` failure is `HeaderRead`. A nonzero escape or a
     length outside `4..=512` is `Validation`, as `read_finish` reports today.
     Body `read_exact` failure is `BodyRead`.
   - `read_control` calls the helper, discards the phase, and then calls
     `validate_control`. Its `io::Result` behavior is unchanged.
   - `read_finish` maps each phase to today's context strings exactly:
     `"FINISH header read"`, `"FINISH body read"`, and
     `"FINISH validation"`. `validate_control` failures stay
     `"FINISH validation"`. The existing `shutdown_tests.rs` assertions on
     these strings must pass unchanged.
3. Tests: existing network and shutdown tests must pass unmodified. Add one
   test that `NetworkSink::connect` yields a writer whose `BufWriter`
   capacity is at least `WRITE_BUFFER_BYTES`, if it is reachable without
   unsafe code.

## Stage R2: Service hardening (items 4, 6, 9)

Files: `src/lib.rs`, `src/service.rs`, `src/pipeline.rs` (only
`parse_args`), `Cargo.toml` (windows-sys feature list only), `README.md` (only
if it describes argument-error behavior).

### Item 4: argument errors

Today `pipeline::parse_args()` calls `CaptureArgs::parse()`. On invalid
arguments clap prints to the missing service stderr and exits before
`StartServiceCtrlDispatcherW`, so SCM reports only error 1053.

1. Change `parse_args()` to return `Result<CaptureArgs, clap::Error>` using
   `CaptureArgs::try_parse()`.
2. In `lib::run`, handle help and version output first. If the error kind is
   `clap::error::ErrorKind::DisplayHelp` or `DisplayVersion`, call
   `error.print()` and return `Ok(())`. For every other error, pass the plain
   rendered text into the service layer as `Err(String)`. Verify that the
   rendering has no ANSI escape codes; clap's `Error::render()` returns a
   `StyledStr`, and its `Display` output is plain text.
3. `service::run` takes `Result<CaptureArgs, String>`. `ARGS` stores that
   result.
4. In `run_registered`, after reporting `SERVICE_START_PENDING`, take `ARGS`.
   On `Err(text)`, return `anyhow!("invalid service arguments: {text}")`. The
   existing path then writes the Event Log failure entry and reports
   `SERVICE_STOPPED` with the failure code. Run `verify_identity` before or
   after argument validation, whichever keeps the code simplest; state which in
   the completion record.
5. In `service::run`, if `StartServiceCtrlDispatcherW` fails (for example,
   the binary was run from a console) and `ARGS` still holds `Err(text)`,
   return the argument error instead of the dispatcher error. The console user
   then sees clap's message.
6. Tests: an invalid argument vector produces `Err` from a testable parse
   helper, not a process exit. `--help` and `--version` map to the
   print-and-succeed path. Factor the kind check into a pure function so it is
   testable.

### Item 6: checkpoint

1. Add a process-lifetime `static CHECKPOINT: AtomicU32`.
2. `report_status` increments it for `SERVICE_START_PENDING` and
   `SERVICE_STOP_PENDING`, passing the new value (starting at 1). It resets it
   to 0 for any other state.
3. `status_record` takes the checkpoint as a parameter instead of deriving
   `u32::from(pending)`.
4. Update `pending_status_accepts_no_controls_and_failure_is_reported` and
   add a test that successive pending reports increase the checkpoint and that
   running and stopped report 0. Keep the pure `status_record` test.

### Item 9: named constants

1. Add `"Win32_System_SystemServices"` to the windows-sys feature list in
   `Cargo.toml`. Features do not change `Cargo.lock`; confirm `--locked`
   still passes.
2. Replace `0x14` and `0x04` in `verify_identity` with `SE_GROUP_ENABLED` and
   `SE_GROUP_USE_FOR_DENY_ONLY`. These are `i32` in windows-sys, so use
   `.cast_unsigned()`. Remove the "TOKEN_GROUP attributes from winnt.h"
   comment because the names now say it.

## Stage R3: ETW session settings

Files: `src/etw.rs`, `src/pipeline.rs`, `README.md` (flag table and ETW
section).

The session uses 64 × 64 KB buffers, which is 4 MB, with a 1-second flush.
Microsoft's [EVENT_TRACE_PROPERTIES](https://learn.microsoft.com/windows/win32/api/evntrace/ns-evntrace-event_trace_properties)
guidance:

- `BufferSize` is 4–16384 KB, and production sessions should use 128 KB or
  less.
- Keep `MinimumBuffers` small; ETW raises it to 2 per logical processor.
- Raise `MaximumBuffers` when events drop at peaks.
- A real-time `FlushTimer` sets delivery latency; higher values cost less CPU.

1. Add a `SessionConfig { buffer_kib: u32, maximum_buffers: u32,
   flush_seconds: u32 }` struct in `src/etw.rs`, with a `Default` matching
   the new defaults below. `SessionProperties::new(&SessionConfig)` uses it
   instead of `SESSION_BUFFER_KIB` and the hard-coded 64/1 values.
   `MinimumBuffers` stays 4. Thread the config through `Capture::start`,
   `start_session`, `reclaim_stale_session`, and every other
   `SessionProperties::new()` call site.
   - Query and stop buffers may use `SessionConfig::default()`. Only
     `Wnode.BufferSize` and the name offsets matter there.
   - `reclaimable` must compare `BufferSize` against the configured value,
     because a leaked session from this configuration has that size. Remove
     `SESSION_BUFFER_KIB` if nothing else uses it.
2. Add CLI flags to `CaptureArgs` and pass them through `run_capture`:
   - `--etw-buffer-kib N`: range 4–1024, default 64. Microsoft treats 256 KB
     and above as diagnostic-only; the help text should say so.
   - `--etw-max-buffers N`: range 4–4096, default 256 (16 MiB at 64 KB).
     This raises peak headroom from 4 MB; ETW allocates beyond the minimum
     only on demand.
   - `--etw-flush-seconds N`: range 1–60, default 1. This is the upper bound
     on ETW delivery latency.
   - Reject `buffer_kib * maximum_buffers` above 1 GiB (1,048,576 KiB)
     during startup with a clear error. clap cannot cross-check two flags, so
     use a small validated constructor such as
     `SessionConfig::new(...) -> Result<Self>`.
3. Include the three values in the existing "capture started"
   `tracing::info!`.
4. Tests:
   - `SessionProperties::new(&config)` writes the configured `BufferSize`,
     `MaximumBuffers`, and `FlushTimer`.
   - CLI defaults and range bounds are enforced (0, below minimum, above
     maximum).
   - The 1 GiB cross-check is enforced.
   - `reclaimable` accepts the configured size and rejects a different one.
   - Update the existing fingerprint tests for the new signature.
5. README: add the three flags to the flag table. In the ETW section, state
   the defaults, memory bound, and latency trade-off. Note that restarting
   with a different `--etw-buffer-kib` will not reclaim a session leaked under
   the old size; reclaim fails safe with the existing error.

## Stage R4: Intake memory and schema lookup

Files: `src/etw.rs`, `src/etw/pool.rs`.

### Extended-item metadata

`MAX_EXTENDED_ITEMS` is 64. Each pool slot carries `[EVENT_HEADER_EXTENDED_DATA_ITEM; 64]`
plus `[usize; 64]`, which is 1,536 bytes, about 43% overhead on a 2 KiB regular
slot. `copy_record` also zeroes arrays of the same size on the stack in every
callback. Windows defines fewer than 20 extended-data types. Each appears at
most once per event, and this session enables no optional enable properties.

1. Set `MAX_EXTENDED_ITEMS` to 16. Add a comment stating the reason above.
   Records with more items are still rejected as malformed by the existing
   `count > MAX_EXTENDED_ITEMS` check and counted in `callback_errors`.
2. Derive `JUMBO_BYTES` in `pool.rs` from the constants instead of the
   literal: two `u16`-sized payload regions plus `MAX_EXTENDED_ITEMS`
   seven-byte alignment gaps, rounded up to a multiple of 8. Update the
   comment. Assert at compile time that the result is a multiple of 8 and at
   least `2 * 65_535 + 7 * MAX_EXTENDED_ITEMS`.
3. Update `x64_storage_sizes_match_documented_budget` to the new `Slot` size,
   and any test that builds 64 or 65 extended items, so the over-limit case
   uses `MAX_EXTENDED_ITEMS + 1`. Update any doc comment that cites the old
   sizes.

### Schema lookup

`Parser.schemas` is a `HashMap<(u16, u8), Schema>` with SipHash hashing on
every event, even though only events 256 and 257 reach it, usually with one
version each.

1. Replace it with a `Vec<((u16, u8), Schema)>` created with capacity
   `MAX_CACHED_SCHEMAS = 8`, searched linearly. Keep the existing policy of
   clearing when full before inserting, at the new bound. Cache hits must
   still borrow without allocating or cloning.
2. Remove the `HashMap` import if unused. Update the test that inserts into
   `parser.schemas` (near `src/etw.rs:2650`) to push a tuple.
3. The existing allocation-free parser tests must keep passing. Add a test
   that inserting `MAX_CACHED_SCHEMAS + 1` distinct keys never exceeds the
   bound.

## Stage R5: Idle parser polling

Files: `src/pipeline.rs`, `README.md` (only if it states the 2 ms poll).

`drain_ring` parks for 2 ms after every empty poll, which is 500 wake-ups per
second while idle. Do not switch to a blocking `recv`/`recv_timeout`.
crossbeam then has `try_send` wake the blocked parser, which puts wake work on
the ETW callback. The [rejected wake-on-enqueue experiment](ring-wakeup.md)
showed that callback-side wakes caused heavy capture drops. The callback must
stay free of wake-ups.

Use bounded exponential idle backoff on the parser side only:

1. Add `IDLE_POLL_MIN = Duration::from_millis(2)` and
   `IDLE_POLL_MAX = Duration::from_millis(20)`.
2. Add a pure `fn next_idle_poll(current: Duration) -> Duration` that doubles
   and clamps to `IDLE_POLL_MAX`. In `drain_ring`, keep a `timeout` that
   starts at `IDLE_POLL_MIN`, resets to `IDLE_POLL_MIN` whenever an event is
   received, and advances with `next_idle_poll` after each empty poll's
   `park_timeout`.
3. Explain the bound in a comment. At 20 ms, an idle-to-burst transition
   queues at most about 2,000 events at 100,000 events per second before the
   parser wakes. That is well under the default 8,192-entry ring. Idle wake-ups
   fall from about 500 to about 50 per second.
4. Shutdown latency is unchanged: `producer_done` publication is followed by
   `unpark`, which ends a long park immediately. Keep the existing ordering
   and the final drain logic exactly.
5. Tests: `next_idle_poll` doubles 2→4→8→16→20 and stays at 20. All existing
   `drain_ring` tests pass unchanged.
6. Documentation: if `README.md` or `docs/ring-buffer.md` describes the 2 ms
   idle poll as current behavior, update that sentence only. Do not edit
   historical campaign documents.

## Stage R6: Session recovery follow-up

Files: `src/etw.rs`, `README.md`, and
`docs/etw-session-recovery-and-event-filter-plan.md` (wording and completion
record only). This stage runs after R3, so `reclaimable` already compares
against the configured buffer size.

### Query buffer with log-file-name storage (finding 1)

`reclaim_stale_session` currently queries with a buffer that has only
logger-name storage and `LogFileNameOffset = 0`. ControlTrace documents setting
both name offsets for QUERY and STOP. Depending on how ETW reports the
log-file offset, the current check either refuses this program's own leaked
session or accepts a real-time session that also writes a log file.

1. Add a private `#[repr(C)] struct QueryProperties { properties:
   EVENT_TRACE_PROPERTIES, logger_name: [u16; 1025], log_file_name: [u16;
   1025] }`. Its `new()` zero-initializes and sets:
   - `Wnode.BufferSize = size_of::<Self>()`;
   - `Wnode.Flags = WNODE_FLAG_TRACED_GUID`;
   - `LoggerNameOffset` and `LogFileNameOffset` to their field offsets.

   Use it for the reclaim QUERY and the reclaim STOP-by-name. Leave
   `SessionProperties` (with `LogFileNameOffset = 0`) for `StartTraceW`,
   because Microsoft requires offset 0 for real-time-only sessions.
   `Capture::stop` is unchanged.
2. Change `reclaimable` to take `&QueryProperties` plus the configured buffer
   size. Require all of the following:
   - `LogFileMode` includes `EVENT_TRACE_REAL_TIME_MODE`;
   - it has none of the file-mode bits already in `FILE_MODE_BITS`, nor
     `EVENT_TRACE_SYSTEM_LOGGER_MODE`, `EVENT_TRACE_BUFFERING_MODE`, or
     `EVENT_TRACE_PRIVATE_LOGGER_MODE` (Stage 3 nit 5);
   - `BufferSize` equals the configured value;
   - the returned `LogFileNameOffset` is either 0 or the unchanged
     `log_file_name` field offset, and in the latter case `log_file_name[0]`
     is 0.

   Reject any other offset. Delete `log_file_name_is_empty` and use safe field
   access.

### Query status handling (finding 2)

3. Add a pure classifier for the QUERY status: `ERROR_SUCCESS` means inspect
   the properties; `ERROR_WMI_INSTANCE_NOT_FOUND` means the session vanished
   after `StartTraceW` reported it; anything else is a failure.
   - On vanished: skip the stop and go straight to the single `StartTraceW`
     retry. Do not increment `stale_sessions_reclaimed` or warn.
   - On failure: return an error naming the session, the operation
     ("query existing ETW session"), and the status with its
     `std::io::Error` text.
   - Report the existing shape-mismatch message only when the query
     succeeded.

### Wording and stale text (finding 3 and Stage 3 nit 7)

4. Event-ID filtering is attribute filtering applied by the ETW runtime, not
   provider-side filtering. It cuts session buffer use and callback dispatch,
   not DNS Server's cost of generating events. Fix the README sentence that
   calls it provider-side. In `docs/etw-session-recovery-and-event-filter-plan.md`,
   fix the Stage 1 heading and text the same way and note the correction in
   its completion record. Rename the
   `docs/README.md` index row wording only if it says provider-side.
5. Fix the `Capture::start` doc comment that says an existing name is an
   error. Replace README's "Optional unused ETW session name" wording for
   `--session-name`, and rewrap the overlong README line the earlier reflow
   left.

### Tests

6. Add or update tests:
   - `QueryProperties::new()` offsets and `Wnode.BufferSize`.
   - `reclaimable` accepts both query shapes: offset unchanged with an empty
     name, and offset 0.
   - `reclaimable` rejects a nonempty log file name, an offset anywhere else,
     each forbidden mode bit, missing real-time mode, and a buffer-size
     mismatch.
   - The query-status classifier returns inspect, vanished, and failure.
7. Add two `#[ignore = "requires an elevated session"]` tests and do not run
   them:
   - Start a real-time-only session with a raw `StartTraceW` and leak it;
     prove `reclaim_stale_session` plus one start succeeds; stop the new
     session.
   - Start a real-time session that also writes a log file under
     `std::env::temp_dir()` with the same name; prove reclaim refuses it;
     stop it by name in cleanup.

   They must compile under `cargo clippy --all-targets`.

## Stage R7: Review

A higher-capability reviewer reads all R1–R6 changes against this plan. The
review covers:

- correctness and FFI soundness;
- the argument-error flow through SCM, including that `SERVICE_STOPPED` is
  reported exactly once;
- checkpoint semantics;
- session configuration plumbing and the reclaim fingerprint;
- pool layout arithmetic;
- parser backoff latency against ring capacity;
- the `read_finish` refactor's error phases;
- test adequacy and documentation accuracy.

Findings are ranked by severity with file and line references and recorded in
the completion record.

## Stage R8: Review remediation

This stage fixes the Stage R7 findings and Stage 3 finding 4 of the
[ETW session recovery plan](etw-session-recovery-and-event-filter-plan.md).
The same rules apply as for R1–R6, except that the coordinator writes the R8
completion record from the agents' reports. Agents do not edit the completion
record unless their sub-stage says so.

### R8a: ETW code and elevated tests

File: `src/etw.rs`.

1. **Log-file refusal test (major).** `reclaim_refuses_session_that_writes_log_file`
   must start its session with `LogFileMode = EVENT_TRACE_REAL_TIME_MODE` only,
   plus a log-file name and offset. That is the case where ETW writes a
   sequential file without any file-mode bit. Assert the error text contains
   "does not match this program's session shape". Optionally keep a separate
   SEQUENTIAL-mode case as a third ignored test with its own session name.
2. **Diagnostics.** In every elevated test, print the queried `LogFileMode`,
   `BufferSize`, `LogFileNameOffset`, and `log_file_name[0]` with
   `eprintln!`, so a live `--ignored --nocapture` run records ETW's actual
   QUERY behavior. Expose a small test-only query helper if needed.
3. **Isolation.** Give each elevated test its own session name. In the
   reclaim test, assert that `start_session` returns `ERROR_ALREADY_EXISTS`
   while the leaked session exists before reclaiming. Use a non-default
   configuration (`buffer_kib = 68`) so a live run exercises exact
   `BufferSize` matching with a value other than 64.
4. **Buffer size granularity.** `SessionConfig::new` must reject a
   `buffer_kib` that is not a multiple of 4. A page-multiple size avoids any
   upward adjustment by ETW that would defeat the exact reclaim fingerprint.
   Update its doc comment and tests.
5. **Memory wording.** Correct the `SessionConfig` doc comment. The 1 GiB
   check bounds the requested maximum only. ETW reserves at least two buffers
   per logical processor and raises `MaximumBuffers` to at least that
   adjusted minimum.
6. **Stale wording.** Change the `SUPPORTED_EVENT_IDS` doc comment from
   "provider-side filter" to "ETW runtime event-ID filter".
7. **Reclaim counter.** Increment `stale_sessions_reclaimed` and warn only
   when the reclaim STOP returned `ERROR_SUCCESS` or `ERROR_MORE_DATA`. If the
   STOP returned `ERROR_WMI_INSTANCE_NOT_FOUND`, the session vanished on its
   own; proceed to the retry without counting.
8. **Decision-table seam.** Make the reclaim decision unit-testable. Factor
   the logic of `reclaim_stale_session` into a function that takes
   query and stop operations as closures (for example
   `FnOnce(&mut QueryProperties) -> u32` and `FnOnce(&mut QueryProperties) -> u32`).
   The production wrapper passes the `ControlTraceW` calls. Unit-test these
   cases:
   - vanished: no stop, no count;
   - failed query: error, no stop;
   - shape mismatch: error, no stop;
   - match and stop success: one count;
   - match and stop more-data: one count;
   - match and stop vanished: no count;
   - match and stop failure: error.

### R8b: Pipeline and argument handling

Files: `src/pipeline.rs`, `src/lib.rs`.

1. Move `next_idle_poll` (with its constants and comment) above
   `drain_ring`'s explanatory comment, so that comment sits directly on
   `drain_ring`.
2. Rewrite the idle-poll comment to state its assumptions. ETW delivers
   real-time events in batches: full buffers, and every buffer on each
   FlushTimer tick. Ring overflow therefore depends on batch size relative to
   free ring space, and the 20 ms bound matters only in proportion to burst
   duration. The headroom figure assumes the default `--capacity`. Small
   capacities and large `--etw-buffer-kib` values reduce it.
3. Factor the reset-or-advance step into a pure helper, for example
   `fn idle_poll_after(received: bool, current: Duration) -> Duration`, used
   by `drain_ring`. Test that it resets to `IDLE_POLL_MIN` after an event.
4. In `lib::run`, strip clap's leading `"error: "` prefix from the rendered
   text before passing it on, so console output and the Event Log do not
   repeat "error:". Test the stripping helper.
5. Make clap the single source of the default session name:
   `#[arg(long, default_value = DEFAULT_SESSION_NAME, value_parser = parse_session_name)] session_name: String`.
   Remove the `unwrap_or_else` in `run_capture`. Change
   `session_name_defaults_and_can_be_overridden` to parse through
   `CaptureArgs::try_parse_from`.
6. Help text:
   - `--etw-buffer-kib` must be a multiple of 4.
   - `--etw-max-buffers` is a requested maximum. ETW may reserve more
     (at least two buffers per logical processor).

### R8c: Documentation

Files: `README.md`, `docs/ring-buffer.md`, `docs/pooled-intake.md`, and this
plan's Stage R2 completion record (one sentence).

1. Update current-behavior statements to the R4 sizes:
   - jumbo slots are 131,184 bytes;
   - regular `Slot` metadata is 408 bytes;
   - the extended-item limit is 16;
   - the schema cache holds at most 8 entries.

   The known stale places are `README.md` (jumbo slot bytes, "caches up to 32
   schemas"), `docs/ring-buffer.md` (jumbo slot bytes), and
   `docs/pooled-intake.md` (64 extended items, 131,584 bytes, the 1,560-byte
   slot, 32 schemas). Grep for `131584`, `131,584`, `1,560`, `1560`, `32
   schemas`, and `64 extended`. Leave figures inside historical measurement
   tables and results unchanged; add a short note where a historical figure
   could be mistaken for current behavior.
2. Correct the peak-memory wording in the README. The 1 GiB check bounds the
   requested buffer size times maximum buffers. ETW reserves at least two
   buffers per logical processor and can exceed the requested maximum on
   many-core hosts. Document that `--etw-buffer-kib` must be a multiple of 4.
3. Wrap the overlong R3 and R5 prose lines in `README.md` (near lines 189,
   191, and 259) and `docs/ring-buffer.md` (near line 15) to about 80 columns,
   matching surrounding prose. Preserve each file's line endings.
4. In this plan's "### Stage R2" record, add one sentence. In normal runs each
   pending phase is reported once, so `dwCheckPoint` reaches 1 and returns to
   0. It advances further only on the error path. Every phase is bounded by
   the 10 s watchdog, under the 15 s wait hint.

## Stage R9: Review

A higher-capability reviewer checks the R8 changes against R8a–R8c and the
Stage R7 findings, with the same read-only rules and output format as R7.

## Live validation (requires separate authorization)

On the DNS lab VM:

- Confirm that an invalid `ImagePath` argument produces an Event Log entry and
  a failed service stop instead of error 1053.
- Compare `etw_lost` and `etw_buffers_lost` with the new buffer defaults
  against the pooled-intake baseline.
- Confirm that idle CPU wake-ups fall and that burst drops do not rise after
  idle periods.

## Completion record

### Stage R1

- `src/network.rs`: added `WRITE_BUFFER_BYTES` (64 KiB) and
  `NetworkSink::connect` now uses `BufWriter::with_capacity`.
- Added `ControlPhase` and `read_control_frame`, which reads one bounded
  control frame and reports header, body, or validation failure. `read_control`
  and `read_finish` both use it; `read_finish` maps phases to the existing
  `FINISH header read`, `FINISH body read`, and `FINISH validation` contexts.
- Added test `connect_uses_write_buffer_capacity`; `shutdown_tests.rs` unchanged.
- `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D warnings`
  clean.
- `cargo test --locked --lib`: 89 passed, 1 ignored (88 baseline plus the new test).
- `git diff --check -- src/network.rs` clean.

### Stage R2

- `pipeline::parse_args` now returns `Result<CaptureArgs, clap::Error>` via
  `try_parse_from`; a private `parse_args_from` generic helper backs it for tests.
- `lib::run` prints help and version output and returns `Ok(())`, using the new
  pure `is_informational` kind check. Other errors go to `service::run` as
  `Err(error.render().to_string())`, and `service::run` takes
  `Result<CaptureArgs, String>`.
- Argument order: `verify_identity` runs before argument validation. Invalid
  arguments fail in `run_registered` and reach the existing single
  `SERVICE_STOPPED` report. If the dispatcher fails from a console launch, the
  argument error is returned instead of the dispatcher error.
- Checkpoint: `static CHECKPOINT: AtomicU32` is advanced by `checkpoint_for` for
  pending states and reset to 0 otherwise. `status_record` takes the checkpoint as
  a parameter.
- Identity: `verify_identity` uses `SE_GROUP_ENABLED` and
  `SE_GROUP_USE_FOR_DENY_ONLY` (via `.cast_unsigned()`) instead of `0x14`/`0x04`.
  `Win32_System_SystemServices` was added to `Cargo.toml`.
- Tests added: `argument_parsing_returns_errors_instead_of_exiting`,
  `only_help_and_version_are_informational`, and
  `checkpoint_advances_across_pending_reports_and_resets_otherwise`. The existing
  pending-status test was updated for the new parameter.
- Checks: `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D
  warnings` clean; `cargo test --locked --lib`: 92 passed, 1 ignored (89 baseline
  plus 3 new tests); `git diff --check` clean on the touched files.

In normal runs each pending phase is reported once, so `dwCheckPoint` reaches 1
and returns to 0. It advances further only on the error path. Every phase is
bounded by the 10 s watchdog, under the 15 s wait hint.

### Stage R3

- `src/etw.rs`: added `SessionConfig` (`buffer_kib`, `maximum_buffers`,
  `flush_seconds`) with `Default` (64, 256, 1) and `SessionConfig::new`, which
  validates the three ranges and rejects `buffer_kib * maximum_buffers` above
  1,048,576 KiB. `SESSION_BUFFER_KIB` is removed. `SessionProperties::new` takes
  `&SessionConfig` (MinimumBuffers stays 4). Query and stop buffers use the
  default.
- `Capture::start`, `start_session`, and `reclaim_stale_session` take the
  config. `reclaimable` compares `BufferSize` with the configured value.
- `src/pipeline.rs`: added `--etw-buffer-kib`, `--etw-max-buffers`, and
  `--etw-flush-seconds` with the planned ranges and defaults. `run_capture`
  builds the config before connecting to the receiver, and the "capture
  started" event logs all three values.
- Tests added: `session_properties_write_configured_values`,
  `fingerprint_compares_against_configured_buffer_size`,
  `session_config_enforces_ranges_and_one_gib_peak` (in `src/etw.rs`), and
  `etw_session_flags_default_and_enforce_ranges` (in `src/pipeline.rs`). Existing
  fingerprint tests use a `reclaimable_by_default` helper.
- `README.md`: three flag rows and an ETW paragraph covering defaults, the 16 MiB
  default peak, the latency trade-off, and that a restart with a different
  `--etw-buffer-kib` does not reclaim a session leaked under the old size.
- Checks: `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D
  warnings` clean; `cargo test --locked --lib`: 96 passed, 1 ignored (92 baseline
  plus 4 new tests); `git diff --check` clean on the touched files.

### Stage R4

- `src/etw.rs`: `MAX_EXTENDED_ITEMS` is 16, with a comment giving the reason
  (fewer than 20 extended-data types, each at most once, no optional enable
  properties). Records with more items are still rejected as malformed.
- `src/etw/pool.rs`: `JUMBO_BYTES` is now `(2 * 65_535 + 7 * MAX_EXTENDED_ITEMS)
  .next_multiple_of(8)` (131,184 bytes), with a const block asserting it is a
  multiple of 8 and covers the bound. The comment shows the largest `copy_record`
  layout (65,535 + 7 * 16 + 65,535) fits.
- Slot size: 1,560 bytes to 408 bytes. `x64_storage_sizes_match_documented_budget`
  is updated to 408.
- Schema cache: `Parser.schemas` is a `Vec<((u16, u8), Schema)>` with capacity
  `MAX_CACHED_SCHEMAS = 8`, scanned linearly. `insert_schema` clears when full
  and pushes; `HashMap` import removed. Cache hits still borrow without
  allocation.
- Tests: over-limit case uses `MAX_EXTENDED_ITEMS + 1`, and 64-item tests use
  the constant. Schema tests now push tuples and cap at 8 entries. New test
  `schema_cache_never_exceeds_bound_or_reallocates` inserts nine distinct keys
  and checks length and capacity.
- Checks: `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D
  warnings` clean; `cargo test --locked --lib`: 97 passed, 1 ignored (96 baseline
  plus 1 new test); `git diff --check` clean on the touched files.

### Stage R5

- `src/pipeline.rs`: `IDLE_POLL_MIN` (2 ms) and `IDLE_POLL_MAX` (20 ms) with a
  comment that keeps the callback wake-free (links `ring-wakeup.md`) and gives
  the bound: about 2,000 queued events per idle-to-burst transition at 100,000
  events/s, versus the 8,192-entry default ring. Idle wake-ups fall from about
  500/s to about 50/s.
- `drain_ring`: the idle timeout resets to 2 ms on each received event and
  doubles via the pure `next_idle_poll` (saturating, clamped to 20 ms) after each
  empty park. The done-branch final drain and check order are unchanged. No
  unpark or wake was added to the callback or producer side.
- Tests: new `idle_poll_backoff_doubles_and_clamps` (2, 4, 8, 16, 20, 20). All
  existing `drain_ring` tests pass unchanged.
- Docs: the current-behavior sentence in `README.md` and `docs/ring-buffer.md`
  now describes the 2 to 20 ms idle backoff. Historical docs are untouched.
- Checks: `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D
  warnings` clean; `cargo test --locked --lib`: 98 passed, 1 ignored (97 baseline
  plus 1 new test); `git diff --check -- src/pipeline.rs` clean.

### Stage R6

- `src/etw.rs`: new private `QueryProperties` (logger-name and log-file-name
  storage, both name offsets set) used for the reclaim QUERY and STOP-by-name.
  `StartTraceW` and `Capture::stop` keep `SessionProperties`.
- `reclaimable` takes `&QueryProperties`. It requires real-time mode, no file
  mode bits and no system, buffering, or private logger bits, the configured
  buffer size, and a log-file offset of 0 or the unchanged field offset with an
  empty name. `log_file_name_is_empty` is removed.
- Pure `query_outcome`: success inspects, `ERROR_WMI_INSTANCE_NOT_FOUND` returns
  without stopping or counting (the single `StartTraceW` retry follows), and
  other statuses fail with the session name and `std::io::Error` text. The
  shape-mismatch error is reached only after a successful query.
- `Capture::start` doc comment now describes reclaiming a matching leaked
  session. README: the event-ID filter sentence no longer says provider-side,
  the `--session-name` row no longer says "unused", and the overlong line is
  rewrapped. The Stage 1 heading and text in the recovery plan were corrected
  the same way, and `docs/README.md` has the matching index row.
- Tests: `query_properties_set_name_offsets_and_size`,
  `fingerprint_accepts_own_session_shape` and `fingerprint_rejects_foreign_sessions`
  rewritten for query-shaped input (each file-mode and other logger mode bit,
  missing real-time, offset 0 accepted), `fingerprint_compares_against_configured_buffer_size`
  updated, and `query_outcome_classifies_statuses` added. Two new ignored tests,
  `reclaim_replaces_leaked_real_time_session` and
  `reclaim_refuses_session_that_writes_log_file`, use a drop guard that stops
  the session by name. Neither was run.
- Checks: `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D
  warnings` clean; `cargo test --locked --lib`: 100 passed, 3 ignored (98 passed,
  1 ignored baseline, plus 2 new tests); `git diff --check -- src/etw.rs
  README.md` clean.
- Limitation: the elevated tests were not run, so live ETW QUERY behavior for
  the log-file offset and the vanished-session status is unverified. Live
  validation remains pending.

### Stage R7

An independent higher-capability review of R1–R6 found no blocker and no FFI
soundness defect. `cargo fmt --check`, strict Clippy, and
`cargo test --locked --lib` (100 passed, 3 ignored) were clean.

- **Major (R6):** the elevated log-file refusal test starts its session with
  `EVENT_TRACE_FILE_MODE_SEQUENTIAL`, so `reclaimable` rejects it on the mode
  bit and never reaches the log-file offset and name check. Start it real-time
  only, assert the shape-mismatch message, and print the queried
  `LogFileMode`, `LogFileNameOffset`, and first log-file unit in both elevated
  tests.
- **Minor:**
  - Both elevated tests share one session name and race under
    `--ignored`; the reclaim test also lacks an `ERROR_ALREADY_EXISTS`
    precondition check.
  - R4 sizes are stale in `README.md`, `docs/ring-buffer.md`, and
    `docs/pooled-intake.md` (131,584 bytes, 64 items, 1,560-byte slot,
    32 schemas).
  - Exact `BufferSize` matching may refuse this program's own session if ETW
    rounds odd `--etw-buffer-kib` values (needs live check).
  - The peak-memory wording ignores ETW's two-buffers-per-processor minimum.
  - The R5 latency comment assumes evenly paced arrivals and the default ring
    capacity.
- **Nits:**
  - `next_idle_poll` sits between `drain_ring` and its comment.
  - `SUPPORTED_EVENT_IDS` still says "provider-side".
  - clap text already begins `error: `, so messages double it.
  - Checkpoints only exceed 1 on the error path; this is acceptable under the
    10 s watchdog and 15 s wait hint.
  - Several R3/R5 README and doc lines are unwrapped.
  - The reclaim counter increments if the session vanishes between QUERY and
    STOP.
- **Untested:** the `reclaim_stale_session` decision table beyond the pure
  classifier, backoff reset after an event, and the end-to-end SCM argument
  path (live only). Stage 3 finding 4 (tests mirror the default-name
  expression) remains open.

Remediation of these findings is pending.

### Stage R8

The coordinator recorded this stage from the sub-stage reports. R8a ran on a
mid-tier model at medium effort, R8b on a low-cost model at medium effort, and
R8c on a low-cost model at low effort. R8a and R8c ran in parallel on disjoint
files, then R8b ran.

- **R8a (`src/etw.rs`).**
  - The log-file refusal test now starts a real-time-only session with a log
    file and asserts the shape-mismatch text.
  - A separate SEQUENTIAL-mode refusal test was added.
  - All three elevated tests have distinct names and print queried
    properties with `print_query`.
  - The replace test uses a 68 KiB buffer and asserts `ERROR_ALREADY_EXISTS`
    before reclaiming.
  - `SessionConfig::new` rejects buffer sizes that are not multiples of 4.
  - The memory and `SUPPORTED_EVENT_IDS` doc comments are corrected.
  - The reclaim counter no longer increments when the session vanished before
    STOP.
  - New `reclaim_decision` takes query and stop closures and has six
    decision-table unit tests.
- **R8b (`src/pipeline.rs`, `src/lib.rs`).**
  - The idle-poll block now sits above `drain_ring`'s comment, which states
    the batch-delivery and capacity assumptions.
  - `idle_poll_after(received, current)` replaces `next_idle_poll` and has a
    reset test.
  - clap's leading `error: ` is stripped once.
  - `--session-name` is a `String` with a clap default.
  - The help text covers the multiple-of-4 rule and the requested-maximum
    semantics.
- **R8c (docs).**
  - Current-behavior sizes are updated in `README.md`, `docs/ring-buffer.md`,
    and `docs/pooled-intake.md`. The coordinator checked the recomputed
    memory budget arithmetically: 29,595,984 bytes (28.22 MiB) default and
    76,032,848 bytes (72.51 MiB) maximum.
  - The peak-memory and multiple-of-4 wording is updated.
  - The named overlong lines are wrapped.
  - A checkpoint sentence was added to the R2 record.
  - Historical capacity-experiment figures were left unchanged. Some other
    README lines over 100 columns remain outside the named scope.
- **Checks.** The final `cargo test --locked --lib` reported 108 passed and
  4 ignored. fmt, strict Clippy, and `git diff --check` were clean.

### Stage R9

An independent higher-capability review of R8 found no blocker, no major
issue, and no soundness defect. Every R7 finding is resolved except the
end-to-end SCM argument path, which needs live validation by design. The review
recomputed the R8c memory budget and confirmed the elevated tests are set up
correctly (real-time mode with a log file, a drop guard before the start call,
distinct names, and accurate `SAFETY` comments).

The coordinator fixed the review's remaining items directly:

- `docs/pooled-intake.md` now describes the 2–20 ms idle backoff instead of
  the old 2 ms park.
- `README.md` and the `--etw-max-buffers` help text describe the 16 MiB
  default and the product rule as requested peaks rather than hard bounds.
- `SessionConfig` doc comments state that real-time-only page-multiple buffer
  sizes are expected, not guaranteed, to be unadjusted, pending the elevated
  tests.
- The duplicated default in the `--session-name` help text was removed.
- This plan was normalized to CRLF line endings.

One optional nit remains: an elevated test run killed before its drop guard
runs leaves a session behind, so the next elevated run fails once while
starting. After the checks, `cargo test --locked --lib` reported 108 passed and
4 ignored; fmt, strict Clippy, and `git diff --check` were clean.

### Live validation (October 30, 2026, lab VMs)

The release build (SHA-256 `5f7e195a…573b`) was deployed to `DNSTap-WS2025`
(Windows Server 2025 build 26100, 4 logical processors). The original binary
(`CE431467…B656`), the service command line, and the stopped state were
restored afterward. A backup remains under
`C:\ProgramData\EtwDnstap\live-checks-20261030`. Evidence is in
`target/live-checks-20261030/`.

- **Elevated reclaim tests.** All three passed, leaving no sessions or `.etl`
  files.
  - A real-time-only session reports the unchanged `LogFileNameOffset` (2170)
    with an empty name.
  - A real-time session that writes a log file reports
    `LogFileMode=0x800100`, with no file-mode bit, so only the log-file name
    check refuses it. This confirms that the Stage 3 major finding was real
    and that R6 fixes it.
  - A 68 KiB `BufferSize` is reported unchanged.
  - ETW adds `0x800000` (persist on hybrid shutdown) to every session, which
    the mode mask tolerates.
- **Invalid arguments (item 4).** Starting the service with an unknown flag
  writes one Application error entry containing clap's message, with no
  doubled `error:`. SCM logs event 7024 (service-specific error 1) instead of
  error 1053, and no session is left. SCM renders code 1 as "Incorrect
  function"; a dedicated code would read better (cosmetic).
- **Normal start.** The service reached Running. `logman` shows a 64 KiB
  buffer, a 1 s flush timer, and real-time mode.
- **Abrupt kill and reclaim (finding 2).**
  - `Stop-Process -Force` left `EtwDnstap-Lab` running, as expected.
  - Restarting through SCM reclaimed the session, ran with a single session,
    and a clean stop removed it.
  - A receiver packet capture decoded six records (three `CLIENT_QUERY` and
    three `CLIENT_RESPONSE`) on each connection. The killed connection ended
    in a reset; the reclaimed connection ended with STOP and a normal close,
    and the service exited 0.
- **Not run.** The load checks (event loss with the new buffer defaults, drops
  after idle, and the keyword K3 comparison) did not run.
  - The receiver VM's root filesystem is full: `/var/log/syslog` is 8.9 GB and
    earlier campaign directories in `/home/dnstaplab` hold 19 GB.
  - No load generator is installed on either VM.
  - The service's loss counters only reach stderr (review finding 1), so loss
    must be measured externally, with `logman` session statistics and
    receiver record counts.
