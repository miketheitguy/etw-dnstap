# Uninstall

> **Status:** Current · **Describes:** `main` at commit `9172f2d` · **Reviewed:** 2026-11-01

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
path, the SID type, the privilege list, and the recovery policy. If the service is
still running, SCM marks it for deletion and removes it when it stops.

## 3. Remove the Event Log source

```powershell
Remove-EventLog -Source EtwDnstap
```

This removes the `EtwDnstap` source registration. It does not delete existing
entries in the Application log. Those remain until you clear the log through your
normal retention process.

## 4. Stop a leftover ETW session

A session can remain after a crash or an abort. Check for it and stop it only if
it belongs to this service.

```powershell
logman query -ets
logman stop EtwDnstap -ets
```

Use the `--session-name` you configured if it was not the default. Stop a
leftover `etw-dnstap-<pid>` session the same way, after confirming its PID.

## 5. Remove the executable and its directory

Confirm the directory holds only the executable before you delete it:

```powershell
$installDirectory = 'C:\Program Files\EtwDnstap'
Get-ChildItem -Force -Path $installDirectory
Remove-Item -Recurse -Force -Path $installDirectory
```

Deleting the directory removes its ACLs with it. You do not need `icacls` for
removal. If you keep the directory for evidence, leave the ACLs in place, so it
stays protected from ordinary users.

## 6. Check the result

```powershell
sc.exe query EtwDnstap
logman query -ets
```

`sc.exe query` should report that the service does not exist (error 1060). The
session list should not show `EtwDnstap`.

## What is not removed

- Dnstap records already sent to the collector. The collector's retention applies.
- Entries in the Application event log.
- The build output under `target\` in your repository clone. Remove it with the
  repository if you no longer need it.
- The Rust toolchain installed by rustup.
