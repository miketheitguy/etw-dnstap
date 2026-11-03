# Windows performance counters (V2) plan

> **Status:** Historical (executed and lab-validated 2026-11-01) · **Evidence dated:** 2026-11-01 · **Commit:** planned on top of `7ae60a6` · **Reviewed:** 2026-11-01

This plan adds a Windows Performance Counters V2 provider to the service. It
replaces the earlier nine-counter proposal in this file. That proposal predated
the periodic ETW query, `queue_depth_max`, and the 200,000, 10,000, and 3,000
queries-per-second lab runs of 2026-11-01. Those runs showed which values
predict loss, and this catalog is built around them.

## Goals

- Let an administrator watch capture health in Performance Monitor, `typeperf`,
  `logman` data collector sets, or WMI (`Win32_PerfRawData_*`), with no log
  parsing.
- Expose the leading indicators of loss, not only the loss totals:
  - the channel fill level against `--capacity`
  - ETW buffers in use against the ETW maximum
- Add no work to the ETW callback or the parser loop.
- A missing or failed counter registration never stops capture.

## Non-goals

- No new per-event instrumentation, timing, or histograms.
- No remote collection setup. PerfLib V2 serves local consumers and remote
  consumers through its own RPC path. The service does nothing for either.
- No change to the `capture stats` and `output stats` log events.

## Sources

The design follows these Microsoft Learn pages:

