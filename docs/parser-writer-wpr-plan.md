# Profile the combined parser and writer

This plan gives the lab operators and reviewer a bounded WPR experiment to
distinguish CPU execution, CPU scheduling delay, and blocking on the existing
combined parser/writer thread. Run one 60-second authoritative workload with
100,000 configured QPS. Analyze thread behavior and stack evidence only; exclude
DNS query counts, achieved QPS, packet drops, collector ledgers, and receipt
reconciliation from this experiment.

## Preserve the reference configuration

Use the existing `DNSTap-WS2025` VM, GUID
`24bafedb-0998-4430-a0c6-7e90350cd9ac`, with four virtual CPUs and 4 GiB memory.
Keep the deployed production executable SHA-256
`EB70865D5D36A6E19868E87F2F7A4CAEE2D260B588DD39983097EA51E52803ED`.
Preflight records the service Running as LocalSystem, PID 1272, with receiver
`172.27.215.60:6000` and session `EtwDnstap-Lab`. Recheck PID and identity before
capture; a PID can change or be reused. Preserve the receiver configuration,
service arguments, security settings, CPU allocation, and memory.

Reuse the preceding host workload: authoritative mode, 5,000 rotating A names,
two client threads, 256 outstanding requests per thread, a 2,000 ms timeout,
and one 60-second sending interval. The workload operator owns the single
launch. Keep service code and the production binary unchanged; do not rebuild
to add symbols, deploy a candidate, add instrumentation, or add load trials.

