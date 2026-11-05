# Documentation index

> **Status:** Current · **Describes:** the documentation layout · **Reviewed:** 2026-11-05

Find the document for your task. Operators install and run the service. Maintainers change the code.

## Operate the service

| To do this | Read |
| --- | --- |
| Build the executable | [Build](guide/build.md) |
| Build the MSI | [Build the MSI](guide/build.md#build-the-msi) |
| Install and register the service | [Install and provision](guide/install.md) |
| Set up the receiver and its certificate | [Set up the dnstap receiver](guide/receiver.md) |
| Look up an option, default, or range | [Configuration reference](guide/configuration.md) |
| Choose settings for a high query rate | [Tune capture for load](guide/tuning.md) |
| Read log events and counters | [Log events and counters](guide/monitoring.md) |
| Fix a failed start, drops, or missing records | [Troubleshooting](guide/troubleshooting.md) |
| Fix `spool key is not usable` | [Spool key errors](guide/troubleshooting.md#spool-key-errors) |
| Fix receiver resets when the service stops | [Service shutdown remediation](guide/service-stop-remediation.md) |
| Upgrade, repair, or change settings | [Upgrade or repair](guide/upgrade.md) |
| Remove the service | [Uninstall](guide/uninstall.md) |
| Generate DNS load for testing | [DNS stress client](../tools/dns-stress/README.md) |

## Understand the design

| To learn this | Read |
| --- | --- |
| How an ETW event becomes a TCP frame | [Data path](design/data-path.md) |
| How memory is budgeted and loss is counted | [Pooled ETW intake](design/pooled-intake.md) |
| How the parser wakes and the service shuts down | [Ring capacity and shutdown](design/ring-buffer.md) |
| How events are decoded | [Single-pass decoder](design/single-pass-decoder.md) |
| What each counter and gate means | [Measurement terms and result gates](design/measurement-terms.md) |
| How the service handles a slow receiver | [Slow and saturated receiver](design/slow-receiver.md) |

## Contribute and report

| To do this | Read |
| --- | --- |
| Build, test, and commit | [Contributing](../CONTRIBUTING.md) |
| Report a vulnerability or read the threat model | [Security policy](../SECURITY.md) |
| See what changed | [Changelog](../CHANGELOG.md) |

## Layout

| Folder | Contents | Status |
| --- | --- | --- |
| [`guide/`](guide) | Operator guides | Current |
| [`design/`](design) | How the code works | Current |
| [`plans/`](plans) | Plans not yet executed | Proposed |
| [`history/`](history) | Dated records of finished and superseded work, with their diagrams and assets | Historical, Superseded |
| [`assets/pipeline/`](assets/pipeline) | Data-path animation | Current |

[CONTRIBUTING.md](../CONTRIBUTING.md#document-types) defines the document types and writing style.

## Status values

Each document starts with a status block.

- **Current:** matches the code. Kept up to date.
- **Historical:** a dated record. Its results apply to the binaries it names. Not edited.
- **Superseded:** replaced by a later document. The status block links the replacement.
- **Proposed:** not yet implemented or executed.

## Plans

Plans move to `history/` when they're executed.

- [Parser and TCP threading analysis](plans/parser-tcp-threading-analysis.md)
- [Parallel parser workers plan](plans/parallel-parser-workers-plan.md): partly executed

## History

`history/` holds lab validations, performance reviews, and executed plans. Each file names the binaries and hosts it covers.

Don't run commands from `history/`. They contain lab addresses and state-changing actions.
