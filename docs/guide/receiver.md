# Set up the dnstap receiver

> **Status:** Current · **Describes:** the code at commit `66c3de4` · **Reviewed:** 2026-11-05

Set up the dnstap receiver (the collector that accepts the service's data) before you start
the service. The service also reconnects if the receiver starts later.

## Receiver configuration

The service keeps one outbound TCP connection to `--receiver`. The connection uses Frame
Streams, a framing protocol for dnstap. The service negotiates the content type
`protobuf:dnstap.Dnstap` with the control messages READY, ACCEPT, and START, and then sends
data frames. When connected at shutdown, the service sends STOP and waits up to 2 seconds for
FINISH.

After a failure, the service reconnects and negotiates again. Frames the socket did not fully
accept go back for retry. With a spool, they are stored and sent after reconnect. Without a
spool, they are counted as dropped. Frames the socket fully accepted are not sent again. While
the receiver is down, new frames go to the spool if one is in use. See [Spool](configuration.md#spool).

TLS (Transport Layer Security, the encryption used by HTTPS) is the default. The service
checks the receiver certificate and presents no client certificate, so the receiver must not
require one. With `--plaintext`, the connection has no peer authentication.

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

A normal dnstap ACCEPT fits the service's control payload limit. See
[Message and transport limits](configuration.md#message-and-transport-limits).

**Transport security.** The example is plaintext, so it needs `--plaintext`. Use plaintext
only on a trusted lab or an independently protected transport. A Hyper-V Default Switch is
reachable by the host and other connected guests, so it is not a confidentiality boundary.
Restrict the listener to the intended guest IP, and its firewall to the Windows DNS guest.
On any other network, keep TLS. See the [receiver certificate step](#receiver-certificate)
and [SECURITY.md](../../SECURITY.md#threat-model-summary).

To enable TLS on the example listener, add these keys to its `dnstap` stanza. Use a
certificate that meets the [receiver certificate step](#receiver-certificate). The
`tls-min-version` default of `1.2` meets the service's requirement.

```yaml
      tls-support: true
      cert-file: "<path to the receiver certificate>"
      key-file: "<path to the receiver private key>"
```

## Receiver certificate

The service checks the receiver certificate against the Windows machine certificate store.
Skip this step only with `--plaintext`. The connection fails unless both conditions below
are met:

- The receiver certificate chains to a CA in `Cert:\LocalMachine\Root`. The service runs
  as LocalService and cannot see a user store, so `Cert:\CurrentUser\Root` does not count.
- The certificate names the receiver: an `iPAddress` SAN for the receiver IP, or the name
  that you pass to `--tls-server-name`. The Windows platform verifier does the name
  matching. Whether a `dNSName` SAN or the common name also works is not verified.

Revocation endpoints do not need to be reachable. If the revocation status cannot be
obtained, the service continues. A slow endpoint can push the handshake past its 2-second
deadline. See [TLS](configuration.md#tls).

To import a private CA into the machine root store, run:

```powershell
Import-Certificate -FilePath '.\receiver-ca.cer' -CertStoreLocation Cert:\LocalMachine\Root
```

If the first connection at startup fails the certificate check, the service stops with
Event ID 1. It also stops if the receiver answers the handshake with plaintext.

If the receiver is down at startup, the service retries. It logs one Event ID 4 error. A
receiver that stays silent or closes the connection during the handshake counts as an outage,
and the service retries. For the full list of TLS failure cases, see [TLS](configuration.md#tls).

## Next steps

- [Install and provision the service](install.md)
- [TLS options](configuration.md#tls)
- [Troubleshooting a receiver outage](troubleshooting.md#the-receiver-is-down-or-unreachable)
