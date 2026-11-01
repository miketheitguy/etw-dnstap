# Symbolized release-optimized WPR results

> **Status:** Historical · **Evidence dated:** not recorded · **Commit:** not recorded · **Reviewed:** 2026-11-01

This report helps the pipeline maintainer choose the next performance changes
from one 60-second workload configured for 100,000 QPS. The loaded trace resolves
the parser's address wait to **timeout-based input polling**, shows substantial
scheduling delay and virtual-network interrupt work, and identifies decoding
and channel operations as CPU optimization candidates. It does **not** show
sustained parser CPU saturation or dominant socket-write blocking.

The build, deployment, symbol gate, single workload, capture, and analysis are
complete. No service implementation change, additional load trial, file-hash
check, DNS query-count analysis, achieved-QPS calculation, packet-drop analysis,
or collector comparison forms part of this result. See the
[execution plan](symbolized-release-wpr-plan.md) and
[earlier unresolved-symbol results](parser-writer-wpr-results.md).

## Main findings

The following findings distinguish observed costs from possible changes:

| Finding | Evidence in the 57.994060-second interior | Implication |
| --- | --- | --- |
| Parser input polling is resolved | 26.805304 s of Wait has `parse_loop → park_timeout → WaitOnAddress`; complete wait median 6.877 ms and p95 15.738 ms | Investigate notification-driven wakeup. The trace does not show when work arrives during a park, so this is not 26.8 s of proven avoidable delay. |
| Scheduling and interrupt pressure are substantial | Parser Ready 16.133125 s; consumer Ready 11.836809 s; DPC/ISR execution 60.673920 CPU-s across the VM | More Rust workers are not an established remedy. Examine virtual-network interrupt distribution and profiling overhead. |
| Parser CPU has identifiable work, without sustained saturation | DPC/ISR-exclusive CPU 9.127795 s, or 15.74% of one logical CPU | Optimize measured decoding/cache work selectively; do not infer a CPU-bound parser from application samples alone. |
| TCP writes consume CPU but do not dominate blocking | No parser socket-wait interval in the interior; regular sampled `send` path 0.759199 s and `setsockopt` path 0.282159 s | A dedicated writer is not supported as a way to hide dominant socket stalls in this run. |
| The native consumer has measurable enqueue cost | `Channel::try_send` leaf weight 1.123402 s; native callback inclusive weight 2.940024 s | Channel and notification design merit a bounded experiment that preserves the callback's nonblocking, allocation-free contract. |

## Build, deployment, and symbol proof

The profiling artifact uses source revision
`5fa21521a915a342023af226f2399d6476ed066d` **with the recorded working-tree
modifications**. The source snapshot, diff, and build receipts are retained;
the revision alone does not identify the complete source. The isolated
`profiling` profile uses optimization level 3, full debug information
(`debug = 2`), panic abort, overflow checks, 16 codegen units, no incremental
compilation, and no LTO. `/DEBUG:FULL`, `/OPT:REF`, `/OPT:ICF`, and
`/INCREMENTAL:NO` retain release optimization while producing a PDB.

Effective-command and PE inspection retain static CRT, CFG, stack protection,
EH continuation metadata, ASLR, DEP, and CET configuration. These checks establish
artifact configuration, not hardware enforcement or performance equivalence to
the earlier production executable. The build, formatting check, and strict
Clippy check pass. No application source was changed for this campaign.

The exact x64 pair is `etw-dnstap.exe` (1,091,072 bytes) and `etw_dnstap.pdb`
(21,393,408 bytes). Executable RSDS and native PDB GUID/Age agree:
**`245F44E2-4605-4624-9934-B8A29072DCC9`, Age 1**. Native DIA inspection also
finds application source-line records. Actual captured stacks resolve Rust
functions with the private PDB; matching identity alone was not the symbol gate.
The retained SymCache directory contains the same GUID/Age identity. The CSV
exports provide function names, not a complete per-instruction source-line view.

The retained VM is `DNSTap-WS2025`, GUID
`24bafedb-0998-4430-a0c6-7e90350cd9ac`, with four vCPUs and 4 GiB memory.
The loaded run uses service PID **1576**, parser TID **4268** (`dnstap-parser`),
and native consumer TID **1852** (`etw-consumer`). Thread names in the loaded
export and the consumer's resolved `sechost!ProcessTrace` stacks agree with
process/thread lifetime metadata. Both workers span the analysis interval.