Store evidence under `target/performance/parser-writer-wpr-100000/` and retain
all earlier campaigns. The deployed binary combines parsing, encoding, network
writes, and idle flush on the thread named `dnstap-parser`; see
[`parse_loop`](../src/pipeline.rs#L252). A high CPU total on that thread alone
does not identify parsing as the expensive operation.

## Assign the work

| Owner | Model and effort | Responsibility |
| --- | --- | --- |
| Plan and analysis reviewer | Sol, medium | Stage this plan; research WPA interpretation; analyze exported thread and stack evidence |
| WPR operator | Sol, medium | Prepare the minimal capture, verify clocks and trace health, capture and export evidence |
| Workload operator | Sol, low | Verify the unchanged client and fixture; run the single workload after the coordinator's gate |
| Coordinator | Root | Verify the plan and readiness receipts; authorize the one launch; integrate findings |

## Capture the scheduling and CPU evidence

Use the campaign's `windows/parser-writer.wprp`, profile `ParserWriter`.
It retains `CpuConfig`, `ProcessThread`, `Loader`, `CSwitch`, `ReadyThread`,
`SampledProfile`, `ThreadPriority`, `DPC`, and `Interrupt`. Enable stacks for
`SampledProfile`, `CSwitch`, and `ReadyThread`. Process/thread and image metadata
support PID/TID lifetimes and module attribution. DPC/ISR events allow the
reviewer to distinguish scheduled-thread accounting from interrupt activity.
Exclude disk/file/memory providers and packet capture from this focused run.

The profile provisions 128 buffers of 1,024 KiB: approximately 128 MiB of ETW
buffer storage. File mode preserves the beginning of the sending interval;
buffer sizing does not cap ETL file growth. The capture operator must use a
bounded recording duration, monitor free space and trace growth lightly, and
stop the owned recording if its storage guard triggers. Keep the existing host
20 GiB and guest 8 GiB startup free-space guards. The operator sets a 4 GiB
temporary-trace guard and checks it and guest free space every five seconds.
Preserve enough space for stop/merge
and transfer. Retain the observed sampling interval; preflight reports 1 ms.

Complete these steps in order:

1. Verify the VM resources, deployed hash, service PID, receiver readiness,
   client/fixture identity, storage limits, and absence of another client.
2. Check WPR session status and preserve unrelated tracing sessions. If an
   unrelated WPR recording prevents this profile from starting, stop this
   experiment and report the conflict. Never cancel another recording.
3. Record host/guest UTC clock brackets immediately before the run, including
   host timestamps on both sides of each guest UTC read. Retain monotonic clock
   readings where available and repeat the mapping after the run.
4. Start the owned WPR recording before load. Save start-call brackets, active
   collector status, profile identity, WPR version, and sampling interval.
   Check that the profile and requested stacks are active before the gate.
5. After the coordinator verifies the staged plan and readiness receipts,
   launch the single configured workload. Retain its sending-window timestamps
   and monotonic duration in a timing-only receipt. Do not analyze its query
   summary or calculate an achieved rate.
6. Stop the owned recording promptly after the sending interval and bounded
   client drain. Use its ownership marker and collector status to establish
   which recording is stopped. Retain stop brackets and final session status.
7. Transfer the ETL after capture. Verify source/destination SHA-256 and byte
   length. Preserve the guest original, transfer receipts, and tool logs.
   Perform expensive export, symbol loading, and analysis after load.

The ETL header's start time is the trace-time origin. Map host sending UTC to
guest UTC using the measured clock bounds, then subtract that origin. Do not
substitute the WPR command's completion time for the ETL origin. Retain the
offset bounds and their effect on the selected interval. If pre/post brackets
disagree, report drift or uncertainty and choose a conservative interior window
common to the supported mappings. Exclude trace setup, client drain, and trace
stop from the main analysis. Record both the full sending-window mapping and
the exact interior WPA export range.

## Verify the trace and export the evidence

Before interpretation, verify ETL duration and interval coverage, required
event types, image/process/thread metadata, stack presence, and ETW events or
buffers lost. **ETW trace-event loss describes profiler completeness; it is
distinct from application/pipeline packet loss and makes no claim about DNS
delivery.** If coverage or required events are missing, limit the conclusion
to what the remaining trace supports. A missing loss report is unknown rather
than proof of a lossless trace.

Use the existing `export-pooled-evidence.ps1` inspection and export workflow
for scheduling and sampled CPU tables, with exact process names/PIDs and a
bounded time range. Reuse `cpu-precise-threads.wpaProfile` for aggregate thread
CPU, Ready, and Wait totals. Preserve command lines, tool versions, profile
hashes, output units, symbol settings, and export ranges. Export these views:

| View | Required evidence |
| --- | --- |
| Precise process/thread utilization | Service and DNS CPU time; parser and native consumer identities; CPU normalization across four CPUs |
| Precise thread scheduling | Running, Ready, and Wait durations; switch-in, ready, and previous switch-out timestamps; previous state/reason |
| Scheduling stacks | Target-thread blocking stack and readying stack/process/thread, preserving their distinct meanings |
| Sampled CPU | Process, TID, thread name, module, stack, sample weight, and time |
| Trace health | Actual trace span, required event/stack presence, and ETW trace-event loss |

Normalize a thread's CPU by elapsed time to express fraction of one logical
CPU; divide by four only when expressing share of whole-VM capacity. Do not
mix inclusive scheduled time with DPC/ISR-exclusive estimates. Confirm units
before computing state totals. Clip intervals at the selected range boundaries;
waits attached to switch-ins can begin before the range, and incomplete waits
at the end can be censored. Treat mismatched state coverage as a limitation
rather than assuming CPU, Ready, and Wait always sum to the window.

## Decide what the evidence supports

Identify `dnstap-parser` from the trace's thread name if available. Otherwise,
combine TID lifetime, native-consumer stacks such as `ProcessTrace`, sampled
application versus transport stacks, and scheduling behavior. State confidence
explicitly; the busiest unnamed thread is only a candidate. Validate the
consumer and other service threads independently.

The release disables debug information, strips symbols, and links with
`/DEBUG:NONE`; see [`Cargo.toml`](../Cargo.toml#L24) and
[linker configuration](../.cargo/config.toml#L4). Keep the exact production
artifact. Microsoft public symbols may resolve OS calls while Rust application
frames remain module offsets. Distinguish captured but unresolved stacks from
missing or truncated stacks. If function-level attribution fails, report module
and thread findings without inventing parser-specific percentages.

Use these interpretation rules:

| Observation | Supported interpretation |
| --- | --- |
| Substantial running time with application CPU stacks | CPU execution on the combined worker; parsing versus encoding needs resolved supporting frames |
| Substantial Ready time | Scheduling contention inside the guest; inspect DNS, native consumer, and other runnable work |
| Wait time with a socket send/flush blocking stack | Evidence for transport-related blocking during those intervals; report stack coverage and duration |
| Wait time without a socket blocking stack | Blocking of unknown or separately identified cause; no TCP attribution |
| Timer/parking or queue-wait stacks | Idle/backoff/input wait evidence, distinct from socket blocking |
| Readying stack from an I/O completion path | Evidence about what readied the thread; require the matching target-thread stack before calling it a socket write wait |

Report whether the captured run supports parser/encoder CPU pressure,
transport-wait overlap opportunity, scheduling pressure, mixed behavior, or
insufficient attribution. One observational trace does not prove a throughput
improvement from another topology. Write findings and limitations in
`docs/parser-writer-wpr-results.md`; make no implementation change or additional
load run from this experiment.

## Interpretation sources

Microsoft's [CPU analysis guidance](https://learn.microsoft.com/windows-hardware/test/wpt/cpu-analysis)
describes sampled versus precise analysis and readying stacks. Its
[critical-path and wait exercise](https://learn.microsoft.com/windows-hardware/test/wpt/optimizing-performance-and-responsiveness-exercise-3)
defines Running, Ready, and Wait and explains the switch-out/ready/switch-in
sequence. The [WPR provider reference](https://learn.microsoft.com/windows-hardware/test/wpt/2-system-and-event-provider-definitions)
and [stack reference](https://learn.microsoft.com/windows-hardware/test/wpt/stack-wpa)
describe keyword/stack configuration. The
[sampled-profile event reference](https://learn.microsoft.com/windows/win32/etw/sampledprofile)
explains instruction-pointer and image-event module attribution.
