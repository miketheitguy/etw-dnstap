# Security policy

### Quick Start

Report privately: `miketheitguy/etw-dnstap` > **Security** > **Report a vulnerability**. No public issues. Include:

- Commit SHA and exact command line (redact a private receiver address).
- Windows build, DNS Server role configuration.
- Repro steps, observed and expected behavior.

Synthetic repros only: no real DNS payloads, client addresses, or query names. No private reporting? Open a public issue asking only for a private security contact.

Supported: `main` only (proposed, pending maintainer confirmation). No tagged releases yet ([CHANGELOG.md](CHANGELOG.md)).

### Steps / Configuration

#### Threat model summary

Trusted:

- The local host, its administrators, the DNS Server role.
- Administrators and Performance Log Users: they can enable the DNS Server ETW provider or create or stop real-time sessions.
- The receiver. It gets full DNS payloads.

##### Service identity

`NT AUTHORITY\LocalService` (`S-1-5-19`), `sc.exe sidtype EtwDnstap restricted`, write-restricted token. Startup refuses a token without:

- LocalService as token user.
- An enabled service SID group, with Performance Log Users enabled through it.
- `SeChangeNotifyPrivilege`, enabled, as the only privilege.
- Restricting SIDs: service SID and Write Restricted (`S-1-5-33`).

Commands: [install guide](docs/guide/install.md#provision-the-service).

- Writes need an ACL entry for the service SID, Everyone, the logon SID, or `S-1-5-33`. Reads get the normal check.
- One ETW grant: `TRACELOG_CREATE_REALTIME` on its own session GUID (`--session-name`), service SID only, via [`Grant-EtwDnstapEtwAccess.ps1`](scripts/provision/README.md). Enabling, consuming, querying, and stopping need none. The shared default ETW descriptor stays untouched.
- Performance Log Users gets only the service SID.
- SCM descriptor (`sc.exe sdset`): SYSTEM and Administrators change and control. Interactive and service logons query. Network and batch logons: none.
- Validated on Windows Server 2025, build 26100 ([record](docs/history/localservice-restricted-validation.md)). Other builds: [validation checklist](docs/guide/install.md#validation-checklist-for-the-localservice-restricted-profile).

##### Transport

- Outbound only: one TCP connection to `--receiver`, reconnected after failure.
- **TLS by default:** 1.2 or 1.3, rustls with `ring`. Windows checks the receiver certificate against the machine store. It must match the receiver IP or `--tls-server-name`.
- No client certificate.
- First-connection TLS failure stops startup. Later failures retry with backoff.

##### Payloads and spool

- No payloads in logs. The spool is the only capture file.
- Spool: `C:\ProgramData\EtwDnstap\spool`. Each frame is a complete dnstap message with client addresses, query names, DNS payloads.
- AES-128-GCM per segment. The key is wrapped by a non-exportable CNG machine key usable by the service, SYSTEM, and Administrators. Changed records fail authentication and never replay. See [Encryption](docs/guide/configuration.md#encryption).
- **Protected:** a spool-folder copy alone; other local accounts; images of a TPM-protected BitLocker volume.
- The idle key stays encrypted in a locked page, out of the pagefile and Windows Error Reporting (WER) dumps.
- **Access:** SYSTEM, Administrators, service SID. Any other read or write grant: folder refused, one warning, no spool. Check with `icacls`.
- `--no-spool` stops writes and keeps existing files ([Uninstall](docs/guide/uninstall.md), [Repair or upgrade](docs/guide/upgrade.md)).

##### Other files

- Ordinary users mustn't write to the executable or install directory. The service SID gets read and execute there.
- Session name defaults to `EtwDnstap`. Startup reclaims a leaked session of the same name and shape.

### Common Gotchas

- **Forged provider records.** Any process can emit Microsoft-Windows-DNSServer events, and any user can register the provider GUID. Injected records go out as DNS traffic. Event-ID, provider, and parser checks apply, but nothing ties a record to `dns.exe`. Code inside the DNS Server process passes the event-ID filter too.
- Treat receiver data as unauthenticated per record. Limit who runs code on the DNS Server host.
- A DNS Server crash and recovery restart keeps capture running. A normal DNS Server stop stops EtwDnstap first.
- **Revocation is best-effort, receiver (end-entity) certificate only.** Revoked: rejected. Unknown status, or no OCSP responder and no CRL distribution point: proceeds. Intermediates aren't checked. See [TLS](docs/guide/configuration.md#tls).
- **`--plaintext` turns TLS off.** Anyone on the path can read or change payloads. Use only on a trusted network or behind IPsec.
- **Spool readable by:**
  - SYSTEM, Administrators, code running as the service.
  - An offline copy of a system volume without BitLocker: the DPAPI secret and `WrappedKey` live in its hives.
  - A full kernel memory capture (memory dump, `hiberfil.sys`, VM saved state or checkpoint).
- **Key in memory exposed to:**
  - Service-process code calling `CryptUnprotectMemory`.
  - A dump during seal or open: XMM registers, stack below the 16 KiB overwritten after each use, and, with key isolation, the unzeroed `NCryptDecrypt` RPC receive buffer.
  - Non-WER crash dumps (`panic = "abort"`).
- **Memory protection fails:** spool suspended until restart, one warning (Event ID 26), new frames counted in `spool_dropped`, segments kept and replayed next start. See [Spool key errors](docs/guide/troubleshooting.md#spool-key-errors).
- Unsent frames survive restarts and reboots. Power loss can lose recent ones: the spool relies on the OS to write data.
- A frame in flight at disconnect is lost and counted in `disconnected_dropped`. Nothing is sent twice. Spooled frames go oldest first ([configuration.md](docs/guide/configuration.md#spool)).
- Administrators and Performance Log Users can stop the ETW session.
- Mitigations are only the [link-time flags](docs/guide/build.md#build-the-executable): CFG, EH continuation guard, ASLR, NX, CET-compatible, stack protection.
