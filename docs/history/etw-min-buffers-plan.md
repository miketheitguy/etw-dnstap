# `--etw-min-buffers` plan

> **Status:** Historical (executed and lab-validated 2026-11-01) · **Evidence dated:** 2026-11-01 · **Commit:** planned on top of `7c62552` with the performance counter change · **Reviewed:** 2026-11-01

This plan adds a command-line option that sets the ETW session's
`MinimumBuffers`. Today the service always requests a hard-coded 4
(`src/etw/session.rs`, `SessionProperties::new`). After this change the code
holds no fixed minimum. The operator chooses the value, and 0 lets ETW choose.

## Why

- ETW reserves the (adjusted) minimum number of buffers when the session starts.
  It allocates more only on demand, up to `MaximumBuffers`.
- On-demand allocation happens during the burst that needs the buffers. In the
  200,000 queries-per-second lab runs of 2026-11-01, ETW grew from 16 to 32
  buffers within one 250 ms sample. In the 10,000 queries-per-second run, buffers
  in use reached 16, against an ETW minimum of 12.
- Microsoft Learn (`EVENT_TRACE_PROPERTIES`, `MinimumBuffers` and
  `MaximumBuffers`): "Most users should start tuning their session by setting
  MinimumBuffers and MaximumBuffers to the same value. You might then increase
  the value of MaximumBuffers if the trace drops events during event rate
  peaks."
- A minimum lets an operator reserve buffers up front for a known peak. They
  are then not allocated during the burst.

The option does not change real-time delivery timing. It does not fix the
channel overflow after a late ETW delivery. That needs `--capacity`.

## ETW behavior to document

From Microsoft Learn (`EVENT_TRACE_PROPERTIES`, `MinimumBuffers` and
`MaximumBuffers`):

- 0 is a valid request. For real-time traces with moderate rates, "most users
  should minimize memory usage by setting MinimumBuffers to 0 or to a small
  minimum (e.g. 4 or 8), allowing ETW to adjust the value upwards based on the
  number of processors."
- ETW raises `MinimumBuffers` to at least 2 per logical processor for a session
  without `EVENT_TRACE_NO_PER_PROCESSOR_BUFFERING`. This service does not set that
  flag.
