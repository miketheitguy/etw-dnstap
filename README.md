# etw-dnstap

> **Status:** Current · **Describes:** `main` at commit `dde7188` · **Reviewed:** 2026-11-01
>
> **Validation status:** the LocalService profile with a restricted service SID
> (`NT AUTHORITY\LocalService`, `sc.exe sidtype EtwDnstap restricted`) was
> validated on one lab DNS Server (Windows Server 2025, build 26100) on
> 2026-11-01. See [localservice-restricted-validation.md](docs/history/localservice-restricted-validation.md).
> A later lab run tested the SCM recovery policy and a DNS Server restart. Load and
> other Windows builds were not covered. See
> [the skills evaluation lab record](docs/history/skills-evaluation-lab-validation.md).

`etw-dnstap` is a Windows service that captures DNS Server client query and
response events and sends them to a [dnstap](https://dnstap.info/) receiver over
TCP using Frame Streams. It runs under the Service Control Manager (SCM) as
`EtwDnstap`, as `NT AUTHORITY\LocalService` with a restricted service SID. The
service only reads ETW events. Its one ETW grant lets it create its own trace
session. The executable has no foreground capture mode. `--help` and `--version` print information and exit
without starting capture.

It enables the `Microsoft-Windows-DNSServer` Event Tracing for Windows (ETW)
provider in its own real-time session. It does not enable or change the DNS
Server Analytical event log.

**Security first.** The transport is **plaintext TCP with no peer
authentication**. DNS payloads can contain private names and addresses. Use it
only on a trusted network or behind an independently protected transport. Read
[SECURITY.md](SECURITY.md) before deploying.

## Quick start

1. Install the pinned Rust toolchain, then build the release executable. See
   [Install the Rust toolchain](docs/guide/install.md#install-the-rust-toolchain) and
   [Build the executable](docs/guide/install.md#build-the-executable).
2. Check that the DNS Server service is Running. The service refuses to start
   without it.
3. Provision the service with the checked receiver address. Provisioning does
   these things:
   - It sets the account to LocalService and runs `sc.exe sidtype EtwDnstap restricted`.
   - It limits the privileges to `SeChangeNotifyPrivilege`.
   - It adds the service SID to Performance Log Users.
   - It runs `scripts\provision\Grant-EtwDnstapEtwAccess.ps1`, then restarts the computer.

   You can start the receiver's listener first, but it is not necessary. The
   service reconnects. See
   [Provision the service](docs/guide/install.md#provision-the-service) and
   [scripts/provision](scripts/provision/README.md).
4. Start the service, generate DNS traffic, and stop it. Check that the receiver
   got the expected records. See [step 7](docs/guide/install.md#7-start-capture-and-check-that-it-is-running).

The release executable is `target\x86_64-pc-windows-msvc\release\etw-dnstap.exe`.
Run `--help` for the options. Their defaults and ranges are in the
[configuration reference](docs/guide/configuration.md).

## Guides

| Task | Document |
| --- | --- |
| Install the toolchain, build, provision, and register the service | [Install and provision](docs/guide/install.md) |
| Every option, default, and range, and the low-rate limits | [Configuration reference](docs/guide/configuration.md) |
| Diagnose a failed start, restarts, drops, or missing records | [Troubleshooting](docs/guide/troubleshooting.md) |
| Read the periodic log counters and what each one means | [Log events and counters](docs/guide/configuration.md#log-events-and-counters) |
| Remove the service, the event source, and leftover ETW sessions | [Uninstall](docs/guide/uninstall.md) |
| Reporting vulnerabilities and the threat model | [SECURITY.md](SECURITY.md) |
| Building, testing, and proposing changes | [CONTRIBUTING.md](CONTRIBUTING.md) |
| Release history | [CHANGELOG.md](CHANGELOG.md) |
| Architecture, measurements, and performance records | [Documentation index](docs/README.md) |

## What is captured

| DNS Server event | dnstap message |
| --- | --- |
| 256, query received | `CLIENT_QUERY` |
| 257, response success | `CLIENT_RESPONSE` |

Events 258 (response failure), 260 and 261 (recursive query and response), and all
other event IDs are excluded. The service emits no resolver records. Excluded
events do not increment any counter.

- Authoritative and forwarder roles are not distinguished automatically.
- Client and peer endpoint fields keep their initiator and responder meaning in
  both directions. The local port is not supplied by the provider, so it is
  absent. The encoder does not invent port 53 or pair events.
- DNS message bytes are preserved verbatim. A malformed or missing packet is
  rejected, never reconstructed from a query name.

Event 256 and 257 selection, the keyword masks, and the callback check are in
[configuration.md](docs/guide/configuration.md#subscription).

## Build and verification

- Toolchain: a pinned nightly in [rust-toolchain.toml](rust-toolchain.toml), with
  `rust-src`, `rustfmt`, and Clippy. The build needs the nightly for its `-Z`
  flags and `build-std`. The project sets no minimum stable Rust version. No
  Protocol Buffers compiler is needed.
- `scripts/ci/verify-pe.ps1` checks the release executable for the hardening
  flags, a missing PDB reference, and unwanted runtime imports. CI runs it.
- `Cargo.toml` denies all rustc warnings. A warning fails the build.
- The first build rebuilds the standard library with the hardening flags
  (`build-std`). It is slow. Later builds reuse the output.
- Production statically links the MSVC C runtime. The release profile aborts on
  panic and enables overflow checks.
- `cargo asan` needs the Visual Studio AddressSanitizer runtime on `PATH`. See
  [optional address-sanitizer builds](docs/guide/install.md#optional-address-sanitizer-builds).

The local build commands, test counts, and CI status are in
[CONTRIBUTING.md](CONTRIBUTING.md#build-and-check). Continuous integration runs
on GitHub Actions (`.github/workflows/ci.yml`).

## Evidence and limits

The unit tests cover parsing, framing, shutdown signaling, and the protobuf bytes.
They do not establish live SCM token behavior, live DNS provider delivery, or
performance under production traffic. The lab records are:

- [Lab validation](docs/history/lab-validation.md): receiver readiness and initial live
  delivery, October 28, 2026. It validated the earlier LocalSystem profile, not
  the LocalService restricted-SID profile.
- [Pooled VM performance review](docs/performance/pooled-vm-performance-review.md): native
  five-minute confirmations on a fixed two-vCPU lab VM, run under LocalSystem.
  These are observations, not a capacity claim for other hardware.

**Known limitations.**

- **Receiver connection.** A receiver failure does not stop capture. The service
  reconnects with backoff (250 ms to 30 s) and repeats the handshake. Events
  that arrive while disconnected, and frames buffered at the failure, are dropped
  and counted in `disconnected_dropped`. They are never resent. See
  [Receiver connection](docs/guide/configuration.md#receiver-connection).
- **DNS Server restarts.** The service exits when the DNS Server process changes
  or stops. SCM then restarts it, and capture of the new process starts after
  that restart. Records from the gap are not captured.
- **Identity.** The service runs as `NT AUTHORITY\LocalService` with a restricted
  service SID. The profile was validated on build 26100 only. On another build,
  follow the
  [validation checklist](docs/guide/install.md#validation-checklist-for-the-localservice-restricted-profile).
- **Recovery.** The policy in
  [install.md](docs/guide/install.md#5-configure-scm-recovery) restarts the
  service after failures, but data in flight at a failure is lost.

## License

[MIT](LICENSE). Copyright (c) 2026 Mike Cramer.
