# Provisioning scripts

> **Status:** Validated on Windows Server 2025, build 26100, on 2026-11-01. See [the validation record](../../docs/history/localservice-restricted-validation.md).

## Grant-EtwDnstapEtwAccess.ps1

EtwDnstap only reads ETW events; it never logs events into ETW. It does create
and control its own real-time trace session. It runs as
`NT AUTHORITY\LocalService` with a restricted service SID
(`sc.exe sidtype EtwDnstap restricted`). Windows calls such a token
*write-restricted*. Any access that Windows classes as a write (a change of
state, not event data) must also pass a second check, and that check considers
only the restricting SIDs: the service SID, Everyone, the logon SID, and
`S-1-5-33`. LocalService and Performance Log Users do not count for it.

Creating the session is such a change. ETW checks it against the descriptor of
the session's GUID, and falls back to the shared default descriptor
`{0811c1af-7a07-4a06-82ed-869455cdf713}` when the GUID has none. EtwDnstap starts
its session with a stable GUID derived from the session name, so the script
grants the one right on that GUID only, to the service SID
`NT SERVICE\EtwDnstap`, with `advapi32!EventAccessControl`:

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

Nothing else is granted. In the lab, these calls passed with the default
descriptor and no grant: `EnableTraceEx2` on the DNS provider, `OpenTrace` and
`ProcessTrace`, and the `ControlTrace` query and stop, including the reclaim of
a stale session.

**Restart the computer after running the script**, and after `-Remove`. ETW
applies these descriptors at boot. In the lab, a grant made without a restart
had no effect.

### Use

Run from an elevated Windows PowerShell 5.1 session:

```powershell
.\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap -WhatIf    # preview
.\Grant-EtwDnstapEtwAccess.ps1 -SessionName EtwDnstap            # grant; then restart
.\Grant-EtwDnstapEtwAccess.ps1 -Remove                           # uninstall; then restart
```

Pass every `--session-name` used on the host to `-SessionName`. A grant on a
session GUID that is not listed is removed.

Every run also strips ACEs for the service SID from every other descriptor
under `HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security`. That removes grants
made by earlier versions of the script, on the default GUID and on the DNS
provider GUID, and copies of them that other components made. A non-default
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

**Upgrading from the first version** (commit `6718863`), which granted `0x423`
on the default GUID and `0x80` on the DNS provider GUID: run the current script
with `-SessionName`, then restart. It removes both old grants and deletes the
provider descriptor the first version created. The lab tested this path.

The full procedure is in [install.md](../../docs/guide/install.md#4-set-the-service-identity-privileges-etw-access-and-event-source)
and [uninstall.md](../../docs/guide/uninstall.md).
