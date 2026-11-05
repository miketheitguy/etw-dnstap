# Provisioning scripts

> **Status:** Current · **Describes:** the grant script on branch `spool-encryption` · **Evidence dated:** 2026-11-01 (validated on Windows Server 2025, build 26100), and 2026-11-03 (run by the MSI) · **Reviewed:** 2026-11-05
>
> See [the validation record](../../docs/history/localservice-restricted-validation.md).

## Grant-EtwDnstapEtwAccess.ps1

EtwDnstap reads ETW events and controls its own real-time trace session. It runs as `NT AUTHORITY\LocalService` with a restricted service SID (`sc.exe sidtype EtwDnstap restricted`). Windows calls such a token *write-restricted*: each state-changing access must also pass a second test against the restricting SIDs only (the service SID, Everyone, the logon SID, and `S-1-5-33`).

Creating the session is such a change. ETW tests it against the descriptor of the session's GUID. When the GUID has no descriptor, ETW uses the shared default descriptor `{0811c1af-7a07-4a06-82ed-869455cdf713}`. EtwDnstap starts its session with a stable GUID derived from the session name. The script grants one right on that GUID, to the service SID `NT SERVICE\EtwDnstap`, with `advapi32!EventAccessControl`:

| GUID | Right | Why |
| --- | --- | --- |
| The session GUID for each `-SessionName` (default `EtwDnstap`: `{05e1db3c-1322-58c7-8235-0463fccba32b}`) | `TRACELOG_CREATE_REALTIME` 0x20 | `StartTrace` for the service's own real-time session |

The session GUID is SHA-1 over the UTF-16LE text `EtwDnstap.Session:<name>` (case as given), with the RFC 9562 version-5 and variant bits set, read as .NET `new Guid(byte[16])`. The service computes it in `src/etw/session_guid.rs` and logs it as `session_guid` in `capture started`.

A grant on the default descriptor would let the service SID create any session without its own descriptor. Windows scopes its own restricted services the same way: on Windows Server 2025, `NT SERVICE\DPS`, `WdiServiceHost`, `EventLog`, `BFE` and `mpssvc` hold rights on specific GUIDs.

The session GUID's new descriptor is seeded from the default descriptor, so Administrators, LocalService, NetworkService, and Performance Log Users also hold the create right on it. They already hold it on every GUID without a descriptor.

In the lab, these calls passed with the default descriptor and no grant:

- `EnableTraceEx2` on the DNS provider.
- `OpenTrace` and `ProcessTrace`.
- The `ControlTrace` query and stop, including the reclaim of a stale session.

**Restart the computer after running the script**, and after `-Remove`. ETW applies these descriptors at boot. 

### Use

Run from an elevated Windows PowerShell 5.1 session:

```powershell
.\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap -WhatIf    # preview
.\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap            # grant; then restart
.\Grant-EtwDnstapEtwAccess.ps1 -Remove                           # uninstall; then restart
.\Grant-EtwDnstapEtwAccess.ps1 -Restore <backup folder>         # undo a run; then restart
```

Pass every `--session-name` used on the host to `-SessionName`. The script removes grants on unlisted session GUIDs. Names follow the service rule `^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}\z`, checked before any change. Match `--session-name` exactly, including case: a grant for `etwdnstap` does not cover `EtwDnstap`, and the service fails to start with access denied.

`ConfirmImpact` is `High`, so the script asks before each change unless you add `-Confirm:$false`. The MSI runs it as SYSTEM with `-Confirm:$false`. See [packaging/msi/README.md](../../packaging/msi/README.md).

Every run strips ACEs for the service SID from every other descriptor under `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security`. This removes grants on the default GUID and on the DNS provider GUID, including copies other components made. A non-default descriptor that equals the default once those ACEs are gone is deleted, so ETW falls back to the default. `-Remove` does the same for every descriptor, including the session GUIDs.

The script is idempotent and leaves group membership unchanged. Add the service SID to Performance Log Users separately:

```powershell
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add
```

The service SID is derived from the service name (SHA-1 of the upper-cased name), so `-Remove` works after `sc.exe delete`.

The script edits each descriptor's registry value directly. It avoids `EventAccessRemove` (removes a whole descriptor) and `EventAccessQuery` (returns the default for a GUID with none).

### Backup and restore

Before its first change, the script makes a backup of the values under `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security` in `-BackupPath`. The default is `$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>`. A custom path must lie below `$env:ProgramData\EtwDnstap\backup`. The script prints the path.

The script creates `EtwDnstap`, `backup` and the run's folder with owner Administrators, no inherited ACEs, and full control for SYSTEM and Administrators only. It stops before any change if one of those levels exists and is a reparse point, has another owner, or lets another principal write to it. Inspect such a folder and remove it only when you know its origin. The script never reuses a backup folder.

| File | Content |
| --- | --- |
| `wmi-security.reg` | `reg.exe export` of the whole `WMI\Security` key, made before any change. For reference and manual recovery. |
| `changes.json` | The original bytes (Base64) of each value the run changed or removed, the names of the values it created, and the SHA-256 of the bytes it left in each value. |

`-WhatIf` makes no backup. A run with nothing to change makes none. If the backup fails, the script stops before any change.

To undo a run, preview and then run:

```powershell
.\Grant-EtwDnstapEtwAccess.ps1 -Restore "$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>" -WhatIf
.\Grant-EtwDnstapEtwAccess.ps1 -Restore "$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>"
```

`-Restore` applies nothing until every entry passes these tests:

1. The owner, the reparse status, and the ACL of `changes.json` and of every folder above it are trusted.
2. Each name is a GUID.
3. Each saved value parses as a security descriptor.
4. The file came from this computer.

It writes the original bytes back, removes the values the run created, and prints the values restored, deleted, and skipped. `-Restore` skips a value changed after the run with a warning. `-Restore -Force` overwrites or removes it. Restart the computer afterward. Group membership stays unchanged.

The script skips a descriptor that it cannot parse and warns: `Descriptor {<guid>} is unparseable; left unchanged`. If the descriptor of the session GUID exists and cannot be parsed, the script stops with an error and does not grant on it.

The full procedure is in [install.md](../../docs/guide/install.md#4-set-the-service-identity-privileges-etw-access-and-event-source) and [uninstall.md](../../docs/guide/uninstall.md).
