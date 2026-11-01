# Provisioning scripts

> **Status:** Current · **Describes:** `main` at commit `dde7188` · **Evidence dated:** 2026-11-01 (validated on Windows Server 2025, build 26100) · **Reviewed:** 2026-11-01
>
> See [the validation record](../../docs/history/localservice-restricted-validation.md).

## Grant-EtwDnstapEtwAccess.ps1

EtwDnstap only reads ETW events. It never logs events into ETW. It does create
and control its own real-time trace session. It runs as
`NT AUTHORITY\LocalService` with a restricted service SID
(`sc.exe sidtype EtwDnstap restricted`). Windows calls such a token
*write-restricted*. Windows classes some accesses as a write. A write here is
a change of state, not event data. Each such access must also pass a second
check. That check considers only the restricting SIDs: the service SID,
Everyone, the logon SID, and `S-1-5-33`. LocalService and Performance Log Users
do not count for it.

Creating the session is such a change. ETW checks it against the descriptor of
the session's GUID. When the GUID has no descriptor, ETW uses the shared default
descriptor `{0811c1af-7a07-4a06-82ed-869455cdf713}`. EtwDnstap starts its
session with a stable GUID derived from the session name. The script therefore
grants the one right on that GUID only. It grants the right to the service SID
`NT SERVICE\EtwDnstap` with `advapi32!EventAccessControl`:

| GUID | Right | Why |
| --- | --- | --- |
| The session GUID for each `-SessionName` (default `EtwDnstap`: `{05e1db3c-1322-58c7-8235-0463fccba32b}`) | `TRACELOG_CREATE_REALTIME` 0x20 | `StartTrace` for the service's own real-time session |

The session GUID is SHA-1 over the UTF-16LE text `EtwDnstap.Session:<name>`,
with the RFC 9562 version-5 and variant bits set, read as .NET
`new Guid(byte[16])`. The service computes the same value
(`src/etw/session_guid.rs`) and logs it as `session_guid` in `capture started`.
The name is hashed exactly as given, with no case folding.

This is how Windows provisions its own restricted LocalService services. On
Windows Server 2025, `NT SERVICE\DPS`, `WdiServiceHost`, `EventLog`, `BFE` and
`mpssvc` hold ETW rights on specific GUIDs, and none on the default. A grant on
the default would let the service SID create any session that has no
descriptor of its own. In the lab it also spread: other components create
descriptors by copying the default, and those copies kept the grant.

The new descriptor of the session GUID is seeded from the default descriptor.
The default descriptor lets Administrators, LocalService, NetworkService, and
Performance Log Users create sessions. These principals can therefore also
create a session with that GUID. This is harmless. They already hold the same
right on every GUID that has no descriptor of its own. The grant adds no right
to any of them.

Nothing else is granted. In the lab, these calls passed with the default
descriptor and no grant:

- `EnableTraceEx2` on the DNS provider.
- `OpenTrace` and `ProcessTrace`.
- The `ControlTrace` query and stop, including the reclaim of a stale session.

**Restart the computer after running the script**, and after `-Remove`. ETW
applies these descriptors at boot. In the lab, a grant made without a restart
had no effect.

### Use

Run from an elevated Windows PowerShell 5.1 session:

```powershell
.\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap -WhatIf    # preview
.\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap            # grant; then restart
.\Grant-EtwDnstapEtwAccess.ps1 -Remove                           # uninstall; then restart
.\Grant-EtwDnstapEtwAccess.ps1 -Restore <backup folder>         # undo a run; then restart
```

Pass every `--session-name` used on the host to `-SessionName`. A grant on a
session GUID that is not listed is removed.

Every run also strips ACEs for the service SID from every other descriptor
under `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security`. This removes two
kinds of grant:

- Grants that earlier versions of the script made on the default GUID and on
  the DNS provider GUID.
- Copies of those grants that other components made.

A non-default
descriptor that equals the default once those ACEs are gone is deleted, so ETW
falls back to the default exactly as before. Nothing else is changed. `-Remove`
does the same for every descriptor, the session GUIDs included.

The script is idempotent. It does not change group membership. Add the service
SID to Performance Log Users separately:

```powershell
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add
```

The service SID is derived from the service name (SHA-1 of the upper-cased name),
so `-Remove` works after `sc.exe delete`. The script cross-checks the derived SID
against the system when the service still exists.

The script never calls `EventAccessRemove`, which would delete a whole
descriptor, including system defaults. It reads each descriptor from the
registry value, removes the ACEs for the service SID, and writes the result
back. It does not use `EventAccessQuery` for this, because that call returns
the default descriptor for a GUID that has none of its own.

### Backup and restore

The script edits values under `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security`.
Before its first change, it makes a backup in `-BackupPath`. The default is
`$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>`, and a custom path must lie
below `$env:ProgramData\EtwDnstap\backup`. The script prints the path.

A standard user can create folders in ProgramData, so the script does not trust
what it finds there. It creates `EtwDnstap`, `backup` and the run's folder with
owner Administrators, no inherited ACEs, and full control for SYSTEM and
Administrators only. It stops with an error before any change if one of those
levels exists and has one of these problems:

- It is a reparse point.
- It has another owner.
- It lets another principal write to it.

It does not take ownership of a folder that someone else
made. Inspect such a folder, and remove it only when you know where it came
from. The script never reuses an existing backup folder.

| File | Content |
| --- | --- |
| `wmi-security.reg` | `reg.exe export` of the whole `WMI\Security` key, made before any change. For reference and manual recovery. |
| `changes.json` | The original bytes (Base64) of each value that the run changed or deleted. The names of the values that the run created. For each value, the SHA-256 of the bytes that the run left in it. |

`-WhatIf` changes nothing, and that includes the backup. A run that has nothing
to change makes no backup. If the script cannot make the backup, it stops
before it changes anything.

To undo a run, preview and then run:

```powershell
.\Grant-EtwDnstapEtwAccess.ps1 -Restore "$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>" -WhatIf
.\Grant-EtwDnstapEtwAccess.ps1 -Restore "$env:ProgramData\EtwDnstap\backup\<yyyyMMddHHmmssfff>"
```

`-Restore` does these checks before it applies anything:

1. It checks the owner, the reparse status, and the ACL of `changes.json` and
   of every folder above it.
2. It checks that each name is a GUID.
3. It checks that each saved value parses as a security descriptor.
4. It checks that the file came from this computer.

It applies nothing until every entry passes. It writes
the original bytes back, deletes the values that the run created, and prints a
summary of the values restored, deleted and skipped. A value that holds other
bytes than the run wrote was changed after the run. `-Restore` skips it with a
warning. `-Restore -Force` overwrites or deletes it. `-Restore` reminds you to
restart the computer, because ETW applies the descriptors at boot. It does not
change group membership.

The script skips a descriptor that it cannot parse and writes a warning:
`Descriptor {<guid>} is unparseable; left unchanged`. If the descriptor of the
session GUID itself exists and cannot be parsed, the script stops with an error
and does not grant on it.

**Upgrading from the first version** (commit `6718863`), which granted `0x423`
on the default GUID and `0x80` on the DNS provider GUID: run the current script
with `-SessionName`, then restart. It removes both old grants and deletes the
provider descriptor the first version created. The lab tested this path.

The full procedure is in [install.md](../../docs/guide/install.md#4-set-the-service-identity-privileges-etw-access-and-event-source)
and [uninstall.md](../../docs/guide/uninstall.md).
