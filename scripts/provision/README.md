# Provisioning scripts

> **Status:** Not validated on a live DNS Server. Run on a disposable host first.

## Grant-EtwDnstapEtwAccess.ps1

EtwDnstap runs as `NT AUTHORITY\LocalService` with a restricted service SID
(`sc.exe sidtype EtwDnstap restricted`), so its token is write-restricted. For
write-type access checks the kernel makes a second pass against the token's
restricting SIDs: the service SID, Everyone, the logon SID, and `S-1-5-33`.
A grant to Performance Log Users alone may therefore not satisfy ETW's
write-type rights. The script grants the rights directly to the service SID
`NT SERVICE\EtwDnstap` with `advapi32!EventAccessControl`.

| GUID | Rights | Why |
| --- | --- | --- |
| `{eb79061a-a566-4698-9119-3ed2807060e7}` (Microsoft-Windows-DNSServer) | `TRACELOG_GUID_ENABLE` 0x80 | `EnableTraceEx2` in the service's own session |
| `{0811c1af-7a07-4a06-82ed-869455cdf713}` (default WMI security) | `WMIGUID_QUERY` 0x1, `WMIGUID_SET` 0x2, `TRACELOG_CREATE_REALTIME` 0x20, `TRACELOG_ACCESS_REALTIME` 0x400 (mask 0x423) | `StartTrace`, query, stop and consume a real-time session |

The masks are assumptions. `-SessionRights` overrides the second mask.

### Use

Run from an elevated Windows PowerShell 5.1 session:

```powershell
.\Grant-EtwDnstapEtwAccess.ps1 -WhatIf    # preview
.\Grant-EtwDnstapEtwAccess.ps1            # grant, then print the SDDL
.\Grant-EtwDnstapEtwAccess.ps1 -Remove    # uninstall: strip the service SID's ACEs
```

The script is idempotent. It does not change group membership. Add the service
SID to Performance Log Users separately:

```powershell
net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add
```

The service SID is derived from the service name (SHA-1 of the upper-cased name),
so `-Remove` works after `sc.exe delete`. The script cross-checks the derived SID
against the system when the service still exists.

`-Remove` does not call `EventAccessRemove`, which would delete a whole
descriptor, including system defaults. It reads the descriptor with
`EventAccessQuery`, removes the ACEs for the service SID, and writes the result to
`HKLM\SYSTEM\CurrentControlSet\Control\WMI\Security\<guid>`. That path and its
effect on a running system are unverified.

The full procedure is in [install.md](../../docs/guide/install.md#4-set-the-service-identity-privileges-etw-access-and-event-source)
and [uninstall.md](../../docs/guide/uninstall.md).
