# Pooled ETW intake: validation plan

> **Status:** Historical · **Evidence dated:** October 2026 (undated in text) · **Commit:** not recorded · **Reviewed:** 2026-11-01 · The current design is in [pooled-intake.md](../design/pooled-intake.md).

This historical plan is for maintainers reviewing the original bounded callback
storage and channel-validation design. It defines the local synthetic checks
proposed before the separately authorized VM campaign. Its installed baseline
is the capacity-only service `CA3342…64347`; later evidence is linked in
[Execution status](#execution-status).

This record describes historical binaries with service JSONL metrics
instrumentation. The TCP-only service has no durable metrics export; its
instrumentation and performance have not been measured by this campaign.
See the [instrumentation compatibility note](../README.md#instrumentation-compatibility-note). Preserved counters,
tables, hashes, and collector artifacts retain their historical meaning.

## Execution status

This is the original validation design and ownership record. The original
`perf_windows` matrix and `perf_review` review remain stopped at the user's
request; their table and commands are retained as a plan, not evidence of
execution. Local implementation checks and a fresh independent read-only audit
are tracked separately in the [audit and remaining validation
plan](pooled-intake-audit-plan.md). That follow-up also records the metrics fix
and documentation reconciliation. At this plan's local-validation cutoff, no
candidate VM deployment or native ETW performance validation had occurred.

The later [pooled VM campaign](../performance/pooled-vm-performance-review.md) completed native
validation of the reviewed `98FD…1F5D7C` candidate, including both high-rate
five-minute confirmations. That separate campaign does not execute or resume
the stopped synthetic matrix. Treat the ownership table and commands here as
the original design record, not pending instructions for the completed campaign.

## Original delegation and file ownership

The original work assignments were:

| Chunk | Model / effort | Owner and context | Output and gate |
| --- | --- | --- | --- |
| Scope and decisions | GPT-6-Astra / high | Coordinator; user requirements and previous capture/WPA results | Pool budgets, matrix, explicit deployment decision |
| Production implementation | GPT-6.1-Sol / high | `pooled_intake`; callback ownership, ETW bounds and shutdown | Owns all `src`, Cargo files and `docs/pooled-intake.md`; bounded pool/channel, test-only real-path hooks, tests and hardened build |
| Independent security review | GPT-6-Astra / high, read-only | `perf_review`; unsafe copies, TLS lifetime, cross-thread Drop, teardown and resource budget | Review gate before any candidate deployment; no source edits |
| Validation orchestration | GPT-6.1-Sol / high | `perf_windows`; prior measurements, independent evidence checks | Owns this plan and `scripts/performance/pooled-intake-*`; exact commands, source provenance, synthetic benchmark results and limitations |

The implementation owner authors the private in-crate benchmark hook so it can
call the real callback without adding public production APIs. The validation
owner controls the matrix, runner, allocator/ledger acceptance and report.
There are no shared source edits and no new SCM or VM actions in this phase.

## Required production contract

The reviewed design uses a preallocated framework `PooledBuffer`, callback TLS,
nonblocking `crossbeam_channel::Sender::try_send`, bounded deep copies through
`copy_nonoverlapping`, and cross-thread buffer return through `Drop`. The regular
class has 2 KiB buffers with `capacity + 2` entries. A bounded jumbo class handles
the maximum valid ETW user/extended-data layout; its intended default is 64
entries, with the CLI bound and total memory budget subject to security review.
The production channel default remains 8,192 entries.
The parser uses only `try_recv` and the existing two-millisecond timed park;
it does not register a blocking channel waiter or use `recv_timeout`/`select`.
The same polling policy applies to the synthetic consumer. The jumbo CLI
range is 0–256 entries, with a default of 64.

The callback must not allocate, wait for queue space, log, parse with TDH, or
grow either pool. Queue-full, pool-empty and disconnected receiver outcomes
must return ownership and increment distinct atomic counters. Pointer fixups,
alignment, metadata preservation, failure cleanup and shutdown are reviewed
against actual source, not inferred from successful synthetic throughput.

Metrics schema 2 preserves `ring_dropped` as channel-full only and adds the
aggregate and component counters. The stable ledger must satisfy:

```text
intake_dropped = ring_dropped + pool_exhausted + receiver_disconnected
received = intake_dropped + callback_errors + dequeued + queue_backlog
```

Existing schema 1 results retain their original meaning. Zero-loss acceptance
for this candidate covers **all intake drops**, not only channel-full drops.

## Interfaces and dependencies

The agreed test hook is `synthetic_pooled_intake_benchmark`, an ignored in-crate
test under the implementation owner's private ETW test module. Its proposed
environment interface is `ETW_BENCH_CAPACITY`, `ETW_BENCH_EVENTS` and
`ETW_BENCH_MODE=steady|bursty`; the runner verifies the landed interface before
execution. One producer installs the real callback TLS and calls the actual
callback path; a separate consumer receives and drops the real event owner.
Thread setup, pool allocation and warmup occur outside measured callback scopes.

The hook emits one machine-readable `ETW_BENCH_JSON=` line. Required fields
include capacity, pool class sizes/budgets, offered/accepted/dequeued counts,
the complete drop ledger, final backlog, elapsed time, recycle verification
and allocation/deallocation/reallocation counts for each warmed failure fixture.
The runner rejects missing evidence or inconsistent counts instead of inventing
zero values. Process CPU and sampled private/working memory measure the whole
test process; they are separate from callback-only allocation evidence.
Both callback and cross-thread recycle allocation/deallocation evidence must be
present. Counts cover the warmed Rust global allocator, not all Windows/native
heap activity: native TLS initialization can call the system allocator directly.
The actual native capture worker must initialize its TLS before `ProcessTrace`.

Execution depends on a completed hook, reviewed source, locked dependencies and
an isolated build output. Release builds must not overwrite the preserved
installed/prior binaries. No dependency or public API is added merely for the
runner. Actual package versions, source hashes and toolchain commands accompany
the results.

## Local validation matrix

1. Review the actual callback/pool/channel hook and allocator measurement scope.
   Confirm it cannot bypass copying, TLS checkout or cross-thread recycling.
2. Run warmed allocation fixtures for success, channel-full, pool-exhausted and
   receiver-disconnected outcomes. Require zero allocations, reallocations and
   deallocations inside each callback scope, correct counters, and ownership
   return after consumer drain. Normal thread/pool startup allocations are not
   mislabeled callback allocations.
3. Run steady and bursty producer/consumer scenarios at capacities 1,024 and
   8,192. Use the same event count and synthetic payload per comparison. Include
   payload/class settings and consumer behavior in every result. Overload drops
   in the burst test are expected observations, not silently retried away.
4. Record elapsed time, offered/accepted/dequeued throughput, aggregate and
   component drops, final backlog, process CPU seconds and sampled memory.
   Require complete final ledgers and cross-thread recycling verification.
5. Run the implementation owner's meaningful safety/ownership tests and build
   checks by reference to their raw receipts; independently validate the hook
   output and runner failure behavior. Avoid duplicating or replacing source
   tests with a mock pool implementation.

Run the independent evidence-reader checks with:

```powershell
python scripts/performance/pooled-intake-run-test.py
```

> **Script path changed (commit `6e62b2e`):** the checks are now `scripts/performance/test_pooled_intake_run.py`. The command above is the
> recorded historical text. See [script moves and renames](../../scripts/performance/README.md#file-moves-and-renames).

After the implementation owner supplies the release test executable, run:

```powershell
python scripts/performance/pooled-intake-run.py `
  --test-exe TEST_EXE `
  --output-dir target/performance/pooled-intake/LOCAL_RUN_ID `
  --events 200000 --repetitions 3 --timeout 120
```

> **Script path changed (commit `6e62b2e`):** the runner is now `scripts/performance/pooled_intake_run.py`. The command above is the
> recorded historical text. See [script moves and renames](../../scripts/performance/README.md#file-moves-and-renames).

Replace `TEST_EXE` with the exact hardened release test executable from the
implementation receipt and `LOCAL_RUN_ID` with a fresh evidence directory.
The runner executes four scenarios, each three times, and passes the ignored
test filter, `--nocapture` and `--test-threads=1`. It checks the unique JSON
output, requested event count, all drop/ownership ledgers and allocator fixtures.
These commands describe the runner interface; the actual Rust hook schema is
checked before execution and recorded with its source hash.

Raw stdout/stderr, exact arguments/environment, source hashes, binary
hash, timestamps and a consolidated JSON report use a fresh evidence directory
under `target/performance/pooled-intake`. Existing outputs are never overwritten.

## Interpretation and release gates

Synthetic measurements exclude live ETW delivery, TDH parsing, protobuf encoding,
Frame Streams and the receiver. They cannot establish maximum DNS throughput,
zero loss in a VM, or an improvement over the preserved allocation-based intake.
A direct baseline is included only if it exercises equivalent real source and
work; otherwise the report explicitly presents candidate-only measurements.
No mock copy loop is labeled the original callback benchmark.

Source/security acceptance requires bounded allocation before capture, safe
pointer/lifetime contracts, race-safe ownership transfer/recycle, no callback
allocation or blocking, distinct drop metrics, and orderly teardown without
use-after-free or double return. Synthetic acceptance requires warmed fixtures
with zero measured allocator calls and exact final ledgers; it does not require
zero overload drops from a deliberately stalled consumer.

VM deployment and repeat authoritative/recursive tests require a subsequent
coordinator decision. Until then, the normal installed service, strict identity,
collector configuration and previous restoration proof remain unchanged.