The service retains its LocalSystem account, installed path
`C:\Program Files\EtwDnstap\etw-dnstap.exe`, receiver `172.27.215.60:6000`,
and session `EtwDnstap-Lab`. An idle capture resolves the fresh parser before
load. An idle guard failure caused by a null detached `Get-Volume` result was
retained and corrected with validated `IO.DriveInfo` reads before launch; it
was not a load trial. The loaded storage guard did not fire.

Builder and VM/WPR operator used Sol at medium effort; the workload operator
used Sol at low effort; the analysis used Astra at high effort, as requested.

## Timing and trace health

The sole client sends from `2026-10-30T20:27:18.044Z` through
`20:28:18.044Z`, with a measured sending duration of 60 seconds. It exits with
**code 0**, the deadline does not expire, and no owned workload process remains.
The fixture retains 5,000 rotating A names, two threads, 256 outstanding
requests per thread, and a 2,000 ms timeout. These are configuration values,
not achieved-rate measurements.

The ETL origin is **`2026-10-30T20:27:06.5516147Z`**; its end is
`20:28:29.4305584Z`, a span of **82.8789437 s**. Stop/merge completes later;
that completion time is not the trace origin or end. Guest and host copies
both have 1,555,038,208 bytes, copying succeeds, and the host trace opens.
The guest original remains retained. No cryptographic transfer claim is made.

Trace headers report **zero lost ETW events and zero lost buffers**. Sampled
Profile, CSwitch, ReadyThread, process/thread/image metadata, DPC, and interrupt
events are present; sampled-profile, switch, and readying stacks are present.
The sampling interval is 1 ms. Zero profiler loss does not establish delivery
correctness or complete stack resolution.

Both pre- and post-run clock sets use `GetSystemTimePreciseAsFileTime`, with
host-before/guest/host-after brackets and QPC anchors. Earlier readiness
`DateTime.UtcNow` samples were quantized and are not used for this mapping.
The retained bounds are:

| Mapping item | Result |
| --- | --- |
| Pre-run guest-minus-host offset | +8.8785 to +12.1253 ms |
| Post-run guest-minus-host offset | +8.1850 to +11.8214 ms |
| Envelope used across the run | +8.1850 to +12.1253 ms |
| Change between bracket-set midpoints | −0.4987 ms; not proof of an exact drift rate |
| Sending-start range in trace time | 11.4995703–11.5055106 s |
| Sending-end range in trace time | 71.4995703–71.5055106 s |
| Conservative common sending interval | 11.5055106–71.4995703 s |
| Analysis interior | **12.5055106–70.4995703 s**, duration **57.9940597 s** |

The mapping allows ±1 ms for the client receipt's timestamp precision and
trims one second inside each edge of the common interval. It assumes no
unobserved clock step or offset excursion outside the pre/post envelope.
Brackets constrain endpoint offsets; they do not continuously observe clocks.

## Scheduling and exclusive CPU

The calculation intersects each switch-in-to-scheduled-run-end, ready-to-switch-in,
and previous-switch-out-to-ready Waiting interval with the interior. It reads
the full trace export and includes a parser wait completed after the interior.
It does not sum WPA's untrimmed Wait/Ready columns.

The scheduled timeline is:

| Thread | Scheduled Running | Ready | Wait |
| --- | ---: | ---: | ---: |
| Parser/writer, TID 4268 | 15.052997 s (25.96%) | 16.133125 s (27.82%) | 26.807938 s (46.23%) |
| Native consumer, TID 1852 | 8.442053 s (14.56%) | 11.836809 s (20.41%) | 37.716117 s (65.03%) |

Parser intervals cover exactly 57.9940597 s, apart from floating-point noise,
with no gap. The consumer export has 0.9321 ms of interval overlap and 0.0132 ms
of uncovered interior time; its raw state sum exceeds the window by 0.9189 ms.
That small accounting discrepancy remains explicit rather than being forced
to zero. Neither target has a material censored boundary interval.

Ready means eligible to run but not scheduled; it is not socket blocking.
For the parser, 15.352130 s of Ready follows a previous state of Ready, while
0.486833 s follows Waiting. Most accumulated Ready time is therefore not just
dispatch latency after its polling wait. These observations do not identify a
unique competing thread or a host-scheduler cause.

Xperf's independent precise CPU report excludes DPC/ISR execution. Percentages
below use the wall-clock interval and four-vCPU capacity, not Xperf's displayed
percentage denominator:

