# Ring-buffer capacity fix: plan and validation gates

This historical plan helps maintainers review the October 29, 2026 UTC
capacity experiment and its acceptance decisions. The experiment is complete;
the [validation report](ring-buffer-fix-validation.md) preserves its outcome.
References to deployment approval and the installed service describe that
campaign. The later pooled implementation and VM validation are documented in
[Pooled ETW intake](pooled-intake.md) and the
[pooled VM review](pooled-vm-performance-review.md).

Target: zero observed ring drops during separate 300-second authoritative and
recursive-request tests at a **global 1,000 QPS ceiling**, while preserving the
bounded ring, SCM-only security profile and existing ownership/shutdown rules.
The claim is limited to those measured five-minute workloads, not every load.

**The campaign's approved final candidate is capacity-only v2, default 8,192 slots.**
The wake-only v1 at 1,024 failed its authoritative gate and is retained as an
experiment, not the production fix. Its recursive run was completed only for
the user's matched CPU comparison. No official wake-v1 acceptance run occurred.
The coordinator approved two official v2 runs after a new source/security/build
gate, based on zero-drop original-binary 8,192-slot short controls.

No new load runs until the coordinator approves the experiment matrix and the
sole Ubuntu owner declares the isolated upstream/receiver ready. Candidate
deployment also requires the production-code security/review gate.

## Delegation and context

The recorded work assignments were:

| Chunk | Model / effort | Owner / context | Outputs and acceptance |
| --- | --- | --- | --- |
| Coordination / matrix | GPT-6-Astra / high | Coordinator; prior capped-run ledgers and WPA findings | Approved bounded matrix, decisions on failures, final evidence review |
| Diagnosis / security review | GPT-6-Astra / high, read-only | `perf_review`; source, prior traces, callback/queue/shutdown boundaries | Race/lifetime/boundedness assessment, explicit pre-deployment code gate and final read-only review |
| Production fix | GPT-6.1-Sol / high | `ring_fix`; baseline source and diagnosis | Preserved/rejected wake-only v1; final default-capacity-only v2, focused tests, strict checks, hardened release hash; no VM actions |
| Windows experiment / reports | GPT-6.1-Sol / high | `perf_windows`; existing VM/SCM and counter helpers | This plan, protected original binary/current-state backup, deployment/orchestration, raw evidence, final validation report |
| Ubuntu workload / receiver | GPT-6.1-Sol / high | Recalled after a slot opens; sole Ubuntu configuration owner | New campaign backup, controlled upstream, durable receiver/metrics, no-drop gates, exact counts/archives/restoration |

Sol workers author all source, scripts and documents. The initial slots are
coordination, implementation, Windows preparation and read-only review; Ubuntu
preparation starts only after an implementation/review slot becomes available. No agent
starts a second configuration owner or performs uncoordinated VM mutations.

## Baseline and preserved artifacts

Use a new campaign root `target/performance/ring-fix-20261029`, with baseline,
candidate manifests, per-run evidence and restoration receipts. Preserve all
previous `target/performance` campaigns, archives, configuration backups and
reports.

Before the release build can replace the target executable, preserve and
hash-verify the current baseline:

```text
baseline service SHA-256
0FCDD17E08A3DDD42F4F132F7F03F80A5513FEBC66E54F90A5EBBC8135BE2C1D

unchanged client SHA-256
7A5800CFC4252196E4C6162F7D2CCF81217FE95A339D7CFBB9CD84063F4B6960
```

The prior capped tests achieved 999.69 QPS with zero client errors and ETW
EventsLost, but had ring drops of 148,638/599,814 (24.78%) authoritative and
341,569/916,490 (37.27%) recursive. Their encoded/file/ingress/egress counts
matched. Low-load WPA parser waits had approximately 16 ms medians, with little
Ready time, despite a source request for a two-millisecond empty-ring park.
Those observations motivate a wakeup investigation; they do not by themselves
prove a specific timer or locking cause.

## Fixed environment and change boundaries

- Windows VM: `DNSTap-WS2025`, GUID
  `24bafedb-0998-4430-a0c6-7e90350cd9ac`, `172.27.215.163`, 2 vCPUs, 4 GiB RAM.
  Verify live identity, resources and readiness before acting.
- Original service: Running, SCM-only, LocalSystem, enabled service SID and
  only `SeChangeNotifyPrivilege`; normal arguments omit `--metrics`.
- Retain `auth.stress.test` with exactly 5,000 A records, `192.0.2.53`, TTL 300.
  Source/name hashes and license remain those in
  [`workload-provenance.json`](../datasets/performance/workload-provenance.json).
