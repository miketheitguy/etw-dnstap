# Documentation index

> **Status:** Current · **Describes:** `main` at commit `5f610f5` for the rows marked with that commit (other rows show the commit they were last checked against) · **Reviewed:** 2026-11-01

This index lists every document in `docs/`, with its status. Operators should start
with the [project README](../README.md) and the [guides](#guides). Maintainers can use the
[design](#design-and-reference) and [performance](#performance) sections.
Administrators should not use the historical campaign commands as procedures.

## Layout

Each folder holds one kind of document. The status block at the top of a document
decides its folder.

| Folder | Contents | Statuses |
| --- | --- | --- |
| [`guide/`](guide/) | Operator guides: install, configuration, troubleshooting, shutdown remediation, uninstall | Current |
| [`design/`](design/) | Architecture and reference: pooled intake, ring and queue, single-pass decoder contract, measurement terms, diagrams | Current |
| [`performance/`](performance/) | The latest completed native performance record and its graphs, trace analysis, and campaign plan | Current review; supporting records Historical |
| [`plans/`](plans/) | Plans and analyses that are not yet executed or implemented | Proposed; a plan with partial execution says so in its status block |
| [`history/`](history/) | Finished campaigns, reviews, completed plans, and superseded documents | Historical, Superseded |
| [`assets/performance/`](assets/performance/) | Graph SVGs, CSV copies, and their provenance manifests | Generated |

The graph assets stay at `docs/assets/performance/` because
[`plot-pooled-wpr.py`](../scripts/performance/plot-pooled-wpr.py) writes there by
default. The pooled campaign plan, graphs, and trace analysis stay next to the
review in `performance/` because
[`build-pooled-report.py`](../scripts/performance/build-pooled-report.py)
generates sibling links to them.

Moves keep history with `git mv`. When you move a document, update every relative
link to it and run a link check over all tracked Markdown files.

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

Historical records keep their recorded command text and source paths. Where a
lab script or source file has moved since, the record has a short note that names
the new path. It is not rewritten. See the
[script moves and renames](../scripts/performance/README.md#file-moves-and-renames)
for commit `6e62b2e`. Commit `dc9d7e0` split the ETW code into `src/etw/`
(`session.rs`, `keywords.rs`, `intake.rs`, `schema.rs`, `diag.rs`, and the other
modules there). `src/etw.rs` remains as the module root. `src/event.rs` and
`src/sink.rs` are the seams for the output sink.

The service sends capture only to its required TCP receiver. It writes no
capture, metrics, or log files. Diagnostics go to the event log and to stderr.

## Guides

These are the operator guides in `guide/`. They are the source of truth for
install, configuration, troubleshooting, and removal.

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Install and provision](guide/install.md) | Current; LocalService restricted-SID profile validated on one lab host (build 26100) | Nov 1, 2026 (lab validation) | `5f610f5` | Toolchain, build, LocalService restricted-SID identity, privileges, SCM recovery, receiver setup, validation checklist |
| [Configuration reference](guide/configuration.md) | Current | not applicable | `efb47af` | Every option and default, the low-rate limits, DNS process filter, receiver connection, deadlines, session reclaim, log events and counters |
| [Troubleshooting](guide/troubleshooting.md) | Current | not applicable | `efb47af` | Event log messages, startup failures, receiver outages, DNS restarts, counters, watchdog aborts |
| [Service shutdown remediation](guide/service-stop-remediation.md) | Current (operator guide) | not applicable | not recorded | Diagnosing receiver resets during STOP/FINISH and the close behavior |
| [Uninstall](guide/uninstall.md) | Current | not applicable | `5f610f5` | Service, group membership, event source, ETW session, and directory removal |
| [Security policy](../SECURITY.md) | Current | not applicable | `5f610f5` | Reporting, threat model summary, producer identity, LocalService restricted-SID profile |
| [Contributing](../CONTRIBUTING.md) | Current | not applicable | `5f610f5` | Build, test, lint, documentation status and layout, commit style |
| [Changelog](../CHANGELOG.md) | Current | not applicable | `5f610f5` | Keep a Changelog format; versioning policy at the top; no tag yet; commit log through `9172f2d` |

## Design and reference

These documents are in `design/`.

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Pooled ETW intake](design/pooled-intake.md) | Current | not applicable | present by `2c62a9f` (its results are documented there) | Ownership, memory budget, and loss accounting of the pooled intake |
| [Ring capacity and shutdown](design/ring-buffer.md) | Current | not applicable (reference) | not recorded | Channel bounds and shutdown ordering. The low-rate limits link to the configuration reference. The historical capacity experiment is in [Ring capacity experiment](history/ring-capacity-experiment.md). |
| [Single-pass decoder](design/single-pass-decoder.md) | Current | not applicable (reference; local tests only) | `efb47af` (decoder added in `e59425f`) | Current fast-path grammar, fallback rule, bounds, and validation scope |
| [Measurement terms and result gates](design/measurement-terms.md) | Current | not applicable | `54eafae` | Units, counters, and gates for reading performance reports, and how the live counters relate to them |
| [Code-architecture diagram](design/diagrams/code-architecture.drawio) | Current (labels updated; box positions from Oct 28, 2026) | Oct 28, 2026 (positions) | `54eafae` (labels) | Editable draw.io diagram. Labels match the module split, DNS process filter, and reconnecting sink. See the [diagram note](#diagram-note). |

The rejected wake-on-enqueue experiment, which explains why the ETW callback stays
wake-free, is a historical record:
[history/ring-wakeup.md](history/ring-wakeup.md).

### Diagram note

The architecture diagram's title is "Code architecture." Its labels match `main`
at `54eafae`: the `src/etw/` module split, the bounded crossbeam channel with
pooled leases, the DNS process filter, the LocalService restricted-token gate, and the
reconnecting network sink. Its box positions are still those of the October 28
layout. Use [Pooled ETW intake](design/pooled-intake.md) for the current
intake design.

## Performance

These documents are in `performance/`. They are the latest completed native
campaign, `pooled-vm-20261029-0942`.

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Pooled intake VM performance review](performance/pooled-vm-performance-review.md) | Current (latest native results; its binaries are historical) | Oct 29, 2026 (campaign `pooled-vm-20261029-0942`) | `2c62a9f` documents the results | The completed pooled campaign: 22 measured stages, 12 strict passes, 10 strict failures. Run under LocalSystem, not the current LocalService restricted-SID profile. |
| [Pooled VM CPU and QPS graphs](performance/pooled-vm-performance-graphs.md) | Historical | Oct 29, 2026 | `2c62a9f` | Graphs of the four primary runs |
| [Pooled VM trace analysis](performance/pooled-vm-trace-analysis.md) | Historical | Oct 29, 2026 | `2c62a9f` | Native WPR evidence |
| [Pooled intake VM performance plan](performance/pooled-vm-performance-plan.md) | Historical | Oct 29, 2026 | `2c62a9f` | Workload, commands, and approvals of the completed campaign |

The campaign directory `target/performance/pooled-vm-20261029-0942` is a local
artifact. It is not in a clone. The review is generated by
[`build-pooled-report.py`](../scripts/performance/build-pooled-report.py), and the
graphs by [`plot-pooled-wpr.py`](../scripts/performance/plot-pooled-wpr.py).
Edit the narrative template there, then regenerate. Do not edit the numbers.

The lab scripts have their own documentation at
[scripts/performance/README.md](../scripts/performance/README.md).

## Plans

These documents are in `plans/`. Each status block gives its execution state. The parallel parser workers plan is partly executed. The others have no recorded execution.

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Filtered DNS ramp plan](plans/filtered-ramp-plan.md) | Proposed (must be regenerated before the next campaign) | Oct 30, 2026 | `3d401cc` and later | Client-event filter ramp. Its capture contract predates `e59425f`, `9172f2d`, and `f0b51f4` (service identity, DNS PID filter, reconnect). See the banner in the document. |
| [Parser and TCP threading analysis](plans/parser-tcp-threading-analysis.md) | Proposed | not recorded | not recorded | Design analysis for parallel parsing and TCP threading. Its fail-fast contract predates `f0b51f4`. |
| [Parallel parser workers plan](plans/parallel-parser-workers-plan.md) | Proposed; partly executed (P0 and diagnostic runs recorded Oct 31, 2026; P1a next; P4 deferred) | Oct 31, 2026 | not recorded | Plan to diagnose the bottleneck and test parallel workers. Its no-reconnect rows predate `f0b51f4`. |
| [Windows V2 performance counter plan](plans/windows-performance-counters-v2-plan.md) | Proposed | not applicable | not applicable | Future per-stage counters; no task has been executed. Its baseline predates periodic counter logging and `f0b51f4`. |
| [Obsolete and legacy code cleanup plan](plans/cleanup-plan.md) | Proposed; in progress (waves run in order; the plan moves to history when it completes) | Nov 1, 2026 | `efb47af` (written at) | Removal of dead code, legacy names, duplicate scaffolding, and stale docs. Each wave is one commit with a gate. |

