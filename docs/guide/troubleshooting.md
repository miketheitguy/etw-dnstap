# Troubleshooting

> **Status:** Current · **Describes:** `main` at commit `907f728` · **Reviewed:** 2026-11-02

Use this guide to find the cause of a failed start, restarts, drops, or missing
records. Start with the Application event log and the SCM state.

## Find your symptom

| Symptom | Section |
| --- | --- |
| The service stops at start, or `sc.exe start` fails | [The service does not reach RUNNING](#the-service-does-not-reach-running) |
| The collector is down, or the service reports receiver errors | [The receiver is down or unreachable](#the-receiver-is-down-or-unreachable) |
| The service restarts again and again | [The service keeps restarting](#the-service-keeps-restarting) |
| Capture stops during a run | [Capture stops during a run](#capture-stops-during-a-run) |
| A counter such as `ring_dropped` or `etw_lost` is above 0 | [Counters show drops](#counters-show-drops) |
| The log shows a rate warning | [Rate warning](#rate-warning) |
| The service runs, but the collector gets nothing | [No records arrive at the collector](#no-records-arrive-at-the-collector) |
| A leftover ETW session blocks the start | [Stale ETW session blocks startup](#stale-etw-session-blocks-startup) |
| The service aborts or does not stop | [Watchdog aborts or stop hangs](#watchdog-aborts-or-stop-hangs) |
| An event log message that you want to look up | [Event log messages](#event-log-messages) |

## Read the diagnostics

The service writes its diagnostics to two places:

- **Application log, source `EtwDnstap`.** Event ID 1 is a fatal service failure.
  Event ID 2 is any information line, including the start and stop counters.
  Event ID 3 is any warning. Event ID 4 is an error that does not by itself stop
  the service. Event ID 5 is a watchdog abort. A log write failure is dropped
  silently and never stops capture. See
  [Log event IDs](monitoring.md#log-event-ids).
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
| `connect/negotiation with dnstap receiver` | Event ID 1 | A protocol failure at startup, such as a wrong content type or a TLS certificate that the service rejects. Fatal. | [RUNNING](#the-service-does-not-reach-running) |
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
| `ETW consumer did not return within 5 s of STOP` | Event ID 3 | The trace handle was closed to force the worker out. | [ETW warnings](#etw-consumer-and-session-warnings) |
| `received rate is above what the default buffer settings were validated for` | Event ID 3 | The load is above the rate that the default settings handle. | [Rate warning](#rate-warning) |
| `rejected ETW event` | Event ID 3 | A parser rejection. The first four are logged. | [Rejected events](#rejected-events) |
| `DNS event schema is unsupported and its events are rejected` | Event ID 3 | A schema did not load or did not compile. Its events are rejected. | [Rejected events](#rejected-events) |
| `stale ETW session used a different buffer size` | Event ID 3 | A leaked session was reclaimed despite a buffer mismatch. | [Stale session](#stale-etw-session-blocks-startup) |
| `ETW keyword narrowing unavailable` | Event ID 3 | The subscription uses the fallback keyword mask. | [No records](#no-records-arrive-at-the-collector) |
| `RESPONSE_FAILURE keyword is not available` | Event ID 3 | The service does not capture failed responses. | [Subscription](configuration.md#subscription) |
| `performance counters are unavailable` | Event ID 3 | Counter registration failed. Capture continues without counters. | [install.md](install.md#7-start-capture-and-check-that-it-is-running) |

## The service does not reach RUNNING

`sc.exe query EtwDnstap` shows `START_PENDING`, then `STOPPED`. A failed start
reports a service-specific error (`WIN32_EXIT_CODE` 1066, `SERVICE_EXIT_CODE` 1).
Work through these causes in order.

| Cause | What you see | Action |
| --- | --- | --- |
| Invalid option | Event ID 1 with `invalid service arguments`. | Correct the binary path. Options are in [configuration.md](configuration.md). Removed options fail here. |
| Wrong identity | Event ID 1: `must run as NT AUTHORITY\LocalService`. The service is still LocalSystem, a virtual account, or another account. | Run `sc.exe qc EtwDnstap`. Run `cmd.exe /c 'sc.exe config EtwDnstap obj= "NT AUTHORITY\LocalService" password= ""'`. Use `cmd.exe` because Windows PowerShell 5.1 drops the empty password. Then run `sc.exe sidtype EtwDnstap restricted` and start again. |
| Service SID missing | Event ID 1: `the service SID NT SERVICE\EtwDnstap is not an enabled group`. | Run `sc.exe qsidtype EtwDnstap`. If it reports `NONE`, run `sc.exe sidtype EtwDnstap restricted`, then start again. |
| Unrestricted token | Event ID 1: `the process token is not restricted`. | Run `sc.exe sidtype EtwDnstap restricted`, then start again. An unrestricted SID type does not meet the profile. |
| Restricting SIDs incomplete | Event ID 1: `restricting SIDs do not include the service SID` or `the Write Restricted SID (S-1-5-33)`. | Run `sc.exe sidtype EtwDnstap restricted`, restart, and compare the token with the assumption in [SECURITY.md](../../SECURITY.md). If the SCM uses a different restricting set on this OS build, record it. |
| Not in Performance Log Users | Event ID 1: `lacks an enabled Performance Log Users`. | Run `net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add`. Restart the service. |
| Privileges | Event ID 1: `configure SCM RequiredPrivileges to only SeChangeNotifyPrivilege` or `requires only an enabled SeChangeNotifyPrivilege`. | Run `sc.exe privs EtwDnstap SeChangeNotifyPrivilege`, then start again. Run `sc.exe qprivs EtwDnstap` to check the result. |
| DNS Server not running | Event ID 1: `service 'DNS' is not running (process ID 0)`. | Start the DNS Server role service. Check that it is Running. |
| Receiver protocol mismatch | Event ID 1: `connect/negotiation with dnstap receiver` with `did not accept dnstap content type`. | Configure the collector for content type `protobuf:dnstap.Dnstap`. A refused or timed-out connection does not stop startup. See [the receiver section](#the-receiver-is-down-or-unreachable). |
| Receiver certificate not accepted | Event ID 1: `connect/negotiation with dnstap receiver` with `TLS handshake`. | Import the receiver CA into `Cert:\LocalMachine\Root`. Use a certificate that names the receiver IP address, or pass its name in `--tls-server-name`. See [the receiver certificate step](receiver.md#receiver-certificate). |
| ETW access denied | Event ID 1: `StartTraceW: access denied` or `EnableTraceEx2: access denied`. | For `StartTraceW`, run `scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` with the session name from the message, then restart the computer. For `EnableTraceEx2`, inspect the DNS provider descriptor and do not add a grant on it. See [install.md](install.md#if-an-etw-access-check-is-denied). |
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
passed. The `WAIT_HINT` value shows the deadline in milliseconds. The
`CHECKPOINT` value advances after every 16,384 allocated pool slots. A
checkpoint that does not advance before the wait hint passes indicates a hang.

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
and correct the collector's content type. Only the first connection at startup stops
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
- Inspect the policy with `sc.exe qfailure EtwDnstap`. Check that
  `sc.exe qfailureflag EtwDnstap` reports the non-crash flag as set.

## Capture stops during a run

Capture no longer stops when the receiver connection fails. It stops only for
the reasons in [the restarts section](#the-service-keeps-restarting), or when
the shutdown exchange fails.

1. Find the failure text in the Application log (Event ID 1). A shutdown failure
   names `finish dnstap STOP/FINISH exchange`.
2. For a DNS restart, check that `foreign_pid_rejected` rose during the gap. Those
   records were not from the process the service was filtering.
3. Check the collector's log for the reset or restart at the same time.
4. Check that the recovery policy is in place. Without it, the service stays down
   until someone starts it.

## Counters show drops

The `output stats` and `capture stats` lines (Event ID 2) report counters every
`--stats-interval-seconds`. `capture stopped` and `output stopped` repeat them at
shutdown. A restart resets every counter. The loss categories are:

| Counter | Meaning | Action |
| --- | --- | --- |
| `ring_dropped` | The event channel was full. The newest event was dropped. | Raise `--capacity`. For moderate or high rates, use the [high-rate settings](tuning.md#high-rate-settings). |
| `pool_exhausted_regular`, `pool_exhausted_jumbo` | The pool had no free slot of the record's class. | Raise `--jumbo-capacity` if `pool_exhausted_jumbo` rises. With `--jumbo-capacity 0`, every jumbo record is counted here. |
| `receiver_disconnected` | The channel's parser end was gone. This is the event channel, not TCP. | Check whether the parser stopped. See [restarts](#the-service-keeps-restarting). |
| `disconnected_dropped` | Frames lost to a TCP outage. | Correct the receiver path. See [the receiver section](#the-receiver-is-down-or-unreachable). |
| `foreign_pid_rejected` | Records from a process other than the DNS Server. | Normally 0. A nonzero value during a DNS restart is expected. Otherwise investigate. |
| `callback_errors` | Records the callback could not hand off. | Investigate with the Event Log. Not in `intake_dropped`. |

`intake_dropped` is the sum of `ring_dropped`, `pool_exhausted_regular`,
`pool_exhausted_jumbo`, and `receiver_disconnected`. Do not add it to them.
`ring_dropped` and the pool fields are parts of `intake_dropped`, not additional
losses. The `etw_lost` counter reports events lost inside ETW. It uses a
different unit from buffer loss (`etw_buffers_lost`), and the two are not added
together. `rejected` counts events the parser rejected. Excluded event IDs (259,
260, 261, and the others) are never counted. The full list and the identities are in
[Counter relationships](monitoring.md#counter-relationships).

Compare your collector's record count with the client's count, and treat any
difference as unexplained until you read the counters.

## Rate warning

With the default buffer settings (the default ETW settings and the default
`--capacity`), the service logs `received rate is above what the default buffer
settings were validated for (about 20,000 events/s)` once per run. It logs it at
the end of the first measurement window above 22,000 events per second. The
warning does not mean that events were lost.

Check `ring_dropped` and `etw_lost` in the `capture stats` events, or the
Channel Full Drops and ETW Events Lost performance counters. If either rises,
restart with the settings from
[High-rate settings](tuning.md#high-rate-settings):

```text
--etw-max-buffers 256 --etw-buffer-kib 64 --capacity 65536
```

On the 6-vCPU lab VM with an authoritative load, this combination lost no
events up to about 44,000 queries per second. At about 49,400 queries per
second, one of two runs dropped 0.28% of events at the channel. After the
restart, check that `ring_dropped` and `etw_lost` stop rising. If
`ring_dropped` still rises, raise `--capacity`, for example to 131,072. The lab
did not test 131,072. The startup deadline is 14 seconds at `--capacity 65536`
and 18 seconds at 131,072. The defaults were validated at 10,000 queries per
second. Lab results for the defaults are in
[Low-rate defaults](tuning.md#low-rate-defaults), and for this
combination in [High-rate settings](tuning.md#high-rate-settings).

## ETW consumer and session warnings

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
  slower decoding path. Every event for that key is rejected and counted in
  `schema_rejected` and `rejected`. Events after the first are rejected without
  a TDH call. The cache holds eight keys and
  is cleared when full. A new Windows build or DNS Server manifest can cause
  this. Report the warning text. Capture of those events needs a decoder change.

## No records arrive at the collector

Check the layers in order.

1. **Is the service running and connected?** Look for a `capture started` line
   (Event ID 2). The lack of one means the service did not reach capture. In
   the latest `output stats` line, `receiver_connected` should be `true`.
2. **Does the DNS Server write the events?** Check that the events are
   generated. Event IDs 256, 257, and 258 come from `Microsoft-Windows-DNSServer`.
   Recursive traffic does not produce client records.
3. **Is the DNS process still the one being filtered?** A DNS restart leaves a
   gap until EtwDnstap restarts. A rising `foreign_pid_rejected` shows records
   from the new process being refused.
4. **Is the subscription the one you expect?** The `capture started` line shows
   the keyword masks. A fallback warning means the keyword lookup failed and the
   subscription uses `0x8000000000000000`. See
   [configuration.md](configuration.md#subscription).
5. **Is the collector's filter or pipeline correct?** The example pipeline in
   [receiver.md](receiver.md#receiver-configuration) forwards to the console.

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
`--capacity`, rounded up. The default gives 12 seconds, and 262,144 gives 26.
Shutdown has a 10-second deadline. Inside shutdown, ETW waits up to 5 seconds for
`ProcessTrace` to return before it closes the trace handle. See
[Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines).

If a deadline passes, the service writes Event ID 5 to the Application log and
the process aborts. The message names the phase, for example
`the startup deadline of 12 s expired`. The abort does not free resources that
native calls still use. The result can be an incomplete capture. It can also
leave a real-time ETW session. An administrator must then inspect and stop that
session, or restart the computer.

A timeout does not prove that the native operation never completed. Check
`logman query -ets` after an abort. The startup budget covers these steps:
the identity checks, the receiver connection, the pool allocation, and the ETW
start. The ETW start also has its own 10-second deadline, so the message can
name 10 s. Large `--capacity` values allocate more pool memory, so they can take
longer to reach capture.

## Upgrading

- A removed option (`--output`, `--max-output-bytes`, `--metrics`,
  `--etw-flush-ms`) fails command-line parsing, and the service does not start.
  Remove it from the binary path. To replace `--etw-flush-ms`, use the
  [high-rate settings](tuning.md#high-rate-settings).
- A service still running as LocalSystem or the virtual account refuses to
  start. Follow the upgrade steps in
  [upgrade.md](upgrade.md#repair-cases). In summary:
  1. Move it to LocalService with a restricted SID type.
  2. Add the service SID to Performance Log Users.
  3. Run the ETW grant script, then restart the computer.
  4. Keep the executable's read and execute grant for the service SID.
  5. Keep the recovery policy.
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