- ETW may reduce a minimum that exceeds an ETW-determined limit.
- If `MaximumBuffers` is below the adjusted minimum, ETW may raise it.
- The minimum cannot change while the session runs (TraceView: "You cannot
  change this value while the trace session is running"). The service always
  starts a new session, so a new value takes effect at the next service start.
- The reserved minimum is allocated at session start. It is memory that the
  session holds for its whole life: `minimum × --etw-buffer-kib`.

## Option

| Item | Value |
| --- | --- |
| Name | `--etw-min-buffers N` |
| Default | `0`: ETW chooses, at least 2 per logical processor. On any host with 2 or more logical processors this gives the same session as today's fixed 4. On a 1-processor host ETW reserves 2 instead of 4. |
| Range | 0 to 4,096. The upper bound matches `--etw-max-buffers`. There is no lower bound beyond 0, because ETW applies its own per-processor floor. |
| Cross-check | `N` must not exceed `--etw-max-buffers`. A larger value is a startup error with a clear message. ETW would silently raise the maximum, and the operator's maximum would no longer mean anything. |
| Help text | Requested minimum ETW buffers, reserved when the session starts, 0-4096 and at most `--etw-max-buffers`. 0 lets ETW choose. ETW raises the value to at least two per logical processor. |

## Code changes

1. `src/etw/session.rs`:
   - Add `minimum_buffers: u32` to `SessionConfig`, with a doc comment covering
     0 (ETW chooses), the ETW adjustment, and the up-front reservation.
     Default 0.
   - `SessionConfig::new` takes `minimum_buffers` and checks
     `(0..=MAX_BUFFERS)` and `minimum_buffers <= maximum_buffers`. The error text
     names both values.
   - The defensive memory cap keeps using `buffer_kib * maximum_buffers`.
     Because the minimum is at most the maximum, that product still bounds the
     request.
   - `SessionProperties::new` sets `MinimumBuffers = config.minimum_buffers`.
     The literal 4 goes away. No other code assumes a minimum.
   - `reclaimable` stays unchanged. Buffer counts are not part of the shape.
2. `src/pipeline.rs`:
   - Add `etw_min_buffers` to `CaptureArgs`, next to `etw_max_buffers`, with the
     clap range 0..=4096 and default 0.
   - Pass it to `SessionConfig::new` in `TryFrom<CaptureArgs>` and in the
     validation path that calls `SessionConfig::new`.
   - Add `etw_min_buffers` (the request) to the `capture started` log event,
     before `etw_max_buffers`.
   - Log the values that ETW applied. The ETW query already runs once before
     the performance counter publisher starts. Store the queried
     `MinimumBuffers` in a new `CaptureCounters` atomic, `etw_buffers_minimum`,
     beside `etw_buffers_maximum`. After that first query, log one info event,
     `ETW buffers applied`, with `etw_min_buffers_applied`,
     `etw_max_buffers_applied`, and `etw_buffers_allocated`. If the query fails,
     log nothing extra. The existing counters and the stats line are unchanged.
   - `uses_default_etw_settings` keeps comparing the whole `SessionConfig`,
     whose default minimum is now 0.
     Any non-default minimum therefore counts as tuned, and the low-rate warning
     does not fire. That is intended.
3. No change to the ETW callback, the parser loop, the pool, or the performance
   counters. `ETW Buffers Allocated` already shows the effective minimum just
   after the session starts.

## Tests

- `SessionConfig::new`:
  - accepts min = 0, min = max, and min = 4096 with max = 4096
  - rejects min > 4096 and min > max, with the error text checked
  - the default has `minimum_buffers == 0`
- `SessionProperties::new` copies `minimum_buffers` into `MinimumBuffers`.
- CLI:
  - the default is 0
  - `--etw-min-buffers 32 --etw-max-buffers 64` parses
  - `--etw-min-buffers 65 --etw-max-buffers 64` fails at validation
  - the range bounds are enforced (4097 is rejected)
- `store_statistics` stores `MinimumBuffers` on success.
- `uses_default_etw_settings` is false when only the minimum differs.
- Update every existing caller and test of `SessionConfig::new` for the new
  parameter.

## Documentation

- `docs/guide/configuration.md`:
  - Add a row to the options table.
  - Extend "ETW buffer minimums" with: what the option reserves, 0 as "ETW
    chooses", the ETW adjustment to 2 per logical processor and the possible
    cap, the up-front memory, the `min <= max` rule, the `ETW buffers applied`
    log event, and Microsoft's advice to start tuning with the two values
    equal. Remove wording that presents 4 as a fixed request.
  - Mention that `ETW Buffers Allocated` shows the effective minimum.
  - Do not add new lab claims.
- `CHANGELOG.md`: an `[Unreleased]` → `Added` entry.
- Any other document that lists every command-line option. Find them with
  `grep -rn "etw-max-buffers" README.md docs/guide`.

## Checks

- `cargo build --locked --release --target x86_64-pc-windows-msvc`
- `cargo test --locked --target x86_64-pc-windows-msvc`, and with
  `--features diag-timing`
- `cargo clippy --locked --all-targets --target x86_64-pc-windows-msvc -- -D warnings`,
  and the `diag-timing` and `fuzzing` variants
- `cargo fmt --check`
- `scripts/ci/verify-pe.ps1` on the release executable
- `python -I scripts/ci/linkcheck.py`
- the host-path check from `CLAUDE.md`

## Lab check

This step is run by the orchestrator, not the implementer.

1. Deploy with the defaults. The `ETW buffers applied` event shows a minimum
   of 12 on the 6-vCPU VM.
2. Deploy with `--etw-min-buffers 32 --etw-max-buffers 64`. `ETW Buffers
   Allocated` reads 32 at idle. The `capture started` event shows
   `etw_min_buffers=32`, and `ETW buffers applied` shows 32.
3. Deploy with `--etw-min-buffers 65 --etw-max-buffers 64`. The service fails
   to start with the validation message.

## Lab results

Run on 2026-11-01 on DNSTap-WS2025 (6 vCPUs). The service ran as LocalService
with a restricted service SID.

| Options | Result |
| --- | --- |
| Defaults (`--etw-min-buffers 0`, max 12) | `ETW buffers applied`: minimum 12, maximum 12, allocated 12. ETW applied 2 per logical processor. |
| `--etw-min-buffers 32 --etw-max-buffers 64` | `ETW buffers applied`: minimum 32, maximum 64, allocated 32. The ETW Buffers Allocated counter read 32 at idle. |
| `--etw-min-buffers 65 --etw-max-buffers 64` | The service did not start. Event ID 1: `ETW minimum buffers (65) must not exceed ETW maximum buffers (64)`. |

The startup failure was logged twice as Event ID 1. The lab did not check
whether other startup validation errors are also logged twice.
