# Security policy

## Supported versions

Proposed policy, to be confirmed by the maintainer: only the current `main`
branch is supported. There are no tagged releases yet (see
[CHANGELOG.md](CHANGELOG.md)).

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use GitHub's private
vulnerability reporting for `miketheitguy/etw-dnstap`
(the repository's **Security** tab, then **Report a vulnerability**). Include:

- the commit SHA and the exact command line (redact the receiver address if it
  is private);
- the Windows build and DNS Server role configuration;
- steps to reproduce, and the observed and expected behavior.

Do not include real DNS payloads, client addresses, or query names from a
production system. Synthetic reproductions are preferred.

Private reporting is the only documented channel. If it is not enabled on the
repository, open a public issue that says only that you need a private security
contact, and do not include details.

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

**Service identity**

- The service runs as `NT AUTHORITY\LocalService` (`S-1-5-19`) with a restricted
  service SID type (`sc.exe sidtype EtwDnstap restricted`), not as LocalSystem. It
  refuses to start unless its token user is S-1-5-19, the service SID is an
  enabled group, the token is restricted with restricting SIDs that include the
  service SID and the Write Restricted SID `S-1-5-33`, Performance Log Users is an
  enabled group (through the service SID's membership), and the token holds only
  `SeChangeNotifyPrivilege`, enabled.
- LocalService has no network-credential identity: on the network it presents as
  anonymous, not as the computer account, and its default local rights are low.
  The restricted SID type adds a write-restricted token. For write-type access
  checks, a second pass runs against the restricting SIDs (the service SID,
  Everyone, the logon SID, and `S-1-5-33`). The service can therefore write only
  to objects whose ACL grants one of those, such as the service SID, Everyone, or
  Write Restricted. Reads use the normal check only.
- ETW rights are granted to the service SID alone, by
  [`scripts/provision/Grant-EtwDnstapEtwAccess.ps1`](scripts/provision/README.md),
  and not to all LocalService processes. The script also adds only that SID to
  Performance Log Users.
- The profile and the restricting-SID set are assumptions from Microsoft
  documentation and have **not been validated on a live DNS Server**.
  The lab record, [docs/history/lab-validation.md](docs/history/lab-validation.md),
  covers the earlier LocalSystem profile. The provisioning checklist is in
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
  users. The service SID needs read and execute on the executable only.
- The ETW session name is fixed by default (`EtwDnstap`) and is reclaimed at
  startup when it matches the program's shape. An administrator or a member of
  Performance Log Users can stop it.

**Not provided**

- No runtime process mitigations beyond the link-time flags listed in
  [README.md](README.md) (CFG, CET-compatible marker, ASLR, NX, stack
  protection).
- No resend. Frames that are lost while the receiver is disconnected are counted
  in `disconnected_dropped` and are not retried. See
  [docs/guide/configuration.md](docs/guide/configuration.md#receiver-connection).
