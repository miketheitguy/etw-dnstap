# Profile a release-optimized build with application symbols

This staged plan gives the lab operator and reviewer a procedure for building
an optimized executable with full application symbols, deploying it to the
existing lab VM, and analyzing one 60-second workload configured for 100,000
queries per second (QPS). The outcome is caller-level evidence about CPU work,
scheduling delay, and waits in the combined parser/writer and native consumer.
**Status: executed on 2026-10-30.** The build, deployment, application-symbol
gate, single configured workload, and analysis are complete. See the separate
[results and limitations](symbolized-release-wpr-results.md). The procedure
below preserves the reviewed plan; execution receipts and findings belong in
the results report. Analysis used Astra at high effort at the user's request,
superseding the planned analyst allocation below.

Exclude file-hash checks, DNS query totals, achieved QPS, packet-drop analysis,
collector counts, and collector-versus-DNS comparisons. Do not invoke existing
helpers that perform those operations. Do not add a trace-free comparison,
additional load trial, instrumentation, or service implementation change.

## Use the earlier evidence to frame the questions

The [previous WPR results](parser-writer-wpr-results.md) identify the named
`dnstap-parser` and `etw-consumer` threads. They show substantial Ready time and
address-based waits, without evidence of dominant socket-write blocking or
sustained parser CPU saturation. Application frames remained unresolved. These
observations motivate the following questions; they are not conclusions about
the build proposed here:

- Which Rust caller reaches `WaitOnAddress`: input parking, another dispatcher
  or timing path, or a synchronization operation?
- How much exclusive thread CPU belongs to event decoding, dnstap/protobuf
  encoding, framing, transport writes, and socket-option updates?
- Which guest threads compete with the parser/writer and native ETW consumer,
  and how much interrupt activity overlaps their scheduled execution?

The previous trace also lacks a bounded pre-run host/guest clock bracket and
a reliable child exit code. Correct both before the next load gate. Its
substantial tracing overhead prevents treating its measurements as unprofiled
production behavior. Replacing the executable creates a separately labeled
profiling build; optimization and hardening preservation do not establish
performance equivalence with the earlier production artifact.

## Assign owners and preserve the lab configuration

Use these roles for future execution after the coordinator reviews the plan:

| Owner | Model and effort | Deliverable |
| --- | --- | --- |
| Builder | Sol, medium | Isolated optimized EXE/PDB pair and build/identity receipts |
| VM and WPR operator | Sol, medium | Backup, deployment, symbol smoke test, bounded trace, cleanup |
| Workload operator | Sol, low | One client launch and reliable timing/exit receipt |
| Analyst | Sol, medium | Resolved stack exports, clipped timeline, findings and limits |
| Coordinator | Root | Review readiness gates and integrate findings; use high-effort review only for ambiguous evidence |

Recheck the existing `DNSTap-WS2025` VM, GUID
`24bafedb-0998-4430-a0c6-7e90350cd9ac`, with four virtual CPUs and 4 GiB
memory. Preserve its service account, service arguments, security settings,
receiver/session configuration, CPU allocation, and memory. Earlier records
identify receiver `172.27.215.60:6000` and session `EtwDnstap-Lab`; verify live
settings during future preflight. Resolve service name, executable path, PID,
and PID lifetime afresh rather than reusing earlier PIDs.

Use a unique campaign directory under `target/analysis/symbol-wpr/RUN_ID/`.
Retain earlier evidence and keep build, deployment, smoke, timing, trace,
exports, and analysis receipts separate. Record paths, UTC timestamps, byte
lengths, tool versions, source revision, and working-tree state. Native
EXE/PDB identities establish symbol pairing; sizes and timestamps document
provenance and transfer. Neither substitutes for cryptographic verification,
which this experiment does not perform.

## Stage 1: produce and validate the symbol build

