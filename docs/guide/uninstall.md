# Uninstall

> **Status:** Current · **Describes:** the code on branch `spool-encryption` · **Reviewed:** 2026-11-05

### Quick Start

Elevated. The uninstall exits `3010`: restart, because ETW removes the session grant at boot.

```powershell
msiexec.exe /x etw-dnstap-<version>-x64.msi /qn /norestart /l*v uninstall.log
```

> **Warning.** This deletes the spool (unsent frames are lost, so check `spool_segments` in the last
> `output stopped` line first: [Spool](configuration.md#spool)) and the `backup` folder of ETW snapshots,
> including ones from before the MSI. Copy what you keep. To restore the grants afterward, install the MSI
> again.

Hand-provisioned? See [Remove a hand-provisioned install](#remove-a-hand-provisioned-install).

### Steps / Configuration

#### Uninstall with the MSI

Use the file name, or the product code (the key under
`HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall` whose `DisplayName` is `EtwDnstap`; each build has
its own):

```powershell
msiexec.exe /x {<product-code>} /qn /norestart /l*v uninstall.log
```

##### What the uninstall removes

| Removed | Kept |
| --- | --- |
| ETW session (remembered `SESSION_NAME`, or `EtwDnstap`) | Items in `C:\ProgramData\EtwDnstap` other than `spool` and `backup` (logged `KEPT (not created by EtwDnstap)`) |
| Service and its registration | A reparse point (link, junction) in `spool` or `backup`, or the `EtwDnstap` folder itself (logged `KEPT`) |
| `NT SERVICE\EtwDnstap` in Performance Log Users (found by SID) | A `--spool-dir` folder |
| Every ETW grant for that SID (grant script `-Remove`) | Application event log entries |
| Counter registration, `C:\Program Files\EtwDnstap\` with its files, event source `EtwDnstap` | Records the collector received |
| `spool` and `backup` under `C:\ProgramData\EtwDnstap`, then `EtwDnstap` if empty | |
| `HKLM\SOFTWARE\EtwDnstap` (remembered settings) | |
| Spool encryption key: CNG key `EtwDnstap Spool Key`, `HKLM\SOFTWARE\ETW-DNSTapService\Protect` (`WrappedKey`), `HKLM\SOFTWARE\ETW-DNSTapService` if then empty | |

A major upgrade keeps the key ([Spool encryption key](install.md#spool-encryption-key)).

Each step writes an `[EtwDnstap]` line to the MSI log. A failed step ends
`WixQuietExec:   FAILED: <cause>` and rolls the uninstall back. The group step logs
`Removed service SID <sid> (NT SERVICE\EtwDnstap) from Performance Log Users.` or
`Service SID <sid> is not in Performance Log Users.` If it can't read or remove the membership, the
uninstall continues and logs:

```text
[EtwDnstap] WARNING: could not read or remove the membership of service SID <sid> in Performance Log Users (S-1-5-32-559). The uninstall continues. Remove the SID from the group by hand. Error: <message>
```

Remove it by SID, because the account no longer resolves:
`net localgroup "Performance Log Users" "*S-1-5-80-..." /delete`. A leftover membership grants nothing.

##### Check the result

After the restart, in elevated Windows PowerShell 5.1:

```powershell
sc.exe query EtwDnstap
net localgroup "Performance Log Users"
logman query -ets
typeperf -q EtwDnstap
Test-Path -LiteralPath "$env:ProgramData\EtwDnstap\spool"
```

Expect error 1060, then no `EtwDnstap` entries, then `False`.

#### Remove a hand-provisioned install

Only for installs made by the [manual steps](install.md#provision-the-service). On an MSI install these
leave the Windows Installer record. Use elevated Windows PowerShell 5.1, and test on a disposable host
first.

##### 1. Stop capture

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

Expect `STOPPED`. If the service was already stopped, `stop` reports an error.

##### 2. Remove the service SID's group membership and ETW grants

Run this while the service is registered: after step 3, `NT SERVICE\EtwDnstap` no longer resolves.

```powershell
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /delete
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -Remove -WhatIf
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -Remove
```

`-Remove` strips every ACE for the service SID from every ETW descriptor (restart afterward). It first saves
`WMI\Security` and each changed value to `$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>` and prints
the path. `-Restore <folder>` undoes it (restart again). `-WhatIf` writes no backup. It derives the SID from
the service name, so it works after `sc.exe delete` too
([scripts/provision/README.md](../../scripts/provision/README.md)). Reverse any provider permission you
granted here.

##### 3. Remove the service registration

```powershell
sc.exe delete EtwDnstap
```

A running service is deleted when it stops.

##### 4. Remove the Event Log source

```powershell
Remove-EventLog -Source EtwDnstap
```

Application log entries stay until normal retention clears them.

##### 5. Stop a leftover ETW session

Use your `--session-name` if it isn't the default.

```powershell
logman query -ets
logman stop EtwDnstap -ets
```

##### 6. Unregister the performance counters

Before step 8, which removes the manifest this reads.

```powershell
unlodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man"
```

##### 7. Delete the spool folder

The spool holds client addresses and query names, so delete every segment. For `--spool-dir`, set
`$spoolDirectory` to the path in the binary path from `sc.exe qc EtwDnstap` (read it before step 3). The
command stops on a folder, link, or reparse point inside.

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

Expect the segment files and `cursor.bin` listed. The `EtwDnstap` folder and `backup` snapshots stay.

##### 8. Remove the executable and its directory

Check that it holds only the executable and its manifest. This also removes its ACLs; skip it to keep the
directory as evidence.

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
Get-ChildItem -Force -Path $installDirectory
Remove-Item -Recurse -Force -Path $installDirectory
```

##### 9. Check the result

Run the [MSI checks](#check-the-result). With `--spool-dir`, test that path instead.

### Common Gotchas

- A first install that rolls back also deletes `spool` and `backup`, even if they existed before.
- Manual removal keeps the `backup` folders under `$env:ProgramData\EtwDnstap` (delete them when you no
  longer need to roll back), records already sent, and Application log entries.
