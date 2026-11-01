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
- Any principal that can enable the DNS Server ETW provider or create or stop
  real-time ETW sessions. The service does not authenticate the producer of
  ETW events beyond the provider GUID, event IDs, and schema. A local process
  that can register the same provider could in principle inject records. This
  is an open verification item on a live host, not a demonstrated attack.
- The configured dnstap receiver. It receives full DNS payloads.

**Protected or limited**

- The service accepts no inbound network connections. It initiates one outbound
  TCP connection to the receiver given by `--receiver`.
- **Transport is plaintext.** Frame Streams over TCP has no TLS and no peer
  authentication. Anyone on the path can read or alter DNS payloads and client
  addresses. Use it only on a trusted network or behind an independently
  protected transport such as IPsec.
- DNS payloads can contain private names and addresses. The service does not
  log payloads, and it writes no capture or metrics files. The collector is
  responsible for retention and access control.
- The service runs as LocalSystem under the token profile described in
  [docs/guide/install.md](docs/guide/install.md). Its executable and install
  directory must not be writable by ordinary users.
- The ETW session name is fixed by default (`EtwDnstap`) and is reclaimed at
  startup when it matches the program's shape. An administrator or a member of
  Performance Log Users can stop it.

**Not provided**

- No runtime process mitigations beyond the link-time flags listed in
  [README.md](README.md) (CFG, CET-compatible marker, ASLR, NX, stack
  protection).
- No automatic reconnection or resend. A transport failure ends capture; see
  [docs/guide/troubleshooting.md](docs/guide/troubleshooting.md).
