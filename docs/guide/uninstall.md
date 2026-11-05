# Uninstall

> **Status:** Current · **Describes:** the code on branch `spool-encryption` · **Reviewed:** 2026-11-05

Removing the service leaves the DNS Server role, its configuration, and its Analytical event log
unchanged. Use the MSI to remove an MSI install. For a hand-provisioned install, see
[Remove a hand-provisioned install](#remove-a-hand-provisioned-install).

## Uninstall with the MSI

Run `msiexec /x` in an elevated session, with the MSI file name or the product code:

```powershell
msiexec.exe /x etw-dnstap-<version>-x64.msi /qn /norestart /l*v uninstall.log
```

```powershell
msiexec.exe /x {<product-code>} /qn /norestart /l*v uninstall.log
```

The product code is the key under `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall` whose
`DisplayName` is `EtwDnstap`. Each build has its own product code. The uninstall exits with code `3010`.
Restart the computer, because ETW removes the session grant at boot.

> **Warning.** The uninstall deletes the spool, so frames that the collector has not received are lost.
> Check `spool_segments` in the last `output stopped` line first ([Spool](configuration.md#spool)).
>
> It also deletes the `backup` folder, which holds the grant script's ETW snapshots, including snapshots
> from before the MSI. Copy any snapshot you want to keep. To restore the grants afterward, install the MSI
> again. A first install that rolls back also deletes `spool` and `backup`, even if they existed before.

### What the uninstall removes

| Removed | Kept |
| --- | --- |
| The ETW session (the remembered `SESSION_NAME`, or `EtwDnstap`) | Items in `C:\ProgramData\EtwDnstap` other than `spool` and `backup`. The uninstall logs `KEPT (not created by EtwDnstap)` for each. |
| The service and its registration | A reparse point (link or junction) in `spool` or `backup`, or the `EtwDnstap` folder itself. The uninstall logs `KEPT`. |
| The `NT SERVICE\EtwDnstap` membership in Performance Log Users, found by service SID | A `--spool-dir` folder |
| Every ETW grant of that service SID (the grant script with `-Remove`) | Application event log entries |
| The performance counter registration, `C:\Program Files\EtwDnstap\` with its files, and the Application event log source `EtwDnstap` | dnstap records that the collector received |
| The `spool` and `backup` folders under `C:\ProgramData\EtwDnstap`, then `EtwDnstap` if empty | |
| `HKLM\SOFTWARE\EtwDnstap` (the remembered settings) | |
| The spool encryption key: the CNG key `EtwDnstap Spool Key`, `HKLM\SOFTWARE\ETW-DNSTapService\Protect` (which holds `WrappedKey`), and `HKLM\SOFTWARE\ETW-DNSTapService` if then empty | |

A major upgrade keeps the spool encryption key ([Spool encryption key](install.md#spool-encryption-key)).

Each step writes an `[EtwDnstap]` line to the MSI log. A failed step ends with
`WixQuietExec:   FAILED: <cause>`, and the uninstall rolls back. The group step logs
`Removed service SID <sid> (NT SERVICE\EtwDnstap) from Performance Log Users.` or
`Service SID <sid> is not in Performance Log Users.` If it cannot read or remove the membership, the
uninstall continues and logs:

```text
[EtwDnstap] WARNING: could not read or remove the membership of service SID <sid> in Performance Log Users (S-1-5-32-559). The uninstall continues. Remove the SID from the group by hand. Error: <message>
```

Remove the SID by hand, naming it by SID because the service account no longer resolves, for example
`net localgroup "Performance Log Users" "*S-1-5-80-..." /delete`. A leftover membership grants nothing.

### Check the result

After the restart, in an elevated Windows PowerShell 5.1 session:

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

Use these steps only for an install made by the [manual steps](install.md#provision-the-service); on an
MSI install they leave the Windows Installer record in place. Run them in an elevated Windows PowerShell
5.1 session, and test them on a disposable host first.

### 1. Stop capture

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

If the service is already stopped, `sc.exe stop` reports an error. Continue when `sc.exe query` shows
`STOPPED`.

### 2. Remove the service SID's group membership and ETW grants

Run this step while the service is registered. After step 3, `NT SERVICE\EtwDnstap` no longer resolves,
and `net localgroup` cannot remove the member.

```powershell
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /delete
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -Remove -WhatIf
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -Remove
```

`-Remove` strips every ACE for the service SID from every ETW descriptor, then you restart the computer.
Before its first change, the script saves the `WMI\Security` key and the original bytes of each changed
value to `$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>` and prints the path. `-Restore <folder>`
undoes the removal (then restart). `-WhatIf` writes no backup. The script derives the SID from the service
name, so this step also works after `sc.exe delete`. See
[scripts/provision/README.md](../../scripts/provision/README.md).

### 3. Remove the service registration

```powershell
sc.exe delete EtwDnstap
```

A running service is deleted when it stops.

### 4. Remove the Event Log source

```powershell
Remove-EventLog -Source EtwDnstap
```

Existing Application log entries remain until your normal retention clears them.

### 5. Stop a leftover ETW session

Stop a session that a crash left behind, if it belongs to this service. Use your `--session-name` if it
is not the default.

```powershell
logman query -ets
logman stop EtwDnstap -ets
```

### 6. Unregister the performance counters

```powershell
unlodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man"
```

Run it after step 1 and before step 8, which removes the manifest that the command reads.

### 7. Delete the spool folder

The spool holds frames that never reached the collector, including client addresses and query names.
Delete any segment files so that no copy is left. If the service ran with `--spool-dir`, set
`$spoolDirectory` to that path, which the binary path in the `sc.exe qc EtwDnstap` output from before
step 3 shows. The command stops if it finds a folder, link, or reparse point inside.

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

The listing shows the segment files and `cursor.bin`. The `EtwDnstap` folder and any `backup` snapshots stay.

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
- The `backup` folders under `$env:ProgramData\EtwDnstap`. Remove them when you no longer need to roll back.
- The provider permission, if you granted one. Reverse it in step 2.
