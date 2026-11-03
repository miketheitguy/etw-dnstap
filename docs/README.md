# Documentation index

> **Status:** Current · **Describes:** `tcp-sink-spool` at commit `0569136` for the layout, conventions, and file list · **Reviewed:** 2026-11-03

This index lists every document in `docs/`. Each document starts with a status
block that gives its status, evidence date, commit, and review date. This page
does not repeat them.

## Where to start

| You are... | Start with |
| --- | --- |
| An operator who installs or runs the service | [Project README](../README.md), then the [operator guides](#operator-guides) |
| A maintainer who changes the code | [Contributing](../CONTRIBUTING.md), then [Design and reference](#design-and-reference) |
| A reader who checks performance claims | [Performance](#performance) and [Lab and validation records](#lab-and-validation-records) |

## Layout

Each folder holds one kind of document. The status in a document's status block
decides its folder.

| Folder | Contents | Status |
| --- | --- | --- |
| [`guide/`](guide/) | Operator guides | Current |
| [`design/`](design/) | Architecture and reference | Current |
| [`performance/`](performance/) | The latest native performance record, its graphs, trace analysis, and campaign plan | Current review. Supporting records are Historical |
| [`plans/`](plans/) | Plans and analyses that are not yet executed | Proposed. A partly executed plan says so in its status block |
| [`history/`](history/) | Finished campaigns, reviews, completed plans, and superseded documents | Historical, Superseded |
| [`assets/performance/`](assets/performance/) | Graph SVGs, CSV copies, and provenance manifests | Generated |
| [`assets/pipeline/`](assets/pipeline/) | Animated GIF of the data path from an ETW event to a TCP frame | Current |
| [`assets/loss-solutioning/`](assets/loss-solutioning/) | Animated GIFs of the event-loss causes and fixes | Historical |

The graph assets stay in `docs/assets/performance/` because
[`plot-pooled-wpr.py`](../scripts/performance/plot-pooled-wpr.py) writes there by
default. The pooled campaign plan, graphs, and trace analysis stay next to the
review in `performance/` because
[`build-pooled-report.py`](../scripts/performance/build-pooled-report.py)
generates sibling links to them.

Inside the Current folders, each document has one type: procedure, reference,
diagnosis, or design. See [Document types](../CONTRIBUTING.md#document-types)
and [Writing style](../CONTRIBUTING.md#writing-style). To move a document, see
[Documentation](../CONTRIBUTING.md#documentation).

## Status values

| Status | Meaning |
| --- | --- |
| **Current** | Describes the code and procedures on `main`. Kept up to date with the code. |
| **Historical** | A dated campaign, review, or plan. Its results, hashes, and counts apply to its recorded binaries and are not edited. |
| **Superseded** | Replaced by a later implementation or document. The status block links the replacement. |
| **Proposed** | A design or plan that is not implemented or not yet executed. |

A commit reference appears only where the document or the repository history
identifies it. Otherwise the record says *not recorded*. The repository history
starts with `08254d4` (Initial Buildout). The repository has no release tags.

## Operator guides

These guides in [`guide/`](guide/) are the source of truth for install,
configuration, troubleshooting, and removal.

| Task | Document |
| --- | --- |
| Build the executable | [Build the executable](guide/build.md) |
| Install and register the service | [Install and provision](guide/install.md) |
| Prepare the receiver and its certificate | [Set up the dnstap receiver](guide/receiver.md) |
| Look up an option, default, or range | [Configuration reference](guide/configuration.md) |
| Choose settings for a high query rate | [Tune capture for load](guide/tuning.md) |
| Read the log events and counters | [Log events and counters](guide/monitoring.md) |
| Fix a failed start, drops, or missing records | [Troubleshooting](guide/troubleshooting.md) |
| Diagnose receiver resets during service stop | [Service shutdown remediation](guide/service-stop-remediation.md) |
| Change or upgrade an installed service | [Repair or upgrade](guide/upgrade.md) |
| Remove the service and its leftovers | [Uninstall](guide/uninstall.md) |

Policy documents in the repository root:

| Document | Contents |
| --- | --- |
| [Security policy](../SECURITY.md) | Reporting, threat model summary, producer identity, LocalService restricted-SID profile |
| [Contributing](../CONTRIBUTING.md) | Build, test, lint, documentation conventions, commit style |
| [Changelog](../CHANGELOG.md) | Keep a Changelog format and the versioning policy. No tag yet |

## Design and reference

These documents in [`design/`](design/) help maintainers.

| Document | Contents |
| --- | --- |
| [Data path](design/data-path.md) | The five stages from an ETW event to a TCP frame, with the thread, code, and loss counters of each stage. Includes the animation |
| [Pooled ETW intake](design/pooled-intake.md) | Ownership, memory budget, buffer-callback parser wake, and loss accounting |
| [Ring capacity and shutdown](design/ring-buffer.md) | Channel bounds, parser wake and linger, ETW delivery batches, the removed periodic flush, and shutdown order |
| [Single-pass decoder](design/single-pass-decoder.md) | Decoder grammar, the schema rejection rule (no per-event TDH reader), bounds, and validation scope |
| [Measurement terms and result gates](design/measurement-terms.md) | Units, counters, and gates for reading performance reports |
| [Code-architecture diagram](design/diagrams/code-architecture.drawio) | Editable draw.io diagram. See the [diagram note](#diagram-note). |

### Diagram note

The diagram's labels match `main` at `a2ae289`. They show the `src/etw/` module split, the
bounded crossbeam channel with pooled leases, the DNS process filter, the
LocalService restricted-token gate, and the reconnecting network sink. Its box
positions are from the October 28, 2026 layout.

It does not show the single-pass decoder (`etw/decode.rs`) and its schema
rejection rule (`ee94de6`), or the ETW buffer callback that wakes the parser
(`7f91264`). For the current design, use
[Data path](design/data-path.md),
[Pooled ETW intake](design/pooled-intake.md), and
[Single-pass decoder](design/single-pass-decoder.md).

## Performance

These documents in [`performance/`](performance/) record the latest completed
native campaign, `pooled-vm-20261029-0942`. The campaign ran under LocalSystem,
not the current LocalService restricted-SID profile.

| Document | Contents |
| --- | --- |
| [Pooled intake VM performance review](performance/pooled-vm-performance-review.md) | The results: 22 measured stages, 12 strict passes, 10 strict failures. Status Current, with historical binaries |
| [Pooled VM CPU and QPS graphs](performance/pooled-vm-performance-graphs.md) | Graphs of the four primary runs |
| [Pooled VM trace analysis](performance/pooled-vm-trace-analysis.md) | Native WPR evidence |
| [Pooled intake VM performance plan](performance/pooled-vm-performance-plan.md) | Workload, commands, and approvals of the campaign |

The campaign directory `target/performance/pooled-vm-20261029-0942` is a local
artifact. It is not in a clone. [`build-pooled-report.py`](../scripts/performance/build-pooled-report.py)
generates the review, and [`plot-pooled-wpr.py`](../scripts/performance/plot-pooled-wpr.py)
generates the graphs. Edit the narrative template in the script, then
regenerate. Do not edit the numbers. The lab scripts are documented in
[scripts/performance/README.md](../scripts/performance/README.md).

## Plans

These documents in [`plans/`](plans/) are not yet executed, or only partly. The
status block of each gives its execution state. A plan moves to `history/` when
it is executed.

| Document | State | Contents |
| --- | --- | --- |
| [Code review remediation plan](plans/code-review-remediation-plan.md) | Proposed. In progress on the `plan/code-review-remediation` branch | Findings of the 2026-11-01 Rust code review and the phased remediation |
| [Filtered DNS ramp plan](plans/filtered-ramp-plan.md) | Proposed. Regenerate before the next campaign | Client-event filter ramp. Its capture contract predates `e59425f`, `9172f2d`, and `b46da39` |
| [Parser and TCP threading analysis](plans/parser-tcp-threading-analysis.md) | Proposed | Parallel parsing and TCP threading. Its fail-fast contract, timed park, and fallback decoding are out of date |
| [Parallel parser workers plan](plans/parallel-parser-workers-plan.md) | Proposed. Partly executed | Diagnose the bottleneck and test parallel workers. P0 and diagnostic runs recorded Oct 31, 2026. P1a is next. P4 is deferred |

## History

These documents in [`history/`](history/) are dated records. Do not use their
commands as procedures. They contain recorded VM addresses, fixed directories,
and state-changing actions. Their status is Historical unless the table says
otherwise.

### Lab and validation records

| Document | Contents |
| --- | --- |
| [Lab validation](history/lab-validation.md) | Initial live receiver and packet-delivery checks, Oct 28, 2026. The LocalSystem profile |
| [LocalService restricted-SID validation](history/localservice-restricted-validation.md) | ETW rights the restricted token needs. The 0x20-only grant |
| [Skills evaluation lab validation](history/skills-evaluation-lab-validation.md) | Mutex removal, reclaim, DNS handle (denied), pinned SDDL, delayed-auto start, backup and restore, SCM recovery with a DNS restart |
| [Cleanup validation](history/cleanup-validation.md) | Gates per wave, lab run of the cleaned build, review results, open problems |
| [ETW buffer wake validation](history/buffer-wake-validation.md) | Cause of the 1 s burst loss, wake options compared, lab results for the per-buffer wake |
| [ETW flush removal validation](history/flush-removal-validation.md) | Lab results without the explicit flush at about 1,000 to 50,000 achieved QPS |
| [200,000 QPS validation](history/200k-qps-validation.md) | Two runs with 128 KiB buffers and a 262,144-slot channel |
| [Live load validation plan](history/live-load-validation-plan.md) | Stages L1 to L5. The pending L4 action was not done. Its premise that counters reach only stderr is out of date |

### Performance campaigns

| Document | Contents |
| --- | --- |
| [Performance test plan, Oct 28](history/performance-test-plan.md) | Method for the unpaced experiment |
| [Performance review, Oct 28](history/performance-review-2026-10-28.md) | Results of the unpaced three-run experiment |
| [Repeat at 1,000 QPS: plan](history/performance-1000qps-plan.md) | Method for the repeat with the original 1,024-entry ring |
| [Repeat at 1,000 QPS: review](history/performance-1000qps-review.md) | Results of the repeat |
| [Single-pass four-CPU test plan](history/singlepass-four-core-test-plan.md) | Plan for one 100,000 QPS run |
| [Single-pass four-CPU test results](history/singlepass-four-core-test-results.md) | Records were 0.43% short. Per-counter attribution was not recorded |
| [Filtered DNS ramp review](history/filtered-ramp-review.md) | Snapshot of an in-progress campaign. It does not establish a maximum rate |
| [Parser optimization DNS ramp plan](history/parser-ramp-plan.md) | Superseded. Aborted, do not resume |
| [Parser optimization DNS ramp review](history/parser-ramp-review.md) | Superseded. Partial record of the aborted ramp |

### Ring, intake, and event-loss work

| Document | Contents |
| --- | --- |
| [Ring-buffer capacity fix: plan](history/ring-buffer-fix-plan.md) | The capacity-only 8,192 candidate and its gates |
| [Ring-buffer capacity fix: validation](history/ring-buffer-fix-validation.md) | Superseded by [pooled intake](design/pooled-intake.md). Results of the `ArrayQueue` candidate |
| [Ring capacity experiment](history/ring-capacity-experiment.md) | The experiment that set the former 8,192 default (now 24,576) |
| [Rejected wake-on-enqueue experiment](history/ring-wakeup.md) | The `wake-1024-v1` candidate. It did not rule out a wake once per buffer |
| [Pooled intake: validation plan](history/pooled-intake-plan.md) | Original validation design and ownership record |
| [Pooled intake: audit plan](history/pooled-intake-audit-plan.md) | Audit of the pooled implementation |
| [Pooled intake: measurement review](history/pooled-intake-review-plan.md) | Review of the Python analysis and metrics accounting |
| [Event-loss solutioning timeline](history/diagrams/loss-solutioning.drawio) | Editable draw.io timeline from the 1,024-slot ring to the per-buffer parser wake. Animations are in [`assets/loss-solutioning/`](assets/loss-solutioning/) |
| [Packet-flow diagram](history/diagrams/packet-flow.drawio) | Editable draw.io diagram of the October 28 design |

### Parser, decoder, and profiling

| Document | Contents |
| --- | --- |
| [Parser and decoder performance review](history/parser-decoder-performance-review.md) | Candidate changes for the parser and decoder |
| [Parser and encoder optimization plan](history/parser-encoder-optimization-plan.md) | Superseded by [single-pass decoding](design/single-pass-decoder.md). Reusable packet-vector implementation |
| [Direct PacketData encoding plan](history/direct-packet-encoding-plan.md) | Superseded by [single-pass decoding](design/single-pass-decoder.md). Direct encoding without a packet vector |
| [Single-pass decoder: implementation plan](history/single-pass-decoder-plan.md) | Staged plan, gates, and completion record. The current contract is [Single-pass decoder](design/single-pass-decoder.md) |
| [TDH fallback removal](history/tdh-fallback-removal-plan.md) | Removal of the per-event TDH reader, executed in `ee94de6` |
| [Combined parser and writer WPR plan](history/parser-writer-wpr-plan.md) | WPR experiment. Its `/DEBUG:NONE` setting is in `.cargo/config.toml` |
| [Combined parser and writer WPR results](history/parser-writer-wpr-results.md) | CPU and blocking results |
| [Schema-cache lookup WPR comparison](history/schema-lookup-wpr-results.md) | Schema-cache comparison |
| [Remove the duplicate schema lookup](history/schema-single-lookup-plan.md) | Plan for a single schema lookup |
| [Symbolized release WPR: plan](history/symbolized-release-wpr-plan.md) | Staged procedure for a symbolized release profile |
| [Symbolized release WPR: results](history/symbolized-release-wpr-results.md) | Results of that profile |

### Service behavior and source reviews

| Document | Contents |
| --- | --- |
| [ETW subscription and intake filter plan](history/etw-subscription-filter-plan.md) | Narrower subscription and accepted event IDs |
| [ETW keyword narrowing plan](history/etw-keyword-narrowing-plan.md) | Keyword narrowing and its provider-metadata checks |
| [ETW session recovery and event-ID filter plan](history/etw-session-recovery-and-event-filter-plan.md) | Stale session reclaim and the event-ID filter |
| [Source review remediation plan](history/src-review-remediation-plan.md) | Remediation of selected findings from the `src` review |
| [Skills evaluation plan](history/skills-evaluation-plan.md) | Evaluation of code, build, CI, and service hardening. Results are in the [lab record](history/skills-evaluation-lab-validation.md) |
| [Obsolete and legacy code cleanup plan](history/cleanup-plan.md) | Removal of dead code, legacy names, and stale docs. Results are in the [validation record](history/cleanup-validation.md) |
| [Service shutdown fix plan](history/service-stop-fix-plan.md) | Superseded by [service shutdown remediation](guide/service-stop-remediation.md) |
| [Service shutdown validation plan](history/service-stop-validation-plan.md) | Bounded receiver-close comparison |
| [Service shutdown validation results](history/service-stop-validation-results.md) | Results of that comparison |

### Feature plans, active

| Document | Contents |
| --- | --- |
| [TCP sink spool plan](history/tcp-sink-spool-plan.md) | Disk spool for the TCP sink, with a sender thread: decisions, design, stage records, and the completion record. Active on the `tcp-sink-spool` branch. Not yet merged |

### Feature plans, executed

| Document | Contents |
| --- | --- |
| [TLS receiver plan](history/tls-receiver-plan.md) | TLS 1.2 and 1.3 for the receiver connection, with the decisions taken during implementation. Executed 2026-11-01 |
| [Windows V2 performance counter plan](history/windows-performance-counters-v2-plan.md) | The `EtwDnstap` counter set: catalog, manifest, registration, and suggested alerts. Lab-validated 2026-11-01 |
| [ETW minimum buffers plan](history/etw-min-buffers-plan.md) | The `--etw-min-buffers` option. Lab-validated 2026-11-01 |
| [Response failure capture plan](history/response-failure-capture-plan.md) | Capture of DNS Server event 258 (`RESPONSE_FAILURE`) as `CLIENT_RESPONSE`. Lab-validated 2026-11-01 |

### Notes on historical records

Records keep their recorded command text and source paths. Where a lab script
or source file moved, the record has a short note that names the new path. See
the
[script moves and renames](../scripts/performance/README.md#file-moves-and-renames)
for commit `f1475d8`. Commit `dc9d7e0` split the ETW code into `src/etw/`.

The 2026-11-03 history rewrite changed every commit hash from `253df79` (old
`8588619`) onward. The [commit hash mapping](history/commit-hash-mapping.md)
lists each old hash with the new one. Files under `datasets/` keep the old
hashes.

## Instrumentation compatibility note

- **Recorded binaries.** The historical campaigns used binaries with a service
  JSONL metrics writer. That writer was removed. The commands, ledgers, and
  overhead in those records apply to the recorded binaries only. The
  service-metrics harnesses reject new campaigns before any VM, load, or SCM
  change. Existing evidence and cleanup actions remain available.
- **Current telemetry.** The service writes its counters to the Application
  event log every `--stats-interval-seconds` and at shutdown. It also publishes
  them as [performance counters](guide/monitoring.md#performance-counters). It
  has no durable metrics export. The historical campaigns did not measure this
  telemetry. Measuring it needs a separately designed method.
- **Before another campaign,** design compatible telemetry, use a fresh backup,
  unique run IDs, verified guest identities, and fixture readiness.

## Supporting material

- [DNS workload provenance](../datasets/performance/README.md) recreates the
  pinned 5,000-name test workload.
- [DNS stress client](../tools/dns-stress/README.md) builds and runs the host
  load generator.
- `docs/assets/performance/` holds graph derivatives and CSV copies. Their
  source manifests are `data/source-manifest.json` and `plot-provenance.json`.
- Raw ETLs, receiver archives, build logs, and campaign receipts are under
  `target/`. That path is gitignored. It is a local artifact and is not in a
  fresh clone. A prose summary or a generated graph does not replace its source
  manifest, final ledger, or integrity check.
