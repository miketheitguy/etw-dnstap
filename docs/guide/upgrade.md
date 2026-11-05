# Repair or upgrade the service

> **Status:** Current · **Describes:** the code on branch `spool-encryption` · **Reviewed:** 2026-11-05

Use this guide to change an installed service ([Install and provision](install.md) covers a new one).
Take a snapshot first, change only what you need, and keep the identity, privileges, executable ACLs, and
recovery policy.

## Upgrade, repair, or change settings with the MSI

Run `msiexec` from an elevated session with the same MSI file or a newer one, using
`/qn /norestart /l*v <log>`. Restart the computer when `msiexec` exits with `3010`. The MSI keeps the
values of the last install in `HKLM\SOFTWARE\EtwDnstap\Installer` and reads them back unless you pass a
new value.

### When the install steps run

The install steps check the folders, add the Performance Log Users membership, run the ETW grant script,
and set the recovery policy. They run only in a pass that installs the executable.

| Command | Install steps | Uninstall steps |
| --- | --- | --- |
| First install, `msiexec /i` | Run | Skip |
| Major upgrade: a newer MSI, or a rebuild with the same version | Run, in the new product | Skip |
| Repair with `/fvomus` or `/fvamus`, or `REINSTALL=ALL` | Run | Skip |
| Maintenance `msiexec /i` of the installed product, with no `REINSTALL` | Skip | Skip |
| Uninstall, `msiexec /x` | Skip | Run |

The same MSI file with the same `ProductCode` is a maintenance run. With a new `SESSION_NAME` or
`RECEIVER` and no `REINSTALL`, it changes nothing.

### Upgrade to a newer build

```powershell
msiexec.exe /i etw-dnstap-<new-version>-x64.msi /qn /norestart /l*v upgrade.log
```

The upgrade stops the service, which starts after the restart. It reuses the remembered `RECEIVER`,
`SESSION_NAME`, and `SERVICE_ARGS` unless you pass new values, and keeps the spool folder, the ETW grants,
and the Performance Log Users membership.

### Downgrade is refused

An MSI with a lower version is refused with `A newer version of EtwDnstap is already installed. Uninstall it first.`
A rebuilt MSI with the same version replaces the installed build, because every build has a new
`ProductCode` and `MajorUpgrade` sets `AllowSameVersionUpgrades="yes"`.