## History

These documents are in `history/`. They are dated records. Do not use their
commands as procedures.

### Campaigns and reviews

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Lab validation](history/lab-validation.md) | Historical | Oct 28, 2026 PDT | not recorded | Initial live receiver and packet-delivery checks |
| [Live load validation plan](history/live-load-validation-plan.md) | Historical (executed L1-L5; the pending L4 action was not done) | Oct 30-31, 2026 | not recorded | Load checks deferred from the October 30 review, with the Stage L5 comparison. Its premise that counters reach only stderr is out of date. |
| [LocalService restricted-SID validation](history/localservice-restricted-validation.md) | Historical | Nov 1, 2026 | parent `6718863` | ETW rights the restricted token needs; the 0x20-only grant |
| [Skills-driven evaluation plan](history/skills-evaluation-plan.md) | Historical (executed Nov 1, 2026) | Nov 1, 2026 | plan `e8560ac`; execution through `5f610f5` | Evaluation of code, build, CI and service hardening against the installed skills. Results: [lab record](history/skills-evaluation-lab-validation.md) and the commit log. |
| [Skills evaluation lab validation](history/skills-evaluation-lab-validation.md) | Historical | Nov 1, 2026 | `77d7c47` (with one uncommitted script fix, now in `d721569`) | Lab run of the evaluation changes: mutex removal, reclaim, DNS handle (denied), pinned SDDL, delayed-auto start, backup and restore, SCM recovery with a DNS restart |
| [DNS capture performance review, 2026-10-28](history/performance-review-2026-10-28.md) | Historical | Oct 28, 2026 | not recorded | Original unpaced three-run experiment |
| [Windows DNS capture performance test plan](history/performance-test-plan.md) | Historical | Oct 28, 2026 | not recorded | Method for the unpaced experiment |
| [Performance repeat at global 1,000 QPS, plan](history/performance-1000qps-plan.md) | Historical | Oct 29, 2026 UTC | not recorded | Method for the repeat with the original 1,024-entry ring. Results are in [the review](history/performance-1000qps-review.md). |
| [Performance repeat at global 1,000 QPS, review](history/performance-1000qps-review.md) | Historical | Oct 29, 2026 UTC | not recorded | Results of the repeat |
| [Ring-buffer capacity fix: plan](history/ring-buffer-fix-plan.md) | Historical | Oct 29, 2026 UTC | not recorded | Capacity-only 8,192 candidate; validation gates |
| [Ring-buffer capacity fix: validation](history/ring-buffer-fix-validation.md) | Superseded by [pooled intake](design/pooled-intake.md) | Oct 29, 2026 UTC | not recorded | Results of the allocation-based `ArrayQueue` candidate |
| [Ring capacity experiment](history/ring-capacity-experiment.md) | Historical | Oct 28-29, 2026 | not recorded | The allocation-based `ArrayQueue` capacity experiment that set the 8,192 default. Moved from the ring-capacity design page. |
| [Rejected wake-on-enqueue experiment](history/ring-wakeup.md) | Historical | Oct 29, 2026 UTC | not recorded | Rejected `wake-1024-v1` candidate. It did not rule out a wake once per buffer. |
| [Pooled ETW intake: validation plan](history/pooled-intake-plan.md) | Historical | Oct 2026 (undated in text) | not recorded | Original pooled validation design and ownership record |
| [Pooled intake audit and remaining validation plan](history/pooled-intake-audit-plan.md) | Historical | Oct 2026 (undated in text) | not recorded | Audit of the pooled implementation |
| [Pooled intake measurement review](history/pooled-intake-review-plan.md) | Historical | Oct 2026 (undated in text) | not recorded | Review of the Python analysis and metrics accounting |
| [Single-pass decoder: four-CPU test results](history/singlepass-four-core-test-results.md) | Historical | Oct 30, 2026 UTC | not recorded (the run predates `e59425f`, which was committed on Oct 31 PDT) | One 100,000 QPS, 60-second authoritative run. Records were 0.43% short. Per-counter attribution and service settings were not recorded. |
| [Single-pass four-CPU test plan](history/singlepass-four-core-test-plan.md) | Historical | Oct 30, 2026 UTC | not recorded | Plan for the four-CPU run |
| [Filtered DNS ramp review](history/filtered-ramp-review.md) | Historical (in progress at its date) | Oct 30, 2026 | `3d401cc` (pushed commit cited) | Snapshot of the filtered campaign. It does not establish collector counts or a maximum rate. |
| [Parser optimization DNS ramp plan](history/parser-ramp-plan.md) | Superseded (aborted; do not resume) | not recorded | not recorded | Ramp plan for the parser and encoder optimization |
| [Parser optimization DNS ramp review](history/parser-ramp-review.md) | Superseded (aborted; do not resume) | not recorded | not recorded | Partial record of the aborted parser ramp |