| Process or thread | Exclusive CPU seconds | Equivalent logical CPUs | Four-vCPU capacity |
| --- | ---: | ---: | ---: |
| DNS, PID 2420 | 116.466780 | 2.0083 | 50.21% |
| Dnstap service, PID 1576 | 13.956633 | 0.2407 | 6.02% |
| Parser/writer, TID 4268 | 9.127795 | 0.1574 | 3.93% |
| Native consumer, TID 1852 | 4.800910 | 0.0828 | 2.07% |
| System, PID 4 | 1.784521 | 0.0308 | 0.77% |

Thread rows are included in the service row. Scheduled Running includes
interrupt execution while a thread is scheduled; exclusive CPU does not.
The parser's 15.053 s scheduled occupancy must not be described as 15.053 s
of Rust execution. Neither its scheduled occupancy nor exclusive CPU supports
sustained saturation of a logical CPU.

## Resolved waits and readying paths

Of the parser's 26.807938 s of Wait, **26.805304 s (99.990%)** has this loaded
blocking caller:

```text
etw_dnstap::pipeline::parse_loop
  → std::thread::functions::park_timeout
  → KernelBase!WaitOnAddress
  → ntdll!RtlWaitOnAddress
  → ntdll!ZwWaitForAlertByThreadId
```

The remaining 2.6333 ms has the same kernel address-wait path but lacks the
user-mode caller. All parser Wait has reason `WrAlertByThreadId`, and all its
readying stacks contain `KiTimerExpiration`. A timer running in a DNS thread's
scheduled context does not mean DNS code explicitly wakes the parser.

The 3,596 fully interior wait episodes have mean **7.451 ms**, median
**6.877 ms**, p95 **15.738 ms**, p99 **16.925 ms**, and maximum **17.748 ms**.
These are blocked durations, excluding subsequent Ready time. The
`drain_ring` source (`src/pipeline.rs`) requests a 2 ms park after a
failed `try_recv`; the regular
`native_etw_callback` (`src/etw.rs`) enqueue path does not unpark it.
The resolved caller and timer readying establish timeout polling, rather than
a Rust mutex-contention diagnosis. The trace does not establish the precise
reason the requested timeout is exceeded.

A failed receive means no event was obtained at that check. It does not show
queue occupancy throughout the later park. Without enqueue timing or occupancy
evidence, the parked interval cannot be divided into useful idle waiting and
delay after work becomes available. No parser socket-send blocking interval
appears in the interior, including its boundary-spanning completion records.

The consumer waits 37.710842 s for real-time ETW delivery through
`ProcessTrace → EtwpProcessRealTimeTraces → ZwWaitForMultipleObjects`.
System TID 992 readies it through
`EtwpLogger → EtwpRealtimeDeliverBuffer → EtwpRealtimeInjectEtwBuffer → KeSetEvent`.
Another **5.2753 ms** is native address-space push-lock waiting while
`EtwpAdvanceToNextBuffer → VirtualFree` runs; the same logger's allocation path
releases the corresponding lock. This small native-memory wait is distinct
from the parser's timeout wait and does not support dominant callback lock
blocking.

## Resolved CPU work

The explicit WPA DPC/ISR classification partitions parser-associated samples
into **8.752382 s regular CPU**, **5.973137 s DPC**, and **0.260539 s ISR**.
Consumer samples partition into **4.520864 s regular CPU**, **3.637192 s DPC**,
and **0.225660 s ISR**. Sampled weight is an estimate and differs from precise
CPU. Interrupt samples are not charged to Rust work in the following table.

Leaf weight attributes the sampled instruction to a function. Inclusive weight
includes descendants and overlaps other rows; the rows must not be added:

| Parser path | Regular leaf weight | Regular inclusive weight |
| --- | ---: | ---: |
| `Parser::parse` | 1.325727 s | 5.552938 s |
| `DecodePlan::decode` | 1.317523 s | 1.615444 s |
| `Encoder::encode_with` | 0.335055 s | 1.122512 s |
| Prost encoding paths | Not separately partitioned here | 0.532239 s |
| `FrameStreamWriter::write_frame` and descendants | 0.037898 s | 1.146954 s |
| `ws2_32!send` and descendants | 0.003983 s | 0.759199 s |
| `ws2_32!setsockopt` and descendants | 0.033818 s | 0.282159 s |

