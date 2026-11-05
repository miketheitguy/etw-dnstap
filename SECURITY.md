# Security policy

## Supported versions

Proposed policy, to be confirmed by the maintainer: only the current `main` branch is supported. The project has no tagged releases yet (see [CHANGELOG.md](CHANGELOG.md)).

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Report it with GitHub private vulnerability reporting for `miketheitguy/etw-dnstap`. Open the **Security** tab, then choose **Report a vulnerability**. Include:

- The commit SHA and the exact command line. Redact the receiver address if it is private.
- The Windows build and the DNS Server role configuration.
- The steps to reproduce, and the observed and expected behavior.

Do not include real DNS payloads, client addresses, or query names. Use synthetic reproductions. If private reporting is not enabled, open a public issue that says only that you need a private security contact.

## Threat model summary

This section states what the service trusts and what it protects. The reference for behavior is [configuration.md](docs/guide/configuration.md) and [README.md](README.md).

### Trusted or assumed

- The local Windows host, its administrators, and the DNS Server role. The service reads DNS Server events from a local ETW session.
- Any principal that can enable the DNS Server ETW provider, or create or stop real-time ETW sessions. Administrators and Performance Log Users members have this ability.
- The configured dnstap receiver. It receives full DNS payloads.

### Forged provider records

The service does not filter records by process ID. The ETW session and the callback accept Microsoft-Windows-DNSServer events from any process. A DNS Server restart does not stop EtwDnstap.

- Any local process that can register the Microsoft-Windows-DNSServer provider GUID and write events can inject records into the capture. Any user can register a provider GUID. The service sends these records to the receiver as DNS queries and responses. The event-ID and provider checks still apply, and the parser rejects malformed records. Nothing ties a record to `dns.exe`.
- Code that runs inside the DNS Server process can emit records that the service accepts. The event-ID filter does not stop it.
- A normal stop of the DNS Server also stops EtwDnstap, because the service depends on the `DNS` service.
- Mitigation: treat the receiver's data as unauthenticated at the record level. Limit who can run code on the DNS Server host.

### Service identity

The service runs as `NT AUTHORITY\LocalService` (`S-1-5-19`) with the restricted service SID type (`sc.exe sidtype EtwDnstap restricted`). It does not run as LocalSystem. Windows gives it a write-restricted token, which adds a second access check on writes. At startup, the service refuses to run unless its token has all of these:

- LocalService as the token user.
- An enabled service SID group, and Performance Log Users as an enabled group through that service SID.
- `SeChangeNotifyPrivilege` as the only privilege, and it is enabled.
- The service SID and the Write Restricted SID (`S-1-5-33`) as restricting SIDs.

