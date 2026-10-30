# Windows DNS capture performance test plan

This lab procedure measures service resource use and locates throughput limits for
maintainers of `etw-dnstap`. The two measured workloads each run for 300 seconds.
Record results separately from this plan; pilot data is not a measured result.

## Status and scope

This is the method and decision record for the original unpaced experiment,
completed on October 28, 2026 PDT (October 29 UTC). The
[three-run review](performance-review-2026-10-28.md) retains its overload and
upstream-fixture findings. Its 1,024-entry allocation-based ring and schema 1
ledgers are historical; use the [pooled VM plan](pooled-vm-performance-plan.md)
for the later pooled campaign and all-category accounting.

The commands here contain recorded lab addresses and campaign IDs. Before
another experiment, verify the guests and create fresh backups and run IDs;
the fixed helper paths are not a portable deployment procedure.

## Lab and prerequisites

The recorded experiment requires the following environment and safeguards:

- Windows VM: `DNSTap-WS2025`, GUID
  `24bafedb-0998-4430-a0c6-7e90350cd9ac`, Windows Server 2025 Core,
  `172.27.215.163`, 2 vCPUs, 4 GiB RAM.
- Host: execute the compiled Rust `tools/dns-stress` client against
  `172.27.215.163:53`. A client running on the DNS guest is not equivalent.
- Ubuntu: `172.27.215.60`, dnsmasq upstream for `rec.stress.test`, and
  dnscollector Frame Streams receiver on TCP 6000.
- Preserve the service's SCM-only entry point, LocalSystem account, enabled
  service SID, and `SeChangeNotifyPrivilege` allowlist.
- Save the installed binary, command line, DNS recursion/forwarder settings,
  and hashes before adding the two temporary zones. Record every build hash.

The names come from the 5,000 unique entries documented in
[`datasets/performance/workload-provenance.json`](../datasets/performance/workload-provenance.json).
Generate the run files with [`New-DnsWorkload.ps1`](../scripts/performance/New-DnsWorkload.ps1);
the measured invocation uses the generated `target/performance/workload`
copies, whose hashes are saved with each client run. Preserve the source commit,
license, count, and SHA-256 with the results. Authoritative names use
`auth.stress.test` and TTL 300. Recursive names use `rec.stress.test` and
TTL 0, with conditional forwarding to the isolated Ubuntu upstream. Verify
repeated answers reach that upstream rather than relying on cache misses by
assumption. Do not send these workloads to a public DNS server.

The pilot found Windows DNS's enabled global query block list rejects `wpad`.
The measured list therefore selects 5,000 names from the pinned larger common
names source while excluding `wpad` and `isatap`. Preserve that security policy;
do not disable it for the benchmark. The replacement fixture is `bills`.

The receiver writes its existing JSON logger output to a temporary durable
append-only file during the test. The test therefore measures the file sink
configuration, rather than console/journal throughput. Save receiver ingress,
egress, discard counters and kernel UDP drops; restore its original console
destination afterward.

## Establish the workload

1. Verify all VM identities, addresses, service tokens/configuration, fixture
   readiness, and collector connectivity. Record UTC on host and both guests.
2. Enable service `--metrics` in the protected task directory. Use the same
   instrumented binary and telemetry in both workloads. Collect 30 seconds of
   idle counters, then stop and save the idle run.
3. Run short authoritative pilots at 1, 2, 4, and 8 host client threads for
   10 seconds each, with 256 in-flight queries per thread and a 2,000 ms
   deadline. Compare achieved QPS, p95 latency, timeout rate, and host CPU.
   Select the lowest thread count that saturates the target without a client
   bottleneck; if saturation remains unclear, use 8 and report that limit.
4. Run the authoritative workload for exactly 300 seconds using the selected
   fixed thread count. Repeat for recursion. Preserve the selected thread
   count, in-flight limit, deadline, names-file hash, and exact commands.
5. For each workload, create a receiver start marker, start Windows counters,
   run the client, wait for its deadline/drain, stop the service to drain the
   native consumer and ring, and create the receiver end marker. Save its
   operation counts restricted to that workload's zone.

