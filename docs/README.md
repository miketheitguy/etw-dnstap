# Documentation index

> **Status:** Current · **Describes:** `main` at commit `9172f2d` · **Reviewed:** 2026-11-01

This index lists every document in `docs/`, with its status. Operators should start
with the [project README](../README.md) and the [guides](#guides). Maintainers can use the
design and performance sections. Administrators should not use the historical
campaign commands as procedures.

## Status conventions

Each document starts with a status block that gives its status, the date of the
evidence, the commit it describes, and the date it was last reviewed.

| Status | Meaning |
| --- | --- |
| **Current** | Describes the code and procedures on `main`. Kept up to date with the code. |
| **Historical** | A dated campaign, review, or plan. Its results, hashes, and counts apply to its recorded binaries and are not edited. |
| **Superseded** | Replaced by a later implementation or document. The header links the replacement. |
| **Proposed** | A design or plan that is not implemented or not yet executed. |

Commit references are given only where the document or the repository history
identifies them. A record that does not name a commit is marked *not recorded*.
The repository's history starts with `08254d4` (Initial Buildout), and it has no
release tags.

The service sends capture only to its required TCP receiver. It writes no
capture, metrics, or log files. Diagnostics go to the event log and to stderr.

## Guides

These are the operator guides. They are the source of truth for install,
configuration, troubleshooting, and removal.

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Install and provision](guide/install.md) | Current | not applicable | `9172f2d` | Toolchain, build, service identity, SCM recovery, receiver setup |
| [Configuration reference](guide/configuration.md) | Current | not applicable | `9172f2d` | Every option and default, the low-rate limits, subscription, session reclaim |
| [Troubleshooting](guide/troubleshooting.md) | Current | not applicable | `9172f2d` | Startup, restarts, drops, missing records, stale sessions, watchdog aborts |
| [Uninstall](guide/uninstall.md) | Current | not applicable | `9172f2d` | Service, event source, ETW session, and directory removal |
| [Security policy](../SECURITY.md) | Current | not applicable | `9172f2d` | Reporting, threat model summary |
| [Contributing](../CONTRIBUTING.md) | Current | not applicable | `9172f2d` | Build, test, lint, documentation status, commit style |
| [Changelog](../CHANGELOG.md) | Current | not applicable | `9172f2d` | Keep a Changelog format; `0.1.0` reconstructed from `git log` |

## Design and reference

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- |
| [Pooled ETW intake](pooled-intake.md) | Current | not applicable | present by `2c62a9f` (its results are documented there) | Ownership, memory budget, and loss accounting of the pooled intake |
| [Ring capacity and shutdown](ring-buffer.md) | Current | Oct 28-29, 2026 (evidence) | not recorded | Channel bounds, shutdown ordering, and the historical capacity experiment. The low-rate limits link to the configuration reference. |
| [Single-pass decoder](single-pass-decoder-plan.md) | Current | not applicable | `e59425f` | Implemented decoding plan, supported layouts, pooled-event lifetime |
| [Measurement terms and result gates](measurement-terms.md) | Current | not applicable | `9172f2d` | Units, counters, and gates for reading performance reports |
| [Service shutdown remediation](service-stop-remediation.md) | Current | not applicable | not recorded | Diagnosing receiver resets during STOP/FINISH and the close behavior |
| [Packet-flow diagram](diagrams/packet-flow.drawio) | Historical | Oct 28, 2026 | not recorded | Editable draw.io diagram of the October 28 design |
| [Code-architecture diagram](diagrams/code-architecture.drawio) | Historical (partly updated) | Oct 28, 2026 | not recorded | Editable draw.io diagram. The queue box is relabelled for the pooled crossbeam channel. Other boxes still reflect the October 28 layout. See the [note inside the diagram](#diagram-note). |

### Diagram note

The architecture diagram's title says "Historical intake architecture." Its queue
box is now labelled as the bounded crossbeam channel with pooled leases, which is
the current transport. The rest of its layout is the October 28 design. Use
[Pooled ETW intake](pooled-intake.md) for the current structure.

## Performance

### Current results

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Pooled intake VM performance review](pooled-vm-performance-review.md) | Current (latest native results; its binaries are historical) | Oct 29, 2026 (campaign `pooled-vm-20261029-0942`) | `2c62a9f` documents the results | The completed pooled campaign: 22 measured stages, 12 strict passes, 10 strict failures |
| [Pooled VM CPU and QPS graphs](pooled-vm-performance-graphs.md) | Historical | Oct 29, 2026 | `2c62a9f` | Graphs of the four primary runs |
| [Pooled VM trace analysis](pooled-vm-trace-analysis.md) | Historical | Oct 29, 2026 | `2c62a9f` | Native WPR evidence |
| [Pooled intake VM performance plan](pooled-vm-performance-plan.md) | Historical | Oct 29, 2026 | `2c62a9f` | Workload, commands, and approvals of the completed campaign |

