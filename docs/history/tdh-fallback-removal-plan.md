# TDH fallback removal plan

> **Status:** Historical (executed 2026-11-01) · **Evidence dated:** not applicable · **Commit:** written at `21e31c0` · **Reviewed:** 2026-11-01

## Goal

Remove the per-event TDH fallback reader from `src/etw/schema.rs`. An event
whose manifest schema does not compile into a single-pass `DecodePlan` is then
rejected and counted as `schema_rejected`. It is no longer decoded with
`TdhGetPropertySize` and `TdhGetProperty` on every event.

`TdhGetEventInformation` stays. The service still reads each (event ID,
version) schema once, to compile the plan and to check the provider, the event
and the manifest source. Only the per-event property reads go.

## Why

- **One decoding path.** The fast path is the one that is fuzzed, has golden
  tests, and is allocation-free. A schema that falls back silently costs several
  TDH calls on every event, and nothing in the service shows that it happens.
  The performance review lists this as finding 5.
- **Less native surface.** The change removes two TDH entry points per event.
  It also removes the `PROPERTY_DATA_DESCRIPTOR` name pointers, a 64 KiB packet
  buffer per parser, and about 120 lines of `unsafe`-adjacent code.
- **Failures are visible.** A host whose manifest does not compile now logs one
  warning per schema key and counts `schema_rejected`. It no longer degrades
  without a trace.

## Behaviour change

On a host where the DNS Server manifest compiles, the dnstap output is
unchanged. The repository fixtures and the saved Windows Server 2025 template
compile.

On a host where a schema does not compile, the change in behaviour is:

| Before | After |
| --- | --- |
| Events were decoded through TDH. | Events are rejected. `schema_rejected` and `rejected` increase. One warning is logged for each key, naming the reason. |

The CHANGELOG records this under **Removed** and calls out the operator signal.
Under the local versioning policy this is not marked Breaking. The supported
configuration still produces the same output, and no reconfiguration or
reprovisioning is needed.

## Steps

1. **`schema.rs`.**
   - `Schema` holds a `DecodePlan`, not an `Option`.
   - `load_schema` fails with a clear reason when normalization or compilation
     declines the schema. The rejected-key cache and the warning that is logged
     once for each key then apply unchanged.
   - Delete `parse_fallback`, `property`, `property_size`, `read_property`,
     `scratch_property`, `PropertyName::native`, `PropertyName::descriptor`, the
     `packet` and `property` parser buffers, `MAX_PACKET_BYTES`, the `packet`
     property type, and the unused imports.
   - The parsed view now borrows only the event lease.
2. **Tests.**
   - Replace the fallback tests with rejection tests. An unsupported schema is
     rejected once, cached, and counted, with no TDH property call.
   - Keep the process-local manifest test as the native-metadata oracle. It shows
     that the real TDH metadata path produces a plan whose output matches the
     fixture, without comparing against TDH property reads.
   - Keep the zero-TDH-call and no-allocation tests.
3. **Docs.** Update these files:
   - `docs/design/single-pass-decoder.md`
   - `docs/guide/configuration.md`
   - `docs/guide/troubleshooting.md`
   - the module map in `src/etw.rs`
   - CHANGELOG
4. **Performance test.**
   - **Local.** A new ignored parse-and-encode benchmark, run on the old and the
     new build in alternating order. Also compare the `Parser` memory and the
     exe size.
   - **Lab.** Do these on DNSTap-WS2025, after a checkpoint:
     - Deploy the new build.
     - Confirm that the live manifest compiles: `schema_rejected=0` and
       `encoded=received`.
     - Compare the service CPU with the currently deployed build (`0865e19`)
       under the same `dns-stress` load. Alternate the runs and repeat each one.
5. **Gate.**
   - fmt and clippy for all feature sets.
   - `cargo test`, including the `diag-timing` feature.
   - A `cargo check` of the fuzz crate.
   - The release build and `verify-pe`.
   - linkcheck.
   - The elevated tests on the guest.

## Rollback

Revert the commit. The lab checkpoint `tdh-removal-pre-2026-11-01` and the
previous executable on the guest (`C:\Lab\etw-dnstap-previous.exe`) restore the
old service.

## Results (2026-11-01)

- **Gate.** fmt and clippy are clean for default, `diag-timing`, `fuzzing` and
  `--all-features`.
  - `cargo test` passes 216/0/7, and 221/0/7 with `diag-timing`. Two tests of
    removed code were deleted. The fallback test became two rejection tests. The
    new ignored benchmark adds the seventh ignored test.
  - The fuzz crate passes `cargo check`. `verify-pe` passes 21 of 21 checks.
    linkcheck finds no broken links.
  - On DNSTap-WS2025, the 5 elevated ETW tests pass.
- **Binary.** The exe shrinks from 1,170,432 to 1,164,800 bytes. The
  `TdhGetProperty` and `TdhGetPropertySize` imports are gone.
- **Memory.** `size_of::<Parser>()` falls from 1,360 to 824 bytes. The parser
  also no longer reserves a 64 KiB packet buffer.
- **Parse and encode benchmark** (`parse_encode_benchmark`, 5,000,000 events, one
  pinned core, 12 alternating runs per core):
  - Core 8: old 140.16 ns, new 141.11 ns.
  - Core 2: old 149.49 ns, new 150.56 ns.
  - Core 16: old 139.54 ns, new 141.31 ns.
  - The difference is 0.7% to 1.3%, inside the host's placement noise. The fast
    path was not changed.
  - Unpinned runs showed a 10% gap that disappeared when the runs were pinned.
- **Lab** (6 vCPUs, default settings, authoritative `dns-stress` at 5,000 QPS for
  50 s, 3 alternating runs per build against `0865e19`):
  - The live Windows Server 2025 schema compiles. The new build shows
    `schema_rejected=0` and `rejected=0`.
  - ETW loses nothing: `etw_lost=0`, and `received` matches twice the number of
    queries sent.
  - Service CPU for each run is 0.33-0.39 s on the old build and 0.34-0.44 s on
    the new one. The difference is within noise.
  - Both builds drop about 17%-19% of events as `ring_dropped` at this rate. The
    parser drains too late. This finding belongs to the high-resolution wait work.
