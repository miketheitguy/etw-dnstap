# Install and provision the service

> **Status:** Current · **Describes:** the code on `main` at commit `592a040` · **Reviewed:** 2026-11-05

### Quick Start

Elevated, in the folder with the MSI ([Build the MSI](build.md#build-the-msi)):

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v install.log RECEIVER=192.0.2.10:6000
Restart-Computer
```

Import the receiver CA first ([receiver](receiver.md)), or add `SERVICE_ARGS=--plaintext`. No MSI? Use
[Provision the service](#provision-the-service).

### Steps / Configuration

#### Install with the MSI

The MSI runs the [manual steps](#provision-the-service) in one pass
([plan and results](../history/msi-installer-plan.md)). It needs:

- Windows x64 with the **DNS Server role** (the MSI stops if `DNS` isn't registered).
- A numeric unicast `RECEIVER`: `192.0.2.10:6000` or `[2001:db8::10]:6000`, port 1 to 65535. No hostnames.
- For TLS, the receiver CA in the machine store ([Receiver certificate](receiver.md#receiver-certificate)).
- No hand-provisioned `EtwDnstap` ([remove it](uninstall.md#remove-a-hand-provisioned-install)).

| Property | Required | Default | Meaning |
| --- | --- | --- | --- |
| `RECEIVER` | Unless remembered | None | `IP:port`. Becomes `--receiver`. |
| `SESSION_NAME` | No | `EtwDnstap` | Becomes `--session-name`. Case-sensitive. 1 to 128 ASCII letters, digits, `_`, `.`, `-`, not starting with `-`. Invalid fails the install. |
| `SERVICE_ARGS` | No | Empty | Extra arguments, such as `--plaintext`. Not checked. See [Configuration](configuration.md). |

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v install.log `
    RECEIVER=192.0.2.10:6000 SESSION_NAME=EtwDnstap-Lab SERVICE_ARGS=--plaintext
```

Quote values with spaces: `'SERVICE_ARGS="--plaintext --capacity 131072"'`. The default ETW buffers suit
about 10,000 queries per second. For 200,000
([more](tuning.md#recommended-settings-for-higher-rates)):

```powershell
'SERVICE_ARGS="--etw-buffer-kib 128 --etw-max-buffers 128 --capacity 262144"'
```

The MSI remembers all three in `HKLM\SOFTWARE\EtwDnstap\Installer` and provisions only when it installs the
executable: first install, major upgrade, repair, or `REINSTALL=ALL`
([when](upgrade.md#when-the-install-steps-run), [Repair or upgrade](upgrade.md#upgrade-repair-or-change-settings-with-the-msi)).

A successful install exits `3010`. After the restart, run the [step 6](#6-inspect-the-registered-identity-and-access-controls)
checks. Expect `sc.exe query EtwDnstap` `RUNNING`, `typeperf -q EtwDnstap` listing the counters, and
Application Event ID 2 `capture started` with a `version` matching the MSI file name. In `install.log`, each
step writes a `[EtwDnstap]` line after the `ProvisionInstall` `WixQuietExec:` line, and a failed one ends
`WixQuietExec:   FAILED: <cause>`.

The deferred provisioning step also creates `C:\ProgramData\EtwDnstap\spool` (the default only; create a
`--spool-dir` folder yourself) and the [spool encryption key](#spool-encryption-key), and adds
`NT SERVICE\EtwDnstap` to Performance Log Users by name, then by SID (both failing fails the install with
`Could not read the members of <group>`). The grant script's ETW snapshots go to
`C:\ProgramData\EtwDnstap\backup` ([deleted on uninstall](uninstall.md#uninstall-with-the-msi)). The
steps follow [1](#1-check-the-receiver-address) to [5](#5-configure-scm-recovery).

#### Spool encryption key

| Item | Value |
| --- | --- |
| CNG key | `EtwDnstap Spool Key`. RSA-3072, Microsoft Software Key Storage Provider, machine scope, not exportable. |
| Key DACL | Protected. SYSTEM, `BUILTIN\Administrators`: all rights. `NT SERVICE\EtwDnstap`: read. |
| Registry key | `HKLM\SOFTWARE\ETW-DNSTapService\Protect`, holding `WrappedKey`. |
| Registry DACL | Protected. SYSTEM, Administrators: full control. `NT SERVICE\EtwDnstap`: query value, set value, read control (`0x20003`), this key only. |

- `Protect` is outside the service key, so a major upgrade keeps the key and `WrappedKey`.
- The installer checks both DACLs after setting them (a repair sets them again). A wrong DACL fails the
  install and names the item.
- It keeps an existing CNG key. A new key plus an existing `WrappedKey` deletes and logs the old value; the
  service then makes a new spool key at its next start and deletes segments it can't read.
- It deletes a `Parameters\SpoolKey` subkey under `Services\EtwDnstap`.
- The service depends on `KeyIso` (CNG Key Isolation), waits at most 3 seconds for the key store, and keeps
  the unwrapped AES key encrypted in one locked page with `CryptProtectMemory`
  ([data path](../design/data-path.md), [SECURITY.md](../../SECURITY.md)).
- [The uninstall](uninstall.md#what-the-uninstall-removes) removes all of it.

#### Prerequisites

For the manual path: Windows x64 with the DNS Server role and `DNS` running; the release executable
([Build the executable](build.md)); elevated Windows PowerShell 5.1; and a receiver taking Frame Streams
over TCP with content type `protobuf:dnstap.Dnstap`, such as
[DNS-collector v3.2.3](https://github.com/dmachard/DNS-collector/releases/tag/v3.2.3). For TLS, import the
receiver CA first ([receiver](receiver.md)).

#### Provision the service

For a manual install, or to see what each MSI action does. Run at the repository root. For an existing
install, see [Repair or upgrade](upgrade.md).

The service runs as `NT AUTHORITY\LocalService` (`S-1-5-19`) with a **restricted** service SID type
(write-restricted token). It refuses to start unless the token user is `S-1-5-19`; `NT SERVICE\EtwDnstap`
(`S-1-5-80-...`) is an enabled group; the restricting SIDs include the service SID and `S-1-5-33`;
Performance Log Users (`S-1-5-32-559`) is an enabled group with the service SID as a member; and the only
privilege is an enabled `SeChangeNotifyPrivilege`. Start the collector first if you can. If it's
unreachable, the service reconnects with backoff.

##### 1. Check the receiver address

Same rules as `check_receiver` in `src/pipeline/config.rs` and `Test-Receiver` in
[Invoke-EtwDnstapProvisioning.ps1](../../packaging/msi/Invoke-EtwDnstapProvisioning.ps1). No reachability
test.

```powershell
$receiverText = '192.0.2.10:6000'
$ip = $null
if ($receiverText -notmatch '^(?:\[(?<a>[^\]]+)\]|(?<a>[0-9.]+)):(?<p>\d{1,5})$' -or
    -not [System.Net.IPAddress]::TryParse($Matches.a, [ref]$ip)) {
    throw 'Receiver must be a numeric IP address and port, for example 192.0.2.10:6000.'
}
$port = [int]$Matches.p
$octet = if ($ip.AddressFamily -eq 'InterNetwork') { $ip.GetAddressBytes()[0] } else { 0 }
if ($port -eq 0 -or $port -gt 65535 -or $ip.Equals([System.Net.IPAddress]::Any) -or
    $ip.Equals([System.Net.IPAddress]::IPv6Any) -or $ip.IsIPv6Multicast -or ($octet -ge 224 -and $octet -le 239)) {
    throw 'Receiver must have a unicast IP address and a nonzero port.'
}
$Receiver = if ($ip.AddressFamily -eq 'InterNetworkV6') { "[$ip]:$port" } else { "${ip}:$port" }
```

##### 2. Create the protected installation directory

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
New-Item -ItemType Directory -Path $installDirectory -ErrorAction Stop
icacls.exe $installDirectory /inheritance:r /grant:r `
    '*S-1-5-18:(OI)(CI)F' '*S-1-5-32-544:(OI)(CI)F'
```

##### 3. Copy the files, register the service and counters, and grant read and execute

Put arguments in the binary path, not in `sc.exe start`. `--capacity 131072` is the default. For more than
about 10,000 queries per second, see [recommended settings](tuning.md#recommended-settings-for-higher-rates).
Add other [options](configuration.md), and `--plaintext` unless you finished the
[receiver certificate step](receiver.md#receiver-certificate). The spool is on by default. Without its
[folder](#create-the-spool-folder) the service logs one warning and runs without a spool. Only the MSI
creates the [spool encryption key](#spool-encryption-key), so for an encrypted spool use the MSI.
Otherwise add `--no-spool`.

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

##### Create a protected snapshot folder

Define these in one session and keep it open.

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
    New-ProtectedFolder $leaf
    $leaf
}
```

`$backup = New-ProtectedBackupFolder` returns a new snapshot folder (and creates
`C:\ProgramData\EtwDnstap` and `backup`).

##### Create the spool folder

After step 3, with the functions above loaded. The service SID gets write, read, and execute on the folder
and Modify on its contents: it creates and deletes segment files, not the folder.

```powershell
$spoolParent = Join-Path $env:ProgramData 'EtwDnstap'
$spoolDirectory = Join-Path $spoolParent 'spool'
if (-not (Test-Path -LiteralPath $spoolParent)) { New-ProtectedFolder $spoolParent }
Test-ProtectedFolder $spoolParent
New-ProtectedFolder $spoolDirectory
icacls.exe $spoolDirectory /grant 'NT SERVICE\EtwDnstap:(RX,W)' | Out-Null
icacls.exe $spoolDirectory /grant 'NT SERVICE\EtwDnstap:(OI)(CI)(IO)(M)' | Out-Null
icacls.exe $spoolDirectory
```

Expect four entries, none `(I)`: `NT SERVICE\EtwDnstap` with write, read, execute and no `(OI)` or `(CI)`
(folder only); `NT SERVICE\EtwDnstap:(OI)(CI)(IO)(M)` (contents only);
`BUILTIN\Administrators:(OI)(CI)(F)`; `NT AUTHORITY\SYSTEM:(OI)(CI)(F)`. The MSI script's
`Test-SpoolFolder` makes the same checks.

##### 4. Set the service identity, privileges, ETW access, and event source

`Grant-EtwDnstapEtwAccess.ps1` grants the service SID `TRACELOG_CREATE_REALTIME` (0x20) on the GUID for
each `-SessionName` (`{05e1db3c-1322-58c7-8235-0463fccba32b}` for `EtwDnstap`) and removes its other ACEs.
`-WhatIf` writes nothing. A real run saves `WMI\Security` and each changed value to `-BackupPath`
([undo](../../scripts/provision/README.md#backup-and-restore)).

```powershell
$backup = New-ProtectedBackupFolder
sc.exe sdshow EtwDnstap | Set-Content -Path "$backup\service-sd.txt" -Encoding ascii
cmd.exe /c 'sc.exe config EtwDnstap obj= "NT AUTHORITY\LocalService" password= ""'
sc.exe sidtype EtwDnstap restricted
sc.exe privs EtwDnstap SeChangeNotifyPrivilege
$expectedSd = 'D:(A;;CCLCSWRPWPDTLOCRRC;;;SY)(A;;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;BA)(A;;CCLCSWLORC;;;IU)(A;;CCLCSWLORC;;;SU)S:(AU;FA;CCDCLCSWRPWPDTLOCRSDRCWDWO;;;WD)'
cmd.exe /c ('sc.exe sdset EtwDnstap "{0}"' -f $expectedSd)
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap -WhatIf
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap -BackupPath "$backup\etw"
New-EventLog -LogName Application -Source EtwDnstap
Restart-Computer
```

##### 5. Configure SCM recovery

Restart after 5 seconds on the first two failures and 60 seconds after; reset the count after one day
(`reset= 86400`); `failureflag 1` applies the actions to non-crash failures.

```powershell
sc.exe failure EtwDnstap reset= 86400 actions= restart/5000/restart/5000/restart/60000
sc.exe failureflag EtwDnstap 1
sc.exe qfailure EtwDnstap
sc.exe qfailureflag EtwDnstap
```

###### Start type

`New-Service` sets Manual start. When [step 7](#7-start-capture-and-check-that-it-is-running) shows
`RUNNING`, set delayed start (`depend=` keeps both dependencies):

```powershell
cmd.exe /c 'sc.exe config EtwDnstap start= delayed-auto depend= DNS/KeyIso'
sc.exe qc EtwDnstap
```

##### 6. Inspect the registered identity and access controls

```powershell
sc.exe query EtwDnstap
sc.exe qc EtwDnstap
sc.exe qsidtype EtwDnstap
sc.exe qprivs EtwDnstap
net localgroup "Performance Log Users"
typeperf -q EtwDnstap
icacls.exe $installDirectory
$actualSd = (sc.exe sdshow EtwDnstap | Where-Object { $_.Trim() }) -join ''
if ($actualSd -ceq $expectedSd) { 'Service descriptor: as expected.' }
else { Write-Warning "Service descriptor differs. Found: $actualSd" }
```

Expect: `qc` shows `NT AUTHORITY\LocalService`, dependencies `DNS` and `KeyIso`, and (after the start type)
`AUTO_START  (DELAYED)`. `qsidtype` shows `RESTRICTED`, `qprivs` only `SeChangeNotifyPrivilege`, and
Performance Log Users lists `NT SERVICE\EtwDnstap`. The `sdshow` string is the step 4 string. Before the
start, `query` isn't `RUNNING` and `typeperf` lists no counters if you skipped `lodctr`.

##### 7. Start capture and check that it is running

```powershell
sc.exe start EtwDnstap
sc.exe query EtwDnstap
typeperf "\EtwDnstap(*)\*" -sc 5
```

Expect `RUNNING` (repeat `query` through `START_PENDING`) and `capture started` (Event ID 2); otherwise see
[troubleshooting](troubleshooting.md#the-service-does-not-reach-running). Gauges read 0 until the first ETW
query succeeds. Then set the [start type](#start-type).

##### 8. Stop the service after the traffic you want

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

Expect `STOPPED`, exit code 0. Check `spool_segments` in the final `output stopped` line: frames the
receiver didn't accept stay in the spool until the next start.

#### Validation checklist for the LocalService restricted profile

Record each result with the binary's commit.

- `RUNNING`. `capture started` (Event ID 2) shows the expected keyword masks, without `keyword_fallback`.
- Under DNS traffic, `received` rises and `receiver_connected` is true.
- With the `0x20` grant and a restart, `StartTraceW` and `EnableTraceEx2` succeed. Without it, `StartTraceW`
  is denied ([fix](#if-an-etw-access-check-is-denied)).
- `sc.exe stop`: `STOPPED`, exit code 0, `output_errors` 0.
- After a kill, the next start reclaims the stale session (`stale_sessions_reclaimed=1`) only when name,
  real-time shape, and GUID all match.
- After a DNS Server crash and recovery restart, capture continues with events from the new process.
- A second `sc.exe start EtwDnstap` fails with error 1056.
- `sc.exe sidtype EtwDnstap unrestricted` stops the service at startup: `the process token is not restricted`.
- `typeperf "\EtwDnstap(*)\*" -sc 5` matches the `capture stopped` counters. The instance is gone after stop.
- Spool: stop the collector under DNS traffic. `spool_segments` and `spooled` rise while `receiver_connected`
  is false. After it returns, `replayed` rises and `spool_bytes` falls.

### Common Gotchas

- **Validated build.** Windows Server 2025, build 26100 ([lab record](../history/localservice-restricted-validation.md)).
  Elsewhere, run the [checklist](#validation-checklist-for-the-localservice-restricted-profile) on a
  disposable host first.
- **Restart.** `Restart-Computer` is required after the grant, because ETW applies descriptors at boot.
  Restart the service after a group membership change.
- **`cmd.exe`.** Run `sc.exe config ... password= ""` and `sdset` through it: Windows PowerShell 5.1 drops
  the empty `""`.
- **`sdset`** replaces the descriptor `New-Service` made. A standard user then gets "Access is denied"
  (error 5) from `sc.exe stop`, `start`, `config`, and `control EtwDnstap 200`.
- **Group membership** must be the **service SID**, enabled in the token, not deny-only.
- **Session names.** The GUID hashes the name without case folding. Pass every `--session-name` the host
  uses, spelled as in the binary path.
- **`New-EventLog`** fails if the source exists. Check
  `[System.Diagnostics.EventLog]::SourceExists('EtwDnstap')`.
- **Failed MSI install.** A first install rolls back by running the uninstall steps. A failed upgrade,
  repair, or `REINSTALL=ALL` keeps the provisioning steps that ran: fix and rerun. A missing or hostname
  `RECEIVER` ends with error `1603`.
- **No spool key.** `spool key is not usable` (Event ID 26), and the service runs without a spool.
- **Counters.** Empty `typeperf -q EtwDnstap`: run `lodctr` (step 3), restart the service. If registration
  fails at startup: `performance counters are unavailable` (Event ID 3), and no counters.
- **Descriptor mismatch.** Someone changed it, or this build prints it differently. Record the string, and
  start only after you know which.
- **DNS restarts.** Stopping `DNS` stops `EtwDnstap`. `Restart-Service DNS` fails, so use `-Force`, then
  start `EtwDnstap`. A DNS crash or restart without an SCM stop leaves it running on the new process.
- **Recovery.** A restart doesn't resend lost data. A restart while `DNS` is stopped fails, and SCM counts it
  ([troubleshooting](troubleshooting.md#the-service-keeps-restarting)).
- **Spool data** is DNS data. Delete it when you no longer need the frames
  ([Uninstall](uninstall.md#7-delete-the-spool-folder)).
- **Spool startup check.** On failure the service logs one warning and runs without a spool: missing folder;
  folder or parent is a reparse point; owner not SYSTEM or Administrators; path with no drive letter on a
  fixed local disk, or with a `.` or `..` part; an allow entry for another account (inherit-only
  `CREATOR OWNER` is fine).

#### If an ETW access check is denied

- **`StartTraceW: access denied`:** The service SID lacks `TRACELOG_CREATE_REALTIME` on the GUID in the
  message, or the computer hasn't restarted since the grant. A changed `--session-name` or case changes the
  GUID. Run `scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` with the name from the
  message, restart, and start the service.
- **`EnableTraceEx2: access denied`:** Inspect
  `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security\eb79061a-a566-4698-9119-3ed2807060e7`, and record the
  OS build and what it holds. Add no grant on the provider GUID.
