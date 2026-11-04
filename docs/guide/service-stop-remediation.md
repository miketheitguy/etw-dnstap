# Diagnose and remediate receiver shutdown resets

> **Status:** Current (operator guide) · **Evidence dated:** the recorded failure is undated in text · **Commit:** not recorded · **Reviewed:** 2026-11-03

This guide is for maintainers of the documented DNS-collector lab receiver. It covers an
intermittent Windows service stop failure during the Frame Streams STOP/FINISH exchange,
and how to test a receiver-side change. A test does not prove that the change fixes the
recorded failed connection.

## What the evidence shows

The recorded failure reports Windows socket error 10054 during STOP/FINISH. The report
combines the STOP write or flush with the FINISH read, so the failed phase is unknown. A
current build names the phase: `final buffered data flush`, `STOP write/flush`,
`FINISH header read`, `FINISH body read`, or `FINISH validation`.

The sender's shutdown has a shared two-second I/O deadline. A STOP/FINISH exchange that does
not complete does not fail the service stop, and the service writes nothing about it. Frames
that were not sent stay in the spool for the next start, or count in `disconnected_dropped`
when no spool is in use. Earlier builds ended the stop with a
service-specific error instead, and SCM then restarted the service.

The strongest receiver-side candidate is DNS-collector's reset close:

- Pinned DNS-collector 3.2.3, at commit
  [`3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03`](https://github.com/dmachard/DNS-collector/tree/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03),
  sets `reset-conn` to true by default and passes it to its connection close helper.
- Pinned `go-netutils` v1.9.1 sets zero linger for a reset close. The Go documentation says
  zero linger can discard unsent or unacknowledged data.
- Pinned `go-framestream` v1.8.0 sends FINISH in response to STOP.

Together, these make one cause plausible: an aggressive close after FINISH can cause an
intermittent reset if FINISH has not reached the sender.

The mechanism is not proven for the failed connection. No capture shows the FINISH and
close order. The log line "framestream reset by sender" is not proof of a TCP reset. The
pinned handler writes it when STOP handling returns EOF. See the
[pooled VM trace analysis](../performance/pooled-vm-trace-analysis.md#finish-only-lifecycle-failure-at-56k)
for the evidence and its limits.

## Configure a graceful close

Set `reset-conn: false` under the existing `pipelines[].dnstap` stanza, as in the
[receiver example](receiver.md#receiver-configuration). Keep the console pipeline and its
output mode. This changes only how the receiver closes the connection.

The bounded comparison applied this setting to the Ubuntu receiver and kept it. Its
configuration SHA-256 is `287c45827a6dfd6471732260c829b51af341c934e75104f1a221c7f5b2a289a8`.
Captures show a FIN exchange instead of resets.
Both arms stopped successfully, so the intermittent failure is not reproduced. See the
[validation results](../history/service-stop-validation-results.md) for the evidence and the
separate collector-record deficits.

## Test the change when load testing resumes

Do not restart load testing until the test owner resumes it. When the owner authorizes a
comparison, follow these steps:

1. Save the active receiver configuration. Record its checksum.
2. Regenerate the [filtered ramp plan](../plans/filtered-ramp-plan.md) from the commit that
   you deploy. Do not run the plan as written.
3. Use the same binaries and the workload that reproduced the failure,
   [`parser-filtered-auth-80000-60s-1`](../history/guide-lab-results-2026-11.md#stop-failure-workload-parser-filtered-auth-80000-60s-1).
   Do not infer results from another workload.
4. Keep the existing console output pipeline and its mode.
5. Compare the current receiver policy with an explicit `reset-conn: false`.
6. For each run, capture the TCP STOP, FINISH, and close sequence, including any RST.
7. For each run, record from the receiver log or a packet capture whether the receiver got
   STOP and sent FINISH. The service stop succeeds either way, so its exit code does not show
   a failed FINISH. Keep lifecycle results separate from record counts.
8. Reconcile the receiver and sender counts. A lifecycle pass must not hide record loss,
   and a complete record count must not hide a failed FINISH.

If the change causes a regression, follow these steps:

1. Restore the saved receiver configuration.
2. Check that its checksum matches the recorded value.
3. Apply the restored configuration with the same receiver procedure.

Do not change the sender to add automatic resend or to extend the shutdown timer. A passing local test or a clean record count does not prove that the live problem is
solved.

## Source versions

The source inspection is pinned to these versions:

- [DNS-collector v3.2.3 configuration defaults](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/pkg/config/collectors.go)
  and [connection handler](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/workers/dnstapserver.go).
- [`go-netutils` v1.9.1 close helper](https://github.com/dmachard/go-netutils/blob/1ac8345ffa557073c4cc53a84050afddac4fd20b/conn.go).
- [`go-framestream` v1.8.0 STOP/FINISH handling](https://github.com/dmachard/go-framestream/blob/0326694be1a73a70d127d8c1530beb677d101fa2/framestream.go).
- [Go `TCPConn.SetLinger` documentation](https://pkg.go.dev/net#TCPConn.SetLinger).