### Parser, decoder, and profiling records

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [Parser and decoder performance review](history/parser-decoder-performance-review.md) | Historical | not recorded | not recorded | Candidate changes for the parser and decoder |
| [Combined parser and writer WPR plan](history/parser-writer-wpr-plan.md) | Historical | not recorded | not recorded | WPR experiment; its `/DEBUG:NONE` setting is in `.cargo/config.toml` |
| [Combined parser and writer WPR results](history/parser-writer-wpr-results.md) | Historical | not recorded | not recorded | CPU and blocking results from that experiment |
| [Schema-cache lookup WPR comparison](history/schema-lookup-wpr-results.md) | Historical | not recorded | not recorded | Schema-cache comparison |
| [Remove the duplicate schema lookup](history/schema-single-lookup-plan.md) | Historical | not recorded | not recorded | Plan for a single schema lookup |
| [Symbolized release-optimized WPR: plan](history/symbolized-release-wpr-plan.md) | Historical | not recorded | not recorded | Staged procedure for a symbolized release profile |
| [Symbolized release-optimized WPR: results](history/symbolized-release-wpr-results.md) | Historical | not recorded | not recorded | Results of the symbolized release profile |
| [Direct PacketData encoding plan](history/direct-packet-encoding-plan.md) | Superseded by [single-pass decoding](design/single-pass-decoder.md) | not recorded | not recorded | Transition from an intermediate packet vector to direct encoding |
| [Parser and encoder optimization plan](history/parser-encoder-optimization-plan.md) | Superseded by [single-pass decoding](design/single-pass-decoder.md) | not recorded | not recorded | Earlier reusable packet-vector implementation |
| [Single-pass decoder: implementation plan](history/single-pass-decoder-plan.md) | Historical (completion record; the current contract is [Single-pass decoder](design/single-pass-decoder.md)) | Oct 30, 2026 | `e59425f` | Staged implementation plan, acceptance gates, and completion record for the fast path |
| [Packet-flow diagram](history/diagrams/packet-flow.drawio) | Historical | Oct 28, 2026 | not recorded | Editable draw.io diagram of the October 28 design |

