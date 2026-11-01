# ETW session recovery and event-ID filter plan

This plan covers two findings from the October 30, 2026 `src` review:

- **Session recovery (finding 2):** a real-time ETW session outlives the process
  that started it. Watchdog aborts, `panic = "abort"`, and pool invariant aborts
  can terminate the service without stopping its session. The default name
  `etw-dnstap-PROCESS_ID` never matches on restart, so leaked sessions
  accumulate toward the system logger limit (usually 64). A fixed
  `--session-name` instead fails every later start with `ERROR_ALREADY_EXISTS`.
- **Event-ID filter (finding 3):** `MatchAnyKeyword` `0x8000000000000000` (superseded by the [keyword narrowing plan](etw-keyword-narrowing-plan.md); now the fallback mask)
  admits the provider's whole analytic channel. Events 258–264+ consume session
  buffers and callback dispatch before the callback discards them, which
  raises `EventsLost` under load.

Microsoft references:
[StartTrace](https://learn.microsoft.com/windows/win32/api/evntrace/nf-evntrace-starttracew),
[ControlTrace](https://learn.microsoft.com/windows/win32/api/evntrace/nf-evntrace-controltracew),
[EVENT_TRACE_PROPERTIES](https://learn.microsoft.com/windows/win32/api/evntrace/ns-evntrace-event_trace_properties),
[EnableTraceEx2](https://learn.microsoft.com/windows/win32/api/evntrace/nf-evntrace-enabletraceex2),
and [EVENT_FILTER_DESCRIPTOR](https://learn.microsoft.com/windows/win32/api/evntprov/ns-evntprov-event_filter_descriptor).

## Stage 0: Baseline

1. Record `git status --short` and SHA-256 hashes of `src/etw.rs` and
   `src/pipeline.rs`. Preserve all uncommitted work; do not commit, stash,
   reset, or revert anything.
2. Confirm `cargo clippy --locked --all-targets -- -D warnings` and
   `cargo test --locked --lib` pass before editing.

## Stage 1: ETW event-ID filter

### Required behavior

- Pass `ENABLE_TRACE_PARAMETERS` (version `ENABLE_TRACE_PARAMETERS_VERSION_2`)
  to `EnableTraceEx2` with one `EVENT_FILTER_DESCRIPTOR` of type
  `EVENT_FILTER_TYPE_EVENT_ID`. Its `EVENT_FILTER_EVENT_ID` payload has
  `FilterIn = true`, `Reserved = 0`, `Count = 2`, and events 256 and 257.
- Keep level 4, `MatchAnyKeyword` `0x8000000000000000` (superseded by the [keyword narrowing plan](etw-keyword-narrowing-plan.md); now the fallback mask), `MatchAllKeyword` 0,
  timeout 5000, and the provider GUID unchanged.
- Define one `SUPPORTED_EVENT_IDS: [u16; 2] = [256, 257]` constant. Both the
  filter payload and the existing `supported()` predicate derive from it so the
  two cannot diverge.
- Keep the callback and parser `supported()` checks as defense in depth. With
  the event-ID filter and `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0`
  (see the [keyword narrowing plan](etw-keyword-narrowing-plan.md)), keyword-zero
  events should not reach the callback; a provider that ignores the filter
  still could.
- Fail startup if `EnableTraceEx2` rejects the filter. Do not silently retry
  without it. Event-ID filtering requires Windows 8.1 / Server 2012 R2 or later.
- Do not add per-event counters or logging on the filtered path; the
  [ETW subscription filter plan](etw-subscription-filter-plan.md) prohibits it.

### Implementation notes

- `windows-sys` 0.61 declares `EVENT_FILTER_EVENT_ID` with a one-element
  flexible `Events: [u16; 1]` array. Use a private `#[repr(C)]` struct with
  `filter_in: bool`, `reserved: u8`, `count: u16`, and `events: [u16; 2]`.
  Prove its layout matches with compile-time `const` assertions on the field
  offsets against `EVENT_FILTER_EVENT_ID` and on the total size (8 bytes,
  equal to `offset_of!(EVENT_FILTER_EVENT_ID, Events) + 2 * size_of::<u16>()`).
- `EVENT_FILTER_DESCRIPTOR.Ptr` is the payload address as `u64`. `Size` is the
  payload size. The payload, descriptor, and parameters are stack locals that
  remain alive for the synchronous `EnableTraceEx2` call. Document this in the
  `SAFETY` comment.
- Put payload and parameter construction in small helpers so unit tests can
  check them without a live session.

### Tests

- Payload bytes are exactly `[1, 0, 2, 0, 0x00, 0x01, 0x01, 0x01]`.
- The descriptor type is `EVENT_FILTER_TYPE_EVENT_ID`, its size is 8, and it
  points at the payload. The parameters use version 2 and one descriptor.
- `supported()` still accepts only 256 and 257. The existing callback filter
  tests continue to pass unchanged.

## Stage 2: Stale session recovery

### Required behavior

- Change the default session name to the fixed `EtwDnstap`. Update the CLI help
  text and the README flag table. `--session-name` remains an override.
- When `StartTraceW` returns `ERROR_ALREADY_EXISTS`, attempt one reclaim:
  1. Query the existing session by name with
     `ControlTraceW(0, name, properties, EVENT_TRACE_CONTROL_QUERY)`.
  2. Reclaim only when the session matches the shape this program creates.
     The query must return `ERROR_SUCCESS`. `LogFileMode` must include
     `EVENT_TRACE_REAL_TIME_MODE`. The session must have no log file
     (`LogFileNameOffset` is zero or names an empty string). `BufferSize` must
     be 64. Do not compare buffer counts, flush timer, or clock type; ETW may
     normalize them. On any mismatch, fail with an error that names the session
     and tells the operator to choose another name or stop it manually.
  3. Stop it by name with `EVENT_TRACE_CONTROL_STOP`. Treat `ERROR_SUCCESS`,
     `ERROR_MORE_DATA` (ETW has already stopped the session), and
     `ERROR_WMI_INSTANCE_NOT_FOUND` (it disappeared concurrently) as stopped.
  4. Retry `StartTraceW` exactly once. A second `ERROR_ALREADY_EXISTS` or any
     other failure is a startup error. Never loop.
- Record the reclaim in a `CaptureCounters` field reported with the existing
  "capture stopped" counters, and emit one `tracing::warn!` at reclaim time.
- In `Capture::stop`, classify `ERROR_MORE_DATA` from the stop call as stopped:
  do not retain the handle for a `Drop` retry or return an error. Statistics
  from that call stay marked invalid. Share one stop-status classifier between
  `Capture::stop` and the reclaim path.
- Put the fingerprint check and the stop-status classifier in pure functions
  over `EVENT_TRACE_PROPERTIES` and `u32` status so they are unit-testable.

### Safety and scope

SCM permits only one running `EtwDnstap` instance, so a session with the default
name and matching shape is a leaked session from an earlier instance. The shape
check protects an unrelated session if an operator chooses a colliding
`--session-name`. Reclaiming runs inside the existing startup watchdog.

Legacy `etw-dnstap-<pid>` sessions left by earlier builds are not reclaimed
automatically. Document one-time cleanup in the README:
`logman query -ets`, then `logman stop etw-dnstap-<pid> -ets` for each.

### Tests

- Fingerprint: accepts the properties built by `SessionProperties::new()`.
  Rejects file-mode sessions, a nonempty log file name, a different buffer
  size, and missing real-time mode.
- Stop classifier: success, more-data, and instance-not-found count as stopped.
  Other codes are failures.
- The CLI default resolves to `EtwDnstap`; an explicit `--session-name` wins.
- Optional: an `#[ignore]`d elevated test that leaks a session with a raw
  `StartTraceW` and then proves `Capture::start` reclaims it. Do not run it
  unelevated.

## Stage 3: Independent review

A higher-capability reviewer reads the full Stage 1 and Stage 2 diff against
this plan. The review checks:

- FFI layout, pointer lifetimes, and `SAFETY` comments;
- reclaim correctness, including races and the risk of stopping a foreign
  session;
- that `supported()` and the filter payload share one constant;
- `ERROR_MORE_DATA` handling and its effect on `Drop` retries;
- test adequacy and documentation accuracy.

Findings are ranked by severity with file and line references.

## Stage 4: Remediation

Fix confirmed review findings and rerun the full local checks. Record
dispositions for findings left unfixed.

## Stage 5: Live validation (requires separate authorization)

This stage is not part of this change. It needs an authorized deployment on the
DNS lab VM.

- Under a recursive workload, the session should no longer deliver
  258/260/261 events. Confirm with a WPR trace or a parallel `logman` session
  rather than a per-event counter. Compare `etw_lost` and `etw_buffers_lost`
  with the pooled-intake baseline at the same load.
- Kill the service process (`taskkill /f`) and restart it through SCM. Startup
  should reclaim `EtwDnstap`, report the reclaim counter, and capture normally.
- Start an unrelated file-mode session named `EtwDnstap` and confirm startup
  refuses to stop it.

## Delegation

- Stage 1 and Stage 2 run sequentially in the main worktree, because both edit
  `Capture::start` and the uncommitted parser work is not in `HEAD`. Each runs
  on a lower-cost implementation agent at medium effort.
- Stage 3 runs on a higher-capability review agent at high effort.
- No agent commits, pushes, deploys, or runs elevated or live tests.

## Completion record

### Stage 1

Changed `src/etw.rs`:

- Added `SUPPORTED_EVENT_IDS: [u16; 2] = [256, 257]`; `supported()` now
  derives from it with `contains` (no allocation).
- Added a private `#[repr(C)]` `EventIdFilterPayload` with compile-time
  offset and size assertions against `EVENT_FILTER_EVENT_ID` (8 bytes), plus
  `EventIdFilterPayload::new()`, `event_id_descriptor()`, and
  `enable_parameters()` (version 2, one descriptor).
- `Capture::start` passes the parameters to `EnableTraceEx2`; level,
  keywords, timeout, and GUID are unchanged. A rejected filter fails startup
  with no retry. The callback and parser checks are unchanged. No per-event
  counters or logging were added.
- README ETW subscription paragraph notes the event-ID filter.

Tests added: exact payload bytes, descriptor type/size/pointer and parameter
version/count, and `supported()` accepting only 256 and 257 across all `u16`
values.

Checks: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D
warnings`, and `git diff --check -- src/etw.rs` pass. `cargo test --locked
--lib`: 84 passed, 0 failed, 1 ignored (baseline 81 passed, 1 ignored).

Limitation: live provider delivery with the filter was not verified; no
elevated or live ETW test was run.

### Stage 2

Changed `src/etw.rs`:

- `reclaimable(&SessionProperties)` (pure fingerprint): real-time bit set, no
  sequential/circular/append/newfile bits, `BufferSize == 64`
  (`SESSION_BUFFER_KIB`, also used by `SessionProperties::new()`), and
  `log_file_name_is_empty` (offset zero, or a bounds-checked, 2-byte-aligned
  offset into the trailing name storage that holds an empty string; safe code
  only). The check takes `&SessionProperties` rather than a bare
  `EVENT_TRACE_PROPERTIES` because it must read the trailing name storage.
- `stop_outcome(u32) -> StopOutcome { Stopped, Failed }`: success, `ERROR_MORE_DATA`,
  and `ERROR_WMI_INSTANCE_NOT_FOUND` are `Stopped`. Shared by `Capture::stop`
  (so `ERROR_MORE_DATA` no longer retains the handle or returns an error;
  statistics stay invalid for that status) and the reclaim path.
- `start_session` wraps `StartTraceW`; `reclaim_stale_session` queries by name,
  checks the fingerprint, stops by name, bumps
  `CaptureCounters::stale_sessions_reclaimed`, and emits one `tracing::warn!`
  (session name only). `Capture::start` calls it only on `ERROR_ALREADY_EXISTS`
  and retries `StartTraceW` exactly once, before any handle exists. A mismatch
  error names the session and suggests another `--session-name` or
  `logman stop NAME -ets`.

Changed `src/pipeline.rs`: `DEFAULT_SESSION_NAME = "EtwDnstap"` replaces
`etw-dnstap-PID`; help text updated; `stale_sessions_reclaimed` added to the
"capture stopped" log. README: flag table default and a reclaim/legacy-cleanup
note.

Tests added: fingerprint accepts the default shape (and normalized counts with
an empty log file name), rejects file modes, missing real-time mode, other
buffer size, nonempty/out-of-range/misaligned log file offsets; stop classifier;
CLI default name and explicit override. The optional elevated test was not
added.

Checks: `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D
warnings`, and `git diff --check -- src/etw.rs src/pipeline.rs` pass. `cargo
test --locked --lib`: 88 passed, 0 failed, 1 ignored (Stage 1: 84 passed).

Limitations: live reclaim is unverified; no elevated or live ETW test was run.
The exact `LogFileNameOffset` and `ERROR_MORE_DATA` behavior of a real-time
QUERY with this fixed-size buffer is assumed from the documentation; a
mismatch fails safe (startup error, no stop).

### Stage 3

An independent higher-capability review found Stage 1 sound and Stage 2 in
need of one fix before live validation. Clippy, formatting, and
`cargo test --locked --lib` (88 passed, 1 ignored) were clean.

- **Major, needs live confirmation:** `reclaim_stale_session` queries with
  `SessionProperties::new()`, which has no log-file-name storage and
  `LogFileNameOffset = 0`. ControlTrace documents setting both name offsets for
  QUERY/STOP. If ETW reports the offset past the buffer, startup refuses its own
  leaked session. If ETW leaves the offset at zero, a real-time session that
  also writes a log file passes the fingerprint. Fix: query with a dedicated
  struct that has logger-name and log-file-name storage and both offsets set,
  and require an unchanged offset and an empty log file name. Add the
  `#[ignore]`d elevated reclaim test.
- **Minor:** handle `ERROR_WMI_INSTANCE_NOT_FOUND` from the query as already
  gone, and report other query failures separately from a shape mismatch.
- **Minor:** event-ID filtering is applied by the ETW runtime, not the
  provider. It reduces session buffer use and callback dispatch, but not DNS
  Server's cost of generating events. Correct the README and plan wording.
- **Minor:** the tests mirror the default-name expression instead of the
  production path, and fingerprint tests use input rather than query-shaped
  properties. Make clap the single source of the default.
- **Nits:** also reject system, buffering, and private logger mode bits. Note
  that `ERROR_ACTIVE_CONNECTIONS` on stop fails safe. Fix the stale "existing
  name is an error" comment and the README "unused" wording.

Stage 4 remediation is pending.

Wording correction: the Stage 1 heading and text now call the event-ID filter
an ETW runtime filter, not provider-side. Stage 3 findings 1-3 were fixed in
`src-review-remediation-plan.md` Stage R6.
