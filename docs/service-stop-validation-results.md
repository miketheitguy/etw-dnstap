# Service shutdown validation results

This report helps maintainers assess the bounded receiver-close comparison in
campaign `service-stop-20261030-0633`. Both arms stop cleanly. The baseline
capture shows receiver resets after FINISH; the graceful arm shows a FIN close
without resets. The earlier intermittent failure was not reproduced, so this
comparison does not establish that it is fixed. Normal operation is restored.
It reports protocol termination separately from DNS record reconciliation.

## Scope and artifact identity

The [validation plan](service-stop-validation-plan.md) authorizes two actual
load runs: authoritative DNS at a requested 80,000 queries per second (QPS)
for 60 seconds, first with inherited `reset-conn: true`, then with explicit
`reset-conn: false`. Both arms use the `client_only` filter, 5,000 names, two
client threads, 256 outstanding requests per thread, and a 2,000-millisecond
timeout. The broad stress ramp remains stopped.

The release executable SHA-256 is
`BB53558822C0A688248C918910CFFF4A4BFBFE0C639CA3DFE8249CFD6CADC8AD`.
The [build receipt](../target/performance/service-stop-20261030-0633/build/receipt.json)
identifies a build from the dirty working tree, without a corresponding Git
commit. Both stage receipts bind to this executable hash. The
[matched-arm receipt](../target/performance/service-stop-20261030-0633/windows/matched-arms.json)
verifies the same candidate, client binary, names file, and workload options.

The receiver configuration identities for the comparison are:

| Arm | Policy | Configuration SHA-256 |
| --- | --- | --- |
| Baseline | Inherited `reset-conn: true` | `5d71afe6f46d8f81802ae592a97c8be2582891b52c5624405c6ef64120065935` |
| Graceful | Explicit `reset-conn: false` | `287c45827a6dfd6471732260c829b51af341c934e75104f1a221c7f5b2a289a8` |

The original setup, `stop-reset-auth-80000-60s-1`, declared `all_supported`
instead of `client_only`. The guard rejected it before service or client
startup. Its receipts and empty captures remain preserved as
`setup_rejected_no_load`; it is not a third load run. The actual baseline is
`stop-reset-auth-80000-60s-2`.

## Run results

The recorded outcomes distinguish DNS delivery, service stop, and capture
reconciliation:

| Observation | Baseline `stop-reset-auth-80000-60s-2` | Graceful `stop-graceful-auth-80000-60s-1` |
| --- | --- | --- |
| Load status | Complete | Complete |
| Client sent / completed / successful | 4,699,730 / 4,699,730 / 4,699,730 | 4,693,651 / 4,693,651 / 4,693,651 |
| Achieved sending rate | 78,328.8333333 QPS | 78,227.5166667 QPS |
| Client anomalies | None; zero final outstanding requests | None; zero final outstanding requests |
| Service Control Manager (SCM) exit / service-specific exit | 0 / 0; stopped | 0 / 0; stopped |
| Stage-scoped service Event Log entries | None | None |
| Collector CLIENT_QUERY | 4,449,581 | 4,300,014 |
| Collector CLIENT_RESPONSE | 4,450,097 | 4,300,931 |
| Sent minus CLIENT_QUERY | 250,149 | 393,637 |
| Completed minus CLIENT_RESPONSE | 249,633 | 392,720 |
| Collector RESOLVER_QUERY / RESOLVER_RESPONSE | 0 / 0 | 0 / 0 |
| Receiver UDP error-counter deltas | Zero | Zero |
| Directional record reconciliation | Fails exact-count reconciliation | Fails exact-count reconciliation |

Both client summaries record a 60-second measured sending interval.
All completed responses are successful. The service-stop receipt records
`Stopped`, process ID zero, and both exit codes zero in each arm. Both
stage-scoped Event Log receipts contain no events between their boundaries.

The baseline receiver ledger counts 8,899,678 selected-zone records with no malformed
DNS records, malformed journal lines, unrelated JSON records, or unknown
operations. The receiver stopped after the final drain, and its ledger
records stable publication, archive identity, and host transfer proof.
Its raw capture SHA-256 is
`8d449f15cd858cbfe434a4c08010b9fd43b45930401df2603c281915588e301f`.

The graceful receiver ledger counts 8,600,945 selected-zone records with the
same zero malformed, unrelated, and unknown-operation categories. Each arm
records 39 diagnostic journal lines. The graceful raw capture contains
8,345,087,710 bytes with SHA-256
`2f4e6182104fa39841f6d9724a10870ac309985aa8df25e45b22a40da93307e7`.
Its ledger also records stable publication after final drain, archive identity,
and host transfer proof.

The clean client path supports comparing directional counts, but the
residuals do not locate the missing records. These observations do not assign
loss to ETW, queue admission, TCP, or collector processing. Zero receiver UDP
error counters do not establish complete dnstap capture. This comparison
provides no CPU, performance-cause, or ring-capacity conclusion. The graceful
arm has larger query and response residuals. These two runs do not establish
that the close policy caused that difference.

## Baseline TCP close evidence

The complete retained receiver-to-client payload stream contains a
framing-proven 42-byte ACCEPT followed by a 12-byte FINISH. The capture
records this close sequence on October 30, 2026, in UTC:

