# Troubleshooting

> **Status:** Current · **Describes:** `main` at commit `9172f2d` · **Reviewed:** 2026-11-01

Start with the Application event log and the SCM state. The service writes its
diagnostics to two places:

- **Application log, source `EtwDnstap`.** Event ID 1 is a fatal service failure
  (level Error). Event ID 2 is a start or stop line with counters. Event ID 3 is
  a rejected-event warning. Event ID 4 is an error. A Log write failure is
  dropped silently and never stops capture.
- **stderr.** Tracing output goes to stderr. Under SCM, stderr is not a durable
  log. Use the event log for evidence.

To read the stored failure text:

```powershell
$eventFilter = @{LogName='Application'; ProviderName='EtwDnstap'; Id=1}
Get-WinEvent -FilterHashtable $eventFilter -MaxEvents 10 |
    Select-Object TimeCreated, Id,
        @{Name='Details'; Expression={$_.Properties[0].Value}}
```

If there are no events, check that the source is registered
(`[System.Diagnostics.EventLog]::SourceExists('EtwDnstap')`). See
[install.md](install.md#4-set-the-service-identity-privileges-and-event-source).

## The service does not reach RUNNING

`sc.exe query EtwDnstap` shows `START_PENDING`, then `STOPPED` with a nonzero
exit code. Work through these causes in order.

| Cause | What you see | Fix |
| --- | --- | --- |
| Invalid option | Event ID 1 with an argument parse error (`invalid service arguments`). | Correct the binary path. Options are in [configuration.md](configuration.md). Removed options fail here. |
| Receiver not listening or unreachable | Event ID 1 mentioning `connect/negotiation with dnstap receiver`. | Start the collector listener first. Check the address, the port, and the firewall. |
| Token gate rejects the process | Event ID 1 with one of: `EtwDnstap currently requires the LocalSystem account`; `service SID must be enabled and not deny-only`; `requires only an enabled SeChangeNotifyPrivilege`. | Run `sc.exe qsidtype EtwDnstap` and `sc.exe qprivs EtwDnstap`. Set `sidtype unrestricted` and `privs SeChangeNotifyPrivilege`, then start again. |
| Leaked ETW session with the same name | Startup fails because a session with the name exists and is not reclaimable. | See [Stale ETW session](#stale-etw-session-blocks-startup). |
| DNS Server not running | The `DNS` dependency is not running, so SCM does not start the service. | Start the DNS Server role service and confirm it is running. |
| Watchdog abort | The process ends without a clean Event ID 1 entry. Startup has a 10-second deadline. | See [Watchdog aborts](#watchdog-aborts-or-stop-hangs). |

Repeat `sc.exe query EtwDnstap` while the state is `START_PENDING` or
`STOP_PENDING`. Do not assume a hang until the 10-second deadline has passed.

## The service keeps restarting

With the recovery policy from [install.md](install.md#5-configure-scm-recovery),
SCM restarts the service after a non-crash failure. The first two restarts occur
after 5 seconds. Later restarts occur after 60 seconds. The failure count resets
after 86,400 seconds without a failure.

- Each restart is a new capture. Records that were in flight at the failure are
  lost. The service does not resend them.
- A restart while the receiver is still down fails again. Fix the receiver
  before expecting the loop to end.
- Check the failures in the Application log, Event ID 1.
- Inspect the policy with `sc.exe qfailure EtwDnstap`. Confirm
  `sc.exe qfailureflag EtwDnstap` reports the non-crash flag as set.

## Capture stops during a run

An I/O failure ends capture and the service exits with a service-specific error.
Any write, flush, or 2-second deadline failure on the receiver connection does
this. A collector restart or a single TCP reset is enough.

1. Find the failure text in the Application log (Event ID 1). The chain includes
   `send dnstap packet`, `flush dnstap TCP sender`, or `finish dnstap STOP/FINISH exchange`.
2. Check the collector's log for the reset or the restart at the same time.
3. Confirm the recovery policy is in place. Without it, the service stays down
   until started manually.

There is no automatic reconnect. Queued events at the moment of failure are
lost, and a successful send does not prove that the collector stored them.

## Counters show drops

The `capture stopped` line (Event ID 2) reports counters. The three loss
categories are:

| Counter | Meaning | Action |
| --- | --- | --- |
| `ring_dropped` | The event channel was full. The newest event was dropped. | Raise `--capacity`, or use the periodic flush for moderate or high rates. See [Low-rate defaults](configuration.md#low-rate-defaults). |
| `pool_exhausted` | The pool had no free slot for a record, regular or jumbo. | Raise `--jumbo-capacity` if the jumbo class is exhausted. |
| `receiver_disconnected` | The receiver connection was lost before the event was written. | Fix the receiver path. Use the recovery policy. |

`intake_dropped` is the sum of the three. Do not add it to them. The `etw_lost`
counter reports events lost inside ETW. It uses a different unit from buffer
loss, and the two are not added together. `rejected` counts events the parser
rejected. Excluded event IDs (258, 260, 261) are never counted.

Historical zero-loss analyses reconciled all three categories. Compare your
collector's record count with the client's count, and treat any difference as
unexplained until you have read the counters.

## No records arrive at the collector

Check the layers in order.

1. **Is the service running and connected?** Look for a `capture started` line
   (Event ID 2). The lack of one means the service did not reach capture.
2. **Does the DNS Server write the events?** Confirm the events are being
   generated: event IDs 256 and 257 come from `Microsoft-Windows-DNSServer`.
   Recursive traffic does not produce client records.
3. **Is the subscription the one you expect?** The `capture started` line shows
   the keyword masks. A fallback warning means the keyword lookup failed and the
   subscription uses `0x8000000000000000`. See
   [configuration.md](configuration.md#subscription).
4. **Is the collector's filter or pipeline correct?** The example pipeline in
   [install.md](install.md#receiver-configuration) forwards to the console.

## Stale ETW session blocks startup

A session that a crashed or aborted service left behind is reclaimed at startup
if its name and shape match. If the buffer size changed, the match fails, and
startup reports the existing session error.

```powershell
logman query -ets
logman stop EtwDnstap -ets
```

Use the `--session-name` you configured in place of `EtwDnstap`. Sessions named
`etw-dnstap-<pid>` from earlier builds are not reclaimed automatically. Stop them
the same way. Only stop a session you can identify as this service's.

## Watchdog aborts or stop hangs

Startup and shutdown each have a 10-second fail-stop watchdog. If a deadline
passes or the watchdog cannot start, the process aborts. It does not free
resources that native calls still use. The result can be an incomplete capture
and a real-time ETW session that needs an administrator to inspect and stop, or a
reboot.

A timeout does not prove that the native operation never completed. Check
`logman query -ets` after an abort. The startup budget covers the account lookup,
the receiver handshake, and the pool allocation. Large `--capacity` values
allocate more pool memory, so they can take longer to reach capture.

## Upgrading

- A removed option (`--output`, `--max-output-bytes`, `--metrics`) fails command-line
  parsing. Remove it from the binary path.
- Preserve the service identity, privileges, ACLs, and recovery policy. Changing
  them is a separate step with its own verification.

## Still stuck

Collect these before asking for help. Redact the receiver address and any query
names first.

- The Application log entries for source `EtwDnstap`, with the time range.
- The output of `sc.exe qc EtwDnstap`, `sc.exe qsidtype EtwDnstap`,
  `sc.exe qprivs EtwDnstap`, `sc.exe qfailure EtwDnstap`, and
  `sc.exe qfailureflag EtwDnstap`.
- The `logman query -ets` list.
- The commit SHA and whether it is a release or a diagnostic (`diag-timing`) build.

For security issues, follow [SECURITY.md](../../SECURITY.md) rather than a public issue.
