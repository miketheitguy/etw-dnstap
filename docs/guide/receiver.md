# Set up the dnstap receiver

> **Status:** Current · **Describes:** the code at commit `66c3de4` · **Reviewed:** 2026-11-05

### Quick Start

DNS-collector 3.2.3: listen for dnstap, print text. Set `listen-ip` to the collector's address and
`listen-port` to the `--receiver` port. This is plaintext, so start the service with `--plaintext`.

```yaml
global:
  framestream:
    content-type: "protobuf:dnstap.Dnstap"
    handshake-timeout: 5
    data-frame-max-length: 1048576
    control-frame-max-length: 4064
pipelines:
  - name: tap
    dnstap:
      listen-ip: 192.0.2.10
      listen-port: 6000
      reset-conn: false
    routing-policy:
      forward: [console]
  - name: console
    stdout:
      mode: text
```

### Steps / Configuration

#### Receiver configuration

The service keeps one outbound TCP connection to `--receiver` and negotiates `protobuf:dnstap.Dnstap` over
Frame Streams (READY, ACCEPT, START). At shutdown it sends STOP and waits up to 2 seconds for FINISH.
After a failure it reconnects. Frames the socket didn't fully accept go back for retry. With a spool, they
are stored and sent after the reconnect, and new frames spool while the receiver is down. Without one,
they count as dropped ([Spool](configuration.md#spool)).

TLS is the default. The service presents no client certificate, so the receiver must not require one. Add
these keys to the `dnstap` stanza, with a certificate that meets the
[receiver certificate step](#receiver-certificate). The default `tls-min-version` `1.2` suffices.

```yaml
      tls-support: true
      cert-file: "<path to the receiver certificate>"
      key-file: "<path to the receiver private key>"
```

#### Receiver certificate

Skip this only with `--plaintext`. Both must hold:

- The certificate chains to a CA in `Cert:\LocalMachine\Root`. The service runs as LocalService, so
  `Cert:\CurrentUser\Root` doesn't count.
- It has an `iPAddress` SAN for the receiver IP, or for the `--tls-server-name` value. The Windows platform
  verifier matches the name.

```powershell
Import-Certificate -FilePath '.\receiver-ca.cer' -CertStoreLocation Cert:\LocalMachine\Root
```

### Common Gotchas

- **Plaintext has no peer authentication.** Use it only on a trusted lab or an independently protected
  transport. A Hyper-V Default Switch is reachable by the host and other guests, so restrict the listener
  and firewall to the Windows DNS guest. Elsewhere, keep TLS
  ([SECURITY.md](../../SECURITY.md#threat-model-summary)).
- A first connection that fails the certificate check, or a receiver that answers the handshake with
  plaintext, stops the service with Event ID 1.
- A receiver that is down at startup, or silent or closing during the handshake, makes the service retry and
  log one Event ID 4 error ([TLS](configuration.md#tls),
  [troubleshooting](troubleshooting.md#the-receiver-is-down-or-unreachable)).
- If revocation status can't be obtained, the service continues. A slow revocation endpoint can push the
  handshake past its 2-second deadline.