The [install guide](docs/guide/install.md#provision-the-service) has the same checks with the provisioning commands.

- LocalService has no network credentials. On the network it is anonymous, and its default local rights are low. For writes, the write-restricted check considers only the restricting SIDs: the service SID, Everyone, the logon SID, and `S-1-5-33`. The service can write only to objects whose ACL grants one of those SIDs. Reads use the normal check only.
- The service only consumes ETW events. It has no right to log events into ETW. Its one ETW grant is `TRACELOG_CREATE_REALTIME` on the GUID of its own session, which `--session-name` sets. The grant goes to the service SID alone, not to all LocalService processes. [`Grant-EtwDnstapEtwAccess.ps1`](scripts/provision/README.md) applies it. Creating a session changes state, so the restricted token needs the service SID in the ACL for it. Enabling the provider, consuming, querying, and stopping need no grant. Provisioning also adds only the service SID to Performance Log Users.
- The shared default ETW descriptor is not changed. Windows gives its own restricted LocalService services (`DPS`, `WdiServiceHost`, `EventLog`, `BFE`, and `mpssvc`) rights on specific GUIDs only. A grant on the default would let the service SID create any session that has no descriptor of its own. In the lab, the grant also spread to components that copy the default.
- The SCM security descriptor is pinned with `sc.exe sdset`. SYSTEM and Administrators can change or control the service. Interactive and service logons can query it, but cannot start, stop, or reconfigure it. Network and batch logons have no access. Install and repair steps are in [install.md](docs/guide/install.md).
- The profile was validated on Windows Server 2025, build 26100, on 2026-11-01 ([record](docs/history/localservice-restricted-validation.md)). For other builds, use the [validation checklist](docs/guide/install.md#validation-checklist-for-the-localservice-restricted-profile).

### Transport

- The service accepts no inbound connections. It holds one outbound TCP connection to the receiver given by `--receiver`, and it reconnects after a failure.
- **TLS is the default.** The connection uses TLS 1.2 or 1.3 (rustls with the `ring` provider). Windows verifies the receiver certificate against the machine certificate store. The certificate must match the receiver IP address, or the name in `--tls-server-name`. The service sends no client certificate, so the receiver does not authenticate the service.
- If the first connection fails the TLS check, startup fails. After the first session, a failed reconnect is logged and retried with backoff.
- **Revocation** is checked for the receiver (end-entity) certificate only. Hard-fail revocation and checks on intermediate certificates are not provided.
  - If the revocation status cannot be obtained, for example because the responder is unreachable, the connection proceeds.
  - A certificate with no OCSP responder and no CRL distribution point is not checked.
  - A certificate that Windows CryptoAPI reports as revoked is rejected.
  - The status can come from a CRL, an OCSP responder, or an OCSP response that the receiver staples.

  See [TLS](docs/guide/configuration.md#tls).
- **`--plaintext` turns TLS off.** Frame Streams over plaintext TCP has no peer authentication. Anyone on the path can read or change DNS payloads and client addresses. Use it only on a trusted network, or behind an independently protected transport such as IPsec.

### Payloads and logs

- DNS payloads can contain private names and addresses. The service does not log payloads, and it writes no capture or metrics files except the spool. The collector is responsible for retention and access control for the records it receives.

### Spool

The spool keeps frames that the receiver has not accepted. It stores them in segment files in `C:\ProgramData\EtwDnstap\spool`. Each frame is a complete dnstap message, with client addresses, query names, and DNS payloads. The service encrypts each segment with AES-128-GCM. The key is stored wrapped by a non-exportable CNG machine key, which only the service, SYSTEM, and Administrators can use. A changed record is detected and never replayed. The folder ACL keeps other accounts out. Details are in [Encryption](docs/guide/configuration.md#encryption).

- **What the encryption protects against.** A copy or backup of the spool folder alone cannot be read. Other local accounts cannot read the frames or use the key. A disk, image, or backup of a volume that BitLocker protects with a TPM cannot be read. The copied-disk case needs BitLocker.
- **What it does not protect against.**
  - SYSTEM, Administrators, and code that runs as the service can read the frames.
  - An offline copy of a system volume without BitLocker can be read. The CNG machine key file is protected by machine DPAPI. The DPAPI secret is in the SECURITY and SYSTEM registry hives on the same volume, and the `WrappedKey` value is in the SOFTWARE hive. Standard offline DPAPI tools recover the RSA key, and with it every segment.
  - A full kernel memory capture (a complete memory dump, `hiberfil.sys`, or a VM saved state or memory checkpoint) likely lets an analyst recover the key. Microsoft does not document where the memory-protection key lives, so assume that a full capture contains it.
- **The key in memory.** Between uses, the service keeps the AES key encrypted in a locked memory page. The page is never written to the pagefile and is left out of Windows Error Reporting (WER) crash dumps. [Encryption](docs/guide/configuration.md#encryption) describes each step. This protects the idle key bytes against a process dump, the pagefile, a memory-disclosure bug in the service, and casual inspection. It does not protect against:
  - Code in the service process. That code can call `CryptUnprotectMemory`. SYSTEM and Administrators can inject such code.
  - Key material left in registers or on the stack. AES-NI leaves round keys in XMM registers, and a dump captures them in thread contexts. The service overwrites 16 KiB of stack after each use, but stack below that is not cleared. With key isolation, the reply of `NCryptDecrypt` passes through an RPC receive buffer in the process that the service does not zero. A dump taken during a seal or open can hold the key schedule.
  - Crash dumps other than WER dumps. The release build uses `panic = "abort"`, so a panic ends the process. The WER exclusion covers the locked page, but not the stack.
- **If memory protection fails.** The service suspends the spool until it restarts, and it keeps the spooled files. It logs one warning (Event ID 26). It refuses new frames and counts them in `spool_dropped`, and it deletes no segment. Live frames go to the receiver when it is up, and are refused and counted when it is down. The next start unwraps the key again and replays the kept backlog. See [Spool key errors](docs/guide/troubleshooting.md#spool-key-errors).
- **Who can read and write.** SYSTEM, Administrators, and the service SID. The install step grants no other account access. The service refuses a folder that gives any other account read or write access. It logs one warning and runs without a spool. Check the ACL after installation with `icacls`.
- **Turning it off.** `--no-spool` stops new writes. It does not delete files, so frames already in the folder stay until someone deletes them. See [Uninstall](docs/guide/uninstall.md) and [Repair or upgrade](docs/guide/upgrade.md).
- **Restarts and power loss.** Unsent frames stay on disk across restarts and reboots. The spool does not flush data to disk, so a power loss can lose recent frames.

### Other files and settings

- The executable and its install directory must not be writable by ordinary users. The service SID needs read and execute on the install directory, which covers the executable. The service writes no files there.
- The ETW session name is fixed by default (`EtwDnstap`). The service reclaims a leaked session of the same name and shape at startup. An administrator or a Performance Log Users member can stop it.

## Not provided

- No runtime process mitigations beyond the link-time flags in [build.md](docs/guide/build.md#build-the-executable): CFG, EH continuation guard, ASLR, NX, CET-compatible marker, and stack protection.
- No resend of a lost frame. A frame in flight when a connection fails is counted in `disconnected_dropped` and is not retried. A frame is never sent twice. A frame held in the spool is sent once, oldest first. See [configuration.md](docs/guide/configuration.md#spool).