| Time | Direction | Observation |
| --- | --- | --- |
| `06:55:10.103945` | Receiver to Windows | FINISH; sequence range `[4174615338, 4174615350)` |
| `06:55:10.104039` | Receiver to Windows | RST/ACK, 94 microseconds after FINISH |
| `06:55:10.104159` | Windows to receiver | FIN/ACK with acknowledgment `4174615350`, covering all FINISH bytes, 214 microseconds after FINISH |
| `06:55:10.104176` | Receiver to Windows | Second RST, 17 microseconds after the client FIN/ACK |

The client acknowledgment establishes receipt by peer TCP, not application
processing. The clean SCM stop is separate evidence that this service run
completed shutdown successfully despite the receiver reset sequence.

A 12-byte STOP-shaped packet appears at `06:55:10.103801` and is acknowledged
before FINISH. However, circular capture rotation omitted 865,342,997 initial
client payload bytes. The packet's byte pattern and position cannot establish
Frame Streams framing from the connection start; STOP remains an unverified
candidate.

Both tcpdump capture sets report zero kernel drops. The analyzer reports zero
truncated packet records, snap-length truncations, and conflicting overlaps.
The circular tail retains 12,616 packet records out of 224,665 captured, so
zero kernel drops does not imply a complete client stream. All five retained
packet-capture file hashes match their receipts.

This baseline demonstrates that receiver resets can follow FINISH while the
service reports success. It does not explain the earlier failing connection.
Because the baseline already passes the service-stop gate, a successful
graceful arm cannot by itself prove that the intermittent failure is fixed.
The graceful close capture establishes a different observed close sequence.

## Graceful TCP close evidence

The complete retained receiver payload stream again contains a framing-proven
ACCEPT followed by FINISH. The capture records the following UTC sequence on
October 30, 2026:

| Time | Direction | Observation |
| --- | --- | --- |
| `07:02:40.437770` | Receiver to Windows | FINISH; sequence range `[1146649020, 1146649032)` |
| `07:02:40.437876` | Receiver to Windows | FIN/ACK, 106 microseconds after FINISH |
| `07:02:40.438034` | Windows to receiver | FIN/ACK with acknowledgment `1146649032`, covering all FINISH bytes, 264 microseconds after FINISH |

Both FINs are acknowledged. No RST appears in the retained tail or complete
SYN/FIN/RST capture. The observed receiver close after FINISH changes from RST
in the baseline to FIN in the graceful arm; both service stops succeed.

The client stream omits 833,897,724 initial payload bytes through circular
rotation, so its STOP-shaped packet remains an unverified framing candidate.
The tail retains 12,809 of 217,180 captured packets. Both capture sets report
zero kernel drops; the analyzer reports zero packet truncations and conflicting
overlaps. All five retained packet-capture file hashes match their receipts.

## Evidence locations

The local [campaign directory](../target/performance/service-stop-20261030-0633/)
contains the source receipts. These ignored `target/` artifacts are not part
of a fresh checkout. The following paths exist under each arm's directory in
`stages/`:

| Evidence | Relative path |
| --- | --- |
| Client options and counts | `client/summary.json` |
| SCM stop result | `service-stop.json` |
| Scoped Event Log result | `stage-eventlog.json` |
| Directional counts and integrity references | `receiver/reconciliation-v2.json` |
| Receiver archive manifest | `receiver/archive/archive-manifest.json` |
| Host transfer proof | `receiver/transfer-proof.json` |
| TCP stream analysis and capture limits | `pcap-analysis.json` |
| Packet control evidence, hash checks, and tcpdump statistics | `pcap-control-proof.json` |

## Final state and review

The Windows service is running with the reviewed executable, original
arguments, ACLs, service SID, privilege allowlist, and DNS settings. The
[Windows restoration receipt](../target/performance/service-stop-20261030-0633/windows/cleanup-Restore-20261030T070837242Z.json)
records PID 3764 and exit code zero. The final no-load readback records no
owned stress client or stage helper. Both lab VMs remain running.

The collector is running with `reset-conn: false`, configuration hash
`287c45827a6dfd6471732260c829b51af341c934e75104f1a221c7f5b2a289a8`,
and normal `journal+console` output. Its executable is unchanged. The
[receiver restoration receipt](../target/performance/service-stop-20261030-0633/ubuntu/normal-receiver-restored.json)
records removal of the temporary output override and no remaining owned
packet-capture processes.

The existing data disk was unmounted and detached from Ubuntu SCSI slot 0:2.
The [disk receipt](../target/performance/service-stop-20261030-0633/ubuntu/raw-vhd-detach-receipt.json)
records the retained disk identifier, original path, and 64 GiB virtual
capacity. Its dynamic file length decreased from 55,838,769,152 to
47,248,834,560 bytes after discard of the two verified new raw captures.
Earlier campaign files were not targeted. Host raw files, compressed archives,
packet captures, and the rejected setup evidence remain preserved.

Sol at high effort reviewed the operational helpers and results. The two
helper findings were corrected: interrupted configuration changes can be
rolled back using the verified original and exact derived changed bytes, and
the packet filter uses the validated collector address. Re-review found no
remaining actionable findings. Ten offline helper fixtures passed; their
POSIX, account, and YAML stubs are documented in the
[local validation receipt](../target/performance/service-stop-20261030-0633/local-helper-review/results.json).
Those fixtures do not establish live rollback behavior. No additional load
was run after the two comparison arms.