- Ubuntu `172.27.215.60`: baseline console receiver TCP 6000 and original
  receive-buffer settings. Only its assigned owner changes the temporary
  recursive upstream, receiver logging, counters and restoration.
- Use PowerShell Direct and the protected DPAPI credential; never print it.
  Save current SCM/DNS/security settings and the guest binary in a **new**
  protected campaign directory. Do not reuse old `Prepare`/`Restore` backups.
- The wake experiment kept capacity **1,024** to isolate notification; it did
  not meet the target. Final v2 raises only the default to **8,192**, reverting
  wake production changes and retaining original polling/flush behavior.
  No foreground mode, privilege expansion, timer-resolution change,
  global cache-policy change, VM resize or unrelated host/network changes.

## Approved experiment matrix

The coordinator approved the following matrix. All runs use the same global
1,000-QPS ceiling, two client workers, 256 outstanding requests per worker and
2,000 ms deadlines:

| Order | Run ID | Binary | Ring capacity | Sending seconds | Purpose |
| --- | --- | --- | ---: | ---: | --- |
| 1 | `ringfix-baseline8192-auth-30s-1` | Protected original | 8,192 | 30 | Capacity-only authoritative diagnostic |
| 2 | `ringfix-baseline8192-rec-30s-1` | Protected original | 8,192 | 30 | Capacity-only recursive diagnostic |
| 3 | `ringfix-baseline1024-auth-30s-1` | Protected original | 1,024 | 30 | Matched authoritative CPU control |
| 4 | `ringfix-baseline1024-rec-30s-1` | Protected original | 1,024 | 30 | Matched recursive CPU control |
| 5 | `ringfix-wake1024-auth-pilot-1` | Wake v1 | 1,024 | 30 | Failed authoritative gate and CPU arm |
| 6 | `ringfix-wake1024-rec-pilot-1` | Wake v1 | 1,024 | 30 | Diagnostic-only CPU arm, not acceptance |
| 7 | `ringfix-capacity8192-auth-300s-1` | Capacity-only v2 | 8,192 | 300 | Official authoritative acceptance |
| 8 | `ringfix-capacity8192-rec-300s-1` | Capacity-only v2 | 8,192 | 300 | Official recursive acceptance |

The implementation/security gate precedes candidate deployment. The original
binary capacity-only diagnostics may run during candidate code validation
only after the Ubuntu owner declares the fixture ready. The earlier original
capacity-1,024 five-minute runs remain the fidelity reference. The user also
requested a CPU-impact comparison, so the two new original-1,024 30-second
controls provide a matched-duration, no-WPR reference rather than comparing
older five-minute traced CPU numbers to short untraced runs.

Dependencies remain:

1. Read-only diagnosis, protected baseline backup and candidate code review.
2. The original-binary capacity diagnostics and matched original-1,024 CPU
   controls, then one 30-second candidate pilot per mode using the unchanged
   global-rate client. Do not use WPR in any 30-second comparison arm.
3. After the failed wake gate, review/deploy capacity-only v2 and verify its
   CLI default and explicit run manifests select 8,192. No further pilot is
   required: the old-binary 8,192 controls use the same production queue/wake
   behavior. Run authoritative then recursive-request for 300 seconds each.
4. Final drain, complete stage/receiver reconciliation, post-fix WPA comparison
   and restoration.

The capacity comparison addresses the user's hypothesis but is descriptive,
not full acceptance. Capacity 8,192 provides eight times as many entry slots;
record measured memory and burst occupancy rather than assuming the memory
cost is free. Maximum retained copied payload could grow from approximately
128 MiB to 1 GiB, excluding slots/descriptors/allocator overhead and in-flight
records; this is a theoretical bound, not expected RSS. Final acceptance uses
8,192 only after the explicit revised coordinator decision, not a silent
capacity increase. Wake-v1 source/binary/receipts and failures remain intact.
Use numbered retry IDs only after an evidence-based revision approved by the
coordinator. Never overwrite a failed run.

### CPU-impact comparison

For each mode compare three matched 30-second/no-WPR arms: original/1,024,
original/8,192 and wake candidate/1,024. Report duration-weighted service CPU
in one-core percent and guest-capacity percent, integrated CPU-seconds and
valid interval coverage, DNS/guest CPU, memory, actual query/upstream/received/
encoded work, and stage drops. Show change in percentage points; relative
changes are secondary and unstable near zero.

Rate-normalized estimates use mean service CPU fraction divided by observed
full-run query or encoded-record rate (CPU ms/query and CPU microseconds per
encoded record). Label the representative-coverage assumption and final-drain
boundary approximation. Do not divide partial-coverage CPU-seconds by full-run
counts and present that as exact. The lossy original arm processes fewer
records, so work differences matter when interpreting CPU.

