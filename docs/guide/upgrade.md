# Repair or upgrade the service

> **Status:** Current · **Describes:** the MSI sources at commit `58fde72` · **Reviewed:** 2026-11-03

Use this guide to change an installed service. To install a new one, see [Install and provision](install.md). Change only what you need. Keep the identity, privileges, executable ACLs, and recovery policy. Take a snapshot first, and restore saved values as they were. Do not reset a value to a guessed default.

## Upgrade, repair, or change settings with the MSI

Run `msiexec` from an elevated session with the same MSI file, or a newer one. Use `/qn /norestart /l*v <log>`. Restart the computer when `msiexec` exits with `3010`. The MSI keeps the values of the last install in `HKLM\SOFTWARE\EtwDnstap\Installer`, and each run reads them back unless you pass a new value.

Lab validation passed on one lab DNS Server (Windows Server 2025, build 26100) on 2026-11-03: install, a failed install and its rollback, repair, a receiver change, and uninstall, each with a restart. A major upgrade from an older MSI is untested. See [the plan and its results](../history/msi-installer-plan.md).

### When the install steps run

The install steps check the folders, add the Performance Log Users membership, run the ETW grant script, and set the recovery policy. They run only in a pass that installs the executable, so the grant and the binary path match `SESSION_NAME`.

| Command | Install steps | Uninstall steps |
| --- | --- | --- |
| First install, `msiexec /i` | Run | Skip |
| Major upgrade: a newer MSI, or a rebuild with the same version | Run, in the new product | Skip. The removal of the old product is part of the upgrade. |
| Repair with `/fvomus` or `/fvamus`, or `REINSTALL=ALL` | Run | Skip |
| Maintenance `msiexec /i` of the installed product with no `REINSTALL` | Skip | Skip |
| Uninstall, `msiexec /x` | Skip | Run |

