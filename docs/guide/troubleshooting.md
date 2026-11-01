# Troubleshooting

> **Status:** Current · **Describes:** `main` at commit `efb47af` · **Reviewed:** 2026-11-01

Start with the Application event log and the SCM state. The service writes its
diagnostics to two places:

- **Application log, source `EtwDnstap`.** Event ID 1 is a fatal service failure.
  Event ID 2 is any information line, including the start and stop counters.
  Event ID 3 is any warning. Event ID 4 is an error that does not by itself stop
  the service. Event ID 5 is a watchdog abort. A log write failure is dropped
  silently and never stops capture. See
  [Log event IDs](configuration.md#log-event-ids).
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
[install.md](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source).

## Event log messages

Search the Application log for the text, then go to the section named.

| Message (starts with or contains) | Level | Meaning | Section |
| --- | --- | --- | --- |
| `invalid service arguments` | Event ID 1 | The binary path has an invalid option. | [RUNNING](#the-service-does-not-reach-running) |
| `must run as NT AUTHORITY\LocalService` | Event ID 1 | The token user is not S-1-5-19 (LocalSystem or a virtual account). | [RUNNING](#the-service-does-not-reach-running) |
| `the service SID NT SERVICE\EtwDnstap is not an enabled group` | Event ID 1 | The service SID type is none, or the group is disabled or deny-only. | [RUNNING](#the-service-does-not-reach-running) |
| `the process token is not restricted` | Event ID 1 | The service SID type is not restricted. | [RUNNING](#the-service-does-not-reach-running) |
| `restricting SIDs do not include` | Event ID 1 | The token is restricted but lacks the service SID or `S-1-5-33`. | [RUNNING](#the-service-does-not-reach-running) |
| `lacks an enabled Performance Log Users` | Event ID 1 | The service SID is not in the group, or the membership is deny-only. | [RUNNING](#the-service-does-not-reach-running) |
| `configure SCM RequiredPrivileges` or `requires only an enabled SeChangeNotifyPrivilege` | Event ID 1 | The token holds more than one privilege, or the one privilege is missing or disabled. | [RUNNING](#the-service-does-not-reach-running) |
| `connect/negotiation with dnstap receiver` | Event ID 1 | A protocol failure at startup, such as a wrong content type. Fatal. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `service 'DNS' is not running` | Event ID 1 | The `DNS` service is stopped. | [RUNNING](#the-service-does-not-reach-running) |
| `EnableTraceEx2: access denied` | Event ID 1 | The DNS provider GUID has a descriptor of its own that grants neither LocalService nor Performance Log Users `TRACELOG_GUID_ENABLE`. Not seen in the lab. | [install.md](install.md#if-an-etw-access-check-is-denied) |
| `StartTraceW: access denied` | Event ID 1 | The service SID lacks `TRACELOG_CREATE_REALTIME` on the session GUID in the message, the session name changed, or the computer has not restarted since the grant. | [install.md](install.md#if-an-etw-access-check-is-denied) |
| `OpenServiceW(DNS)` or `OpenSCManagerW` with `access denied` | Event ID 1 | The restricted token cannot query the `DNS` service. | [RUNNING](#the-service-does-not-reach-running) |
| `already exists and does not match this program's session shape` | Event ID 1 | A session with the name exists and cannot be reclaimed. Its shape or its GUID differs from this service's. | [Stale session](#stale-etw-session-blocks-startup) |
| `EtwDnstap watchdog: the startup deadline` or `shutdown deadline` | Event ID 5 | A deadline expired, and the process aborted. | [Watchdog](#watchdog-aborts-or-stop-hangs) |
| `dnstap receiver unavailable at startup` | Event ID 3 | The service is running and reconnecting. Not fatal. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `dnstap receiver connection lost` | Event ID 3 | An established session failed. Not fatal. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `dnstap reconnect failed` | Event ID 3 | A reconnect attempt failed. Logged for the first four failures, then for the 61st, 121st, and every 60 failures after that. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `dnstap receiver reconnected` | Event ID 2 | A session was re-established. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `shutting down while the dnstap receiver is disconnected` | Event ID 3 | The service stopped without sending STOP. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `the DNS Server service restarted` or `the DNS Server service stopped` | Event ID 4, then Event ID 1 | The DNS Server process changed or stopped, so the service exits for restart. | [Restarts](#the-service-keeps-restarting) |
| `a capture worker exited before shutdown was requested` | Event ID 1 | A worker thread ended on its own. | [Restarts](#the-service-keeps-restarting) |
| `ETW session flush failed` | Event ID 3 | A periodic flush failed once. Capture continues. | [ETW warnings](#etw-flush-and-consumer-warnings) |
| `ETW session flush took` | Event ID 3 | One flush call took over 1 second. | [ETW warnings](#etw-flush-and-consumer-warnings) |
| `ETW consumer did not return within 5 s of STOP` | Event ID 3 | The trace handle was closed to force the worker out. | [ETW warnings](#etw-flush-and-consumer-warnings) |
| `received rate is above what the default low-rate ETW settings` | Event ID 3 | The defaults are too small for the load. | [Low-rate warning](#low-rate-warning) |
| `rejected ETW event` | Event ID 3 | A parser rejection. The first four are logged. | [Rejected events](#rejected-events) |
| `DNS event schema failed to load` | Event ID 3 | A schema is cached as rejected. | [Rejected events](#rejected-events) |
| `stale ETW session used a different buffer size` | Event ID 3 | A leaked session was reclaimed despite a buffer mismatch. | [Stale session](#stale-etw-session-blocks-startup) |
| `ETW keyword narrowing unavailable` | Event ID 3 | The subscription uses the fallback keyword mask. | [Counters](#counters-show-drops) |

## The service does not reach RUNNING

`sc.exe query EtwDnstap` shows `START_PENDING`, then `STOPPED` with a nonzero
exit code. Work through these causes in order.

| Cause | What you see | Fix |
| --- | --- | --- |
| Invalid option | Event ID 1 with `invalid service arguments`. | Correct the binary path. Options are in [configuration.md](configuration.md). Removed options fail here. |
| Wrong identity | Event ID 1: `must run as NT AUTHORITY\LocalService`. The service is still LocalSystem, a virtual account, or another account. | Run `sc.exe qc EtwDnstap`. Set `cmd.exe /c 'sc.exe config EtwDnstap obj= "NT AUTHORITY\LocalService" password= ""'` (through `cmd.exe`, because Windows PowerShell 5.1 drops the empty password), then `sc.exe sidtype EtwDnstap restricted`, then start again. |
| Service SID missing | Event ID 1: `the service SID NT SERVICE\EtwDnstap is not an enabled group`. | Run `sc.exe qsidtype EtwDnstap`. If it reports `NONE`, run `sc.exe sidtype EtwDnstap restricted`, then start again. |
| Unrestricted token | Event ID 1: `the process token is not restricted`. | Run `sc.exe sidtype EtwDnstap restricted`, then start again. An unrestricted SID type does not meet the profile. |
| Restricting SIDs incomplete | Event ID 1: `restricting SIDs do not include the service SID` or `the Write Restricted SID (S-1-5-33)`. | Run `sc.exe sidtype EtwDnstap restricted`, restart, and compare the token with the assumption in [SECURITY.md](../../SECURITY.md). If the SCM uses a different restricting set on this OS build, record it. |
| Not in Performance Log Users | Event ID 1: `lacks an enabled Performance Log Users`. | Run `net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add`. Restart the service. |
| Privileges | Event ID 1: `configure SCM RequiredPrivileges to only SeChangeNotifyPrivilege` or `requires only an enabled SeChangeNotifyPrivilege`. | Run `sc.exe privs EtwDnstap SeChangeNotifyPrivilege`, then start again. Run `sc.exe qprivs EtwDnstap` to confirm. |
| DNS Server not running | Event ID 1: `service 'DNS' is not running (process ID 0)`. | Start the DNS Server role service and confirm it is Running. |
| Receiver protocol mismatch | Event ID 1: `connect/negotiation with dnstap receiver` with `did not accept dnstap content type`. | Configure the collector for content type `protobuf:dnstap.Dnstap`. Network failures do not stop startup; see [the receiver section](#the-receiver-is-down-or-unreachable). |
| ETW access denied | Event ID 1: `StartTraceW: access denied` or `EnableTraceEx2: access denied`. | Run `scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` with the session name from the message, then restart the computer. See [install.md](install.md#if-an-etw-access-check-is-denied). |
| SCM query of `DNS` denied | Event ID 1: `OpenServiceW(DNS)` or `OpenSCManagerW` with `access denied`. | The restricted token needs `SC_MANAGER_CONNECT` and `SERVICE_QUERY_STATUS` for Everyone or the service SID. Inspect `sc.exe sdshow DNS`. The query succeeded in the lab on build 26100. |
| Leaked ETW session with the same name | Event ID 1: a session with the name exists and is not reclaimable (different shape or different GUID). | See [Stale ETW session](#stale-etw-session-blocks-startup). |
| Watchdog abort | No Event ID 1. Event ID 5 names the startup deadline. | See [Watchdog aborts](#watchdog-aborts-or-stop-hangs). |

Only one copy of the service runs. The SCM starts one process for the service,
and a second `sc.exe start` fails with error 1056 (already running). The
identity check refuses a process that does not run as this service. The service
has no instance mutex, so there is no mutex to clear.

Repeat `sc.exe query EtwDnstap` while the state is `START_PENDING` or
`STOP_PENDING`. The startup deadline is 10 seconds plus 1 second per 16,384
channel slots of `--capacity`. Do not assume a hang until that deadline has
passed.

## The receiver is down or unreachable

A receiver that is down does not stop the service. The sink logs the event,
drops and counts the events it cannot deliver, and retries with backoff.

1. **At startup:** Event ID 3, `dnstap receiver unavailable at startup; capture
   is running, events are dropped and counted until it connects`. The service
   reaches `RUNNING`.
2. **During a run:** Event ID 3, `dnstap receiver connection lost`. Then
   `dnstap reconnect failed` appears for the first four failures, then for the 61st,
   121st, and every 60 failures after that. When a session is re-established, Event ID 2 logs
   `dnstap receiver reconnected`.
3. **Counters:** `receiver_connected` is `false` while down.
   `receiver_disconnects`, `receiver_reconnect_failures`, and
   `disconnected_dropped` show the loss. See [Counters](#counters-show-drops).

Frames in the write buffer when a session failed, and events that arrived while
disconnected, are never resent. The sink's retry delay has a base of 250 ms that
doubles up to 30 seconds, and each wait is randomized. See
[Receiver connection](configuration.md#receiver-connection).

**Incompatible receiver after startup.** If the receiver answers with the wrong
content type during a reconnect, each attempt fails with `dnstap reconnect
failed`. The service does not stop. Check that `receiver_connected` stays `false`
and fix the collector's content type. Only the first connection at startup stops
the service on a protocol failure.

**Shutdown while disconnected.** The service logs `shutting down while the dnstap
receiver is disconnected; no STOP sent` and stops with exit code 0. Records
captured during the outage are in `disconnected_dropped`, not in the collector.

Check the collector's listener, its port, its content type, and the firewall
between the hosts.

## The service keeps restarting

The service exits with a service-specific error in these cases, and SCM restarts
it under the recovery policy in [install.md](install.md#5-configure-scm-recovery):

- **The DNS Server process changed or stopped.** Event ID 4 names the reason, for
  example `the DNS Server service restarted (process ID 4242 -> 5151)`. Event
  ID 1 follows. The service checks this every 5 seconds.
- **A capture worker ended on its own.** Event ID 1, `a capture worker exited
  before shutdown was requested`.
- **A shutdown failure.** Event ID 1 names `finish dnstap STOP/FINISH exchange`.
- **Any other run failure.**

The first two restarts occur after 5 seconds. Later restarts occur after 60
seconds. The failure count resets after 86,400 seconds without a failure.

- Each restart is a new capture. Records that were in flight at the failure are
  lost, and the service does not resend them.
- When the DNS Server restarted, records from the new DNS process are not
  captured until EtwDnstap restarts, so expect a gap.
- If the `DNS` service is not running when EtwDnstap starts, startup fails with
  `service 'DNS' is not running`. Start the `DNS` service.
- Check the failures in the Application log, Event ID 1.
- Inspect the policy with `sc.exe qfailure EtwDnstap`. Confirm
  `sc.exe qfailureflag EtwDnstap` reports the non-crash flag as set.

## Capture stops during a run

Capture no longer stops when the receiver connection fails. It stops only for
the reasons in [the restarts section](#the-service-keeps-restarting), or when
the shutdown exchange fails.

1. Find the failure text in the Application log (Event ID 1). A shutdown failure
   names `finish dnstap STOP/FINISH exchange`.
2. For a DNS restart, confirm `foreign_pid_rejected` rose during the gap. Those
   records were not from the process the service was filtering.
3. Check the collector's log for the reset or restart at the same time.
4. Confirm the recovery policy is in place. Without it, the service stays down
   until someone starts it.

## Counters show drops

The `output stats` and `capture stats` lines (Event ID 2) report counters every
`--stats-interval-seconds`. `capture stopped` and `output stopped` repeat them at
shutdown. A restart resets every counter. The loss categories are:

| Counter | Meaning | Action |
| --- | --- | --- |
| `ring_dropped` | The event channel was full. The newest event was dropped. | Raise `--capacity`, or use the periodic flush for moderate or high rates. See [Low-rate defaults](configuration.md#low-rate-defaults). |
| `pool_exhausted` (and its `_jumbo` share) | The pool had no free slot for a record. | Raise `--jumbo-capacity` if the jumbo class is exhausted. |
| `receiver_disconnected` | The channel's parser end was gone. This is the event channel, not TCP. | Check whether the parser stopped; see [restarts](#the-service-keeps-restarting). |
| `disconnected_dropped` | Frames lost to a TCP outage. | Fix the receiver path. See [the receiver section](#the-receiver-is-down-or-unreachable). |
| `foreign_pid_rejected` | Records from a process other than the DNS Server. | Normally 0. A nonzero value during a DNS restart is expected. Otherwise investigate. |
| `callback_errors` | Records the callback could not hand off. | Investigate with the Event Log. Not in `intake_dropped`. |

`intake_dropped` is the sum of `ring_dropped`, `pool_exhausted`, and
`receiver_disconnected`. Do not add it to them. `ring_dropped` and the pool
fields are parts of `intake_dropped`, not additional losses. The `etw_lost`
counter reports events lost inside ETW. It uses a different unit from buffer
loss (`etw_buffers_lost`), and the two are not added together. `rejected` counts
events the parser rejected. Excluded event IDs (258, 260, 261) are never
counted. The full list and the identities are in
[Counter relationships](configuration.md#counter-relationships).

Compare your collector's record count with the client's count, and treat any
difference as unexplained until you have read the counters.

## Low-rate warning

With the default ETW settings, the service logs `received rate is above what the
default low-rate ETW settings are sized for (about 1,500 events/s) and events may
be dropped`. It repeats at the end of each measurement window while the rate
stays high.

Restart with the high-rate settings from
[Low-rate defaults](configuration.md#low-rate-defaults):

```text
--etw-flush-ms 50 --etw-max-buffers 256 --etw-buffer-kib 64
```

Then confirm `ring_dropped` stops rising. The defaults are tested only to about
1,000 queries per second.

## ETW flush and consumer warnings

- **`ETW session flush failed`** (once). Later flush failures are counted in
  `etw_flush_errors`. Capture continues.
- **`ETW session flush took N ms`** (once). A flush call took over one second.
  Check the host's load. `etw_flush_max_us` records the longest flush.
- **`ETW consumer did not return within 5 s of STOP`.** The service closes the
  trace handle to force the worker out. Events still in ETW buffers at that
  point may not be delivered, and `etw_lost` may not account for them. Check the
  host for overload.
- **`stale ETW session used a different buffer size; reclaiming anyway`.** Not
  an error. See [Stale ETW session](#stale-etw-session-blocks-startup).

## Rejected events

- **`rejected ETW event; further errors counted`.** The first four rejections
  are logged, and `rejected` counts every one. The message never includes
  payloads or names.
- **`DNS event schema is unsupported and its events are rejected`.** Logged
  once for each (event ID, version) key, with the reason. The schema could not
  be loaded, or it does not match the single-pass decoder grammar. There is no
  slower decoding path: every event for that key is rejected without a TDH call
  and counted in `schema_rejected` and `rejected`. The cache holds eight keys and
  is cleared when full. A new Windows build or DNS Server manifest can cause
  this. Report the warning text; capture of those events needs a decoder change.

## No records arrive at the collector

Check the layers in order.

1. **Is the service running and connected?** Look for a `capture started` line
   (Event ID 2). The lack of one means the service did not reach capture. In
   the latest `output stats` line, `receiver_connected` should be `true`.
2. **Does the DNS Server write the events?** Confirm the events are being
   generated: event IDs 256 and 257 come from `Microsoft-Windows-DNSServer`.
   Recursive traffic does not produce client records.
3. **Is the DNS process still the one being filtered?** A DNS restart leaves a
   gap until EtwDnstap restarts. A rising `foreign_pid_rejected` shows records
   from the new process being refused.
4. **Is the subscription the one you expect?** The `capture started` line shows
   the keyword masks. A fallback warning means the keyword lookup failed and the
   subscription uses `0x8000000000000000`. See
   [configuration.md](configuration.md#subscription).
5. **Is the collector's filter or pipeline correct?** The example pipeline in
   [install.md](install.md#receiver-configuration) forwards to the console.

## Stale ETW session blocks startup

A session that a crashed or aborted service left behind is reclaimed at startup
if its name, its real-time shape, and its session GUID match. The GUID is the
one that the service derives from the session name. The buffer size does not
have to match. A reclaim logs `reclaimed stale ETW session`, and a size
difference logs `stale ETW session used a different buffer size; reclaiming
anyway`.

If startup still reports `already exists and does not match this program's
session shape`, the session has a file or other logger mode, a log file, or a
different GUID. The service does not stop such a session, because another
program or account may own it. Find the owner first. Then stop the session
manually:

```powershell
logman query -ets
logman stop EtwDnstap -ets
```

Use the `--session-name` you configured in place of `EtwDnstap`. Other accounts
that may create ETW sessions (Administrators, LocalService, NetworkService, and
Performance Log Users) can create a session with this name and block startup.
Remove the cause, and then stop that session. Sessions named
`etw-dnstap-<pid>` from earlier builds are not reclaimed automatically. Stop them
the same way. Only stop a session you can identify as this service's.

## Watchdog aborts or stop hangs

Startup has a deadline of 10 seconds plus 1 second per 16,384 channel slots of
`--capacity`, rounded up. The default gives 11 seconds, and 262,144 gives 26.
Shutdown has a 10-second deadline. Inside shutdown, ETW waits up to 5 seconds for
`ProcessTrace` to return before it closes the trace handle. See
[Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines).

If a deadline passes, the service writes Event ID 5 to the Application log and
the process aborts. The message names the phase, for example
`the startup deadline of 11 s expired`. The abort does not free resources that
native calls still use, so the result can be an incomplete capture and a
real-time ETW session that needs an administrator to inspect and stop, or a
reboot.

A timeout does not prove that the native operation never completed. Check
`logman query -ets` after an abort. The startup budget covers the identity
checks, the receiver connection, the pool allocation, and the ETW start. Large
`--capacity` values allocate more pool memory, so they can take longer to reach
capture.

## Upgrading

- A removed option (`--output`, `--max-output-bytes`, `--metrics`) fails command-line
  parsing. Remove it from the binary path.
- A service still running as LocalSystem or the virtual account refuses to
  start. Move it to LocalService with a restricted SID type, add the service
  SID to Performance Log Users, run the ETW grant script, keep the
  executable's read and execute grant for the service SID, and keep the
  recovery policy. See
  [install.md](install.md#repair-or-upgrade).
- Changing the identity, privileges, or ACLs is a separate step with its own
  verification. Record the result against the binary's commit.

## Still stuck

Collect these before asking for help. Redact the receiver address and any query
names first.

- The Application log entries for source `EtwDnstap`, with the time range.
- The output of `sc.exe qc EtwDnstap`, `sc.exe qsidtype EtwDnstap`,
  `sc.exe qprivs EtwDnstap`, `sc.exe qfailure EtwDnstap`, and
  `sc.exe qfailureflag EtwDnstap`.
- The members of `net localgroup "Performance Log Users"`, and the SDDL
  printed by `scripts\provision\Grant-EtwDnstapEtwAccess.ps1`.
- The `logman query -ets` list.
- The commit SHA and whether it is a release or a diagnostic (`diag-timing`) build.

For security issues, follow [SECURITY.md](../../SECURITY.md) rather than a public issue.
