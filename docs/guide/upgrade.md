# Repair or upgrade the service

> **Status:** Current · **Describes:** the code on `main` at commit `592a040` · **Reviewed:** 2026-11-05

### Quick Start

Elevated, with a newer MSI. Restart when `msiexec` exits `3010`.

```powershell
msiexec.exe /i etw-dnstap-<new-version>-x64.msi /qn /norestart /l*v upgrade.log
```

New install? See [Install and provision](install.md).

### Steps / Configuration

#### Upgrade, repair, or change settings with the MSI

Run `msiexec` elevated with the same or a newer MSI and `/qn /norestart /l*v <log>`, and restart on `3010`.
The MSI reads the last install's values from `HKLM\SOFTWARE\EtwDnstap\Installer` unless you pass new ones.

##### When the install steps run

The install steps (folder checks, Performance Log Users membership, ETW grant script, recovery policy) run
only in a pass that installs the executable.

| Command | Install steps | Uninstall steps |
| --- | --- | --- |
| First install, `msiexec /i` | Run | Skip |
| Major upgrade: newer MSI, or a rebuild with the same version | Run, in the new product | Skip |
| Repair `/fvomus` or `/fvamus`, or `REINSTALL=ALL` | Run | Skip |
| Maintenance `msiexec /i`, no `REINSTALL` | Skip | Skip |
| Uninstall, `msiexec /x` | Skip | Run |

##### Upgrade to a newer build

The Quick Start command stops the service (it starts after the restart), reuses the remembered
`RECEIVER`, `SESSION_NAME`, and `SERVICE_ARGS` unless you pass new values, and keeps the spool folder, ETW
grants, and Performance Log Users membership.

##### Repair