The [build research](../target/analysis/symbol-wpr/build-plan.md) records the
observed configuration and a proposed isolated Cargo profile. The repository's
[`Cargo.toml`](../Cargo.toml) release profile sets optimization level 3,
`debug = 0`, symbol stripping, overflow checks, and panic abort. Its
[Cargo configuration](../.cargo/config.toml) adds release-only `/DEBUG:NONE`
and target-wide hardening. A normal development build does not satisfy this
experiment.

1. Freeze the source revision and record local modifications. Use the pinned
   nightly toolchain, lockfile, Windows MSVC target, and existing rebuilt
   standard-library configuration. Preserve the service implementation.
2. Create an isolated `profiling` profile and target directory. The proposed
   profile inherits `dev` to avoid release-only `/DEBUG:NONE`, then explicitly
   sets release performance behavior: `opt-level = 3`, debug assertions off,
   overflow checks on, panic abort, incremental off, 16 codegen units, LTO off,
   and rpath off. Set `debug = 2` and `strip = "none"`.
3. Request a full PDB with `/DEBUG:FULL`; explicitly retain `/OPT:REF`,
   `/OPT:ICF`, and `/INCREMENTAL:NO`. Preserve static CRT, CFG, stack protection,
   EHCONT, PIC, ASLR, DEP, CET, and the existing target flags. Do not replace
   target hardening through environment flag overrides.
4. Validate the pinned Cargo's custom-profile behavior and effective compiler
   and linker flags from verbose build logs. Require optimization, full debug
   information, all target hardening, and absence of `/DEBUG:NONE`. Inspect the
   final PE headers/load configuration and imports. The proposed profile has
   not been compiled or validated; stop before deployment if effective flags
   differ from these requirements.
5. Archive the final EXE and full PDB together with profile/configuration,
   build logs, tool versions, source provenance, and lockfile. Confirm emitted
   filenames rather than assuming the PDB basename. Avoid packaging or stripping
   steps that discard symbols.
6. Read the executable's CodeView RSDS GUID and Age and the PDB's internal GUID
   and Age using available native tooling. Require an exact match, correct
   architecture, and recorded paths and sizes. Discover and validate tool syntax
   during execution; filename or timestamp agreement alone does not pass.

If future execution changes configuration or source, run the repository's
formatting and Clippy checks and appropriate existing focused tests. Use an
unwind test profile separately from the panic-abort deployment artifact. Keep
those checks within the no-hash restriction; this planning change adds no tests
or executable configuration.

**Build gate:** the coordinator accepts effective release optimization and
hardening, full debug information, and the matched EXE/PDB identities. GUID/Age
matching is a native symbol-pairing check, not a file-hash check. Optimized
inlining, code motion, eliminated functions, and identical code folding can
still limit attribution; retain those optimizations and report their effects.

## Stage 2: back up, deploy, and prove application symbol resolution

1. Save the installed EXE, any sidecar PDB or its documented absence, and
   service ImagePath, arguments, account, start mode, state, recovery settings,
   and protected ACL metadata outside the overwrite path. Record a rollback
   manifest with paths and sizes. Preserve receiver readiness without collecting
   delivery or count evidence.
2. Stop the service and wait for its process to exit. Stage the matched pair,
   replace the executable at its verified installation path, and preserve
   protected permissions. Restart the service with its recorded configuration.
3. Verify service health, fresh PID and lifetime, loaded executable path,
   runtime configuration, and the installed image's native identity against
   the archived pair. Retain the analysis copy of the exact EXE/PDB regardless
   of whether the PDB is also placed in the guest.
4. Configure the private PDB directory and fresh symbol caches using the settings
   in Stage 5 before the smoke export. Take a short bounded WPR smoke capture of the deployed process before the
   configured load, without generating a DNS validation workload. Stop/save/merge
   on the guest before transfer so image identification can inspect guest
   binaries. This is a symbol/capture readiness check, not another load trial.
   Export through WPAExporter with symbols enabled and save its diagnostics
   and resolved stacks. Use Windows Performance Analyzer (WPA) interactively
   to diagnose a failed export, then repair and repeat the export gate.