The recorded pilots selected two fixed client threads for both measured runs:
180k QPS at two threads was about 96% of the four-thread pilot's 188k QPS.
Eight threads had substantial endpoint-rotation waits and timeouts. Four- and
eight-thread pilots overlapped tracing, so their comparison includes tracing
overhead. Keep the workload uncapped and preserve all service drops.

The measured receiver log guard is 24 GB with a 3 GB free-space floor. Pilot
output projected roughly 14 GB per five-minute run at two threads. Archive the
owned JSON output losslessly between runs, outside measurement: verify gzip
integrity and both compressed and decompressed SHA-256 on the host before
removing only the archived generated file. Start each run with a fresh receiver
log and sufficient free space. Do not compress or change the JSON output format
during a measured interval.

The first recursive run exposed UDP receive-buffer drops in the temporary
Ubuntu upstream. Retain that complete 300-second observation, back up its
receive-buffer settings, raise the owned fixture's socket buffer, and require
a short same-configuration burst with zero upstream drops before repeating the
300-second recursive measurement. The completed experiment therefore has three
300-second runs. Keep Windows DNS cache settings unchanged for comparability.
Describe recursion as a repeated 5,000-name recursive-request working set:
TTL-zero answers do not establish one upstream lookup per client query under
this load. Use measured upstream and resolver events instead of a four-records
per query assumption.

## Delegated work and acceptance

The original experiment assigned the following work and acceptance gates:

| Chunk | Assigned model / effort | Owned work | Acceptance evidence |
| --- | --- | --- | --- |
| Coordination | GPT-6-Astra / high | Plan, boundaries, evidence review, final decisions | Gates before measured runs and explicit fixture repair decision |
| Windows performance | GPT-6.1-Sol / high | Service telemetry, Windows counters, host execution, analysis, report | SCM smoke ledger, exact PIDs, raw BLG/CSV/ETL, final run ledgers |
| Rust stress client | GPT-6.1-Sol / high | Independent UDP workers, bounded in-flight requests, latency/count outputs, socket rotation diagnostics | Eight tests, strict Clippy, release hash and explicit VM target |
| Workload and Ubuntu | GPT-6.1-Sol / high | Licensed names, isolated upstream/receiver, per-second resources, durable counts and archives | Provenance, TTL probes, UDP drop gates, JSON/counter reconciliation, hash-verified archives and restoration |
| Independent review | GPT-6-Astra / high, read-only | Client, telemetry, counters, timing, trace and claims review | Corrected reused thread-instance rates, checked weighted CPU/windows, independently reconciled receiver counts |

All source, script, and documentation edits are assigned to the Sol workers.
The Astra coordinator and independent reviewer inspect evidence and decisions.

Use this client command pattern with the recorded values:

```powershell
$dnsStress = 'tools/dns-stress/target/x86_64-pc-windows-msvc/release/dns-stress.exe'
& $dnsStress --server 172.27.215.163:53 --names NAMES --mode MODE `
  --threads THREADS --duration 300 --inflight 256 --timeout-ms 2000 `
  --output-dir RUN_DIRECTORY
```

