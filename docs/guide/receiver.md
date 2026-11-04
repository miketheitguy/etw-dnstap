# Set up the dnstap receiver

> **Status:** Current · **Describes:** the code at commit `58fde72` · **Reviewed:** 2026-11-03

Prepare the collector before you start the service. The service also reconnects if the
receiver starts later.

## Receiver configuration

The service keeps one outbound TCP connection to `--receiver`. It negotiates the content
type `protobuf:dnstap.Dnstap` with READY, ACCEPT, and START, then sends data frames. On
shutdown, it sends STOP and waits for FINISH. After a failure, it reconnects and
negotiates again. Frames in flight at a failure are counted, not resent. While the receiver
is down, frames go to the spool if one is in use. See [Spool](configuration.md#spool).

TLS is the default. The service checks the receiver certificate and presents no client
certificate, so the receiver must not require one. With `--plaintext`, the connection has
no peer authentication.

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

Revocation endpoints need not be reachable. The service proceeds when a revocation status
cannot be obtained. A slow endpoint can push the handshake past its 2-second deadline. See
[TLS](configuration.md#tls).

To import a private CA into the machine root store, run:

```powershell
Import-Certificate -FilePath '.\receiver-ca.cer' -CertStoreLocation Cert:\LocalMachine\Root
```

A certificate rejected on the first connection at startup stops the service with Event ID 1.
So does a receiver that answers the handshake with plaintext.

If the receiver is down at startup, the service retries. It logs one Event ID 4 error that
names the trust store, the server name, and the client-certificate requirement as checks.
A receiver that stays silent or closes the connection during the handshake is an outage,
and the service retries. See [TLS](configuration.md#tls).

## Next steps

- [Install and provision the service](install.md)
- [TLS options](configuration.md#tls)
- [Troubleshooting a receiver outage](troubleshooting.md#the-receiver-is-down-or-unreachable)