The campaign directory `target/performance/pooled-vm-20261029-0942` is a local
artifact. It is not in a clone. The review is generated by
[`build-pooled-report.py`](../scripts/performance/build-pooled-report.py), and the
graphs by [`plot-pooled-wpr.py`](../scripts/performance/plot-pooled-wpr.py).
Edit the narrative template there, then regenerate. Do not edit the numbers.

The lab scripts have their own documentation at
[scripts/performance/README.md](../scripts/performance/README.md).

### Single-pass and the four-CPU run

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Single-pass decoder: four-CPU test results](singlepass-four-core-test-results.md) | Historical | Oct 30, 2026 UTC | not recorded (the run predates `e59425f`, which was committed on Oct 31 PDT) | One 100,000 QPS, 60-second authoritative run. Records were 0.43% short. Per-counter attribution and service settings were not recorded. |
| [Single-pass four-CPU test plan](singlepass-four-core-test-plan.md) | Historical | Oct 30, 2026 UTC | not recorded | Plan for the four-CPU run |

### Campaigns and reviews

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Lab validation](lab-validation.md) | Historical | Oct 28, 2026 PDT | not recorded | Initial live receiver and packet-delivery checks |
| [DNS capture performance review, 2026-10-28](performance-review-2026-10-28.md) | Historical | Oct 28, 2026 | not recorded | Original unpaced three-run experiment |
| [Windows DNS capture performance test plan](performance-test-plan.md) | Historical | Oct 28, 2026 | not recorded | Method for the unpaced experiment |
| [Performance repeat at global 1,000 QPS, plan](performance-1000qps-plan.md) | Historical | Oct 29, 2026 UTC | not recorded | Method for the repeat with the original 1,024-entry ring. Results are in [the review](performance-1000qps-review.md). |
| [Performance repeat at global 1,000 QPS, review](performance-1000qps-review.md) | Historical | Oct 29, 2026 UTC | not recorded | Results of the repeat |
| [Ring-buffer capacity fix: plan](ring-buffer-fix-plan.md) | Historical | Oct 29, 2026 UTC | not recorded | Capacity-only 8,192 candidate; validation gates |
| [Ring-buffer capacity fix: validation](ring-buffer-fix-validation.md) | Superseded by [pooled intake](pooled-intake.md) | Oct 29, 2026 UTC | not recorded | Results of the allocation-based `ArrayQueue` candidate |
| [Rejected wake-on-enqueue experiment](ring-wakeup.md) | Historical | Oct 29, 2026 UTC | not recorded | Rejected `wake-1024-v1` candidate. It did not rule out a wake once per buffer. |
| [Pooled ETW intake: validation plan](pooled-intake-plan.md) | Historical | Oct 2026 (undated in text) | not recorded | Original pooled validation design and ownership record |
| [Pooled intake audit and remaining validation plan](pooled-intake-audit-plan.md) | Historical | Oct 2026 (undated in text) | not recorded | Audit of the pooled implementation |
| [Pooled intake measurement review](pooled-intake-review-plan.md) | Historical | Oct 2026 (undated in text) | not recorded | Review of the Python analysis and metrics accounting |
| [Parser and decoder performance review](parser-decoder-performance-review.md) | Historical | not recorded | not recorded | Candidate changes for the parser and decoder |
| [Parser and TCP threading analysis](parser-tcp-threading-analysis.md) | Proposed | not recorded | not recorded | Design analysis for parallel parsing and TCP threading |
| [Parallel parser workers plan](parallel-parser-workers-plan.md) | Proposed | not recorded | not recorded | Plan to diagnose the bottleneck and test parallel workers |
| [Combined parser and writer WPR plan](parser-writer-wpr-plan.md) | Historical | not recorded | not recorded | WPR experiment; its `/DEBUG:NONE` setting is in `.cargo/config.toml` |
| [Combined parser and writer WPR results](parser-writer-wpr-results.md) | Historical | not recorded | not recorded | CPU and blocking results from that experiment |
| [Schema-cache lookup WPR comparison](schema-lookup-wpr-results.md) | Historical | not recorded | not recorded | Schema-cache comparison |
| [Remove the duplicate schema lookup](schema-single-lookup-plan.md) | Historical | not recorded | not recorded | Plan for a single schema lookup |
| [Symbolized release-optimized WPR: plan](symbolized-release-wpr-plan.md) | Historical | not recorded | not recorded | Staged procedure for a symbolized release profile |
| [Symbolized release-optimized WPR: results](symbolized-release-wpr-results.md) | Historical | not recorded | not recorded | Results of the symbolized release profile |
| [Direct PacketData encoding plan](direct-packet-encoding-plan.md) | Superseded by [single-pass decoding](single-pass-decoder-plan.md) | not recorded | not recorded | Transition from an intermediate packet vector to direct encoding |
| [Parser and encoder optimization plan](parser-encoder-optimization-plan.md) | Superseded by [single-pass decoding](single-pass-decoder-plan.md) | not recorded | not recorded | Earlier reusable packet-vector implementation |
| [Windows V2 performance counter plan](windows-performance-counters-v2-plan.md) | Proposed | not applicable | not applicable | Future per-stage counters; no task has been executed |

