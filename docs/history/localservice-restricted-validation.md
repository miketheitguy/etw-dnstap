# LocalService restricted-SID lab validation

> **Status:** Historical · **Evidence dated:** 2026-11-01 (America/Los_Angeles, timestamps in UTC) · **Commit:** the commit that adds this record (parent `6718863`) · **Reviewed:** 2026-11-01

This record describes the first live run of the `NT AUTHORITY\LocalService`
profile with a restricted service SID. It answers one question: which ETW
rights does the restricted token need, and where should the grant go? It also
lists the defects the run found.

## Environment

| Component | State |
| --- | --- |
| Windows VM | `DNSTap-WS2025` (`24bafedb-0998-4430-a0c6-7e90350cd9ac`), Windows Server 2025, build 26100, DNS Server role, `DNS` Running |
| Receiver | DNS-collector 3.2.3 on `DNSTap-Ubuntu2604`, `172.27.215.60:6000` |
| Checkpoint | `pre-localservice-acl-test-2026-11-01`, taken before any change |
| Service | `EtwDnstap`, `--receiver 172.27.215.60:6000 --session-name EtwDnstap-Lab` |
| Final binary | SHA-256 `E92DB5F185262900337686CDE874C85208173E0C3FA3BAE70E7231F1D0380156` (session GUID build) |

Provisioning: `obj= "NT AUTHORITY\LocalService"`, `sidtype restricted`,
`privs SeChangeNotifyPrivilege`, `NT SERVICE\EtwDnstap` in Performance Log
Users, and read and execute on the installation directory for the service SID.

## The default ETW descriptor

The default WMI security GUID `{0811c1af-7a07-4a06-82ed-869455cdf713}` held
these ACEs. The DNS provider GUID `{eb79061a-a566-4698-9119-3ed2807060e7}` had
no descriptor of its own, so the default applied to it too.

| SID | Mask |
| --- | --- |
| Everyone `S-1-1-0` | `0x1800` |
| SYSTEM, LocalService, NetworkService, Administrators | `0x120fff` |
| Performance Log Users `S-1-5-32-559` | `0xee5` |
| Performance Monitor Users `S-1-5-32-558` | `0x4` |
| ALL APPLICATION PACKAGES and one capability SID | `0x1800` |

The restricted token's second access check considers only the restricting
SIDs: the service SID, Everyone, the logon SID, and `S-1-5-33`. Of those, only
Everyone appears, with `0x1800`, which grants no session right.

## Results

Every row was measured after a restart of the guest, except where noted.

| Grant to the service SID on the default GUID | SID type | Result |
| --- | --- | --- |
| none (no change since boot) | restricted | `StartTraceW` access denied |
| `0x20`, `0x21`, then `0x120fff`, applied live without a restart | restricted | `StartTraceW` access denied each time |
| none | unrestricted | runs |
| `0x120fff`, then restart | restricted | runs |
| `0x20` (`TRACELOG_CREATE_REALTIME`) only, then restart | restricted | runs |

With only `0x20`:

- `StartTraceW`, `EnableTraceEx2` with the PID filter, `OpenTraceW`,
  `ProcessTrace`, the `ControlTrace` query and stop, and the reclaim of a stale
  session all succeeded.
- 200 queries gave `received=800 encoded=800`, with every drop and reject
  counter at 0. The collector logged the records with `identity etw-dnstap`.
- `sc.exe stop` gave `STOPPED` with exit code 0, `output_errors=0`, and no
  session left behind.
- After `Stop-Process -Force` the session stayed. The next start logged
  `reclaimed stale ETW session` and captured again
  (`stale_sessions_reclaimed=1`).
- The Application log received Event IDs 1, 2 and 3 from the restricted
  token.
- The `Global\EtwDnstap-<session>` mutex was created, and the SCM query of the
  `DNS` service succeeded. Startup depends on both.

The rows above used the zero session GUID, so the default descriptor applied.

## Where Windows puts its own grants

On the same guest, 8 services run as LocalService with the restricted SID
type. Seven are inbox services: `BFE`, `CoreMessagingRegistrar`, `DPS`,
`mpssvc`, `NetTcpPortSharing`, `PerfHost` and `pla`. `pla` is the Performance
Logs and Alerts service behind `logman`, an ETW controller; `DPS` consumes ETW.
Of the 379 descriptors under `WMI\Security`, those naming a service SID were
all on specific GUIDs: `NT SERVICE\DPS`, `WdiServiceHost`, `EventLog`, `BFE`,
`mpssvc`, `MSDTC` and `netprofm`. None was on the default GUID.

