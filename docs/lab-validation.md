# DNS service to DNS-collector lab validation

This historical record helps maintainers assess initial live validation of the
Service Control Manager (SCM)-only Windows service against
DNS-collector 3.2.3 in two Hyper-V guests. It separates verified guest readiness
from live packet-delivery results. The session date is October 28, 2026 in
America/Los_Angeles; evidence timestamps after midnight use October 29 UTC.

The binary hashes, test counts, PIDs, and guest state in this record describe
that session's cutoff. Later builds and performance work are recorded in the
[documentation index](README.md#review-earlier-decisions-and-validation) and
[completed pooled VM review](pooled-vm-performance-review.md). These small probes
establish the listed payload and lifecycle observations, not load capacity.

## Verified environment

The following guest state is recorded in local provisioning evidence:

| Component | Observed state |
| --- | --- |
| Windows VM | `DNSTap-WS2025`, Windows Server 2025 Standard Evaluation, build 26100 |
| Windows address | `172.27.215.163/20` |
| DNS Server role | Installed successfully, no restart required; `DNS` service Running |
| Lab DNS zone | Primary zone `example.test` |
| Ubuntu VM | `DNSTap-Ubuntu2604`, Ubuntu 26.04.1 LTS, kernel `7.0.0-38-generic` |
| Ubuntu address | `172.27.215.60/20` |
| Receiver | `172.27.215.60:6000`, dnstap over plaintext TCP |
| Collector | DNS-collector 3.2.3, revision `3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03` |
| Collector output | JSON on stdout, retained by the systemd journal and also sent to the console |
| Content type | `protobuf:dnstap.Dnstap` |

Both guests use the existing Hyper-V Default Switch. The Ubuntu listener binds
only its guest IP. A dedicated persistent nftables rule drops traffic to that
IP on TCP port 6000 unless the source is the Windows guest address
`172.27.215.163`. The filter service starts before the collector. This source
filter does not provide cryptographic authentication or confidentiality, and
the Default Switch is reachable by the host and other connected guests.

The receiver uses the official
[DNS-collector v3.2.3 release](https://github.com/dmachard/DNS-collector/releases/tag/v3.2.3).
Its archive SHA-256 matches both the publisher's checksum file and the GitHub
release asset digest:

```text
Archive: 2c989d7f35de3511175028d961f25dcaca85c841324a5d7dbe4aca30225a3c4d
Binary:  fea6c2f23d27d51e281de2f84aa56a014067e2cce1ccd53200ed894fae05e406
```

No TLS certificate is used because this validation deliberately uses plaintext
within the agreed lab. The client implementation does not implement TLS.

## Build and local checks

The client is built for `x86_64-pc-windows-msvc` with the pinned
`nightly-2026-10-01` toolchain and the checked-in target hardening settings.
The network-client build completed these local checks before Application Event
Log failure reporting was added:

- `cargo fmt --check` and `cargo check --locked` passed.
- `cargo test --locked` passed all 29 tests.
- `cargo clippy --locked --all-targets -- -D warnings` passed.
- `cargo asan` passed all 29 tests after the selected trusted MSVC x64 ASan
  runtime directory was added to PATH for that process only.
- The ordinary release build passed. PE inspection found x64 architecture,
  base relocations, large-address support, ASLR, high-entropy ASLR, NX, CFG
  tables, a security cookie, EH continuation metadata, and CET compatibility.
- The release has no ASan runtime import or PDB/CodeView reference.
- `--help` exited 0; an invalid receiver exited 2. A valid receiver argument
  outside SCM exited 1 with Windows error 1063 before capture or a connection.

ASan covers instrumented Rust code, dependencies, and the rebuilt Rust standard
library. Windows system DLLs and prebuilt CRT code remain outside that scope,
including prebuilt CRT bytes statically linked into the executable. CRT
servicing requires a rebuilt and redeployed executable.
The project introduces no C/C++ source compilation, `cc`/CMake dependency build
steps, or TLS library dependency. Windows APIs and the platform runtime remain
native dependencies. PE metadata does not prove runtime enforcement of CET or
complete per-function stack protection.

The initial static-CRT release used for the client and lifecycle tests has
SHA-256:

```text
9123AB217385E25AB4CAD3210D5C7C3AAD308FE98EE4964DFCC73CDDBB73AD68
```

Independent inspection of this PE found x64 architecture, relocations,
large-address support, ASLR, high-entropy ASLR, NX, 771 CFG targets, a security
cookie, 11 EH continuation entries, and CET compatibility. It has no PDB or
CodeView reference, no ASan runtime import, and no VCRUNTIME, UCRT, or MSVCP DLL
imports. Its remaining imported DLLs are Windows system components.

The final corrected static-CRT build passed all 30 ASan tests. That rerun
used the trusted MSVC x64 ASan runtime directory on PATH for that task only;
`target/asan-static-crt-test.log` records the result.

The deployment profile requires LocalSystem, an enabled unrestricted
`NT SERVICE\EtwDnstap` SID, and SCM RequiredPrivileges containing only
`SeChangeNotifyPrivilege`. The Application Event Log source `EtwDnstap`
must be registered before starting the service. The program checks the effective token
before opening the receiver or output. Guest installation and live token
acceptance are recorded in [Live results](#live-results) rather than inferred from unit
tests. The observed service SID type is 1, RequiredPrivileges contains only
`SeChangeNotifyPrivilege`, and the quoted service binary path selects
`--receiver 172.27.215.60:6000 --session-name EtwDnstap-Lab`. The SCM ACL permits
SYSTEM and administrators to control/reconfigure the service; authenticated
users have query/read access. The installed executable and its directory grant
full access only to SYSTEM and administrators.

## Live results

Receiver readiness is verified: systemd reports `dnscollector.service` active,
the process reports version 3.2.3, and the listener is bound to
`172.27.215.60:6000`. Initial collector logs show zero restarts and the dnstap
pipeline routed to the JSON stdout worker.

The first clean Server Core launch exposed a missing `VCRUNTIME140.dll` and
failed before `main` with loader status `0xC0000135`. This was a runtime loading
failure, not a service-token rejection. The final production build statically
links the CRT, and its import scan confirms that the missing redistributable
DLL dependency is removed.

The initial static-CRT executable started through SCM and remained `Running` while the
known queries completed. Its strict service-token checks accepted the actual
guest token. The collector accepted the connection from
`172.27.215.163:49839` at `2026-10-29T04:18:40.826377Z` and initialized the
Frame Streams receiver.

Three batches of four known DNS requests produced 24 decoded collector records:
one `CLIENT_QUERY` and one `CLIENT_RESPONSE` per request. The repeat batch
has these results at `2026-10-29T04:20:27Z`:

| DNS ID | Request | Client transport/address | Client port | Decoded response |
| --- | --- | --- | --- | --- |
| `0x4d21` | `probe.example.test A` | IPv4 UDP, `172.27.215.163` | 61934 | `NOERROR`, `192.0.2.53` |
| `0x4d22` | `probe.example.test AAAA` | IPv4 TCP, `172.27.215.163` | 49841 | `NOERROR`, `2001:db8::53` |
| `0x4d23` | `missing.example.test A` | IPv4 UDP, `172.27.215.163` | 61935 | `NXDOMAIN`, zone SOA |
| `0x4d24` | `probe.example.test A` | IPv6 UDP, `::1` | 61936 | `NOERROR`, `192.0.2.53` |

All 24 collector records have `malformed-packet: false`. The repeat batch's
provider timestamps range from `2026-10-29T04:20:27.0803663Z` through
`2026-10-29T04:20:27.1205313Z`. The collector exposes the expected family,
protocol, query and response addresses, DNS ID, client port, question, answer,
and operation. The sender omits the DNS server port, so `response-port` is
`-`; complete endpoint-port metadata is not claimed.

A scoped receiver-side packet capture independently verifies wire preservation.
The verifier reassembles TCP data, parses Frame Streams and the dnstap protobuf,
and compares the DNS payload with the exact request and response hex saved by
the Windows probe. All 16 payloads from the repeat and post-restart batches
match byte for byte, keyed by DNS ID, operation, transport, and client port. This confirms both query and
response bytes for the listed UDP, TCP, IPv4, IPv6, and NXDOMAIN cases. The
collector's JSON summary alone is not used as proof of unchanged packet bytes.

The receiver filter also passed a negative host-source test: a host connection
to TCP 6000 timed out after one second, and its dedicated nftables counter
recorded two dropped packets. SSH remained available and the collector stayed
active. This check verifies the source restriction, not sender authentication.

The controlled SCM stop completed in 473 ms with Win32 and service-specific
exit codes both zero. The owned `EtwDnstap-Lab` ETW session was absent after
stop. The receiver capture contains the sender's `STOP` control frame and the
collector's `FINISH` response. The collector describes this normal close as
`framestream reset by sender` and cleans up the connection.

A negative startup test added `SeDebugPrivilege` to the SCM required privilege
list. Startup correctly failed with Win32 1066, service-specific code 1, and
Application Event ID 1 naming the required configuration. The test restored
`SeChangeNotifyPrivilege` as the only required privilege and started the
service successfully as PID 1996. The receiver accepted a new connection from
`172.27.215.163:49842` at `2026-10-29T04:22:04.211485Z`; the capture confirms
`READY`, `ACCEPT`, and `START` control frames.

The post-restart batch at `2026-10-29T04:22:27Z` delivered the same four cases
with client ports 61937, 49843, 61938, and 61939. All eight additional query and
response payloads match the saved probe bytes. The collector remained running
through the lifecycle test with `NRestarts=0` and the final sender connection
established. The temporary packet capture was stopped after exporting evidence;
the Windows sender, Ubuntu collector, and both VMs remain running.

Across the three batches, the JSON console output contains 12 client queries
and 12 client responses: 18 IPv4 and 6 IPv6 records, 18 UDP and 6 TCP records,
9 successful responses, and 3 NXDOMAIN responses. There are no malformed
records or missing expected messages in these probes. The receiver-side
capture reports 58 packets and zero kernel capture drops. Those capture
statistics do not measure ETW queue loss or establish zero loss under load.

## Resolver direction and unavailable metadata

A separate conditional-forwarder test exercised resolver events 260 and 261.
A temporary Ubuntu UDP responder bound only `172.27.215.60:53`, accepted only
the Windows guest, and answered `probe.upstream.test A` with `192.0.2.123`,
TTL 60. The Windows guest forwarded only the owned `upstream.test` test zone;
the conditional forwarder was removed after the query. No public upstream
DNS server was involved.

At `2026-10-29T04:28:21Z`, the collector decoded four additional records:
`CLIENT_QUERY`, `RESOLVER_QUERY`, `RESOLVER_RESPONSE`, and `CLIENT_RESPONSE`.
The client DNS ID was `0x4d31`; Windows used upstream DNS ID `0x4aad`. The
resolver query includes EDNS and is 48 bytes; its response is 53 bytes. Both
resolver payloads match the actual UDP packets and responder hex byte for byte.
All four decoded records have `malformed-packet: false`.

The UDP capture independently shows the actual upstream endpoints as
`172.27.215.163:57320` to `172.27.215.60:53`. The provider's endpoint metadata
does not expose those ports in this case. For both resolver messages, the
captured dnstap protobuf contains query address tag 4 as `0.0.0.0`, response
address tag 5 as `172.27.215.60`, no query port tag 6, and response port tag 7
as zero. The collector displays both ports as `-`. This confirms zero and an
unspecified local interface in the captured metadata; it does not justify
substituting the independently observed wire ports into the provider record.

The unavailable values prompted a correction that normalizes zero ports and
unspecified IPv4/IPv6 addresses to absent optional fields. The final deployed
release has SHA-256:

```text
2487FFEFA54D4F6FC2665695E564F6F2B0F254A6C2FA67D8443F4B97AC04E792
```

The corrected build passed all 30 unit tests, all 30 ASan tests, formatting,
strict Clippy checks, and the release build. The final ASan result is recorded
in `target/asan-static-crt-test.log`; actual compiler flags and the trusted
signed MSVC runtime identity are in
`target/lab-windows/build-verification.json`. A fresh
`final.upstream.test A` probe at `2026-10-29T04:31:11Z` verified the deployed
binary as PID 2092. It produced four decoded client/resolver records with no
malformed packets. The client DNS ID is `0x4d32`; the upstream ID is `0x5f35`.
The resolver query is 37 bytes and its response is 53 bytes. Both DNS payloads
match the captured UDP packets and responder hex byte for byte.

For both final resolver records, raw protobuf query address tag 4, query port
tag 6, and response port tag 7 are absent; response address tag 5 retains the
known peer `172.27.215.60`. The actual wire endpoints are independently
`172.27.215.163:57341` to `172.27.215.60:53`. Those values are not fabricated
into the ETW-derived record. The collector correctly displays unavailable
fields as `-`. Final evidence is in `resolver-final-byte-proof.json`,
`collector-resolver-final.jsonl`, `upstream-dns-final.jsonl`, and
`dnstap-resolver-final.pcap` under `target/lab-ubuntu/`; the Windows evidence is
`target/lab-windows/resolver-final-query.json`.
The temporary responder and capture were stopped and their dedicated UDP53
filter table removed. The guest no longer listens on `172.27.215.60:53`;
its original loopback DNS stub listeners and the running dnstap collector
remain intact. Final checks show the collector active with `NRestarts=0`,
connected to the corrected Windows sender at `172.27.215.163:49848`.
The Windows service remains `Running` as PID 2092 with the final hash recorded
in `target/lab-windows/final-state.json`. Only the permanent receiver filter
table `inet dnstap_lab` remains. Resolver evidence lives in `resolver-byte-proof.json`,
`collector-resolver.jsonl`, `upstream-dns.jsonl`, and
`dnstap-resolver-proof.pcap` under `target/lab-ubuntu/`. Windows query and
forwarder cleanup evidence is `target/lab-windows/resolver-query.json`.

## Evidence locations

Session evidence lives under ignored `target/` paths and is not committed to
the repository. This report retains the relevant nonsecret observations:

| Local evidence | Purpose |
| --- | --- |
| `target/lab-windows/guest-ready.json` | Windows OS, DNS role/service, address, and zone readiness |
| `target/lab-windows/provider-templates.json` | Live DNS provider event templates |
| `target/lab-ubuntu/provenance.json` | Ubuntu/collector versions, hashes, addresses, and receiver restriction |
| `target/lab-ubuntu/collector-config.yml` | Exact receiver-to-JSON pipeline |
| `target/lab-ubuntu/readiness.log` | Version, service, listener, and network checks |
| `target/lab-ubuntu/collector-pre-traffic.log` | Collector readiness before test traffic |
| `target/lab-windows/wire-queries.json` | Exact repeat-batch query/response bytes and client ports |
| `target/lab-windows/wire-queries-restart.json` | Exact post-restart probe bytes and client ports |
| `target/lab-windows/service-start.json` | Successful SCM startup evidence |
| `target/lab-windows/final-state.json` | Final corrected binary hash and Running PID 2092 |
| `target/lab-windows/build-verification.json` | Final build/ASan flags and trusted runtime identity |
| `target/lab-windows/service-lifecycle.json` | Clean stop, privilege rejection, and restored final service |
| `target/lab-ubuntu/collector-live.jsonl` | All 24 decoded console records |
| `target/lab-ubuntu/live-summary.json` | Parsed record counts and verification scope |
| `target/lab-ubuntu/final-receiver-status.log` | Final connection, service state, and capture drop statistics |
| `target/lab-ubuntu/lifecycle-byte-proof.json` | All 16 repeat/restart payload comparisons and control frames |
| `target/lab-ubuntu/dnstap-proof-final.pcap` | Receiver-side dnstap TCP capture, including lifecycle |
| `target/lab-ubuntu/verify_dnstap_capture.py` | Independent Frame Streams/protobuf verifier |
| `target/release-build.log` | Effective production compiler/linker invocations |
| `target/pe-inspection.txt` | PE headers, load configuration, and imports |
| `target/asan-test.log` | Initial instrumented local test results |
| `target/asan-static-crt-test.log` | Final corrected static-CRT binary, 30 ASan tests passed |

The live packet check does not establish production throughput, zero loss under
load, all provider event versions, authoritative/forwarder role classification,
or durable collector storage. See the [README](../README.md) for operational
limits and deployment prerequisites.
