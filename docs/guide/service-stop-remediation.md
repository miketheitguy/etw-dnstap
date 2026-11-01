# Diagnose and remediate receiver shutdown resets

> **Status:** Current (operator guide) · **Evidence dated:** the recorded failure is undated in text · **Commit:** not recorded · **Reviewed:** 2026-11-01

This operator guide explains the intermittent Windows service-stop failure
seen during the Frame Streams STOP/FINISH exchange and how to validate a
receiver-side mitigation. It is for maintainers of the documented DNS-collector
lab receiver. It does not establish that the mitigation fixes the particular
failed connection.

## What the evidence shows

The recorded failure reports Windows socket error 10054 during the STOP/FINISH
exchange. The available error was coalesced across the STOP write or flush and
the FINISH read, so the exact failing phase is unknown. The sender's shutdown
path has a shared two-second I/O deadline and requires a valid FINISH response.
A failure at this stage ends service stop with an error; it is reported
separately from per-record output failures.

The strongest receiver-side candidate in the inspected sources is
DNS-collector's reset close. The pinned DNS-collector release is 3.2.3 at
commit [`3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03`](https://github.com/dmachard/DNS-collector/tree/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03).
It defaults `reset-conn` to true and passes that option to its connection close
helper. The pinned `go-netutils` v1.9.1 implementation sets zero linger for a
reset close; Go documents that zero linger can discard unsent or unacknowledged
data. The pinned `go-framestream` v1.8.0 sends FINISH in response to STOP.
Together, these sources make an aggressive close after FINISH a plausible
cause of an intermittent reset if FINISH has not reached the sender.

This mechanism is not proven for the failed connection: no packet capture
shows its FINISH and TCP close ordering or identifies pending bytes. A collector
log that says “framestream reset by sender” is not by itself proof of a TCP
reset; the pinned handler uses that wording when STOP handling returns EOF.
See the [pooled VM trace analysis](../performance/pooled-vm-trace-analysis.md#finish-only-lifecycle-failure-at-56k)
and its pinned source links for the evidence and limits.

## Configure a graceful close

Set `reset-conn: false` under the existing `pipelines[].dnstap` stanza. The
[README receiver example](../../README.md) shows this setting. Keep the existing
plain TCP console pipeline and its output mode; this setting changes receiver
connection close behavior only.

The bounded comparison applied this setting to the Ubuntu receiver and
retained it after validation. Its configuration SHA-256 is
`287c45827a6dfd6471732260c829b51af341c934e75104f1a221c7f5b2a289a8`.
The original configuration is preserved. Captures show a FIN exchange instead
of receiver resets, but both comparison arms stopped successfully, so the
earlier intermittent failure remains unreproduced. See the
[validation results](../history/service-stop-validation-results.md) for the evidence and
the separate collector-record deficits.

## Validate after testing resumes

Do not restart load testing until the test owner resumes it. Before changing
the receiver, save the active configuration and record its checksum. When
authorized, compare the current receiver policy with explicit
`reset-conn: false` using the same binaries and the exact existing workload
that reproduced the stop failure: `parser-filtered-auth-80000-60s-1`, the
80,000-QPS, 60-second filtered authoritative DNS workload. Follow the
[filtered ramp plan](../plans/filtered-ramp-plan.md) when resuming that campaign; do not
infer results from a different workload. Preserve the existing console output
pipeline and mode.

For each run, capture the TCP STOP, FINISH, and connection-close sequence,
including any RST. Record whether service stop succeeds and keep protocol
lifecycle results separate from record counts. Reconcile the same receiver and
sender counts used by the existing workload so a lifecycle pass does not hide
record loss, and a complete record count does not hide a failed FINISH.

If the receiver change causes a regression, restore the saved receiver
configuration, verify that its checksum matches the recorded value, and apply
the restored configuration using the same receiver procedure. Keep the saved
configuration and original output settings intact through the comparison.
Do not change sender behavior to ignore FINISH, add automatic resend, or extend
the shutdown timer as part of this mitigation. A passing local test or a clean
record count alone does not prove the live shutdown issue is resolved.

## Source versions

The source inspection is pinned to these versions:

- [DNS-collector v3.2.3 configuration defaults](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/pkg/config/collectors.go)
  and [connection handler](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/workers/dnstapserver.go).
- [`go-netutils` v1.9.1 close helper](https://github.com/dmachard/go-netutils/blob/1ac8345ffa557073c4cc53a84050afddac4fd20b/conn.go).
- [`go-framestream` v1.8.0 STOP/FINISH handling](https://github.com/dmachard/go-framestream/blob/0326694be1a73a70d127d8c1530beb677d101fa2/framestream.go).
- [Go `TCPConn.SetLinger` documentation](https://pkg.go.dev/net#TCPConn.SetLinger).
