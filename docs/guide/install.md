# Install and provision the service

> **Status:** Current · **Describes:** the MSI sources at commit `58fde72` · **Reviewed:** 2026-11-03

Use this guide to install the service on a DNS Server host. It covers the
service identity, the ETW access grant, the SCM registration, recovery, and the
first start. It assumes you already built the executable.

**Validation status.** The LocalService restricted-SID profile was validated on
one lab DNS Server (Windows Server 2025, build 26100) on 2026-11-01. See
[localservice-restricted-validation.md](../history/localservice-restricted-validation.md).
Load and other Windows builds were not covered. A later lab run tested the SCM
recovery policy and a DNS Server restart. See
[the lab record](../history/skills-evaluation-lab-validation.md). On other
builds, run the [validation checklist](#validation-checklist-for-the-localservice-restricted-profile)
on a disposable host first. Steps 3, 4, and 7 ran on the lab host. An
administrator ran the performance counter commands in steps 3 and 7 there on
2026-11-01. The other steps come from the source and the reviewed lab
procedure.

| Task | Guide |
| --- | --- |
| Build the executable | [Build the executable](build.md) |
| Prepare the receiver and its certificate | [Set up the dnstap receiver](receiver.md) |
| Install with the MSI | [Install with the MSI](#install-with-the-msi) |
| Install by hand | This guide, steps 1 to 8 |
| Test the profile on a new Windows build | [Validation checklist](#validation-checklist-for-the-localservice-restricted-profile) |
| Change an existing installation | [Repair or upgrade](upgrade.md) |
| Set options | [Configuration reference](configuration.md) |
| Remove the service | [Uninstall](uninstall.md) |

## Install with the MSI

The MSI does the provisioning that the manual steps below do. It installs the
files, sets the folder ACLs, registers and configures the service, grants ETW
access, registers the performance counters and the Application event log
source, and creates the data folders. It does not start the service. A restart
starts it.

Use the manual steps on a host where the MSI is not used, or to see what each
MSI action changes. The manual steps stay the reference.

Lab validation: passed on one lab DNS Server (Windows Server 2025, build 26100)
on 2026-11-03: install, a failed install and its rollback, repair, a receiver
change, and uninstall, each with a restart. A major upgrade from an older MSI
has not been tested. See [the plan and its results](../history/msi-installer-plan.md).

### MSI prerequisites

- Windows x64 with the **DNS Server role** installed. The MSI stops if the
  `DNS` service is not registered. It does not check that the service is
  Running, and the EtwDnstap service depends on `DNS`.
- A **receiver** address for the `RECEIVER` property: the collector's numeric
  IPv4 or `[IPv6]` address, a colon, and a port from 1 to 65535. Hostnames are
  refused. The receiver does not need to run before the install.
- The **receiver CA** in the Windows machine certificate store, for the default
  TLS connection. The MSI does not install it. See
  [Set up the dnstap receiver](receiver.md#receiver-certificate). To send
  plaintext on a trusted network, pass `SERVICE_ARGS=--plaintext`.
- A host with no hand-provisioned `EtwDnstap` service. Remove a manual install
  first. See [Remove a hand-provisioned install](uninstall.md#remove-a-hand-provisioned-install).
- An elevated session, and the MSI file `etw-dnstap-<version>-x64.msi`. To build
  it, see [Build the MSI](build.md#build-the-msi).

### Run the installer

Run the command from the folder that holds the MSI. The log file name is any
name you can write to.

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v install.log RECEIVER=192.0.2.10:6000
```

The MSI has three public properties:

| Property | Required | Default | Meaning |
| --- | --- | --- | --- |
| `RECEIVER` | Yes, unless a value was remembered | None | The collector's `IP:port`, as in step 1 of the manual steps. It becomes the `--receiver` argument. |
| `SESSION_NAME` | No | `EtwDnstap` | The ETW session name, which becomes `--session-name`. It takes 1 to 128 ASCII letters, digits, `_`, `.`, and `-`. It must not start with `-`. It is case-sensitive. A launch condition refuses a value that starts with `-` before the install changes anything, with the message `SESSION_NAME must not start with '-'. Use 1 to 128 ASCII letters, digits, '_', '.', or '-', and do not start with '-'.` A launch condition cannot test the length or the character set, so the provisioning script rejects any other invalid value, and the install fails and rolls back. A removal skips the launch condition. No real `msiexec` run has shown the message yet. |
| `SERVICE_ARGS` | No | Empty | More arguments for the service's binary path, for example `--plaintext`. The MSI does not check them. The MSI creates only the default spool folder, so a `--spool-dir` folder is yours to create. See the [Configuration reference](configuration.md). |

To set all three, add them to the same command:

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v install.log `
    RECEIVER=192.0.2.10:6000 SESSION_NAME=EtwDnstap-Lab SERVICE_ARGS=--plaintext
```

If a value has spaces, put the whole property in quotes, for example
`'SERVICE_ARGS="--plaintext --capacity 24576"'`.

The MSI keeps these values in `HKLM\SOFTWARE\EtwDnstap\Installer`. Repair and
upgrade read them back. See [Upgrade, repair, or change settings with the MSI](upgrade.md#upgrade-repair-or-change-settings-with-the-msi).

The provisioning steps (the folder checks, the group membership, the ETW grant,
and the recovery policy) run only in a pass that installs the executable
component: a first install, a major upgrade, a repair, or `REINSTALL=ALL`. A
plain `msiexec /i` of an installed product with a changed `RECEIVER` or
`SESSION_NAME` and no `REINSTALL` skips them and changes nothing. See
[When the install steps run](upgrade.md#when-the-install-steps-run).

### Restart the computer

A successful install exits with code `3010`. That code means the install
succeeded and a restart is required. `/norestart` stops `msiexec` from
restarting the computer, so restart it yourself.

Restart the computer. ETW applies the session grant only at boot (step 4 of the
manual steps). Until the restart, the grant has no effect.

The MSI sets the service to delayed automatic start and does not start it. After
the restart, Windows starts `EtwDnstap` a short time after boot, once `DNS` is
running. Do not start it by hand unless it is not `RUNNING` five minutes after the
restart. If it is not running, see
[troubleshooting.md](troubleshooting.md#the-service-does-not-reach-running).

If an action fails during a first install, Windows Installer rolls back. It
runs the uninstall provisioning for the changes the install made, and it
removes the files. On an upgrade, the rollback restores the previous files and
service settings, but it does not undo the provisioning steps. Fix the cause
and run the install again. The provisioning steps can run again.

If the install fails, `msiexec` exits with a non-zero code and the log says why. In
the lab, a missing `RECEIVER` and a hostname `RECEIVER` each ended with `1603`.

### Check the result

After the restart, run these in an elevated Windows PowerShell 5.1 session:

```powershell
sc.exe query EtwDnstap
sc.exe qc EtwDnstap
sc.exe qsidtype EtwDnstap
sc.exe qprivs EtwDnstap
typeperf -q EtwDnstap
```

- `sc.exe query` shows `RUNNING` after the delayed start.
- `sc.exe qc` shows `START_TYPE` as `AUTO_START  (DELAYED)`, `SERVICE_START_NAME`
  as `NT AUTHORITY\LocalService`, and `DEPENDENCIES` as `DNS`.
- `sc.exe qsidtype` shows `RESTRICTED`. `sc.exe qprivs` lists only
  `SeChangeNotifyPrivilege`.
- `typeperf -q EtwDnstap` lists the counter names.
- The Application log has Event ID 2, `capture started`. Its `version` field
  matches the version in the MSI file name.
- The MSI log, `install.log`, holds the output of the provisioning script. The
  lines that start with `WixQuietExec:` after the `ProvisionInstall` action say
  what each step did. A line `WixQuietExec:   FAILED: <cause>` gives the cause.

Then check the service as in [step 6](#6-inspect-the-registered-identity-and-access-controls)
and [step 7](#7-start-capture-and-check-that-it-is-running), without starting it
by hand.

### What the MSI does in place of the manual steps

| MSI action | Manual step | What the action does |
| --- | --- | --- |
| Launch conditions | Prerequisites | Check 64-bit Windows, the `DNS` service registration, that `RECEIVER` is set or remembered, and that `SESSION_NAME` does not start with `-`. |
| Provisioning, receiver check | Step 1 | Runs the same check as step 1 on `RECEIVER`. A bad value stops the install. |
| `InstallFolderAcl` component | Step 2 | Creates `C:\Program Files\EtwDnstap\` with SYSTEM and Administrators full control and no inherited ACEs. |
| `EtwDnstapExe`, `EtwDnstapManifest`, `GrantScript`, `ProvisioningScript` components | Step 3 (copy files) | Copy the executable, the counter manifest, the grant script, and the provisioning script. The service SID gets read and execute on the folder. |
| `util:PerfCounterManifest` | Step 3 (`lodctr`) | Registers the counter set. Uninstall removes the registration. |
| `ServiceInstall` | Steps 3 and 4 (`New-Service`, `sc.exe config`, `sc.exe sdset`) | Creates the service as `NT AUTHORITY\LocalService`, with the step 4 descriptor and the `DNS` dependency. |
| `ServiceConfig` | Step 4 (`sc.exe sidtype`, `sc.exe privs`), step 5 (failure flag), and the start type | Sets the restricted SID type, only `SeChangeNotifyPrivilege`, delayed automatic start, and the non-crash failure flag. |
| Provisioning, recovery policy | Step 5 (`sc.exe failure`) | Runs the step 5 `sc.exe failure` command: reset after 86400 seconds, restart after 5 seconds, 5 seconds, and 60 seconds. The MSI service failure-actions table is not used, because on the lab host it failed with access denied (error 1939). |
| Provisioning, group step | Step 4 (`net localgroup ... /add`) | Adds `NT SERVICE\EtwDnstap` to Performance Log Users. Skips it if already a member. The membership check reads the group by account name first. If that throws, the script prints `The name-based membership check on <group> failed (<error>). Reading the members by SID instead.` and reads the members by SID through a small `Add-Type` class that it compiles once. If the compile fails, for example because application control blocks it, the install fails with `Could not read the members of <group>. By name: <error> By SID: <error>`. The lab has not run the fallback. |
| Provisioning, grant step | Step 4 (grant script) | Runs `Grant-EtwDnstapEtwAccess.ps1 -SessionName` from the install folder. |
| `util:EventSource` | Step 4 (`New-EventLog`) | Registers the `EtwDnstap` source in the Application log, with the same message file. |
| Provisioning, data folders | Create the spool folder, and the `ProgramData` part of the snapshot folder | Creates `C:\ProgramData\EtwDnstap` and `spool` with the rights of the spool section, or accepts them if they exist and pass the checks. An existing folder that fails a check stops the install. |
| `ScheduleReboot` | Step 4 (`Restart-Computer`) | Schedules the restart, which the install requires. |
| `RememberedSettings` component | None | Keeps `RECEIVER`, `SESSION_NAME`, and `SERVICE_ARGS` in the registry for repair and upgrade. |
| No start action | Step 7 (`sc.exe start`) | The service starts after the restart, with delayed automatic start. |
| No checks | Step 6 | The MSI does not run the step 6 checks. Use [Check the result](#check-the-result). |

The grant script also writes a snapshot of the ETW descriptors under
`C:\ProgramData\EtwDnstap\backup` when it changes a descriptor. Uninstall
deletes that folder. See [Uninstall with the MSI](uninstall.md#uninstall-with-the-msi).

## Prerequisites

- Windows x64 with the **DNS Server role** installed, and the `DNS` service
  Running. The service depends on the `DNS` service. It refuses to start while
  the `DNS` service is not running, and it has nothing to capture without it.
- The release executable. See [Build the executable](build.md).
- A dnstap receiver that accepts Frame Streams over TCP and speaks content type
  `protobuf:dnstap.Dnstap`. The lab used
  [DNS-collector v3.2.3](https://github.com/dmachard/DNS-collector/releases/tag/v3.2.3).
  For TLS, import the receiver CA first. See
  [Set up the dnstap receiver](receiver.md).
- An elevated Windows PowerShell 5.1 session for the provisioning steps.
  Run `New-EventLog` and `Remove-EventLog` from Windows PowerShell.

## Provision the service

Use these manual steps on a host without the MSI, or to see what each MSI action
changes. For the MSI, see [Install with the MSI](#install-with-the-msi).

Use an elevated Windows PowerShell 5.1 session at the repository root. The steps
assume the service and the installation directory do not exist yet. For an
existing installation, see [Repair or upgrade](upgrade.md).

The service runs as `NT AUTHORITY\LocalService` (`S-1-5-19`) with a
**restricted** service SID type. Windows calls the resulting token
*write-restricted*. The name refers to Windows access control, not to ETW data:
the service only reads events. At startup it requires all of the following, and
refuses to start otherwise:

- The token user is `S-1-5-19`.
- The service SID `NT SERVICE\EtwDnstap` (`S-1-5-80-...`) is an enabled group.
- The token is restricted. Its restricting SIDs include the service SID and
  the Write Restricted SID `S-1-5-33`. The SCM also adds Everyone and the logon
  SID.
- Performance Log Users (`S-1-5-32-559`) is an enabled group. The token gets
  this group because the service SID is a member of it.
- The only privilege is an enabled `SeChangeNotifyPrivilege`.

For any access that Windows classes as a write (a change of state), a
write-restricted token must also pass a second access test. That test considers
only the restricting SIDs. Creating the service's own real-time trace session is
such a change. LocalService and Performance Log Users do not count for that test, so
step 4 grants `TRACELOG_CREATE_REALTIME` directly to the service SID. Every
other ETW call the service makes passed in the lab without a grant: enabling
the provider, consuming the session, and querying and stopping it.

The grant covers only the service's own session. The service starts its ETW
session with a stable GUID derived from `--session-name`, and ETW tests the
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
$addressText = $null
$portText = $null
if ($receiverText -match '^\[([^\]]+)\]:(\d{1,5})$') {
    $addressText = $Matches[1]
    $portText = $Matches[2]
}
elseif ($receiverText -match '^([0-9.]+):(\d{1,5})$') {
    $addressText = $Matches[1]
    $portText = $Matches[2]
}
$receiverAddress = $null
if ($null -eq $addressText -or -not [System.Net.IPAddress]::TryParse($addressText, [ref]$receiverAddress)) {
    throw 'Receiver must be a numeric IP address and port, for example 192.0.2.10:6000.'
}
$receiverPort = [int]$portText
$firstOctet = if ($receiverAddress.AddressFamily -eq 'InterNetwork') { $receiverAddress.GetAddressBytes()[0] } else { 0 }
$isMulticast = ($receiverAddress.AddressFamily -eq 'InterNetworkV6' -and $receiverAddress.IsIPv6Multicast) -or
    ($firstOctet -ge 224 -and $firstOctet -le 239)
if ($receiverPort -eq 0 -or $receiverPort -gt 65535 -or
    $receiverAddress.Equals([System.Net.IPAddress]::Any) -or
    $receiverAddress.Equals([System.Net.IPAddress]::IPv6Any) -or
    $isMulticast) {
    throw 'Receiver must have a unicast IP address and a nonzero port.'
}
# IPv6 is written as [address]:port
$Receiver = if ($receiverAddress.AddressFamily -eq 'InterNetworkV6') {
    "[$($receiverAddress)]:$receiverPort"
} else {
    "$($receiverAddress):$receiverPort"
}
```

This check mirrors `check_receiver` in `src/pipeline/config.rs`. Windows
PowerShell 5.1 (.NET Framework) has no `[System.Net.IPEndPoint]::TryParse`, so
the block splits the endpoint and parses the address with
`[System.Net.IPAddress]::TryParse`. The logic is the same as `Test-Receiver` in
[Invoke-EtwDnstapProvisioning.ps1](../../packaging/msi/Invoke-EtwDnstapProvisioning.ps1),
which the MSI runs. It accepts a numeric IP
address with a nonzero port that is not unspecified and not multicast. It does not test
reachability. Test that with a connection test to the collector.

### 2. Create the protected installation directory

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
New-Item -ItemType Directory -Path $installDirectory -ErrorAction Stop
icacls.exe $installDirectory /inheritance:r /grant:r `
    '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F'
```

Only SYSTEM and Administrators keep write access. Ordinary users must not be
able to replace the executable. The service SID gets read and execute in
step 3. No write ACE is needed here. The service writes no files in this
folder. Its spool folder is separate, and [Create the spool folder](#create-the-spool-folder)
sets it up.

### 3. Copy the files, register the service and counters, and grant read and execute

```powershell
Copy-Item '.\target\x86_64-pc-windows-msvc\release\etw-dnstap.exe' $installDirectory
Copy-Item '.\packaging\perfcounters\etw-dnstap.man' $installDirectory
$imagePath = '"{0}\etw-dnstap.exe" --receiver "{1}" --capacity 24576' -f `
    $installDirectory, $Receiver
New-Service -Name EtwDnstap -BinaryPathName $imagePath `
    -StartupType Manual -DependsOn DNS
icacls.exe $installDirectory /grant 'NT SERVICE\EtwDnstap:(OI)(CI)(RX)'
lodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man" "C:\Program Files\EtwDnstap"
```

Arguments go in the binary path. Arguments passed to `sc.exe start` are not used
as configuration. The `--capacity 24576` shown here is the default. It is written
out so the value is visible. Add other options from
[configuration.md](configuration.md) to the same string. The connection is TLS
by default, so complete the [receiver certificate step](receiver.md#receiver-certificate)
first. To send plaintext to a receiver on a trusted network, add `--plaintext`
to the string instead.

The spool is on by default. The service uses the folder from
[Create the spool folder](#create-the-spool-folder). Step 3 does not create that
folder. Until the folder exists and passes the service's checks, the service
logs one warning and runs without a spool. To run without a spool, add
`--no-spool` to the same string.

The name `NT SERVICE\EtwDnstap` resolves once the service exists, so the
`icacls` grant comes after `New-Service`. Step 2 removed inherited ACEs. Without
this grant, the copied executable is not readable by the service, and the
service cannot start. Read and execute is not a write, so this grant to the
service SID is enough. The lab test showed this.

The manifest, `etw-dnstap.man`, defines the performance counter set. `lodctr`
reads it from the protected directory and registers the counter set. The
counter instance appears only while the service runs. If the manifest is not
registered, the service captures normally and logs nothing about it, but
consumers cannot find the counter set: `typeperf -q EtwDnstap` lists nothing. To
register the manifest, run `lodctr` again and restart the service.

The lab validated this step on 2026-11-01 with the LocalService restricted-SID
profile. The counters were read by an administrator. Reading by a non-admin
member of Performance Monitor Users has not been tested. Consumers load the
counter names from `etw-dnstap.exe`, and the step 2 ACL does not grant that group
read access.

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

### Create the spool folder

The spool is on by default. The service keeps frames that the receiver has not
accepted yet in segment files in the spool folder. The default folder is
`C:\ProgramData\EtwDnstap\spool`. The service uses the folder only when it
exists and passes the service's own checks. The service never creates the
folder. Run this step after step 3. The grant below names the service SID that
step 3 creates.

The service trusts only a folder that an administrator set up with a known owner
and ACL. A folder the service created would be owned by the service account, which fails
the owner check, and the service has no right to create it there. This step sets
the owner and the ACL on purpose, and it checks them. It also stops
if the folder already exists, so an existing folder is never reused without a
look. Inspect such a folder first. Do not take ownership of it.

Define `Test-SpoolFolder` in the same PowerShell session as the snapshot
functions. It replaces `Test-ProtectedFolder` for this folder.

```powershell
function Test-SpoolFolder {
    param([string]$Path)
    $item = Get-Item -LiteralPath $Path -Force -ErrorAction Stop
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "'$Path' is a reparse point. Inspect it. Do not use it."
    }
    Test-ProtectedFolder (Split-Path -Parent $Path)
    $acl = Get-Acl -LiteralPath $Path
    $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    if ($trustedSids -notcontains $owner) {
        throw "'$Path' is owned by $owner. Inspect it. Do not use it."
    }
    $serviceSid = (New-Object Security.Principal.NTAccount 'NT SERVICE\EtwDnstap').
        Translate([Security.Principal.SecurityIdentifier]).Value
    $rules = @($acl.GetAccessRules($true, $true, [Security.Principal.SecurityIdentifier]))
    $extra = @($rules | Where-Object {
            $trustedSids -notcontains $_.IdentityReference.Value -and
            $_.IdentityReference.Value -ne $serviceSid
        })
    if ($extra.Count -gt 0) {
        throw "'$Path' has an ACE for $($extra[0].IdentityReference). Inspect it. Do not use it."
    }
    # The service gets two entries: the folder itself (read and execute, and
    # write, but not delete), and Modify on the contents only.
    $service = @($rules | Where-Object { $_.IdentityReference.Value -eq $serviceSid })
    $own = @($service | Where-Object {
            $_.InheritanceFlags -eq [Security.AccessControl.InheritanceFlags]::None
        })
    $contents = @($service | Where-Object {
            $_.InheritanceFlags -ne [Security.AccessControl.InheritanceFlags]::None
        })
    $modify = [Security.AccessControl.FileSystemRights]::Modify -bor
        [Security.AccessControl.FileSystemRights]::Synchronize
    $container = [Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
        [Security.AccessControl.InheritanceFlags]::ObjectInherit
    $inheritOnly = [Security.AccessControl.PropagationFlags]::InheritOnly
    $ownRights = if ($own.Count -eq 1) { $own[0].FileSystemRights } else { 0 }
    if ($service.Count -ne 2 -or $own.Count -ne 1 -or $contents.Count -ne 1 -or
        ($ownRights -band [Security.AccessControl.FileSystemRights]::Delete) -ne 0 -or
        ($ownRights -band [Security.AccessControl.FileSystemRights]::WriteData) -eq 0 -or
        $contents[0].InheritanceFlags -ne $container -or
        $contents[0].PropagationFlags -ne $inheritOnly -or
        $contents[0].FileSystemRights -ne $modify) {
        throw "'$Path' does not give the service the expected rights. Inspect it."
    }
    'Spool folder: as expected.'
}
```

Then create the folder. Grant the service SID write and read and execute on the
folder itself, and Modify on the contents only. The service can then create and
delete segment files, but it cannot delete the folder:

```powershell
$spoolParent = Join-Path $env:ProgramData 'EtwDnstap'
$spoolDirectory = Join-Path $spoolParent 'spool'
if (-not (Test-Path -LiteralPath $spoolParent)) { New-ProtectedFolder $spoolParent }
Test-ProtectedFolder $spoolParent
New-ProtectedFolder $spoolDirectory
icacls.exe $spoolDirectory /grant 'NT SERVICE\EtwDnstap:(RX,W)' | Out-Null
icacls.exe $spoolDirectory /grant 'NT SERVICE\EtwDnstap:(OI)(CI)(IO)(M)' | Out-Null
Test-SpoolFolder $spoolDirectory
icacls.exe $spoolDirectory
```

`New-ProtectedFolder` checks the folder before the grant, so it passes. Do not
run `Test-ProtectedFolder` on the spool folder after the grant. It allows only
SYSTEM and Administrators, so it rejects the service's entries. Use
`Test-SpoolFolder`, which also runs `Test-ProtectedFolder` on the parent. The
grants cover this folder only. They do not change the parent folder or the
install folder.

The last `icacls.exe` command should list four entries, and none has `(I)`:

- `NT SERVICE\EtwDnstap` with write and read and execute, and with no `(OI)` or
  `(CI)` flags. This entry applies to the folder only.
- `NT SERVICE\EtwDnstap:(OI)(CI)(IO)(M)`. This entry applies to the contents only.
- `BUILTIN\Administrators:(OI)(CI)(F)`
- `NT AUTHORITY\SYSTEM:(OI)(CI)(F)`

The service runs a check of its own at startup. It refuses the spool, logs one
warning, and runs without a spool if any of these checks fails. The service's
check is looser than `Test-SpoolFolder`, which allows only the three accounts
above:

- The spool folder exists and is a folder. Its parent is not a reparse point,
  and the folder is not one either. Links and junctions are refused.
- The folder is on a fixed local disk. The path starts with a drive letter. It
  has no `.` or `..` part. Network and device paths are refused.
- SYSTEM or Administrators owns the folder.
- The folder has a DACL. No allow entry grants read or write access to an
  account other than SYSTEM, Administrators, or a service SID. An inherit-only
  `CREATOR OWNER` entry is allowed. Deny entries do not count.

The `icacls.exe` output is the check for other accounts. Run
`Test-SpoolFolder $spoolDirectory` again after any change to the folder.

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
- `privs` limits the service to `SeChangeNotifyPrivilege`. LocalService holds more
  privileges by default. The service refuses to run if the token holds any other
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
  same reason as `obj=`. That keeps the quotes and the parentheses of the string
  intact. The default descriptor also gives interactive and service logons the
  user-defined control right. The service does not use it, so the new descriptor
  omits it. In the lab on build 26100, `sc.exe sdshow EtwDnstap` printed exactly
  the string in step 6 after `sdset`. A standard user got "Access is denied"
  (error 5) from `sc.exe stop`, `start`, `config`, and `control EtwDnstap 200`.
  That user could query the service.
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
  changes nothing. Both runs print the SDDL of the descriptor of each session
  GUID that has one, and of the default descriptor. A `-WhatIf` run leaves that
  SDDL unchanged. Before the first change, the script exports the `WMI\Security`
  registry key. It also saves the original bytes of each value that it changes
  into `-BackupPath`. A run with `-WhatIf` writes no backup. To undo the run,
  use `-Restore` as described in
  [scripts/provision/README.md](../../scripts/provision/README.md#backup-and-restore).
- `New-EventLog` registers the `EtwDnstap` source in the Application log, with a
  message file. Event Viewer uses the registration to show the message text. The
  service opens this source name with `RegisterEventSourceW`. The lab test
  showed that the restricted token can write these events.
- `Restart-Computer` is required. ETW applies its security descriptors at boot.
  In the lab, a grant applied without a restart had no effect.

If the source already exists, `New-EventLog` fails. Check it first with
`[System.Diagnostics.EventLog]::SourceExists('EtwDnstap')`.

### If an ETW access check is denied

- **`StartTraceW: access denied`:** the service SID lacks
  `TRACELOG_CREATE_REALTIME` on the session GUID the message names, or the
  computer has not restarted since the grant. A changed `--session-name` also
  changes the GUID, and so does a change of case, because the name is hashed
  as written. Run `scripts\provision\Grant-EtwDnstapEtwAccess.ps1
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
  showed this. `Get-Service` shows `Automatic`.
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

A difference means that someone changed the descriptor, or that this OS build
prints it in another form. Record the string that you found. Do not start the
service until you know which.

### 7. Start capture and check that it is running

```powershell
sc.exe start EtwDnstap
sc.exe query EtwDnstap
```

Repeat `sc.exe query EtwDnstap` while the state is `START_PENDING`. It should
reach `RUNNING`. If it does not, see
[troubleshooting.md](troubleshooting.md#the-service-does-not-reach-running).
Look for a `capture started` line (Event ID 2) in the Application log.

Check that the performance counters are visible:

```powershell
typeperf -q EtwDnstap
typeperf "\EtwDnstap(*)\*" -sc 5
```

`typeperf -q` lists the counter names. The second command prints five samples
of every counter in every instance. The ETW gauges read 0 until the first ETW
query succeeds. If the names do not appear, the manifest is not registered: run
the `lodctr` command from step 3 again and restart the service. If counter
registration fails at startup, the service logs the warning `performance
counters are unavailable` (Event ID 3). Capture continues without counters.

Then set the start type as described in [step 5](#start-type).

### 8. Stop the service after the traffic you want

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

The service should reach `STOPPED` with exit code 0. Check that the collector
received the expected records. The service writes no capture or metrics files.
Retention is the collector's job for the records it received.

Frames that the receiver did not accept stay in the spool folder. A backlog is
not sent before the service exits, so it waits on disk for the next start. Check
`spool_segments` in the final `output stopped` line. It shows how many segment
files remain. The folder holds DNS data. Delete it when you no longer need the
frames. See [Uninstall](uninstall.md#7-delete-the-spool-folder).

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
- *lab:* The counters are visible. `typeperf -q EtwDnstap` lists the counter
  names, and `typeperf "\EtwDnstap(*)\*" -sc 5` shows values while the service
  runs. The totals match the `capture stopped` counters, and the instance is gone
  after the service stops.
- The spool. With the folder from [Create the spool folder](#create-the-spool-folder),
  stop the collector for a short time under DNS traffic. `spool_segments` and
  `spooled` rise while `receiver_connected` is false. After the collector
  returns, `replayed` rises and `spool_bytes` falls. The lab validation has not covered this check.


## Next steps

- [Set up the dnstap receiver](receiver.md)
- [Configuration reference](configuration.md)
- [Log events and counters](monitoring.md)
- [Troubleshooting](troubleshooting.md)
- [Repair or upgrade](upgrade.md)
- [Uninstall](uninstall.md)
