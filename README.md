# etw-dnstap

`etw-dnstap` is a Windows service for administrators who need to capture Windows
DNS Server query and response packets as a dnstap TCP stream. Capture starts
only through the Service Control Manager (SCM), under service name
`EtwDnstap`. The executable has no foreground capture mode; `--help` and
`--version` print command metadata without starting capture.

The service enables the `Microsoft-Windows-DNSServer` Event Tracing for Windows
(ETW) provider in its own
real-time session. It does not enable or alter the Event Viewer analytical log.

To start, follow these tasks in order:

1. [Build and check](#build-and-check) the executable on Windows x64.
2. [Provision a service](#provision-a-service) with a protected executable and the
   required service identity.
3. [Configure capture](#configure-capture) and
   [send to DNS-collector](#send-to-dns-collector).

See the [documentation index](docs/README.md) for architecture, measurement
terms, the completed native performance campaign, and historical reports.

## Build and check

Build on Windows x64 with Visual Studio C++ Build Tools and its Windows SDK.
The pinned `nightly-2026-10-01` toolchain needs `rust-src`, `rustfmt`, and Clippy.
The project rebuilds the Rust standard library with the target hardening flags.
No Protocol Buffers compiler is required.

Run these commands from the repository:

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

The executable is
`target\x86_64-pc-windows-msvc\release\etw-dnstap.exe`. To inspect the available
configuration flags without invoking capture, run:

```powershell
.\target\x86_64-pc-windows-msvc\release\etw-dnstap.exe --help
```

The release profile enables overflow checks and aborts on panic. Production
statically links the MSVC C runtime, so the clean Server Core deployment does
not need the Visual C++ Redistributable DLLs. Those prebuilt native runtime
bytes are not instrumented by Rust compiler flags; servicing them requires
rebuilding and redeploying the executable. The target
configuration enables CFG, stack protection, exception-continuation metadata,
ASLR, NX compatibility, and CET compatibility. A CET image marker does not prove
that Windows or the hardware enforces shadow stacks. Inspect the final PE after
changing the toolchain or build flags. `cargo test-release` tests with unwind
enabled; rebuild the ordinary release before distribution. `cargo asan` builds
the Rust test graph and standard library with AddressSanitizer in a separate
output directory. Windows system DLLs and prebuilt CRT code, including CRT bytes
linked into the executable, are outside that instrumentation.

## Provision a service

Use a Windows Server host with the DNS Server role installed. This version
accepts only an SCM-issued LocalSystem process token with an enabled,
non-deny-only `NT SERVICE\EtwDnstap` group and exactly the enabled
`SeChangeNotifyPrivilege`. Configure an unrestricted service SID and the SCM
privilege allowlist before the first start. Validate this profile against the
real DNS provider in your deployment. Unit tests do not establish live provider
delivery; the [Windows Server 2025 lab record](docs/lab-validation.md) and
[pooled VM campaign](docs/pooled-vm-performance-review.md) record successful
native capture under this profile in the stated lab.

The service does not accept network connections. The network sink initiates an
outbound TCP connection. This application selects LocalSystem with the token
profile described here; this is not an inherent requirement that every ETW
consumer use LocalSystem. Removing token privileges does not remove its
powerful account/group access. A separate privileged broker is not implemented.

Protect the executable and its parent directories against writes by ordinary
users. DNS payloads can contain private names and addresses, so restrict access
to the collector and protect the transport as described in
[Send to DNS-collector](#send-to-dns-collector).

For a **new installation**, complete the following steps from the repository
root in an elevated Windows PowerShell 5.1 session. `New-EventLog` requires
Windows PowerShell. Use the release executable from [Build and check](#build-and-check).
This procedure expects the service and installation directory to be absent.
Start the collector listener first and use its numeric IP address for `RECEIVER_IP`.

1. Create and protect the installation directory:

   ```powershell
   $installDirectory = 'C:\Program Files\EtwDnstap'
   New-Item -ItemType Directory -Path $installDirectory -ErrorAction Stop
   icacls.exe $installDirectory /inheritance:r /grant:r `
       '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F'
   ```

2. Copy the executable and register the service with its TCP receiver:

   ```powershell
   Copy-Item '.\target\x86_64-pc-windows-msvc\release\etw-dnstap.exe' $installDirectory
   $receiver = 'RECEIVER_IP:6000'
   $imagePath = '"{0}\etw-dnstap.exe" --receiver "{1}" --capacity 8192' -f `
       $installDirectory, $receiver
   New-Service -Name EtwDnstap -BinaryPathName $imagePath `
       -StartupType Manual -DependsOn DNS
   ```

3. Configure the service SID, privilege allowlist, and Event Log source:

   ```powershell
   sc.exe sidtype EtwDnstap unrestricted
   sc.exe privs EtwDnstap SeChangeNotifyPrivilege
   New-EventLog -LogName Application -Source EtwDnstap
   ```

4. Inspect the registered identity and access controls:

   ```powershell
   sc.exe qsidtype EtwDnstap
   sc.exe qprivs EtwDnstap
   sc.exe qc EtwDnstap
   sc.exe sdshow EtwDnstap
   ```

   Confirm that each command succeeds, the account is LocalSystem, and only
   SYSTEM and administrators can change the service configuration, start/stop
   it, or write its executable and installation directory. Preserve existing
   identity, privileges, ACLs, and recovery policy when repairing an existing
   installation. These commands were not executed as part of the documentation
   checks.

5. Start capture through SCM:

   ```powershell
   sc.exe start EtwDnstap
   ```

6. Check that the service reaches `RUNNING`:

   ```powershell
   sc.exe query EtwDnstap
   ```

7. After generating the DNS traffic you want to capture, stop the service:

   ```powershell
   sc.exe stop EtwDnstap
   sc.exe query EtwDnstap
   ```

Confirm that the service reaches `STOPPED` with a zero exit code and that the
collector receives the expected dnstap records. The service writes no capture,
metrics, or log files. Use the collector's configured destination for retention.

If `sc.exe query` reports `START_PENDING` or `STOP_PENDING`, repeat it until the
transition completes. For a failed start or nonzero stop code, inspect the
[Application Event Log error](#packet-processing-and-limits).

## Configure capture

When upgrading an existing installation, remove `--output`/`-o`,
`--max-output-bytes`, and `--metrics` from the SCM binary path, and supply
`--receiver` with the collector's numeric IP address and port before starting
the updated executable. Removed arguments fail command-line parsing. Preserve
the service identity, privileges, and protected executable ACLs.

Set configuration arguments in the service binary path before starting the
service. Arguments passed to `sc.exe start` are not used as configuration. The
following flags are available:

| Flag | Behavior |
| --- | --- |
| `--receiver IP:PORT` | Required plaintext bidirectional Frame Streams TCP receiver; accepts a numeric unicast IP and nonzero port. IPv6 uses `[IP]:PORT`. |
| `--capacity N` | Event-channel capacity, 1–16384 events; default 8192. A full channel drops the newest event. |
| `--jumbo-capacity N` | Pool slots for records whose padded payload exceeds 2 KiB, 0–256; default 64. Zero disables jumbo capture. |
| `--duration SECONDS` | Stop the service after 1–604800 seconds; otherwise run until SCM STOP or a capture failure. |
| `--session-name NAME` | Optional unused ETW session name, up to 200 UTF-16 units; default `etw-dnstap-PROCESS_ID`. |
| `--identity TEXT` | Optional dnstap producer identity, up to 1024 UTF-8 bytes; default `etw-dnstap`. Empty text omits the field. |
| `--help`, `-h` | Display configuration help without starting capture. |
| `--version`, `-V` | Display the package version without starting capture. |

## Send to DNS-collector

Set `--receiver "RECEIVER_IP:6000"` in the service binary path.
Replace `RECEIVER_IP` with the collector's
numeric IP address. Start the collector listener before starting `EtwDnstap`.

The TCP sink uses READY/ACCEPT/START negotiation for content type
`protobuf:dnstap.Dnstap`, sends data frames, then exchanges STOP/FINISH during
orderly shutdown. Connection establishment has a two-second timeout. The whole
handshake, each buffered write/flush operation, and STOP/FINISH each have a
two-second deadline shared across partial socket calls. A slow peer cannot
extend a deadline by delivering one byte at a time. Intake remains bounded;
channel or pool exhaustion drops incoming events while the parser waits on the
sink.

An I/O failure ends the service. There is no automatic reconnect or resend;
buffered or queued packets can be lost, and a successful send does not prove
that a collector durably stored the message. The parser flushes buffered TCP
output at least once per second while active.

The sender has no TLS or peer authentication. Plaintext is suitable only for a
trusted lab or an independently protected transport. A Hyper-V Default Switch
is reachable by the host and other connected guests; it is not a confidentiality
boundary. Restrict the listener to the intended guest IP and restrict its
firewall to the Windows DNS guest. No certificate is required for this plaintext
lab configuration. Do not expose it to an untrusted network.

The official [DNS-collector v3.2.3 release](https://github.com/dmachard/DNS-collector/releases/tag/v3.2.3)
uses this dnstap content type and supports the following receiver-to-console
pipeline in its [versioned configuration](https://github.com/dmachard/DNS-collector/blob/v3.2.3/config.yml):

```yaml
global:
  framestream:
    content-type: "protobuf:dnstap.Dnstap"
    handshake-timeout: 5
    data-frame-max-length: 1048576
    control-frame-max-length: 4064
pipelines:
  - name: tap
    dnstap:
      listen-ip: RECEIVER_IP
      listen-port: 6000
    routing-policy:
      forward: [console]
  - name: console
    stdout:
      mode: text
```

Replace `RECEIVER_IP` with the Ubuntu guest's listening IP. The sender accepts
control payloads up to 512 bytes and content-type fields up to 256 bytes,
matching the Frame Streams reference implementation's control bounds. A normal
dnstap ACCEPT is well within those limits.

## Packet processing and limits

The native ETW callback runs in user mode on the `ProcessTrace` worker. It
copies the borrowed event header, payload, and extended data into a preallocated
`PooledBuffer` lease before returning. A bounded `crossbeam_channel` transfers
owned events to a separate parser thread. The pool uses `ArrayQueue` free lists
for slot checkout and return. Callback admission does not allocate, grow
storage, log, or wait for capacity. Atomic retries, spinning, yielding, and
preemption still prevent a formal wait-free execution guarantee.

The parser uses `try_recv` and a 2 ms timed park on an empty channel. The default
channel holds 8192 events; regular storage has 8194 slots of 2 KiB, and the
separate jumbo pool has 64 slots of 131584 bytes. Jumbo exhaustion can drop a
record even when the channel has space. Counted payload, slot, free-list, and
channel storage is 37.32 MiB at the defaults and 90.89 MiB at both maximum
capacities, before allocator, native ETW, parser, and output overhead. These are
storage calculations, not measured process-memory limits. See
[Pooled ETW intake](docs/pooled-intake.md) for ownership and memory accounting and
[Ring capacity and shutdown](docs/ring-buffer.md) for the historical capacity
evidence.

The separate [pooled VM performance review](docs/pooled-vm-performance-review.md)
records native five-minute confirmations at 47,303.24 authoritative DNS QPS
and 27,758.24 recursive DNS QPS, with exact zero-loss delivery of 94,606.49 and
64,973.55 ETW records per sending second. These are observations on the fixed
two-vCPU/4 GiB lab VM with the stated recursive upstream mix. The report retains
overload failures, DNS/lifecycle qualifications, instrumentation differences,
and restoration evidence; it does not establish universal capacity or a causal
CPU improvement from pooling.

The parser uses TDH property APIs and validated metadata to read `PacketData`,
transport, addresses, ports, and timestamps. It caches up to 32 metadata/type
schemas by event ID and version, while individual properties still use TDH
lookups per event. Five property reads still make five TDH size calls and five
TDH extraction calls; their names use static UTF-16 storage. One parser reuses
bounded property and address scratch storage and a packet vector. One Prost
encoder retains identity, address, message, and output storage; it returns the
packet allocation after serialization without an intermediate packet copy.
Cold schema loads, capacity growth, errors, native TDH internals, and output
remain outside the focused allocation tests. See the
[parser and encoder plan](docs/parser-encoder-optimization-plan.md) for the
reuse contract and local validation scope. The parser does not use a manually fixed
payload layout. Measure all intake loss categories and CPU use on the target
DNS host before choosing pool or channel capacities. Pooling alone does not
establish higher throughput or a smaller safe queue.

The encoder preserves `PacketData` bytes verbatim. It checks the 12–65535 byte
DNS envelope and QR direction, but does not validate every DNS name or resource
record. It does not strip or guess a TCP length prefix. Missing or malformed
packet data is rejected; it is never reconstructed from a query name.

The ETW subscription uses level 4 (informational) and `MatchAnyKeyword`
`0x8000000000000000`. The callback accepts only event IDs 256 and 257 before
accessing callback context, counters, pooled storage, or parsing. These events
produce the following client records:

| DNS Server event | dnstap message type |
| --- | --- |
| 256, query received | `CLIENT_QUERY` |
| 257, response success | `CLIENT_RESPONSE` |

Event 258 (response failure), 260 (recursive query out), 261 (recursive response
in), and all other event IDs are excluded. This intake emits no resolver records.
Excluded records do not increment intake, drop, or error counters. Authoritative
and forwarder roles are not automatically distinguished. Endpoint fields retain
initiator/responder meaning across both directions. The peer address/port and local interface address are included
when observed; the provider does not supply the local port for these events,
so it stays absent. The encoder does not invent port 53, pair separate events,
or infer a missing query timestamp for a response.

Frame Streams output uses content type `protobuf:dnstap.Dnstap`, protobuf data
frames, and the TCP control exchange described in
[Send to DNS-collector](#send-to-dns-collector).
An output error ends capture; an I/O-poisoned writer does not append a misleading
STOP after a partial frame.

Diagnostic counters distinguish channel-full `ring_dropped`, `pool_exhausted`,
and `receiver_disconnected`; `intake_dropped` is their sum. Historical zero-loss
analysis includes all three categories and reconciles final capture and parser
ledgers from the recorded instrumentation. Counters also distinguish received events, encoded messages, rejected
events, callback errors, ETW events lost, and real-time ETW
buffers lost. Buffer loss and event loss use different units and are not added
together. SCM exposes service status and a service-specific failure code. The
tracing diagnostics, including packet/loss counters, use stderr, which is not a
durable logging sink under SCM.

Historical performance reports use the removed service JSONL metrics writer.
Their instrumentation overhead and final ledgers apply to the identified
historical binaries. The service has no durable metrics export; stderr does not
replace those ledgers. See [Measurement terms and result gates](docs/measurement-terms.md)
to interpret the preserved counters and CPU units.

Service failures also produce a best-effort Application Event Log entry from
source `EtwDnstap`, event ID 1, level Error, before SCM receives Stopped. Register
the source before the first start as shown in the provisioning commands. The
single insertion string contains the error chain, bounded to 8192 UTF-16 units;
it does not include packet buffers or query names. Event Log failure does not
hide the original service failure. To inspect the stored error text, run:

```powershell
$eventFilter = @{LogName='Application'; ProviderName='EtwDnstap'; Id=1}
Get-WinEvent -FilterHashtable $eventFilter -MaxEvents 10 |
    Select-Object TimeCreated, Id,
        @{Name='Details'; Expression={$_.Properties[0].Value}}
```

Startup and shutdown have 10-second fail-stop watchdogs. On a deadline or an
inability to start a watchdog, the process aborts without freeing resources
still used by native calls. An aborted process can leave an incomplete capture
and a real-time ETW session that requires administrator inspection and cleanup
or reboot. A timeout does not prove that a native operation never completed.

## Format sources and verification scope

[dnstap's source index](https://dnstap.info/Source/) links the
[official proto2 schema](https://github.com/dnstap/dnstap.pb/blob/main/dnstap.proto)
and [Frame Streams reference](https://github.com/farsightsec/fstrm/blob/master/fstrm/control.h).
The handwritten Prost subset retains the upstream field numbers, enum values,
optional-field presence, and `fixed32` nanosecond encoding. The upstream schema
is CC0; the project does not require `protoc`.

[RFC 8618](https://www.rfc-editor.org/info/rfc8618/) defines C-DNS, a separate
CBOR-based capture format. This program writes dnstap rather than C-DNS.
The supported event names and payload fields follow
[Microsoft's DNS logging documentation](https://learn.microsoft.com/en-us/windows-server/networking/dns/dns-logging-and-diagnostics).

Unit tests exercise synthetic callback ownership, malformed native/property
bounds, full-ring loss, shutdown signaling, service status/SID bounds,
exact protobuf bytes, and Frame Streams failure handling. They do not
establish real SCM-issued token behavior, live DNS provider delivery, or
performance under production traffic. No authored C/C++ translation units or
`cc`/CMake dependency build steps are used; the native boundary consists of Rust
calls through `windows-sys` into Windows APIs and linked system/runtime code.

The disposable two-guest validation record is in
[Lab validation](docs/lab-validation.md). It distinguishes receiver readiness,
local build checks, and observed live packet-delivery evidence.
