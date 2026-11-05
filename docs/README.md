# Documentation index

> **Status:** Current · **Describes:** the repository at commit `58fde72` for the layout, conventions, and file list · **Reviewed:** 2026-11-03

This index lists every document in `docs/`. Each status block gives its status, evidence date, commit, and review date.

## Where to start

| You are... | Start with |
| --- | --- |
| An operator who installs or runs the service | [Project README](../README.md), then the [operator guides](#operator-guides) |
| A maintainer who changes the code | [Contributing](../CONTRIBUTING.md), then [Design and reference](#design-and-reference) |
| A reader who checks performance claims | [Performance](#performance) and [Lab and validation records](#lab-and-validation-records) |

## Layout

| Folder | Contents | Status |
| --- | --- | --- |
| [`guide/`](guide/) | Operator guides | Current |
| [`design/`](design/) | Architecture and reference | Current |
| [`performance/`](performance/) | Latest native performance record and its support | Current review. Supporting records are Historical |
| [`plans/`](plans/) | Plans and analyses not yet executed | Proposed |
| [`history/`](history/) | Finished campaigns, reviews, completed plans, superseded documents | Historical, Superseded |
| [`assets/performance/`](assets/performance/) | Graph SVGs, CSV copies, provenance manifests | Generated |
| [`assets/pipeline/`](assets/pipeline/) | Animated GIF of the data path | Current |
| [`assets/loss-solutioning/`](assets/loss-solutioning/) | Animated GIFs of the event-loss causes and fixes | Historical |

Graph assets stay in `docs/assets/performance/` because [`plot-pooled-wpr.py`](../scripts/performance/plot-pooled-wpr.py) writes there by default. The pooled plan, graphs, and trace analysis stay in `performance/` because [`build-pooled-report.py`](../scripts/performance/build-pooled-report.py) links to them.

Each Current document has one type: procedure, reference, diagnosis, or design. See [Document types](../CONTRIBUTING.md#document-types) and [Writing style](../CONTRIBUTING.md#writing-style).

## Status values

| Status | Meaning |
| --- | --- |
| **Current** | Describes the code and procedures on `main`. Kept up to date with the code. |
| **Historical** | A dated campaign, review, or plan. Its results and hashes apply to its recorded binaries and are not edited. |
| **Superseded** | Replaced by a later implementation or document. The status block links the replacement. |
| **Proposed** | A design or plan that is not implemented or not yet executed. |

A commit reference appears only where the document or the history identifies it. Otherwise the record says *not recorded*. History starts at `08254d4` (Initial Buildout). There are no release tags.

## Operator guides

The [`guide/`](guide/) documents are the source of truth for install, configuration, troubleshooting, and removal.

| Task | Document |
| --- | --- |
| Build the executable | [Build the executable](guide/build.md) |
| Build the MSI installer | [Build the MSI](guide/build.md#build-the-msi) |
| Install and register the service | [Install and provision](guide/install.md) |
| Install with the MSI | [Install with the MSI](guide/install.md#install-with-the-msi) |
| Prepare the receiver and its certificate | [Set up the dnstap receiver](guide/receiver.md) |
| Look up an option, default, or range | [Configuration reference](guide/configuration.md) |
| Choose settings for a high query rate (defaults suit 10,000 queries per second) | [Tune capture for load](guide/tuning.md) |
| Check the spool encryption key and the service dependencies | [Spool encryption key](guide/install.md#spool-encryption-key) |
| Fix `spool key is not usable` or force a new spool key | [Spool key errors](guide/troubleshooting.md#spool-key-errors) |
| Read the log events and counters | [Log events and counters](guide/monitoring.md) |
| Fix a failed start, drops, or missing records | [Troubleshooting](guide/troubleshooting.md) |
| Diagnose receiver resets during service stop | [Service shutdown remediation](guide/service-stop-remediation.md) |
| Change or upgrade an installed service | [Repair or upgrade](guide/upgrade.md) |
| Upgrade, repair, or change settings of an MSI install | [Upgrade, repair, or change settings with the MSI](guide/upgrade.md#upgrade-repair-or-change-settings-with-the-msi) |
| Remove the service and its leftovers | [Uninstall](guide/uninstall.md) |
| Remove an MSI install | [Uninstall with the MSI](guide/uninstall.md#uninstall-with-the-msi) |

Policy documents in the repository root:

| Document | Contents |
| --- | --- |
| [Security policy](../SECURITY.md) | Reporting, threat model, producer identity, LocalService profile |
| [Contributing](../CONTRIBUTING.md) | Build, test, lint, documentation conventions, commit style |
| [Changelog](../CHANGELOG.md) | Keep a Changelog format and versioning policy. No tag yet |

## Design and reference

Maintainer documents in [`design/`](design/):

| Document | Contents |
| --- | --- |
| [Data path](design/data-path.md) | Five stages from ETW event to TCP frame, with threads and loss counters |
| [Pooled ETW intake](design/pooled-intake.md) | Ownership, memory budget, parser wake, loss accounting |
| [Ring capacity and shutdown](design/ring-buffer.md) | Channel bounds, parser wake, delivery batches, shutdown order |
| [Single-pass decoder](design/single-pass-decoder.md) | Decoder grammar, schema rejection rule, bounds |
| [Measurement terms and result gates](design/measurement-terms.md) | Units, counters, and gates for performance reports |
| [Code-architecture diagram](design/diagrams/code-architecture.drawio) | Editable architecture diagram. See the [diagram note](#diagram-note) |

### Diagram note

The labels match `main` at `a2ae289`, and the layout is from October 28, 2026. It is out of date. It omits the single-pass decoder (`etw/decode.rs`, `ee94de6`), the parser wake callback (`7f91264`), TLS in `network.rs`, `QueuedSink` and `dnstap-sender` (`sender.rs`), and the disk spool (`src/spool/`). Its supervisor box lists "flush", but the parser thread flushes. For the current design, use [Data path](design/data-path.md), [Pooled ETW intake](design/pooled-intake.md), and [Single-pass decoder](design/single-pass-decoder.md).

## Performance

[`performance/`](performance/) records the latest completed native campaign, `pooled-vm-20261029-0942`, which ran under LocalSystem, not the LocalService restricted-SID profile.

| Document | Contents |
| --- | --- |
| [Pooled intake VM performance review](performance/pooled-vm-performance-review.md) | Results: 22 stages, 12 strict passes, 10 strict failures |
| [Pooled VM CPU and QPS graphs](performance/pooled-vm-performance-graphs.md) | Graphs of the four primary runs |
| [Pooled VM trace analysis](performance/pooled-vm-trace-analysis.md) | Native WPR evidence |
| [Pooled intake VM performance plan](performance/pooled-vm-performance-plan.md) | Workload, commands, and approvals |

The campaign directory `target/performance/pooled-vm-20261029-0942` is a local artifact, not in a clone. [`build-pooled-report.py`](../scripts/performance/build-pooled-report.py) generates the review, and [`plot-pooled-wpr.py`](../scripts/performance/plot-pooled-wpr.py) generates the graphs. To change the narrative, edit the script's template and regenerate. Do not edit the numbers. See [scripts/performance/README.md](../scripts/performance/README.md).

## Plans

Plans in [`plans/`](plans/) are not executed, or only partly. A plan moves to `history/` when it is executed and merged.

| Document | Contents |
| --- | --- |
| [Spool encryption plan](plans/spool-encryption-plan.md) | AES-128-GCM spool records, the CNG-wrapped key, and the key held in memory. Implemented on the `spool-encryption` branch; lab run open |
| [Filtered DNS ramp plan](plans/filtered-ramp-plan.md) | Client-event filter ramp. Regenerate before the next campaign |
| [Parser and TCP threading analysis](plans/parser-tcp-threading-analysis.md) | Parallel parsing and TCP threading. Partly out of date |
| [Parallel parser workers plan](plans/parallel-parser-workers-plan.md) | Bottleneck diagnosis and parallel workers. Partly executed |

## History

Dated records in [`history/`](history/). Do not use their commands as procedures: they contain recorded VM addresses, fixed directories, and state-changing actions. Status is Historical unless the table says otherwise.

### Lab and validation records

| Document | Contents |
| --- | --- |
| [Lab validation](history/lab-validation.md) | First live receiver checks, Oct 28, 2026 (LocalSystem) |
| [LocalService restricted-SID validation](history/localservice-restricted-validation.md) | ETW rights the restricted token needs |
| [Skills evaluation lab validation](history/skills-evaluation-lab-validation.md) | Lab checks: reclaim, pinned SDDL, backup, SCM recovery |
| [Cleanup validation](history/cleanup-validation.md) | Gates per wave, cleaned-build lab run, review results |
| [ETW buffer wake validation](history/buffer-wake-validation.md) | Cause of the 1 s burst loss; per-buffer wake results |
| [ETW flush removal validation](history/flush-removal-validation.md) | Results without the explicit flush |
| [200,000 QPS validation](history/200k-qps-validation.md) | Two runs with 128 KiB buffers and a 262,144-slot channel |
| [Guide lab results, 2026-11](history/guide-lab-results-2026-11.md) | Lab results stated in the guides before the trim: 10,000 and 200,000 QPS, flush-era loss |
| [TCP sink spool lab validation](history/tcp-sink-spool-lab-validation.md) | Receiver outages, restart with leftovers, spool grant |
| [Live load validation plan](history/live-load-validation-plan.md) | Stages L1 to L5. L4 not done |

### Performance campaigns

| Document | Contents |
| --- | --- |
| [Performance test plan, Oct 28](history/performance-test-plan.md) | Method for the unpaced experiment |
| [Performance review, Oct 28](history/performance-review-2026-10-28.md) | Results of the unpaced three-run experiment |
| [Repeat at 1,000 QPS: plan](history/performance-1000qps-plan.md) | Method for the 1,000 QPS repeat |
| [Repeat at 1,000 QPS: review](history/performance-1000qps-review.md) | Results of the 1,000 QPS repeat |
| [Single-pass four-CPU test plan](history/singlepass-four-core-test-plan.md) | Plan for one 100,000 QPS run |
| [Single-pass four-CPU test results](history/singlepass-four-core-test-results.md) | Records 0.43% short. No per-counter attribution |
| [Filtered DNS ramp review](history/filtered-ramp-review.md) | Snapshot of an in-progress campaign. No maximum rate |
| [Parser optimization DNS ramp plan](history/parser-ramp-plan.md) | Superseded. Aborted, do not resume |
| [Parser optimization DNS ramp review](history/parser-ramp-review.md) | Superseded. Partial record of the aborted ramp |

### Ring, intake, and event-loss work

| Document | Contents |
| --- | --- |
| [Ring-buffer capacity fix: plan](history/ring-buffer-fix-plan.md) | Capacity-only 8,192 candidate and its gates |
| [Ring-buffer capacity fix: validation](history/ring-buffer-fix-validation.md) | Superseded by pooled intake. ArrayQueue results |
| [Ring capacity experiment](history/ring-capacity-experiment.md) | Set the former 8,192 default (now 131,072) |
| [Rejected wake-on-enqueue experiment](history/ring-wakeup.md) | Rejected wake-on-enqueue candidate |
| [Pooled intake: validation plan](history/pooled-intake-plan.md) | Original validation design and ownership record |
| [Pooled intake: audit plan](history/pooled-intake-audit-plan.md) | Audit of the pooled implementation |
| [Pooled intake: measurement review](history/pooled-intake-review-plan.md) | Review of the Python analysis and metrics |
| [Event-loss solutioning timeline](history/diagrams/loss-solutioning.drawio) | Editable loss-solutioning timeline |
| [Packet-flow diagram](history/diagrams/packet-flow.drawio) | Editable packet-flow diagram of Oct 28 design |

### Parser, decoder, and profiling

| Document | Contents |
| --- | --- |
| [Parser and decoder performance review](history/parser-decoder-performance-review.md) | Candidate parser and decoder changes |
| [Parser and encoder optimization plan](history/parser-encoder-optimization-plan.md) | Superseded by single-pass decoding |
| [Direct PacketData encoding plan](history/direct-packet-encoding-plan.md) | Superseded by single-pass decoding |
| [Single-pass decoder: implementation plan](history/single-pass-decoder-plan.md) | Staged plan, gates, and completion record |
| [TDH fallback removal](history/tdh-fallback-removal-plan.md) | Removal of the per-event TDH reader (`ee94de6`) |
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
| [ETW keyword narrowing plan](history/etw-keyword-narrowing-plan.md) | Keyword narrowing and provider-metadata checks |
| [ETW session recovery and event-ID filter plan](history/etw-session-recovery-and-event-filter-plan.md) | Stale session reclaim and event-ID filter |
| [Source review remediation plan](history/src-review-remediation-plan.md) | Selected findings from the `src` review |
| [Skills evaluation plan](history/skills-evaluation-plan.md) | Code, build, CI, and service hardening evaluation |
| [Obsolete and legacy code cleanup plan](history/cleanup-plan.md) | Removal of dead code, legacy names, and stale docs |
| [Service shutdown fix plan](history/service-stop-fix-plan.md) | Superseded by service shutdown remediation |
| [Service shutdown validation plan](history/service-stop-validation-plan.md) | Bounded receiver-close comparison |
| [Service shutdown validation results](history/service-stop-validation-results.md) | Results of that comparison |

### Feature plans, executed

| Document | Contents |
| --- | --- |
| [No console output plan](history/no-console-output-plan.md) | Removal of all console output. Lab-validated 2026-11-03 |
| [MSI installer plan](history/msi-installer-plan.md) | MSI package, provisioning script, lab results |
| [Code review findings fix plan](history/code-review-findings-fix-plan.md) | Fixes for findings F1 to F12. MSI lab validation open |
| [Code review remediation plan](history/code-review-remediation-plan.md) | Findings of the 2026-11-01 review. Merged to `main` |
| [TCP sink spool plan](history/tcp-sink-spool-plan.md) | Disk spool for the TCP sink. Lab-validated 2026-11-03 |
| [TLS receiver plan](history/tls-receiver-plan.md) | TLS 1.2 and 1.3 for the receiver connection |
| [Windows V2 performance counter plan](history/windows-performance-counters-v2-plan.md) | The `EtwDnstap` counter set. Lab-validated 2026-11-01 |
| [ETW minimum buffers plan](history/etw-min-buffers-plan.md) | The `--etw-min-buffers` option. Lab-validated 2026-11-01 |
| [Response failure capture plan](history/response-failure-capture-plan.md) | Event 258 capture. Lab-validated 2026-11-01 |

### Notes on historical records

Records keep their command text and source paths. Where a file moved, the record has a note that names the new path. See [script moves and renames](../scripts/performance/README.md#file-moves-and-renames) for commit `f1475d8`. Commit `dc9d7e0` split the ETW code into `src/etw/`.

The 2026-11-03 history rewrite changed every commit hash from `253df79` (old `8588619`) onward. The [commit hash mapping](history/commit-hash-mapping.md) lists old and new hashes. Files under `datasets/` keep the old hashes.

## Instrumentation compatibility note

- **Recorded binaries.** The historical campaigns used binaries with a service JSONL metrics writer, since removed. Their commands, ledgers, and overhead apply to those binaries only. The service-metrics harnesses reject new campaigns before any VM, load, or SCM change.
- **Current telemetry.** The service writes its counters to the Application event log every `--stats-interval-seconds` and at shutdown, and publishes them as [performance counters](guide/monitoring.md#performance-counters). It has no durable metrics export. The historical campaigns did not measure this telemetry, and measuring it needs a separately designed method.
- **Before another campaign,** design compatible telemetry, and use a fresh backup, unique run IDs, verified guest identities, and fixture readiness.

## Supporting material

- [DNS workload provenance](../datasets/performance/README.md) recreates the pinned 5,000-name test workload.
- [DNS stress client](../tools/dns-stress/README.md) builds and runs the host load generator.
- `docs/assets/performance/` holds graph derivatives and CSV copies, with source manifests `data/source-manifest.json` and `plot-provenance.json`.
- Raw ETLs, receiver archives, build logs, and campaign receipts are under `target/`. That path is gitignored and not in a fresh clone. A prose summary or a generated graph does not replace its source manifest, final ledger, or integrity check.
