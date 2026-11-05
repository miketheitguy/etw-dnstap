# etw-dnstap

> **Status:** Current · **Describes:** the `spool-encryption` branch at commit `66c3de4` · **Reviewed:** 2026-11-05

`etw-dnstap` is a Windows service. It reads DNS Server query and response events from Event Tracing for Windows (ETW), the Windows event tracing system. It sends the events over TCP to a [dnstap](https://dnstap.info/) receiver. The records use Frame Streams, a simple framing format.

- **Runs as** the Service Control Manager (SCM) service `EtwDnstap`. It runs under `NT AUTHORITY\LocalService` with a restricted service SID, which is a security identifier for that one service.
- **Reads events only.** It does not turn on or change the DNS Server analytical event log. Its one ETW right lets it create its own real-time trace session.
- **Has no foreground mode.** It has no `--help` or `--version` option.
- **Writes files only for the spool.** The spool is a folder that holds frames the receiver has not accepted yet. Its default path is `C:\ProgramData\EtwDnstap\spool`. Diagnostics go to the Application event log and to performance counters. See [Spool](docs/guide/configuration.md#spool).
- **Encrypts the spool.** Each record uses AES-128-GCM, which also detects changed data. A non-exportable CNG machine key wraps the AES key. CNG is the Cryptography API: Next Generation. See [Spool encryption](docs/guide/configuration.md#encryption) and [SECURITY.md](SECURITY.md).
- **Depends on `DNS` and `KeyIso`.** `KeyIso` is the CNG Key Isolation service. If the key store does not answer within 3 seconds, the service runs without a spool and logs Event ID 26. See [Spool encryption key](docs/guide/install.md#spool-encryption-key).
- **Has no process ID filter.** Capture continues after a DNS Server restart. See [SECURITY.md](SECURITY.md) for the trade-off.

> **Security.** TLS 1.2 or 1.3 is the default. `--plaintext` turns TLS off, so use it only on a trusted network. Read [SECURITY.md](SECURITY.md) and [TLS](docs/guide/configuration.md#tls) before you deploy.

> **Validation.** The LocalService restricted-SID profile was validated on one lab DNS Server (Windows Server 2025, build 26100) on 2026-11-01. Other Windows builds and higher query rates were not tested. Event 258 capture was validated on the same host. Spool encryption ran in the lab on 2026-11-04, at 10,000 queries per second. See the [lab records](docs/README.md#lab-and-validation-records).

## Quick start

1. [Build the executable](docs/guide/build.md). You need the pinned Rust nightly and the Visual Studio C++ Build Tools.
2. Check that the `DNS` service is running. The service does not start without it.
3. [Provision the service](docs/guide/install.md#provision-the-service) with the receiver address. Then grant the ETW right with [`Grant-EtwDnstapEtwAccess.ps1`](scripts/provision/README.md). The receiver does not need to run first.
4. Restart the computer. ETW applies the grant only at boot.
5. [Start the service](docs/guide/install.md#7-start-capture-and-check-that-it-is-running). Generate DNS traffic, then check that the receiver gets the records.

The [MSI installer](docs/guide/install.md#install-with-the-msi) does the provisioning in one pass. It does not start the service. Restart the computer after it finishes, and the restart starts the service.

The release executable is `target\x86_64-pc-windows-msvc\release\etw-dnstap.exe`. Options are in [configuration.md](docs/guide/configuration.md).

## Documentation

| I want to... | Read |
| --- | --- |
| Build the executable | [Build the executable](docs/guide/build.md) |
| Install and register the service | [Install and provision](docs/guide/install.md) |
| Prepare the receiver | [Set up the dnstap receiver](docs/guide/receiver.md) |
| Look up an option, default, or range | [Configuration reference](docs/guide/configuration.md) |
| Tune for a high query rate (defaults suit 10,000 queries per second) | [Tune capture for load](docs/guide/tuning.md) |
| Read the log events and counters | [Log events and counters](docs/guide/monitoring.md) |
| Fix a failed start, drops, or missing records | [Troubleshooting](docs/guide/troubleshooting.md) |
| Change or upgrade an installed service | [Repair or upgrade](docs/guide/upgrade.md) |
| Remove the service | [Uninstall](docs/guide/uninstall.md) |
| Report a vulnerability | [SECURITY.md](SECURITY.md) |
| Build, test, and propose changes | [CONTRIBUTING.md](CONTRIBUTING.md) |
| See the release history | [CHANGELOG.md](CHANGELOG.md) |
| Read the design, measurements, and records | [Documentation index](docs/README.md) |

## How it works

An event moves through five stages, from ETW to the TCP receiver. While the receiver is down or slow, frames wait in the disk spool. See the [data path](docs/design/data-path.md) for each stage.

![Animated data path: ETW session, intake callback, bounded channel, parser, dnstap-sender thread, and TCP receiver. Frames go to the disk spool, encrypted, while the receiver is down. Replay decrypts them, oldest first, after reconnect.](docs/assets/pipeline/etw-to-tcp-pipeline.gif)

## What is captured

| DNS Server event | dnstap message |
| --- | --- |
| 256, query received | `CLIENT_QUERY` |
| 257, response success | `CLIENT_RESPONSE` |
| 258, response failure | `CLIENT_RESPONSE` |

- **Event 258** is a failed response, such as SERVFAIL or a policy REFUSED. A 258 event whose packet is not a response is rejected and counted in `rejected`. NXDOMAIN is a successful response, so it is event 257.
- **Not captured:** event 259 (`IGNORED_QUERY`, which has no packet), events 260 and 261 (recursive query and response), and all other IDs. These increment no counter. The service emits no resolver records.
- **Message bytes** are kept as received. A malformed or missing packet is rejected, never rebuilt.
- **Endpoints and roles.** The records have no local port. The service does not tell authoritative and forwarder roles apart.

The keyword masks, the event-ID filter, and the callback check are in [Subscription](docs/guide/configuration.md#subscription).

## Known limitations

- **Receiver outage.** The service reconnects with backoff, from 250 ms up to 30 s. With a spool, frames wait on disk. Without one, frames lost during the outage are counted in `disconnected_dropped`, and they are never resent. See [Spool](docs/guide/configuration.md#spool) and [Receiver connection](docs/guide/configuration.md#receiver-connection).
- **DNS Server restart.** Capture continues with the new DNS Server process. The session has no process ID filter, so it also accepts events from any other process that writes to the DNS Server provider. See [SECURITY.md](SECURITY.md).
- **Spool encryption.** A copy of the spool folder alone cannot be read. Encryption does not protect against SYSTEM, Administrators, or code running as the service. It also does not protect against an offline copy of a system volume without BitLocker, or a full kernel memory dump or hibernation file. See [SECURITY.md](SECURITY.md). For key errors, see [Spool key errors](docs/guide/troubleshooting.md#spool-key-errors).
- **Recovery.** Data in flight at a failure is lost. See the [recovery policy](docs/guide/install.md#5-configure-scm-recovery).
- **Windows builds.** The identity profile was validated on build 26100 only. See the [validation checklist](docs/guide/install.md#validation-checklist-for-the-localservice-restricted-profile).
- **Evidence.** Unit tests do not cover live SCM token behavior, live DNS provider delivery, or production load. The [pooled VM performance review](docs/performance/pooled-vm-performance-review.md) is not a capacity claim.

## License

[MIT](LICENSE). Copyright (c) 2026 miketheitguy.
