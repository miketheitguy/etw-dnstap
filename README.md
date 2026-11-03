# etw-dnstap

> **Status:** Current · **Describes:** `no-console-output` branch, with the TLS receiver and response failure capture (event 258) · **Reviewed:** 2026-11-03

`etw-dnstap` is a Windows service. It captures DNS Server client query and
response events from Event Tracing for Windows (ETW). It sends them to a
[dnstap](https://dnstap.info/) receiver over TCP, using Frame Streams.

- **Runs as** the Service Control Manager (SCM) service `EtwDnstap`, under
  `NT AUTHORITY\LocalService` with a restricted service SID.
- **Only reads events.** Its one ETW grant lets it create its own real-time
  trace session. It does not enable or change the DNS Server Analytical event
  log.
- **Has no foreground mode.** It has no `--help` or `--version`. A console
  launch exits with code 1 and prints nothing.
- **Writes files only for the spool.** Diagnostics go to the Application event
  log and Windows performance counters. The spool keeps frames that the
  receiver has not accepted yet, in `C:\ProgramData\EtwDnstap\spool` by default.
  See [Spool](docs/guide/configuration.md#spool).

> **Security.** The transport is TLS 1.2 or 1.3 by default (rustls). The service
> verifies the receiver certificate with the Windows machine certificate store.
> `--plaintext` turns TLS off. DNS payloads can contain private names and
> addresses, so use `--plaintext` only on a trusted network. Read
> [SECURITY.md](SECURITY.md) and [TLS](docs/guide/configuration.md#tls) before
> you deploy.

> **Validation.** The LocalService restricted-SID profile was validated on one
> lab DNS Server (Windows Server 2025, build 26100) on 2026-11-01. Load and
> other Windows builds were not covered. Event 258 capture was validated on the
> same host. See the [lab records](docs/README.md#lab-and-validation-records).

## Quick start

1. [Build the executable](docs/guide/build.md). It needs the pinned Rust nightly
   and Visual Studio C++ Build Tools.
2. Check that the `DNS` service is Running. The service refuses to start
   without it.
3. [Provision the service](docs/guide/install.md#provision-the-service) with the
   receiver address. Provisioning does these things:
   - Sets the account to LocalService and the SID type to `restricted`.
   - Limits the privileges to `SeChangeNotifyPrivilege`.
   - Adds the service SID to Performance Log Users.
   - Grants the ETW right with
     [`Grant-EtwDnstapEtwAccess.ps1`](scripts/provision/README.md), then
     restarts the computer.

   The receiver does not need to run first. The service reconnects.
4. [Start the service](docs/guide/install.md#7-start-capture-and-check-that-it-is-running),
   generate DNS traffic, and stop it. Check that the receiver got the records.

The release executable is `target\x86_64-pc-windows-msvc\release\etw-dnstap.exe`.
The options are listed in [configuration.md](docs/guide/configuration.md).

## Documentation

| I want to... | Read |
| --- | --- |
| Build the executable | [Build the executable](docs/guide/build.md) |
| Install and register the service | [Install and provision](docs/guide/install.md) |
| Prepare the receiver and its certificate | [Set up the dnstap receiver](docs/guide/receiver.md) |
| Look up an option, default, or range | [Configuration reference](docs/guide/configuration.md) |
| Choose settings for a high query rate | [Tune capture for load](docs/guide/tuning.md) |
| Read the log events and counters | [Log events and counters](docs/guide/monitoring.md) |
| Fix a failed start, drops, or missing records | [Troubleshooting](docs/guide/troubleshooting.md) |
| Change or upgrade an installed service | [Repair or upgrade](docs/guide/upgrade.md) |
| Remove the service and its leftovers | [Uninstall](docs/guide/uninstall.md) |
| Report a vulnerability or read the threat model | [SECURITY.md](SECURITY.md) |
| Build, test, and propose changes | [CONTRIBUTING.md](CONTRIBUTING.md) |
| See the release history | [CHANGELOG.md](CHANGELOG.md) |
| Follow an event from ETW to the receiver | [Data path](docs/design/data-path.md) |
| Understand the design, measurements, and records | [Documentation index](docs/README.md) |

## How it works

An event moves through five stages: the ETW producer, the intake callback, a
bounded buffer, the parser, and the sink. The sink queues frames for a sender
thread, which writes them to the TCP receiver, or to a disk spool while the
receiver is down or slow. The animation's last stage shows a receiver outage.
The [data path](docs/design/data-path.md) reference describes each stage.

![Animation of the data path in six stages. A DNS Server event moves from the ETW session to the intake callback, which checks it and copies it into a pool slot. A bounded channel queues the slot. The parser thread decodes the record, encodes it as dnstap, releases the slot, and packs the frame into a 64 KiB block. The dnstap-sender thread writes the block to the TCP receiver. In the last stage the receiver is down: frames go to the disk spool, and after the reconnect the spool replays them, oldest first, before newer frames.](docs/assets/pipeline/etw-to-tcp-pipeline.gif)

## What is captured

| DNS Server event | dnstap message |
| --- | --- |
| 256, query received | `CLIENT_QUERY` |
| 257, response success | `CLIENT_RESPONSE` |
| 258, response failure | `CLIENT_RESPONSE` |

- **Event 258** is a failed response, for example SERVFAIL or a policy REFUSED.
  A 258 whose packet is not a response is rejected and counted in `rejected`.
  NXDOMAIN is a successful response (257).
- **Excluded events:** 259 (`IGNORED_QUERY`, which has no packet), 260 and 261
  (recursive query and response), and all other IDs. They increment no counter.
  The service emits no resolver records.
- **Message bytes** are preserved verbatim. A malformed or missing packet is
  rejected, never rebuilt from a query name.
- **Endpoints.** The provider does not supply the local port, so the record has
  none. The encoder does not invent port 53 or pair events.
- **Roles.** Authoritative and forwarder roles are not told apart.

The keyword masks, the event-ID filter, and the callback check are in
[Subscription](docs/guide/configuration.md#subscription).

## Known limitations

- **Receiver outage.** Capture continues, and the service reconnects with
  backoff (250 ms to 30 s). While the receiver is down, frames go to the spool
  when one is in use, and they are sent after the receiver returns, oldest
  first. Without a spool, frames that arrive while the receiver is down are
  counted in `disconnected_dropped`. Frames in flight when a connection fails are
  also counted there, and they are never resent. See
  [Spool](docs/guide/configuration.md#spool) and
  [Receiver connection](docs/guide/configuration.md#receiver-connection).
- **DNS Server restart.** The service exits when the DNS Server process changes
  or stops. SCM restarts it, and capture of the new process starts after that.
  Records from the gap are not captured.
- **Recovery.** The [recovery policy](docs/guide/install.md#5-configure-scm-recovery)
  restarts the service after a failure. Data in flight at the failure is lost.
- **Windows builds.** The identity profile was validated on build 26100 only. On
  another build, run the
  [validation checklist](docs/guide/install.md#validation-checklist-for-the-localservice-restricted-profile).
- **Evidence.** Unit tests cover parsing, framing, shutdown signaling, and the
  protobuf bytes. They do not establish live SCM token behavior, live DNS
  provider delivery, or performance under production traffic. The
  [pooled VM performance review](docs/performance/pooled-vm-performance-review.md)
  gives observations on one two-vCPU lab VM. It is not a capacity claim for other
  hardware.

## License

[MIT](LICENSE). Copyright (c) 2026 miketheitguy.