The source connects `Parser::parse` (`src/etw.rs`) to the cached schema,
`DecodePlan::decode` (`src/etw/decode.rs`), endpoint parsing, and
`Encoder::encode_with` (`src/dnstap.rs`). The encoder's resolved generic
instantiation identifies the `parse_fast` closure; this is not a separately
measured `parse_fast` call frame. Inlining and identical-code folding prevent
a clean source-operation partition of every leaf sample. `Parser::parse`
inclusive weight also includes encoding and some tracing work.

Other observed parser leaf costs include address conversion
(`etw::address` 0.205468 s and `read_ipv4_addr` 0.212355 s), hashing
(`BuildHasher::hash_one` 0.123906 s and the hash writer 0.097740 s), and
`Instant::now`/`RtlQueryPerformanceCounter` execution. These identify work to
inspect, not independently additive elapsed-time bottlenecks.

The `FrameStreamWriter` (`src/framestream.rs`) already writes through a
`BufWriter`. `DeadlineTcp::write` (`src/network.rs`) updates the socket
write timeout before each underlying write, explaining the resolved
`set_write_timeout → setsockopt` path. These are CPU costs, not evidence that
the timeout expires or the socket blocks. The observed setsockopt path is only
0.282159 sampled seconds, about 0.49% of one CPU over this window; it is a
smaller target than decoding or the consumer's enqueue path.

The consumer's regular callback inclusive weight is 2.940024 s. Its largest
application leaf is bounded-channel `Channel::try_send` at 1.123402 s,
followed by the callback's TLS closure at 0.651519 s. Pool acquisition and
copying also resolve. These samples show CPU in the nonblocking callback;
they do not establish channel lock blocking or queue occupancy.

## Competing work, interrupts, and profiling limits

Clipped DPC/ISR fragments account for **60.235976 s DPC** and **0.437944 s
ISR**, totaling **60.673920 CPU-s**, or **26.16% of four-vCPU capacity**.
The principal DPC entry routines are `vmbus!ChildInterruptDpc` (47.020132 s)
and `vmbkmcl!InpProcessingDpcRoutine` (13.139713 s). Interrupt work is strongly
concentrated on logical CPU 0 (30.483740 s) and CPU 2 (30.156639 s), roughly
52% of each CPU's available time; CPUs 1 and 3 have 0.020313 s and 0.013228 s.

Resolved interrupt samples traverse VMBus, `netvsc`, NDIS, UDP receive, and AFD
completion paths. This supports virtual-network receive/completion work as a
substantial competing cost. It does not assign the whole DPC entry-routine
duration to a single descendant or prove a unique host, driver, or hardware
cause. Summing clipped interrupt fragments with all exclusive thread CPU,
including idle, recovers the four-CPU interval within **16.9 microseconds**.

DNS consumes approximately two logical CPUs of exclusive execution across its
workers. `dns!FastMutex_Acquire` has 10.068767 s of regular leaf sample weight;
resolved DNS mutex blocking stacks total about 0.392 s, while the dominant
active-worker waits use `Udp_RecvThread → GetQueuedCompletionStatus` and
network-completion readying stacks. Lock-related CPU and blocked duration are
different measurements. These observations support investigating DNS and
network competition, not a DNS capacity or delivery claim. DNS timeline
exports have small accounting overlaps, so no exact DNS state partition is
presented.

Profiling itself is visible: stacks containing `EtwpTraceStackWalk` account
for **1.280486 s of parser regular samples** and **0.915123 s of consumer
regular samples**. Context-switch and readying instrumentation, unwind work,
and logger execution are also visible. These overlapping sample categories
are not a complete estimate of profiling overhead; the ETW real-time delivery
logger also belongs to the application's capture path. No trace-free
comparison was performed, so loaded tracing overhead remains unquantified.

Missing full stacks account for 0.667596 s (7.63%) of parser regular samples
and 0.367685 s (8.13%) of consumer regular samples. Some leaf symbols remain
unknown. The application `Missing ImageId` problem from the preceding report
does not appear in these target stacks, but successful symbols do not recover
optimized-away frames or every missing stack.

## Prioritized next changes

The following are proposed follow-up targets, not implemented changes or
predicted throughput gains:

1. **Investigate scheduling and virtual-network DPC concentration before adding
   workers.** Retain the same interval/accounting definitions when evaluating
   interrupt distribution or a separately authorized lower-overhead capture.
   The substantial Ready and DPC time is observed; its avoidable fraction is
   unknown. Do not choose affinity, priority, or VM CPU changes from aggregate
   CPU alone.
