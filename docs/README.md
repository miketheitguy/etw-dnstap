# Documentation index

> **Status:** Current · **Describes:** the `spool-encryption` branch for the layout, conventions, and file list · **Reviewed:** 2026-11-05

This index lists every Markdown and diagram file in `docs/`, except `docs/assets/`.

## Where to start

Operators: [project README](../README.md), then [operator guides](#operator-guides). Maintainers: [Contributing](../CONTRIBUTING.md), then [Design and reference](#design-and-reference). Performance claims: [Performance](#performance) and [Lab and validation records](#lab-and-validation-records).

## Layout

| Folder | Contents | Status |
| --- | --- | --- |
| [`guide/`](guide/) | Operator guides | Current |
| [`design/`](design/) | Architecture and reference | Current |
| [`performance/`](performance/) | Latest native performance record | Current review, Historical support |
| [`plans/`](plans/) | Plans not executed, or only partly | Proposed, Approved, In progress |
| [`history/`](history/) | Finished campaigns, reviews, completed and superseded documents | Historical, Superseded |
| [`assets/performance/`](assets/performance/) | Graph SVGs, CSV copies, manifests | Generated |
| [`assets/pipeline/`](assets/pipeline/) | Animated GIF of the data path | Current |
| [`assets/loss-solutioning/`](assets/loss-solutioning/) | Animated GIFs of event-loss causes and fixes | Historical |

Generator scripts write to `assets/performance/` and link to `performance/`, so those files stay there. Document types are in [Document types](../CONTRIBUTING.md#document-types) and [Writing style](../CONTRIBUTING.md#writing-style).

## Status values

| Status | Meaning |
| --- | --- |
| **Current** | Describes the code and procedures at the commit it names. Kept up to date. |
| **Historical** | A dated campaign, review, or plan. Its results apply to its recorded binaries and are not edited. |
| **Superseded** | Replaced by a later document. The status block links the replacement. |
| **Proposed** | A design or plan not yet implemented or executed. |

A record without a known commit says *not recorded*. History starts at `08254d4`. There are no release tags.

## Operator guides

- Build the executable: [Build the executable](guide/build.md)
- Build the MSI installer: [Build the MSI](guide/build.md#build-the-msi)
- Install and register the service: [Install and provision](guide/install.md)
- Install with the MSI: [Install with the MSI](guide/install.md#install-with-the-msi)
- Prepare the receiver and certificate: [Set up the dnstap receiver](guide/receiver.md)
- Look up an option, default, or range: [Configuration reference](guide/configuration.md)
- Choose settings for a high query rate: [Tune capture for load](guide/tuning.md)
- Check the spool key and service dependencies: [Spool encryption key](guide/install.md#spool-encryption-key)
- Fix `spool key is not usable` or force a new key: [Spool key errors](guide/troubleshooting.md#spool-key-errors)
- Read log events and counters: [Log events and counters](guide/monitoring.md)
- Fix a failed start, drops, or missing records: [Troubleshooting](guide/troubleshooting.md)
- Diagnose receiver resets during service stop: [Service shutdown remediation](guide/service-stop-remediation.md)
- Change or upgrade an installed service: [Repair or upgrade](guide/upgrade.md)
- Upgrade or repair an MSI install: [Upgrade, repair, or change settings with the MSI](guide/upgrade.md#upgrade-repair-or-change-settings-with-the-msi)
- Remove the service: [Uninstall](guide/uninstall.md)
- Remove an MSI install: [Uninstall with the MSI](guide/uninstall.md#uninstall-with-the-msi)

Root documents:

- [Security policy](../SECURITY.md)
- [Contributing](../CONTRIBUTING.md)
- [Changelog](../CHANGELOG.md)

## Design and reference

- [Data path](design/data-path.md). Five stages from ETW event to TCP frame
- [Pooled ETW intake](design/pooled-intake.md). Ownership, memory budget, parser wake, loss accounting
- [Ring capacity and shutdown](design/ring-buffer.md). Channel bounds, delivery batches, shutdown order
- [Single-pass decoder](design/single-pass-decoder.md). Decoder grammar, schema rejection rule, bounds
- [Measurement terms and result gates](design/measurement-terms.md). Units, counters, and gates for performance reports
- [Slow and saturated receiver](design/slow-receiver.md). Routing and link design for a slow receiver
- [Code-architecture diagram](design/diagrams/code-architecture.drawio). Editable diagram. See the [diagram note](#diagram-note)

### Diagram note

The diagram dates from October 28, 2026 and is out of date. It omits the single-pass decoder (`src/etw/decode.rs`), the parser wake callback, TLS in `network.rs`, `QueuedSink` and `dnstap-sender` (`sender.rs`), and the disk spool (`src/spool/`). Use [Data path](design/data-path.md), [Pooled ETW intake](design/pooled-intake.md), and [Single-pass decoder](design/single-pass-decoder.md) for the current design.

## Performance

[`performance/`](performance/) records the latest completed native campaign, `pooled-vm-20261029-0942`. It ran under LocalSystem, not the LocalService restricted-SID profile.

| Document | Contents |
| --- | --- |
| [Pooled intake VM performance review](performance/pooled-vm-performance-review.md) | Results: 22 stages, 12 strict passes, 10 strict failures |
| [Pooled VM CPU and QPS graphs](performance/pooled-vm-performance-graphs.md) | Graphs of the four primary runs |
| [Pooled VM trace analysis](performance/pooled-vm-trace-analysis.md) | Native WPR evidence |
| [Pooled intake VM performance plan](performance/pooled-vm-performance-plan.md) | Workload, commands, and approvals |

`target/performance/pooled-vm-20261029-0942` is a local artifact. `build-pooled-report.py` and `plot-pooled-wpr.py` generate the review and graphs: edit the script template and regenerate. See [scripts/performance/README.md](../scripts/performance/README.md).

## Plans

A plan moves to `history/` when it is executed and merged.

- [Spool encryption plan](plans/spool-encryption-plan.md). Encrypted spool records and CNG-wrapped key. Implemented on `spool-encryption` ([validation](history/spool-encryption-validation.md))
- [Slow and saturated receiver plan](plans/slow-receiver-plan.md). In progress. Findings L-7 and L-8 open
- [Filtered DNS ramp plan](plans/filtered-ramp-plan.md). Client-event filter ramp. Regenerate before the next campaign
- [Parser and TCP threading analysis](plans/parser-tcp-threading-analysis.md)
- [Parallel parser workers plan](plans/parallel-parser-workers-plan.md)

## History

Dated records in [`history/`](history/). Their commands contain lab addresses and state-changing actions. Do not use them as procedures.

### Lab and validation records

- [Lab validation](history/lab-validation.md). First live receiver checks, 2026-10-28
- [LocalService restricted-SID validation](history/localservice-restricted-validation.md). ETW rights the restricted token needs
- [Skills evaluation lab validation](history/skills-evaluation-lab-validation.md). Reclaim, pinned SDDL, backup, SCM recovery
- [Cleanup validation](history/cleanup-validation.md). Gates per wave, cleaned-build lab run
- [ETW buffer wake validation](history/buffer-wake-validation.md). Cause of the 1 s burst loss
- [ETW flush removal validation](history/flush-removal-validation.md). Results without the explicit flush
- [200,000 QPS validation](history/200k-qps-validation.md). Two runs at 128 KiB buffers
- [Guide lab results, 2026-11](history/guide-lab-results-2026-11.md). Lab results moved out of the guides
- [TCP sink spool lab validation](history/tcp-sink-spool-lab-validation.md). Receiver outages, restart with leftovers
- [Spool encryption lab validation](history/spool-encryption-validation.md). Encrypted spool at 10,000 QPS, 2026-11-04
- [Slow receiver validation](history/slow-receiver-validation.md). Before and after runs, 2026-11-04
- [Live load validation plan](history/live-load-validation-plan.md). Stages L1 to L5. L4 not done

### Performance campaigns

- [Performance test plan, Oct 28](history/performance-test-plan.md)
- [Performance review, Oct 28](history/performance-review-2026-10-28.md)
- [Repeat at 1,000 QPS: plan](history/performance-1000qps-plan.md)
- [Repeat at 1,000 QPS: review](history/performance-1000qps-review.md)
- [Single-pass four-CPU test plan](history/singlepass-four-core-test-plan.md). One 100,000 QPS run
- [Single-pass four-CPU test results](history/singlepass-four-core-test-results.md). 0.43% short, no per-counter attribution
- [Filtered DNS ramp review](history/filtered-ramp-review.md). In-progress snapshot, no maximum rate
- [Parser optimization DNS ramp plan](history/parser-ramp-plan.md). Superseded, aborted
- [Parser optimization DNS ramp review](history/parser-ramp-review.md). Superseded, partial record

### Ring, intake, and event-loss work

- [Ring-buffer capacity fix: plan](history/ring-buffer-fix-plan.md)
- [Ring-buffer capacity fix: validation](history/ring-buffer-fix-validation.md). Superseded. ArrayQueue results
- [Ring capacity experiment](history/ring-capacity-experiment.md)
- [Rejected wake-on-enqueue experiment](history/ring-wakeup.md)
- [Pooled intake: validation plan](history/pooled-intake-plan.md)
- [Pooled intake: audit plan](history/pooled-intake-audit-plan.md)
- [Pooled intake: measurement review](history/pooled-intake-review-plan.md)
- [Event-loss solutioning timeline](history/diagrams/loss-solutioning.drawio)
- [Packet-flow diagram](history/diagrams/packet-flow.drawio)

### Parser, decoder, and profiling

- [Parser and decoder performance review](history/parser-decoder-performance-review.md)
- [Parser and encoder optimization plan](history/parser-encoder-optimization-plan.md). Superseded by single-pass decoding
- [Direct PacketData encoding plan](history/direct-packet-encoding-plan.md). Superseded by single-pass decoding
- [Single-pass decoder: implementation plan](history/single-pass-decoder-plan.md)
- [TDH fallback removal](history/tdh-fallback-removal-plan.md)
- [Combined parser and writer WPR plan](history/parser-writer-wpr-plan.md)
- [Combined parser and writer WPR results](history/parser-writer-wpr-results.md)
- [Schema-cache lookup WPR comparison](history/schema-lookup-wpr-results.md)
- [Remove the duplicate schema lookup](history/schema-single-lookup-plan.md)
- [Symbolized release WPR: plan](history/symbolized-release-wpr-plan.md)
- [Symbolized release WPR: results](history/symbolized-release-wpr-results.md)

### Service behavior and source reviews

- [ETW subscription and intake filter plan](history/etw-subscription-filter-plan.md). Narrower subscription, accepted event IDs
- [ETW keyword narrowing plan](history/etw-keyword-narrowing-plan.md). Keyword narrowing, provider-metadata checks
- [ETW session recovery and event-ID filter plan](history/etw-session-recovery-and-event-filter-plan.md). Stale session reclaim, event-ID filter
- [Source review remediation plan](history/src-review-remediation-plan.md). Selected `src` review findings
- [Skills evaluation plan](history/skills-evaluation-plan.md). Code, build, CI, and service hardening evaluation
- [Obsolete and legacy code cleanup plan](history/cleanup-plan.md). Dead code, legacy names, stale docs
- [Service shutdown fix plan](history/service-stop-fix-plan.md). Superseded by service shutdown remediation
- [Service shutdown validation plan](history/service-stop-validation-plan.md)
- [Service shutdown validation results](history/service-stop-validation-results.md)

### Feature plans, executed

- [No console output plan](history/no-console-output-plan.md). Lab-validated 2026-11-03
- [MSI installer plan](history/msi-installer-plan.md). MSI package, provisioning script, lab results
- [Code review findings fix plan](history/code-review-findings-fix-plan.md). Findings F1 to F12. MSI lab validation open
- [Code review remediation plan](history/code-review-remediation-plan.md). 2026-11-01 review findings. Merged
- [TCP sink spool plan](history/tcp-sink-spool-plan.md). Lab-validated 2026-11-03
- [TLS receiver plan](history/tls-receiver-plan.md). TLS 1.2 and 1.3
- [Windows V2 performance counter plan](history/windows-performance-counters-v2-plan.md). Lab-validated 2026-11-01
- [ETW minimum buffers plan](history/etw-min-buffers-plan.md). `--etw-min-buffers`. Lab-validated 2026-11-01
- [Response failure capture plan](history/response-failure-capture-plan.md). Event 258. Lab-validated 2026-11-01

### Notes on historical records

Records keep their command text and source paths. Where a file moved, the record names the new path. See [script moves and renames](../scripts/performance/README.md#file-moves-and-renames).

The 2026-11-03 history rewrite changed commit hashes from `253df79` (old `8588619`) onward. See the [commit hash mapping](history/commit-hash-mapping.md). Files under `datasets/` keep the old hashes.

## Instrumentation compatibility note

- **Recorded binaries.** The historical campaigns used binaries with a service JSONL metrics writer, which the current service lacks. Their commands, ledgers, and overhead apply to those binaries only. The service-metrics harnesses reject new campaigns before any VM, load, or SCM change.
- **Current telemetry.** Counters go to the Application event log every `--stats-interval-seconds` and at shutdown, and to [performance counters](guide/monitoring.md#performance-counters). The historical campaigns did not measure them.
- **Before another campaign,** design compatible telemetry, and use a fresh backup, unique run IDs, verified guest identities, and fixture readiness.

## Supporting material

- [DNS workload provenance](../datasets/performance/README.md) recreates the pinned 5,000-name test workload.
- [DNS stress client](../tools/dns-stress/README.md) builds and runs the host load generator.
- `docs/assets/performance/` holds graphs and CSV copies, with `data/source-manifest.json` and `plot-provenance.json`.
- Raw ETLs, receipts, and build logs are under the gitignored `target/`. A summary or graph does not replace its source manifest, final ledger, or integrity check.
