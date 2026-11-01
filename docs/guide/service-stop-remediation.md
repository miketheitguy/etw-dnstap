# Diagnose and remediate receiver shutdown resets

> **Status:** Current (operator guide) · **Evidence dated:** the recorded failure is undated in text · **Commit:** not recorded · **Reviewed:** 2026-11-01

This guide is for maintainers of the documented DNS-collector lab receiver. It
explains an intermittent failure of the Windows service stop. The failure occurs
during the Frame Streams STOP/FINISH exchange. The guide also tells you how to
test a change on the receiver side. It does not prove that this change fixes the
recorded failed connection.

## What the evidence shows

The recorded failure reports Windows socket error 10054 during the STOP/FINISH
exchange. The error report combines the STOP write or flush with the FINISH
read. The exact phase that failed is therefore unknown.

The sender's shutdown path has a shared two-second I/O deadline. It also
requires a valid FINISH response. A failure at this stage ends the service stop
with an error. The service reports it separately from output failures for
individual records.

The strongest receiver-side candidate in the inspected sources is the reset
close of DNS-collector:

- The pinned DNS-collector release is 3.2.3 at commit
  [`3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03`](https://github.com/dmachard/DNS-collector/tree/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03).
  It sets `reset-conn` to true by default and passes that option to its
  connection close helper.
- The pinned `go-netutils` v1.9.1 sets zero linger for a reset close. The Go
  documentation says that zero linger can discard unsent or unacknowledged data.
- The pinned `go-framestream` v1.8.0 sends FINISH in response to STOP.

Together, these sources make one cause plausible. An aggressive close after
FINISH can cause an intermittent reset if FINISH has not reached the sender.

This mechanism is not proven for the failed connection. No packet capture shows
its FINISH and TCP close order, or identifies pending bytes. A collector log
line "framestream reset by sender" is not proof of a TCP reset by itself. The
pinned handler writes that text when STOP handling returns EOF. See the
[pooled VM trace analysis](../performance/pooled-vm-trace-analysis.md#finish-only-lifecycle-failure-at-56k)
and its pinned source links for the evidence and its limits.

## Configure a graceful close

Set `reset-conn: false` under the existing `pipelines[].dnstap` stanza. The
[receiver example in install.md](install.md#receiver-configuration) shows this
setting. Keep the existing plain TCP console pipeline and its output mode. This
setting changes only how the receiver closes the connection.

The bounded comparison applied this setting to the Ubuntu receiver and kept it
after validation. Its configuration SHA-256 is
`287c45827a6dfd6471732260c829b51af341c934e75104f1a221c7f5b2a289a8`.
The original configuration is preserved. Captures show a FIN exchange instead
of receiver resets. Both comparison arms stopped successfully, so the earlier
intermittent failure is still not reproduced. See the
[validation results](../history/service-stop-validation-results.md) for the
evidence and the separate collector-record deficits.

## Test the change when load testing resumes

Do not restart load testing until the test owner resumes it. When the owner
authorizes a comparison, do these steps:

1. Save the active receiver configuration. Record its checksum.
2. Regenerate the [filtered ramp plan](../plans/filtered-ramp-plan.md) from the
   commit that you deploy. Do not run the plan as written.
3. Use the same binaries and the exact workload that reproduced the stop
   failure: `parser-filtered-auth-80000-60s-1`. This is the 80,000-QPS,
   60-second filtered authoritative DNS workload. Do not infer results from a
   different workload.
4. Keep the existing console output pipeline and its mode.
5. Compare the current receiver policy with an explicit `reset-conn: false`.
6. For each run, capture the TCP STOP, FINISH, and connection-close sequence.
   Include any RST.
7. For each run, record whether the service stop succeeds. Keep the protocol
   lifecycle results separate from the record counts.
8. Reconcile the receiver and sender counts that the existing workload uses. A
   lifecycle pass must not hide record loss. A complete record count must not
   hide a failed FINISH.

If the receiver change causes a regression, do these steps:

1. Restore the saved receiver configuration.
2. Check that its checksum matches the recorded value.
3. Apply the restored configuration with the same receiver procedure.

Keep the saved configuration and the original output settings unchanged during
the comparison.

Do not change the sender as part of this mitigation. Specifically, do not make
it ignore FINISH, do not add automatic resend, and do not extend the shutdown
timer. A passing local test or a clean record count does not by itself prove
that the live shutdown problem is solved.

## Source versions

The source inspection is pinned to these versions:

- [DNS-collector v3.2.3 configuration defaults](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/pkg/config/collectors.go)
  and [connection handler](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/workers/dnstapserver.go).
- [`go-netutils` v1.9.1 close helper](https://github.com/dmachard/go-netutils/blob/1ac8345ffa557073c4cc53a84050afddac4fd20b/conn.go).
- [`go-framestream` v1.8.0 STOP/FINISH handling](https://github.com/dmachard/go-framestream/blob/0326694be1a73a70d127d8c1530beb677d101fa2/framestream.go).
- [Go `TCPConn.SetLinger` documentation](https://pkg.go.dev/net#TCPConn.SetLinger).
