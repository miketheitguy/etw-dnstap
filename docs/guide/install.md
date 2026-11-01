# Install and provision the service

> **Status:** Current · **Describes:** `main` at commit `9172f2d` · **Reviewed:** 2026-11-01

This guide covers the toolchain, the build, the service identity, and the SCM
registration for a new installation. Options are in
[configuration.md](configuration.md). Problems are in
[troubleshooting.md](troubleshooting.md). Removal is in [uninstall.md](uninstall.md).

The commands below were written from the source and the reviewed lab procedure.
They were **not executed** as part of this documentation pass. Run them on a
disposable host first.

## Prerequisites

- Windows x64 with the **DNS Server role** installed. The service depends on the
  `DNS` service and is useless without it.
- Visual Studio C++ Build Tools with the Windows SDK, for the MSVC linker.
- A dnstap receiver that accepts Frame Streams over TCP and speaks content type
  `protobuf:dnstap.Dnstap`. The lab used
  [DNS-collector v3.2.3](https://github.com/dmachard/DNS-collector/releases/tag/v3.2.3).
- An elevated Windows PowerShell 5.1 session for the provisioning steps.
  `New-EventLog` and `Remove-EventLog` are run from Windows PowerShell.

No Protocol Buffers compiler is required.

## Install the Rust toolchain

The repository pins its toolchain in [rust-toolchain.toml](../../rust-toolchain.toml).
It is a dated nightly, so a stable toolchain is not enough.

1. Install rustup from <https://rustup.rs> (the `rustup-init.exe` for Windows x64).
2. From the repository root, run `rustup show`. rustup reads
   `rust-toolchain.toml` and installs the pinned nightly with `clippy`,
   `rustfmt`, `rust-src`, and the `x86_64-pc-windows-msvc` target.
3. Confirm with `rustup show active-toolchain`, which should name the pinned
   channel.

**Updating the toolchain.** A pinned channel does not follow `rustup update`.
Change `channel` in `rust-toolchain.toml` in a reviewed commit, then run
`rustup show` again. Record the change in the [CHANGELOG](../../CHANGELOG.md),
because the build output and the measured behavior depend on it. Remove an old
toolchain with `rustup toolchain uninstall <name>` only after the new one builds.

## Build the executable

Run these from the repository root:

```powershell
cargo fmt --check
cargo check --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --locked --release
```

The release executable is
`target\x86_64-pc-windows-msvc\release\etw-dnstap.exe`. `target\` is ignored by
Git. It is a local build artifact, not part of the clone.

Points to know before you build:

- **The first build is slow.** `.cargo/config.toml` enables `build-std`, so
  Cargo rebuilds the standard library and `panic_unwind` with the target
  hardening flags. Later builds reuse that output.
- **Warnings fail the build.** `Cargo.toml` sets `[lints.rust] warnings = "deny"`.
  Any compiler warning is an error, including in a release build.
- **The release profile** enables overflow checks and `panic = "abort"`, and
  statically links the MSVC C runtime (`+crt-static`). The clean Server Core
  deployment does not need the Visual C++ Redistributable.
- **Hardening flags** (CFG, EH continuation guard, ASLR, high-entropy VA, NX,
  CET-compatible marker, `/DEBUG:NONE` for release) come from
  `.cargo/config.toml`. The CET flag is a compatibility marker. It does not prove
  that the hardware enforces shadow stacks. Inspect the final PE after changing
  the toolchain or flags.
- **Check the binary.** `etw-dnstap.exe --version` prints the package version.
  `--help` prints the options without starting capture.

### Optional: diagnostic build

The `diag-timing` Cargo feature adds parser wait and socket-write timing to the
stop report, and enables two diagnostic flags. Build it separately and do not
deploy it as the production binary:

```powershell
cargo build --locked --release --features diag-timing
```

See [configuration.md](configuration.md#diagnostic-flags-diag-timing-builds-only).

### Optional: address-sanitizer builds

`cargo asan` runs the test suite with AddressSanitizer in `target\asan`:

```powershell
cargo asan
```

This needs more than the Rust nightly. On MSVC it needs the AddressSanitizer
runtime from Visual Studio:

1. In the Visual Studio Installer, add the individual component for C++
   AddressSanitizer (the MSVC x64 ASan runtime). Confirm its exact name in your
   installer version.
2. Put the directory that contains `clang_rt.asan_dynamic-x86_64.dll` on `PATH`.
   The DLL is not installed in `System32`, so instrumented test binaries cannot
   find it otherwise.

The alias instruments the Rust test graph and standard library. Windows system
DLLs and prebuilt CRT code are not instrumented. Reading an ASan result is
therefore a statement about Rust-compiled code only.

## Provision the service

Use an elevated Windows PowerShell 5.1 session at the repository root. The steps
assume the service and the installation directory do not exist yet. For an
existing installation, see [Repair or upgrade](#repair-or-upgrade) below.

Start the collector listener **before** the service starts. The service connects
during startup, and it does not wait for a receiver that comes up later.

### 1. Validate the receiver address

Set the collector's numeric IP address and port. Hostnames are rejected, as they
are by the executable's `--receiver` parser.

```powershell
$receiverText = '192.0.2.10:6000'   # replace with your collector's IP:port
$receiverEndpoint = $null
if (-not [System.Net.IPEndPoint]::TryParse($receiverText, [ref]$receiverEndpoint)) {
    throw "Receiver must be a numeric IP address and port, for example 192.0.2.10:6000."
}
$receiverAddress = $receiverEndpoint.Address
$firstOctet = if ($receiverAddress.AddressFamily -eq 'InterNetwork') { $receiverAddress.GetAddressBytes()[0] } else { 0 }
$isMulticast = $receiverAddress.IsIPv6Multicast -or ($firstOctet -ge 224 -and $firstOctet -le 239)
if ($receiverEndpoint.Port -eq 0 -or
    $receiverAddress.Equals([System.Net.IPAddress]::Any) -or
    $receiverAddress.Equals([System.Net.IPAddress]::IPv6Any) -or
    $isMulticast) {
    throw 'Receiver must have a unicast IP address and a nonzero port.'
}
$Receiver = $receiverEndpoint.ToString()   # IPv6 is written as [address]:port
```

The validation mirrors `parse_receiver` in `src/pipeline.rs`: a numeric IP with a
nonzero port, not unspecified and not multicast. It does not test reachability.
Check that with a connection test to the collector.

### 2. Create the protected installation directory

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
New-Item -ItemType Directory -Path $installDirectory -ErrorAction Stop
icacls.exe $installDirectory /inheritance:r /grant:r `
    '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F'
```

Only SYSTEM and Administrators keep write access. Ordinary users must not be
able to replace the executable.

### 3. Copy the executable and register the service

```powershell
Copy-Item '.\target\x86_64-pc-windows-msvc\release\etw-dnstap.exe' $installDirectory
$imagePath = '"{0}\etw-dnstap.exe" --receiver "{1}" --capacity 8192' -f `
    $installDirectory, $Receiver
New-Service -Name EtwDnstap -BinaryPathName $imagePath `
    -StartupType Manual -DependsOn DNS
```

Arguments go in the binary path. Arguments passed to `sc.exe start` are not used
as configuration. The `--capacity 8192` shown here is the default; it is written
out so the value is visible. Add other options from
[configuration.md](configuration.md) to the same string.

### 4. Set the service identity, privileges, and event source

```powershell
sc.exe sidtype EtwDnstap unrestricted
sc.exe privs EtwDnstap SeChangeNotifyPrivilege
New-EventLog -LogName Application -Source EtwDnstap
```

- `sidtype unrestricted` makes the service SID enabled in the process token.
  The service refuses to run if the SID is missing or deny-only.
- `privs` leaves only `SeChangeNotifyPrivilege`. The service refuses to run if
  the token holds any other privilege.
- `New-EventLog` registers the `EtwDnstap` source. Without it, service failures
  and the capture start and stop lines are not written to the Application log.

If the source already exists, `New-EventLog` fails. Check it first with
`[System.Diagnostics.EventLog]::SourceExists('EtwDnstap')`.

### 5. Configure SCM recovery

The service exits with a service-specific error on I/O failure, such as a
dropped receiver connection. That is a non-crash failure, so SCM ignores its
recovery policy unless the non-crash flag is set. Without these two commands,
one dropped connection stops DNS capture until someone starts the service.

```powershell
sc.exe failure EtwDnstap reset= 86400 actions= restart/5000/restart/5000/restart/60000
sc.exe failureflag EtwDnstap 1
```

- `reset= 86400` clears the failure count after one day without failures.
- The first two failures restart the service after 5 seconds. Later failures
  restart it after 60 seconds.
- `failureflag 1` applies the actions to non-crash failures (service-specific
  exit codes). Without it, the `failure` actions do not run for this service.

A restart does not resend lost data. It only restarts capture. A restart that
happens while the receiver is still down fails again, and SCM applies the
delays above. See [troubleshooting.md](troubleshooting.md#the-service-keeps-restarting).

Check the policy:

```powershell
sc.exe qfailure EtwDnstap
sc.exe qfailureflag EtwDnstap
```

### 6. Inspect the registered identity and access controls

```powershell
sc.exe qsidtype EtwDnstap
sc.exe qprivs EtwDnstap
sc.exe qc EtwDnstap
sc.exe sdshow EtwDnstap
```

Confirm that each command succeeds, that the account is LocalSystem, and that
only SYSTEM and Administrators can change the configuration or start and stop
the service.

### 7. Start capture and confirm it is running

```powershell
sc.exe start EtwDnstap
sc.exe query EtwDnstap
```

Repeat `sc.exe query EtwDnstap` while the state is `START_PENDING`. It should
reach `RUNNING`. If it does not, see
[troubleshooting.md](troubleshooting.md#the-service-does-not-reach-running).

### 8. Stop the service after the traffic you want

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

The service should reach `STOPPED` with exit code 0. Confirm that the collector
received the expected records. The service writes no capture, metrics, or log
files. Retention is the collector's job.

## Receiver configuration

The service sends one outbound TCP connection to `--receiver`. It negotiates
content type `protobuf:dnstap.Dnstap` with READY, ACCEPT, and START, then sends
data frames. On shutdown it sends STOP and waits for FINISH. There is no TLS and
no peer authentication.

An example receiver for DNS-collector 3.2.3 listens for dnstap and writes text to
the console. Replace the placeholders with your collector's address:

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
      listen-ip: 192.0.2.10   # the collector's listening IP
      listen-port: 6000        # must match the --receiver port
      reset-conn: false
    routing-policy:
      forward: [console]
  - name: console
    stdout:
      mode: text
```

The service accepts control payloads up to 512 bytes and content-type fields up
to 256 bytes. A normal dnstap ACCEPT is well within those limits.

**Transport security.** Plaintext is suitable only for a trusted lab or an
independently protected transport. A Hyper-V Default Switch is reachable by the
host and by other connected guests, so it is not a confidentiality boundary.
Restrict the listener to the intended guest IP, and restrict its firewall to the
Windows DNS guest. Do not expose the listener to an untrusted network. See
[SECURITY.md](../../SECURITY.md#threat-model-summary).

## Repair or upgrade

For an existing installation, keep the service identity, privileges, executable
ACLs, and recovery policy as they are. Change only what you need.

- **Upgrading from a build with `--output`, `--max-output-bytes`, or `--metrics`.**
  Remove those arguments from the binary path before starting the new
  executable. They now fail command-line parsing. Add `--receiver` with the
  collector's numeric IP and port.
- **Replacing the executable.** Stop the service, copy the new executable over
  the old one, then start the service. Do not change the ACLs.
- **Changing the binary path.** Use `sc.exe config EtwDnstap binPath= "..."`,
  keeping the space after `binPath=`.

## Related

- [Configuration reference](configuration.md)
- [Troubleshooting](troubleshooting.md)
- [Uninstall](uninstall.md)
- [Pooled ETW intake](../pooled-intake.md) for the memory budget