### Service behavior and source reviews

| Document | Status | Evidence date | Commit | Summary |
| --- | --- | --- | --- | --- |
| [ETW subscription and intake filter plan](history/etw-subscription-filter-plan.md) | Historical | not recorded | not recorded | Narrower subscription and accepted event IDs |
| [ETW keyword narrowing plan](history/etw-keyword-narrowing-plan.md) | Historical | not recorded | not recorded | Keyword narrowing and its provider-metadata checks |
| [ETW session recovery and event-ID filter plan](history/etw-session-recovery-and-event-filter-plan.md) | Historical | Oct 30, 2026 | not recorded | Stale session reclaim and the event-ID filter |
| [Source review remediation plan](history/src-review-remediation-plan.md) | Historical | Oct 30, 2026 | not recorded | Remediation of selected findings from the `src` review |
| [Service shutdown fix plan](history/service-stop-fix-plan.md) | Superseded by [service shutdown remediation](guide/service-stop-remediation.md) | not recorded | not recorded | Recorded STOP/FINISH failure and its implementation boundaries |
| [Service shutdown validation plan](history/service-stop-validation-plan.md) | Historical | not recorded | not recorded | Bounded receiver-close comparison |
| [Service shutdown validation results](history/service-stop-validation-results.md) | Historical | not recorded | not recorded | Results of that comparison |

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
clone. A link into `target/` must say that it is a local artifact. A prose
summary or a generated graph does not replace its source manifest, final ledger,
or integrity check.

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

The current service writes its diagnostic counters to the Application event log
every `--stats-interval-seconds` and at shutdown. It has no durable metrics
export. It samples the deepest queue depth as a lower bound (`queue_depth_max`). Its performance has not been measured by
the historical campaigns. Measuring it needs a separately designed telemetry
method. Collector output, WPR traces, and host tool artifacts are separate from
the service.

Use the [project README](../README.md) and the [configuration reference](guide/configuration.md)
for the supported service options.
