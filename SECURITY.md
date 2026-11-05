# Security policy

## Supported versions

Proposed policy, to be confirmed by the maintainer: only the current `main` branch is supported. The project has no tagged releases yet (see [CHANGELOG.md](CHANGELOG.md)).

## Reporting a vulnerability

Report a suspected vulnerability privately, through GitHub private vulnerability reporting for `miketheitguy/etw-dnstap`. Open the **Security** tab, then choose **Report a vulnerability**. Do not open a public issue. Include:

- The commit SHA and the exact command line. Redact the receiver address if it is private.
- The Windows build and the DNS Server role configuration.
- The steps to reproduce, and the observed and expected behavior.

Use synthetic reproductions, not real DNS payloads, client addresses, or query names. If private reporting is not enabled, open a public issue that says only that you need a private security contact.

## Threat model summary

### Trusted or assumed

- The local Windows host, its administrators, and the DNS Server role. The service reads DNS Server events from a local ETW session.
- Any principal that can enable the DNS Server ETW provider or create or stop real-time ETW sessions: Administrators and Performance Log Users members.
- The configured dnstap receiver. It receives full DNS payloads.

### Forged provider records

The service accepts Microsoft-Windows-DNSServer events from any process. After a DNS Server crash and recovery restart, capture continues. A normal stop of the DNS Server stops EtwDnstap first, because the service depends on the `DNS` service.

- Any user can register the provider GUID. A local process that does so can inject records, which the service sends as DNS queries and responses. The event-ID and provider checks apply and the parser rejects malformed records, but nothing ties a record to `dns.exe`.
- Code that runs inside the DNS Server process can emit records that pass the event-ID filter.
- Mitigation: treat the receiver's data as unauthenticated at the record level. Limit who can run code on the DNS Server host.

### Service identity

The service runs as `NT AUTHORITY\LocalService` (`S-1-5-19`) with the restricted service SID type (`sc.exe sidtype EtwDnstap restricted`). Windows gives it a write-restricted token, which adds a second access check on writes. At startup, the service runs only if its token has all of these:

- LocalService as the token user.
- An enabled service SID group, and Performance Log Users as an enabled group through that service SID.
- `SeChangeNotifyPrivilege` as the only privilege, and it is enabled.
- The service SID and the Write Restricted SID (`S-1-5-33`) as restricting SIDs.

