# Pooled intake VM performance plan

This completed plan helps maintainers review or reproduce the method for the
reviewed pooled candidate's Windows DNS capture checks and bounded throughput
discovery. Native VM measurements,
actual WPR traces, raw counters, and exact receiver ledgers determine the result.
Local allocator tests do not establish throughput or native loss behavior.

## Campaign status and use of this plan

All 22 stages and restoration checks are complete. The
[performance review](pooled-vm-performance-review.md) retains 12 strict passes
and 10 strict failures; both high-rate five-minute confirmations pass every
gate. The [final completion record](#final-completion-record) identifies the
restored state at the campaign cutoff.

The procedure, approvals, staged decisions, and fixed addresses here record
this campaign. Do not replay its `Prepare`, deployment, or restoration actions
against an existing installation without a fresh campaign backup, unique IDs,
verified guests, and the stated readiness gates. The plan is not a live-state
inventory. See [Measurement terms and result gates](measurement-terms.md) for
shared units and outcome definitions.

## Ownership and dependencies

The recorded work assignments and required evidence were:

| Work | Model / effort | Owner | Required output |
| --- | --- | --- | --- |
| Matrix decisions and read-only integration review | GPT-6 Astra / high | Root coordinator | Approve matrix, assess failures, review final claims |
| Build, Windows deployment, load, counters, WPR, scripts, reports | GPT-6.1 Sol / high | `pooled_vm_windows` | This plan, hardened build receipt, exact commands, per-run evidence, restored service |
| Ubuntu receiver and upstream configuration | GPT-6.1 Sol / high | `pooled_vm_ubuntu` | Protected baseline, READY marker, actual upstream counters, stable receiver boundaries, durable archives, restoration |
| Independent WPR guidance and read-only trace analysis | GPT-6.1 Sol / high | `pooled_wpr_analysis` | Supported profile and retention policy, trace-loss/coverage checks, actual scheduling and CPU evidence |

The stopped `perf_windows` and `perf_review` work remains stopped. Sol authors
scripts, code, and documentation; Astra coordinates and reviews read-only. There
are no source architecture changes during measurement and no commits. A failure
requiring source, timer, pool, VM, or workload changes goes to the coordinator.

Load waits for all of these gates:

1. The reviewed source hashes match the audit receipt; fresh formatting, strict
   Clippy, locked release, and actual PE verification pass.
2. A new protected Windows backup preserves the original binary, arguments,
   service identity/ACL policy, DNS state, and unique campaign ownership.
3. The Ubuntu owner verifies its baseline and declares receiver/upstream READY.
4. The trace reviewer approves the actual supported WPR profile and bounded
   retention policy, and the root coordinator approves the matrix.
5. Each run has unique directories, sufficient disk/resources, no unrelated
   trace recording, a fresh receiver start marker, and matching deployed hash.

## Provenance and fixed environment

Campaign root: `target/performance/pooled-vm-20261029-0942`.
Guest evidence root: `C:/ProgramData/EtwDnstap/performance/pooled-vm-20261029-0942`.
All previous campaign artifacts and receipts remain preserved.

| Artifact | SHA-256 |
| --- | --- |
| Installed capacity-only baseline | `CA3342D848BD06B7B8E1E6CE9ADC05E2157C3E86A18980E69E74B16555264347` |
| Reviewed pooled candidate and fresh locked release | `98FDAB43AA24AE1BE31A7AABCCBDD67A2F64C87DC1C50C428B638B3C091F5D7C` |
| Initial pooled candidate, preserved | `3280D954E17E1448C51FBFF5B253807E473CF9BE59FAC18BC94DBBD51D95B841` |
| Unchanged DNS client | `7A5800CFC4252196E4C6162F7D2CCF81217FE95A339D7CFBB9CD84063F4B6960` |

The audited source identity is recorded in
`datasets/pooled-intake-local/audit-fixes-v1/receipt.json`; a copy and fresh build
logs are in the campaign's `build` directory. Commands executed before load:

```powershell
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release -vv
```

The effective pinned toolchain is `nightly-2026-10-01`, target
`x86_64-pc-windows-msvc`. Preserve static CRT, overflow checks, panic abort,
stack protector, CFG, EH continuation guard, NX, ASLR, high-entropy VA, CET image
marker, and release stripping/no PDB. Inspect actual final PE headers, load
configuration, and dependencies. Image markers do not prove runtime enforcement.
The unchanged audited source already passed 45 development, release-unwind, and
ASan tests; do not relabel those as new native tests or repeat them needlessly.

Live preflight verified Windows `DNSTap-WS2025`, GUID
`24bafedb-0998-4430-a0c6-7e90350cd9ac`, `172.27.215.163`, fixed 2 vCPUs/4 GiB,
and 48.8 GiB free guest C drive. Host C has approximately 402 GiB free. The
baseline is Running through SCM as LocalSystem, service SID UNRESTRICTED, with
only `SeChangeNotifyPrivilege`. Binary and SCM ACLs are protected. Normal arguments:

```text
"C:\Program Files\EtwDnstap\etw-dnstap.exe" --receiver 172.27.215.60:6000 --session-name EtwDnstap-Lab
```

The measured service uses only an additional unique `--metrics` prefix. It uses
the reviewed binary defaults: queue capacity 8,192 and jumbo capacity 64, regular
2 KiB slots (capacity plus two), jumbo 131,584-byte slots. Preserve the ten-second
shutdown watchdog and SCM-only startup; no foreground mode, privilege expansion,
global cache/timer changes, VM resize, or pool/capacity tuning.

Retain the existing 5,000 A records in `auth.stress.test`, value `192.0.2.53`,
TTL 300, and the existing name-file hashes/provenance. Create only a marked,
temporary `rec.stress.test` conditional forwarder to the Ubuntu owner's verified
`172.27.215.60` TTL-zero controlled upstream after backup. No extra queries occur
inside sending windows. The actual upstream query/reply counters determine
recursive expected records: `2 * client_sent + upstream_queries + upstream_replies`.
Do not assume four events per request or one cache miss per repeated name.

## Approved bounded matrix

Root approved this bounded matrix before the first load. Each run still requires
actual WPR probe/export acceptance and Ubuntu READY. All stages
retain two workers, 256 in-flight requests per worker, 2,000 ms deadlines, a global
QPS ceiling, and the same 5,000-name rotation.

| Stage | Mode | Requested QPS | Sending seconds | Purpose |
| --- | --- | ---: | ---: | --- |
| Matched baseline acceptance | Authoritative, then recursive | 1,000 | 300 each | Repeat earlier five-minute workload on reviewed pooled candidate |
| Geometric discovery | Each mode independently | 2,000; 4,000; 8,000; 16,000; 32,000; 64,000; 128,000 | 60 each | Ascend only while clean; stop that mode at first limiting result |
| Bracket refinement | Each mode with pass/fail bracket | At most two midpoint rates | 60 each | Narrow first failure without unlimited retries |
| Sustained confirmation | Each mode | Largest clean discovered/refined rate | 300 each | Establish an observed five-minute result, not universal capacity |

If the largest-rate confirmation fails, permit at most one 300-second fallback
at the previous clean lower stage. Skip redundant confirmation when the largest
clean rate remains the already confirmed 1,000 QPS baseline. The 128,000 QPS
ceiling is this campaign's test cap, not an absolute platform limit. Skip any
stage whose projected trace/raw/archive budget cannot fit its full duration.

If the first 1,000 QPS capture gate fails, preserve evidence, stop ascent, and ask
the coordinator to decide diagnosis/continuation. A repeated isolated RA=false
response is reported separately from throughput and capture loss; it prevents
a full DNS-validity pass, and never disappears through an automatic rerun.

The exact client command expands the recorded workload parameters into:

```powershell
$dnsStress = 'tools/dns-stress/target/x86_64-pc-windows-msvc/release/dns-stress.exe'
& $dnsStress --server 172.27.215.163:53 --names NAMES --mode MODE `
  --threads 2 --duration SECONDS --inflight 256 --timeout-ms 2000 `
  --output-dir NEW_RUN_DIRECTORY --qps RATE
```

Replace `NAMES` with the generated names-file path, `MODE` with `authoritative`
or `recursive`, `RATE` with the approved global ceiling, `SECONDS` with 60 or
300, and `NEW_RUN_DIRECTORY` with a fresh output directory.

Use `windows-run-client.ps1` with explicit `-Mode`, `-RunId`, `-Duration`, `-Qps`,
and the fresh campaign `client` root. Record expanded arguments, executable/name
hashes, actual sending duration, sent/success rates, response/flag/timeout/socket
counters, client CPU/memory, and latency-bin upper bounds. Requested QPS alone
is not achieved throughput; fewer than 98% of requested QPS marks a generator or
target limit. The two-worker/512-outstanding bound and host CPU can limit discovery.

The stage wrapper records and guards this protocol. For the first authoritative
run, after the actual receiver READY receipt exists, execute:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/performance/windows-pooled-stage.ps1 -Mode authoritative -RunId pooled-auth-1000-300s-1 -Qps 1000 -Duration 300 -ReceiverReadyReceipt RECEIPT_PATH -MatrixApproved
```

`RECEIPT_PATH` is the Ubuntu owner's preserved run-specific READY/start-marker
receipt. Later commands change mode, unique run ID, QPS, and duration only within
the approved matrix. The wrapper calls `windows-pooled-wpr.ps1`,
`windows-pooled-campaign.ps1`, `windows-capture.ps1`, and the unchanged client
wrapper. Its completion requires the independent stable receiver end-marker and
exact all-category gate before another run.

## Per-run protocol and stop criteria

1. Check exact service hash, default configuration, guest/host resources, no
   conflicting WPR/logman collector, and fresh run ID. Require guest free disk
   at least 8 GiB and host C at least 20 GiB. Preserve at least 1 GiB guest
   available memory; sustained exhaustion, unexpected process exit, trace growth
   past the approved bound, or collector failure stops the stage.
2. Obtain Ubuntu start-marker/READY receipt, start the service through SCM with
   a new metrics path, and start exact-PID PerfMon. Record three host/guest clock
   bounds before and after each stage.
3. Start the approved actual WPR recording and the unchanged load. The final
   profile, timing, buffer count, file-mode policy, and size guard are recorded
   in [Metrics and analysis](#metrics-and-analysis). Never claim WPR evidence until an ETL
   exists, its source trace has been inspected, and relevant event coverage/loss
   is reported. Do not substitute synthetic tests or the ignored local benchmark.
4. During load collect one-second process, thread, guest, and host-client metrics;
   inspect only lightweight health/space/counter status. Do not export ETLs,
   compress archives, or scan large JSON files while sending.
5. After client response drain, allow receiver publication, stop through SCM,
   verify final schema-2 metrics and native/parser completion, then notify the
   Ubuntu owner to establish a stable end marker. Final backlog must be zero.
6. Stop only task-owned trace/counter collectors, export counters, and archive
   receiver bytes between stable markers. Hash and preserve the raw ETL, BLG,
   CSV, service metrics, client results, upstream counts, receiver archives,
   manifests, and gate receipts before deciding the next stage.

A latency result includes p99 relative to the earlier baseline. Any fidelity
loss stops ascent for classification; a practical overload includes at least
1% timeouts or requests exceeding the fixed two-second deadline. An isolated RA
flag anomaly remains a DNS quality failure and is reported independently.

### Record the first authoritative limit

The coordinator classified `pooled-auth-64000-60s-1` as a capture-capacity failure
and an offered-rate failure. The client completed all 3,754,406 queries without
DNS errors, but achieved 62,573.43 QPS (97.77% of requested), and the service
dropped 1,409,877 of 7,508,812 received ETW records (18.7763%) at channel admission.
Other intake categories, callback/parser/output/ETW losses, and final backlog
were zero. The sampled loss appears throughout all 60 sending intervals, so a
startup-only explanation is insufficient. CPU/thread evidence must precede
attribution to TDH, locks, or a specific scheduler cause.

Stop authoritative ascent before 128,000 QPS. After exact lossy-ledger/receiver
reconciliation, trace loss/coverage checks, and brief focused trace analysis,
run 48,000 QPS for 60 seconds. Use the second and final midpoint at 40,000 if
48,000 fails, or 56,000 if 48,000 passes. Preserve failed evidence and all fixed
service, VM, client, provider, and durability settings. Before any 300-second
confirmation, independently check both Windows trace and Ubuntu raw/archive
budgets; a storage veto does not establish a performance limit.

The original 48,000 QPS midpoint, `pooled-auth-48000-60s-1`, is retained as a
perturbed measurement. A legacy-derived Status action enumerated 5,000 DNS
records during its sending window, overlapping its only two loss intervals at
send +39.048 and +40.089 seconds. This is a potential observer effect, not proof
of causation. The coordinator authorized one explicit replacement,
`pooled-auth-48000-60s-2`, after correcting the helper. The replacement passes
its complete zero-loss gate; the second and final valid midpoint is therefore
56,000 QPS. Keep the original lossy evidence and its machine-readable
`measurement-confound.json`; exclude it from intrinsic threshold classification.

The management audit also identifies in-load enumeration in the original
authoritative and recursive 1,000 QPS runs and authoritative 8,000 QPS run. Those
stages had zero capture loss, but their CPU measurements include that observer.
No other action receipt overlaps their sending windows; Start precedes client
launch and Stop/Fetch follow drain. The corrected helper restricts DNS inventory
to pre-load/proof actions. Do not call Status during subsequent load; the
existing resource loop reads the native metric file's last two lines instead.

After all discovery and confirmation work, the coordinator authorizes exactly
one clean 300-second 1,000 QPS repeat per mode, using
`pooled-auth-1000-300s-2` and `pooled-rec-1000-300s-2`. Use these as the primary
1,000 QPS CPU/memory/thread evidence. Preserve the original capture successes
and their observer qualifications. No other automatic reruns are authorized.

### Record the recursive bracket and confirmation

Recursive discovery first reaches channel-admission loss at 32,000 requested
QPS: 7,535 of 4,379,440 received records are dropped, while every direct DNS
query receives a valid response. The first midpoint at 24,000 QPS passes all
gates for 60 seconds at 23,800.92 achieved QPS. The final midpoint at 28,000 QPS
has zero event loss and exact delivery of 3,888,766 records at 27,759.65 QPS,
but four received RA=false responses and the exact final-only reset make its
strict overall result a failure. Preserve those qualifications; do not classify
them as missing DNS responses or a channel-throughput failure.

The coordinator authorizes the bounded 28,000 QPS, 300-second confirmation
under the standing qualifications. That confirmation passes every gate:
8,327,473 valid queries at 27,758.24 QPS, actual upstream query/reply counts
1,418,559 each, and 19,492,064 exact records with zero loss or final errors,
SCM exit 0, and queue high water 5,293. Full native WPR covers the sending
interval without event/buffer loss. Together with the clean authoritative
48,000 QPS confirmation, this closes both sustained-rate targets; no lower
confirmation fallback or further ascent is required.

Both 1,000 QPS repeats without in-load management inventory are complete.
Authoritative sends 299,910 valid queries at 999.70 QPS and closes 599,820 exact
records with SCM 0 and high water 3,634. Recursive sends/completes 299,903 queries
at 999.68 QPS, closes 909,954 exact records with SCM 0 and high water 4,786, and
retains one received RA=false response as a strict DNS-validity failure.
Both have zero capture/event loss and valid full native traces. No further
load or rerun follows these repeats; restore normal operation.

The recursive confirmation uses an 8 GiB Windows trace cap and 6 GiB forecast.
Its live receipt records 27,160,457,216 guest free bytes against a required
23,622,320,128 bytes, and 396,148,908,032 host free bytes against a required
34,359,738,368 bytes. Its final native ETL is 5,250,220,032 bytes and transfers
losslessly after stop. Ubuntu's distinct raw/archive forecasts and identities
remain in the fresh per-stage READY receipt; they are not a legacy
double-footprint claim.

### Recover the unused prepared receiver epoch

After the 18th completed stage, the campaign has an approximately twelve-hour
idle gap. Read-only recovery at 2026-10-30 00:03 UTC verifies the exact running
VM, stopped service, matching candidate hash, no WPR recording or campaign
counter/load process, and no Windows 28,000 QPS artifacts. Ubuntu has only an
unused prepared receiver epoch with 1,026 raw bytes and no active connection.
Preserve it under `ubuntu/stale-ready-20261029-1141`, then obtain fresh READY
and clock bounds before the pending midpoint. No interrupted Windows load is
invented and neither VM is power-cycled. The recovery inventory is
`recovery-preflight-20261030.json`.

### Preserve the recurring finish-only reset

The coordinator classified the 56,000 QPS final STOP/FINISH error as an isolated
remote connection reset (`10054`) after every encoded record reached the
receiver. SCM exit 1066 and `output_errors=1` remain failed lifecycle results;
zero `output_failed_events` does not make shutdown clean. The same error chain
exists in the pre-deployment baseline. Its cause remains unresolved.

For each recurrence, preserve raw Windows Application event XML and require
the exact final STOP/FINISH `10054` chain, zero live output errors, stable final
counts, zero backlog, and exact receiver delivery before continuing under the
coordinator's standing exception. Record each occurrence separately. The
receiver owner first takes a read-only stable marker and waits for those
proofs before sealing its archive. A different error chain, mid-load reset,
drain anomaly, or record mismatch still pauses the campaign. Do not change
source, Frame Streams options, or protocol deadlines during this matrix.

A stage has clean capture only when all intake admission losses (`ring_dropped`,
`pool_exhausted`, `receiver_disconnected`), callback errors, parser ignored/
rejected/output-limit losses, output failures, final queue backlog, ETW EventsLost,
and real-time buffer loss are zero; final ETW statistics must be valid. Record
transient statistics-query errors separately. Validate schema-2 final closure:

```text
intake_dropped = ring_dropped + pool_exhausted + receiver_disconnected
received = intake_dropped + callback_errors + dequeued + queue_backlog
dequeued = encoded + ignored + rejected + output_limit_dropped + output_failed_events
```

Use the strict shared `metrics_ledger.py`, never a ring-only gate. Require exact
`encoded = durable_JSON = receiver_ingress = receiver_egress`, no receiver
discard/malformed data, and reconciled event domains/upstream counts. Report
client timeout/send-miss/achieved-rate limits separately. Discovery stops upward
at the first meaningful drop, timeout, rate miss, drain failure, resource guard,
or receiver failure. Preserve that failing stage; no silent tuning or replacement.

## Metrics and analysis

The single recording profile is `scripts/performance/pooled-vm.wprp`,
`PooledVM.Verbose.File`. WPR 10.0.26100 accepted an actual ten-second idle
start/stop probe and produced an 8 MiB ETL in `capture/wpr-probe-3`. The fixed
collector uses 128 buffers of 1,024 KiB. Sampling remains the existing 1 ms
interval; no timer/profile interval is changed. Start before the measured SCM
service to retain thread creation/names and stop after client/native/parser
drain. Keep identical full-stage instrumentation across modes/rates.

The live guest schema requires `DiskIOInit`; the initially suggested
`DiskIOInitialization` spelling was rejected before any load and its diagnostics
remain in `wpr-probe-2`. Active profile details confirm CPU sampling,
CSwitch/ReadyThread, process/thread and loader lifetimes, disk/file I/O, and
hard-fault events with the requested stacks. WPR status reports zero lost events
in the idle probe. This is a profile-support result, not a load result.

File mode does not impose a file-size ceiling. Each stage uses a 4 GiB trace
temporary-data guard, 8 GiB guest-free guard, 20 GiB host-free guard, and Ubuntu's
12 GiB raw/3 GiB free guard with archive headroom forecast. Health checks occur
every five seconds during load. Guard activation stops the stage, preserves
partial evidence, and bounds the result by instrumentation. WPR stop/merge may
temporarily need additional disk headroom. No trace export or compression runs
while the host client is active.

The live Windows ETW temporary file reports zero length while open. Consequently
the 4 GiB guard also checks the decrease in guest free space from preflight,
conservatively counting all guest disk writes. The first stage discovered this
reporting behavior during load and added a separately recorded watcher; later
stages integrate this check into the existing five-second guard loop. This changes
measurement safeguards only, not service, workload, or recording settings.

The coordinator permits a documented trace cap up to 12 GiB for a high-rate
300-second confirmation only. Before raising it, require an explicit ETL
forecast within the cap, guest free space at least twice the forecast plus
8 GiB floor and 2 GiB safety, and host free space at least twice the forecast
plus 20 GiB. Record `-TraceBudgetGiB`, `-ForecastTraceBytes`, and actual headroom
in the stage's `budget-receipt.json`. All discovery stages retain the 4 GiB cap.
Storage guard activation is an instrumentation limit, not a capture-capacity
failure. The full recording profile and other safeguards remain unchanged.

The coordinator also approved lossless transfer compression after WPR has
stopped. A bounded idle probe in `capture/wpr-transfer-probe-1` validates the
built-in Windows PowerShell/.NET `GZipStream` Fastest path: the 7,340,032-byte
guest native ETL and decompressed host ETL have identical SHA-256
`7C1E319268A4F2B8290148169615AFF6CC60379C2847E8CF24967EC5C4BE4B5D`.
The guest original ETL and its hash remain preserved. Later stages may specify
`-CompressTraceTransfer`; they retain both gzip and host raw ETL, source/archive
hash manifests, and a successful `transfer-roundtrip.json` identity receipt
before analysis. Compression, transfer, decompression, and exports occur only
between sending windows and do not change the recording profile or durability.

### Record the confirmation storage boundary

The coordinator approved a temporary 64 GiB receiver data VHDX for the
48,000 QPS, 300-second authoritative confirmation. The original Ubuntu disk and
seed DVD remain unchanged. The verified free attachment is SCSI 0:2; SCSI 0:1
contains the existing DVD and was preserved. The data device is `/dev/sdb`, WWN
`0x600224805c7ffa68166d3a03b164a313`, filesystem UUID
`3046058a-a6b1-4e74-97f5-a63905494a1f`, mounted at
`/mnt/dnstap-pooled-vm-20261029-0942`. It is fully initialized and checked before
load. Only the collector's raw stdout destination changes; its YAML hash,
Frame Streams behavior, CPU, RAM, network, and workload remain unchanged.
Preserve `ubuntu/temporary-data-disk.json` and `ubuntu/collector-data-path.diff`.
This is an explicit receiver-storage difference from the earlier root-volume
stages, not a matched-storage comparison.

The new READY protocol is `separate_raw_data_disk_and_root_archives`, with
`archive_double_footprint_fits=false`. Check distinct live raw/archive volume
identities and free-byte counters. The raw cap is 48 GiB; each volume retains
a 3 GiB free floor and 1 GiB safety allowance. For this confirmation the raw
forecast is 31,680,000,000 bytes, requiring 35,974,967,296 free bytes on the data
volume. The separate root archive forecast reserves 25% of raw size, or
7,920,000,000 bytes, requiring 12,214,967,296 free bytes on root. The READY
receipt must explicitly verify these independent reserves; do not fabricate a
passing legacy double-footprint flag.

For recursive 56,000 or 64,000 QPS discovery only, the coordinator permits a
guard-only raw cap of 24 GiB when the conservative full-stage forecast exceeds
the usual 12 GiB cap. Both live volume reserves must still pass, and the
run-specific READY receipt records the change. This adjusts a storage guard,
not the workload or receiver behavior. Lower discovery rates retain 12 GiB.

The Windows trace forecast is 8,449,425,408 bytes: five times the clean
60-second ETL plus a 20% margin. The coordinator approved a 10 GiB recording
cap. Recheck Windows guest free space at least 27,636,269,056 bytes and host
free space at least 38,373,687,296 bytes immediately before recording. Preserve
the per-stage budget receipt. Full file-mode WPR coverage, buffer settings,
loss checks, and post-stop hash-verified compression remain unchanged.

Microsoft Learn guidance was consulted using its MCP server for
[logging modes and buffer sizing](https://learn.microsoft.com/windows-hardware/test/wpt/logging-mode),
[collector definitions](https://learn.microsoft.com/windows-hardware/test/wpt/1-collector-definitions),
and [general system diagnosis](https://learn.microsoft.com/windows-hardware/test/wpt/recording-for-basic-system-diagnosis).

PerfMon matches earlier metrics: service/DNS CPU in one-core percent and percent
of two-vCPU capacity, user/kernel CPU, private/working/virtual bytes, page faults,
thread/handle count, I/O byte/operation rates; per-thread PID/TID/CPU/context
switches/state/wait reason; guest CPU/queue/memory/disk/network/UDP/TCP/DNS; and
host client CPU/memory. Validate consecutive PID/TID identities and report valid
interval coverage. Separate interval averages from integrated CPU work and
reserved versus resident memory.

Actual WPR analysis must identify service PID, thread names/lifetimes, sampled
CPU stacks where symbols permit, CPU Precise scheduling/Ready/Wait distributions,
context switch/wake cadence, disk/file I/O, and hard-fault coverage. The release
is stripped; function-name attribution may be unavailable, which must be
reported rather than guessed. Check trace event/buffer loss and retained time
bounds. WPR cannot measure application queue, output, receiver, or DNS-client
loss, so stage ledgers and durable receiver checks remain mandatory.

Compare measured pooled five-minute results against the preserved capacity-only
report with workload/event-rate/CPU-normalization context. Instrumentation
differs; a single changed-instrumentation trial cannot establish a causal CPU
improvement. Short discovery stages establish brackets, not certified capacity.
Only the 300-second confirmations support an observed sustained result.

## Restore and deliver

Use the campaign's protected backup and verified task ownership for restoration
or recovery. The completed restoration is recorded in
[Final completion record](#final-completion-record).

### Handle a failed WPR start helper conservatively

Post-load review identifies a runner error-path limitation. The stage controller
sets `traceStarted` only after the WPR Start helper returns. The helper starts
native recording before querying active status and writing `owned-recording.txt`.
A helper failure after successful native start can therefore leave a recording
active while the stage controller still believes it did not start one.

The generic `PooledVM` marker does not uniquely identify a live recording.
Moving it earlier would not make stale-marker cleanup safe. Preserve the failed
helper's logs and active state, reject the next load while WPR is recording,
and establish current recording identity and task ownership before any scoped
recovery. Never stop a recording solely because a campaign marker exists.
Automatic recovery requires a separately validated live-identity protocol and
failure-injection checks; it is not added during this completed measurement
matrix. The post-load review is `build/wpr-cleanup-review-20261030.json`.
No such orphan is observed in this campaign's recovery or final state, and all
measured stages retain complete native trace gates.

### Restore normal operation

If basic candidate capture fails or a lifecycle failure falls outside the
coordinator's exact final-reset exception, retain evidence and restore the
protected baseline binary/hash and normal arguments through SCM. The documented
exception preserves a failed lifecycle result after exact delivery; it does not
invalidate the later clean confirmations or require changing the validated
candidate. If native validation succeeds, retain the reviewed candidate with
original normal arguments, no metrics/capacity overrides, and reviewed defaults.
Remove only the
owned temporary recursive forwarder. Restore and compare DNS settings, binary/
SCM ACLs, SID/privileges, and task-owned collector state. Ubuntu restoration is
owned by its assigned worker. Keep both VMs running. After the last measured
SCM stop, restore the Ubuntu normal receiver first and verify its readiness,
then restore/start the Windows service. This prevents a receiver restart against
the already restored running service.

Verify Running service, exact retained hash, strict service policy, ordinary
authoritative answer/TTL, and visible baseline console CQ/CR output outside any
measurement window. Deliver `docs/pooled-vm-performance-review.md` with build and
deployment identity, approved matrix, pass/fail tables, actual rates/ledgers,
resource and WPR analysis, raw evidence paths, limits, and restoration receipts.

## Final completion record

All 22 measured stages have closed, classified client/native/parser/receiver
evidence and passing native trace integrity gates. Completion does not turn the
four ring-loss stages, RA validation failures, or four exact final-reset
lifecycle failures into passes. Both high-rate 300-second confirmations pass
every gate; the clean recursive 1,000 QPS repeat retains one RA=false response
and therefore fails its separate strict DNS-validity gate. No further ascent,
fallback or rerun occurs.

| Final check | Result | Evidence under the campaign root |
| --- | --- | --- |
| Windows retained candidate | `98FD…1F5D7C`, SCM Running PID 2592, original normal arguments | `windows-campaign/final-policy-proof.json` |
| Windows service policy | LocalSystem, enabled service SID, only `SeChangeNotifyPrivilege`, executable/SCM ACLs match protected baseline | `windows-campaign/final-policy-proof.json` |
| DNS policy and fixtures | Recursion/cache/forwarders match backup, 5,000 A records retained, owned recursive forwarder absent | `windows-campaign/final-policy-proof.json` |
| Task collectors | WPR not recording; no campaign logman collector remains | `windows-campaign/final-policy-proof.json` |
| Ubuntu normal receiver | Protected configuration hash restored, journal/console stdout, original socket sysctls, temporary telemetry/upstream/firewall fixtures removed | `ubuntu/restoration-final.json` |
| Temporary data disk | Exact UUID unmounted with no open descriptors, owned VHDX detached and retained | `ubuntu/temporary-data-disk-unmounted.json`, `ubuntu/temporary-data-disk-detached.json` |
| VMs | Both Running, fixed 2 vCPUs/4 GiB each, original OS disks/DVDs retained | `ubuntu/restoration-vm-proof.json` |
| Ordinary service proof | `www.auth.stress.test` A returns `192.0.2.53`, TTL 300; exact normal console CQ/CR ID 36450 at 00:34:03 UTC | `windows-campaign/final-query-proof.json`, `ubuntu/restoration-console-proof.json` |
| Analysis gate regression | Actual clean, RA-only, final-reset and ring-loss receipts produce distinct event/lifecycle/strict outcomes; report predicate rejects extra client/upstream errors and live/PID states; original summary hashes unchanged | `build/summary-gates-check-20261030-v2.json` |

No production source, pool/capacity, VM resources, timers, cache policy or process
priority changes occur during this campaign. Formatting, strict Clippy, locked
release and PE checks remain the pre-load build evidence; local tests are not
relabeled as new native tests. Retain all original, failed and perturbed
receipts. The final report includes the descriptive historical CPU/memory
comparison and its instrumentation/storage limitations, without a causal
pooling-efficiency claim. Receiver visibility-delay distribution remains
unavailable; no archive is rescanned to manufacture one.

Generate the integrated report only after the classified evidence and restoration
proofs exist:

```powershell
python scripts/performance/build-pooled-report.py `
  target/performance/pooled-vm-20261029-0942 `
  docs/pooled-vm-performance-review.md --complete
```

`--complete` requires exact receiver/domain/ledger closure for all 22 stages,
native trace integrity, lifecycle assessments for nonzero SCM exits, and restored
normal Windows/Ubuntu/storage/console proofs. It accepts preserved classified
strict failures; it does not require every stage to pass.
