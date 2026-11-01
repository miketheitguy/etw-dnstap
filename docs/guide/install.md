# Install and provision the service

> **Status:** Current · **Describes:** `main` at commit `dde7188`, with the LocalService restricted-SID profile validated on 2026-11-01 · **Reviewed:** 2026-11-01

> **Validation status:** the LocalService restricted-SID profile was validated
> on one lab DNS Server (Windows Server 2025, build 26100) on 2026-11-01. See
> [localservice-restricted-validation.md](../history/localservice-restricted-validation.md).
> Load and other Windows builds were not covered. A later lab run tested the
> SCM recovery policy and a DNS Server restart. See
> [the lab record](../history/skills-evaluation-lab-validation.md). On
> other builds, run the
> [validation checklist](#validation-checklist-for-the-localservice-restricted-profile)
> on a disposable host first.

Use this guide to set up a new host. It covers the toolchain, the build, the
service identity, and the SCM registration.

| Task | Section |
| --- | --- |
| Prepare the host | [Prerequisites](#prerequisites) |
| Build the executable | [Install the Rust toolchain](#install-the-rust-toolchain), [Build the executable](#build-the-executable) |
| Install a new service | [Provision the service](#provision-the-service), steps 1 to 8 |
| Test the profile on a new Windows build | [Validation checklist](#validation-checklist-for-the-localservice-restricted-profile) |
| Set up the dnstap receiver | [Receiver configuration](#receiver-configuration) |
| Change an existing installation | [Repair or upgrade](#repair-or-upgrade) |

The options are in [configuration.md](configuration.md). Diagnostics are in
[troubleshooting.md](troubleshooting.md). Removal is in [uninstall.md](uninstall.md).

Steps 3, 4 and 7 were run on the lab host recorded above. The other steps come
from the source and the reviewed lab procedure.

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
2. From the repository root, run `rustup toolchain install` with no toolchain
   name. rustup reads `rust-toolchain.toml` and installs the pinned nightly with
   `clippy`, `rustfmt`, `rust-src`, and the `x86_64-pc-windows-msvc` target. CI
   uses the same command. Current rustup versions do not install a toolchain
   from `rustup show`.
3. Run `rustup show active-toolchain`. Check that the output names the pinned
   channel.

**Updating the toolchain.** A pinned channel does not follow `rustup update`.
Change `channel` in `rust-toolchain.toml` in a reviewed commit, then run `rustup
toolchain install` again. Record the change in the
[CHANGELOG](../../CHANGELOG.md), because the build output and the measured
behavior depend on it. Remove an old toolchain with `rustup toolchain uninstall
<name>` only after the new one builds.

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
- **Warnings fail the build.** Under `[lints.rust]`, `Cargo.toml` sets
  `warnings` to `deny`. Any compiler warning is an error, including in a
  release build.
- **The release profile** enables overflow checks and `panic = "abort"`.
  `.cargo/config.toml` statically links the MSVC C runtime (`+crt-static`) for
  every build of the target. The clean Server Core deployment does not need the
  Visual C++ Redistributable.
- **Hardening flags** (CFG, EH continuation guard, ASLR, high-entropy VA, NX,
  CET-compatible marker, stack protection (`-Z stack-protector=all`), and
  `/DEBUG:NONE` for release) come from
  `.cargo/config.toml`. The CET flag is a compatibility marker. It does not prove
  that the hardware enforces shadow stacks. After each build, and after any
  toolchain or flag change, check the release executable:

  ```powershell
  scripts\ci\verify-pe.ps1 -Path target\x86_64-pc-windows-msvc\release\etw-dnstap.exe
  ```

  The script fails in each of these conditions:

  - A hardening flag is missing.
  - The image has a PDB entry.
  - The image imports an ASan runtime, or the dynamic C or Universal CRT.
  - The image contains an absolute `C:\Users\` path.

  CI runs the same check on the release build.
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
   AddressSanitizer (the MSVC x64 ASan runtime). Check its exact name in your
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
**restricted** service SID type. Windows calls the resulting token
*write-restricted*. The name refers to Windows access checks, not to ETW data:
the service only reads events. At
startup it requires all of the following, and refuses to start otherwise:

- The token user is `S-1-5-19`.
- The service SID `NT SERVICE\EtwDnstap` (`S-1-5-80-...`) is an enabled group.
- The token is restricted. Its restricting SIDs include the service SID and
  the Write Restricted SID `S-1-5-33`. The SCM also adds Everyone and the logon
  SID.
- Performance Log Users (`S-1-5-32-559`) is an enabled group. The token gets
  this group because the service SID is a member of it.
- The only privilege is an enabled `SeChangeNotifyPrivilege`.

For any access that Windows classes as a write (a change of state), a
write-restricted token must also pass a second check. That check considers only
the restricting SIDs. Creating the service's own real-time trace session is
such a change. LocalService and Performance Log Users do not count for it, so
step 4 grants `TRACELOG_CREATE_REALTIME` directly to the service SID. Every
other ETW call the service makes passed in the lab without a grant: enabling
the provider, consuming the session, and querying and stopping it.

The grant is scoped to the service's own session. The service starts its ETW
session with a stable GUID derived from `--session-name`, and ETW checks the
right against that GUID's descriptor. Step 4 grants on that GUID only and
leaves the shared default descriptor unchanged. This matches how Windows
provisions its own restricted LocalService services, such as `DPS`: their ETW
rights are granted to their service SIDs on specific GUIDs.

Starting the collector listener first is still the simplest path, but it is not
required. If the receiver is unreachable at startup, the service runs
disconnected and reconnects with backoff. A receiver that answers but does not
accept the dnstap content type stops the service at startup.

### 1. Check the receiver address

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
as configuration. The `--capacity 8192` shown here is the default. It is written
out so the value is visible. Add other options from
[configuration.md](configuration.md) to the same string.

The name `NT SERVICE\EtwDnstap` resolves once the service exists, so the
`icacls` grant comes after `New-Service`. Step 2 removed inherited ACEs. Without
this grant, the copied executable is not readable by the service, and the
service cannot start. Read and execute is not a write, so this grant to the
service SID is enough. The lab confirmed it.

### Create a protected snapshot folder

A standard user can create folders in `C:\ProgramData`. A folder that a user made
before you, or a link that a user planted in it, would make a later restore
unsafe. This step creates `C:\ProgramData\EtwDnstap`, `\backup`, and a folder for
each snapshot with Administrators as owner, no inherited ACEs, and full control
for SYSTEM and Administrators only. It stops if a folder already exists with
another owner, with an extra ACE, or as a reparse point. Do not take ownership
of such a folder. Inspect it, and remove it only when you know where it came
from.

```powershell
$trustedSids = 'S-1-5-18', 'S-1-5-32-544'

function Test-ProtectedFolder {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "'$Path' is a reparse point. Inspect it. Do not use it."
    }
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($trustedSids -notcontains $owner) {
        throw "'$Path' is owned by $owner. Inspect it. Do not use it."
    }
    $extra = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]) |
            Where-Object { $trustedSids -notcontains $_.IdentityReference.Value })
    if ($extra.Count -gt 0) {
        throw "'$Path' has an ACE for $($extra[0].IdentityReference). Inspect it. Do not use it."
    }
}

function New-ProtectedFolder {
    param([string]$Path)
    New-Item -ItemType Directory -Path $Path -ErrorAction Stop | Out-Null
    icacls.exe $Path /setowner '*S-1-5-32-544' | Out-Null
    icacls.exe $Path /inheritance:r /grant:r '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F' | Out-Null
    if (@(Get-ChildItem -LiteralPath $Path -Force).Count -ne 0) { throw "'$Path' is not empty. Inspect it." }
    Test-ProtectedFolder $Path
}

function New-ProtectedBackupFolder {
    $root = Join-Path $env:ProgramData 'EtwDnstap\backup'
    foreach ($dir in (Split-Path -Parent $root), $root) {
        if (-not (Test-Path -LiteralPath $dir)) { New-ProtectedFolder $dir }
        Test-ProtectedFolder $dir
    }
    $leaf = Join-Path $root (Get-Date -Format yyyyMMddHHmmss)
    New-ProtectedFolder $leaf    # fails if the folder exists
    $leaf
}
```

Run this block in the same PowerShell session as the commands that follow. Then
`$backup = New-ProtectedBackupFolder` creates a snapshot folder and returns its
path. The grant script applies the same checks to its own backup folders, and it
never reuses a folder that exists.

### 4. Set the service identity, privileges, ETW access, and event source

```powershell
$backup = New-ProtectedBackupFolder
sc.exe sdshow EtwDnstap | Set-Content -Path "$backup\service-sd.txt" -Encoding ascii
cmd.exe /c 'sc.exe config EtwDnstap obj= "NT AUTHORITY\LocalService" password= ""'
sc.exe sidtype EtwDnstap restricted
sc.exe privs EtwDnstap SeChangeNotifyPrivilege
cmd.exe /c 'sc.exe sdset EtwDnstap "D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLORC;;;IU)(A;;CCLCSWLORC;;;SU)S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)"'
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap -WhatIf
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap -BackupPath "$backup\etw"
New-EventLog -LogName Application -Source EtwDnstap
Restart-Computer
```

- `obj=` runs the service as LocalService, which has no password. Run it
  through `cmd.exe`. Windows PowerShell 5.1 drops an empty `""` argument, so
  `sc.exe` prints its usage and the account stays unchanged. Check the result
  with `sc.exe qc EtwDnstap`. A service still set to LocalSystem or a virtual
  account fails the identity check.
- `sidtype restricted` makes the SCM build a write-restricted token. The
  restricting SIDs are the service SID, Everyone, the logon SID, and
  `S-1-5-33`. An unrestricted token fails the identity check.
- `privs` leaves only `SeChangeNotifyPrivilege` out of LocalService's default
  privileges. The service refuses to run if the token holds any other
  privilege, or if that privilege is disabled.
- `sc.exe sdshow` saves the service security descriptor that the SCM gave to
  `New-Service`. `sc.exe sdset` replaces it. The new descriptor gives SYSTEM
  query, start, stop and control rights. It gives Administrators full control.
  Interactive and service logons get only these rights: query the configuration,
  query the status, enumerate dependents, interrogate the service, and read the
  descriptor. They cannot start, stop, pause, or
  reconfigure the service. Network and batch logons get no right, so a remote
  user cannot read the binary path, which holds the collector address. The SACL
  audits failed access by any principal. Run `sdset` through `cmd.exe` for the
  same reason as `obj=`: it keeps the quotes and the parentheses of the string
  intact. The default descriptor also gives interactive and service logons the
  user-defined control right. The service does not use it, so the new descriptor
  omits it. The lab confirmed on build 26100 that `sc.exe sdshow EtwDnstap`
  prints exactly the string in step 6 after `sdset`. A standard user got "Access
  is denied" (error 5) from `sc.exe stop`, `start`, `config`, and `control
  EtwDnstap 200`, and could query the service.
- `net localgroup` adds the **service SID**, not all LocalService processes, to
  Performance Log Users. That lets the service create and control ETW sessions.
  The membership must be enabled in the token, not deny-only. Restart the
  service after you change membership.
- `Grant-EtwDnstapEtwAccess.ps1` grants one right to the service SID with
  `EventAccessControl`: `TRACELOG_CREATE_REALTIME` (0x20) on the session GUID
  for each `-SessionName`. The GUID for the default name `EtwDnstap` is
  `{05e1db3c-1322-58c7-8235-0463fccba32b}`. Pass every `--session-name` this host uses, spelled
  exactly as in the binary path. The name is hashed without case folding. The
  script grants no right to log events. It also removes any other ACE for the
  service SID from every ETW descriptor, including grants from earlier versions
  of the script. Run it with `-WhatIf` first. That run lists each change and
  changes nothing. Both runs print the SDDL of each session GUID and of the
  default descriptor. After a `-WhatIf` run, that SDDL is unchanged.
  Before the first change, it exports the `WMI\Security` registry key and saves
  the original bytes of each value it changes into `-BackupPath`. A run with
  `-WhatIf` writes no backup. To undo the run, use `-Restore` as described in
  [scripts/provision/README.md](../../scripts/provision/README.md#backup-and-restore).
- `New-EventLog` registers the `EtwDnstap` source. Without it, service failures
  and the capture start and stop lines are not written to the Application log.
  The lab confirmed that the restricted token can write these events.
- `Restart-Computer` is required. ETW applies its security descriptors at boot.
  In the lab, a grant applied without a restart had no effect.

If the source already exists, `New-EventLog` fails. Check it first with
`[System.Diagnostics.EventLog]::SourceExists('EtwDnstap')`.

### If an ETW access check is denied

- **`StartTraceW: access denied`:** the service SID lacks
  `TRACELOG_CREATE_REALTIME` on the session GUID the message names, or the
  computer has not restarted since the grant. A changed `--session-name` also
  changes the GUID. Run `scripts\provision\Grant-EtwDnstapEtwAccess.ps1
  -SessionName <name>` with the name from the message, restart the computer,
  then start the service.
- **`EnableTraceEx2: access denied`:** this did not occur in the lab. The
  default descriptor already grants LocalService and Performance Log Users
  `TRACELOG_GUID_ENABLE`. A denial may mean this host has a descriptor of its
  own for the DNS provider GUID, or differs from the lab in another way. Inspect
  `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security\eb79061a-a566-4698-9119-3ed2807060e7`
  and record what it holds. Do not add a grant on the provider GUID. The lab
  needed none, and an earlier version of the grant script created a provider
  descriptor by granting on it.

Record the OS build and the descriptor that was found.

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
stopped fails at startup, unless the service depends on `DNS` (see
[Start type](#start-type)). The SCM counts every failure, including a refused
start, so the delay is 60 seconds from the third failure on. See
[troubleshooting.md](troubleshooting.md#the-service-keeps-restarting).

Check the policy:

```powershell
sc.exe qfailure EtwDnstap
sc.exe qfailureflag EtwDnstap
```

#### Start type

`New-Service` in step 3 sets the start type to Manual. Manual keeps the service
stopped until provisioning is complete. After the reboot in step 4, a Manual
service stays stopped, so capture is off until someone starts it. When
[step 7](#7-start-capture-and-check-that-it-is-running) shows `RUNNING`, set the
start type to delayed automatic start:

```powershell
cmd.exe /c 'sc.exe config EtwDnstap start= delayed-auto depend= DNS'
sc.exe qc EtwDnstap
```

- `depend= DNS` keeps the dependency on the `DNS` service. Include it so that
  the dependency does not change.
- `sc.exe qc` should show `START_TYPE` as `AUTO_START  (DELAYED)`. The lab
  confirmed this. `Get-Service` shows `Automatic`.
- With `depend= DNS`, the SCM stops `EtwDnstap` whenever it stops `DNS` on
  request. `Restart-Service DNS` fails because `DNS` has a dependent service.
  Use `Restart-Service DNS -Force`. In the lab, this command stopped `EtwDnstap`
  with exit code 0 and restarted `DNS` and `EtwDnstap`. Capture of the new DNS
  Server process started in less than 2 seconds. After
  `Stop-Service DNS -Force` and `Start-Service DNS`, `EtwDnstap` stays stopped.
  Start it with `Start-Service EtwDnstap`.
- If the DNS Server process crashes, the `DNS` dependency does not stop
  `EtwDnstap`. The service stops itself with a service-specific error about 5
  seconds later, and the recovery policy restarts it 5 seconds after that. The
  lab showed that this restart also starts `DNS` again, because of the
  dependency.
- Do not set this before step 7 succeeds. A service that fails at startup would
  then fail again at each boot.

### 6. Inspect the registered identity and access controls

```powershell
sc.exe qc EtwDnstap
sc.exe qsidtype EtwDnstap
sc.exe qprivs EtwDnstap
net localgroup "Performance Log Users"
sc.exe sdshow EtwDnstap
icacls.exe $installDirectory
$expectedSd = 'D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLORC;;;IU)(A;;CCLCSWLORC;;;SU)S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)'
$actualSd = (sc.exe sdshow EtwDnstap | Where-Object { $_.Trim() }) -join ''
if ($actualSd -ceq $expectedSd) { 'Service descriptor: as expected.' }
else { Write-Warning "Service descriptor differs. Found: $actualSd" }
```

Check each of these results:

- Each command succeeds.
- `sc.exe qc` shows `SERVICE_START_NAME` as `NT AUTHORITY\LocalService`.
- `sc.exe qsidtype` reports `RESTRICTED`.
- `sc.exe qprivs` lists only `SeChangeNotifyPrivilege`.
- Performance Log Users lists `NT SERVICE\EtwDnstap`.
- Only SYSTEM and Administrators can change the configuration or start and stop
  the service.
- The `sdshow` output equals the string from step 4 exactly.

A difference means that someone
changed the descriptor, or that this OS build prints it in another form. Record
the found string, and do not start the service until you know which.

### 7. Start capture and check that it is running

```powershell
sc.exe start EtwDnstap
sc.exe query EtwDnstap
```

Repeat `sc.exe query EtwDnstap` while the state is `START_PENDING`. It should
reach `RUNNING`. If it does not, see
[troubleshooting.md](troubleshooting.md#the-service-does-not-reach-running).
Look for a `capture started` line (Event ID 2) in the Application log. Then set
the start type as described in [step 5](#start-type).

### 8. Stop the service after the traffic you want

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

The service should reach `STOPPED` with exit code 0. Check that the collector
received the expected records. The service writes no capture, metrics, or log
files. Retention is the collector's job.

### Validation checklist for the LocalService restricted profile

Complete this checklist on a disposable DNS host before you rely on the profile
on a build other than 26100. Record each result with the binary's commit. Items
marked *lab* passed on build 26100. See
[localservice-restricted-validation.md](../history/localservice-restricted-validation.md).

- *lab:* The service reaches `RUNNING`, and `capture started` shows the
  expected keyword masks without `keyword_fallback` set.
- *lab:* `foreign_pid_rejected` stays 0 under DNS traffic, and `received` rises.
- *lab:* With only the `0x20` grant and a restart, `StartTraceW` and
  `EnableTraceEx2` are not denied. Without the grant, `StartTraceW` is denied.
  If either is denied, follow
  [the ETW access step](#if-an-etw-access-check-is-denied).
- *lab:* Application log writes work: Event ID 2 (`capture started`) appears.
- *lab:* Under DNS traffic, `receiver_connected` is true, and the collector
  receives the records.
- *lab:* After `sc.exe stop`, the service reaches `STOPPED` with exit code 0,
  and the final `output stopped` line reports `output_errors` 0.
- *lab:* After the process is killed, the next start reclaims the stale session
  (`stale_sessions_reclaimed=1`). The service reclaims a session only if its
  name, its real-time shape, and its session GUID all match.
- *lab:* A DNS Server restart or crash produces a gap and an automatic
  EtwDnstap restart under the recovery policy. In the lab, the service stopped
  4 to 5 seconds after the restart. Capture resumed with the new process ID
  5 seconds later, 9 to 10 seconds in total.
- While the service runs, a second `sc.exe start EtwDnstap` fails with error
  1056. Only one instance runs because the SCM starts one process for the
  service and the identity check refuses any other process. The service has no
  instance mutex.
- *lab:* The SCM query of the `DNS` service (`OpenSCManagerW`, `OpenServiceW`,
  `QueryServiceStatusEx`) succeeds under the restricted token.
- *lab:* `sc.exe sidtype EtwDnstap unrestricted` (a negative test) makes the
  service stop at startup with `the process token is not restricted`.

## Receiver configuration

The service keeps one outbound TCP connection to `--receiver`. It negotiates
content type `protobuf:dnstap.Dnstap` with READY, ACCEPT, and START, then sends
data frames. On shutdown it sends STOP and waits for FINISH. After a failure it
reconnects and negotiates again. Frames lost in the gap are counted, not resent.
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
ACLs, and recovery policy as they are. Change only what you need. Take the
snapshot first. Do not reset a value to a guessed default. Restore the value
that the snapshot holds.

### Snapshot before a change

First run the functions from
[Create a protected snapshot folder](#create-a-protected-snapshot-folder).

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
$backup = New-ProtectedBackupFolder
foreach ($query in 'qc', 'qsidtype', 'qprivs', 'qfailure', 'qfailureflag', 'sdshow') {
    sc.exe $query EtwDnstap | Set-Content -Path "$backup\$query.txt" -Encoding ascii
}
reg.exe export HKLM\SYSTEM\CurrentControlSet\Services\EtwDnstap "$backup\service.reg" /y
reg.exe export HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security "$backup\wmi-security.reg" /y
icacls.exe $installDirectory /save "$backup\install.acl" /t
net localgroup "Performance Log Users" | Set-Content -Path "$backup\plu.txt" -Encoding ascii
Get-FileHash "$installDirectory\etw-dnstap.exe" -Algorithm SHA256 |
    Format-List | Out-String | Set-Content -Path "$backup\exe-hash.txt" -Encoding ascii
Copy-Item "$installDirectory\etw-dnstap.exe" $backup
```

The folder holds these items:

- The account, the SID type, and the privileges.
- The recovery policy and the service descriptor.
- The directory ACLs and the group membership.
- A copy of the executable.

Only SYSTEM and Administrators can read it. The `.reg`
files are for reference. Do not restore the service from them (see below).
When you run the grant script in a repair, add `-BackupPath "$backup\etw"` so
that its backup is in the same folder.

### Restore from the snapshot

First check the snapshot folder again. Someone may have changed it since you
made it. Run the functions from
[Create a protected snapshot folder](#create-a-protected-snapshot-folder), set
`$backup` to the snapshot folder, and run this check. Stop if it throws, and do
not use any file from the folder.

```powershell
$backup = '<snapshot folder>'
$backupRoot = Join-Path $env:ProgramData 'EtwDnstap\backup'
if (-not $backup.StartsWith($backupRoot + '\', 'OrdinalIgnoreCase')) { throw 'Not a snapshot folder.' }
foreach ($dir in (Split-Path -Parent $backupRoot), $backupRoot, $backup) { Test-ProtectedFolder $dir }
foreach ($file in Get-ChildItem -LiteralPath $backup -Recurse -Force) {
    if ($file.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Reparse point: $($file.FullName)" }
    $owner = (Get-Acl -LiteralPath $file.FullName).GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($trustedSids -notcontains $owner) { throw "Unexpected owner $owner on $($file.FullName)" }
}
```

Do not use `reg import` for the service key. The SCM keeps the service
configuration in memory, and a registry import can leave the SCM and the
registry in disagreement. Use `sc.exe` instead. Read the values from the
`.txt` files in the snapshot folder. Replace each `<...>` with the value in the
file.

```powershell
sc.exe stop EtwDnstap
cmd.exe /c 'sc.exe config EtwDnstap binPath= "<BINARY_PATH_NAME from qc.txt>" start= <demand|auto|delayed-auto> obj= "<SERVICE_START_NAME from qc.txt>" password= ""'
sc.exe sidtype EtwDnstap <none|unrestricted|restricted>      # from qsidtype.txt
sc.exe privs EtwDnstap <privileges from qprivs.txt, separated by />
sc.exe failure EtwDnstap reset= <seconds> actions= <actions from qfailure.txt>
sc.exe failureflag EtwDnstap <0|1>                           # from qfailureflag.txt
$sd = (Get-Content "$backup\sdshow.txt" | Where-Object { $_.Trim() }) -join ''
cmd.exe /c ('sc.exe sdset EtwDnstap "{0}"' -f $sd)
icacls.exe 'C:\Program Files' /restore "$backup\install.acl"
```

Then restore the other parts of the snapshot:

1. Run `net localgroup "Performance Log Users"` and compare it with
   `plu.txt`. Add or remove `NT SERVICE\EtwDnstap` so that they match.
2. If a repair ran the grant script, run it with `-Restore "$backup\etw"`.
   The script checks the folders and `changes.json`, validates every entry
   before it applies any, and skips a value that changed after the run that saved
   it. Use `-Force` only after you have looked at the skipped values.
3. If you replaced the executable, put the saved copy back (see below).
4. Run `Restart-Computer` if the grant script restored any descriptor. ETW
   applies descriptors at boot.
5. Run the checks in [step 6](#6-inspect-the-registered-identity-and-access-controls)
   and compare the result with the saved `.txt` files.

`icacls /restore` runs on the parent folder, because `/save` stores paths
relative to the parent.

### Repair cases

- **Upgrading from LocalSystem or the virtual account.** A service still
  registered as LocalSystem or `NT SERVICE\EtwDnstap` refuses to run with this
  build. Do all of these, in this order:
  1. Take the snapshot.
  2. Set the identity, the SID type, and the privileges with the `sc.exe`
     lines from
     [step 4](#4-set-the-service-identity-privileges-etw-access-and-event-source).
  3. Add `NT SERVICE\EtwDnstap` to Performance Log Users with `net localgroup`.
     If it is already a member, the command reports an error. That is fine.
  4. Run `Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` for every session
     name, with `-WhatIf` first. Then run `Restart-Computer`. Without the
     grant and the restart, the start fails with `StartTraceW: access denied`.
  5. Keep the service SID's read and execute grant from
     [step 3](#3-copy-the-executable-register-the-service-and-grant-read-and-execute).
     Keep the recovery policy from [step 5](#5-configure-scm-recovery).
  6. Start the service. Confirm the account with `sc.exe qc EtwDnstap`.

  In the lab on build 26100, `obj=` did not clear the SID type or the privilege
  list. This held for a change to the same account, and for a change to
  LocalSystem and back. After any `obj=` change, still run
  `sc.exe sidtype EtwDnstap restricted` and check it with
  `sc.exe qsidtype EtwDnstap`. The check costs little.
- **Upgrading from the first version of the grant script** (commit `6718863`).
  That version granted `0x423` on the default GUID and `0x80` on the DNS
  provider GUID, and created a descriptor of its own for the provider. Replace
  the executable, run the current script with `-SessionName`, then restart the
  computer. The script removes the old grants, deletes the provider
  descriptor once it matches the default again, and grants on the session GUID.
  The lab tested this path.
- **Changing `--session-name`.** The session GUID follows the name. Run the
  grant script with the new name (and any others still in use), then restart
  the computer before starting the service.
- **Upgrading from a build with `--output`, `--max-output-bytes`, or `--metrics`.**
  Remove those arguments from the binary path before starting the new
  executable. They now fail command-line parsing. Add `--receiver` with the
  collector's numeric IP and port.
- **Changing the binary path.** Use `sc.exe config EtwDnstap binPath= "..."`,
  keeping the space after `binPath=`.

### Replace the executable

Take the snapshot first. Then:

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap          # wait for STOPPED
$exe = "$installDirectory\etw-dnstap.exe"
Get-FileHash $exe -Algorithm SHA256
Rename-Item $exe 'etw-dnstap.exe.bak'
Copy-Item '.\target\x86_64-pc-windows-msvc\release\etw-dnstap.exe' $installDirectory
Get-FileHash $exe -Algorithm SHA256
icacls.exe $exe
sc.exe start EtwDnstap
sc.exe query EtwDnstap
```

- The first hash is the old executable. Compare it with `exe-hash.txt`. The
  second hash is the new executable. Compare it with the hash of the file you
  built.
- `icacls` must show `NT SERVICE\EtwDnstap:(I)(RX)` for the new file. If it does
  not, the service cannot start. Do not change the other ACLs. Fix only the
  service SID's read and execute grant from step 3.
- The service must reach `RUNNING`, and a `capture started` line must appear.

To roll back, stop the service, delete the new file, rename the backup back,
and start the service:

```powershell
sc.exe stop EtwDnstap
Remove-Item $exe
Rename-Item "$exe.bak" 'etw-dnstap.exe'
sc.exe start EtwDnstap
```

When the new executable works, delete `etw-dnstap.exe.bak`. The copy in the
snapshot folder stays until you remove the folder.

## Related

- [Configuration reference](configuration.md)
- [Troubleshooting](troubleshooting.md)
- [Uninstall](uninstall.md)
- [Pooled ETW intake](../design/pooled-intake.md) for the memory budget
