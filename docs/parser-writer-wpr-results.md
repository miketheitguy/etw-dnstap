# Combined parser and writer WPR results

This report helps the pipeline maintainer assess CPU work and blocking on the
production combined parser/writer thread during one 60-second authoritative
workload configured for 100,000 QPS. The trace does **not** support socket-write
blocking as a dominant delay, or show the parser saturating one CPU. It shows
substantial scheduling delay and address-based waits whose Rust caller remains
unresolved. These observations do not establish a benefit from another worker
topology.

DNS query counts, achieved QPS, packet drops, collector receipt reconciliation,
and delivery correctness are outside this report. No service implementation,
binary deployment, or additional load trial forms part of this experiment.
See the [staged plan](parser-writer-wpr-plan.md).

## Reference and trace quality

The retained Windows VM `DNSTap-WS2025`, GUID
`24bafedb-0998-4430-a0c6-7e90350cd9ac`, has four virtual CPUs and 4 GiB memory.
The Running service is PID 1272 and uses production executable SHA-256
`EB70865D5D36A6E19868E87F2F7A4CAEE2D260B588DD39983097EA51E52803ED`.
The workload runs on the host with the preceding authoritative fixture:
5,000 names, two threads, 256 outstanding requests per thread, a 2,000 ms
timeout, and 60 seconds of sending. Service and receiver settings remain fixed.

The ETL contains sampled CPU, CSwitch, ReadyThread, process/thread/image
metadata, and DPC/ISR events. SampledProfile, CSwitch, and ReadyThread stacks
are present. The sampling interval is 1 ms. Trace headers report **zero lost
ETW events and zero lost buffers**. This is a profiler-health result, distinct
from application/pipeline packet loss; it says nothing about DNS delivery.

The trace starts at `2026-10-30T17:27:27.5816767Z` and ends at
`17:28:51.4284180Z`, covering 83.8467413 seconds with a PerfCounter clock.
Its 1,429,209,088 bytes transfer identically from guest to host; SHA-256 is
`F14BC24BCAFB9604F02C96BCF45709CB82C94BCC2CC4B7ED6F82C13A1D3FE453`.
The 128 × 1,024 KiB buffer configuration uses approximately 128 MiB. The
operator applies a 4 GiB temporary-file guard and 8 GiB guest-free-space guard.
The recording stops successfully and WPR returns to its non-recording state.

## Measurement interval and accounting

The timing-only workload receipt records sending from
`17:27:39.322Z` through `17:28:39.322Z`, exactly 60 seconds. It records client
completion with no remaining workload process. The child exit code is
unavailable because PowerShell returns null after completion; wrapper exit 0
does not establish client exit 0. No retry is performed.

Subtracting the ETL origin gives a nominal trace interval
`11.7403233–71.7403233` seconds. The principal analysis trims one second from
each end, using **`12.7403233–70.7403233` seconds, duration 58 seconds**.
Post-run host/guest round trips place guest UTC approximately 8–37 ms ahead
of host UTC. A bounded pre-run round trip is missing. The trim is conservative
under an approximately stable clock offset; it cannot prove the earlier offset
or exclude an unobserved clock step. QPC/UTC anchors support guest timing, but
do not repair the missing host-side pre-run bracket.

The experiment therefore only partially meets the plan's timing-readiness
gate: trace coverage is complete, but the required bounded pre-run host/guest
clock mapping is absent. The results remain conditional on the stable-offset
assumption above. The child exit-status gap and application-symbol limitations
also constrain interpretation; neither is replaced by a success assumption.

The raw WPA range export clips CPU weight but retains waits and Ready intervals
that extend outside the selected range. Summing its Wait columns directly
would incorrectly include idle time outside load. The analysis script instead
intersects each prior-switch-out → ready interval and ready → switch-in interval
with the 58-second window. It includes waits whose switch-in occurs after the
window and verifies coverage for the two named threads. Running intervals use
switch-in plus CPU duration; the tiny boundary discrepancy on the consumer is
25 microseconds. The two named threads have no material censored interval at
the selected boundaries.

The scheduled Running column includes interrupts executed while the thread is
scheduled. The separate Xperf CPU report excludes DPC/ISR. Keep these accounting
bases separate: scheduled Running plus Ready plus Wait describes the scheduling
timeline, while DPC/ISR-exclusive CPU better isolates thread execution.

## Thread scheduling and CPU

Thread identity confidence is high: trace metadata directly names
`dnstap-parser` TID 2560 and `etw-consumer` TID 2304 in service PID 1272.
The consumer stacks independently resolve through `sechost!ProcessTrace`.
These are existing threads recovered by rundown; identity does not rely on
choosing the busiest unnamed thread.

| Thread | Scheduled Running | Ready | Wait | State coverage |
| --- | ---: | ---: | ---: | ---: |
| Combined parser/writer, TID 2560 | 14.524524 s (25.04%) | 14.215911 s (24.51%) | 29.259565 s (50.45%) | 58.000000 s |
| Native ETW consumer, TID 2304 | 8.253248 s (14.23%) | 11.106784 s (19.15%) | 38.639993 s (66.62%) | 58.000025 s |

State percentages use the 58-second wall-clock denominator. Both threads spend
substantial time Ready: they can run, but the guest scheduler has not dispatched
them. This supports scheduling pressure during the recorded run. It does not
identify a unique competing process or prove that the host scheduler caused
the delay.

The independent Xperf report for the same window excludes DPC/ISR execution:

