# etw-dnstap

> **Status:** Current · **Describes:** the `spool-encryption` branch · **Reviewed:** 2026-11-05

`etw-dnstap` is a Windows service. It reads DNS Server query and response events from Event Tracing for Windows (ETW) and sends them over TCP to a [dnstap](https://dnstap.info/) receiver as Frame Streams records.

- **Runs as** the Service Control Manager (SCM) service `EtwDnstap`, under `NT AUTHORITY\LocalService` with a restricted service SID.
- **Reads events** through its own real-time ETW trace session. That is its one ETW right.
- **Spools to disk.** The spool holds frames the receiver has not accepted. The default path is `C:\ProgramData\EtwDnstap\spool`. Diagnostics go to the Application event log and performance counters. See [Spool](docs/guide/configuration.md#spool).
- **Encrypts the spool** with AES-128-GCM. A non-exportable Cryptography API: Next Generation (CNG) machine key wraps the AES key. See [Spool encryption](docs/guide/configuration.md#encryption) and [SECURITY.md](SECURITY.md).
- **Depends on `DNS` and `KeyIso`** (CNG Key Isolation). If the key store does not answer within 3 seconds, the service runs without a spool and logs Event ID 26. See [Spool encryption key](docs/guide/install.md#spool-encryption-key).
- **Accepts events from any process** that writes to the DNS Server provider, and keeps capturing after a DNS Server crash and recovery restart. See [SECURITY.md](SECURITY.md).

> **Security.** TLS 1.2 or 1.3 is the default. Use `--plaintext` only on a trusted network. Read [SECURITY.md](SECURITY.md) and [TLS](docs/guide/configuration.md#tls) before you deploy.

> **Validation.** The LocalService restricted-SID profile and event 258 capture were validated on one lab DNS Server (Windows Server 2025, build 26100) on 2026-11-01. Spool encryption ran at 10,000 queries per second on 2026-11-04. Other builds and rates are untested. See the [lab records](docs/README.md#lab-and-validation-records).

## Quick start

1. [Build the executable](docs/guide/build.md) with the pinned Rust nightly and the Visual Studio C++ Build Tools.
2. Start the `DNS` service.
3. [Provision the service](docs/guide/install.md#provision-the-service) with the receiver address, then grant the ETW right with [`Grant-EtwDnstapEtwAccess.ps1`](scripts/provision/README.md).
4. Restart the computer. ETW applies the grant at boot.
5. [Start the service](docs/guide/install.md#7-start-capture-and-check-that-it-is-running), generate DNS traffic, and check that the receiver gets records.

The [MSI installer](docs/guide/install.md#install-with-the-msi) provisions in one pass, and the restart after it starts the service.

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
| Fix a failed start or drops | [Troubleshooting](docs/guide/troubleshooting.md) |
| Change or upgrade an installed service | [Repair or upgrade](docs/guide/upgrade.md) |
| Remove the service | [Uninstall](docs/guide/uninstall.md) |
| Report a vulnerability | [SECURITY.md](SECURITY.md) |
| Build, test, and propose changes | [CONTRIBUTING.md](CONTRIBUTING.md) |
| See the release history | [CHANGELOG.md](CHANGELOG.md) |
| Read design, measurements, records | [Documentation index](docs/README.md) |

## How it works

An event moves through five stages from ETW to the TCP receiver. Frames wait in the disk spool while the receiver is down or slow. See the [data path](docs/design/data-path.md).

![Animated data path: ETW session, intake callback, bounded channel, parser, dnstap-sender thread, and TCP receiver. Frames go to the disk spool, encrypted, while the receiver is down. Replay decrypts them, oldest first, after reconnect.](docs/assets/pipeline/etw-to-tcp-pipeline.gif)

## What is captured

| DNS Server event | dnstap message |
| --- | --- |
| 256, query received | `CLIENT_QUERY` |
| 257, response success | `CLIENT_RESPONSE` |
| 258, response failure | `CLIENT_RESPONSE` |

- **Event 258** is a failed response, such as SERVFAIL or a policy REFUSED. A 258 event whose packet is not a response is counted in `rejected`. NXDOMAIN is event 257.
- **Other events** (259 `IGNORED_QUERY`, 260 and 261, all other IDs) increment no counter.
- **Message bytes** are kept as received. A malformed or missing packet is rejected.
- **Records** carry no local port and do not distinguish authoritative and forwarder roles.

Keyword masks and the event-ID filter are in [Subscription](docs/guide/configuration.md#subscription).

## Known limitations

- **Receiver outage.** The service reconnects with backoff from 250 ms to 30 s. With a spool, frames wait on disk. Without one, frames lost during the outage are counted in `disconnected_dropped` and not resent. See [Spool](docs/guide/configuration.md#spool) and [Receiver connection](docs/guide/configuration.md#receiver-connection).
- **DNS Server crash and recovery restart.** Capture continues, and the session accepts events from any process that writes to the provider. See [SECURITY.md](SECURITY.md).
- **Spool encryption.** It protects a copy of the spool folder alone. SYSTEM, Administrators, code running as the service, an offline copy of a system volume without BitLocker, and a kernel memory dump or hibernation file can read the data. See [SECURITY.md](SECURITY.md) and [Spool key errors](docs/guide/troubleshooting.md#spool-key-errors).
- **Recovery.** Data in flight at a failure is lost. See the [recovery policy](docs/guide/install.md#5-configure-scm-recovery).
- **Windows builds.** The identity profile was validated on build 26100. See the [validation checklist](docs/guide/install.md#validation-checklist-for-the-localservice-restricted-profile). The [pooled VM performance review](docs/performance/pooled-vm-performance-review.md) is not a capacity claim.

## License

[MIT](LICENSE). Copyright (c) 2026 miketheitguy.
