# Set up the dnstap receiver

> **Status:** Current · **Describes:** `main` at commit `dde7188` with the TLS receiver change · **Reviewed:** 2026-11-02

Use this guide to prepare the collector that receives the records. Do this
before you start the service, although the service also reconnects if the
receiver starts later.

## Receiver configuration

The service keeps one outbound TCP connection to `--receiver`. It negotiates
content type `protobuf:dnstap.Dnstap` with READY, ACCEPT, and START, then sends
data frames. On shutdown it sends STOP and waits for FINISH. After a failure it
reconnects and negotiates again. Frames lost in the gap are counted, not resent.
The connection is TLS by default, and the service verifies the receiver
certificate. It sends no client certificate. With `--plaintext`, the connection
is plaintext with no peer authentication.

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

## Receiver certificate

The service connects with TLS and verifies the receiver certificate with the
Windows machine certificate store. Skip this step only if the binary path
contains `--plaintext`. The connection fails if any of these conditions is not
met:

- The receiver certificate chains to a CA in `Cert:\LocalMachine\Root`. The
  service runs as LocalService and cannot see a user store. A CA in
  `Cert:\CurrentUser\Root` does not count.
- The certificate carries the receiver IP as an `iPAddress` SAN, or it carries
  the name that you pass to `--tls-server-name`.

Revocation endpoints do not need to be reachable. The service checks revocation
for the receiver certificate only and proceeds when the status cannot be
obtained. A reachable endpoint that answers slowly can push the handshake past
its 2-second deadline; see [TLS](configuration.md#tls).

To import a private CA into the machine root store:

```powershell
Import-Certificate -FilePath '.\receiver-ca.cer' -CertStoreLocation Cert:\LocalMachine\Root
```

A certificate that does not verify stops the service at startup with Event ID 1.
A receiver that speaks plaintext while TLS is on has the same effect. See
[TLS](configuration.md#tls).


## Next steps

- [Install and provision the service](install.md)
- [TLS options](configuration.md#tls)
- [Troubleshooting a receiver outage](troubleshooting.md#the-receiver-is-down-or-unreachable)
