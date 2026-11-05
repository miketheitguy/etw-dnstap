# Install and provision the service

> **Status:** Current · **Describes:** the code on branch `spool-encryption` · **Reviewed:** 2026-11-05

Install the service on a DNS Server host. First [build the executable](build.md) and set up the
collector ([Set up the dnstap receiver](receiver.md)).

The LocalService restricted-SID profile was validated on Windows Server 2025, build 26100. On
another build, run the
[validation checklist](#validation-checklist-for-the-localservice-restricted-profile) on a
disposable host first ([lab record](../history/localservice-restricted-validation.md)).

## Install with the MSI

The MSI does the [manual steps](#provision-the-service) in one pass. A restart starts the service.
See [the plan and its results](../history/msi-installer-plan.md).

### MSI prerequisites

- Windows x64 with the **DNS Server role**. The MSI stops if the `DNS` service is not registered.
- A `RECEIVER` value: a numeric unicast IPv4 address (`192.0.2.10:6000`) or a bracketed IPv6 address
  (`[2001:db8::10]:6000`), with a port from 1 to 65535. Hostnames are refused.
- For TLS (the default), the **receiver CA** in the machine certificate store. See
  [Set up the dnstap receiver](receiver.md#receiver-certificate). For plaintext, pass
  `SERVICE_ARGS=--plaintext`.
- No hand-provisioned `EtwDnstap` service. See
  [Remove a hand-provisioned install](uninstall.md#remove-a-hand-provisioned-install).
- An elevated session and `etw-dnstap-<version>-x64.msi`. See [Build the MSI](build.md#build-the-msi).

### Run the installer

From the folder that holds the MSI:

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v install.log RECEIVER=192.0.2.10:6000
```

| Property | Required | Default | Meaning |
| --- | --- | --- | --- |
| `RECEIVER` | Yes, unless remembered | None | The collector's `IP:port`. Becomes `--receiver`. |
| `SESSION_NAME` | No | `EtwDnstap` | The ETW session name. Becomes `--session-name`. Case-sensitive. 1 to 128 ASCII letters, digits, `_`, `.`, or `-`, not starting with `-`. |
| `SERVICE_ARGS` | No | Empty | More service arguments, such as `--plaintext`. The MSI does not check them and creates only the default spool folder. To use `--spool-dir`, create that folder yourself. See the [Configuration reference](configuration.md). |

An invalid `SESSION_NAME` fails the install. To set all three properties:

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v install.log `
    RECEIVER=192.0.2.10:6000 SESSION_NAME=EtwDnstap-Lab SERVICE_ARGS=--plaintext
```

Quote a value that has spaces, for example `'SERVICE_ARGS="--plaintext --capacity 131072"'`.

The default ETW buffer settings suit about 10,000 queries per second. For a higher rate, add its
settings to `SERVICE_ARGS`. For 200,000 queries per second, for example:

```powershell
'SERVICE_ARGS="--etw-buffer-kib 128 --etw-max-buffers 128 --capacity 262144"'
```

See [Recommended settings for higher rates](tuning.md#recommended-settings-for-higher-rates).

The MSI remembers the three values in `HKLM\SOFTWARE\EtwDnstap\Installer`, and a later run reuses them
unless you pass new values. Provisioning runs only in a pass that installs the executable: a first
install, a major upgrade, a repair, or `REINSTALL=ALL`. See
[When the install steps run](upgrade.md#when-the-install-steps-run) and
[Repair or upgrade](upgrade.md#upgrade-repair-or-change-settings-with-the-msi).

### Restart the computer

A successful install exits with code `3010`. Restart the computer, because ETW applies the session
grant only at boot. The service starts with delayed automatic start once `DNS` runs. If it is not
`RUNNING` five minutes later, start it by hand. See
[troubleshooting.md](troubleshooting.md#the-service-does-not-reach-running).

A failed first install rolls back by running the uninstall steps. A failed upgrade, repair, or
`REINSTALL=ALL` run keeps the provisioning steps that already ran. Fix the cause and run the install
again. A missing or hostname `RECEIVER` ends with error `1603`.

### Check the result

After the restart, in an elevated Windows PowerShell 5.1 session:

```powershell
sc.exe query EtwDnstap
sc.exe qc EtwDnstap
sc.exe qsidtype EtwDnstap
sc.exe qprivs EtwDnstap
typeperf -q EtwDnstap
```

- `sc.exe query` shows `RUNNING`.
- `sc.exe qc` shows `AUTO_START  (DELAYED)`, `NT AUTHORITY\LocalService`, and the dependencies
  `DNS` and `KeyIso`.
- `sc.exe qsidtype` shows `RESTRICTED`. `sc.exe qprivs` lists only `SeChangeNotifyPrivilege`.
- `typeperf -q EtwDnstap` lists the counter names.
- The Application log has Event ID 2, `capture started`, with a `version` that matches the MSI file name.

In `install.log`, each provisioning step writes a `[EtwDnstap]` line after the `ProvisionInstall`
`WixQuietExec:` line. A failed step ends with `WixQuietExec:   FAILED: <cause>`. Then run the
[step 6](#6-inspect-the-registered-identity-and-access-controls) checks.

### What the MSI does

After it checks `RECEIVER` and the DNS Server role, copies the files, and creates the service, a
deferred provisioning step runs in order:

- Checks the `RECEIVER` format, as in [step 1](#1-check-the-receiver-address).
- Creates `C:\ProgramData\EtwDnstap\spool` and the spool encryption key
  ([below](#spool-encryption-key)).
- Adds `NT SERVICE\EtwDnstap` to Performance Log Users.
- Runs the grant script for `SESSION_NAME`. It writes ETW snapshots under
  `C:\ProgramData\EtwDnstap\backup`, which the uninstall deletes. See
  [Uninstall with the MSI](uninstall.md#uninstall-with-the-msi).
- Sets the recovery policy with `sc.exe failure`.

If the group check by name fails, the script reads the members by SID. If that also fails, the install
fails with `Could not read the members of <group>`.

#### Spool encryption key

If the keys are missing, the service logs `spool key is not usable` (Event ID 26) and runs without a
spool.

| Item | Value |
| --- | --- |
| CNG key | `EtwDnstap Spool Key`. RSA-3072, Microsoft Software Key Storage Provider, machine scope, not exportable. |
| Key DACL | Protected. SYSTEM and `BUILTIN\Administrators`: all rights. `NT SERVICE\EtwDnstap`: read. |
| Registry key | `HKLM\SOFTWARE\ETW-DNSTapService\Protect`, which holds the `WrappedKey` value. |
| Registry DACL | Protected. SYSTEM and Administrators: full control. `NT SERVICE\EtwDnstap`: query value, set value, and read control (`0x20003`), on this key only. |

- The `Protect` key is outside the service key, so a major upgrade keeps the key and `WrappedKey`.
- The installer checks both DACLs after it sets them, and a repair sets them again. A wrong DACL fails
  the install and names the item.
- The installer keeps an existing CNG key. If it creates a new key while a `WrappedKey` value exists, it
  deletes and logs the old value. The service then creates a new spool key at its next start and deletes
  the spool segments it cannot read.
- The installer deletes a `Parameters\SpoolKey` subkey under `Services\EtwDnstap` if one exists.
- The service depends on `KeyIso` (CNG Key Isolation) and waits at most 3 seconds for the key store.
- The service keeps the unwrapped AES key encrypted in one locked page with `CryptProtectMemory`.
  See [the data path](../design/data-path.md) and [SECURITY.md](../../SECURITY.md).
- Uninstall removes these items. See [What the uninstall removes](uninstall.md#what-the-uninstall-removes).

## Prerequisites

- Windows x64 with the **DNS Server role**, and the `DNS` service running.
- The release executable ([Build the executable](build.md)).
- A dnstap receiver that accepts Frame Streams over TCP with the content type
  `protobuf:dnstap.Dnstap`, such as
  [DNS-collector v3.2.3](https://github.com/dmachard/DNS-collector/releases/tag/v3.2.3). For TLS,
  import the receiver CA first ([Set up the dnstap receiver](receiver.md)).
- An elevated Windows PowerShell 5.1 session (the steps use `New-EventLog` and `Remove-EventLog`).

## Provision the service

Use these steps on a host without the MSI, or to see what each MSI action changes. Run them in an
elevated Windows PowerShell 5.1 session at the repository root. For an existing installation, see
[Repair or upgrade](upgrade.md).

The service runs as `NT AUTHORITY\LocalService` (`S-1-5-19`) with a **restricted** service SID type,
which gives a *write-restricted* token. At startup it refuses to start unless all of these hold:

- The token user is `S-1-5-19`.
- `NT SERVICE\EtwDnstap` (`S-1-5-80-...`) is an enabled group.
- The token is restricted, and its restricting SIDs include the service SID and `S-1-5-33`.
- Performance Log Users (`S-1-5-32-559`) is an enabled group, and the service SID is a member.
- The only privilege is an enabled `SeChangeNotifyPrivilege`.

Creating the trace session is a write, so step 4 grants `TRACELOG_CREATE_REALTIME` on the GUID that
`--session-name` derives. Start the collector first if you can. If it is unreachable, the service
reconnects with backoff.

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
[Invoke-EtwDnstapProvisioning.ps1](../../packaging/msi/Invoke-EtwDnstapProvisioning.ps1). It does
not test reachability.

### 2. Create the protected installation directory

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
New-Item -ItemType Directory -Path $installDirectory -ErrorAction Stop
icacls.exe $installDirectory /inheritance:r /grant:r `
    '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F'
```

Only SYSTEM and Administrators can write to the folder, so ordinary users cannot replace the executable.

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

- Put arguments in the binary path, not in `sc.exe start`. `--capacity 131072` is the default. For
  rates above about 10,000 queries per second, see
  [Recommended settings for higher rates](tuning.md#recommended-settings-for-higher-rates). Add other
  options from [configuration.md](configuration.md) to the same string.
- TLS is the default. Complete the [receiver certificate step](receiver.md#receiver-certificate) first,
  or add `--plaintext`.
- Run the `icacls` grant after `New-Service`, because `NT SERVICE\EtwDnstap` resolves only once the
  service exists. Without it, the service cannot read its executable.
- Without the `lodctr` registration, `typeperf -q EtwDnstap` lists nothing. If you run `lodctr` later,
  restart the service.
- The spool is on by default. Until its folder passes the checks, the service runs without a spool and
  logs one warning ([Create the spool folder](#create-the-spool-folder)). Only the MSI creates the spool
  encryption key ([Spool encryption key](#spool-encryption-key)). For an encrypted spool, use the MSI.
  Otherwise add `--no-spool`.

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

This block creates `C:\ProgramData\EtwDnstap`, `backup`, and a new snapshot folder for each run. It
stops on a folder with another owner, an extra ACE, or a reparse point. Inspect such a folder, and do
not take ownership of it. Run the commands that follow in the same session.
`$backup = New-ProtectedBackupFolder` returns the snapshot path.

### Create the spool folder

Create `C:\ProgramData\EtwDnstap\spool` after step 3. This step stops if the folder exists. Define
`Test-SpoolFolder` in the same session. It replaces `Test-ProtectedFolder` for this folder.

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

The service SID gets write, read, and execute on the folder itself, and Modify on its contents, so it
can create and delete segment files but not the folder:

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

Check this folder with `Test-SpoolFolder` after any change. The last `icacls.exe` output lists four
entries, none with `(I)`:

- `NT SERVICE\EtwDnstap` with write, read, and execute, and no `(OI)` or `(CI)`. It applies to the folder only.
- `NT SERVICE\EtwDnstap:(OI)(CI)(IO)(M)`. It applies to the contents only.
- `BUILTIN\Administrators:(OI)(CI)(F)`
- `NT AUTHORITY\SYSTEM:(OI)(CI)(F)`

At startup the service runs a looser check. If it fails, the service logs one warning and runs without
a spool. The check fails for:

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

- `obj=` runs the service as LocalService, which has no password. Run it through `cmd.exe`, because
  Windows PowerShell 5.1 drops the empty `""`. Check the result with `sc.exe qc`.
- `sidtype restricted` makes SCM build a write-restricted token. `privs` limits the service to
  `SeChangeNotifyPrivilege`.
- `sc.exe sdset` replaces the descriptor that `New-Service` created. Run it through `cmd.exe`. A standard
  user then gets "Access is denied" (error 5) from `sc.exe stop`, `start`, `config`, and
  `control EtwDnstap 200`.
- `net localgroup` adds the **service SID** to Performance Log Users. The membership must be enabled in
  the token, not deny-only. Restart the service after a membership change.
- `Grant-EtwDnstapEtwAccess.ps1` grants the service SID `TRACELOG_CREATE_REALTIME` (0x20) on the session
  GUID for each `-SessionName`. For `EtwDnstap`, the GUID is `{05e1db3c-1322-58c7-8235-0463fccba32b}`.
  Pass every `--session-name` that this host uses, spelled exactly as in the binary path, because the
  name is hashed without case folding. The script removes other ACEs for the service SID.
- Run the script with `-WhatIf` first. A real run first saves the `WMI\Security` key and the original
  bytes of each changed value to `-BackupPath`. To undo a run, see
  [scripts/provision/README.md](../../scripts/provision/README.md#backup-and-restore).
- `New-EventLog` registers the `EtwDnstap` event source and fails if it exists. Check first with
  `[System.Diagnostics.EventLog]::SourceExists('EtwDnstap')`.
- `Restart-Computer` is required, because ETW applies its descriptors at boot.

### If an ETW access check is denied

- **`StartTraceW: access denied`:** The service SID lacks `TRACELOG_CREATE_REALTIME` on the GUID in the
  message, or the computer has not restarted since the grant. A changed `--session-name` or a change of
  case changes the GUID. Run `scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` with
  the name from the message, restart the computer, and start the service.
- **`EnableTraceEx2: access denied`:** Inspect
  `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security\eb79061a-a566-4698-9119-3ed2807060e7`, and record
  the OS build and what it holds. Add no grant on the provider GUID.

### 5. Configure SCM recovery

SCM acts on a service-specific exit only when the non-crash failure flag is set. These commands restart
the service after a failure and set the flag:

```powershell
sc.exe failure EtwDnstap reset= 86400 actions= restart/5000/restart/5000/restart/60000
sc.exe failureflag EtwDnstap 1
```

- `reset= 86400` clears the failure count after one day without failures.
- The first two failures restart the service after 5 seconds. Later failures restart it after 60 seconds.
- `failureflag 1` applies the actions to non-crash failures.

A restart does not resend lost data. A restart while `DNS` is stopped fails, and SCM counts that failure
too. See [troubleshooting.md](troubleshooting.md#the-service-keeps-restarting). Check the policy:

```powershell
sc.exe qfailure EtwDnstap
sc.exe qfailureflag EtwDnstap
```

#### Start type

`New-Service` sets Manual start. When [step 7](#7-start-capture-and-check-that-it-is-running) shows
`RUNNING`, set delayed automatic start:

```powershell
cmd.exe /c 'sc.exe config EtwDnstap start= delayed-auto depend= DNS/KeyIso'
sc.exe qc EtwDnstap
```

- `depend= DNS/KeyIso` keeps both dependencies.
- Stopping `DNS` also stops `EtwDnstap`. `Restart-Service DNS` fails, so use `Restart-Service DNS -Force`.
  After that, or after `Stop-Service DNS -Force` and `Start-Service DNS`, start `EtwDnstap` by hand.
- If the DNS Server process crashes or restarts without an SCM stop, `EtwDnstap` keeps running and
  captures from the new process.

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

Check that each command succeeds and that:

- `sc.exe qc` shows `NT AUTHORITY\LocalService`, `sc.exe qsidtype` reports `RESTRICTED`, and
  `sc.exe qprivs` lists only `SeChangeNotifyPrivilege`.
- Performance Log Users lists `NT SERVICE\EtwDnstap`.
- The `sdshow` output equals the step 4 string exactly.

A difference means that someone changed the descriptor, or that this OS build prints it in another
form. Record the string, and start the service only after you know which.

### 7. Start capture and check that it is running

```powershell
sc.exe start EtwDnstap
sc.exe query EtwDnstap
```

Repeat `sc.exe query EtwDnstap` while the state is `START_PENDING`. It should reach `RUNNING` and log
`capture started` (Event ID 2). If not, see
[troubleshooting.md](troubleshooting.md#the-service-does-not-reach-running). Check the counters:

```powershell
typeperf -q EtwDnstap
typeperf "\EtwDnstap(*)\*" -sc 5
```

The ETW gauges read 0 until the first ETW query succeeds. If the counter names do not appear, run
`lodctr` from step 3 again and restart the service. If counter registration fails at startup, the
service logs `performance counters are unavailable` (Event ID 3) and runs without counters. Then set
the start type as in [step 5](#start-type).

### 8. Stop the service after the traffic you want

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

The service should reach `STOPPED` with exit code 0. Frames that the receiver did not accept stay in the
spool until the next start. Check `spool_segments` in the final `output stopped` line. The spool holds
DNS data, so delete it when you no longer need the frames
([Uninstall](uninstall.md#7-delete-the-spool-folder)).

### Validation checklist for the LocalService restricted profile

Record each result with the binary's commit.

- The service reaches `RUNNING`. `capture started` (Event ID 2) shows the expected keyword masks,
  without `keyword_fallback`.
- `received` rises, and `receiver_connected` is true, under DNS traffic.
- With the `0x20` grant and a restart, `StartTraceW` and `EnableTraceEx2` succeed. Without the grant,
  `StartTraceW` is denied ([fix](#if-an-etw-access-check-is-denied)).
- After `sc.exe stop`: `STOPPED`, exit code 0, and `output_errors` 0.
- After a kill, the next start reclaims the stale session (`stale_sessions_reclaimed=1`) only when its
  name, real-time shape, and GUID all match.
- After a DNS Server crash and restart by its recovery policy, capture continues and events from the new
  process are accepted. A stop of DNS Server through SCM stops EtwDnstap first. Start EtwDnstap again.
- A second `sc.exe start EtwDnstap` fails with error 1056 while the service runs.
- `sc.exe sidtype EtwDnstap unrestricted` stops the service at startup with
  `the process token is not restricted`.
- `typeperf -q EtwDnstap` lists the counters, and `typeperf "\EtwDnstap(*)\*" -sc 5` shows values that
  match the `capture stopped` counters. The instance is gone after stop.
- Spool: stop the collector briefly under DNS traffic. `spool_segments` and `spooled` rise while
  `receiver_connected` is false. After the collector returns, `replayed` rises and `spool_bytes` falls.

## Next steps

- [Set up the dnstap receiver](receiver.md)
- [Configuration reference](configuration.md)
- [Log events and counters](monitoring.md)
- [Troubleshooting](troubleshooting.md)
- [Repair or upgrade](upgrade.md)
- [Uninstall](uninstall.md)