- [Providing Counter Data Using Version 2.0](https://learn.microsoft.com/windows/win32/perfctrs/providing-counter-data-using-version-2-0)
- [Performance Counters Schema](https://learn.microsoft.com/windows/win32/perfctrs/performance-counters-schema),
  [counterSet](https://learn.microsoft.com/windows/win32/perfctrs/performance-counters-counterset-complex-type),
  and [counter](https://learn.microsoft.com/windows/win32/perfctrs/performance-counters-counter-complex-type)
- [CTRPP](https://learn.microsoft.com/windows/win32/perfctrs/ctrpp)
- [About Performance Counters](https://learn.microsoft.com/windows/win32/perfctrs/about-performance-counters)
  for counter types and the stateless-provider rule
- [PerfStartProviderEx](https://learn.microsoft.com/windows/win32/api/perflib/nf-perflib-perfstartproviderex),
  [PerfSetCounterSetInfo](https://learn.microsoft.com/windows/win32/api/perflib/nf-perflib-perfsetcountersetinfo),
  [PerfCreateInstance](https://learn.microsoft.com/windows/win32/api/perflib/nf-perflib-perfcreateinstance),
  [PerfSetULongLongCounterValue](https://learn.microsoft.com/windows/win32/api/perflib/nf-perflib-perfsetulonglongcountervalue)

Key facts from those pages:

- A V2 provider is defined by a `.man` manifest. `lodctr /m:` registers it under
  `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Perflib\_V2Providers\{ProviderGuid}`.
  `unlodctr /m:` removes it. A changed counter set must be unregistered before it
  is registered again.
- The counter names and descriptions are string resources. CTRPP generates them
  as an `.rc` file. They must be linked into the binary that the manifest's
  `applicationIdentity` names. Consumers load the strings from that binary.
- The provider calls `PerfStartProvider`, `PerfSetCounterSetInfo`,
  `PerfCreateInstance`, then sets values with `PerfSetULongLongCounterValue`. It
  calls `PerfDeleteInstance` and `PerfStopProvider` before it exits.
- PERFLIB copies the instance block on a worker thread when a consumer
  collects. Collection goes through `PCW.sys`. The provider needs no IPC.
- Providers must behave as stateless. They never reset a value when a consumer
  reads it. Rates come from `PERF_COUNTER_BULK_COUNT`: the consumer divides the
  delta of a 64-bit total by the elapsed time.

## Architecture

```
ETW callback ──► CaptureCounters (atomics) ◄── ETW session query (1 s)
parser loop ───► ParseCounters  (atomics)            │
TCP sink ──────► SinkStats      (atomics)            │
                         │  relaxed loads, 1 s       │
                         ▼                           │
              supervisor thread (existing 50 ms loop)┘
                         │  CounterValues snapshot
                         ▼
              PerfPublisher ──PerfSetULongLongCounterValue──► PERFLIB instance block
                                                                  │
                         Performance Monitor / typeperf / WMI ◄───┘ (PCW.sys)
```

- **Hybrid update model.** A counter whose value is exactly one existing
  `AtomicU64` is read by reference. `PerfSetCounterRefValue` points PERFLIB at
  the atomic once, when the instance is created. PERFLIB then reads the live
  value at each collection. Rates and totals are therefore exact at the moment
  of collection, and the counter needs no publishing. A counter whose value is
  derived (depth, ETW buffers in use, fractions, a boolean, the capacity) is set
  by value with `PerfSetULongLongCounterValue` once a second.
- **Why by reference for rates.** A by-value total published once a second and
  sampled once a second by a consumer on its own timer gives some intervals with
  no publication and some with two. The rate then reads 0 and then twice the
  true value. A reference has no such beat.
- **Reference safety.** PERFLIB reads the referenced `AtomicU64` with a plain
  load on its own thread. Microsoft Learn (Interlocked Variable Access) states
  that simple reads and writes to properly aligned 64-bit variables are atomic
  on 64-bit Windows. `AtomicU64` is 8 bytes with 8-byte alignment, and Rust
  writes it only with atomic operations. The module does not build for 32-bit
  targets. The publisher owns `Arc` clones of the counter groups, and its drop
  order is fixed: `PerfDeleteInstance`, then `PerfStopProvider`, then the `Arc`s
  are released. PERFLIB can therefore never read freed memory.
- **Publisher thread.** The existing supervisor thread publishes the by-value
  counters. It already wakes every 50 ms and owns references to every counter
  group. A 1-second `Schedule` adds the publication. No new thread, no
  `ControlCallback`, and no global state are needed.
- **Hot paths unchanged.** The ETW callback and the parser loop gain no stores,
  branches, or calls. They keep the same atomic increments.
- **ETW gauges.** The supervisor runs `query_statistics()` each publication
  tick, as it already does for each `capture stats` tick. A query costs one
  `ControlTraceW` call. The query also stores `NumberOfBuffers`, `FreeBuffers`,
  and `MaximumBuffers` in three new `CaptureCounters` atomics.
- **Channel depth.** The publisher derives the depth from the existing ledger
  `received = intake_dropped + callback_errors + dequeued + backlog`. It clamps
  the result to `0..=capacity`. It does not clone a channel endpoint, because a
  cloned `Sender` or `Receiver` would change the channel's disconnect semantics.
- **Instances.** The counter set is multi-instance. The instance name is the
  ETW session name (`--session-name`, default `EtwDnstap`). ETW session names are
  unique on a host, so the name is unique and stable across restarts, as the
  instance-name rule requires. Counters restart from zero with the service.
- **Failure policy.** If any registration call fails, the service logs one
  warning naming the call and the Win32 status, then captures without counters.
  A failed `PerfSetULongLongCounterValue` is ignored. An unregistered manifest
  is not a failure: the provider calls succeed, and consumers cannot find the
  counter set by name.

## Counter catalog

Counter set `EtwDnstap`, multi-instance. Every counter is 8 bytes. The ID is the
`id` attribute in the manifest and the `CounterId` in Rust. IDs are frozen once
released. Types are the manifest names.

Update model: IDs 1 to 3, 10 to 21, 32, and 51 are by reference. Each one carries
`<counterAttribute name="reference"/>` in the manifest. IDs 30, 31, 33, 34, 40 to
44, and 50 are by value, published once a second.

### Throughput rates

| ID | Name | Type | Source |
| --- | --- | --- | --- |
| 1 | Events Received/sec | `perf_counter_bulk_count` | `CaptureCounters.received` |
| 2 | Frames Sent/sec | `perf_counter_bulk_count` | `ParseCounters.encoded` |
| 3 | Events Dropped/sec | `perf_counter_bulk_count` | `CaptureCounters.intake_dropped` |

### Totals since the service started

| ID | Name | Type | Detail | Source |
| --- | --- | --- | --- | --- |
| 10 | Events Received | `perf_counter_large_rawcount` | standard | `CaptureCounters.received` |
| 11 | Frames Sent | `perf_counter_large_rawcount` | standard | `ParseCounters.encoded` |
| 12 | Events Dropped | `perf_counter_large_rawcount` | standard | `CaptureCounters.intake_dropped` |
| 13 | Channel Full Drops | `perf_counter_large_rawcount` | standard | `CaptureCounters.dropped` (`ring_dropped`) |
| 14 | Pool Exhausted Drops | `perf_counter_large_rawcount` | advanced | `CaptureCounters.pool_exhausted` |
| 15 | Jumbo Pool Exhausted Drops | `perf_counter_large_rawcount` | advanced | `CaptureCounters.pool_exhausted_jumbo` |
| 16 | ETW Events Lost | `perf_counter_large_rawcount` | standard | `CaptureCounters.etw_lost` |
| 17 | ETW Buffers Lost | `perf_counter_large_rawcount` | standard | `CaptureCounters.etw_buffers_lost` |
| 18 | Receiver Down Drops | `perf_counter_large_rawcount` | standard | `SinkStats.disconnected_dropped` |
| 19 | Events Rejected | `perf_counter_large_rawcount` | advanced | `ParseCounters.rejected` |
| 20 | Callback Errors | `perf_counter_large_rawcount` | advanced | `CaptureCounters.callback_errors` |
| 21 | Receiver Disconnects | `perf_counter_large_rawcount` | standard | `SinkStats.disconnects` |

`Events Dropped` is the total of callback admission drops: channel full, pool
exhausted, and in-process receiver disconnected. IDs 13 to 15 are parts of it.
ETW losses (16, 17) and receiver-down drops (18) are separate losses. Add 12, 16,
and 18 for all losses that the service can count.

### Gauges

| ID | Name | Type | Detail | Source |
| --- | --- | --- | --- | --- |
| 30 | Channel Depth | `perf_counter_large_rawcount` | standard | derived backlog (see Architecture) |
| 31 | Channel Capacity | `perf_counter_large_rawcount` | standard | `--capacity` |
| 32 | Channel Peak Depth | `perf_counter_large_rawcount` | standard | `ParseCounters.queue_depth_max` |
| 33 | % Channel Used | `perf_large_raw_fraction`, `baseID="34"` | standard | depth |
| 34 | (base of 33) | `perf_large_raw_base`, `noDisplay` | | `--capacity` |
| 40 | ETW Buffers Allocated | `perf_counter_large_rawcount` | standard | `NumberOfBuffers` |
| 41 | ETW Buffers In Use | `perf_counter_large_rawcount` | standard | `NumberOfBuffers - FreeBuffers`, saturating |
| 42 | ETW Buffers Maximum | `perf_counter_large_rawcount` | standard | `MaximumBuffers` as ETW reports it |
| 43 | % ETW Buffers Used | `perf_large_raw_fraction`, `baseID="44"` | standard | in use |
| 44 | (base of 43) | `perf_large_raw_base`, `noDisplay` | | maximum |
| 50 | Receiver Connected | `perf_counter_large_rawcount` | standard | `SinkStats.connected`: 1 or 0 |
| 51 | Receiver Reconnects | `perf_counter_large_rawcount` | advanced | `SinkStats.reconnects` |

The ETW gauges come from the last successful query. They read 0 until the first
query succeeds.

### Why these counters

- **% Channel Used** and **% ETW Buffers Used** are the leading indicators. In
  the 200,000 queries-per-second runs, ring drops followed the channel reaching
  `--capacity`, and `etw_lost` followed ETW buffers in use reaching the maximum.
  At 10,000 queries per second, ETW used 16 buffers in a spike against a default
  maximum of 12.
- **Events Dropped/sec**, **ETW Events Lost**, and **Receiver Down Drops** are
  the loss alarms. Each one should read 0.
- **Events Received/sec** and **Frames Sent/sec** show throughput. They track
  each other when nothing is lost.
- **Channel Peak Depth** keeps the worst moment that a 1-second sample can miss.

Suggested alerts: `% Channel Used` above 75, `% ETW Buffers Used` above 75, any
increase in `Events Dropped`, `ETW Events Lost`, or `Receiver Down Drops`, and
`Receiver Connected` at 0.

## Manifest and identifiers

| Item | Value |
| --- | --- |
| Manifest | `packaging/perfcounters/etw-dnstap.man` |
| Provider name | `EtwDnstap` |
| Provider GUID | `{A7F0B1F4-C87D-4267-979F-594A4AB2145D}` |
| Provider symbol | `EtwDnstapProvider` |
| `providerType` | `userMode` |
| `applicationIdentity` | `etw-dnstap.exe` |
| Counter set name | `EtwDnstap` |
| Counter set GUID | `{B02EDFA2-B961-4E06-89D2-56C95ECB336C}` |
| Counter set symbol | `EtwDnstapPipeline` |
| `instances` | `multiple` |
| Counter URIs | `EtwDnstap.Pipeline.<CamelCaseName>` |

The provider GUID is new. It is not the DNS Server ETW provider GUID and not the
session GUID.

## Build

- Generate the resources once with CTRPP from the Windows SDK and commit the
  output:
  `ctrpp -o target/ctrpp/etw-dnstap-perf.h -rc packaging/perfcounters/etw-dnstap-perf.rc packaging/perfcounters/etw-dnstap.man`.
  Commit only the `.rc`. The `.h` is C helper code that the Rust provider does
  not use. Check the generated `.rc` for host paths before commit.
- Add a `build.rs` that compiles the committed `.rc` with the `embed-resource`
  crate (a build dependency) and links it into the executable. Building already
  requires the Windows SDK, which provides `rc.exe`.
- Add the `Win32_System_Performance` feature to `windows-sys`. The counter type
  values are not in `windows-sys`. Define them in Rust from `winperf.h`:
  - `PERF_COUNTER_BULK_COUNT = 0x1041_0500`
  - `PERF_COUNTER_LARGE_RAWCOUNT = 0x0001_0100`
  - `PERF_LARGE_RAW_FRACTION = 0x2002_0500`
  - `PERF_LARGE_RAW_BASE = 0x4003_0500`
- A unit test reads the manifest with `include_str!` and checks that every Rust
  counter has the same ID and type as the manifest, and that the counts match.
- `scripts/ci/verify-pe.ps1` must still pass on the release binary.

## Code layout

- `src/perfcounters.rs` (new):
  - The counter table, `[CounterDef; N]` with ID, type, and detail level, in
    manifest order.
  - Each `CounterDef` has a `by_reference` flag. The manifest test checks that
    exactly these counters carry the `reference` attribute.
  - `CounterValues`: plain `u64` fields for the by-value counters only.
  - `fn snapshot(capture, parse, sink, capacity) -> CounterValues`: pure, unit
    tested. It covers the depth derivation and clamp, the saturating in-use
    count, and booleans as 0 or 1.
  - `PerfPublisher`:
    - `start(instance_name, capture, parse, sink) -> Result<Self>` runs
      `PerfStartProviderEx` with no callback, then `PerfSetCounterSetInfo` with a
      `#[repr(C)]` template (`PERF_COUNTERSET_INFO` followed by
      `PERF_COUNTER_INFO[N]`, `Offset = 8 * index`, `Size = 8`), then
      `PerfCreateInstance` with instance ID 0. It then sets each by-reference
      counter once with `PerfSetCounterRefValue` and `AtomicU64::as_ptr()`. If
      any step fails, it undoes the earlier steps and returns the error.
    - It owns `Arc` clones of `CaptureCounters`, `ParseCounters`, and
      `SinkStats`.
    - `publish(&self, &CounterValues)` sets each by-value counter.
    - `Drop` runs `PerfDeleteInstance` then `PerfStopProvider`, before the
      `Arc` fields are released.
    - Compile-time checks: `AtomicU64` is 8 bytes with 8-byte alignment, and the
      target is 64-bit.
    - The type is neither `Send` nor `Sync`. It lives on the supervisor thread.
- `src/etw.rs`, `src/etw/session.rs`: add `etw_buffers_allocated`,
  `etw_buffers_free`, and `etw_buffers_maximum` to `CaptureCounters`.
  `store_statistics` stores them on success.
- `src/pipeline.rs`: start the publisher after capture starts. Add a 1-second
  `publish` schedule to `Monitor`. On each tick, run `query_statistics()` and
  then `publish`. Drop the publisher after the workers join, so the instance
  disappears when capture ends.
- Every `unsafe` block carries a `SAFETY:` comment, and every FFI status is
  checked. Nothing in this path panics.

## Installation

- Copy `etw-dnstap.man` into `C:\Program Files\EtwDnstap\` next to the
  executable.
- Register it as an administrator:
  `lodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man" "C:\Program Files\EtwDnstap"`.
- Unregister before uninstall, or before replacing a manifest whose counters
  changed: `unlodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man"`.
- Read the counters with
  `typeperf "\EtwDnstap(*)\*" -sc 5`, or in Performance Monitor under
  **EtwDnstap**.

## Risks and checks

| Risk | Check |
| --- | --- |
| The LocalService restricted-SID token cannot register a provider or create an instance | Lab: install, start, and read the counters with `typeperf`. Look for the warning event. |
| Consumers cannot load the strings, so counters show no names | Lab: `typeperf -q EtwDnstap` lists the names. Inspect `ApplicationIdentity` under the `_V2Providers` key. |
| Fraction counters display wrongly | Lab: `% Channel Used` agrees with Channel Depth / Channel Capacity under load. |
| The publication costs measurable CPU | 10 setter calls and one `ControlTraceW` per second. Compare service CPU under the 10,000 queries-per-second load. |
| PERFLIB reads a referenced counter after it is freed | Drop order in `PerfPublisher` (delete instance, stop provider, release the `Arc`s), with a test that the counter groups outlive the publisher. |
| A by-reference rate reads wrongly | Lab: `Events Received/sec` agrees with the client rate and does not alternate between 0 and twice the rate. |
| Manifest and Rust drift apart | The unit test from the Build section. |

## Development plan

| Step | Owner | Output |
| --- | --- | --- |
| 1 | Rust implementation agent | Manifest, CTRPP `.rc`, `build.rs`, `src/perfcounters.rs`, the `CaptureCounters` and `store_statistics` changes, supervisor wiring, unit tests. `cargo build --release`, `cargo test`, and `cargo clippy --all-targets` are clean. |
| 2 | Documentation agent, in parallel with 1 | `docs/guide/install.md` (copy and register the manifest), `docs/guide/uninstall.md` (unregister), `docs/guide/configuration.md` (a performance counters section with the catalog and alerts), `docs/README.md` index. |
| 3 | Orchestrator | Build check, `verify-pe.ps1`, the host-path check, then lab validation on DNSTap-WS2025 with the LocalService restricted profile and a 10,000 queries-per-second load. |
| 4 | Code-review agent | Review of the full diff, with emphasis on FFI safety, failure handling, and hot-path impact. |
| 5 | Orchestrator | Fix the confirmed findings, re-run the checks, and report. |

## Lab results

Run on 2026-11-01 on DNSTap-WS2025 (6 vCPUs, Windows Server 2025). The service
ran as LocalService with a restricted service SID, with
`--etw-buffer-kib 16 --etw-max-buffers 32 --capacity 8192`.

- `lodctr /m:` succeeded and recorded the installed executable as
  `ApplicationIdentity`. An administrator's `typeperf -q EtwDnstap` listed all
  counters by name, so the string resources load.
- In a 60-second run at 10,000 queries per second, the by-reference totals
  matched the `capture stopped` event exactly: received 1,200,000, `ring_dropped`
  10,751, and `queue_depth_max` 8,193. Events Received/sec read 19,700 to 21,000
  in steady seconds. It did not alternate between 0 and twice the rate.
- The gauges showed the cause of the drops. ETW skipped one delivery, and the
  next second delivered about 40,000 events. `% Channel Used` read 98% in one
  sample.
- The instance disappeared when the service stopped.

## Review outcome

An independent code review found no blocker in the FFI, lifetime, or concurrency
code. It found the following, which are fixed:

- An unregistered manifest is not a failure. The docs and the warning said
  otherwise.
- `ParseCounters.output_errors` cannot change while capture runs, because the
  network sink never returns a write or flush error. ID 21 is now Receiver
  Disconnects, from `SinkStats.disconnects`.
- The ETW maximum was 0 until the first 1-second tick. One query now runs before
  the publisher starts.
- The tests now check the exact fraction and base pairs, and the committed `.rc`
  strings against the manifest.

## Open items

- A non-admin member of Performance Monitor Users has not read the counters.
  Consumers load the names from `etw-dnstap.exe`, and the install ACL does not
  grant that group read access. Test it, then either grant read on the
  executable or document the limit.
- Remote collection and WMI are not tested.
- The committed `.rc` keeps the CTRPP header text, which says not to commit the
  file. This repository commits it on purpose, so that the build needs only
  `rc.exe` and not CTRPP. The drift test protects it.
- Fuzz builds now run `build.rs` and need `rc.exe`.
