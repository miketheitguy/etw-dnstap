# Provisioning scripts

> **Status:** Current · **Describes:** the grant script on branch `spool-encryption` · **Evidence dated:** 2026-11-01 (validated on Windows Server 2025, build 26100), and 2026-11-03 (run by the MSI) · **Reviewed:** 2026-11-05
>
> See [the validation record](../../docs/history/localservice-restricted-validation.md).

### Quick Start

`Grant-EtwDnstapEtwAccess.ps1`, from elevated Windows PowerShell 5.1. Restart after each run.

```powershell
.\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap -WhatIf    # preview
.\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap            # grant; then restart
.\Grant-EtwDnstapEtwAccess.ps1 -Remove                           # uninstall; then restart
.\Grant-EtwDnstapEtwAccess.ps1 -Restore <backup folder>         # undo a run; then restart
```

Procedure: [install.md](../../docs/guide/install.md#4-set-the-service-identity-privileges-etw-access-and-event-source), [uninstall.md](../../docs/guide/uninstall.md).

### Steps / Configuration

EtwDnstap runs as `NT AUTHORITY\LocalService` with a restricted service SID (`sc.exe sidtype EtwDnstap restricted`). Such a token is *write-restricted*: state-changing access must also pass a second test against the restricting SIDs only (service SID, Everyone, logon SID, `S-1-5-33`). Creating the session is such a change. ETW tests the session GUID's descriptor, or the default descriptor `{0811c1af-7a07-4a06-82ed-869455cdf713}` when none exists.

The script grants one right to `NT SERVICE\EtwDnstap` with `advapi32!EventAccessControl`:

| GUID | Right | Why |
| --- | --- | --- |
| Session GUID per `-SessionName` (default `EtwDnstap`: `{05e1db3c-1322-58c7-8235-0463fccba32b}`) | `TRACELOG_CREATE_REALTIME` 0x20 | `StartTrace` for the service's session |

The GUID is SHA-1 over UTF-16LE `EtwDnstap.Session:<name>` (case as given), RFC 9562 version-5 and variant bits set, read as .NET `new Guid(byte[16])`. The service computes it in `src/etw/session_guid.rs` and logs `session_guid` in `capture started`.

A grant on the default descriptor would let the service SID create any session. Windows scopes `NT SERVICE\DPS`, `WdiServiceHost`, `EventLog`, `BFE`, and `mpssvc` per GUID the same way (Windows Server 2025). The new descriptor is seeded from the default, so Administrators, LocalService, NetworkService, and Performance Log Users also get the create right, as they already have on every GUID without a descriptor.

Passed in the lab with the default descriptor and no grant: `EnableTraceEx2` on the DNS provider, `OpenTrace`, `ProcessTrace`, `ControlTrace` query and stop (stale-session reclaim included).

#### Use

- Pass every `--session-name` to `-SessionName`; grants on unlisted session GUIDs are removed. Pattern `^[A-Za-z0-9_.][A-Za-z0-9_.-]{0,127}\z`, checked before any change.
- `ConfirmImpact` is `High`: it prompts unless `-Confirm:$false`. The MSI runs it as SYSTEM with `-Confirm:$false` ([packaging/msi/README.md](../../packaging/msi/README.md)).
- Every run strips the service SID's ACEs from every other descriptor under `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security` (default GUID, DNS provider GUID, other components' copies). A non-default descriptor equal to the default afterward is deleted. `-Remove` does this for every descriptor, session GUIDs included.
- Idempotent. Group membership is untouched, so add it yourself:

```powershell
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add
```

- The service SID derives from the upper-cased service name (SHA-1), so `-Remove` works after `sc.exe delete`.
- The script edits registry values directly, avoiding `EventAccessRemove` (removes a whole descriptor) and `EventAccessQuery` (returns the default for a GUID with none).

#### Backup and restore

Before its first change, the script backs up the `WMI\Security` values into `-BackupPath` and prints it. Default `$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>`; a custom path must lie below `$env:ProgramData\EtwDnstap\backup`. It creates `EtwDnstap`, `backup`, and the run folder with owner Administrators, no inherited ACEs, full control for SYSTEM and Administrators only. It never reuses a folder.

| File | Content |
| --- | --- |
| `wmi-security.reg` | `reg.exe export` of `WMI\Security`, before any change |
| `changes.json` | Original bytes (Base64) of changed or removed values, names of created values, SHA-256 of bytes left |

`-WhatIf` and a no-change run make no backup. A failed backup stops the script before any change.

```powershell
.\Grant-EtwDnstapEtwAccess.ps1 -Restore "$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>" -WhatIf
.\Grant-EtwDnstapEtwAccess.ps1 -Restore "$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>"
```

`-Restore` applies nothing until every entry passes:

1. Owner, reparse status, and ACL of `changes.json` and every folder above it are trusted.
2. Each name is a GUID.
3. Each value parses as a security descriptor.
4. The file came from this computer.

It writes original bytes back, removes created values, and prints restored, deleted, and skipped values. A value changed after the run is skipped with a warning; `-Restore -Force` overwrites or removes it.

### Common Gotchas

- Restart after the script and after `-Remove`: ETW applies descriptors at boot.
- Match `--session-name` exactly, case included. A grant for `etwdnstap` doesn't cover `EtwDnstap`, and the service fails to start with access denied.
- A backup level that's a reparse point, has another owner, or is writable by another principal stops the script before any change. Inspect it and remove it only if you know its origin.
- Unparseable descriptor: skipped with `Descriptor {<guid>} is unparseable; left unchanged`. If the session GUID's own descriptor can't be parsed, the script stops with an error and doesn't grant on it.