2. **Prototype a coalesced, notification-driven empty-input wait.** Replace
   timeout-only discovery of arriving work with an arm/recheck protocol that
   prevents lost wakeups. Preserve the native callback's nonblocking,
   allocation-free behavior, bounded resource use, flush timing, and shutdown
   drain. Audit callback wakeup cost and test enqueue/park/shutdown races before
   adoption. A blocking channel receive is not automatically a safe performance
   replacement. Do not introduce busy spinning or a high-resolution timer
   merely because the observed waits exceed 2 ms. The trace establishes the
   polling mechanism, not the number of milliseconds this change would save.
3. **Reduce measured parser and producer CPU work selectively.** Inspect the
   compiled decode walk and schema-cache lookup first; the two largest parser
   application leaf buckets are `Parser::parse` and `DecodePlan::decode`.
   Preserve schema-version handling, bounds checks, malformed-input rejection,
   and fallback behavior. Review the 1.123402 s enqueue leaf budget alongside
   notification design before replacing the channel or pool. Function-level
   samples cannot predict the benefit of a particular rewrite.
4. **Treat transport bookkeeping as a smaller target.** Review redundant socket
   timeout updates and time-query frequency while retaining whole-operation
   deadlines and failure semantics. The measured send/setsockopt paths justify
   CPU-cost investigation; they do not justify separating a writer to hide
   socket stalls. The existing buffered framing also limits what an additional
   batching layer can be assumed to improve.

One profiled run of a separately labeled symbol build cannot demonstrate a
production throughput gain, establish equivalence to the stripped artifact,
or rank future implementations by achieved rate. The measured budgets guide
where to experiment; they are not recoverable-time or speedup estimates.

## Retained evidence and final lab state

The campaign root is
[`target/analysis/symbol-wpr/executed-100000`](../target/analysis/symbol-wpr/executed-100000/).
The following receipts make the findings reviewable:

| Evidence | Campaign-relative location |
| --- | --- |
| Build/profile/source and native identity | `build/READY.json`, `build/effective-flags.json`, `build/source/`, `build/pdb-proof.txt`, `symbols/app/` |
| Deployment, exact symbol gate, and cache identity | `windows/deployment.json`, `windows/symbol-gate.json`, `windows/app-symcache.json`, `windows/deployed-pdb.txt` |
| Client timing and actual exit | `client-window.json`, `client-exit.json`, `client-invocation.json` |
| Clock brackets and derived range | `windows/clock-pre-loaded.json`, `windows/clock-post-loaded.json`, `analysis/window.json`, `analysis/map-window.py` |
| ETL, transfer, capture, and sampling | `trace/loaded-100000/trace.etl`, `transfer.json`, `Stop-receipt.json`, `profint.txt` in that directory |
| Trace health and lifetimes | `analysis/trace-health.txt`, `analysis/trace-health-stacks.txt`, `analysis/process-thread-lifetimes.txt` |
| Full scheduling/readying stacks | `exports/loaded-100000-stacks-all/` |
| Explicit sample classification and interrupt fragments | `exports/loaded-100000-interrupt-analysis/` |
| Reproducible clipped calculations | `analysis/analyze.py`, `interior.json`, `interrupts-cpu.py`, `interrupts-cpu.json`, `dns-waits.py`, `dns-waits.json`, `summarize.py`, `summary.json` |
| Exclusive CPU report | `analysis/cpu-exclusive.txt` |
| Final service/WPR/guard state | `windows/final-health.json`, `windows/operator-receipt.json` |

Exports retain invocation, symbol path, diagnostics, tool version, and exit
receipts. WPAExporter is 11.8.423.12582; the host Xperf reports 10.0.28000,
and guest WPR reports 10.0.26100. The calculation scripts write only derived
analysis and preserve the operator's exports.

The profiling build remains deployed and **EtwDnstap is running as PID 1576**
with its receiver connection established. **WPR is idle**, no owned guard or
load process remains, and the original executable backup is retained at
`C:\ProgramData\EtwDnstap\performance\symbol-wpr-executed-100000\rollback`.
The original installation had no sidecar PDB. These retained artifacts allow
rollback without rebuilding the original binary.

Microsoft's [CPU analysis reference](https://learn.microsoft.com/windows-hardware/test/wpt/cpu-analysis)
defines sampled versus precise analysis, Running/Ready/Waiting, sampled
DPC/ISR classification, and interrupt-fragment accounting. Those definitions
support the measurement distinctions used here; the numerical findings come
from the retained campaign evidence.