5. Require recognizable `etw_dnstap` application function names and expected
   source information in actual captured stacks from this deployed image.
   System names alone, an existing PDB, or an RSDS match does not pass. Require
   a real application caller in the available idle switch-out/parking stacks or
   known startup work, plus readable consumer/OS frames. Do not require decode
   functions that cannot be sampled without load or every optimized inline
   source line. If the smoke lacks enough application frames, stop and resolve
   readiness without launching the 100,000-QPS run.

**Symbol gate:** the coordinator reviews proof that WPA loads the exact private
PDB for the deployed image and resolves application frames. Disable acceptance
of mismatched symbols. Keep the smoke ETL, image/PDB identity receipt, symbol
path, diagnostics, and screenshot or export showing resolution.

If `<Missing ImageId event>` persists, diagnose image correlation separately
from PDB availability: correlate PID lifetime, image load/rundown base and size,
stack addresses, ImageId, and RSDS GUID/Age. The preceding trace contained image
events despite that displayed label. Check guest merge, exporter/profile/cache,
and symbol diagnostics. An idle service restart under recording can capture
live image loads, but does not guarantee correlation; record fresh identities
and pass the same export gate before load.

## Stage 3: prepare the bounded capture and client

Use the previous minimal sampled/precise capture design: process/thread and
image metadata, CPU configuration, sampled profile, context switches,
ReadyThread, thread priority, DPC, and interrupt events; stacks for sampled
profile, context switches, and ReadyThread. Preserve thread names and rundown
needed for exact lifetimes. Exclude disk/file/memory providers and packet
capture. Keep the observed sampling interval in the receipt; do not increase
sampling or add providers without a stated need.

Retain file mode, 128 buffers of 1,024 KiB, host startup free space of 20 GiB,
guest startup free space of 8 GiB, and a 4 GiB temporary-trace guard, with light
five-second storage checks. Buffers reserve about 128 MiB and do not cap ETL
growth. Reserve space for stop/merge and transfer. Preserve unrelated tracing;
if it conflicts with this recording, stop this experiment, not that session.
Bound the whole loaded recording to about 90 seconds with an owned-session stop
guard, allowing the send interval, bounded drain, and prompt stop. A guard
expiration makes the run incomplete and does not authorize another launch.

Before the launch gate, complete these preparations:

1. Review the planned no-hash capture, transfer, export, and deployment workflow.
   Existing `windows-pooled-wpr.ps1` Probe/Stop/Fetch and several campaign,
   control, staging, inventory, compression, and export helpers perform hash
   checks. Do not call them unchanged or indirectly. Reuse verified profile
   definitions and bounded export concepts only after reviewing every invoked
   path; do not implement or execute the replacement as part of this plan.
2. Verify the original authoritative fixture: 5,000 rotating A names, two host
   client threads, 256 outstanding requests per thread, 2,000 ms timeout, one
   60-second sending interval, and 100,000 configured QPS. Verify no other
   workload client is active. Assign the workload operator the sole launch.
3. Prepare a timing-only client wrapper that retains the actual process handle
   before completion, waits for that process, refreshes and reads its real exit
   code, and records PID/start time, launch/send/end timestamps, monotonic
   sending duration, drain deadline, exit, and absence of an owned survivor.
   A wrapper exit of zero cannot substitute for a missing child exit code.
   Validate exit capture without a DNS load trial. Do not parse query summaries.
4. Capture complete pre-run host/guest clock mappings: bracket each guest UTC
   read with host UTC before and after, retain round-trip bounds and monotonic
   anchors where available, and preserve raw samples. Schedule the same complete
   mapping after the run. A bare guest timestamp is insufficient.
5. Start the owned WPR session before load; retain start-call UTC brackets,
   session ownership, WPR/tool version, profile identity, collector status,
   active events/stacks, and sampling interval. Check storage and readiness.