`o` replaces a file only when it's missing or older; `a` replaces every file
([`msiexec` repair options](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/msiexec#repair-options)).
Repair restores files, rewrites registry values, and sets the service settings again.

```powershell
msiexec.exe /fvomus etw-dnstap-<version>-x64.msi /qn /norestart /l*v repair.log
msiexec.exe /fvamus etw-dnstap-<version>-x64.msi /qn /norestart /l*v repair.log
```

##### Change the receiver or the session name

Reinstall the installed MSI with `REINSTALL=ALL`, then restart (`3010`). The
[name rules](configuration.md#options) apply. The grant script grants the service SID on the new GUID and
removes its grants from every other descriptor.

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v receiver.log `
    REINSTALL=ALL REINSTALLMODE=vomus RECEIVER=192.0.2.20:6000
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v session.log `
    REINSTALL=ALL REINSTALLMODE=vomus SESSION_NAME=EtwDnstap-New
```

#### Snapshot before a change

For a hand-provisioned service. Load the functions from
[Create a protected snapshot folder](install.md#create-a-protected-snapshot-folder) first.

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

The `.reg` files are for reference only. In a repair that runs the grant script, add
`-BackupPath "$backup\etw"`.

#### Restore from the snapshot

Load the same functions, set `$backup`, and run this check first. If it throws, use no file from the
folder.

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

Restore the service key with `sc.exe`, not `reg import`. Fill each `<...>` from the `.txt` file:

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

Then:

1. Match `net localgroup "Performance Log Users"` to `plu.txt` (add or remove `NT SERVICE\EtwDnstap`).
2. If a repair ran the grant script, run it with `-Restore "$backup\etw"`. It checks every entry first and skips values changed since the saving run. Use `-Force` only after you review the skipped values.
3. If you replaced the executable, put the saved copy back ([Replace the executable](#replace-the-executable)).
4. `Restart-Computer` if the script restored any descriptor.
5. Compare the [step 6](install.md#6-inspect-the-registered-identity-and-access-controls) checks with the `.txt` files.

#### Add the spool folder

The spool is on by default, at `C:\ProgramData\EtwDnstap\spool`. Do all three:

1. Take the snapshot ([Snapshot before a change](#snapshot-before-a-change)).
2. Create the folder ([Create the spool folder](install.md#create-the-spool-folder)) in the same session, after the service SID exists (step 3). If the folder exists, inspect it: don't take ownership or reuse it.
3. Replace the [executable](#replace-the-executable) and the [counter manifest](#replace-the-counter-manifest). Without the manifest, counters 52 to 60 are missing from `typeperf -q EtwDnstap`.

Expect `spool_available` `true` in the `output stats` line. To run without a spool, add `--no-spool` to the
binary path.

#### Repair cases

| Symptom | Fix |
| --- | --- |
| LocalSystem or virtual-account install refuses to run | Snapshot. Run the `sc.exe` lines of [step 4](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source). Run `net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add` (an error means already a member). Run `Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` per session name (`-WhatIf` first), then `Restart-Computer`. Keep [step 3](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute) and [step 5](install.md#5-configure-scm-recovery). Start, then `sc.exe qc EtwDnstap`. |
| Start fails `StartTraceW: access denied` | Missing grant or restart: run the grant script, then restart |
| `obj=` left the SID type or privileges (build 26100) | `sc.exe sidtype EtwDnstap restricted`, then `sc.exe qsidtype EtwDnstap` |
| Changed `--session-name` | Grant script with the new name and any still in use, then restart before starting |
| Changed binary path | `sc.exe config EtwDnstap binPath= "..."` (space after `binPath=`) |

#### Replace the executable

Hand-provisioned installs (an MSI install uses [Upgrade to a newer build](#upgrade-to-a-newer-build)). Take
the snapshot first. Run from the folder with the new build, or change the `Copy-Item` source.

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

Expect: hash 1 equals `exe-hash.txt`, hash 2 your build, `icacls` showing `NT SERVICE\EtwDnstap:(I)(RX)`,
then `RUNNING` and a `capture started` line. If the release changes `etw-dnstap.man`, do
[Replace the counter manifest](#replace-the-counter-manifest) between stop and start. To roll back:

```powershell
sc.exe stop EtwDnstap
Remove-Item $exe
Rename-Item "$exe.bak" 'etw-dnstap.exe'
sc.exe start EtwDnstap
```

Remove `etw-dnstap.exe.bak` once the new build works. The snapshot copy stays.

#### Replace the counter manifest

Hand-provisioned installs, when a release changes `etw-dnstap.man`. Snapshot first, and keep a copy of the
old `etw-dnstap.man` (the snapshot omits it). With the service stopped, run from the folder with the new
build, or change the source:

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
unlodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man"
Copy-Item '.\packaging\perfcounters\etw-dnstap.man' $installDirectory
lodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man" "C:\Program Files\EtwDnstap"
```

Start the service and check the counters ([step 7](install.md#7-start-capture-and-check-that-it-is-running)).

### Common Gotchas

#### Downgrade is refused

A lower-version MSI fails with `A newer version of EtwDnstap is already installed. Uninstall it first.` A
same-version rebuild replaces the build (every build has a new `ProductCode`, and `MajorUpgrade` sets
`AllowSameVersionUpgrades="yes"`). To go back, uninstall, restart, and install the older MSI. The uninstall
deletes the spool ([Uninstall with the MSI](uninstall.md#uninstall-with-the-msi)).

#### Other traps

- Same MSI file, same `ProductCode`, new `SESSION_NAME` or `RECEIVER`, no `REINSTALL`: a maintenance run
  that changes nothing (exit `3010`, binary path and remembered value unchanged). Add `REINSTALL=ALL`.
- A `RECEIVER` that fails the [step 1](install.md#1-check-the-receiver-address) check fails the reinstall
  and rolls back file and registry changes, not provisioning steps that ran. Fix it and rerun.
- An MSI built before the skip rule moves the grant to the new GUID on a maintenance run but keeps the old
  name in the binary path (`StartTraceW: access denied`). Upgrade to a current MSI first.
- `SERVICE_ARGS=`, `RECEIVER=`, and `SESSION_NAME=` count as not set and keep the remembered value. To
  clear `SERVICE_ARGS`, check the spool, uninstall, restart, and install again without it.
- Repair needs the installed build's MSI (same `ProductCode`). A rebuild fails `/f` with error 1605: upgrade
  with it instead.
- Spool folder not usable: one warning, `spool folder is not usable; capture continues without a spool`,
  with a `reason` field naming the failed check ([Spool warnings](troubleshooting.md#spool-warnings)). Keep
  the folder during a repair, because it can hold unsent frames.
- `FAILED 1062` from `sc.exe stop` means the service was already stopped.
- `icacls /restore` runs on the parent folder, because `/save` stores paths relative to the parent.
