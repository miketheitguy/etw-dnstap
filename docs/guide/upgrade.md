# Repair or upgrade the service

> **Status:** Current · **Describes:** `tcp-sink-spool` at commit `0569136` · **Reviewed:** 2026-11-03

Use this guide to change an installed service. It covers these tasks: take a
snapshot, restore it, move from an older build, and replace the executable or
the counter manifest, and add the spool folder. To install a new service, see
[Install and provision](install.md).


For an existing installation, keep the service identity, privileges, executable
ACLs, and recovery policy as they are. Change only what you need. Take the
snapshot first. Do not reset a value to a guessed default. Restore the value
that the snapshot holds.

## Snapshot before a change

First run the functions from
[Create a protected snapshot folder](install.md#create-a-protected-snapshot-folder).

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

The folder holds these items:

- The account, the SID type, and the privileges.
- The recovery policy and the service descriptor.
- The directory ACLs and the group membership.
- A copy of the executable.

Only SYSTEM and Administrators can read it. The `.reg`
files are for reference. Do not restore the service from them (see below).
When you run the grant script in a repair, add `-BackupPath "$backup\etw"` so
that its backup is in the same folder.

## Restore from the snapshot

First check the snapshot folder again. Someone may have changed it since you
made it. Run the functions from
[Create a protected snapshot folder](install.md#create-a-protected-snapshot-folder), set
`$backup` to the snapshot folder, and run this check. Stop if it throws, and do
not use any file from the folder.

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

Do not use `reg import` for the service key. The SCM keeps the service
configuration in memory, and a registry import can leave the SCM and the
registry in disagreement. Use `sc.exe` instead. Read the values from the
`.txt` files in the snapshot folder. Replace each `<...>` with the value in the
file.

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

Then restore the other parts of the snapshot:

1. Run `net localgroup "Performance Log Users"` and compare it with
   `plu.txt`. Add or remove `NT SERVICE\EtwDnstap` so that they match.
2. If a repair ran the grant script, run it with `-Restore "$backup\etw"`.
   The script checks the folders and `changes.json`. It checks every entry
   before it applies any, and it skips a value that changed after the run that
   saved it. Use `-Force` only after you look at the skipped values.
3. If you replaced the executable, put the saved copy back (see below).
4. Run `Restart-Computer` if the grant script restored any descriptor. ETW
   applies descriptors at boot.
5. Run the checks in [step 6](install.md#6-inspect-the-registered-identity-and-access-controls)
   and compare the result with the saved `.txt` files.

`icacls /restore` runs on the parent folder, because `/save` stores paths
relative to the parent.

## Add the spool folder

This build adds the disk spool. The spool is on by default, so the service
looks for the spool folder at startup. The default folder is
`C:\ProgramData\EtwDnstap\spool`. Do these steps in the same change as the
executable and the manifest:

1. Take the snapshot, as in [Snapshot before a change](#snapshot-before-a-change).
2. Create the folder with
   [Create the spool folder](install.md#create-the-spool-folder). Run the steps
   in the same PowerShell session as the snapshot functions. The steps need the
   service SID from step 3, so the service must already be registered.
   `New-ProtectedFolder` stops if the folder exists. If it does, inspect it. Do
   not take ownership of it, and do not reuse it without that check.
3. Replace the executable and the counter manifest, as in
   [Replace the executable](#replace-the-executable) and
   [Replace the counter manifest](#replace-the-counter-manifest). The manifest
   change is required. Without the new registration, counters 52 to 60 do not
   appear in `typeperf -q EtwDnstap`.

Until the folder exists and passes the checks, the service logs one warning
and runs without a spool. The warning starts with `spool folder is not usable;
capture continues without a spool`. Its `reason` names the failed check. See
[Spool warnings](troubleshooting.md#spool-warnings).

Do not delete the spool folder during a repair. It can hold frames that were
not sent. The snapshot does not include it, and the restore steps do not change
it. To run without a spool, add `--no-spool` to the binary path. Change the path
as in [Changing the binary path](#repair-cases).

After the service starts, `spool_available` in the `output stats` line reads
`true` while the spool is in use.

## Repair cases

- **Upgrading from LocalSystem or the virtual account.** A service still
  registered as LocalSystem or `NT SERVICE\EtwDnstap` refuses to run with this
  build. Do all of these, in this order:
  1. Take the snapshot.
  2. Set the identity, the SID type, and the privileges with the `sc.exe`
     lines from
     [step 4](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source).
  3. Add `NT SERVICE\EtwDnstap` to Performance Log Users with `net localgroup`.
     If it is already a member, the command reports an error. That is fine.
  4. Run `Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` for every session
     name, with `-WhatIf` first. Then run `Restart-Computer`. Without the
     grant and the restart, the start fails with `StartTraceW: access denied`.
  5. Keep the service SID's read and execute grant from
     [step 3](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute).
     Keep the recovery policy from [step 5](install.md#5-configure-scm-recovery).
  6. Start the service. Check the account with `sc.exe qc EtwDnstap`.

  In the lab on build 26100, `obj=` did not clear the SID type or the privilege
  list. This held for a change to the same account, and for a change to
  LocalSystem and back. After any `obj=` change, still run
  `sc.exe sidtype EtwDnstap restricted` and check it with
  `sc.exe qsidtype EtwDnstap`. The check costs little.
- **Upgrading from the first version of the grant script** (commit `1a54272`).
  That version granted `0x423` on the default GUID and `0x80` on the DNS
  provider GUID, and created a descriptor of its own for the provider. Replace
  the executable, run the current script with `-SessionName`, then restart the
  computer. The script removes the old grants and the provider
  descriptor once it matches the default again. It then grants on the session GUID.
  The lab tested this path.
- **Changing `--session-name`.** The session GUID follows the name. Run the
  grant script with the new name (and any others still in use), then restart
  the computer before starting the service.
- **Upgrading from a build with `--output`, `--max-output-bytes`, or `--metrics`.**
  Remove those arguments from the binary path before starting the new
  executable. They now fail command-line parsing. Add `--receiver` with the
  collector's numeric IP and port.
- **Changing the binary path.** Use `sc.exe config EtwDnstap binPath= "..."`,
  keeping the space after `binPath=`.

## Replace the executable

Take the snapshot first. Run the commands from the folder that holds the new
`etw-dnstap.exe` under `target\x86_64-pc-windows-msvc\release\`, or change the
`Copy-Item` source to the folder where you put it. If the service is already
stopped, `sc.exe stop` prints `FAILED 1062`. That is expected. Then:

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

- The first hash is the old executable. Compare it with `exe-hash.txt`. The
  second hash is the new executable. Compare it with the hash of the file you
  built.
- `icacls` must show `NT SERVICE\EtwDnstap:(I)(RX)` for the new file. If it does
  not, the service cannot start. Do not change the other ACLs. Restore only the
  service SID's read and execute grant from step 3.
- The service must reach `RUNNING`, and a `capture started` line must appear.
- If the new release also changes `etw-dnstap.man`, follow
  [Replace the counter manifest](#replace-the-counter-manifest) between the stop
  and the start.

To roll back, stop the service, remove the new file, rename the backup back,
and start the service:

```powershell
sc.exe stop EtwDnstap
Remove-Item $exe
Rename-Item "$exe.bak" 'etw-dnstap.exe'
sc.exe start EtwDnstap
```

When the new executable works, remove `etw-dnstap.exe.bak`. The copy in the
snapshot folder stays until you remove the folder.

## Replace the counter manifest

Do this only when a new release changes `etw-dnstap.man`. The release that
adds the spool changes it, so [Add the spool folder](#add-the-spool-folder)
calls this section. Take the snapshot
first, as for the executable. Run these commands while the service is stopped,
between `sc.exe stop` and `sc.exe start`. Run them from the folder that holds
the new `etw-dnstap.man` under `packaging\perfcounters\`, or change the
`Copy-Item` source to the folder where you put it.

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
unlodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man"
Copy-Item '.\packaging\perfcounters\etw-dnstap.man' $installDirectory
lodctr /m:"C:\Program Files\EtwDnstap\etw-dnstap.man" "C:\Program Files\EtwDnstap"
```

- Run `unlodctr` before you replace the file. It removes the registration of
  the old manifest.
- Run `lodctr` after the copy. It registers the new manifest.
- Start the service, and check the counters as in
  [step 7](install.md#7-start-capture-and-check-that-it-is-running).
- The snapshot folder does not hold the manifest. Keep a copy of the old
  `etw-dnstap.man` if you may need to roll back. To roll back, repeat these
  commands with the old file.


## Next steps

- [Troubleshooting](troubleshooting.md)
- [Uninstall](uninstall.md)
