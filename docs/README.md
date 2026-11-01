# Documentation index

Use this index to find installation instructions, understand the capture path,
or assess the evidence for a performance claim. Administrators should start
with the [project README](../README.md); maintainers can use the references and
campaign records listed here.

The service sends capture only to a required TCP receiver and writes no local
capture, metrics, or log files. The historical campaigns used binaries with a
service JSONL metrics writer that has been removed. Their commands, final
ledgers, and measured overhead apply to those recorded binaries. The historical
service-metrics harnesses require that legacy instrumentation and reject new campaigns before
VM, load, or SCM changes. Existing evidence and cleanup actions remain available.
Measuring this service needs a separately designed telemetry method. Removal
also eliminates per-event timing, periodic ETW statistics queries, and queue
high-water tracking, so the archived performance is not a measurement of this
build. No replacement durable metrics export is provided. Collector output,
WPR traces, and host tool artifacts remain
separate from the service. Use the README for supported service arguments.

The separate [filtered DNS ramp plan](filtered-ramp-plan.md) uses client and
collector counts with the TCP-only service. Its Windows and receiver helpers
preserve historical guards and export no service metrics. It starts a fresh
campaign after a verified build, commit, push, deployment, and receiver gate.
The earlier [parser ramp](parser-ramp-review.md) was aborted by the user and
must not be resumed.

## Install, operate, and understand capture

The following pages describe the service and its measurement tools:

| Task | Document |
| --- | --- |
| Build, provision, configure, and stop the service | [Project README](../README.md) |
| Understand STOP/FINISH shutdown resets and configure graceful receiver close | [Service stop remediation](service-stop-remediation.md) |
| Assess the bounded receiver-close comparison and its evidence limits | [Service shutdown validation results](service-stop-validation-results.md) |
| Review the ETW subscription, accepted event IDs, and local validation | [ETW subscription filter plan](etw-subscription-filter-plan.md) |
| Review ETW event-ID filtering and stale ETW session recovery | [ETW session recovery and event-ID filter plan](etw-session-recovery-and-event-filter-plan.md) |
| Review service, ETW session, intake, parser polling, and network remediation | [Source review remediation plan](src-review-remediation-plan.md) |
| Review ETW keyword narrowing, its provider-metadata checks, and live validation | [ETW keyword narrowing plan](etw-keyword-narrowing-plan.md) |
| Review Event Log counters, receiver output bounds, and the live and heavy load checks | [Live load validation plan](live-load-validation-plan.md) |
| Plan the bottleneck diagnosis and parallel parser workers sharing one ETW feed | [Parallel parser workers plan](parallel-parser-workers-plan.md) |
| Review pool ownership, callback allocation scope, and loss counters | [Pooled ETW intake](pooled-intake.md) |
| Review single-pass decoding, TDH fallback, pooled-event lifetime, and local validation | [Single-pass decoder plan](single-pass-decoder-plan.md) |
| Review the historical transition to direct packet encoding | [Direct packet encoding plan](direct-packet-encoding-plan.md) |
| Review the historical parser and encoder reuse implementation and its validation | [Parser and encoder optimization plan](parser-encoder-optimization-plan.md) |
| Choose queue and pool capacity; review final-drain ownership | [Ring capacity and shutdown](ring-buffer.md) |
| Review the proposed per-stage Windows V2 counter design and implementation tasks | [Windows V2 performance counter plan](windows-performance-counters-v2-plan.md) |
| Interpret QPS, record counts, CPU units, and independent result gates | [Measurement terms and result gates](measurement-terms.md) |
| Build and run the host DNS load generator; interpret its output | [DNS stress client](../tools/dns-stress/README.md) |
| Recreate the pinned 5,000-name test workload | [DNS workload provenance](../datasets/performance/README.md) |