| Process or thread | CPU seconds | Mean equivalent logical CPUs | Share of four-CPU VM capacity |
| --- | ---: | ---: | ---: |
| DNS service, PID 2420 | 117.222918 | 2.0211 | 50.53% |
| Dnstap service, PID 1272 | 13.169606 | 0.2271 | 5.68% |
| Combined parser/writer, TID 2560 | 8.565599 | 0.1477 | 3.69% |
| Native ETW consumer, TID 2304 | 4.577550 | 0.0789 | 1.97% |
| System, PID 4 | 1.589751 | 0.0274 | 0.69% |

Thread rows are included in the service row. DNS is the largest measured
process CPU consumer, but this report does not infer a workload-capacity limit
or pipeline bottleneck from process CPU alone. The combined worker's exclusive
CPU averages 14.77% of one logical CPU. Neither that value nor its 25.04%
scheduled occupancy supports a claim of sustained parser CPU saturation.

## Blocking and sampled stacks

All **29.259565 seconds** of parser/writer Wait time in the 58-second interval
have the address-wait path:

```text
etw-dnstap.exe!<Missing ImageId event>
  → KernelBase!WaitOnAddress → ntdll!RtlWaitOnAddress
  → ntdll!ZwWaitForAlertByThreadId
```

The recorded wait reason is `WrAlertByThreadId`. This establishes an
address-based synchronization wait. The source's empty-input path parks for
2 ms, so input parking is a plausible explanation. Unresolved application
frames prevent distinguishing that caller from another synchronization path;
the trace does not establish queue emptiness or lock contention.

No socket-send blocking interval overlaps the 58-second interior. As a
boundary sensitivity check, the wider exploratory `12–71` second interval
contains one 27.5 microsecond wait under
`ws2_32!send → mswsock!WSPSend → mswsock!SockWaitForSingleObject`.
That stack demonstrates a socket wait, but its observed duration provides no
support for dominant TCP blocking. Ready time is scheduling delay and is not
added to socket-blocking duration.

The parser-associated sampled weight is 14.540390 seconds in the 58-second
window. Its largest leaf modules are:

| Leaf module | Sample weight |
| --- | ---: |
| `ntoskrnl.exe` | 6.972038 s |
| `etw-dnstap.exe` | 5.210618 s |
| `tcpip.sys` | 0.662004 s |
| `ntdll.dll` | 0.411577 s |
| `netvsc.sys` | 0.316450 s |
| `afd.sys` | 0.304515 s |

These samples include interrupt/DPC execution associated with the scheduled
thread; they are not all CPU instructions executed by the Rust worker.
Resolved stacks include network interrupt paths as well as user-thread send
and socket-option calls. Stacks containing `ws2_32!send` account for 1.028219 s
of sample weight, and stacks containing `ws2_32!setsockopt` for 0.408306 s.
Those are sampled execution observations, not blocking-time measurements.
Application leaf samples cannot be partitioned reliably into parsing,
protobuf encoding, framing, or other Rust work.

## Symbol and profiling limits

The production release has no Rust debug information and strips symbols.
Windows symbols resolve OS functions. Application frames display
`<Missing ImageId event>` in WPA exports, but an independent identity-event
inspection finds the executable's ImageId and image rundown records with
`DbgId/None`. The displayed label therefore reflects an identity-correlation
or symbol-export limitation; it does not prove that the trace lacks all image
events. There is no matching application PDB with which to recover Rust names.
Some sampled stacks are also `n/a` (0.750176 s of parser-associated weight).

Profiling overhead is material to interpretation. Sampled stacks show WPR/ETW
stack-walk and context-switch tracing work executing on the observed threads;
the logger thread in System also consumes CPU. The idle probe is small but
cannot calibrate loaded overhead. No matched trace-free trial is performed,
so observed scheduling and CPU values characterize this profiled run only.

The evidence supports address waits and scheduling delay as substantial parts
of this worker's observed timeline. It does not justify a parser CPU
parallelization claim, a dedicated writer to hide dominant socket waits, or
attribution of the address wait to a specific Rust call site. A topology
decision needs stronger caller-level evidence and a separately authorized
comparison; this experiment makes no implementation change.

If a follow-up investigation is authorized, first resolve the Rust caller of
the address wait, then inspect the competing runnable threads and interrupt
activity alongside DNS CPU demand. Separate the loaded tracing cost from
workload behavior before choosing a concurrency change. These are next
investigation priorities, not demonstrated causes or predicted performance
gains.

## Evidence and interpretation references

Evidence is under
`target/performance/parser-writer-wpr-100000/windows/auth-100000-60s-1/`:
`trace-health.txt`, `trace-health-stacks.txt`, `transfer-roundtrip.json`,
`process-thread-lifetimes.txt`, `image-identity-events.txt`, `export-raw-v3/`,
`export-symbols/`, `cpu-precise-exclusive-interior58s.txt`,
`analyze-interior.py`, and `analysis-interior-58s.json`. Host timing is in
`client-window.json` and the post-run clock samples are in
`windows/clock-post.json` at the campaign root.

Microsoft's [CPU analysis guidance](https://learn.microsoft.com/windows-hardware/test/wpt/cpu-analysis)
and [critical-path and wait analysis exercise](https://learn.microsoft.com/windows-hardware/test/wpt/optimizing-performance-and-responsiveness-exercise-3)
define the Running/Ready/Wait interpretation and distinguish a waiting thread's
stack from its readying thread's stack. The
[sampled-profile reference](https://learn.microsoft.com/windows/win32/etw/sampledprofile)
explains sampled instruction-pointer and module attribution.
