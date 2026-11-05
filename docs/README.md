# Documentation index

> **Status:** Current · **Describes:** the `spool-encryption` branch for the layout, conventions, and file list · **Reviewed:** 2026-11-05

### Quick Start

- Operators: [project README](../README.md), then [operator guides](#operator-guides).
- Maintainers: [Contributing](../CONTRIBUTING.md), then [design and reference](#design-and-reference).
- Performance claims: [Performance](#performance), then [Lab and validation records](#lab-and-validation-records).

### Steps / Configuration

Every Markdown and diagram file in `docs/` is listed, except `docs/assets/`.

#### Layout

| Folder | Contents | Status |
| --- | --- | --- |
| [`guide/`](guide/) | Guides | Current |
| [`design/`](design/) | Design | Current |
| [`performance/`](performance/) | Latest native record | Current review, Historical support |
| [`plans/`](plans/) | Unexecuted plans | Proposed, Approved, In progress |
| [`history/`](history/) | Finished and superseded work | Historical, Superseded |
| [`assets/performance/`](assets/performance/) | Graphs, CSVs, manifests | Generated |
| [`assets/pipeline/`](assets/pipeline/) | Data-path GIF | Current |
| [`assets/loss-solutioning/`](assets/loss-solutioning/) | Event-loss GIFs | Historical |

Generator scripts write to `assets/performance/` and link to `performance/`, so those files stay there. See [Document types](../CONTRIBUTING.md#document-types), [Writing style](../CONTRIBUTING.md#writing-style).

#### Status values

- **Current:** matches the code and procedures at its named commit. Kept up to date.
- **Historical:** dated campaign, review, or plan. Results apply to its recorded binaries. Not edited.
- **Superseded:** replaced by a later document. The status block links the replacement.
- **Proposed:** not yet implemented or executed.

A record without a known commit says *not recorded*. History starts at `08254d4`. There are no release tags.

#### Operator guides

- [Build the executable](guide/build.md), [Build the MSI](guide/build.md#build-the-msi)
- [Install and provision](guide/install.md), [Install with the MSI](guide/install.md#install-with-the-msi)
- [Set up the dnstap receiver](guide/receiver.md)
- [Configuration reference](guide/configuration.md)
- [Tune capture for load](guide/tuning.md)
- [Spool encryption key](guide/install.md#spool-encryption-key)
- [Spool key errors](guide/troubleshooting.md#spool-key-errors)
- [Log events and counters](guide/monitoring.md)
- [Troubleshooting](guide/troubleshooting.md)
- [Service shutdown remediation](guide/service-stop-remediation.md)
- [Repair or upgrade](guide/upgrade.md), [MSI upgrade, repair, or settings change](guide/upgrade.md#upgrade-repair-or-change-settings-with-the-msi)
- [Uninstall](guide/uninstall.md), [Uninstall with the MSI](guide/uninstall.md#uninstall-with-the-msi)
- Root: [Security policy](../SECURITY.md), [Contributing](../CONTRIBUTING.md), [Changelog](../CHANGELOG.md)

#### Design and reference

- [Data path](design/data-path.md)
- [Pooled ETW intake](design/pooled-intake.md)
- [Ring capacity and shutdown](design/ring-buffer.md)
- [Single-pass decoder](design/single-pass-decoder.md)
- [Measurement terms and result gates](design/measurement-terms.md)
- [Slow and saturated receiver](design/slow-receiver.md)
- [Code-architecture diagram](design/diagrams/code-architecture.drawio): editable, out of date (see [Common Gotchas](#common-gotchas))

#### Performance

[`performance/`](performance/) records the latest completed native campaign, `pooled-vm-20261029-0942`. It ran under LocalSystem, not the LocalService restricted-SID profile.

- [Pooled intake VM performance review](performance/pooled-vm-performance-review.md)
- [Pooled VM CPU and QPS graphs](performance/pooled-vm-performance-graphs.md)
- [Pooled VM trace analysis](performance/pooled-vm-trace-analysis.md)
- [Pooled intake VM performance plan](performance/pooled-vm-performance-plan.md)

`target/performance/pooled-vm-20261029-0942` is a local artifact. `build-pooled-report.py` and `plot-pooled-wpr.py` generate the review and graphs: edit the script template and regenerate ([scripts/performance/README.md](../scripts/performance/README.md)).

#### Plans

A plan moves to `history/` when it's executed and merged.

- [Spool encryption plan](plans/spool-encryption-plan.md): implemented on `spool-encryption` ([validation](history/spool-encryption-validation.md))
- [Slow and saturated receiver plan](plans/slow-receiver-plan.md): in progress, findings L-7 and L-8 open
- [Filtered DNS ramp plan](plans/filtered-ramp-plan.md): regenerate before the next campaign
- [Parser and TCP threading analysis](plans/parser-tcp-threading-analysis.md)
- [Parallel parser workers plan](plans/parallel-parser-workers-plan.md)

#### Lab and validation records

- [Lab validation](history/lab-validation.md)
- [LocalService restricted-SID validation](history/localservice-restricted-validation.md)
- [Skills evaluation lab validation](history/skills-evaluation-lab-validation.md)
- [Cleanup validation](history/cleanup-validation.md)
- [ETW buffer wake validation](history/buffer-wake-validation.md)
- [ETW flush removal validation](history/flush-removal-validation.md)
- [200,000 QPS validation](history/200k-qps-validation.md)
- [Guide lab results, 2026-11](history/guide-lab-results-2026-11.md)
- [TCP sink spool lab validation](history/tcp-sink-spool-lab-validation.md)
- [Spool encryption lab validation](history/spool-encryption-validation.md)
- [Slow receiver validation](history/slow-receiver-validation.md)
- [Live load validation plan](history/live-load-validation-plan.md): stages L1 to L5, L4 not done

#### Performance campaigns

- [Performance test plan, Oct 28](history/performance-test-plan.md), [review](history/performance-review-2026-10-28.md)
- [Repeat at 1,000 QPS: plan](history/performance-1000qps-plan.md), [review](history/performance-1000qps-review.md)
- [Single-pass four-CPU test plan](history/singlepass-four-core-test-plan.md)
- [Single-pass four-CPU test results](history/singlepass-four-core-test-results.md)
- [Filtered DNS ramp review](history/filtered-ramp-review.md): in-progress snapshot, no maximum rate
- [Parser optimization DNS ramp plan](history/parser-ramp-plan.md): Superseded, aborted
- [Parser optimization DNS ramp review](history/parser-ramp-review.md): Superseded, partial record

#### Ring, intake, and event-loss work

- [Ring-buffer capacity fix: plan](history/ring-buffer-fix-plan.md)
- [Ring-buffer capacity fix: validation](history/ring-buffer-fix-validation.md): Superseded, ArrayQueue results
- [Ring capacity experiment](history/ring-capacity-experiment.md)
- [Rejected wake-on-enqueue experiment](history/ring-wakeup.md)
- Pooled intake: [validation plan](history/pooled-intake-plan.md), [audit plan](history/pooled-intake-audit-plan.md), [measurement review](history/pooled-intake-review-plan.md)
- Diagrams: [event-loss solutioning timeline](history/diagrams/loss-solutioning.drawio), [packet flow](history/diagrams/packet-flow.drawio)

#### Parser, decoder, and profiling

- [Parser and decoder performance review](history/parser-decoder-performance-review.md)
- [Parser and encoder optimization plan](history/parser-encoder-optimization-plan.md): Superseded by single-pass decoding
- [Direct PacketData encoding plan](history/direct-packet-encoding-plan.md): Superseded by single-pass decoding
- [Single-pass decoder: implementation plan](history/single-pass-decoder-plan.md)
- [TDH fallback removal](history/tdh-fallback-removal-plan.md)
- [Combined parser and writer WPR plan](history/parser-writer-wpr-plan.md), [results](history/parser-writer-wpr-results.md)
- [Schema-cache lookup WPR comparison](history/schema-lookup-wpr-results.md)
- [Remove the duplicate schema lookup](history/schema-single-lookup-plan.md)
- [Symbolized release WPR: plan](history/symbolized-release-wpr-plan.md), [results](history/symbolized-release-wpr-results.md)

#### Service behavior and source reviews

- [ETW subscription and intake filter plan](history/etw-subscription-filter-plan.md)
- [ETW keyword narrowing plan](history/etw-keyword-narrowing-plan.md)
- [ETW session recovery and event-ID filter plan](history/etw-session-recovery-and-event-filter-plan.md)
- [Source review remediation plan](history/src-review-remediation-plan.md)
- [Skills evaluation plan](history/skills-evaluation-plan.md)
- [Obsolete and legacy code cleanup plan](history/cleanup-plan.md)
- [Service shutdown fix plan](history/service-stop-fix-plan.md): Superseded by service shutdown remediation
- Service shutdown validation: [plan](history/service-stop-validation-plan.md), [results](history/service-stop-validation-results.md)

#### Feature plans, executed

- [No console output plan](history/no-console-output-plan.md), lab-validated 2026-11-03
- [MSI installer plan](history/msi-installer-plan.md)
- [Code review findings fix plan](history/code-review-findings-fix-plan.md): F1 to F12, MSI lab validation open
- [Code review remediation plan](history/code-review-remediation-plan.md): 2026-11-01 findings, merged
- [TCP sink spool plan](history/tcp-sink-spool-plan.md), lab-validated 2026-11-03
- [TLS receiver plan](history/tls-receiver-plan.md)
- [Windows V2 performance counter plan](history/windows-performance-counters-v2-plan.md), lab-validated 2026-11-01
- [ETW minimum buffers plan](history/etw-min-buffers-plan.md): `--etw-min-buffers`, lab-validated 2026-11-01
- [Response failure capture plan](history/response-failure-capture-plan.md): event 258, lab-validated 2026-11-01

#### Supporting material

- [DNS workload provenance](../datasets/performance/README.md)
- [DNS stress client](../tools/dns-stress/README.md)
- `docs/assets/performance/`: graphs, CSV copies, `data/source-manifest.json`, `plot-provenance.json`

### Common Gotchas

- **Don't run commands from `history/`.** They contain lab addresses and state-changing actions.
- **Records keep their command text and source paths.** A moved file is named by its new path: [script moves and renames](../scripts/performance/README.md#file-moves-and-renames).
- **Commit hashes changed.** The 2026-11-03 history rewrite changed hashes from `253df79` (old `8588619`) onward. See the [commit hash mapping](history/commit-hash-mapping.md). Files under `datasets/` keep the old hashes.
- **The code-architecture diagram is out of date.** It dates from October 28, 2026 and omits the single-pass decoder (`src/etw/decode.rs`), the parser wake callback, TLS in `network.rs`, `QueuedSink` and `dnstap-sender` (`sender.rs`), and the disk spool (`src/spool/`). Use [Data path](design/data-path.md), [Pooled ETW intake](design/pooled-intake.md), and [Single-pass decoder](design/single-pass-decoder.md).
- **Raw ETLs, receipts, and build logs live under the gitignored `target/`.** A summary or graph doesn't replace its source manifest, final ledger, or integrity check.

#### Instrumentation compatibility note

- **Recorded binaries:** historical campaigns used a service JSONL metrics writer the current service lacks. Their commands, ledgers, and overhead apply to those binaries only. The service-metrics harnesses reject new campaigns before any VM, load, or SCM change.
- **Current telemetry.** Counters go to the Application event log every `--stats-interval-seconds` and at shutdown, and to [performance counters](guide/monitoring.md#performance-counters). The historical campaigns didn't measure them.
- **Before another campaign,** design compatible telemetry, and use a fresh backup, unique run IDs, verified guest identities, and fixture readiness.
