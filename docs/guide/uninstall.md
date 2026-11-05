# Uninstall

> **Status:** Current · **Describes:** the code on branch `spool-encryption` · **Reviewed:** 2026-11-05

Removing the service does not change the DNS Server role, its configuration, or its
Analytical event log.

Use the MSI to remove an MSI install. If you installed the service by hand, use
[Remove a hand-provisioned install](#remove-a-hand-provisioned-install).

## Uninstall with the MSI

Run `msiexec /x` in an elevated session. Use either the MSI file name or the product code:

```powershell
msiexec.exe /x etw-dnstap-<version>-x64.msi /qn /norestart /l*v uninstall.log
```

```powershell
msiexec.exe /x {<product-code>} /qn /norestart /l*v uninstall.log
```

The product code is the key under `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall`
whose `DisplayName` is `EtwDnstap`. Each build has its own product code.

The uninstall exits with code `3010`. Restart the computer. ETW removes the session grant at boot.
See [Install with the MSI](install.md#install-with-the-msi).

### What the uninstall removes

- The `EtwDnstap` ETW session. Its name is the remembered `SESSION_NAME`, or `EtwDnstap`.
- The service and its registration.
- The `NT SERVICE\EtwDnstap` membership in Performance Log Users. The uninstall finds the membership
  by the service SID, not by the account name.
- Every ETW grant of that service SID. The grant script removes them with `-Remove`.
- The performance counter registration, the install folder `C:\Program Files\EtwDnstap\` with its
  files, and the Application event log source `EtwDnstap`. Existing log entries stay.
- The `spool` and `backup` folders under `C:\ProgramData\EtwDnstap`. Then the `EtwDnstap` folder,
  if it is empty.
- The registry key `HKLM\SOFTWARE\EtwDnstap`, which holds the remembered settings.
- The spool encryption key: the CNG key `EtwDnstap Spool Key`, and the registry key
  `HKLM\SOFTWARE\ETW-DNSTapService\Protect`, which holds `WrappedKey`. The uninstall also deletes
  `HKLM\SOFTWARE\ETW-DNSTapService` if it is then empty. A major upgrade keeps all of these items,
  because they are outside the service key. See [Spool encryption key](install.md#spool-encryption-key).

> **Warning.** The uninstall deletes the spool. Frames that the collector has not received are lost.
> Check `spool_segments` in the last `output stopped` line first. See [Spool](configuration.md#spool).
>
> The uninstall also deletes the `backup` folder. That folder holds the grant script's ETW snapshots,
> including snapshots from before the MSI. Copy any snapshot that you want to keep. You cannot restore
> from these snapshots after the uninstall. To restore the grants, install the MSI again.
>
> A first install that rolls back also deletes the `spool` and `backup` folders, even if they existed
> before the install.

### What the uninstall keeps

- Items in `C:\ProgramData\EtwDnstap` other than `spool` and `backup`. The uninstall logs
  `KEPT (not created by EtwDnstap)` for each one.
- A reparse point (a link or a junction) in `spool` or `backup`, or the `EtwDnstap` folder itself. The
  uninstall logs `KEPT` and does not follow or delete the reparse point.
- A `--spool-dir` folder, Application event log entries, and dnstap records that the collector received.

Each step writes a line that starts with `[EtwDnstap]` to the MSI log. A failed step ends with
`WixQuietExec:   FAILED: <cause>`, and the uninstall rolls back.

If the service group membership cannot be read or removed, the uninstall continues and logs this warning:

```text
[EtwDnstap] WARNING: could not read or remove the membership of service SID <sid> in Performance Log Users (S-1-5-32-559). The uninstall continues. Remove the SID from the group by hand. Error: <message>
```

If the membership remains, remove the SID by hand. Name it by SID, for example
`net localgroup "Performance Log Users" "*S-1-5-80-..." /delete`, because the service account no
longer resolves. A leftover membership grants nothing. If application control or antivirus
blocks the `netapi32` call that the SID path uses, the uninstall also continues. The lab has not run
that case.

A successful step logs `Removed service SID <sid> (NT SERVICE\EtwDnstap) from Performance Log Users.`,
or `Service SID <sid> is not in Performance Log Users.`

If `sc.exe delete EtwDnstap` ran before the uninstall, the uninstall still completes through the service
SID. The lab has not run that case with the current MSI. See
[the plan](../history/code-review-findings-fix-plan.md#lab-validation-not-part-of-this-plans-agents).

### Check the result

After the restart, run these in an elevated Windows PowerShell 5.1 session:

```powershell
sc.exe query EtwDnstap
net localgroup "Performance Log Users"
logman query -ets
typeperf -q EtwDnstap
Test-Path -LiteralPath "$env:ProgramData\EtwDnstap\spool"
```

- `sc.exe query` reports error 1060.
- `net localgroup`, `logman query -ets`, and `typeperf` show no `EtwDnstap` entries.
- `Test-Path` returns `False`.

## Remove a hand-provisioned install

Use these steps for an install that you made by hand, by following the [manual steps](install.md#provision-the-service).
Do not use them on an MSI install. They leave the Windows Installer record in place.

Run the commands in an elevated Windows PowerShell 5.1 session. The lab tested only the `-Remove`
run of the step 2 grant script. The other commands come from the provisioning steps. They were
**not executed** in this documentation pass. Test them on a disposable host first.

### 1. Stop capture

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

If the service is already stopped, `sc.exe stop` reports an error. Continue when `sc.exe query`
shows `STOPPED`.

### 2. Remove the service SID's group membership and ETW grants

Run this step while the service is still registered. After step 3, `NT SERVICE\EtwDnstap` no
longer resolves, and `net localgroup` cannot remove the member. This guide has not tested that case.

```powershell
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /delete
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -Remove -WhatIf
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -Remove
```

`-Remove` strips every ACE for the service SID from every ETW descriptor. That includes grants made
by earlier script versions. Restart the computer, because ETW applies descriptors at boot.

Before its first change, the script saves the `WMI\Security` key and the original bytes of each changed
value to `$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>`, and prints the path. `-Restore <folder>`
undoes the removal, and then you must restart. `-WhatIf` writes no backup. The script derives the SID
from the service name, so this step also works after `sc.exe delete`. The lab tested removal on build
26100. See [scripts/provision/README.md](../../scripts/provision/README.md).

### 3. Remove the service registration

```powershell
sc.exe delete EtwDnstap
```

`sc.exe delete` removes the configuration. A running service is deleted when it stops.

### 4. Remove the Event Log source

```powershell
Remove-EventLog -Source EtwDnstap
```

Existing Application log entries remain until you clear the log through your normal retention process.

### 5. Stop a leftover ETW session

A session can remain after a crash or an abort. Stop it only if it belongs to this service.

```powershell
logman query -ets
logman stop EtwDnstap -ets
```

Use your `--session-name` if it is not the default. Earlier builds named sessions `etw-dnstap-<pid>`.
To stop one of those, find its process ID with `logman query -ets`, and then run
`logman stop etw-dnstap-<pid> -ets`.

### 6. Unregister the performance counters

```powershell
unlodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man"
```

Run it after step 1 and before step 8. Step 8 removes the manifest that the command reads.

### 7. Delete the spool folder

The spool holds frames that never reached the collector. These include client addresses and query
names. If the folder holds segment files, delete them so that no copy is left. The service does not
write to the folder after step 1 stops it.

If the service ran with `--spool-dir`, set `$spoolDirectory` to that path. The binary path in the
`sc.exe qc EtwDnstap` output from before step 3 shows the path. The command stops if it finds a folder,
a link, or a reparse point inside.

```powershell
$spoolDirectory = Join-Path $env:ProgramData 'EtwDnstap\spool'
if (Test-Path -LiteralPath $spoolDirectory) {
    $folder = Get-Item -LiteralPath $spoolDirectory -Force -ErrorAction Stop
    if ($folder.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw 'The spool folder is a reparse point. Inspect it. Do not delete it.'
    }
    $children = @(Get-ChildItem -LiteralPath $spoolDirectory -Force)
    foreach ($child in $children) {
        if ($child.PSIsContainer -or ($child.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "'$($child.FullName)' is a folder or a reparse point. Inspect it. Do not delete it."
        }
    }
    $children | Format-Table FullName, Length, LastWriteTime
    foreach ($child in $children) { Remove-Item -LiteralPath $child.FullName -Force }
    Remove-Item -LiteralPath $spoolDirectory
} else {
    'No spool folder.'
}
```

The listing shows the segment files and `cursor.bin`. The `EtwDnstap` folder and any `backup`
snapshots stay.

### 8. Remove the executable and its directory

Check that the directory holds only the executable and its manifest:

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
Get-ChildItem -Force -Path $installDirectory
Remove-Item -Recurse -Force -Path $installDirectory
```

Removing the directory also removes its ACLs. To keep the directory as evidence, skip this step.

### 9. Check the result

```powershell
sc.exe query EtwDnstap
net localgroup "Performance Log Users"
logman query -ets
typeperf -q EtwDnstap
Test-Path -LiteralPath "$env:ProgramData\EtwDnstap\spool"
```

The results match [Check the result](#check-the-result) for the MSI path. If the service used
`--spool-dir`, check that path instead.

## What is not removed

- Records already sent to the collector, and Application event log entries.
- The `backup` folders under `$env:ProgramData\EtwDnstap`. Only SYSTEM and Administrators can read
  them. Remove them when you no longer need to roll back.
- The provider permission, if you granted one. Reverse it in step 2.
