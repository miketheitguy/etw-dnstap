# Set up the dnstap receiver

> **Status:** Current · **Describes:** the code at commit `58fde72` · **Reviewed:** 2026-11-03

Use this guide to prepare the collector that receives the records. Do this
before you start the service, although the service also reconnects if the
receiver starts later.

## Receiver configuration

The service keeps one outbound TCP connection to `--receiver`. It negotiates
content type `protobuf:dnstap.Dnstap` with READY, ACCEPT, and START, then sends
data frames. On shutdown it sends STOP and waits for FINISH. After a failure it
reconnects and negotiates again. Frames in flight when a connection fails are
counted and not resent. While the receiver is down, frames go to the spool when one
is in use, and they are sent after it returns. See [Spool](configuration.md#spool).
The connection is TLS by default, and the service checks the receiver
certificate. It presents no client certificate, so the receiver must not require
one: a receiver that does rejects the connection with a TLS alert. With
`--plaintext`, the connection is plaintext with no peer authentication.

An example receiver for DNS-collector 3.2.3 listens for dnstap and writes text to
the console. Replace the placeholders with your collector's address:

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

The service accepts control payloads up to 512 bytes and content-type fields up
to 256 bytes. A normal dnstap ACCEPT is well within those limits.

**Transport security.** The example above is a plaintext listener, so the
service needs `--plaintext` to reach it. Plaintext is suitable only for a
trusted lab or an independently protected transport. A Hyper-V Default Switch
is reachable by the host and by other connected guests, so it is not a
confidentiality boundary. Restrict the listener to the intended guest IP, and
restrict its firewall to the Windows DNS guest. Do not expose the listener to an
untrusted network. On any other network, keep the TLS default and give the
receiver a certificate. See the
[receiver certificate step](#receiver-certificate) and
[SECURITY.md](../../SECURITY.md#threat-model-summary).

To make the example listener accept TLS, add these keys to its `dnstap` stanza.
Use a certificate that meets the requirements in the
[receiver certificate step](#receiver-certificate). DNS-collector 3.2.3 reads
`tls-support`, `cert-file`, and `key-file`, and its `tls-min-version` defaults to
`1.2`, which the service meets.

```yaml
      tls-support: true
      cert-file: "<path to the receiver certificate>"
      key-file: "<path to the receiver private key>"
```

## Receiver certificate

The service connects with TLS and checks the receiver certificate against the
Windows machine certificate store. Skip this step only if the binary path
contains `--plaintext`. The connection fails if any of these conditions is not
met:

- The receiver certificate chains to a CA in `Cert:\LocalMachine\Root`. The
  service runs as LocalService and cannot see a user store. A CA in
  `Cert:\CurrentUser\Root` does not count.
- The certificate carries the receiver IP as an `iPAddress` SAN, or it carries
  the name that you pass to `--tls-server-name`. When the server name is an IP
  address (the default, or an IP literal), the certificate should carry an
  `iPAddress` SAN for that address. The Windows platform verifier does the name
  matching. Whether it also accepts the address in a `dNSName` SAN or falls back
  to the common name is not verified.

Revocation endpoints do not need to be reachable. The service checks revocation
for the receiver certificate only and proceeds when the status cannot be
obtained. A reachable endpoint that answers slowly can push the handshake past
its 2-second deadline. See [TLS](configuration.md#tls).

To import a private CA into the machine root store:

```powershell
Import-Certificate -FilePath '.\receiver-ca.cer' -CertStoreLocation Cert:\LocalMachine\Root
```

A certificate that the service rejects stops the service with Event ID 1 when
it fails the first connection at startup. A receiver that answers the TLS
handshake with plaintext has the same effect. If the receiver is down at
startup, the first connection that reaches the handshake and fails is a counted
failed reconnect, and the service keeps retrying. The service logs one error
(Event ID 4) that tells you to check the Windows trust store, the server name,
and that the receiver does not require a client certificate. A receiver that
stays silent or
closes the connection during the handshake is an outage, and the service
retries. See [TLS](configuration.md#tls).


## Next steps

- [Install and provision the service](install.md)
- [TLS options](configuration.md#tls)
- [Troubleshooting a receiver outage](troubleshooting.md#the-receiver-is-down-or-unreachable)
