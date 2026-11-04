# etw-dnstap

> **Status:** Current · **Describes:** the repository at commit `58fde72`, with the MSI, the TLS receiver, and response failure capture (event 258) · **Reviewed:** 2026-11-03

`etw-dnstap` is a Windows service. It captures DNS Server query and response events from Event Tracing for Windows (ETW) and sends them to a [dnstap](https://dnstap.info/) receiver over TCP, using Frame Streams.

- **Runs as** the Service Control Manager (SCM) service `EtwDnstap`, under `NT AUTHORITY\LocalService` with a restricted service SID.
- **Only reads events.** Its one ETW grant lets it create its own real-time trace session. It does not enable or change the DNS Server Analytical event log.
- **Has no foreground mode.** It has no `--help` or `--version`.
- **Writes files only for the spool**, in `C:\ProgramData\EtwDnstap\spool` by default. The spool keeps frames that the receiver has not accepted yet. Diagnostics go to the Application event log and performance counters. See [Spool](docs/guide/configuration.md#spool).

> **Security.** TLS 1.2 or 1.3 is the default transport. `--plaintext` turns TLS off, so use it only on a trusted network. Read [SECURITY.md](SECURITY.md) and [TLS](docs/guide/configuration.md#tls) before you deploy.

> **Validation.** The LocalService restricted-SID profile was validated on one lab DNS Server (Windows Server 2025, build 26100) on 2026-11-01. Load and other Windows builds were not covered. Event 258 capture was validated on the same host. See the [lab records](docs/README.md#lab-and-validation-records).

## Quick start

1. [Build the executable](docs/guide/build.md). It needs the pinned Rust nightly and Visual Studio C++ Build Tools.
2. Check that the `DNS` service is Running. The service refuses to start without it.
3. [Provision the service](docs/guide/install.md#provision-the-service) with the receiver address, and grant the ETW right with [`Grant-EtwDnstapEtwAccess.ps1`](scripts/provision/README.md). The receiver does not need to run first.
4. [Start the service](docs/guide/install.md#7-start-capture-and-check-that-it-is-running), generate DNS traffic, and check that the receiver got the records.

An [MSI installer](docs/guide/install.md#install-with-the-msi) does steps 2 to 6 and needs one restart.

The release executable is `target\x86_64-pc-windows-msvc\release\etw-dnstap.exe`. Options are in [configuration.md](docs/guide/configuration.md).

## Documentation

| I want to... | Read |
| --- | --- |
| Build the executable | [Build the executable](docs/guide/build.md) |
| Install and register the service | [Install and provision](docs/guide/install.md) |
| Prepare the receiver | [Set up the dnstap receiver](docs/guide/receiver.md) |
| Look up an option, default, or range | [Configuration reference](docs/guide/configuration.md) |
| Tune for a high query rate | [Tune capture for load](docs/guide/tuning.md) |
| Read the log events and counters | [Log events and counters](docs/guide/monitoring.md) |
| Fix a failed start, drops, or missing records | [Troubleshooting](docs/guide/troubleshooting.md) |
| Change or upgrade an installed service | [Repair or upgrade](docs/guide/upgrade.md) |
| Remove the service | [Uninstall](docs/guide/uninstall.md) |
| Report a vulnerability | [SECURITY.md](SECURITY.md) |
| Build, test, and propose changes | [CONTRIBUTING.md](CONTRIBUTING.md) |
| See the release history | [CHANGELOG.md](CHANGELOG.md) |
| Read the design, measurements, and records | [Documentation index](docs/README.md) |

## How it works

An event moves from the ETW producer through a bounded buffer to the parser and the sink. A sender thread writes the frames to the TCP receiver. While the receiver is down or slow, frames go to the disk spool. See the [data path](docs/design/data-path.md) for each stage.

![Animated data path: ETW session, intake callback, bounded channel, parser, dnstap-sender thread, and TCP receiver. Frames go to the disk spool while the receiver is down and replay oldest first after reconnect.](docs/assets/pipeline/etw-to-tcp-pipeline.gif)

## What is captured

| DNS Server event | dnstap message |
| --- | --- |
| 256, query received | `CLIENT_QUERY` |
| 257, response success | `CLIENT_RESPONSE` |
| 258, response failure | `CLIENT_RESPONSE` |

- **Event 258** is a failed response, such as SERVFAIL or a policy REFUSED. A 258 whose packet is not a response is rejected and counted in `rejected`. NXDOMAIN is a successful response (257).
- **Excluded:** 259 (`IGNORED_QUERY`, which has no packet), 260 and 261 (recursive query and response), and all other IDs. They increment no counter. The service emits no resolver records.
- **Message bytes** are preserved verbatim. A malformed or missing packet is rejected, never rebuilt.
- **Endpoints and roles.** The record has no local port. Authoritative and forwarder roles are not told apart.

The keyword masks, the event-ID filter, and the callback check are in [Subscription](docs/guide/configuration.md#subscription).

## Known limitations

- **Receiver outage.** The service reconnects with backoff (250 ms to 30 s). Frames go to the spool when one is in use. Without a spool, frames lost during the outage are counted in `disconnected_dropped`, and they are never resent. See [Spool](docs/guide/configuration.md#spool) and [Receiver connection](docs/guide/configuration.md#receiver-connection).
- **DNS Server restart.** The service exits and SCM restarts it. Records from the gap are not captured.
- **Recovery.** Data in flight at a failure is lost. See the [recovery policy](docs/guide/install.md#5-configure-scm-recovery).
- **Windows builds.** The identity profile was validated on build 26100 only. See the [validation checklist](docs/guide/install.md#validation-checklist-for-the-localservice-restricted-profile).
- **Evidence.** Unit tests do not cover live SCM token behavior, live DNS provider delivery, or production load. The [pooled VM performance review](docs/performance/pooled-vm-performance-review.md) is not a capacity claim.

## License

[MIT](LICENSE). Copyright (c) 2026 miketheitguy.
