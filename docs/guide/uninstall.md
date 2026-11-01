# Uninstall

> **Status:** Current · **Describes:** `main` at commit `dde7188` · **Reviewed:** 2026-11-01

Removing the service does not change the DNS Server role, its configuration, or
its Analytical event log. The service never enabled those.

Run the commands in an elevated Windows PowerShell 5.1 session. The commands
were written from the provisioning steps in [install.md](install.md) and were
**not executed** in this documentation pass. Test them on a disposable host
first.

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
deletes any descriptor other than the default that then matches the default.
That also covers the grants of earlier versions of the script. Restart the
computer afterwards: ETW applies these descriptors at boot. Run `net localgroup
"Performance Log Users"` to confirm that the entry is gone.

Before its first change, the script exports the `WMI\Security` key and saves
the original bytes of each value it changes. The folder is
`$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>`, and the script prints
its path. To undo the removal, run the script with `-Restore <folder>` and
restart the computer. With `-WhatIf`, the script writes no backup.

The script derives the service SID from the service name, so `-Remove` works
after `sc.exe delete` as well. It strips only the ACEs for that SID. It prints
one line for each descriptor that it changes or deletes, and the SDDL of the
default descriptor. It edits the descriptors under
`HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security`. The lab tested removal on
build 26100. See
[scripts/provision/README.md](../../scripts/provision/README.md).

## 4. Remove the Event Log source

```powershell
Remove-EventLog -Source EtwDnstap
```

This removes the `EtwDnstap` source registration. It does not delete existing
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

Earlier builds named sessions `etw-dnstap-<pid>`. Those can remain after a crash. Confirm the process ID with `logman query -ets`,
then stop only that session with `logman stop etw-dnstap-<pid> -ets`.

## 6. Remove the executable and its directory

Confirm the directory holds only the executable before you delete it:

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
Get-ChildItem -Force -Path $installDirectory
Remove-Item -Recurse -Force -Path $installDirectory
```

Deleting the directory removes its ACLs with it, including the service SID's
read and execute grant. You do not need `icacls` for removal. If you keep the
directory for evidence, leave the ACLs in place, so it stays protected from
ordinary users.

## 7. Check the result

```powershell
sc.exe query EtwDnstap
net localgroup "Performance Log Users"
logman query -ets
```

`sc.exe query` should report that the service does not exist (error 1060). The
group list should not show `NT SERVICE\EtwDnstap`. The session list should not
show `EtwDnstap`.

## What is not removed

- Dnstap records already sent to the collector. The collector's retention applies.
- Entries in the Application event log.
- The folders under `$env:ProgramData\EtwDnstap\backup`. They hold snapshots of
  the service configuration, the ETW descriptors, and a copy of the executable.
  Only SYSTEM and Administrators can read them. Delete them when you no longer
  need to roll back.
- The provider permission, if you granted one, until you reverse it in step 3.
- The build output under `target\` in your repository clone. Remove it with the
  repository if you no longer need it.
- The Rust toolchain installed by rustup.
