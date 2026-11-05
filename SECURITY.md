# Security policy

## Supported versions

Proposed policy, to be confirmed by the maintainer: only the current `main` branch is supported. There are no tagged releases yet (see [CHANGELOG.md](CHANGELOG.md)).

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use GitHub private vulnerability reporting for `miketheitguy/etw-dnstap`: the **Security** tab, then **Report a vulnerability**. Include:

- The commit SHA and the exact command line. Redact the receiver address if it is private.
- The Windows build and the DNS Server role configuration.
- The steps to reproduce, and the observed and expected behavior.

Do not include real DNS payloads, client addresses, or query names. Synthetic reproductions are preferred. If private reporting is not enabled, open a public issue that says only that you need a private security contact.

## Threat model summary

This summarizes what the service trusts and protects. The authoritative behavior is in [configuration.md](docs/guide/configuration.md) and [README.md](README.md).

**Trusted or assumed**

- The local Windows host, its administrators, and the DNS Server role. The service reads DNS Server events from a local ETW session.
- Any principal that can enable the DNS Server ETW provider, or create or stop real-time ETW sessions. Administrators and Performance Log Users members have this ability.
- The configured dnstap receiver. It receives full DNS payloads.

**Producer identity (forged provider records)**

- The service admits only records whose process ID is the running `DNS` service process. The ETW session filters on that process ID (`EVENT_FILTER_TYPE_PID`), and the callback checks it again before it copies a record. Other records are counted in `foreign_pid_rejected` and not captured.
- This narrows the earlier open item, in which a local process that registered the same provider could inject records. A nonzero `foreign_pid_rejected` shows that such records reached the callback. Whether the ETW-side filter removes them first is unverified on a live host.
- The check does not stop code that runs inside the DNS Server process. That code can emit records the service accepts.
- Any user can register the DNS provider GUID, so the process ID filter is the only check on the source. The process ID is read at startup and checked every 5 seconds. If the DNS Server exits and Windows gives its process ID to another process before the next check, that process can inject records. A DNS Server restart also makes the service exit for SCM restart, so records from the new process are not captured in that gap. Holding the DNS Server process open would close the window, but the restricted LocalService token is denied `OpenProcess` on it (lab, build 26100). See [the lab record](docs/history/skills-evaluation-lab-validation.md).

**Service identity**