**Launch gate:** require accepted build/deployment/symbol receipts, a reviewed
no-hash workflow, complete pre-run clock brackets, reliable client exit capture,
unchanged workload settings, active required capture, and safe storage. The
coordinator then authorizes the single launch. Missing gates stop the run.

## Stage 4: capture once and close the recording

1. Launch the single 60-second configured workload and retain its timing-only
   receipt. Keep symbol downloads, heavy exports, and analysis outside load.
2. Wait for the sending interval and bounded client drain; record the actual
   client exit code. On a deadline, terminate only the owned client, record the
   failure, and do not retry automatically.
3. Collect complete post-run clock brackets immediately after sending/drain,
   then stop and merge only the owned WPR recording promptly. Save stop-call clock
   brackets, exit/status logs, and final session state. Stop early if the
   storage guard triggers and mark the capture incomplete.
4. Copy the ETL and receipts while preserving the guest original. Record source and
   destination paths, byte lengths, and copy status; require that the host ETL
   opens and its metadata agrees. Do not claim cryptographic copy verification.
5. Confirm no owned client remains, WPR is idle for this experiment, and the
   service runs normally with its verified PID/path/configuration. Preserve
   unrelated tracing and service state.

## Stage 5: load the exact symbols and analyze the interior interval

Set the analysis symbol path to the archived private EXE/PDB directory followed
by a dedicated local cache for Microsoft's public symbol server. Load symbols
explicitly in WPA for the exact captured profiling image. Retain the path,
cache location, symbol-load diagnostics, resolved module identities, and tool
versions. Public system symbols supplement the private PDB; they cannot replace
it. Never substitute the previous production executable or an independently
rebuilt PDB because its filename matches.

Use these symbol settings, replacing `CAMPAIGN_ROOT` with the absolute campaign
path and placing the exact EXE/PDB pair in `symbols\app`:

```text
_NT_SYMBOL_PATH=CAMPAIGN_ROOT\symbols\app;srv*CAMPAIGN_ROOT\symbols\microsoft*https://msdl.microsoft.com/download/symbols
_NT_SYMCACHE_PATH=CAMPAIGN_ROOT\symbols\symcache
```

