# Security policy

## Supported versions

Proposed policy, to be confirmed by the maintainer: only the current `main`
branch is supported. There are no tagged releases yet (see
[CHANGELOG.md](CHANGELOG.md)).

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use GitHub's private
vulnerability reporting for `miketheitguy/etw-dnstap`
(the repository's **Security** tab, then **Report a vulnerability**). Include:

- The commit SHA and the exact command line. Redact the receiver address if it
  is private.
- The Windows build and the DNS Server role configuration.
- The steps to reproduce, and the observed and expected behavior.

Do not include real DNS payloads, client addresses, or query names from a
production system. Synthetic reproductions are preferred.

Private reporting is the only documented channel. If it is not enabled on the
repository, open a public issue that says only that you need a private security
contact. Do not include details.

## Threat model summary

This is the project's own summary of what the service trusts and protects. The
authoritative behavior is in [docs/guide/configuration.md](docs/guide/configuration.md)
and [README.md](README.md).

**Trusted or assumed**

- The local Windows host, its administrators, and the DNS Server role. The
  service reads DNS Server events from a local ETW session.
- Any principal that can enable the DNS Server ETW provider, or create or stop
  real-time ETW sessions. Administrators and members of Performance Log Users
  have this ability.
- The configured dnstap receiver. It receives full DNS payloads.

**Producer identity (forged provider records)**

- The service admits only records whose process ID is the running `DNS` service
  process. The ETW session is filtered by that process ID
  (`EVENT_FILTER_TYPE_PID`), and the callback checks the record's process ID
  again before it copies the record. Records from any other process are counted
  in `foreign_pid_rejected` and are not captured.
- This narrows the earlier open item, in which a local process that could
  register the same provider could inject records. A nonzero
  `foreign_pid_rejected` shows that records from another process reached the
  callback. Whether the ETW-side filter removes them first is unverified on a
  live host.
- The check does not stop code that runs inside the DNS Server process. A
  process that can run code as the DNS Server can emit records the service
  accepts.
- The process ID is read at startup and checked every 5 seconds. A DNS Server
  restart makes the service exit for SCM restart. Records from the new process
  are not captured during that gap.
- Any user can register the DNS provider GUID and write events, so the process
  ID filter is the only check on the source. If the DNS Server exits and Windows
  gives its process ID to another process before the next 5-second check, that
  process can inject records. Holding the DNS Server process open would close
  the window, but the restricted LocalService token is denied `OpenProcess` on
  it (lab, build 26100). See
  [the lab record](docs/history/skills-evaluation-lab-validation.md).

**Service identity**

- The service runs as `NT AUTHORITY\LocalService` (`S-1-5-19`) with a restricted
  service SID type (`sc.exe sidtype EtwDnstap restricted`), not as LocalSystem. It
  refuses to start unless all of these are true:
  - The token user is S-1-5-19.
  - The service SID is an enabled group.
  - The token is restricted. Its restricting SIDs include the service SID and
    the Write Restricted SID `S-1-5-33`.
  - Performance Log Users is an enabled group, through the service SID's
    membership.
  - The token holds only `SeChangeNotifyPrivilege`, and it is enabled.

  The [install guide](docs/guide/install.md#provision-the-service) has the same
  list with the provisioning commands.
- LocalService has no network-credential identity: on the network it presents as
  anonymous, not as the computer account, and its default local rights are low.
  The restricted SID type adds a write-restricted token. For write-type access
  checks, a second pass runs against the restricting SIDs (the service SID,
  Everyone, the logon SID, and `S-1-5-33`). The service can therefore write only
  to objects whose ACL grants one of those, such as the service SID, Everyone, or
  Write Restricted. Reads use the normal check only.
- The service only consumes ETW events. It holds no right to log events into
  ETW. Its one ETW grant is `TRACELOG_CREATE_REALTIME` on its own session's
  GUID, derived from `--session-name`, given to the service SID alone, not to
  all LocalService processes, by
  [`scripts/provision/Grant-EtwDnstapEtwAccess.ps1`](scripts/provision/README.md).
  Creating a session is a change of state, so the write-restricted token needs a
  restricting SID in the ACL for it. Enabling the provider, consuming, querying
  and stopping need no grant. Provisioning also adds only the service SID to
  Performance Log Users.
- The shared default ETW descriptor is not changed. Windows provisions its own
  restricted LocalService services the same way: `DPS`, `WdiServiceHost`,
  `EventLog`, `BFE` and `mpssvc` hold ETW rights on specific GUIDs, never on the
  default. A grant on the default would let the service SID create any session
  that has no descriptor of its own. In the lab it also spread: other
  components copy the default when they create descriptors.
- The service's SCM security descriptor is pinned with `sc.exe sdset`. SYSTEM and
  Administrators can change or control the service. Interactive and service
  logons can query it, and they cannot start, stop, or reconfigure it. Network
  and batch logons get no access. Install and repair steps are
  in [install.md](docs/guide/install.md).
- The profile was validated on Windows Server 2025, build 26100, on 2026-11-01.
  See [docs/history/localservice-restricted-validation.md](docs/history/localservice-restricted-validation.md).
  Other builds are covered by the checklist in
  [docs/guide/install.md](docs/guide/install.md).

**Protected or limited**

- The service accepts no inbound network connections. It holds one outbound TCP
  connection to the receiver given by `--receiver`, and it reconnects after a
  failure.
- **Transport is plaintext.** Frame Streams over TCP has no TLS and no peer
  authentication. Anyone on the path can read or alter DNS payloads and client
  addresses. Use it only on a trusted network or behind an independently
  protected transport such as IPsec.
- DNS payloads can contain private names and addresses. The service does not
  log payloads, and it writes no capture or metrics files. The collector is
  responsible for retention and access control.
- The executable and its install directory must not be writable by ordinary
  users. The service SID needs read and execute on the install directory, which
  covers the executable. The service writes no files.
- The ETW session name is fixed by default (`EtwDnstap`) and is reclaimed at
  startup when it matches the program's shape. An administrator or a member of
  Performance Log Users can stop it.

**Not provided**

- No runtime process mitigations beyond the link-time flags listed in
  [install.md](docs/guide/install.md#build-the-executable) (CFG, EH continuation
  guard, ASLR, NX, CET-compatible marker, stack protection).
- No resend. Frames that are lost while the receiver is disconnected are counted
  in `disconnected_dropped` and are not retried. See
  [docs/guide/configuration.md](docs/guide/configuration.md#receiver-connection).
