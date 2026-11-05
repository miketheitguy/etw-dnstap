# Set up the dnstap receiver

> **Status:** Current · **Describes:** the code at commit `66c3de4` · **Reviewed:** 2026-11-05

Set up the dnstap receiver (the collector that accepts the service's data) before you start
the service.

## Receiver configuration

The service keeps one outbound TCP connection to `--receiver`. The connection uses Frame
Streams, a framing protocol for dnstap. The service negotiates the content type
`protobuf:dnstap.Dnstap` with the control messages READY, ACCEPT, and START, and then sends
data frames. At shutdown, the service sends STOP and waits up to 2 seconds for FINISH.

After a failure, the service reconnects and negotiates again. Frames the socket did not fully
accept go back for retry. With a spool, they are stored and sent after reconnect, and new frames
go to the spool while the receiver is down. Without a spool, they count as dropped. See
[Spool](configuration.md#spool).

TLS is the default. The service checks the receiver certificate and presents no client
certificate, so the receiver must not require one. With `--plaintext`, the connection has no peer
authentication.

This example is for DNS-collector 3.2.3. It listens for dnstap and writes text to the
console. Replace the listen IP with your collector's address:

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
      listen-ip: 192.0.2.10   # the collector's listening IP
      listen-port: 6000        # must match the --receiver port
      reset-conn: false
    routing-policy:
      forward: [console]
  - name: console
    stdout:
      mode: text
```

**Transport security.** The example is plaintext, so it needs `--plaintext`. Use plaintext only on
a trusted lab or an independently protected transport. A Hyper-V Default Switch is reachable by the
host and other guests. Restrict the listener and its firewall to the Windows DNS guest. On any other
network, keep TLS ([SECURITY.md](../../SECURITY.md#threat-model-summary)).

To enable TLS, add these keys to the `dnstap` stanza, with a certificate that meets the
[receiver certificate step](#receiver-certificate). The `tls-min-version` default of `1.2` suffices.

```yaml
      tls-support: true
      cert-file: "<path to the receiver certificate>"
      key-file: "<path to the receiver private key>"
```

## Receiver certificate

Skip this step only with `--plaintext`. The connection needs both of these:

- The receiver certificate chains to a CA in `Cert:\LocalMachine\Root`. The service runs as
  LocalService, so `Cert:\CurrentUser\Root` does not count.
- The certificate has an `iPAddress` SAN for the receiver IP, or the name that you pass to
  `--tls-server-name`. The Windows platform verifier matches the name.

If the revocation status cannot be obtained, the service continues. A slow revocation endpoint
can push the handshake past its 2-second deadline ([TLS](configuration.md#tls)).

To import a private CA into the machine root store, run:

```powershell
Import-Certificate -FilePath '.\receiver-ca.cer' -CertStoreLocation Cert:\LocalMachine\Root
```

If the first connection at startup fails the certificate check, or the receiver answers the
handshake with plaintext, the service stops with Event ID 1.

If the receiver is down at startup, or stays silent or closes the connection during the
handshake, the service retries and logs one Event ID 4 error. For the full list of TLS failure
cases, see [TLS](configuration.md#tls).

## Next steps

- [Install and provision the service](install.md)
- [TLS options](configuration.md#tls)
- [Troubleshooting a receiver outage](troubleshooting.md#the-receiver-is-down-or-unreachable)