The [install guide](docs/guide/install.md#provision-the-service) has the same checks with the provisioning commands.

- LocalService is anonymous on the network and has low default local rights. The service can write only to objects whose ACL grants a restricting SID: the service SID, Everyone, the logon SID, or `S-1-5-33`. Reads use the normal check only.
- The service only consumes ETW events. Its one ETW grant is `TRACELOG_CREATE_REALTIME` on the GUID of its own session, which `--session-name` sets. The grant goes to the service SID alone. [`Grant-EtwDnstapEtwAccess.ps1`](scripts/provision/README.md) applies it. Enabling the provider, consuming, querying, and stopping need no grant. Provisioning adds only the service SID to Performance Log Users.
- The shared default ETW descriptor stays unchanged, because a grant there would let the service SID create any session without its own descriptor. Windows grants its own restricted LocalService services (`DPS`, `WdiServiceHost`, `EventLog`, `BFE`, and `mpssvc`) rights on specific GUIDs in the same way.
- The SCM security descriptor is pinned with `sc.exe sdset`. SYSTEM and Administrators can change or control the service. Interactive and service logons can query it only. Network and batch logons have no access.
- The profile is validated on Windows Server 2025, build 26100 ([record](docs/history/localservice-restricted-validation.md)). For other builds, use the [validation checklist](docs/guide/install.md#validation-checklist-for-the-localservice-restricted-profile).

### Transport

- The service accepts no inbound connections. It holds one outbound TCP connection to `--receiver` and reconnects after a failure.
- **TLS is the default:** TLS 1.2 or 1.3, through rustls with the `ring` provider. Windows verifies the receiver certificate against the machine certificate store. The certificate must match the receiver IP address, or the name in `--tls-server-name`. The service sends no client certificate, so the receiver cannot authenticate the service.
- A failed TLS check on the first connection stops startup. Later, a failed reconnect is logged and retried with backoff.
- **Revocation is best-effort, for the receiver (end-entity) certificate only.** A certificate that Windows CryptoAPI reports as revoked is rejected. If the status cannot be obtained, or the certificate names no OCSP responder and no CRL distribution point, the connection proceeds. Intermediate certificates are not checked. See [TLS](docs/guide/configuration.md#tls).
- **`--plaintext` turns TLS off.** Frame Streams over plaintext TCP has no peer authentication. Anyone on the path can read or change DNS payloads and client addresses. Use it only on a trusted network, or behind an independently protected transport such as IPsec.

### Payloads and logs

DNS payloads can contain private names and addresses. The service logs no payloads, and the spool is its only capture file. The collector owns retention and access control for the records it receives.

### Spool

The spool keeps unaccepted frames in segment files in `C:\ProgramData\EtwDnstap\spool`. Each frame is a complete dnstap message with client addresses, query names, and DNS payloads. The service encrypts each segment with AES-128-GCM. The AES key is stored wrapped by a non-exportable CNG machine key that only the service, SYSTEM, and Administrators can use. A changed record fails authentication and is never replayed. Key handling and its cost are in [Encryption](docs/guide/configuration.md#encryption).

- **Protected:** a copy or backup of the spool folder alone; other local accounts, which can read neither the frames nor the key; and a disk, image, or backup of a volume that BitLocker protects with a TPM.
- **Exposed:**
  - SYSTEM, Administrators, and code that runs as the service can read the frames.
  - An offline copy of a system volume without BitLocker can be read. The machine DPAPI secret and the `WrappedKey` value are in registry hives on the same volume, so standard offline DPAPI tools recover the RSA key, and with it every segment.
  - Assume that a full kernel memory capture (a complete memory dump, `hiberfil.sys`, or a VM saved state or memory checkpoint) lets an analyst recover the key.
- **The key in memory:** between uses, the AES key stays encrypted in a locked page that is kept out of the pagefile and out of Windows Error Reporting (WER) crash dumps. This protects the idle key against a process dump, the pagefile, and a memory-disclosure bug in the service. It remains exposed to:
  - Code in the service process, which can call `CryptUnprotectMemory`. SYSTEM and Administrators can inject such code.
  - A dump taken during a seal or open. Key material can remain in XMM registers, in stack below the 16 KiB that the service overwrites after each use, and, with key isolation, in the unzeroed RPC receive buffer that carries the `NCryptDecrypt` reply.
  - Crash dumps other than WER dumps. The release build uses `panic = "abort"`, so a panic ends the process. The WER exclusion covers the locked page only.
- **If memory protection fails:** the service suspends the spool until it restarts. It logs one warning (Event ID 26), refuses new frames, counts them in `spool_dropped`, and keeps every segment. The next start unwraps the key again and replays the kept backlog. See [Spool key errors](docs/guide/troubleshooting.md#spool-key-errors).
- **Access:** SYSTEM, Administrators, and the service SID only. The service refuses a folder that gives any other account read or write access: it logs one warning and runs without a spool. Check the ACL after installation with `icacls`.
- **`--no-spool`** stops new writes and leaves existing files in place until someone deletes them. See [Uninstall](docs/guide/uninstall.md) and [Repair or upgrade](docs/guide/upgrade.md).
- **Restarts and power loss:** unsent frames stay on disk across restarts and reboots. The spool relies on the OS to write data, so a power loss can lose recent frames.

### Other files and settings

- Ordinary users must not be able to write to the executable or its install directory. The service SID needs read and execute on the install directory, and writes no files there.
- The ETW session name defaults to `EtwDnstap`. The service reclaims a leaked session of the same name and shape at startup. An administrator or a Performance Log Users member can stop it.

## Limits

- Process mitigations are limited to the link-time flags in [build.md](docs/guide/build.md#build-the-executable): CFG, EH continuation guard, ASLR, NX, CET-compatible marker, and stack protection.
- A frame in flight when a connection fails is counted in `disconnected_dropped` and is lost. A frame is never sent twice. A frame held in the spool is sent once, oldest first. See [configuration.md](docs/guide/configuration.md#spool).