## Service behavior and source reviews

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [ETW subscription and intake filter plan](etw-subscription-filter-plan.md) | Historical | not recorded | not recorded | Narrower subscription and accepted event IDs |
| [ETW keyword narrowing plan](etw-keyword-narrowing-plan.md) | Historical | not recorded | not recorded | Keyword narrowing and its provider-metadata checks |
| [ETW session recovery and event-ID filter plan](etw-session-recovery-and-event-filter-plan.md) | Historical | Oct 30, 2026 | not recorded | Stale session reclaim and the event-ID filter |
| [Source review remediation plan](src-review-remediation-plan.md) | Historical | Oct 30, 2026 | not recorded | Remediation of selected findings from the `src` review |
| [Live load validation plan](live-load-validation-plan.md) | Proposed (execution not recorded) | Oct 30, 2026 | not recorded | Load checks deferred from the October 30 review |
| [Service shutdown fix plan](service-stop-fix-plan.md) | Superseded by [service shutdown remediation](service-stop-remediation.md) | not recorded | not recorded | Recorded STOP/FINISH failure and its implementation boundaries |
| [Service shutdown validation plan](service-stop-validation-plan.md) | Historical | not recorded | not recorded | Bounded receiver-close comparison |
| [Service shutdown validation results](service-stop-validation-results.md) | Historical | not recorded | not recorded | Results of that comparison |

## Filtered DNS ramp

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Filtered DNS ramp plan](filtered-ramp-plan.md) | Proposed (must be regenerated before the next campaign) | Oct 30, 2026 | `3d401cc` and later | Client-event filter ramp. Its capture contract predates `e59425f` and `9172f2d`. See the banner in the document. |
| [Filtered DNS ramp review](filtered-ramp-review.md) | Historical (in progress at its date) | Oct 30, 2026 | `3d401cc` (pushed commit cited) | Snapshot of the filtered campaign. It does not establish collector counts or a maximum rate. |
| [Parser optimization DNS ramp plan](parser-ramp-plan.md) | Superseded (aborted; do not resume) | not recorded | not recorded | Ramp plan for the parser and encoder optimization |
| [Parser optimization DNS ramp review](parser-ramp-review.md) | Superseded (aborted; do not resume) | not recorded | not recorded | Partial record of the aborted parser ramp |

## Evidence and supporting material

- [DNS workload provenance](../datasets/performance/README.md): recreates the pinned
  5,000-name test workload.
- [DNS stress client](../tools/dns-stress/README.md): builds and runs the host
  load generator, and explains its output.
- `docs/assets/performance/`: graph derivatives and CSV copies. Their source
  manifests are in `docs/assets/performance/data/source-manifest.json` and
  `docs/assets/performance/plot-provenance.json`.

Raw ETLs, receiver archives, build logs, and campaign receipts are under
`target/`. That path is gitignored. It is a local artifact and is not in a fresh
clone. A prose summary or a generated graph does not replace its source manifest,
final ledger, or integrity check.

Campaign commands in the historical records contain recorded VM addresses,
fixed directories, and state-changing actions. They describe the recorded
experiment. They are not procedures for the current service. Before another
campaign, design compatible telemetry, use a fresh backup, unique run IDs,
verified guest identities, and fixture readiness.

## Instrumentation compatibility note

The historical campaigns used binaries with a service JSONL metrics writer. That
writer was removed. The commands, final ledgers, and measured overhead in those
records apply to the recorded binaries only.

The historical service-metrics harnesses require that legacy instrumentation.
They reject new campaigns before any VM, load, or SCM change. Existing evidence and
cleanup actions remain available.

The current service keeps diagnostic counters, but it has no durable metrics
export and no queue high-water tracking. Its performance has not been measured by
the historical campaigns. Measuring it needs a separately designed telemetry
method. Collector output, WPR traces, and host tool artifacts are separate from
the service.

Use the [project README](../README.md) and the [configuration reference](guide/configuration.md)
for the supported service options.

## Planned layout

The folders below are the intended layout. Nothing has been moved yet. Moves
need `git mv`, so that history is kept, and the relative links must be updated
afterward. The paths in this index are the current ones.

- `guide/`: operator guides (exists)
- `design/`: architecture and reference (pooled intake, ring and queue, measurement terms)
- `performance/`: current results and tuning guidance
- `history/`: finished campaigns, reviews, and superseded plans