These are single-shot exploratory arms. Short-run variation and CPU accounting
quantization may prevent a reliable ranking; report that limit instead of
declaring an automatic efficiency winner. Additional matched repeats need a
coordinator decision after seeing the data; none are added by this plan.

## Load, metrics and exact acceptance

The same compiled host Rust client must target the VM explicitly:

```text
--server 172.27.215.163:53 --names NAMES --mode MODE --threads 2 --duration 300 --inflight 256 --timeout-ms 2000 --output-dir NEW_RUN_DIRECTORY --qps 1000
```

This is an argument vector. Replace `NAMES` with the generated names-file path,
`MODE` with `authoritative` or `recursive`, and `NEW_RUN_DIRECTORY` with a
fresh result directory when constructing the recorded client invocation.

`--qps 1000` is a global ceiling: two workers each request a 2 ms interval.
It permits paired microbursts and does not guarantee exactly 300,000 sends.
Record the expanded command, executable and names hashes, actual sends/rate,
monotonic duration, all response/flag/error counters and latency-bin upper
bounds. Sending windows last 300 seconds; bounded response drain is separate.

For every candidate acceptance run require:

- Approximately 1,000 achieved QPS over the full configured sending interval,
  with all DNS responses passing the client's checks. Independent probes
  verify expected A values, TTLs and fixture scope.
- **Zero** ring drops, callback-copy errors, parser rejection/ignored records,
  output-limit/failed-frame/operation errors, and final queue backlog.
- Valid final ETW statistics with **zero** EventsLost, real-time buffers lost
  and statistics-query failures.
- Stable final stage ledgers and exact encoded = durable JSON = receiver
  ingress = receiver egress, with zero receiver discards/malformed records.
- Zero upstream socket/kernel UDP errors. Use actual upstream query/reply
  counts and background/zone separation; never assume four recursive events
  per client query or one cache miss per repeated name.

After the client and response drain, allow at least three seconds for receiver
counter publication, stop the service to drain the native consumer/parser,
then settle the end byte-offset marker. Check only stable final schema 1 ledgers:

```text
received = ring_dropped + callback_errors + dequeued + queue_backlog
dequeued = encoded + ignored + rejected + output_limit_dropped + output_failed_events
```

Keep ETW event and buffer loss separate, and check event domains before any
combined denominator. A nonzero acceptance metric stops the pass decision:
retain evidence, notify the coordinator, diagnose, revise only with evidence,
and rerun the affected official workload under a new ID. Do not silently raise
capacity, lower rate or suppress errors to meet the target.

Collect one-second exact-PID PerfMon BLG/CSV service/DNS CPU, private/working/
virtual memory, faults, thread/handle and I/O rates; thread PID/TID/CPU/context
switches/state; guest CPU/queue/memory/disk/network/DNS; host client CPU/memory;
and Ubuntu upstream/receiver/UDP/disk resources. Validate consecutive identities
for rates, report actual sample counts/coverage, and distinguish interval
averages, CPU work and memory reservations.

Take short bounded WPR recordings during load. After all relevant sending
windows finish, inspect retained coverage and export matched interior views
for parser/consumer CPU, Ready/Wait distributions and wake cadence. State
boundary, circular-retention, instrumentation and timer-cause limitations.
No heavy trace exports, compression or full JSON scans during measurement.

## Deployment, restoration and final evidence

Do not deploy until implementation checks and the independent security gate
pass. Deploy only to the verified lab VM with its service stopped, preserve
installed executable ACLs and strict token policy, record candidate hash and
capacity, then start through SCM with a unique protected metrics path.

After validation, retain the verified capacity-only v2 binary, restore the
original normal arguments without metrics or a capacity override (the reviewed
new default selects 8,192), retain the
authoritative zone, remove only the new owned recursive forwarder, and have
the Ubuntu owner restore console/sysctls/firewall/upstream/monitor state.
Finish/delete only task-owned trace/counter collectors. Verify Running service,
strict identity/settings, final binary hash, authoritative A answer/TTL and
visible ordinary CQ/CR console output. Both VMs remain running.

Final results, candidate provenance, reviewed decisions, comparison against the
unchanged baseline, pass/fail ledgers, resource/WPA evidence and new restoration
receipts go in [`ring-buffer-fix-validation.md`](ring-buffer-fix-validation.md).

## Recorded outcome

Both official 300-second runs met the zero-ring/callback/parser/output/ETW-loss
and exact durable-delivery gates with default 8,192. The recursive client had
one RA-check failure, so the original all-DNS-validity gate was **not met**.
It remains an explicit unresolved DNS-behavior finding; the criteria were not
relaxed and no automatic rerun hid it. See the [validation report](ring-buffer-fix-validation.md).