The [historical packet-flow diagram](diagrams/packet-flow.drawio) and
[historical code-architecture diagram](diagrams/code-architecture.drawio) are
editable draw.io files based on the October 28, 2026 lab. The architecture
diagram retains the earlier allocation-based `ArrayQueue` transport and has
its obsolete capture-file branch removed. For the pooled channel,
ownership, and memory budget, use the README and pooled-intake reference.

## Assess the completed pooled VM campaign

The campaign at `target/performance/pooled-vm-20261029-0942` contains 22 measured
stages. Both high-rate five-minute confirmations pass every gate. The strict
stage outcomes remain 12 passes and 10 failures; a passing WPR trace gate or
exact receiver ledger alone does not make a stage pass.

Choose the page that answers your question:

| Question | Document |
| --- | --- |
| What rates passed, what failed, and what was restored? | [Pooled intake VM performance review](pooled-vm-performance-review.md) |
| How did CPU and achieved sending rate vary during the four primary runs? | [Pooled VM CPU and QPS graphs](pooled-vm-performance-graphs.md) |
| What do native CPU, Ready/Wait, I/O, and trace-retention observations support? | [Pooled VM trace analysis](pooled-vm-trace-analysis.md) |
| Which workload, commands, guards, approvals, and exceptions produced the results? | [Pooled intake VM performance plan](pooled-vm-performance-plan.md) |

The review is generated by
[`build-pooled-report.py`](../scripts/performance/build-pooled-report.py).
Change its narrative template when updating report prose, then regenerate from
the preserved evidence. The [plot renderer](../scripts/performance/plot-pooled-wpr.py)
owns the graph SVGs, CSV copies, and plot provenance; the graph methods page is
maintained separately. Do not edit numeric evidence to change a result.

## Review earlier decisions and validation

These dated records explain how the service reached the pooled implementation.
Their hashes, test counts, installed-state descriptions, and acceptance criteria
apply at each record's cutoff. Read the completed pooled VM review for the
later native campaign; do not treat an older plan's pending work as its status.

| Evidence or decision | Plan or method | Results |
| --- | --- | --- |
| Initial live service, payload preservation, and receiver checks; October 28, 2026 PDT | [README deployment requirements](../README.md#provision-a-service) | [Lab validation](lab-validation.md) |
| Original unpaced three-run experiment; October 28, 2026 PDT | [Performance test plan](performance-test-plan.md) | [DNS capture performance review](performance-review-2026-10-28.md) |
| Original 1,024-entry ring at a global 1,000 QPS ceiling; October 29, 2026 UTC | [1,000 QPS repeat plan](performance-1000qps-plan.md) | [1,000 QPS repeat review](performance-1000qps-review.md) |
| Capacity-only 8,192-entry candidate and rejected wake experiment; October 29, 2026 UTC | [Ring-buffer fix plan](ring-buffer-fix-plan.md) | [Capacity validation](ring-buffer-fix-validation.md) and [rejected wake experiment](ring-wakeup.md) |
| Pooled implementation and scoped local validation | [Original pooled validation plan](pooled-intake-plan.md) | [Pooled intake audit and follow-up](pooled-intake-audit-plan.md) |
| Metrics schema 2 accounting correction | [Measurement review and verification record](pooled-intake-review-plan.md) | [Audit acceptance and artifact identities](pooled-intake-audit-plan.md#artifact-identity-and-evidence-limits) |

Campaign commands contain recorded VM addresses, fixed campaign directories,
and state-changing setup or restoration actions. Use them to understand the
recorded experiment; they are not procedures for the TCP-only service. Before
another campaign, design and validate compatible telemetry, then establish a fresh backup, unique
run IDs, verified guest identities, and fixture readiness as the relevant plan
requires.

## Evidence availability

Small repository-owned receipts and graph derivatives live under `datasets/`
and `docs/assets/performance/`. Raw ETLs, receiver archives, build logs, and
campaign receipts live under ignored `target/` paths. A fresh checkout does not
contain that local evidence. A prose summary or generated graph does not replace
its source manifests, final ledgers, or integrity checks.