- The service runs as `NT AUTHORITY\LocalService` (`S-1-5-19`) with the restricted service SID type (`sc.exe sidtype EtwDnstap restricted`), not as LocalSystem. It refuses to start unless its token has:
  - LocalService as the user.
  - An enabled service SID group, and Performance Log Users as an enabled group through the service SID.
  - Only `SeChangeNotifyPrivilege`, enabled.
  - The service SID and the Write Restricted SID `S-1-5-33` as restricting SIDs.

  The [install guide](docs/guide/install.md#provision-the-service) has the same checks with the provisioning commands.
- LocalService has no network-credential identity. On the network it presents as anonymous, and its default local rights are low. The write-restricted token adds a second access check for write requests, against the restricting SIDs (the service SID, Everyone, the logon SID, and `S-1-5-33`). The service can write only to objects whose ACL grants one of those. Reads use the normal check only.
- The service only consumes ETW events, and holds no right to log events into ETW. Its one ETW grant is `TRACELOG_CREATE_REALTIME` on its own session's GUID, derived from `--session-name`. The grant goes to the service SID alone, not to all LocalService processes. [`Grant-EtwDnstapEtwAccess.ps1`](scripts/provision/README.md) applies it. Creating a session changes state, so the restricted token needs a restricting SID in the ACL for it. Enabling the provider, consuming, querying, and stopping need no grant. Provisioning also adds only the service SID to Performance Log Users.
- The shared default ETW descriptor is not changed. Windows gives its own restricted LocalService services (`DPS`, `WdiServiceHost`, `EventLog`, `BFE`, and `mpssvc`) rights on specific GUIDs only. A grant on the default would let the service SID create any session without its own descriptor. In the lab, the grant also spread to components that copy the default.
- The SCM security descriptor is pinned with `sc.exe sdset`. SYSTEM and Administrators can change or control the service. Interactive and service logons can query it, but cannot start, stop, or reconfigure it. Network and batch logons get no access. Install and repair steps are in [install.md](docs/guide/install.md).
- The profile was validated on Windows Server 2025, build 26100, on 2026-11-01 ([record](docs/history/localservice-restricted-validation.md)). For other builds, use the checklist in [install.md](docs/guide/install.md).

**Protected or limited**

- The service accepts no inbound connections. It holds one outbound TCP connection to the receiver given by `--receiver`, and reconnects after a failure.
- **Transport is TLS by default.** The connection uses TLS 1.2 or 1.3 (rustls, `ring` provider). Windows CryptoAPI verifies the receiver certificate against the machine certificate store. The certificate must match the receiver IP, or the name in `--tls-server-name`. The service sends no client certificate, so the receiver does not authenticate the service. A certificate that does not verify stops the service at startup.
- **Revocation** is checked for the receiver (end-entity) certificate only.
  - If the revocation status cannot be obtained, for example because the endpoint is unreachable, the connection proceeds.
  - A certificate with no OCSP responder and no CRL distribution point is not checked.
  - A certificate that CryptoAPI reports as revoked is rejected.
  - The status can come from a CRL, an OCSP responder, or an OCSP response that the receiver staples.
  - Hard-fail revocation and checks on intermediate certificates are not provided.

  See [configuration.md](docs/guide/configuration.md#tls).
- **`--plaintext` disables TLS.** Frame Streams over plaintext TCP has no peer authentication. Anyone on the path can read or alter DNS payloads and client addresses. Use it only on a trusted network, or behind an independently protected transport such as IPsec.
- DNS payloads can contain private names and addresses. The service does not log payloads, and writes no capture or metrics files except the spool. The collector is responsible for retention and access control for the records it receives.
- **Spool data at rest.** The spool keeps frames that the receiver has not accepted, in segment files in `C:\ProgramData\EtwDnstap\spool`. Each frame is a complete dnstap message, with client addresses, query names, and DNS payloads. The service encrypts each segment with AES-128-GCM. The key is stored wrapped by a non-exportable CNG machine key that only the service, SYSTEM, and Administrators can use. A changed record is detected and never replayed. The folder ACL still keeps other accounts out. See [Encryption](docs/guide/configuration.md#encryption).
  - **What the encryption protects against.** A copy or backup of the spool folder alone cannot be read. Other local accounts cannot read the frames and cannot use the key. A disk, image, or backup of a volume that BitLocker protects, with a TPM, cannot be read. The copied-disk case needs BitLocker.
  - **What it does not protect against.** SYSTEM, Administrators, and code running as the service can read the frames. An offline copy of a system volume without BitLocker can be read. The key file of the CNG machine key is protected by machine DPAPI. The DPAPI secret is in the SECURITY and SYSTEM registry hives on the same volume, and `WrappedKey` is in the SOFTWARE hive. Standard offline DPAPI tools recover the RSA key, and with it every segment. A VM saved state or memory checkpoint, the hibernation file, and a complete kernel memory dump also hold the key in the clear.
  - **The key in memory.** The service unwraps the key once at each start. It keeps the AES key schedule in one locked page that is never written to the pagefile and is left out of WER crash dumps (Windows Error Reporting), and it zeroes the page when the spool closes. The key is in the clear in that page while the service runs. The page is in physical memory, so a VM saved state or memory checkpoint, `hiberfil.sys`, and a complete kernel memory dump contain it. The WER exclusion covers WER crash dumps only.
  - **Who can read and write.** SYSTEM, Administrators, and the service SID. The install step grants no other account access. The service refuses a folder that gives any other account read or write access, logs one warning, and runs without a spool. Check the ACL after installation with `icacls`.
  - **Turning it off.** `--no-spool` stops new writes. It does not delete files, so frames already in the folder stay until someone deletes them. See [Uninstall](docs/guide/uninstall.md) and [Repair or upgrade](docs/guide/upgrade.md).
  - **Restarts and power loss.** Unsent frames stay on disk across restarts and reboots. The spool does not flush data to disk, so a power loss can lose recent frames.
- The executable and its install directory must not be writable by ordinary users. The service SID needs read and execute on the install directory, which covers the executable. The service writes no files there.
- The ETW session name is fixed by default (`EtwDnstap`). It is reclaimed at startup when it matches the program's shape. An administrator or a Performance Log Users member can stop it.

**Not provided**

- No runtime process mitigations beyond the link-time flags in [build.md](docs/guide/build.md#build-the-executable) (CFG, EH continuation guard, ASLR, NX, CET-compatible marker, stack protection).
- No resend of a lost frame. A frame in flight when a connection fails is counted in `disconnected_dropped` and is not retried. A frame is never sent twice. A frame held in the spool is sent once, oldest first. See [configuration.md](docs/guide/configuration.md#spool).