The same MSI file, with the same `ProductCode`, is a maintenance run, not an upgrade. A maintenance run with a new `SESSION_NAME` or `RECEIVER` and no `REINSTALL` changes nothing. A product installed from an MSI built before this rule keeps the old behavior until you upgrade it. The lab has not yet run the maintenance case with the current MSI. See [the plan](../history/code-review-findings-fix-plan.md#lab-validation-not-part-of-this-plans-agents).

### Upgrade to a newer build

```powershell
msiexec.exe /i etw-dnstap-<new-version>-x64.msi /qn /norestart /l*v upgrade.log
```

The newer MSI removes the installed product and installs the new one in one transaction. The service stops during the upgrade and starts after the restart. It reuses the remembered `RECEIVER`, `SESSION_NAME`, and `SERVICE_ARGS` unless you pass them, and it keeps the spool folder, the ETW grants, and the Performance Log Users membership.

### Upgrade to a build with spool encryption

A build with spool encryption writes encrypted segments. The MSI writes the service dependency list again, so the service depends on `DNS` and `KeyIso`. Check it with `sc.exe qc EtwDnstap`. The spool key (`WrappedKey`) is kept on a major upgrade.

At its first start, the new build makes a spool key and deletes every spool segment it cannot read. That includes the plaintext segments of the earlier build. It counts their bytes in `spool_dropped_bytes` and logs one warning with the counts. Drain the spool before you upgrade to keep those frames. See [Spool key errors](troubleshooting.md#spool-key-errors).

### Downgrade is refused

An MSI with a lower version is refused with `A newer version of EtwDnstap is already installed. Uninstall it first.`

A rebuilt MSI with the same version is not refused. Every build has a new `ProductCode`, and `MajorUpgrade` sets `AllowSameVersionUpgrades="yes"`, so the rebuild replaces the installed build in place. Without that setting, it would install beside it as a second product. The lab has not yet run a same-version upgrade.

To return to an older build, uninstall the current one, restart the computer, and install the older MSI. The uninstall deletes the spool. See [Uninstall with the MSI](uninstall.md#uninstall-with-the-msi).

### Repair

```powershell
msiexec.exe /fvomus etw-dnstap-<version>-x64.msi /qn /norestart /l*v repair.log
```

Repair restores missing or older files, rewrites the registry values, and sets the service settings again. It keeps the remembered values, stops the service without starting it, and exits with `3010`. Restart the computer, and the service starts with the delayed start.

- **Same `ProductCode` only.** Use the MSI file of the installed build. A rebuilt MSI fails `/f` with error 1605. To replace the installed build, run the rebuild as an upgrade. The lab did not test the error.
- **Which files.** `o` replaces a file only when it is missing or older, so a damaged executable with the same version stays. To replace every file, use `a` in place of `o`:

  ```powershell
  msiexec.exe /fvamus etw-dnstap-<version>-x64.msi /qn /norestart /l*v repair.log
  ```

  See the [`msiexec` repair options](https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/msiexec#repair-options).

### Empty values keep the remembered value

An empty property counts as not set, so `SERVICE_ARGS=` does not clear the remembered value. The same applies to `RECEIVER=` and `SESSION_NAME=`. To clear `SERVICE_ARGS`, uninstall (which deletes the spool, so check it first), restart, and install again without it. See [Uninstall with the MSI](uninstall.md#uninstall-with-the-msi).

### Change the receiver

Reinstall the installed MSI with the new value:

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v receiver.log `
    REINSTALL=ALL REINSTALLMODE=vomus RECEIVER=192.0.2.20:6000
```

`REINSTALL=ALL` is required. In the lab, a plain `msiexec /i` with `RECEIVER=` changed neither the binary path nor the remembered value, although it exited with `3010`. The `REINSTALL=ALL` form changed both.

The reinstall stops the service without starting it, and exits with `3010`. Restart the computer. The MSI checks the value as in [step 1](install.md#1-check-the-receiver-address), and a bad value rolls the install back. The ETW grants depend on `SESSION_NAME`, not `RECEIVER`.

### Change the session name

Reinstall with the new name, as for the receiver:

```powershell
msiexec.exe /i etw-dnstap-<version>-x64.msi /qn /norestart /l*v session.log `
    REINSTALL=ALL REINSTALLMODE=vomus SESSION_NAME=EtwDnstap-New
```

The name must be 1 to 128 ASCII letters, digits, underscore, dot, or hyphen, and it must not start with a hyphen. A launch condition refuses a name that starts with `-`. The provisioning script rejects any other invalid name, and the install fails and rolls back. See [configuration.md](configuration.md#options).

Without `REINSTALL=ALL`, the run is a maintenance run, and the grant, the binary path, and the remembered name stay as they were. A product installed from an MSI built before the skip rule still runs the install steps on that run. The grant then moves to the new GUID, but the binary path keeps the old name, and the service fails with `StartTraceW: access denied`. Upgrade to a current MSI first. The lab tested the reinstall form for a receiver change only.

The new session name has its own GUID. The grant script grants the service SID on that GUID, and removes its grants from the other GUIDs that it manages. Restart the computer after the install, because the grant applies at boot.

## Snapshot before a change

This section applies to a service that you provisioned by hand. For an MSI install, use the MSI as above.

Run the functions from [Create a protected snapshot folder](install.md#create-a-protected-snapshot-folder) first, then:

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

The folder holds the account, SID type, privileges, recovery policy, service descriptor, directory ACLs, group membership, and a copy of the executable. Only SYSTEM and Administrators can read it. The `.reg` files are for reference only. Do not restore the service from them. In a repair that runs the grant script, add `-BackupPath "$backup\etw"` so its backup is in the same folder.

## Restore from the snapshot

Check the snapshot folder first, because someone may have changed it. Stop if this check throws, and do not use any file from the folder. Set `$backup` to the snapshot folder, after you run the functions from [Create a protected snapshot folder](install.md#create-a-protected-snapshot-folder).

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

Do not use `reg import` for the service key. The SCM keeps the service configuration in memory, and a registry import can leave the SCM and the registry in disagreement. Use `sc.exe` instead. Read the values from the `.txt` files, and replace each `<...>` with the value in the file.

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
2. If a repair ran the grant script, run it with `-Restore "$backup\etw"`. It checks every entry before it applies any, and it skips a value that changed after the run that saved it. Use `-Force` only after you review the skipped values.
3. If you replaced the executable, put the saved copy back. See [Replace the executable](#replace-the-executable).
4. Run `Restart-Computer` if the grant script restored any descriptor. ETW applies descriptors at boot.
5. Compare the checks in [step 6](install.md#6-inspect-the-registered-identity-and-access-controls) with the saved `.txt` files.

`icacls /restore` runs on the parent folder, because `/save` stores paths relative to the parent.

## Add the spool folder

This build adds the disk spool, which is on by default, at `C:\ProgramData\EtwDnstap\spool`. Make these changes together:

1. Take the snapshot, as in [Snapshot before a change](#snapshot-before-a-change).
2. Create the folder with [Create the spool folder](install.md#create-the-spool-folder), in the same PowerShell session. The service SID from step 3 must already be registered. `New-ProtectedFolder` stops if the folder exists. Inspect it first, and do not take ownership of it or reuse it without that check.
3. Replace the executable and the counter manifest, as in [Replace the executable](#replace-the-executable) and [Replace the counter manifest](#replace-the-counter-manifest). The manifest change is required. Without it, counters 52 to 60 do not appear in `typeperf -q EtwDnstap`.

Until the folder exists and passes the checks, the service logs one warning, `spool folder is not usable; capture continues without a spool`, and runs without a spool. Its `reason` names the failed check. See [Spool warnings](troubleshooting.md#spool-warnings).

Do not delete the spool folder during a repair. It can hold frames that were not sent, and the snapshot does not include it. To run without a spool, add `--no-spool` to the binary path (see [Changing the binary path](#repair-cases)). When the spool is in use, `spool_available` in the `output stats` line reads `true`.

## Repair cases

- **LocalSystem or virtual-account upgrade.** The service refuses to run with this build until you:
  1. Take the snapshot.
  2. Set the identity, SID type, and privileges with the `sc.exe` lines from [step 4](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source).
  3. Add `NT SERVICE\EtwDnstap` to Performance Log Users with `net localgroup`. An error means it is already a member.
  4. Run `Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` for every session name, with `-WhatIf` first. Then run `Restart-Computer`. Without the grant and restart, the start fails with `StartTraceW: access denied`.
  5. Keep the grant from [step 3](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute) and the recovery policy from [step 5](install.md#5-configure-scm-recovery).
  6. Start the service, and check the account with `sc.exe qc EtwDnstap`.

  On build 26100, `obj=` did not clear the SID type or the privilege list. After any `obj=` change, run `sc.exe sidtype EtwDnstap restricted`, and check it with `sc.exe qsidtype EtwDnstap`.
- **First grant-script version** (commit `1a54272`). It granted `0x423` on the default GUID and `0x80` on the DNS provider GUID. Replace the executable, run the current script with `-SessionName`, and restart. The script removes the old grants and the provider descriptor. The lab tested this path.
- **Changing `--session-name`.** Run the grant script with the new name and any others still in use. Restart before you start the service.
- **Removed arguments.** `--output`, `--max-output-bytes`, and `--metrics` fail parsing. Remove them from the binary path, and add `--receiver` with the collector's numeric IP and port.
- **Changing the binary path.** Use `sc.exe config EtwDnstap binPath= "..."`, and keep the space after `binPath=`.

## Replace the executable

Take the snapshot first. Run these from the folder with the new `etw-dnstap.exe` under `target\x86_64-pc-windows-msvc\release\`, or change the `Copy-Item` source. If the service is already stopped, `sc.exe stop` prints `FAILED 1062`, which is expected.

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
- `icacls` must show `NT SERVICE\EtwDnstap:(I)(RX)` for the new file, or the service cannot start. Do not change the other ACLs.
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

Do this only when a new release changes `etw-dnstap.man`. The spool release does, so [Add the spool folder](#add-the-spool-folder) calls this section. Take the snapshot first. Run these while the service is stopped, between `sc.exe stop` and `sc.exe start`, from the folder with the new `etw-dnstap.man` under `packaging\perfcounters\`, or with the `Copy-Item` source changed.

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
unlodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man"
Copy-Item '.\packaging\perfcounters\etw-dnstap.man' $installDirectory
lodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man" "C:\Program Files\EtwDnstap"
```

Run `unlodctr` before you replace the file, to remove the old registration, and `lodctr` after the copy, to register the new one. Start the service, and check the counters as in [step 7](install.md#7-start-capture-and-check-that-it-is-running). The snapshot folder does not hold the manifest. Keep a copy of the old `etw-dnstap.man` if you may need to roll back.

## Next steps

- [Troubleshooting](troubleshooting.md)
- [Uninstall](uninstall.md)
