# Install and provision the service

> **Status:** Current · **Describes:** the MSI sources at commit `58fde72` · **Reviewed:** 2026-11-03

Install the service on a DNS Server host. Build the executable first
([Build the executable](build.md)), and prepare the receiver
([Set up the dnstap receiver](receiver.md)).

**Validation status.** The LocalService restricted-SID profile was validated on one lab DNS
Server (Windows Server 2025, build 26100). Steps 3, 4, and 7 ran there. Run the
[validation checklist](#validation-checklist-for-the-localservice-restricted-profile) on a
disposable host before you use another build. See [the lab record](../history/localservice-restricted-validation.md).

## Install with the MSI

The MSI does the provisioning that the manual steps describe. It does not start the service.
A restart starts it. Lab validation covered install, rollback, repair, a receiver change, and
uninstall. A major upgrade is untested. See [the plan and its results](../history/msi-installer-plan.md).

### MSI prerequisites

- Windows x64 with the **DNS Server role**. The MSI stops if the `DNS` service is not
  registered. It does not check that `DNS` runs.
- A `RECEIVER` value: a numeric IPv4 or `[IPv6]` address, a colon, and a port from 1 to
  65535. Hostnames are refused.
- For TLS, the **receiver CA** in the Windows machine certificate store. The MSI does not
  install it. See [Set up the dnstap receiver](receiver.md#receiver-certificate). For
  plaintext, pass `SERVICE_ARGS=--plaintext`.
- No hand-provisioned `EtwDnstap` service. See
  [Remove a hand-provisioned install](uninstall.md#remove-a-hand-provisioned-install).
- An elevated session and `etw-dnstap-<version>-x64.msi`. See
  [Build the MSI](build.md#build-the-msi).

### Run the installer

Run the command from the folder that holds the MSI:

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v install.log RECEIVER=192.0.2.10:6000
```

The MSI has three public properties:

| Property | Required | Default | Meaning |
| --- | --- | --- | --- |
| `RECEIVER` | Yes, unless remembered | None | The collector's `IP:port`, as in step 1. Becomes `--receiver`. |
| `SESSION_NAME` | No | `EtwDnstap` | Becomes `--session-name`. Case-sensitive. 1 to 128 ASCII letters, digits, `_`, `.`, or `-`, not starting with `-`. A leading `-` triggers the launch condition message `SESSION_NAME must not start with '-'. Use 1 to 128 ASCII letters, digits, '_', '.', or '-', and do not start with '-'.` Other values fail the provisioning script, and the install rolls back. |
| `SERVICE_ARGS` | No | Empty | More binary path arguments, such as `--plaintext`. The MSI does not check them, and it creates only the default spool folder. Create a `--spool-dir` folder yourself. See the [Configuration reference](configuration.md). |

The launch condition cannot test the length or the characters of `SESSION_NAME`. The message
has not appeared in a real `msiexec` run yet.

To set all three:

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v install.log `
    RECEIVER=192.0.2.10:6000 SESSION_NAME=EtwDnstap-Lab SERVICE_ARGS=--plaintext
```

Quote a property that has spaces, for example `'SERVICE_ARGS="--plaintext --capacity 131072"'`.

The ETW buffer and channel defaults are sized for 10,000 queries per second. Above that rate,
add the settings for your rate to `SERVICE_ARGS`. For 200,000 queries per second, for example:

```powershell
'SERVICE_ARGS="--etw-buffer-kib 128 --etw-max-buffers 128 --capacity 262144"'
```

See [Recommended settings for higher rates](tuning.md#recommended-settings-for-higher-rates).
The MSI keeps the values in `HKLM\SOFTWARE\EtwDnstap\Installer`. See
[Upgrade, repair, or change settings with the MSI](upgrade.md#upgrade-repair-or-change-settings-with-the-msi).

Provisioning runs only in a pass that installs the executable: a first install, a major
upgrade, a repair, or `REINSTALL=ALL`. A plain `msiexec /i` with a changed `RECEIVER` or
`SESSION_NAME` changes nothing. See [When the install steps run](upgrade.md#when-the-install-steps-run).

### Restart the computer

A successful install exits with code `3010`. Restart the computer yourself. ETW applies the
session grant only at boot.

The service uses delayed automatic start. Windows starts it after the restart, once `DNS` runs. If it is not `RUNNING` five minutes after the restart, start it by hand. See
[troubleshooting.md](troubleshooting.md#the-service-does-not-reach-running).

If an action fails during a first install, Windows Installer rolls back and removes the
files. On an upgrade, the rollback does not undo the provisioning steps. Fix the cause, and
run the install again. A missing or hostname `RECEIVER` each ended with `1603` in the lab.

### Check the result

After the restart, run these in an elevated Windows PowerShell 5.1 session:

```powershell
sc.exe query EtwDnstap
sc.exe qc EtwDnstap
sc.exe qsidtype EtwDnstap
sc.exe qprivs EtwDnstap
typeperf -q EtwDnstap
```

- `sc.exe query` shows `RUNNING`.
- `sc.exe qc` shows `AUTO_START  (DELAYED)`, `NT AUTHORITY\LocalService`, and `DNS`.
- `sc.exe qsidtype` shows `RESTRICTED`. `sc.exe qprivs` lists only `SeChangeNotifyPrivilege`.
- `typeperf -q EtwDnstap` lists the counter names.
- The Application log has Event ID 2, `capture started`, with a `version` that matches the
  MSI file name.

In `install.log`, the `WixQuietExec:` lines after `ProvisionInstall` report each step.
`WixQuietExec:   FAILED: <cause>` gives the cause.

Run the [step 6](#6-inspect-the-registered-identity-and-access-controls) checks. Start the
service by hand only as [Restart the computer](#restart-the-computer) describes.

### What the MSI does

The MSI checks `RECEIVER` before it changes anything. It then performs the steps of this page
in one pass. These include the folders, files, service, identity, privileges, counters, event
log source, group membership, and ETW grant. The recovery policy is set by the script, because the MSI
failure-actions table failed with access denied (error 1939) in the lab. The MSI remembers
`RECEIVER`, `SESSION_NAME`, and `SERVICE_ARGS` for repair and upgrade. It does not start the service.

If the group check fails by name, the script prints `The name-based membership check on <group>
failed (<error>). Reading the members by SID instead.` If the SID check also fails, the install
fails with `Could not read the members of <group>. By name: <error> By SID: <error>`. The lab has
not run the SID fallback.

The grant script writes ETW descriptor snapshots under `C:\ProgramData\EtwDnstap\backup`.
Uninstall deletes that folder. See [Uninstall with the MSI](uninstall.md#uninstall-with-the-msi).

#### Spool encryption key

The MSI also creates what the encrypted spool needs. The service does not create these itself.

| Item | Value |
| --- | --- |
| CNG key | `EtwDnstap Spool Key`, RSA-3072, in the Microsoft Software Key Storage Provider, machine scope. It is not exportable. |
| Key DACL | Protected (no inheritance). SYSTEM and `BUILTIN\Administrators`: all rights. `NT SERVICE\EtwDnstap`: read. |
| Registry key | `HKLM\SOFTWARE\ETW-DNSTapService\Protect`. |
| Registry DACL | Protected, set when the key is created and checked again on every install. SYSTEM and Administrators: full control. `NT SERVICE\EtwDnstap`: query value, set value, and read control (`0x20003`) on this key only. |

The service writes the wrapped spool key to the `WrappedKey` value in the `Protect` key.
Only the service, SYSTEM, and Administrators can use the CNG key. Nobody can export it.

The `Protect` key is outside the service key, so deleting the old service during a major upgrade
does not delete it. The installer never writes `WrappedKey`, and it does not create, read, or
change anything under `Services\EtwDnstap\Parameters`. It deletes the `Parameters\SpoolKey`
subkey that an earlier build made. It checks both DACLs after it sets them, and the install fails
with a message that names the item if a DACL is not as expected. A repair or reinstall sets them
again. It never replaces an existing CNG key. If it has to create a new CNG key and a `WrappedKey`
value is already there, the old value cannot be unwrapped, so the installer deletes it and logs
that. The service then makes a new spool key at its next start and deletes the spool segments it
cannot read.

A major upgrade keeps the CNG key and the `WrappedKey` value. Uninstall deletes `Protect`, then
`HKLM\SOFTWARE\ETW-DNSTapService` if it is empty, then the CNG key. A failed delete is a warning
in the MSI log, and the uninstall finishes; delete what is left by hand. A rollback of a failed
first install also deletes a CNG key that the install only accepted.

The service depends on `DNS` and on `KeyIso` (CNG Key Isolation). The service uses CNG while it
starts, and a service must not do that without `KeyIso` running, so SCM starts `KeyIso` first.
A major upgrade or a repair writes the dependency list again. Check it with `sc.exe qc EtwDnstap`.

## Prerequisites

- Windows x64 with the **DNS Server role**, and the `DNS` service running. The service
  refuses to start while `DNS` is stopped.
- The release executable. See [Build the executable](build.md).
- A dnstap receiver that accepts Frame Streams over TCP and content type
  `protobuf:dnstap.Dnstap`. The lab used
  [DNS-collector v3.2.3](https://github.com/dmachard/DNS-collector/releases/tag/v3.2.3).
  For TLS, import the receiver CA first. See [Set up the dnstap receiver](receiver.md).
- An elevated Windows PowerShell 5.1 session, for `New-EventLog` and `Remove-EventLog`.

## Provision the service

Use these steps on a host without the MSI, or to see what each MSI action changes. Run them
in an elevated Windows PowerShell 5.1 session at the repository root. They assume that the
service and the install folder do not exist. For an existing installation, see
[Repair or upgrade](upgrade.md).

The service runs as `NT AUTHORITY\LocalService` (`S-1-5-19`) with a **restricted** service
SID type. Windows calls the token *write-restricted*. The name refers to access control, not
to ETW data. The service only reads events. At startup it requires all of the following, and
it refuses to start otherwise:

- The token user is `S-1-5-19`.
- `NT SERVICE\EtwDnstap` (`S-1-5-80-...`) is an enabled group.
- The token is restricted. Its restricting SIDs include the service SID and `S-1-5-33`.
- Performance Log Users (`S-1-5-32-559`) is an enabled group. The service SID is a member.
- The only privilege is an enabled `SeChangeNotifyPrivilege`.

Creating the service's own trace session is a write, so a write-restricted token needs one
more grant. Step 4 grants `TRACELOG_CREATE_REALTIME` on the GUID that `--session-name` derives.
The other ETW calls passed in the lab without a grant.

Starting the collector first is simplest, but not required. An unreachable receiver leaves the
service disconnected, and it reconnects with backoff. A receiver that rejects the dnstap content
type stops the service at startup.

### 1. Check the receiver address

Set the collector's numeric IP address and port. Hostnames are rejected.

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

This check matches `check_receiver` in `src/pipeline/config.rs` and `Test-Receiver` in
[Invoke-EtwDnstapProvisioning.ps1](../../packaging/msi/Invoke-EtwDnstapProvisioning.ps1).
It does not test reachability.

### 2. Create the protected installation directory

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
New-Item -ItemType Directory -Path $installDirectory -ErrorAction Stop
icacls.exe $installDirectory /inheritance:r /grant:r `
    '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F'
```

Only SYSTEM and Administrators can write, so ordinary users cannot replace the executable.

### 3. Copy the files, register the service and counters, and grant read and execute

```powershell
Copy-Item '.\target\x86_64-pc-windows-msvc\release\etw-dnstap.exe' $installDirectory
Copy-Item '.\packaging\perfcounters\etw-dnstap.man' $installDirectory
$imagePath = '"{0}\etw-dnstap.exe" --receiver "{1}" --capacity 131072' -f `
    $installDirectory, $Receiver
New-Service -Name EtwDnstap -BinaryPathName $imagePath `
    -StartupType Manual -DependsOn DNS, KeyIso
icacls.exe $installDirectory /grant 'NT SERVICE\EtwDnstap:(OI)(CI)(RX)'
lodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man" "C:\Program Files\EtwDnstap"
```

Arguments go in the binary path, not in `sc.exe start`. `--capacity 131072` is the default. The ETW buffer defaults are sized for 10,000 queries per
second. For higher rates, see
[Recommended settings for higher rates](tuning.md#recommended-settings-for-higher-rates).
Add other options from [configuration.md](configuration.md) to the same string. TLS is the
default, so complete the [receiver certificate step](receiver.md#receiver-certificate) first.
For plaintext, add `--plaintext`.

The spool is on by default, but step 3 does not create its folder. Until the folder passes the
checks, the service runs without a spool and logs one warning. Add `--no-spool` to run without
one. See [Create the spool folder](#create-the-spool-folder).

The `icacls` grant must follow `New-Service`, because `NT SERVICE\EtwDnstap` resolves only once
the service exists. Without it, the service cannot read the executable. Without the `lodctr`
registration, `typeperf -q EtwDnstap` lists nothing. Run `lodctr` again and restart the service.
Non-admin readers in Performance Monitor Users are untested.

### Create a protected snapshot folder

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

This block creates `C:\ProgramData\EtwDnstap`, `backup`, and a snapshot folder for each run.
It stops on a folder with another owner, an extra ACE, or a reparse point. Inspect such a
folder first. Do not take ownership of it. Run the block in the same session as the commands
that follow. `$backup = New-ProtectedBackupFolder` returns the snapshot path.

### Create the spool folder

The spool holds frames that the receiver has not accepted, as segment files, in
`C:\ProgramData\EtwDnstap\spool`. The service uses it only when it passes the checks, and it
never creates it. Run this step after step 3. It stops if the folder exists.

Define `Test-SpoolFolder` in the same session. It replaces `Test-ProtectedFolder` for this
folder.

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

Create the folder. The service SID gets write, read and execute on the folder, and Modify on
its contents. The service can create and delete segment files, but not the folder:

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

Do not run `Test-ProtectedFolder` on this folder after the grant. Use `Test-SpoolFolder`, and
run it again after any change. The last `icacls.exe` output lists four entries, none with `(I)`:

- `NT SERVICE\EtwDnstap` with write, read and execute, and no `(OI)` or `(CI)`. It applies to
  the folder only.
- `NT SERVICE\EtwDnstap:(OI)(CI)(IO)(M)`. It applies to the contents only.
- `BUILTIN\Administrators:(OI)(CI)(F)`
- `NT AUTHORITY\SYSTEM:(OI)(CI)(F)`

At startup the service runs a looser check. If it fails, the service refuses the spool, logs
one warning, and runs without one. The check fails for:

- A missing folder, or a folder or parent that is a reparse point.
- An owner other than SYSTEM or Administrators.
- A path with no drive letter on a fixed local disk, or with a `.` or `..` part.
- An allow entry for another account. An inherit-only `CREATOR OWNER` entry is allowed.

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

- `obj=` runs the service as LocalService, which has no password. Run it through `cmd.exe`,
  because Windows PowerShell 5.1 drops the empty `""`. Check with `sc.exe qc`.
- `sidtype restricted` makes the SCM build a write-restricted token. An unrestricted token
  fails the identity check.
- `privs` limits the service to `SeChangeNotifyPrivilege`. The service refuses any other
  privilege, or that one disabled.
- `sc.exe sdset` replaces the descriptor that `New-Service` created. Run it through `cmd.exe`,
  as for `obj=`. In the lab, a standard user got "Access is denied" (error 5) from `sc.exe stop`,
  `start`, `config`, and `control EtwDnstap 200`.
- `net localgroup` adds the **service SID** to Performance Log Users, so the service can create
  and control ETW sessions. The membership must be enabled in the token, not deny-only.
  Restart the service after a membership change.
- `Grant-EtwDnstapEtwAccess.ps1` grants the service SID `TRACELOG_CREATE_REALTIME` (0x20) on
  the session GUID for each `-SessionName`. For `EtwDnstap`, the GUID is
  `{05e1db3c-1322-58c7-8235-0463fccba32b}`. Pass every `--session-name` that this host uses,
  spelled exactly as in the binary path. The name is hashed without case folding. The script
  grants no logging right, and it removes other ACEs for the service SID.
- Run the script with `-WhatIf` first. It changes nothing and writes no backup. A real run
  first saves the `WMI\Security` key and the original bytes of each changed value to
  `-BackupPath`. To undo a run, use `-Restore` as described in
  [scripts/provision/README.md](../../scripts/provision/README.md#backup-and-restore).
- `New-EventLog` registers the `EtwDnstap` source. The restricted token can write these events.
- `Restart-Computer` is required. ETW applies its descriptors at boot. A grant without a
  restart had no effect in the lab.

If the source already exists, `New-EventLog` fails. Check first with
`[System.Diagnostics.EventLog]::SourceExists('EtwDnstap')`.

### If an ETW access check is denied

- **`StartTraceW: access denied`:** the service SID lacks `TRACELOG_CREATE_REALTIME` on the GUID
  in the message, or the computer has not restarted since the grant. A changed
  `--session-name` changes the GUID, and so does a change of case. Run
  `scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` with the name from the
  message, restart the computer, and start the service.
- **`EnableTraceEx2: access denied`:** not seen in the lab. Inspect
  `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security\eb79061a-a566-4698-9119-3ed2807060e7`,
  and record what it holds. Do not add a grant on the provider GUID. The lab needed none.

Record the OS build and the descriptor that you found.

### 5. Configure SCM recovery

A DNS Server restart, a capture worker failure, or another run failure makes the service exit
with a service-specific error. SCM ignores that non-crash failure unless the non-crash flag is
set. Without these two commands, one failure stops capture.

```powershell
sc.exe failure EtwDnstap reset= 86400 actions= restart/5000/restart/5000/restart/60000
sc.exe failureflag EtwDnstap 1
```

- `reset= 86400` clears the failure count after one day without failures.
- The first two failures restart the service after 5 seconds. Later failures restart it after
  60 seconds.
- `failureflag 1` applies the actions to non-crash failures.

A restart does not resend lost data. A restart while `DNS` is stopped fails at startup, unless
the service depends on `DNS` and `KeyIso` (see [Start type](#start-type)). SCM counts every failure,
including a refused start. See [troubleshooting.md](troubleshooting.md#the-service-keeps-restarting).

Check the policy:

```powershell
sc.exe qfailure EtwDnstap
sc.exe qfailureflag EtwDnstap
```

#### Start type

`New-Service` sets Manual, so the service stays stopped after the step 4 reboot. When
[step 7](#7-start-capture-and-check-that-it-is-running) shows `RUNNING`, set delayed automatic
start:

```powershell
cmd.exe /c 'sc.exe config EtwDnstap start= delayed-auto depend= DNS/KeyIso'
sc.exe qc EtwDnstap
```

- `depend= DNS/KeyIso` keeps both dependencies, so the dependencies do not change. `KeyIso` is the CNG Key Isolation service, which the service needs when it starts.
- With the `DNS` dependency, stopping `DNS` on request also stops `EtwDnstap`. `Restart-Service DNS`
  fails, so use `Restart-Service DNS -Force`. In the lab, that command stopped `EtwDnstap` with
  exit code 0, and the new DNS Server process was captured in under 2 seconds. After
  `Stop-Service DNS -Force` and `Start-Service DNS`, start `EtwDnstap` by hand.
- If the DNS Server process crashes, `EtwDnstap` stops about 5 seconds later with a
  service-specific error. The recovery policy restarts it 5 seconds after that. The lab
  restart also started `DNS`.
- Do not set this before step 7 succeeds. A service that fails at startup would fail at each
  boot.

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

Check that each command succeeds, and that these results hold:

- `sc.exe qc` shows `NT AUTHORITY\LocalService`. `sc.exe qsidtype` reports `RESTRICTED`.
  `sc.exe qprivs` lists only `SeChangeNotifyPrivilege`.
- Performance Log Users lists `NT SERVICE\EtwDnstap`.
- The `sdshow` output equals the step 4 string exactly.

A difference means that someone changed the descriptor, or that this OS build prints it in
another form. Record the string, and do not start the service until you know which.

### 7. Start capture and check that it is running

```powershell
sc.exe start EtwDnstap
sc.exe query EtwDnstap
```

Repeat `sc.exe query EtwDnstap` while the state is `START_PENDING`. It should reach `RUNNING`.
If not, see [troubleshooting.md](troubleshooting.md#the-service-does-not-reach-running). Look
for `capture started` (Event ID 2).

Check that the counters are visible:

```powershell
typeperf -q EtwDnstap
typeperf "\EtwDnstap(*)\*" -sc 5
```

The ETW gauges read 0 until the first ETW query succeeds. If the names do not appear, run
`lodctr` from step 3 again and restart the service. If counter registration fails at startup,
the service logs `performance counters are unavailable` (Event ID 3) and runs without counters.

Then set the start type as described in [step 5](#start-type).

### 8. Stop the service after the traffic you want

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

The service should reach `STOPPED` with exit code 0. It writes no capture or metrics files.

Frames that the receiver did not accept stay in the spool until the next start. Check
`spool_segments` in the final `output stopped` line. The spool holds DNS data. Delete it when
you no longer need the frames. See [Uninstall](uninstall.md#7-delete-the-spool-folder).

### Validation checklist for the LocalService restricted profile

Complete this checklist on a disposable DNS host before you rely on the profile on another
build. Record each result with the binary's commit. Items marked *lab* passed on build 26100.

- *lab:* The service reaches `RUNNING`. `capture started` (Event ID 2) shows the expected
  keyword masks, without `keyword_fallback`.
- *lab:* `foreign_pid_rejected` stays 0. `received` rises, and `receiver_connected` is true,
  under DNS traffic.
- *lab:* With the `0x20` grant and a restart, `StartTraceW` and `EnableTraceEx2` are not
  denied. Without the grant, `StartTraceW` is denied. Otherwise, follow
  [the ETW access step](#if-an-etw-access-check-is-denied).
- *lab:* After `sc.exe stop`: `STOPPED`, exit code 0, and `output_errors` 0.
- *lab:* After a kill, the next start reclaims the stale session (`stale_sessions_reclaimed=1`),
  only when its name, real-time shape, and GUID all match.
- *lab:* After a DNS Server restart or crash, the service restarts under the recovery policy.
  The gap was 9 to 10 seconds: the service stopped 4 to 5 seconds after the restart, and
  capture resumed 5 seconds later.
- A second `sc.exe start EtwDnstap` fails with error 1056 while the service runs. The service
  has no instance mutex. The identity check refuses any other process.
- *lab:* The SCM query of `DNS` (`OpenSCManagerW`, `OpenServiceW`, `QueryServiceStatusEx`)
  succeeds. `sc.exe sidtype EtwDnstap unrestricted` stops the service at startup with
  `the process token is not restricted`.
- *lab:* `typeperf -q EtwDnstap` lists the counters, and `typeperf "\EtwDnstap(*)\*" -sc 5`
  shows values that match the `capture stopped` counters. The instance is gone after stop.
- Not covered by the lab: the spool. Stop the collector briefly under DNS traffic.
  `spool_segments` and `spooled` rise while `receiver_connected` is false. After the collector
  returns, `replayed` rises and `spool_bytes` falls.

## Next steps

- [Set up the dnstap receiver](receiver.md)
- [Configuration reference](configuration.md)
- [Log events and counters](monitoring.md)
- [Troubleshooting](troubleshooting.md)
- [Repair or upgrade](upgrade.md)
- [Uninstall](uninstall.md)
