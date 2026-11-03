# Uninstall

> **Status:** Current · **Describes:** `tcp-sink-spool` at commit `0569136` · **Reviewed:** 2026-11-03

Removing the service does not change the DNS Server role, its configuration, or
its Analytical event log. The service never enabled those.

Run the commands in an elevated Windows PowerShell 5.1 session. The lab tested
the `-Remove` run of the grant script in step 3. The other commands come from
the provisioning steps in [install.md](install.md). They were **not executed**
in this documentation pass. Test them on a disposable host first.

## 1. Stop capture

```powershell
sc.exe stop EtwDnstap
sc.exe query EtwDnstap
```

If the service is already stopped, `sc.exe stop` reports an error. That is
fine. Continue when `sc.exe query` shows `STOPPED`.

## 2. Remove the service registration

```powershell
sc.exe delete EtwDnstap
```

`sc.exe delete` removes the service's configuration. That includes the binary
path, the account (`NT AUTHORITY\LocalService`), the SID type, the privilege list,
and the recovery policy. If the service is still running, SCM marks it for
deletion and removes it when it stops.

## 3. Remove the service SID's group membership and ETW grants

```powershell
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /delete
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -Remove -WhatIf
.\scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -Remove
```

Provisioning added the service SID to Performance Log Users and granted it
`TRACELOG_CREATE_REALTIME` on each session GUID. Removing the service removes
neither. Remove them here, so no SID keeps ETW rights after the service is gone.
`-Remove` strips every ACE for the service SID from every ETW descriptor. It
removes any descriptor other than the default that then matches the default.
That also covers the grants of earlier versions of the script. Restart the
computer afterwards, because ETW applies these descriptors at boot. Run `net
localgroup "Performance Log Users"` to check that the entry is gone.

Before its first change, the script exports the `WMI\Security` key and saves
the original bytes of each value it changes. The folder is
`$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>`, and the script prints
its path. To undo the removal, run the script with `-Restore <folder>` and
restart the computer. With `-WhatIf`, the script writes no backup.

The script derives the service SID from the service name, so `-Remove` works
after `sc.exe delete` as well. It strips only the ACEs for that SID. It prints
one line for each descriptor that it changes or removes, and the SDDL of the
default descriptor. It edits the descriptors under
`HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security`. The lab tested removal on
build 26100. See
[scripts/provision/README.md](../../scripts/provision/README.md).

## 4. Remove the Event Log source

```powershell
Remove-EventLog -Source EtwDnstap
```

This removes the `EtwDnstap` source registration. It does not remove existing
entries in the Application log. Those remain until you clear the log through your
normal retention process.

## 5. Stop a leftover ETW session

A session can remain after a crash or an abort. Check for it and stop it only if
it belongs to this service.

```powershell
logman query -ets
logman stop EtwDnstap -ets
```

Use the `--session-name` you configured if it was not the default.

Earlier builds named sessions `etw-dnstap-<pid>`. Those can remain after a
crash. Check the process ID with `logman query -ets`. Then stop only that
session with `logman stop etw-dnstap-<pid> -ets`.

## 6. Unregister the performance counters

```powershell
unlodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man"
```

This removes the counter set registration. Run it after step 1 and before
step 8. The command reads the manifest, and step 8 removes it.

## 7. Delete the spool folder

The spool holds frames that the service did not send. These frames include
client addresses and query names, and they are not encrypted. Step 1 has
stopped the service, so nothing writes to the folder now. Delete the folder when
you no longer need the frames. Frames in the spool never reached the collector.

If the service ran with `--spool-dir`, set `$spoolDirectory` to that path. The
binary path that `sc.exe qc EtwDnstap` showed before step 2 has it. The command
below deletes the files in the folder, and then the empty folder. It stops if it
finds a folder, a link, or a reparse point inside.

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

The listing shows the segment files and `cursor.bin`. The step removes only the
spool folder. The `EtwDnstap` folder above it stays, with any snapshots under
`backup`.

## 8. Remove the executable and its directory

Check that the directory holds only the executable and its manifest before you
remove it:

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
Get-ChildItem -Force -Path $installDirectory
Remove-Item -Recurse -Force -Path $installDirectory
```

Removing the directory removes its ACLs with it, including the service SID's
read and execute grant. You do not need `icacls` for removal. If you keep the
directory for evidence, leave the ACLs in place, so it stays protected from
ordinary users.

## 9. Check the result

```powershell
sc.exe query EtwDnstap
net localgroup "Performance Log Users"
logman query -ets
typeperf -q EtwDnstap
Test-Path -LiteralPath "$env:ProgramData\EtwDnstap\spool"
```

`sc.exe query` should report that the service does not exist (error 1060). The
group list should not show `NT SERVICE\EtwDnstap`. The session list should not
show `EtwDnstap`. The `typeperf -q EtwDnstap` list should not show the counters.
`Test-Path` should return `False` for the spool folder. If the service used
`--spool-dir`, check that path instead.

## What is not removed

- Dnstap records already sent to the collector. The collector's retention applies.
- Entries in the Application event log.
- The folders under `$env:ProgramData\EtwDnstap\backup`. They hold snapshots of
  the service configuration, the ETW descriptors, and a copy of the executable.
  Only SYSTEM and Administrators can read them. Remove them when you no longer
  need to roll back.
- The `EtwDnstap` folder under `$env:ProgramData`, which can hold snapshots in
  `backup`. Step 7 removes only the `spool` folder.
- The provider permission, if you granted one, until you reverse it in step 3.
- The build output under `target\` in your repository clone. Remove it with the
  repository if you no longer need it.
- The Rust toolchain installed by rustup.
