# Install and provision the service

> **Status:** Current · **Describes:** `main` at commit `54eafae` (service identity and reconnect behavior from `f0b51f4`) · **Reviewed:** 2026-11-01

> **Validation status: the LocalService restricted-SID profile has not been
> validated on a live DNS Server.** The lab record,
> [lab-validation.md](../history/lab-validation.md), validated the earlier
> LocalSystem profile only. The provisioning steps below are written from the
> source and have not been run on a live DNS Server under
> `NT AUTHORITY\LocalService` with a restricted service SID. Whether ETW access
> checks pass for a write-restricted token is an assumption. Run the steps on a
> disposable DNS host first, complete the
> [validation checklist](#validation-checklist-for-the-localservice-restricted-profile),
> and record the result before production use.

This guide covers the toolchain, the build, the service identity, and the SCM
registration for a new installation. Options are in
[configuration.md](configuration.md). Problems are in
[troubleshooting.md](troubleshooting.md). Removal is in [uninstall.md](uninstall.md).

The commands below were written from the source and the reviewed lab procedure.
They were **not executed** as part of this documentation pass. Run them on a
disposable host first.

## Prerequisites

- Windows x64 with the **DNS Server role** installed, and the `DNS` service
  Running. The service depends on the `DNS` service. It refuses to start while
  the `DNS` service is not running, and it has nothing to capture without it.
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

The service runs as `NT AUTHORITY\LocalService` (`S-1-5-19`) with a
**restricted** service SID type, which gives it a write-restricted token. At
startup it requires all of the following, and refuses to start otherwise:

- the token user is `S-1-5-19`;
- the service SID `NT SERVICE\EtwDnstap` (`S-1-5-80-...`) is an enabled group;
- the token is restricted, and its restricting SIDs include the service SID and
  the Write Restricted SID `S-1-5-33` (the SCM also adds Everyone and the logon
  SID);
- Performance Log Users (`S-1-5-32-559`) is an enabled group, which arrives
  because the service SID is a member of that group;
- the only privilege is an enabled `SeChangeNotifyPrivilege`.

Because the token is write-restricted, write-type access checks must also be
satisfied by a restricting SID. Group membership alone may not be enough, so
step 4 grants the ETW rights directly to the service SID.

Starting the collector listener first is still the simplest path, but it is not
required. If the receiver is unreachable at startup, the service runs
disconnected and reconnects with backoff. A receiver that answers but does not
accept the dnstap content type stops the service at startup.

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

The validation mirrors `check_receiver` in `src/pipeline.rs`: a numeric IP with a
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
able to replace the executable. The service SID gets read and execute in
step 3. No write ACE is needed: the service writes no files.

### 3. Copy the executable, register the service, and grant read and execute

```powershell
Copy-Item '.\target\x86_64-pc-windows-msvc\release\etw-dnstap.exe' $installDirectory
$imagePath = '"{0}\etw-dnstap.exe" --receiver "{1}" --capacity 8192' -f `
    $installDirectory, $Receiver
New-Service -Name EtwDnstap -BinaryPathName $imagePath `
    -StartupType Manual -DependsOn DNS
icacls.exe $installDirectory /grant 'NT SERVICE\EtwDnstap:(OI)(CI)(RX)'
```

Arguments go in the binary path. Arguments passed to `sc.exe start` are not used
as configuration. The `--capacity 8192` shown here is the default; it is written
out so the value is visible. Add other options from
[configuration.md](configuration.md) to the same string.

The name `NT SERVICE\EtwDnstap` resolves once the service exists, so the
`icacls` grant comes after `New-Service`. Step 2 removed inherited ACEs. Without
this grant, the copied executable is not readable by the service, and the
service cannot start. **Unverified:** read and execute access should pass the
normal access check, because the second, restricting-SID pass applies only to
write-type access. Confirm on a disposable host that the service starts.

### 4. Set the service identity, privileges, ETW access, and event source

```powershell
sc.exe config EtwDnstap obj= "NT AUTHORITY\LocalService" password= ""
sc.exe sidtype EtwDnstap restricted
sc.exe privs EtwDnstap SeChangeNotifyPrivilege
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -WhatIf
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1
New-EventLog -LogName Application -Source EtwDnstap
```

- `obj=` runs the service as LocalService, which has no password. A service
  still set to LocalSystem or a virtual account fails the identity check.
- `sidtype restricted` makes the SCM build a write-restricted token. The
  restricting SIDs are the service SID, Everyone, the logon SID, and
  `S-1-5-33`. An unrestricted token fails the identity check.
- `privs` leaves only `SeChangeNotifyPrivilege` out of LocalService's default
  privileges. The service refuses to run if the token holds any other
  privilege, or if that privilege is disabled.
- `net localgroup` adds the **service SID**, not all LocalService processes, to
  Performance Log Users. That lets the service create and control ETW sessions.
  The membership must be enabled in the token, not deny-only. Restart the
  service after you change membership.
- `Grant-EtwDnstapEtwAccess.ps1` grants rights directly to the service SID with
  `EventAccessControl`: `TRACELOG_GUID_ENABLE` (0x80) on the DNS provider GUID,
  and query, set, `TRACELOG_CREATE_REALTIME` and `TRACELOG_ACCESS_REALTIME` (mask
  0x423) on the default WMI security GUID. These are needed because a
  write-restricted token only matches restricting SIDs on write-type checks. Run
  it with `-WhatIf` first. It prints the resulting SDDL. Details are in
  [scripts/provision/README.md](../../scripts/provision/README.md).
- `New-EventLog` registers the `EtwDnstap` source. Without it, service failures
  and the capture start and stop lines are not written to the Application log.
  The default security descriptor of the Application channel is believed to
  grant write to `S-1-5-33`, so `ReportEvent` should work for a restricted
  service. **Unverified:** confirm that Event ID 2 appears after start.

If the source already exists, `New-EventLog` fails. Check it first with
`[System.Diagnostics.EventLog]::SourceExists('EtwDnstap')`.

### If an ETW access check is denied

If the log shows `StartTraceW: access denied` or
`EnableTraceEx2: access denied enabling the Microsoft-Windows-DNSServer provider`,
the service SID lacks a right the message names. Run
`scripts\provision\Grant-EtwDnstapEtwAccess.ps1` as an administrator, confirm
the service SID is in Performance Log Users, and restart the service. If it is
still denied, the access mask in the script is incomplete for this OS build.
Inspect the printed SDDL, and record the mask that works.

### 5. Configure SCM recovery

The service exits with a service-specific error when the DNS Server process
changes or stops, when a capture worker fails, and on other run failures. That
is a non-crash failure, so SCM ignores its recovery policy unless the non-crash
flag is set. Without these two commands, one such failure stops DNS capture
until someone starts the service.

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
happens while the receiver is still down does not fail: the service starts
disconnected and retries. A restart that happens while the `DNS` service is
stopped fails at startup. See
[troubleshooting.md](troubleshooting.md#the-service-keeps-restarting).

Check the policy:

```powershell
sc.exe qfailure EtwDnstap
sc.exe qfailureflag EtwDnstap
```

### 6. Inspect the registered identity and access controls

```powershell
sc.exe qc EtwDnstap
sc.exe qsidtype EtwDnstap
sc.exe qprivs EtwDnstap
net localgroup "Performance Log Users"
sc.exe sdshow EtwDnstap
icacls.exe $installDirectory
```

Confirm that each command succeeds, that `sc.exe qc` shows `SERVICE_START_NAME`
as `NT AUTHORITY\LocalService`, that `sc.exe qsidtype` reports `RESTRICTED`,
that `sc.exe qprivs` lists only `SeChangeNotifyPrivilege`, that Performance Log
Users lists `NT SERVICE\EtwDnstap`, and that only SYSTEM and Administrators can
change the configuration or start and stop the service.

### 7. Start capture and confirm it is running

```powershell
sc.exe start EtwDnstap
sc.exe query EtwDnstap
```

Repeat `sc.exe query EtwDnstap` while the state is `START_PENDING`. It should
reach `RUNNING`. If it does not, see
[troubleshooting.md](troubleshooting.md#the-service-does-not-reach-running).
Look for a `capture started` line (Event ID 2) in the Application log.

### 8. Stop the service after the traffic you want

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

The service should reach `STOPPED` with exit code 0. Confirm that the collector
received the expected records. The service writes no capture, metrics, or log
files. Retention is the collector's job.

### Validation checklist for the LocalService restricted profile

Complete this checklist on a disposable DNS host before you rely on the profile.
Record each result with the binary's commit. None of it has been run.

- The service reaches `RUNNING`, and `capture started` shows the expected keyword
  masks without `keyword_fallback` set.
- `foreign_pid_rejected` stays 0 under DNS traffic, and `received` rises.
- `StartTraceW` and `EnableTraceEx2` are not denied. If they are, follow
  [the ETW access step](#if-an-etw-access-check-is-denied).
- Application log writes work: Event ID 2 (`capture started`) appears.
- Under DNS traffic, `output stats` shows `receiver_connected` true, and the
  collector receives the records.
- After `sc.exe stop`, the service reaches `STOPPED` with exit code 0, and the
  final `output stopped` line reports `output_errors` 0.
- A DNS Server restart produces a gap and an automatic EtwDnstap restart under
  the recovery policy.
- The `Global\EtwDnstap-<session>` instance mutex is created. LocalService
  normally holds `SeCreateGlobalPrivilege`, and the restricted privilege list
  removes it. No unit test or lab run has covered this combination.
- The SCM query of the `DNS` service (`OpenSCManagerW`, `OpenServiceW`,
  `QueryServiceStatusEx`) succeeds under the restricted token.
- `sc.exe sidtype EtwDnstap none` (a negative test) makes the service stop at
  startup with the restricted-token message.

## Receiver configuration

The service keeps one outbound TCP connection to `--receiver`. It negotiates
content type `protobuf:dnstap.Dnstap` with READY, ACCEPT, and START, then sends
data frames. On shutdown it sends STOP and waits for FINISH. After a failure it
reconnects and negotiates again; frames lost in the gap are counted, not resent.
There is no TLS and no peer authentication.

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

- **Upgrading from LocalSystem or the virtual account.** A service still
  registered as LocalSystem or `NT SERVICE\EtwDnstap` refuses to run with this
  build. Set the identity from
  [step 4](#4-set-the-service-identity-privileges-etw-access-and-event-source),
  keep the service SID's read and execute grant from [step 3](#3-copy-the-executable-register-the-service-and-grant-read-and-execute),
  keep the recovery policy from [step 5](#5-configure-scm-recovery), and then
  start the service. Confirm the account with `sc.exe qc EtwDnstap`. Changing
  `obj=` can clear the SID type, so rerun `sc.exe sidtype EtwDnstap restricted`
  and check it with `sc.exe qsidtype EtwDnstap`.
- **Upgrading from a build with `--output`, `--max-output-bytes`, or `--metrics`.**
  Remove those arguments from the binary path before starting the new
  executable. They now fail command-line parsing. Add `--receiver` with the
  collector's numeric IP and port.
- **Replacing the executable.** Stop the service, copy the new executable over
  the old one, then start the service. Do not change the ACLs. The service
  SID's read and execute grant must remain.
- **Changing the binary path.** Use `sc.exe config EtwDnstap binPath= "..."`,
  keeping the space after `binPath=`.

## Related

- [Configuration reference](configuration.md)
- [Troubleshooting](troubleshooting.md)
- [Uninstall](uninstall.md)
- [Pooled ETW intake](../design/pooled-intake.md) for the memory budget