## The default grant spreads

While the service SID held `0x20` on the default, seven other descriptors
appeared with the same ACE. Each was a full copy of the default plus an
AppContainer ACE with `0x200`. So another component creates descriptors by
copying the default, and every copy made while the grant is there keeps it. One
copy carried `0x120fff` from the live full-rights test above, and one appeared
after a single restart. Starting and stopping the service three times added no
values, so the service does not create them. `-Remove` on the default GUID
alone cannot reach these copies.

## Grant on the session GUID

The service was then given a fixed session GUID (`Wnode.Guid`). The default
descriptor was restored to its original content.

| Grant to the service SID | Result after a restart |
| --- | --- |
| `0x20` on the session GUID only, default unchanged | runs: start, 800 events, stop with exit code 0, stale-session reclaim |
| no descriptor for the session GUID | `StartTraceW` access denied |

So ETW checks `TRACELOG_CREATE_REALTIME` against the session GUID's own
descriptor, and the default no longer needs to change. The shipped build
derives the GUID from `--session-name`. For `EtwDnstap-Lab` it is
`{43b7b2fc-777a-5214-b6ed-665c1ff9e972}`, logged as `session_guid`. With it:

- The script cleaned the default (and one inherited copy) and granted on the
  session GUID. The default then matched the original descriptor. After a
  restart the full run passed, and only the session GUID named the service SID.
- `-Remove` deleted the session descriptor. After a restart, start was denied
  with a message that names the session GUID and the `-SessionName` to use.
- The script was run again, and the guest restarted. Only the session GUID named
  the service SID.
- First-version upgrade: the commit `6718863` script granted `0x423` on the
  default and `0x80` on the provider. The new script removed both and deleted
  the provider descriptor. The default matched the original afterwards.

## Conclusions

- **The service never logs into ETW.** It needs no `TRACELOG_LOG_EVENT`. The
  one right it needs is `TRACELOG_CREATE_REALTIME` (`0x20`), granted to the
  service SID on its own session GUID. Windows treats session creation as a write
  for a write-restricted token, so LocalService and Performance Log Users do
  not satisfy it.
- **Every other operation passes without a grant.** That covers enabling the
  provider, consuming the session, and querying and stopping it.
  `WMIGUID_QUERY`, `WMIGUID_SET`, `TRACELOG_ACCESS_REALTIME` and
  `TRACELOG_GUID_ENABLE` are not needed.
- **ETW applies descriptor changes at boot.** Grants applied live had no effect
  until the guest restarted.
- **The grant belongs on the session GUID, not the default.** That matches the
  inbox services, keeps the shared default unchanged, and avoids the spread
  into copies.

## Defects found and fixed in the same commit

- **The startup check reported "truncated token groups" for an unrestricted
  token.** `TokenRestrictedSids` returns only `GroupCount = 0`, which is shorter
  than `TOKEN_GROUPS`. With the fix, the service reports `the process token is
  not restricted`, confirmed on the guest.
- **The documented `sc.exe config ... password= ""` fails in Windows PowerShell
  5.1.** PowerShell drops the empty argument, `sc.exe` prints its usage, and the
  account does not change. The docs now run it through `cmd.exe /c`.
- **The grant script over-granted.** It granted `0x423` on the default GUID and
  `0x80` on the provider GUID. The grant on the provider created a descriptor
  of its own for that provider. The script now grants only `0x20` on the
  session GUID, and every run removes the earlier version's grants.
- **An intermediate `-Remove` created a provider descriptor where none had
  existed.** `EventAccessQuery` falls back to the default descriptor, so that
  code wrote the copy back. The script now reads and writes the registry values
  directly.

## Not covered

- The SCM recovery policy and a DNS Server restart. The guest had no failure
  actions configured.
- Load. This run used small query batches only.
- Builds other than 26100.

## Guest state at the end

The service is installed with the validated profile, the session GUID build,
and the `0x20` grant on `{43b7b2fc-777a-5214-b6ed-665c1ff9e972}`. It is Stopped.
No other descriptor names the service SID. The VM is Running. The checkpoint
above restores the earlier LocalSystem profile.