To return to an older build, uninstall the current one, restart the computer, and install the older MSI.
The uninstall deletes the spool ([Uninstall with the MSI](uninstall.md#uninstall-with-the-msi)).

### Repair

```powershell
msiexec.exe /fvomus etw-dnstap-<version>-x64.msi /qn /norestart /l*v repair.log
```

Repair restores missing or older files, rewrites the registry values, and sets the service settings again.
Restart the computer when it exits with `3010`.

- **Same `ProductCode` only.** Use the MSI file of the installed build. A rebuilt MSI fails `/f` with
  error 1605. To replace the installed build, run the rebuild as an upgrade.
- **Which files.** `o` replaces a file only when it is missing or older. To replace every file, use `a`
  in place of `o` ([`msiexec` repair options](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/msiexec#repair-options)):

  ```powershell
  msiexec.exe /fvamus etw-dnstap-<version>-x64.msi /qn /norestart /l*v repair.log
  ```

### Empty values keep the remembered value

`SERVICE_ARGS=`, `RECEIVER=`, and `SESSION_NAME=` count as not set and keep the remembered value. To clear
`SERVICE_ARGS`, check the spool, uninstall, restart, and install again without it
([Uninstall with the MSI](uninstall.md#uninstall-with-the-msi)).

### Change the receiver

Reinstall the installed MSI with the new value. `REINSTALL=ALL` is required, because a plain `msiexec /i`
with a new `RECEIVER=` exits with `3010` but changes neither the binary path nor the remembered value.

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v receiver.log `
    REINSTALL=ALL REINSTALLMODE=vomus RECEIVER=192.0.2.20:6000
```

The reinstall stops the service and exits with `3010`. Restart the computer. A value that fails the
[step 1](install.md#1-check-the-receiver-address) check fails the reinstall and rolls back the file and
registry changes, but not provisioning steps that already ran. Fix the value and run the command again.

### Change the session name

Reinstall with the new name, as for the receiver:

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v session.log `
    REINSTALL=ALL REINSTALLMODE=vomus SESSION_NAME=EtwDnstap-New
```

The name rules are in [configuration.md](configuration.md#options). `REINSTALL=ALL` is required. An
MSI built before the skip rule moves the grant to the new GUID on a maintenance run but keeps the old
name in the binary path, and the service fails with `StartTraceW: access denied`. Upgrade to a current MSI
first.

The grant script grants the service SID on the new GUID and removes its grants from every other
descriptor. Restart the computer after the install.

## Snapshot before a change

This section applies to a hand-provisioned service. Run the functions from
[Create a protected snapshot folder](install.md#create-a-protected-snapshot-folder) first, then:

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

The folder holds the account, SID type, privileges, recovery policy, service descriptor, directory ACLs,
group membership, and a copy of the executable. Use the `.reg` files for reference only. In a repair that
runs the grant script, add `-BackupPath "$backup\etw"`.

## Restore from the snapshot

Run the functions from [Create a protected snapshot folder](install.md#create-a-protected-snapshot-folder),
set `$backup` to the snapshot folder, and run this check. If it throws, use no file from the folder.

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

Restore the service key with `sc.exe`, not `reg import`, which can leave the SCM and the registry in
disagreement. Replace each `<...>` with the value in the `.txt` file:

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

Then restore the rest of the snapshot:

1. Compare `net localgroup "Performance Log Users"` with `plu.txt`. Add or remove `NT SERVICE\EtwDnstap` so that they match.
2. If a repair ran the grant script, run it with `-Restore "$backup\etw"`. The script checks every entry before it applies any, and skips a value that changed after the run that saved it. Use `-Force` only after you review the skipped values.
3. If you replaced the executable, put the saved copy back ([Replace the executable](#replace-the-executable)).
4. Run `Restart-Computer` if the grant script restored any descriptor.
5. Compare the checks in [step 6](install.md#6-inspect-the-registered-identity-and-access-controls) with the saved `.txt` files.

`icacls /restore` runs on the parent folder, because `/save` stores paths relative to the parent.

## Add the spool folder

The disk spool is on by default, at `C:\ProgramData\EtwDnstap\spool`. Make these changes together:

1. Take the snapshot ([Snapshot before a change](#snapshot-before-a-change)).
2. Create the folder with [Create the spool folder](install.md#create-the-spool-folder), in the same PowerShell session. The service SID from step 3 must already be registered. If the folder exists, inspect it instead of taking ownership or reusing it.
3. Replace the executable and the counter manifest ([Replace the executable](#replace-the-executable), [Replace the counter manifest](#replace-the-counter-manifest)). Without the manifest, counters 52 to 60 do not appear in `typeperf -q EtwDnstap`.

Until the folder passes the checks, the service logs one warning, `spool folder is not usable; capture continues without a spool` (its `reason` field names the failed check; see [Spool warnings](troubleshooting.md#spool-warnings)), and runs without a spool. When the spool is in use, `spool_available` in the `output stats` line reads `true`. Keep the folder during a repair, because it can hold unsent frames. To run without a spool, add `--no-spool` to the binary path.

## Repair cases

| Case | Command or action |
| --- | --- |
| LocalSystem or virtual-account upgrade (the service refuses to run) | 1. Take the snapshot. 2. Run the `sc.exe` lines from [step 4](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source). 3. Run `net localgroup` to add `NT SERVICE\EtwDnstap` to Performance Log Users (an error means it is already a member). 4. Run `Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` for every session name, with `-WhatIf` first, then `Restart-Computer`. Without the grant and restart, the start fails with `StartTraceW: access denied`. 5. Keep the grant from [step 3](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute) and the recovery policy from [step 5](install.md#5-configure-scm-recovery). 6. Start the service and check `sc.exe qc EtwDnstap`. |
| `obj=` did not clear the SID type or privilege list (build 26100) | After any `obj=` change, run `sc.exe sidtype EtwDnstap restricted` and check `sc.exe qsidtype EtwDnstap`. |
| Changing `--session-name` | Run the grant script with the new name and any others still in use. Restart before you start the service. |
| Changing the binary path | `sc.exe config EtwDnstap binPath= "..."`, with a space after `binPath=`. |

## Replace the executable

For a hand-provisioned install (an MSI install uses [Upgrade to a newer build](#upgrade-to-a-newer-build)).
Take the snapshot first, and run from the folder with the new build under
`target\x86_64-pc-windows-msvc\release\`, or change the `Copy-Item` source. `FAILED 1062` from
`sc.exe stop` means the service was already stopped.

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

- The first hash is the old executable, to compare with `exe-hash.txt`. The second is the new one, to compare with your build.
- `icacls` must show `NT SERVICE\EtwDnstap:(I)(RX)` for the new file, or the service cannot start.
- The service must reach `RUNNING`, and a `capture started` line must appear.
- If the release also changes `etw-dnstap.man`, follow [Replace the counter manifest](#replace-the-counter-manifest) between the stop and the start.

To roll back, stop the service, remove the new file, rename the backup back, and start the service:

```powershell
sc.exe stop EtwDnstap
Remove-Item $exe
Rename-Item "$exe.bak" 'etw-dnstap.exe'
sc.exe start EtwDnstap
```

When the new executable works, remove `etw-dnstap.exe.bak`. The snapshot copy stays until you remove the snapshot folder.

## Replace the counter manifest

For a hand-provisioned install, when a release changes `etw-dnstap.man`. Take the snapshot first. Run
these while the service is stopped, from the folder with the new manifest under `packaging\perfcounters\`,
or change the `Copy-Item` source.

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
unlodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man"
Copy-Item '.\packaging\perfcounters\etw-dnstap.man' $installDirectory
lodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man" "C:\Program Files\EtwDnstap"
```

Start the service and check the counters as in [step 7](install.md#7-start-capture-and-check-that-it-is-running).
The snapshot omits the manifest, so keep a copy of the old `etw-dnstap.man` for a rollback.

## Next steps

- [Troubleshooting](troubleshooting.md)
- [Uninstall](uninstall.md)
