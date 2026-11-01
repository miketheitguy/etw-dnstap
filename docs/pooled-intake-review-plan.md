# Pooled intake measurement review

> **Status:** Historical · **Evidence dated:** October 2026 (undated in text) · **Commit:** not recorded · **Reviewed:** 2026-11-01

This review record helps maintainers assess the Python analysis of pooled Event Tracing
for Windows (ETW) intake metrics. You can use its checks to distinguish complete
loss accounting from a zero-loss result. It covers analysis helpers and their
tests; it does not run a benchmark matrix or change Rust, installed services,
virtual machines, or preserved campaign artifacts.

This record describes historical binaries with service JSONL metrics
instrumentation. The TCP-only service has no durable metrics export; its
instrumentation and performance have not been measured by this campaign.
See the [instrumentation compatibility note](README.md#instrumentation-compatibility-note). Preserved counters,
tables, hashes, and collector artifacts retain their historical meaning.

## Status and evidence cutoff

The measurement correction and independent local review are complete. The
finding in the next section describes the defect before that correction; the
[verification record](#verification-record) describes the implemented result.
The later [native VM review](pooled-vm-performance-review.md) applies the corrected
all-category accounting to actual DNS traffic.

## Confirmed finding and evidence

Before the correction, the receiver summary could claim
`capture_pipeline_zero_loss_verified=true` for a
schema 2 run with 100 received events, 10 pool-exhausted events, and 90 dequeued,
encoded, and durable events. Its predicate checked ring loss but omitted pool
exhaustion and receiver disconnection. It reported the capture residual without
gating the result. Conversely, the Windows analyzer rejected that reconciled
ledger because it subtracted only ring loss. Receiver disconnection had the same
accounting gap. Severity: P1, measurement correctness.

## Ownership and implementation order

GPT-6.1 Sol at high reasoning effort owns this plan, the shared Python ledger
module, the four analysis consumers, and focused tests. GPT-6 Astra at xhigh
reasoning effort performs the independent read-only review. The root agent
coordinates scope and integrates findings. The stopped performance and security
agents remain stopped. No commit or virtual-machine action is part of this fix.

Apply these changes in order:

1. Add one shared normalizer for supported metrics schemas 1 and 2.
2. Integrate it into `analyze-windows.py` and the receiver summary's actual
   zero-loss predicate.
3. Integrate schema-aware intake counts into phase and CPU analysis while
   preserving the meaning of ring-only percentages.
4. Exercise the consumers with independent fixtures and reanalyze preserved
   schema 1 evidence into a separate temporary output where available.
5. Ask the independent reviewer to check the change and record the verification.

## Helper contract

Require explicit supported `schema` and Boolean `final` fields. Require all
ledger counters as nonnegative integers; reject Boolean counter values. Schema
1 normalizes total intake loss to `ring_dropped` and its two unavailable loss
categories to zero. Schema 2 requires all three categories and the explicit
`intake_dropped` total. Missing required fields and unknown schemas are errors.
Both schemas require a nonnegative integer `etw_stats_errors` counter. The
receiver summary reports this count and final `etw_stats_valid` separately.
Transient statistics-query errors don't establish capture loss: a valid final
statistics query with zero cumulative ETW loss can satisfy the zero-loss gate
even when `etw_stats_errors` is positive. Invalid final statistics cannot pass.

Final analysis requires `final=true` and exact identities:

```text
schema 2 intake_dropped = ring_dropped + pool_exhausted + receiver_disconnected
received = intake_dropped + callback_errors + dequeued + queue_backlog
dequeued = encoded + ignored + rejected + output_limit_dropped + output_failed_events
```

Intermediate phase samples retain strict schema and field validation but do not
require these quiescent identities: samples read independent relaxed atomics.
An explicitly final sample always requires closure, including in phase analysis.
Phase deltas reject counter resets and changes of schema or process/run identity.
The zero-loss predicate additionally requires zero total intake loss and all
existing relevant callback, parser, output, backlog, ETW, and archive checks.

## Verification gates

Use built-in Python `unittest` with no additional dependency. Cover schema 2
pool-only and disconnection-only loss, a valid lossy ledger accepted by the
Windows analyzer, schema 1 zero/loss cases, malformed counters, missing fields,
unknown schema, nonfinal input, capture/parser residuals, and phase reset and
nonfinal sampling behavior. Exercise the actual receiver summary and Windows
analyzer command paths, not just a duplicate of their predicates.

Preserve all previous raw data, reports, and results. Write reanalysis output
only under a fresh temporary directory and compare critical schema 1 ledger
values with the archived analysis. These checks validate measurement logic;
they do not establish native capture performance or service security.

## Verification record

The shared normalizer and all four consumers implement the contract. The
receiver summary gates its zero-loss flag on a validated final ledger and every
intake loss category. The Windows analyzer accepts reconciled schema 2 loss
ledgers. Phase and CPU output expose total intake loss alongside ring loss.
Original final metric objects remain intact in output for schema 1 compatibility.

Run the focused checks with these commands:

```powershell
python scripts/performance/metrics-ledger-test.py -v
python scripts/performance/pooled-intake-run-test.py -v
```

The focused suite passes 14 tests, including the actual command paths of all
four consumers. The existing pooled-run validation suite passes 5 tests. The
independent reviewer identified one additional edge case: final samples in phase
analysis must reconcile even when the consumer allows intermediate samples.
The implementation and regression test include that correction.

Separate-output reanalysis matches five preserved schema 1 Windows analyses:
`auth-1`, `auth-1000qps-1`, `rec-1000qps-1`, and both
`ringfix-capacity8192-*-300s-1` runs. The first three contain ring loss; both
capacity-8192 runs have zero loss. Comparisons cover original final metrics,
capture/parser residuals, process counters, system counters, and window data.
All five archived final samples also pass the required statistics-error counter
validation. Updated receiver summaries preserve all previous output fields and
add final statistics validity and error counts.
Three receiver runs also retain their common archived counts and residuals; the
older receiver report predates the zero-loss flag.

The comparison record and generated analyses are under
`target/metrics-ledger-reanalysis-f52bd56b1fbe4dd7a58e27a22ce3055e/`.
Receiver reanalysis uses fresh copies of the service metric files because the
original deployment `Start.json` files contain lists while the existing loader
expects objects. That unrelated deployment-metadata incompatibility remains
outside this fix. No preserved artifact, Rust source, VM, or service changed.

The independent read-only review reports no remaining blocker and independently
passes all 14 focused tests, including the statistics-query follow-up, with
`python -B scripts/performance/metrics-ledger-test.py`.
The metrics-only receiver comparison verifies ledger compatibility; it does not
establish compatibility with the original deployment-metadata command inputs.