Replace `MODE` with `authoritative` or `recursive`, `NAMES` with its generated
names-file path, `THREADS` with the selected worker count, and `RUN_DIRECTORY`
with a fresh result directory. This historical experiment deliberately omits
`--qps`; the [client guide](../tools/dns-stress/README.md#run-a-measured-scenario)
starts with a capped example.

Run the checked-in Windows helpers from elevated Windows PowerShell 5.1:

```powershell
powershell.exe -NoProfile -File scripts/performance/windows-control.ps1 -Action Start -RunId auth-1
powershell.exe -NoProfile -File scripts/performance/windows-capture.ps1 -Action Start -RunId auth-1
powershell.exe -NoProfile -File scripts/performance/windows-capture.ps1 -Action Trace -RunId auth-1
powershell.exe -NoProfile -File scripts/performance/windows-control.ps1 -Action Stop -RunId auth-1
powershell.exe -NoProfile -File scripts/performance/windows-capture.ps1 -Action Stop -RunId auth-1
powershell.exe -NoProfile -File scripts/performance/windows-control.ps1 -Action Fetch -RunId auth-1
```

Start the 45-second WPR window during sustained load. The helper uses the
General profile in memory mode, refuses an existing recording, and stops only
its successfully started recording. Do not cancel another recording.

## Evidence and interpretation

`logman` records one-second BLG samples, exports CSV with `relog`, and records
the service and DNS PIDs. Validate `ID Process` at every sample before
attributing an instance; exclude invalid rates and report valid sample counts.
Preserve discovered counter paths and excluded counters. Collect:

- Service and DNS process CPU, user/kernel CPU, private/working/private working
  set/virtual memory, faults, thread and handle counts, and I/O bytes and ops.
- Thread IDs, process IDs, CPU, context switches, state, and wait reason.
- Guest total CPU, runnable queue, memory, paging, disk latency/queue, network,
  UDP errors, TCP retransmits, and DNS query/response/recursive/drop counters.
- Host client CPU/memory and Ubuntu upstream/receiver CPU/memory/disk/network.
- Bounded WPR sampled CPU, context-switch and ready-thread trace for stack/wait
  analysis. Thread state snapshots and context switches alone do not establish
  lock contention. If trace symbols or WPA analysis are unavailable, state that
  limitation explicitly and do not infer a lock bottleneck from waiting threads.

Process CPU can exceed 100% because it sums CPU time across vCPUs. Report both
raw process percentage and guest-capacity percentage (raw / 2). Report mean,
p95, and maximum CPU; initial/final/delta/peak private bytes; working-set peak;
thread/handle ranges; I/O rates; DNS QPS; client latency and error rates.
Keep idle, pilots, tracing interval, and measured intervals distinct.

Service metrics are an opt-in one-second JSONL stream with a unique run ID,
PID, session, UTC milliseconds, and final marker after capture stops and parser
joins. One dedicated writer thread and per-event timing add instrumentation
overhead; report this in both workload results. The file is limited to 1 GiB.
Any writer error terminates capture and invalidates the measured run. Lines are
flushed each second; final metrics are synchronized to disk. An interrupted
process may lack a final sample.

For a stable final sample from this schema 1 binary, check these independent ledgers:

```text
received = ring_dropped + callback_errors + dequeued + queue_backlog
dequeued = encoded + ignored + rejected + output_limit_dropped + output_failed_events
```

Live atomic samples can straddle updates; apply equality checks only after the
final drain. `encoded` means accepted by the Frame Streams writer; it does not
mean the receiver acknowledged that record. Flush/finish failures are separate
output operation errors and can leave previously accepted records uncertain.
Compare with receiver ingress/egress/file counts to detect transport or sink
loss. The high-water metric is the maximum queue length observed after a push;
a concurrent parser pop can make it underestimate an instantaneous peak.

ETW `EventsLost` counts events not recorded for the enabled session, including
event IDs the service does not support. `RealTimeBuffersLost` counts buffers,
not events. Keep them separate and never add them. Only use ETW statistics when
`etw_stats_valid` is true; retain query failures. Do not divide ETW event loss
by supported received events and label it an exact end-to-end loss percentage.

After all measured runs, stop task collectors and restore the original service
arguments without periodic metrics. Retain the tested instrumented binary and
the reusable `auth.stress.test` zone with its 5,000 records. Remove only the
temporary owned `rec.stress.test` conditional forwarder and Ubuntu upstream
fixture. Restore receiver output and verify the service and collector are
healthy. Retain the original binary backup, new deployed hash, raw evidence,
and final restoration status under `target/performance`.

## Microsoft references

- [logman counter collection options](https://learn.microsoft.com/windows-server/administration/windows-commands/logman-create-counter)
- [WPR General profile and memory recording](https://learn.microsoft.com/windows-hardware/test/wpt/recording-for-basic-system-diagnosis)
- [CPU and wait analysis with WPR/WPA](https://learn.microsoft.com/troubleshoot/windows-server/support-tools/support-tools-xperf-wpa-wpr)
- [ETW session loss statistics](https://learn.microsoft.com/windows/win32/api/evntrace/ns-evntrace-event_trace_properties)