Create fresh campaign caches and retain the guest's exact module identities;
do not substitute host OS binaries. The
[Microsoft symbol-loading guidance](https://learn.microsoft.com/windows-hardware/test/wpt/loading-symbols)
describes symbol paths, explicit loading, and SymCache behavior.

Verify trace span, required event types/stacks, process/thread/image metadata,
and reported ETW event/buffer loss before interpreting it. Profiler-event loss
describes trace completeness, not DNS delivery. Missing loss information is
unknown. Distinguish absent, truncated, unresolved, and optimized-away frames.

Map the host sending interval into guest time using both clock-bracket sets,
then subtract the ETL header's actual trace origin. Record uncertainty and
drift; do not use WPR command completion as the origin. Select a conservative
interior common to supported mappings, excluding setup, drain, and stop. Record
both full sending and exact interior ranges; do not assume the previous
58-second range automatically applies.

Export only bounded process/thread and stack evidence for the fresh service
PID, named parser/writer and native consumer lifetimes, DNS, and relevant
competing guest threads. Produce these complementary views:

| View | Evidence and interpretation |
| --- | --- |
| Clipped thread timeline | Intersect Running, previous switch-out-to-ready Wait, and ready-to-switch-in Ready intervals with the interior range; include intervals completed after its end and report censored coverage |
| Exclusive CPU | DPC/ISR-exclusive process and thread CPU, with seconds, fraction of one logical CPU, and separately share of four-CPU VM capacity |
| Sampled stacks | Resolved application/native/module callers, sample weights, inline/folding limits, unresolved weight, and separate interrupt attribution |
| Wait and readying stacks | Target's blocking caller, wait reason, readying process/thread/stack, duration, and coverage; keep the two stack meanings distinct |
| Interrupt and competitors | DPC/ISR totals and relevant DNS/native-consumer/other runnable CPU and Ready behavior in the same interior |

Scheduled Running includes interrupt work while that thread is scheduled;
exclusive CPU does not. Do not mix those accounting bases or equate sampled
weight with blocking duration. Check units and clipped state coverage instead
of summing raw WPA Wait/Ready columns across boundary-spanning intervals.

Resolve work through the source: [`parse_loop` and input parking](../src/pipeline.rs),
[`Parser` and event decoding](../src/etw.rs),
[`DecodePlan`](../src/etw/decode.rs),
[`Encoder` and protobuf encoding](../src/dnstap.rs),
[`FrameStreamWriter`](../src/framestream.rs), and
[`NetworkSink`/`DeadlineTcp`](../src/network.rs). Inspect sends and repeated
socket-timeout updates (`setsockopt`) separately. Identify the native consumer
through its thread name and `ProcessTrace` stacks, and inspect DNS CPU demand
without deriving a capacity or delivery claim from process CPU alone.

An address-wait stack establishes an address-based wait. Require its resolved
Rust caller to distinguish `park_timeout`, dispatcher/timing behavior, queue
waiting, and synchronization; it does not alone establish lock contention or
queue emptiness. Ready time establishes scheduling delay, not socket blocking.
A readying I/O stack needs the matching target blocking stack before attributing
a socket wait. Report parser/decode/protobuf CPU or transport costs only where
resolved stack evidence supports them, with coverage and uncertainty.

## Stage 6: report findings and leave a recoverable lab

Write a separate results report with build label and identity, deployment and
symbol proof, workload timing/exit, clock bounds, trace health, exact ranges,
state accounting, exclusive CPU, resolved hotspots/wait callers, competing work,
and evidence links. Distinguish observed costs from hypotheses and proposed
changes. State whether attribution supports CPU work, input/timing waits,
transport blocking, scheduling pressure, mixed behavior, or insufficient
evidence. One profiled build/run cannot prove a throughput gain or an exact
equivalence to the earlier production artifact. Loaded tracing overhead remains
unquantified without an additional comparison, which this plan does not add.

On success, leave the profiling build deployed and the service running normally;
record that choice and the backup location. Return the owned WPR recording to
idle and remove only owned temporary resources after retaining evidence. On
deployment, startup, identity, symbol-readiness, or capture failure, preserve
failure evidence, stop the candidate, wait for exit, restore the original EXE
and PDB state, configuration and ACLs, and restore the recorded service state.
Verify rollback PID/path/configuration and normal service health without an
extra load trial. Never delete the rollback backup as routine cleanup.

## Evidence and sources

Local research inputs are the [build/deployment plan](../target/analysis/symbol-wpr/build-plan.md)
and [profiling plan](../target/analysis/symbol-wpr/profiling-plan.md), plus the
[earlier WPR results](parser-writer-wpr-results.md). The proposed profile,
native identity reader, no-hash workflow, and client wrapper remain execution
readiness items, not tested commands or completed changes.

Microsoft's [PDB identity guidance](https://learn.microsoft.com/windows/win32/dxtecharts/debugging-with-symbols)
and [DIA GUID/Age validation reference](https://learn.microsoft.com/visualstudio/debugger/debug-interface-access/idiadatasourceex-validatepdb?view=visualstudio)
support native pairing. The [WPT symbol reference](https://learn.microsoft.com/windows-hardware/test/wpt/symbols)
describes merge/image identification, and the
[WPAExporter reference](https://learn.microsoft.com/windows-hardware/test/wpt/exporter)
documents symbol-enabled exports and range units. Microsoft's
[CPU analysis guidance](https://learn.microsoft.com/windows-hardware/test/wpt/cpu-analysis)
distinguishes sampled/precise analysis and DPC/ISR accounting. The profiling
research retains the full source inventory and local evidence paths.
