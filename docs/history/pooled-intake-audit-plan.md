# Pooled intake audit and remaining validation plan

> **Status:** Historical · **Evidence dated:** October 2026 (undated in text) · **Commit:** not recorded · **Reviewed:** 2026-11-01

This audit record helps maintainers assess the pooled Event Tracing for Windows (ETW)
candidate after source review. It separates confirmed defects, local validation,
and remaining evidence needed for deployment or performance claims. It does not
authorize a VM run, restart the stopped benchmark matrix, or change the installed
service.

This record describes historical binaries with service JSONL metrics
instrumentation. The TCP-only service has no durable metrics export; its
instrumentation and performance have not been measured by this campaign.
See the [instrumentation compatibility note](../README.md#instrumentation-compatibility-note). Preserved counters,
tables, hashes, and collector artifacts retain their historical meaning.

## Status and evidence cutoff

The local source, analysis, and documentation follow-up is complete, with no
remaining blocker from its independent review. The validation requirements and
ownership table preserve that work's scope. The separate
[native VM campaign](../performance/pooled-vm-performance-review.md) later supplied deployment
and performance evidence for the same reviewed candidate. Its results do not
alter the pending-at-build status in immutable local receipts or resume the
stopped original benchmark matrix.

## Findings and scope

The fresh Astra read-only audit found no core pool memory-safety or lifetime
defect. Its initial result was a conditional pass with the measurement defect,
documentation mismatch, avoidable parser allocation, and reconstruction test
coverage addressed by the follow-up recorded here. A conditional source pass does not
establish native ETW delivery or production performance.

The audit distinguishes these findings:

| Finding | Classification | Required response |
| --- | --- | --- |
| Schema 2 pool exhaustion or receiver disconnection can be omitted from a receiver zero-loss claim; the Windows analyzer rejects valid lossy schema 2 closure | Confirmed P1 measurement bug | Normalize both schemas and gate zero loss on all intake loss and exact final ledgers |
| README and ring-capacity note describe allocation-based `ArrayQueue` transport and near-1 GiB memory as the active implementation | Confirmed P2 documentation bug | Describe the channel and bounded pools; label the old capacity results and memory calculation historical |
| Five parser property reads construct UTF-16 name vectors for a closed set of six names | Confirmed avoidable parser cost | Use static terminated names without changing TDH property semantics |
| Tests copy and inspect owned storage without fully exercising the parser's reconstructed native record | Test coverage gap, not a demonstrated memory defect | Share one scoped reconstruction helper between parsing and direct pointer/metadata tests |
| Pooling increases throughput, permits a smaller safe queue, or removes TDH/export bottlenecks | Unmeasured hypotheses | Make no performance claim without equivalent native workload evidence |

The implementation keeps the user-mode native callback, TLS before
`ProcessTrace`, deep copying, bounded regular/jumbo classes, channel polling,
service identity, and native/parser shutdown order. The callback performs no
allocation, growth, logging, or wait for capacity. Atomic retry, spin, yield, and
preemption mean it has no formal wait-free bound. Parser TDH extraction,
protobuf encoding, and output still allocate. A large TDH, custom-queue, or
exporter rewrite is not justified by this audit.

## Ownership, dependencies, and gates

All code, scripts, and documentation in this follow-up are authored by
GPT-6.1 Sol. GPT-6 Astra coordinates and reviews read-only. The original
`perf_windows` and `perf_review` agents remain stopped; this work does not resume
their matrix or review. No commits, SCM changes, or VM actions are part of these
chunks.

| Chunk | Model / effort | Owner | Dependencies | Validation and status |
| --- | --- | --- | --- | --- |
| Measurement correctness | GPT-6.1 Sol / high | `intake_metrics_fix` | Audit reproduction and existing schema 1 evidence | Complete: shared normalizer, four consumers, 14 focused tests and five existing runner tests; Astra independently passes all 14 focused tests |
| Parser name storage and record reconstruction | GPT-6.1 Sol / high | `pooled_intake`, sole Rust/Cargo owner | Closed property-name set and actual parser path | Complete: 45 passing tests and one ignored manual benchmark in development, release-unwind, and ASan; independent review approved with no blocker |
| Architecture documentation | GPT-6.1 Sol / high | `intake_docs` | Actual source, measurement handoff, final Rust receipt | Complete: source, receipt, budget arithmetic, and local links checked; independent review approved with no blocker |
| Independent final review | GPT-6 Astra / xhigh, read-only | Fresh `intake_audit` review under root coordination | Completed fix diffs, tests, documentation, and exact candidate identity | Complete: measurement fix and final Rust/documentation follow-up approved with no blocker; native/performance evidence limits retained |
| Integration and scope | GPT-6 Astra, read-only coordination | Root coordinator | All chunk handoffs | Preserve stopped work and baseline evidence; report remaining native/performance limits |

The final read-only review approves the completed changes with no blocker. It
independently verifies source and candidate hashes, the three 45/0/1 raw test
results, release flags and PE checks, preservation of the initial candidate,
and links in all six documents. It accepts the static-name test correction
without treating it as a production fix. This approval retains the native ETW,
VM, and performance evidence limits; it does not infer a native-performance
result from local ownership tests.

## Measurement fix and checks

`scripts/performance/metrics_ledger.py` validates explicit supported schemas 1 and
2, Boolean final/statistics-valid fields, and nonnegative integer counters,
rejecting Boolean counters. Schema 1 retains ring-only intake loss. Schema 2
requires each admission category and its aggregate. Final samples must satisfy:

```text
intake_dropped = ring_dropped + pool_exhausted + receiver_disconnected
received = intake_dropped + callback_errors + dequeued + queue_backlog
dequeued = encoded + ignored + rejected + output_limit_dropped + output_failed_events
```

Phase analysis permits nonquiescent intermediate samples because separate
relaxed atomic reads need not reconcile. An explicitly final sample always
requires closure. Phase deltas reject counter resets and process/run/schema
identity changes. `etw_stats_errors` is required and reported separately from
capture loss: transient query failures do not imply packet loss when the final
query is valid and cumulative ETW loss is zero. Invalid final statistics cannot
pass the zero-loss gate.

The focused tests exercise the actual analyzer command paths, pool-only and
disconnection-only loss, reconciled lossy ledgers, malformed or absent fields,
unknown schemas, final closure, live phase samples, resets, and final statistics
validity. Run the two suites with:

```powershell
python -B scripts/performance/metrics-ledger-test.py -v
python -B scripts/performance/pooled-intake-run-test.py -v
```

> **Script path changed (commit `6e62b2e`):** the suites are now `scripts/performance/test_metrics_ledger.py` and `scripts/performance/test_pooled_intake_run.py`. The command above is the
> recorded historical text. See [script moves and renames](../../scripts/performance/README.md#file-moves-and-renames).

Sol reports 14 focused and five existing tests passed. Astra independently
verified all 14 focused tests, including the statistics-query follow-up. Separate
reanalysis preserves the critical counts and residuals in five archived Windows
and three receiver runs. It changes no archived data or reports. Receiver
comparison uses fresh metric-only copies because the existing loader expects
an object but the archived deployment `Start.json` contains a list. This
unrelated input incompatibility remains outside the fix. See the
[measurement review and comparison record](pooled-intake-review-plan.md).

## Rust follow-up acceptance

The source follow-up uses six static terminated UTF-16 names for five property
reads, selecting `Source` or `Destination` by event ID. It removes five
per-event name-vector constructions. A schema-cache hit retains five TDH size
queries and five TDH property extraction calls; result buffers still allocate.
Checks must verify every spelling, terminator, descriptor field, and static
storage lifetime without adding dynamic property-name input. Static lifetime
does not imply identical addresses across inlined calls: the optimizer can
duplicate literal storage. The release-unwind check exposed that test-only
address-identity assumption; the corrected assertion checks terminated contents
and descriptor fields without changing production behavior.

`RawEtwEvent::with_record` is the scoped native-view helper used by the parser
and reconstruction tests. The tests must verify the actual `EVENT_RECORD`
header and buffer context, null `UserContext`, lengths, user data, descriptor
metadata, repaired payload pointers, eight-byte alignment, and bounds. Include
zero extended items and the maximum 64, destroy native source storage before
the view, recycle a dirty slot into a shorter record, and retain queued ownership
after native-owner exit. The event lease and typed descriptor array must remain
alive for the full synchronous view.

Retain callback allocator fixtures, cross-thread recycling, pool-exhaustion and
channel-failure counters, final drain, concurrent unique-address checks, and
memory-budget assertions. Record formatting, strict Clippy, development tests,
release-unwind tests, ASan tests, release-abort build, and PE hardening checks in
a fresh versioned receipt. Do not overwrite the initial candidate or label its
42-test result as validation of changed source.

## Artifact identity and evidence limits

The installed capacity-only baseline remains:

```text
CA3342D848BD06B7B8E1E6CE9ADC05E2157C3E86A18980E69E74B16555264347
```

Its 1,000 QPS, 300-second authoritative and recursive capture results apply to
that allocation-based binary. They establish neither pooled-candidate capture
nor a jumbo admission budget. The recursive run also retained its separate
RA-check failure. See the preserved
[capacity validation report](ring-buffer-fix-validation.md).

The initial pooled local candidate remains preserved with its
[receipt](../../datasets/pooled-intake-local/receipt.json):

```text
3280D954E17E1448C51FBFF5B253807E473CF9BE59FAC18BC94DBBD51D95B841
```

That receipt records 42 passing tests and one ignored manual benchmark in each
of development, release-unwind, and ASan configurations, plus formatting,
strict Clippy, release build, and PE checks. It predates the parser-name and
reconstruction follow-up.

The follow-up local candidate is preserved with its versioned
[receipt](../../datasets/pooled-intake-local/audit-fixes-v1/receipt.json):

```text
98FDAB43AA24AE1BE31A7AABCCBDD67A2F64C87DC1C50C428B638B3C091F5D7C
```

The follow-up receipt records 45 passing tests, zero failures, and one ignored
manual benchmark in each of development, release-unwind, and ASan. Formatting,
strict all-target Clippy, the ordinary release-abort build, and actual PE checks
pass. The PE includes DEP, ASLR, high-entropy VA, CFG with 802 function targets,
a security cookie, the CET compatibility marker, and EH continuation metadata;
it has no PDB CodeView record or ASan runtime dependency. Image markers do not
prove runtime enforcement. The three added static-name/native-view allocator
scopes each record zero allocations, reallocations, and deallocations, excluding
TDH calls, setup, and shutdown. The initial release test failure log is preserved
alongside the corrected passing results. Fresh final independent review approves
the source, local evidence, and documentation with no blocker. The immutable
receipt retains its accurate pending-at-build review status; this plan records
the subsequent signoff.

The PE inspection text files are [pooled-intake-release-pe.txt](../../datasets/pooled-intake-local/pooled-intake-release-pe.txt) and [audit-fixes-v1/release-pe.txt](../../datasets/pooled-intake-local/audit-fixes-v1/release-pe.txt). The `etw-dnstap.exe` binaries beside them are local only; git ignores `*.exe`.

The optional synthetic harness and unexecuted matrix exclude native delivery,
TDH parsing, protobuf encoding, and output. Local allocator counters instrument
the Rust allocator during scoped callback/recycle work, not arbitrary Windows
DLL allocations. At this local-audit cutoff, no pooled candidate has native VM
validation and no measured production improvement is claimed. Later performance/deployment work needs
its own scope, exact binary identity, all-category loss accounting, and matched
authoritative/recursive evidence.

The later, separately authorized [native VM campaign](../performance/pooled-vm-performance-plan.md)
and [performance review](../performance/pooled-vm-performance-review.md) record actual pooled
candidate validation, including two clean five-minute high-rate confirmations.
That campaign has its own backup, build/deployment identity, exact receiver and
all-category loss gates, full native traces, qualifications, and restoration
proofs. The local audit and original stopped benchmark matrix remain separate;
their immutable receipts retain their accurate status at creation.

## Documentation verification

The documentation pass checks source-backed transport, pool sizes and CLI
bounds, callback allocation scope, native-record ownership, parser polling,
schema 2 closure, and the distinction between historical and candidate
evidence. Links to the current reference and preserved reports must resolve.
The detailed budget remains 37.32 MiB default and 90.89 MiB maximum counted
storage, excluding allocator/native/parser/output overhead. Pooling alone does
not justify reducing the queue.

Source checks cover `src/etw.rs`, `src/etw/pool.rs`, `src/pipeline.rs`, and
`scripts/performance/metrics_ledger.py`. All local Markdown links in the six
changed documents resolve. The final candidate identity and counts match its
versioned receipt. The fresh independent reviewer separately confirms source
alignment, all six documents' local links, candidate identity, and validation
counts, and approves the documentation with no blocker.
