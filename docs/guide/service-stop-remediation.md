# Diagnose and remediate receiver shutdown resets

> **Status:** Current (operator guide) · **Evidence dated:** the recorded failure is undated in text · **Commit:** not recorded · **Reviewed:** 2026-11-05

### Quick Start

Under the existing `pipelines[].dnstap` stanza ([receiver example](receiver.md#receiver-configuration)), keep the console pipeline and its output mode, and set:

```yaml
reset-conn: false
```

The Ubuntu receiver in the bounded comparison runs it (configuration SHA-256 `287c45827a6dfd6471732260c829b51af341c934e75104f1a221c7f5b2a289a8`). Captures show a FIN exchange, not resets ([validation results](../history/service-stop-validation-results.md)).

### Steps / Configuration

#### What the evidence shows

- The recorded failure is Windows socket error 10054 during STOP/FINISH, phase not recorded. A current build names it: `final buffered data flush`, `STOP write/flush`, `FINISH header read`, `FINISH body read`, or `FINISH validation`.
- Shutdown I/O shares a two-second deadline. An incomplete STOP/FINISH exchange still stops the service cleanly and logs nothing. Unsent frames stay in the spool, or count in `disconnected_dropped` with no spool.
- Suspect: DNS-collector's reset close. Pinned DNS-collector 3.2.3 sets `reset-conn` to true by default and passes it to its connection close helper. Pinned `go-netutils` v1.9.1 sets zero linger for a reset close, which Go documents can discard unsent or unacknowledged data. Pinned `go-framestream` v1.8.0 sends FINISH in response to STOP. So an aggressive close after FINISH can reset the connection if FINISH hasn't reached the sender.

#### Test the change when load testing resumes

When the test owner authorizes load testing:

1. Save the active receiver configuration. Record its checksum.
2. Regenerate the [filtered ramp plan](../plans/filtered-ramp-plan.md) from the commit you deploy.
3. Use the same binaries, the same console output pipeline and mode, and the workload that reproduced the failure, [`parser-filtered-auth-80000-60s-1`](../history/guide-lab-results-2026-11.md#stop-failure-workload-parser-filtered-auth-80000-60s-1).
4. Compare the current receiver policy with an explicit `reset-conn: false`.
5. Per run, capture the TCP STOP, FINISH, and close sequence, including any RST.
6. Per run, record from the receiver log or a packet capture whether the receiver got STOP and sent FINISH. Keep lifecycle results separate from record counts.
7. Reconcile receiver and sender counts.

On a regression, restore the saved configuration, check its checksum against the recorded value, and apply it with the same receiver procedure.

Keep the sender as it is: no automatic resend, no longer shutdown timer.

#### Source versions

- DNS-collector v3.2.3, commit `3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03`: [tree](https://github.com/dmachard/DNS-collector/tree/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03), [configuration defaults](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/pkg/config/collectors.go), [connection handler](https://github.com/dmachard/DNS-collector/blob/3b5f3f77d75a0a47a5b8f1d189f05e22bf663a03/workers/dnstapserver.go).
- [`go-netutils` v1.9.1 close helper](https://github.com/dmachard/go-netutils/blob/1ac8345ffa557073c4cc53a84050afddac4fd20b/conn.go).
- [`go-framestream` v1.8.0 STOP/FINISH handling](https://github.com/dmachard/go-framestream/blob/0326694be1a73a70d127d8c1530beb677d101fa2/framestream.go).
- [Go `TCPConn.SetLinger`](https://pkg.go.dev/net#TCPConn.SetLinger).

### Common Gotchas

- **The mechanism is unproven.** No capture shows the FINISH and close order, and a passing test doesn't prove the fix.
- **"framestream reset by sender" isn't a TCP reset.** It's written when STOP handling returns EOF ([pooled VM trace analysis](../history/pooled-vm-trace-analysis.md#finish-only-lifecycle-failure-at-56k)).
- **The service exit code doesn't show a failed FINISH.** A lifecycle pass can hide record loss, and a complete record count can hide a failed FINISH.
