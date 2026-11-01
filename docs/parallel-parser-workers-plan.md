# Parallel parser workers plan

**Status:** Stage P0 was completed on October 31, 2026. The P1 decision
selects the new Stage P1a, a scheduling-priority experiment, and defers the
worker implementation (P4). Stages P0, P1a, P2, and P6 change lab state and
need explicit authorization before they run.

## Goal

Raise the event rate the service can deliver without loss. The proposed design
keeps one ETW session and one callback thread, all feeding the existing
queue. N identical workers pull from that queue. Each worker parses, encodes,
and sends over its own TCP connection to the receiver.

Do not build anything before diagnosing the bottleneck. The evidence below
suggests the parser thread was mostly waiting rather than computing, so extra
workers help only if the wait is on something per-connection.

## Evidence (Stage L5, October 31, 2026)

Source: `target/live-checks-20261031/heavy-load-results.md` and the
[live load validation plan](live-load-validation-plan.md#stage-l5).

- **Loss at uncapped load.** At about 160,000 queries per second (about
  320,000 events per second), every buffer setting lost events. With the
  defaults (H3), the service dequeued about 123,000 events per second, the
  intake queue dropped 11.4 million events, and ETW lost 0.67 million.
- **The parser was mostly waiting.**
  - Whole-process `etw-dnstap` CPU was 12.7% of one core in H3 and 13.9% in
    H2. That figure includes the ETW callback, which handled about 256,000 to
    314,000 events per second.
  - Total CPU per dequeued event was therefore at most about 1 µs. A
    saturated parser would handle well over 500,000 events per second.
  - So while the queue overflowed, the parser thread was busy only about 10%
    of the time.
- **Where it can wait.** When the queue is not empty, `parse_loop` waits only
  in the blocking TCP send (`write_frame` and the periodic `flush`).
  - It is therefore most likely blocked by TCP backpressure from the receiver.
  - A weaker hypothesis is that the idle park (2–20 ms requested; Windows timer
    granularity can stretch it to about 15.6 ms) misses ETW delivery bursts.
    Park latency alone would cap throughput near 400,000 events per second,
    well above the measured rate, so it is unlikely to be the main cause.
- **The receiver drops records.** The collector journal for October 31 holds
  `worker[console] buffer is full, 221459 dnsmessage(s) dropped` and
  `worker[dnstap-processor] buffer is full, 7596 dnsmessage(s) dropped`.
  - The lab collector routes every record to a JSON `stdout` logger. That
    output goes to `journal+console`, and journald applies its default rate
    limit.
  - At high rates the lab therefore cannot confirm end-to-end delivery. The
    service's `encoded` counter means accepted by the socket writer, not
    received.

## Design under evaluation: N workers on one ETW feed

| Piece | Change |
| --- | --- |
| ETW session and callback | Unchanged: one session, one `ProcessTrace` thread, a nonblocking `try_send`, and no wake-up. |
| Queue | The existing bounded crossbeam channel. Each worker holds a cloned `Receiver`. The channel delivers every event to exactly one consumer, so no dispatcher is needed. |
| Flag | `--workers N` from 1 to 4, default 1. With 1 worker, behavior and output match today exactly. |
| Worker thread | `dnstap-parser-<i>`. It owns its own `etw::Parser` (schema cache), `Encoder`, and `NetworkSink`, and runs the existing `drain_ring` and `parse_loop` logic. |
| Startup | Connect and negotiate all N sinks before starting ETW, inside the startup watchdog. If any connection fails, finish or close the ones already open and fail startup. |
| Pool | Raise the regular slots from `capacity + 2` to `capacity + N + 1`, because each worker can hold one lease while parsing. Jumbo slots are unchanged. |
| Counters | Each worker gets its own `ParseCounters`. `report_counts` adds them up and also logs per-worker `encoded` counts so an imbalance is visible. This avoids sharing atomic cache lines across cores at two updates per event. |
| Supervision | The supervisor loop treats any worker that finishes early as an unexpected exit. |
| Shutdown | Stop and join `ProcessTrace`, publish `producer_done`, and unpark every worker. Each worker drains until the queue is empty, then sends its own STOP/FINISH. Join all workers. The finishes run in parallel under the single existing 10-second watchdog. Any worker error fails the service. There is no reconnect or replay. |
| Memory | Adds about 140 KiB per extra worker (encoder output, 64 KiB write buffer, parser scratch, and schema cache), plus N extra pool slots. |

**Ordering contract.** Records leave in completion order per connection.
A query and its response can travel on different connections, and records
within one connection are not in global time order.

- dnstap itself does not require pairing.
- Collector-side query/response matching would see only the records on its
  own connection. Examples are latency transforms and anything else that
  joins a query to its response.
- Preserving affinity would need routing by a key such as the client port
  before parsing. That conflicts with a shared feed, so it is out of scope.
- Document the contract in the README.

**What it can and cannot fix:**

| Limit | Effect of N workers |
| --- | --- |
| A single TCP stream or the collector's per-connection reader or processor | Scales with N. DNS-collector creates a processor per connection. |
| The receiver's shared output stage (JSON `stdout`, journald, console) | No gain, because every connection feeds it. |
| Idle park latency | Small gain, because all N workers rarely sleep at once. |
| The single `ProcessTrace` callback thread | No gain. This is the ceiling of any shared-feed design. |

The [parser and TCP threading analysis](parser-tcp-threading-analysis.md)
covers the alternative topologies, the lease rules, and the shutdown
invariants in depth. This plan uses its "private parser/encoder/TCP loops"
option and needs no encoded-output queue.

## Stage P0: Diagnose the bottleneck (lab; requires authorization)

**Model:** Sonnet at medium effort.

**Prerequisites:**

- The lab identity checks and restore rules from the
  [live load validation plan](live-load-validation-plan.md#lab-identity-verify-before-every-stage)
  apply.
- Use the Stage L5 candidate binary
  (`target/live-checks-20261031/candidate-etw-dnstap.exe`) with default
  flags.
- Use the Stage L5 client settings: `--threads 4 --inflight 256`, uncapped,
  60 seconds.

**Receiver change (D3 only).** Add a temporary systemd drop-in that sets
`StandardOutput=null` and keeps `StandardError=journal` for `dnscollector`,
then restart the collector.

- Collector warnings, including `buffer is full` drops, stay visible in the
  journal.
- Remove the drop-in and restart the collector after D3.
- Record the collector's `MainPID` and unit text before and after.
- Do not edit `config.yml`.

**Runs.** Each run repeats H3 with the change listed:

| ID | Change from H3 | Purpose |
| --- | --- | --- |
| D1 | None. Add per-thread CPU sampling for `etw-dnstap`: `\Thread(etw-dnstap/*)\% Processor Time` and `\Thread(etw-dnstap/*)\ID Thread`, mapped to thread names with a one-time `Get-Process` thread listing. On the receiver, log `ss -tni '( sport = :6000 )'` every second (Recv-Q, Send-Q, rtt, cwnd). | Show whether the parser thread is near 100% or mostly waiting, and whether the receiver socket's Recv-Q stays full. A full Recv-Q means the receiving application is slow. |
| D2 | `--capacity 16384` | Test the burst and park-latency hypothesis. A large fall in `ring_dropped` points to bursts rather than throughput. |
| D3 | The receiver drop-in above | Remove the receiver's output stage. If drops mostly disappear, the receiver output is the limit. |

For every run, record:

- the service counters from the "capture stopped" event (ID 2);
- `logman` statistics;
- client sent, successful, and timeout counts;
- mean CPU for `dns`, `etw-dnstap`, and total, plus the parser thread's CPU
  in D1;
- collector `buffer is full` warnings since the run started;
- receiver CPU per process (`top -b -n 1 -H -p $(pidof dnscollector)` every
  5 seconds), including journald.

Write the results to `target/parallel-workers-20261031/diagnostics.md` and
record a short completion record here.

## Stage P1: Decision gate (coordinator)

Choose the next stage from the P0 evidence:

| Observation | Next step |
| --- | --- |
| D3 removes most loss | The receiver output is the limit. Make the lab receiver capable first (Stage P2). Then rerun D1 to find the service-side limit before P3 or P4. |
| D1 shows the parser thread waiting while receiver Recv-Q is near zero and the collector is not CPU-bound | The wait is in the service's send path or the single stream. Proceed with Stage P4 (workers). |
| D1 shows the receiver Recv-Q full and a single collector processor thread near 100% | The per-connection collector limit. Proceed with Stage P4 (workers). |
| D2 cuts `ring_dropped` sharply | Do Stage P3 (park refinement) first. Reassess P4 afterwards. |
| The parser thread is near 100% of one core | CPU-bound parsing. Proceed with Stage P4. More workers give direct CPU parallelism. |
| ETW loss (`etw_lost`) dominates once queue drops are gone | The single callback thread is the limit. Evaluate the callback thread priority experiment in Stage P6 before any multi-session design. |
| The VM is CPU-saturated and the service threads are mostly Ready (runnable but not scheduled) | CPU contention with DNS Server. Run Stage P1a (scheduling priority). Defer P4: more threads at the same priority compete for the same saturated CPUs. |

Record the decision, with the numbers behind it, in this plan.

## Stage P0b: Repeat the diagnosis on 6 vCPUs (lab)

On October 31, 2026 the user raised `DNSTap-WS2025` from 4 to 6 virtual
processors and asked for the bottleneck tests to be rerun. This stage repeats
P0 exactly, so the results compare directly:

- the same candidate binary, client settings, samplers, and D3 receiver
  drop-in;
- one extra run, D1b, which repeats D1 to measure run-to-run spread.

**Model:** Sonnet at medium effort.

**Evidence:** `target/parallel-workers-20261031-6cpu/`.

**Comparison.** Compare each run with its 4-vCPU counterpart and answer two
questions:

- Does loss fall when the VM is no longer saturated?
- Which P1 row matches now?

## Stage P0c: Attribute the parser's waiting (code and lab; proposed)

On 6 vCPUs the parser still dropped about 27% of events while it used about
17% of one core and was Waiting in about half its samples. The thread-state
counters cannot say what it waits on. There are two candidates:

- **Idle park.** The ring briefly empties, and the parser parks. Windows
  timeouts can stretch the requested 2–20 ms to about 15.6 ms, and an ETW
  burst then overflows the ring.
- **Blocking send.** The parser waits in `write_frame` or `flush` for the
  socket.

**Step 1 (code; Sonnet at medium effort).** Add a diagnostic build behind a
non-default Cargo feature `diag-timing`. Release builds without the feature
must be byte-for-byte unaffected in behavior. Per parser, measure:

- the number of parks and the total time parked: `Instant` around
  `park_timeout` in `drain_ring`;
- the total time inside `write_frame` and `flush`, and how many calls took
  longer than 1 ms;
- the queue length (`queue.len()`) seen at each wake after a park: maximum,
  and a coarse histogram with buckets 0, <1k, <4k, <8k, and full;
- the events drained per wake.

Report these in the "capture stopped" entry. Unit-test the bookkeeping. Run
the usual checks with and without the feature.

**Step 2 (lab; Sonnet at medium effort; requires authorization).** Rerun D1
and D2 from Stage P0b with the diagnostic build on the 6-vCPU VM.

**Decision:**

| Result | Next step |
| --- | --- |
| Time parked dominates the parser's non-running time, and wakes often find a full or nearly full queue | Stage P3 (spin or yield before parking, or a high-resolution wait) |
| Time in `write_frame` and `flush` dominates | Stage P4 (workers, each with its own connection), or a dedicated writer thread |
| Both | P3 first, because it is smaller, then remeasure |

## Stage P3r: Latency and headroom experiments (revised P3; proposed)

P0c showed three things:

- **Parked time.** Under load the parser was parked for 48–55% of the load
  window. The mean park was 9–12 ms, although every park after work requests
  2 ms. Windows' default timer granularity (about 15.6 ms) stretches short
  waits.
- **Little write time.** Writes took 6–7% of the load window.
- **Unaccounted time.** A further 17–21% was neither parked, writing, nor
  running, which is most likely Ready time.

At about 300,000 events per second, an 8,192-slot ring fills in about 27 ms,
so both long parks and preemption overflow it. Doubling the ring (T2) cut
drops by 42%. Spinning briefly after work (the original P3) would not cover
gaps that last milliseconds, so this stage tests three cheaper levers
separately and together.

**Step 1 (code; Sonnet at medium effort).** These changes stay behind the
`diag-timing` feature so they can be toggled at runtime:

1. **`--diag-timer-resolution-ms <1..=15>`.** Call `timeBeginPeriod(n)` at
   capture start and the matching `timeEndPeriod(n)` on every exit path.
   Since Windows 10 version 2004 this affects only the calling process
   ([timeBeginPeriod](https://learn.microsoft.com/windows/win32/api/timeapi/nf-timeapi-timebeginperiod)).
   Microsoft recommends high-resolution timers over raising the resolution.
   If this lever wins, replace it in production with a high-resolution
   waitable timer (`CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`) and an event for
   unpark.
2. **`--diag-thread-priority above-normal|highest`.** Call `SetThreadPriority`
   on the `dnstap-parser` and `etw-consumer` threads.
3. **A higher `--capacity` ceiling.** Raise the maximum from 16,384 to 65,536
   in all builds, and keep the default at 8,192. Update the pool memory
   figures in the README and `docs/pooled-intake.md`.

**Step 2 (lab; Sonnet at medium effort; requires authorization).** Run the
P0c T1 conditions (uncapped) and R0 (`--qps 50000`) on the 6-vCPU VM. Collect
the diagnostic timing for each:

| ID | Timer resolution | Priority | Capacity |
| --- | --- | --- | --- |
| X0 | default | normal | 8,192 |
| X1 | 1 ms | normal | 8,192 |
| X2 | default | above-normal | 8,192 |
| X3 | default | normal | 65,536 |
| X4 | 1 ms | above-normal | 65,536 |

For each run, record:

- loss;
- mean park;
- parker share;
- the wake histogram;
- parser and consumer CPU and Ready shares;
- the DNS impact: client rate, timeouts, and `dns` CPU.

**Decision:** productionize the smallest combination that removes most of
the loss with no material cost to DNS. Workers (P4) remain deferred unless
the parser becomes CPU-bound afterwards.

## Stage P3s: Confirm and productionize (proposed)

**User decision (October 31, 2026).** Keep the current thread priority
(normal). Do not add a priority flag. Test a queue capacity of 32,768.

**Step 1 (lab; Sonnet at medium effort; authorized).** Use the P3r diagnostic
build (`F10F3B5A...9E14`) on 6 vCPUs with no timer or priority lever. Run in
this order so that the upward drift in offered load is spread across the
settings:

| Run | Flags | Load |
| --- | --- | --- |
| X0-max | none (capacity 8,192) | uncapped |
| X6n-max-a | `--capacity 32768` | uncapped |
| X3r-max | `--capacity 65536` | uncapped |
| X6n-max-b | `--capacity 32768` | uncapped |
| X6n-50k | `--capacity 32768` | `--qps 50000` |

Record the P3r metrics for each run, plus the peak queue depth
(`wake_len_max`) and the private bytes.

**Step 2 (code; only if the user selects a new default).**

1. **Capacity.** Change the default `--capacity` to the value the user
   selects from the step 1 results.
2. **README.** Update the memory figures, and document `--capacity` as the
   lever for high load.
3. **Timer and priority.** Leave both levers in the diagnostic build only.

## Stage P1a: Scheduling priority experiment (lab; requires authorization)

**Model:** Sonnet at medium effort.

**Purpose.** Test whether the capture threads lose events because DNS Server
takes their CPU time. The service needs only about 0.35 of a core at about
200,000 queries per second: the P0 parser and consumer threads together.

**Steps.** Use the P0 setup: the Stage L5 candidate, the D1 client settings,
and the same samplers, including thread state.

1. **No code change.** Raise the service's priority class at runtime right
   after each start, inside the PowerShell Direct session:
   `(Get-Process etw-dnstap).PriorityClass = '<class>'`. Run 60 seconds each:

   | ID | Priority class | Load |
   | --- | --- | --- |
   | E1 | `Normal` (repeat of D1, for run-to-run spread) | uncapped |
   | E2 | `AboveNormal` | uncapped |
   | E3 | `High` | uncapped |
   | E4 | `Normal` | `--qps 100000` |
   | E5 | The best class from E2 and E3 | `--qps 100000` |

2. **DNS impact.** For each run, record:
   - the client's achieved rate, timeouts, and any latency figures in
     `summary.json`;
   - `dns` CPU.

   A priority change that removes loss but materially cuts the DNS rate or
   raises timeouts has a real cost to operators, and must be reported that
   way.
3. **Decision.**
   - If E2 or E3 removes most `ring_dropped` with an acceptable DNS impact,
     implement a code change. Either set `THREAD_PRIORITY_ABOVE_NORMAL` on the
     `etw-consumer` and `dnstap-parser` threads, or add an opt-in
     `--priority` flag. Sonnet at medium effort writes the change, followed
     by an Opus review.
   - If priority does not help, consider more vCPUs for the lab VM. The VM
     has 4 and the host has 24. That is a VM configuration change and needs
     separate authorization.
   - Revisit P4 only after one of these. More workers at the same priority
     would only win CPU by adding runnable threads, which takes CPU from DNS
     less controllably than a priority setting does.

## Stage P2: Receiver able to measure (lab; requires authorization)

**Model:** Sonnet at medium effort.

The lab collector must keep up with the service, or no Windows-side
improvement can be shown. Choose one option and agree it with the user before
changing anything:

- **Option A: null output.** Make the P0 D3 drop-in permanent for
  performance runs. Delivery then rests on the absence of collector drop
  warnings plus the service counters.
- **Option B: output to a file on the data disk.** Attach and mount the
  performance data disk with `scripts/performance/ubuntu-performance-data-disk.ps1`
  and `prepare-ubuntu-data-disk.py`. Then count JSON lines per run. This
  needs about 1 KB per record of disk space, so size runs to fit.
- **Option C: larger collector buffers.** Raise the collector's worker
  buffer sizes in `config.yml`. This is the first change to the collector
  configuration in this lab, so back up the file and record the diff.

Either way, record a no-loss baseline at the Stage L3 rate (47,000 queries
per second) to prove the receiver itself drops nothing.

## Stage P3: Idle park refinement (code; only if P1 selects it)

**Model:** Sonnet at medium effort.

In `drain_ring`, after a receive succeeds, poll with `std::hint::spin_loop` or
`thread::yield_now` for up to about 100 µs before the first timed park. Keep
the 2–20 ms backoff for genuine idle periods.

**Constraints:**

- The callback stays wake-free (see [the rejected wake experiment](ring-wakeup.md)).
- The idle wake-up rate must not rise. Spinning happens only immediately
  after work.
- Shutdown and final-drain behavior are unchanged.

**Tests:** extend the existing `idle_poll_after` and final-drain tests. Run
the full check set listed under Stage P4.

## Stage P4: `--workers` implementation (code)

**Model:** Sonnet at high effort. The stage changes concurrency, startup
rollback, and shutdown.

Implement the design table above. Specifically:

1. **CLI.** `--workers` (u8, default 1, range 1–4). The help text states the
   ordering contract.
2. **`run_capture`.**
   - Connect N sinks in sequence. On a failure, call `finish` (or drop) on
     the sinks already connected and return the error.
   - Spawn N workers, each with a cloned `Receiver` and its own
     `Arc<ParseCounters>`.
   - Supervise them all, and unpark them all at completion.
   - Join them all, and combine the results so that the first error wins and
     the others are attached as context.
3. **Pool.** `PoolManager::new(capacity, jumbo, workers)` provisions
   `capacity + workers + 1` regular slots. Update the memory figures in
   `docs/pooled-intake.md` and the README.
4. **Counters.** `report_counts` sums the per-worker counters and logs
   `encoded_per_worker`. The totals keep their current field names, so
   existing tooling still works.
5. **Startup log.** Add `workers` to the "capture started" entry.
6. **Tests.** Use the existing fake-receiver test patterns in
   `src/pipeline.rs` and `src/network/shutdown_tests.rs`:
   - Two workers, both receivers get records, and the total equals the
     events sent.
   - The second connection fails at startup: the first is closed and an
     error is returned.
   - One worker's receiver fails mid-run: the service fails, and the healthy
     stream is finished or closed inside the deadline.
   - Final enqueue races completion with N workers: no event is left in the
     queue.
   - With `--workers 1`, counters and bytes match the previous single-worker
     path.
   - Pool slot count for N.
7. **Docs.** README: the flag, the ordering contract, per-worker memory, and
   the effect on collector-side pairing. Add a row to `docs/README.md`.
8. **Checks:**
   - `cargo fmt --check`
   - `cargo clippy --locked --all-targets -- -D warnings`
   - `cargo test --locked --lib` (the count was 119 passed and 5 ignored
     before this work)
   - `git diff --check`

## Stage P5: Review

An Opus reviewer at high effort checks:

- the startup rollback, supervision, and shutdown ordering;
- final-drain correctness with multiple consumers;
- the pool budget;
- counter aggregation;
- that `--workers 1` matches the previous behavior;
- documentation accuracy.

A Sonnet agent at medium effort fixes the findings.

## Stage P6: Live comparison (lab; requires authorization)

**Model:** Sonnet at medium effort.

**Prerequisites:** P2 is complete, and so are P3 or P4 (or both) and P5.

1. **Load matrix.** At uncapped load and at fixed rates of 60,000, 80,000,
   and 100,000 queries per second, run `--workers` 1, 2, and 4, 60 seconds
   each. Record:
   - service counters;
   - per-worker `encoded`;
   - collector drop warnings;
   - CPU for `dns`, `etw-dnstap`, and total, plus receiver CPU;
   - client results.
2. **Optional: callback thread priority.** Only if ETW loss remains after
   queue loss is gone, test a build that raises the `ProcessTrace` thread to
   `THREAD_PRIORITY_ABOVE_NORMAL`. Compare `etw_lost` at the same load.
3. **Supported rate.** Report the highest zero-loss rate for each worker
   count. That gives a supported maximum rate for the README.
4. **Restore.** Restore as in the live load plan.

## Out of scope

- **Multiple ETW sessions.** Separate sessions per worker, split by event ID
  or by payload `BETWEEN` filters on `Port` or `XID`, were considered and
  rejected for this plan, because the workers must share one feed. ETW allows
  up to eight sessions per manifest provider. Payload filters are evaluated
  per session after the event is generated
  ([EnableTraceEx2 remarks](https://learn.microsoft.com/windows/win32/api/evntrace/nf-evntrace-enabletraceex2),
  [PAYLOAD_FILTER_PREDICATE](https://learn.microsoft.com/windows/win32/api/tdh/ns-tdh-payload_filter_predicate)).
  Revisit this only if the single callback thread is shown to be the limit.
- **Async I/O rewrites.**
- **Global reordering.**
- **Reconnect or replay.**

## Completion record

### Stage P0

Run October 31, 2026 on `DNSTap-WS2025` (4 logical processors) with the Stage L5 candidate
(SHA-256 `51D7DF2F...7A8F`, verified before and after the copy). Client: 4 threads, 256 in flight,
uncapped, 60 s. Results, raw files and scripts are in `target/parallel-workers-20261031/`
(`diagnostics.md`). Thread names came from `GetThreadDescription` (the parser is `dnstap-parser`; the
`ProcessTrace` thread is `etw-consumer`).

| Run | received | dequeued | ring_dropped | etw_lost | Parser CPU | etw-consumer CPU | Total CPU (load) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| D1 defaults | 23.98M | 13.52M | 10.46M | 0 | 19.3% | 13.6% | 93.0% |
| D2 `--capacity 16384` | 21.10M | 14.21M | 6.89M | 0 | 23.2% | 15.0% | 88.5% |
| D3 collector `StandardOutput=null` | 25.50M | 11.75M | 13.75M | 0.19M | 19.1% | 15.5% | 92.2% |

- **Parser.** It used about a fifth of one core while the queue was overflowing, so it is not CPU-bound.
  The thread-state counter showed it Ready (runnable, waiting for a CPU) in 53-65% of loaded samples,
  Running in 12-15% and Waiting in 22-35%. It was never in EventPairLow or UserRequest. The consumer was
  Ready in 70-77% of samples. The VM ran at about 100% total CPU, with dns at about 250% of 400%.
- **Receiver.** Recv-Q was zero in more than 93% of samples (maximum 0.46 MB), Send-Q was always 0, no
  collector thread exceeded 18% CPU, and there were no `buffer is full` warnings. journald sat at about
  90% of a core in D1 and D2 and was idle in D3. Removing the output stage (D3) did not reduce loss.
- **D2.** `ring_dropped` fell 34%, but `dequeued` rose only 5% and `received` fell 12%. The fall is mostly
  lower offered load, not faster draining. The run-to-run spread of `dequeued` is about 20%.
- **Receiver change.** The D3 drop-in was removed. `dnscollector` is active, `MainPID` 3128 changed to 29419
  (two restarts), `StandardOutput` is back to `journal+console`, and `systemctl cat` matches the preflight text.
- **Decision input (for the P1 gate).** No row matches cleanly. The second row (workers) is the nearest, with low
  confidence: Recv-Q near zero, collector not CPU-bound, parser not near 100%. But the parser's non-running
  time is mostly CPU wait on a saturated VM, not blocking in a send. Whether extra workers gain anything on a
  saturated 4-vCPU guest is not shown. The sender-side TCP state was not measured.
- **Restore.** Original binary hash `CE431467...B656`, original `PathName`, service Stopped, no `EtwDnstap*`
  sessions, no leftover receiver samplers or `/tmp/p0-*` files.

### Stage P1 decision (coordinator)

- **Matching row.** The evidence matches the new CPU contention row, not the
  worker row:
  - the VM ran at about 93% total CPU, with `dns` at about 250% of 400%;
  - `dnstap-parser` was Ready in 53–65% of samples, and `etw-consumer` in
    70–77%;
  - neither thread used more than about 23% of a core;
  - the receiver kept up: Recv-Q was almost always 0, and the collector
    reported no drops.
- **Receiver output is not the limit.** D3 removed the collector's output
  stage without reducing loss. Stage P2 is no longer a prerequisite for
  diagnosis. It is still needed before claiming end-to-end delivery in P6.
- **Next and deferred.** The next stage is P1a. P3 and P4 are deferred.
- **Restore.** The coordinator independently verified the restore: original
  hash, original `PathName`, service Stopped, no sessions, no collector
  drop-ins, `StandardOutput` `journal+console`, collector and filter active,
  and 18 GB free.

### Stage P0b

Run October 31, 2026 on `DNSTap-WS2025` with 6 logical processors, same candidate (SHA-256 `51D7DF2F...7A8F`,
verified on the VM), client settings, samplers and D3 drop-in as P0, plus D1b (repeat of D1). Results, raw files
and scripts are in `target/parallel-workers-20261031-6cpu/` (`diagnostics.md`).

| Run | vCPU | client sent | received | dequeued | ring_dropped | etw_lost | dequeue/s | Total CPU (load) | Parser CPU / Ready | Consumer CPU / Ready |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | --- | --- |
| D1 | 4 | 11.99M | 23.98M | 13.52M | 10.46M | 0 | 225k | 93.0% | 19.3% / 62% | 13.6% / 70% |
| D1 | 6 | 8.61M | 17.21M | 12.59M | 4.62M | 0 | 210k | 64.1% | 16.7% / 33% | 10.7% / 48% |
| D1b | 6 | 8.30M | 16.58M | 12.05M | 4.54M | 0 | 201k | 64.9% | 16.7% / 31% | 11.1% / 42% |
| D2 | 4 | 10.55M | 21.10M | 14.21M | 6.89M | 0 | 237k | 88.5% | 23.2% / 65% | 15.0% / 71% |
| D2 | 6 | 8.15M | 16.28M | 13.35M | 2.93M | 0 | 223k | 64.6% | 21.3% / 26% | 12.7% / 29% |
| D3 | 4 | 12.84M | 25.50M | 11.75M | 13.75M | 0.19M | 196k | 92.2% | 19.1% / 53% | 15.5% / 71% |
| D3 | 6 | 8.24M | 16.45M | 10.97M | 5.48M | 12k | 183k | 60.7% | 13.1% / 20% | 7.9% / 31% |

- **Saturation gone.** Load-sample total CPU fell from 88-93% to 61-65%. `dns` stayed at about 240-260% (it did
  not absorb the new CPUs). Ready share fell by about half. Samples above 90% total fell from 24-31 of 34-37 to 4-6 of 33-36.
- **Drain rate unchanged.** Dequeue rate was 183-223k/s, 6-7% below the 4-vCPU runs, with the parser using 13-21% of
  a core and Waiting in 47-57% of load samples. The loss count fell because offered load fell: the client rate dropped
  23-36% and timeouts rose about 4x (cause not measured).
- **Spread.** D1 versus D1b: `dequeued` -4.3%, `ring_dropped` -1.9%, drop share 26.9% versus 27.3%. D2 versus the D1/D1b
  mean: `ring_dropped` -36%, `dequeued` +8%. D3 `dequeued` -11% versus the D1/D1b mean.
- **Receiver.** Recv-Q was zero in at least 95% of samples (maximum 0.46 MB), Send-Q 0, no collector thread above 20%,
  no `buffer is full` warnings; journald near one core in D1, D1b and D2, idle in D3. The receiver is not the limit.
- **Decision input (for P1).** The CPU contention row no longer matches (about 90% confidence). The nearest row is
  the second one (workers), at about 55% confidence, with P3 as secondary. The counters do not say whether the parser
  waits in the send path or in a park/wake handoff, and the sender-side TCP state was not measured.
- **Restore.** Original binary `CE431467...B656`, original `PathName`, service Stopped, no `EtwDnstap*` sessions,
  D3 drop-in removed and `systemctl cat` identical to the preflight, `StandardOutput` `journal+console`, collector and filter
  active, no leftover samplers or `/tmp/p0*` files, 18 GB free.

### Stage P1 decision after P0b (coordinator)

- **CPU contention is no longer the explanation.** With 6 vCPUs, total CPU
  was 61–65% and Ready time roughly halved. Loss persisted (`ring_dropped`
  about 27% of received in D1 and D1b), and the dequeue rate did not rise
  (about 200–210k events per second).
- **The receiver is ruled out again.** D3 did not help. Recv-Q was almost
  always 0, and the collector logged no drops.
- **The parser waits for an unknown reason.** It is waiting about half the
  time and is not running when events are dropped, so something other than
  CPU or the receiver gates it.
- **Next.** The next stage is P0c, to tell the idle park apart from a
  blocking send before choosing between P3 and P4. P1a (priority) is
  deferred.
- **Also observed.** The client's achieved rate fell about 28% and timeouts
  rose about four times on 6 vCPUs, which is unexplained. Cross-vCPU-count
  comparisons should use the dequeue rate, not drop totals.
- **Restore.** The coordinator independently verified the restore.

### Stage P0c

Run October 31, 2026 on `DNSTap-WS2025` (6 vCPUs) with the diagnostic build (SHA-256 `F3B92E8C...5238`, verified on
the host and on the VM). Same client, samplers and procedure as P0b, no receiver drop-in. Results, raw files and
scripts are in `target/parallel-workers-20261031-6cpu/diag/` (`timing-results.md`).

| Run | Flags / client | received | dequeued | ring_dropped | Parser lifetime | Parked | Write + flush | Remainder / measured CPU |
| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| R0 | defaults / 50k qps | 5.82M | 5.58M | 0.25M (4.3%) | 105.3 s | 93.5% | 0.9% | 5.9 s / 5.8 s |
| T1 | defaults / uncapped | 17.91M | 12.00M | 5.91M (33.0%) | 105.3 s | 74.6% | 3.4% | 23.2 s / 12.6 s |
| T2 | `--capacity 16384` / uncapped | 18.10M | 14.66M | 3.44M (19.0%) | 106.4 s | 70.7% | 3.6% | 27.2 s / 14.7 s |

The lifetime includes about 45 s of idle, when the parser is parked 99.99% of the time (measured separately).
Removing the idle estimate, the 60 s load window is:

| Run | Parked | Write + flush | Remainder | Parks/s | Mean park | Events per wake | Wakes >= 4k queued | Full wakes | `max_run` | Writes > 1 ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| R0 | 88.5% | 1.6% | 9.9% | 96 | 9.2 ms | 965 | 6.8% | 0.8% | 26.9k | 41 |
| T1 | 55.4% | 6.0% | 38.6% | 48 | 11.6 ms | 4,188 | 18.0% | 7.6% | 71.0k | 476 |
| T2 | 48.0% | 6.7% | 45.3% | 43 | 11.1 ms | 5,669 | 16.3% | 5.9% (>= 8k of 16k) | 246.6k | 527 |

- **Park dominates, writes do not.** Parking is 48-55% of the load window against 6-7% for writes. About 8-9 writes
  per second exceed 1 ms.
- **Headroom.** The idle poll asked for 20 ms and slept 26.7 ms (Windows timer rounding). At about 300k events/s an
  8,192-slot ring fills in 27 ms. Mean events per wake was 4.2k (T1), half the ring.
- **Not the whole story.** The 219 (T1) and 153 (T2) full wakes cannot account for 5.9M and 3.4M dropped events. In
  T1 and T2 the remainder is about 1.85 times the measured parser CPU (10.6 s and 12.5 s unexplained, 17-21% of the load
  window; the parser was Ready in 11% and 38% of samples). R0 reconciles (5.9 s against 5.8 s).
- **Decision.** The P0c table gives **Stage P3 first**, then remeasure (confidence about 65% that P3 is the right next
  step, about 35% that it removes most of the loss alone). Stage P4 is not supported: the parser used 21-27% of a core
  and drained at over 450k events/s while running. If P3 leaves the unexplained remainder, return to P1a (priority).
- **Anomaly.** R0 at 50k qps is not loss-free (4.3% dropped).
- **Restore.** Original binary `CE431467...B656` restored from the backup and hash-verified, original `PathName`, service
  Stopped, no `EtwDnstap*` sessions, no temporary VM files (`etw-dnstap-perf.exe` untouched), receiver `/tmp` script
  removed, no leftover samplers, `dnscollector` and `dnstap-lab-filter` active, 18 GB free.

### Stage P0c decision (coordinator)

- **The wait is parking and scheduling, not the network.**
  - Parking took about half the load window, with a mean of 9–12 ms against
    a 2 ms request.
  - Writes took 6–7%.
  - About a fifth of the window was unaccounted for, most likely Ready time.
- **P4 (workers) stays deferred.**
- **The original P3 (spinning briefly before parking) is replaced by Stage
  P3r.** P3r tests timer resolution, thread priority, and ring capacity
  separately and together.
- **R0 lost 4.3% at 50,000 queries per second on 6 vCPUs.** Stage L3 lost
  nothing at 47,000 on 4 vCPUs with the baseline build. The difference in VM
  size or build is not yet explained. P3r includes R0 so the result can be
  compared directly.
- **Restore.** The coordinator independently verified the restore: original
  hash, original `PathName`, service Stopped, no sessions, collector and
  filter active, and no receiver temporary files.

### Stage P3r

Run October 31, 2026 on `DNSTap-WS2025` (6 vCPUs) with the diagnostic build (SHA-256 `F10F3B5A...9E14`, verified on the
host and on the VM). Client as in P0c (`--threads 4 --inflight 256`, 60 s, with `--qps 50000` or uncapped). Eleven runs; results,
raw files and scripts are in `target/parallel-workers-20261031-6cpu/p3r/` (`p3r-results.md`).

| Setting | Drop % at 50k | Drop % uncapped | Client rate uncapped | Parser Ready uncapped | Private bytes |
| --- | ---: | ---: | ---: | ---: | ---: |
| X0 default (X0b repeat) | 0.11 | 35.3 (35.25) | 121k (135k) | 26% (25%) | 31.6 MB |
| X1 timer 1 ms | 13.0 | 32.6 | 127k | 27% | 31.6 MB |
| X2 above-normal | 0.05 | 15.7 | 135k | 11% | 31.6 MB |
| X3 capacity 65,536 | 0.00 | 4.85 | 144k | 40% | 178.6 MB |
| X4 all three | 0.00 | 0.00 | 149k | 3% | 178.6 MB |

- **Noise.** X0 and X0b dropped 35.30% and 35.25% although the client rate moved 121k to 135k. Offered load drifted upward
  through the sequence, so the X3 and X4 comparisons are conservative. At 50k there is no repeat (R0 lost 4.3%, X0-50k 0.11%).
- **Timer.** Mean park in the load window fell from 12.9-14.0 ms to 9.7 ms but the loss did not. X1-50k lost 13.0% (775k
  events) with only 36 full wakes. It is unrepeated and unexplained.
- **Priority.** Halved the loss at max by cutting the parser's Ready share from 25-26% to 11%. This is the P0c "unaccounted" time.
- **Capacity.** Removed 86% of the loss at max and all of it at 50k. The ring never filled (maximum 46.5k queued of 65,536). Cost
  was 147 MB of private bytes.
- **Combination.** X4 removed all the loss at both loads; the parser's Ready share was 3%.
- **DNS impact.** None measurable. X2 and X4 matched or exceeded X0's client rate, timeouts were 0.11-0.14% of sent against
  0.13-0.16%, and `dns` CPU per 1k queries was 1.74-1.90 against 1.72-1.94.
- **Decision.** Productionize capacity plus above-normal priority on the parser and consumer threads. Do not productionize the
  timer resolution (no gain alone; X1-50k worse). Confidence about 70% that priority plus capacity matches X4. The 32,768
  capacity is untested. P4 stays deferred (parser CPU 24% of a core at 273-299k events/s). Next: X5 (priority + 65,536),
  X6 (priority + 32,768) with repeats, and a repeat of X1-50k.
- **Deviations.** Two 45 s idle starts to measure the idle park rate per timer setting; the receiver's sampler script was
  recreated and removed.
- **Restore.** Original binary `CE431467...B656` restored from the backup and hash-verified, original `PathName`, service
  Stopped, no `EtwDnstap*` sessions, no temporary VM files of mine, receiver `/tmp` script removed, no leftover samplers,
  `dnscollector` and `dnstap-lab-filter` active, 18 GB free.

### Stage P3r decision (coordinator)

- **The two levers that work are priority and capacity.** Loss at the
  uncapped rate (about 121–149k queries per second):

  | Setting | Loss |
  | --- | ---: |
  | Baseline (X0 / X0b) | 35.3% / 35.25% |
  | Above-normal priority (X2) | 15.7% |
  | Capacity 65,536 (X3) | 4.9% |
  | All three levers (X4) | 0 |

  X4 also lost nothing at 50,000 queries per second.
- **The timer resolution alone (X1) did not help.** Its 13% loss at 50,000
  is unexplained and needs a repeat.
- **There was no measurable DNS cost.** Client rate, timeouts, and `dns` CPU
  per 1,000 queries stayed within the X0 to X0b noise band.
- **Capacity 65,536 adds about 147 MB** of private memory. The peak queue
  depth was 46,533.
- **Workers (P4) stay unsupported.** Under full load the parser used only
  about 24% of a core.
- **Next.** Stage P3s confirms priority together with capacity, without the
  timer, and then productionizes it. The user chooses the defaults.
- **Restore.** The coordinator independently verified the restore.

### Stage P3s step 1

Run October 31, 2026 on `DNSTap-WS2025` (6 vCPUs) with the P3r diagnostic build (`F10F3B5A...9E14`), default timer and
normal priority. Results, raw files and scripts are in `target/parallel-workers-20261031-6cpu/p3s/` (`p3s-results.md`).

| Run | Capacity | Drop % | Client rate | Peak queue depth | Private bytes | Parser Ready |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| X0-max | 8,192 | 29.32 | 138.8k | 8,192 | 31.6 MB | 27% |
| X6n-max-a | 32,768 | 8.70 | 151.7k | 32,768 | 94.7 MB | 37% |
| X3r-max | 65,536 | 4.36 | 138.6k | 42,540 | 178.6 MB | 34% |
| X6n-max-b | 32,768 | 8.90 | 153.8k | 32,768 | 94.7 MB | 24% |
| X6n-50k | 32,768 | 1.07 | 49.0k | 32,768 | 94.7 MB | 0% |

- **32,768 at max.** Removed 70–75% of the loss against 8,192. The repeats agree to 0.2 points, and both had the highest
  offered load of the session.
- **65,536 at max.** Reproduced P3r X3 (4.36% against 4.85%).
- **32,768 at 50k.** Lost 1.07% (62,703 events), and the ring filled. P3r X0-50k lost 0.11% and X3-50k lost nothing.
  This run is unrepeated.
- **Residual loss.** At normal priority the parser's Ready share stayed at 24–37% at max for every capacity.
- **DNS impact.** None visible: timeouts were 0.07–0.12% of sent at max, and `dns` CPU per 1,000 queries was 1.77–1.99.
- **Restore.** The agent restored the lab, and the coordinator independently verified it: original binary hash and
  `PathName`, service Stopped, no `EtwDnstap*` sessions, no receiver `/tmp/p0-*` files or samplers, collector and filter
  active, 18 GB free.
