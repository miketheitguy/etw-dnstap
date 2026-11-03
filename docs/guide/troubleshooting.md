# Troubleshooting

> **Status:** Current · **Describes:** the code at commit `58fde72` · **Reviewed:** 2026-11-03

Use this guide to find the cause of a failed start, restarts, drops, or missing
records. Start with the Application event log and the SCM state.

## Find your symptom

| Symptom | Section |
| --- | --- |
| The service stops at start, or `sc.exe start` fails | [The service does not reach RUNNING](#the-service-does-not-reach-running) |
| The collector is down, or the service reports receiver errors | [The receiver is down or unreachable](#the-receiver-is-down-or-unreachable) |
| The receiver is slow, or frames go to the spool | [The receiver is slow](#the-receiver-is-slow) |
| The log shows a spool warning | [Spool warnings](#spool-warnings) |
| The service restarts again and again | [The service keeps restarting](#the-service-keeps-restarting) |
| Capture stops during a run | [Capture stops during a run](#capture-stops-during-a-run) |
| A counter such as `ring_dropped` or `etw_lost` is above 0 | [Counters show drops](#counters-show-drops) |
| The log shows a rate warning | [Rate warning](#rate-warning) |
| The service runs, but the collector gets nothing | [No records arrive at the collector](#no-records-arrive-at-the-collector) |
| A leftover ETW session blocks the start | [Stale ETW session blocks startup](#stale-etw-session-blocks-startup) |
| The service aborts or does not stop | [Watchdog aborts or stop hangs](#watchdog-aborts-or-stop-hangs) |
| The binary prints nothing, or a console launch exits with code 1 | [Run from a console](#run-from-a-console) |
| The Application log has Event ID 6 | [A panic aborts the process](#a-panic-aborts-the-process) |
| An event log message that you want to look up | [Event log messages](#event-log-messages) |

## Read the diagnostics

The service writes its diagnostics only to the Application log, source
`EtwDnstap`. It writes nothing to stdout or stderr. Event ID 1 is a fatal service
failure. Event ID 2 is any information line, including the start and stop
counters. Event ID 3 is any warning. Event ID 4 is any other error-level line,
such as `ETW cleanup failed`, or the reason for a stop after the DNS Server
process changed. Event ID 5 is a watchdog abort. Event ID 6 is a panic.
A log write failure is dropped silently and never stops capture. See
[Log event IDs](monitoring.md#log-event-ids).

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

## Run from a console

The binary writes nothing to stdout or stderr. A console launch exits with code
1 and prints nothing, whatever arguments you pass. A console process cannot
connect to SCM, so it does not write to the event log either. `--help`, `-h`,
`--version`, and `-V` are not options. They fail like any unknown argument.

To read the running version, start the service and read the `version` field of
the `capture started` event. To read the version of the file without running
it, use `(Get-Item <exe>).VersionInfo`. To check an argument, start the service through SCM
and read Event ID 1, as described in
[The service does not reach RUNNING](#the-service-does-not-reach-running).

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
| `StartTraceW: access denied` | Event ID 1 | The service SID lacks `TRACELOG_CREATE_REALTIME` on the session GUID in the message, the session name changed or the grant spells it differently (the name is case-sensitive), or the computer has not restarted since the grant. | [install.md](install.md#if-an-etw-access-check-is-denied) |
| `OpenServiceW(DNS)`, `OpenSCManagerW`, or `QueryServiceStatusEx(DNS)` with `(code 5)` | Event ID 1 | The restricted token cannot query the `DNS` service. The message adds `the restricted service token needs SC_MANAGER_CONNECT and SERVICE_QUERY_STATUS`. | [RUNNING](#the-service-does-not-reach-running) |
| `already exists and does not match this program's session shape` | Event ID 1 | A session with the name exists and cannot be reclaimed. Its shape or its GUID differs from this service's. | [Stale session](#stale-etw-session-blocks-startup) |
| `EtwDnstap watchdog: the startup deadline` or `shutdown deadline` | Event ID 5 | A deadline expired, and the process aborted. The text names the phase: `startup` while the service starts, `shutdown` while it stops. | [Watchdog](#watchdog-aborts-or-stop-hangs) |
| `EtwDnstap panicked:` | Event ID 6 | A panic in the service. The text gives the panic message and location, and the process aborts. | [Panic](#a-panic-aborts-the-process) |
| `dnstap receiver unavailable at startup; capture is running, and frames go to the spool, or are dropped and counted when no spool is in use, until it connects (retrying with backoff)` | Event ID 3 | The service is running and reconnecting. Not fatal. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `dnstap receiver connection lost; frames go to the spool, or are dropped and counted when no spool is in use, until it reconnects (frames already sent are never resent)` | Event ID 3 | An established session failed. Not fatal. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `dnstap reconnect failed` | Event ID 3 | A reconnect attempt failed. The `cause` field is `connect`, `timeout`, `tls`, or `protocol`. Logged for the first four failures, then for the 61st, 121st, and every 60 failures after that, and for the first failure of each `cause` since a session was last established. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `dnstap receiver failed the TLS handshake before the first session` | Event ID 4 | The receiver was down at startup, and the first attempt that reached it ended with a TLS rejection. The text names the trust store, the server name, and a required client certificate as the checks. Logged once. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `dnstap receiver refused the Frame Streams handshake before the first session` | Event ID 4 | The same, with a Frame Streams handshake rejection. Logged once. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `could not start the dnstap reconnect thread` or `dnstap reconnect thread ended without a result` | Event ID 3, Event ID 4 | The service could not run a reconnect attempt. It retries with backoff. The first four, then the 61st, 121st, and so on. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `dnstap receiver reconnected` | Event ID 2 | A session was re-established. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `shutting down while the dnstap receiver is disconnected` | Event ID 3 | The service stopped without sending STOP. | [Receiver](#the-receiver-is-down-or-unreachable) |
| `spool folder is not usable; capture continues without a spool` | Event ID 3 | The folder failed a check. The `reason` field names the check. The service runs without a spool. | [Spool warnings](#spool-warnings) |
| `spool could not be opened; capture continues without a spool` | Event ID 3 | The folder passed the checks, but the spool could not open. The `error` field gives the cause. | [Spool warnings](#spool-warnings) |
| `dnstap spool deleted its oldest segment unsent` | Event ID 3 | The size cap or the free-space floor removed a segment that held unsent frames. | [Spool warnings](#spool-warnings) |
| `dnstap spool discarded unreadable segment data` | Event ID 3 | Segment data was unreadable: a corrupt or cut-off tail, a missing START, a read error after retries, or an invalid cursor that dropped the oldest segment. The `frames` and `bytes` fields give the loss. | [Spool warnings](#spool-warnings) |
| `dnstap spool I/O error` | Event ID 3 | A spool file operation failed. The spool pauses for 10 seconds. | [Spool warnings](#spool-warnings) |
| `dnstap spool is refusing blocks: the volume is below the free-space floor and no older segment is left to delete` | Event ID 3 | The volume has less free space than `--spool-min-free-mib` plus one segment, and only the newest segment is left. The spool refuses blocks for 10 seconds. | [Spool warnings](#spool-warnings) |
| `could not seal the dnstap spool segment` | Event ID 3 | At shutdown, the active segment could not be closed with STOP. | [Spool warnings](#spool-warnings) |
| `dnstap spool backlog kept on disk for the next run` | Event ID 2 | Unsent frames wait on disk for the next start. Not an error. | [Spool warnings](#spool-warnings) |
| `spool in use` | Event ID 2 | The spool opened. The `leftover_segments` and `leftover_bytes` fields show what an earlier run left. | [Spool warnings](#spool-warnings) |
| `the DNS Server service restarted` or `the DNS Server service stopped` | Event ID 4, then Event ID 1 | The DNS Server process changed or stopped, so the service exits for restart. | [Restarts](#the-service-keeps-restarting) |
| `a capture worker exited before shutdown was requested` | Event ID 1 | A worker thread ended on its own. | [Restarts](#the-service-keeps-restarting) |
| `ETW consumer did not return within 5 s of STOP` | Event ID 3 | The trace handle was closed to force the worker out. | [ETW warnings](#etw-consumer-and-session-warnings) |
| `received rate is above 22,000 events/s` | Event ID 3 | The load is above the rate that the default settings handle. | [Rate warning](#rate-warning) |
| `rejected ETW event` | Event ID 3 | A parser rejection. The first four are logged, then the 61st, 121st, and so on. | [Rejected events](#rejected-events) |
| `DNS event schema load failed; its events are rejected until the load is retried` | Event ID 3 | Windows could not return the metadata of an event schema. The service retries after 65,536 more rejected events of that key. | [Rejected events](#rejected-events) |
| `DNS event schema is unsupported and its events are rejected` | Event ID 3 | A schema loaded but did not compile into a decode plan, or it is not the expected event. The service logs it once per key and does not retry the key. It can log it again after the list of rejected keys (8 entries) is cleared because it is full, or when the retry of a load failure fails as unsupported. | [Rejected events](#rejected-events) |
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
| Invalid session name | Event ID 1 with `invalid service arguments` and `session name must be 1 to 128 ASCII letters, digits, underscore, dot, or hyphen, not starting with a hyphen`. The text ends with the reason: the first character that is not allowed and its position, `it starts with a hyphen`, or the length. | Use a name of 1 to 128 ASCII letters, digits, underscore, dot, or hyphen that does not start with a hyphen. See [configuration.md](configuration.md#options). |
| Malformed `--spool-dir` | Event ID 1 with `invalid service arguments`. | A value that is not absolute, is not a path that starts with a drive letter, or has a `.` or `..` part fails command-line parsing. Correct the value in the binary path. Only the default folder falls back silently. |
| Wrong identity | Event ID 1: `must run as NT AUTHORITY\LocalService`. The service is still LocalSystem, a virtual account, or another account. | Run `sc.exe qc EtwDnstap`. Run `cmd.exe /c 'sc.exe config EtwDnstap obj= "NT AUTHORITY\LocalService" password= ""'`. Use `cmd.exe` because Windows PowerShell 5.1 drops the empty password. Then run `sc.exe sidtype EtwDnstap restricted` and start again. |
| Service SID missing | Event ID 1: `the service SID NT SERVICE\EtwDnstap is not an enabled group`. | Run `sc.exe qsidtype EtwDnstap`. If it reports `NONE`, run `sc.exe sidtype EtwDnstap restricted`, then start again. |
| Unrestricted token | Event ID 1: `the process token is not restricted`. | Run `sc.exe sidtype EtwDnstap restricted`, then start again. An unrestricted SID type does not meet the profile. |
| Restricting SIDs incomplete | Event ID 1: `restricting SIDs do not include the service SID` or `the Write Restricted SID (S-1-5-33)`. | Run `sc.exe sidtype EtwDnstap restricted`, restart, and compare the token with the assumption in [SECURITY.md](../../SECURITY.md). If the SCM uses a different restricting set on this OS build, record it. |
| Not in Performance Log Users | Event ID 1: `lacks an enabled Performance Log Users`. | Run `net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add`. Restart the service. |
| Privileges | Event ID 1: `configure SCM RequiredPrivileges to only SeChangeNotifyPrivilege` or `requires only an enabled SeChangeNotifyPrivilege`. | Run `sc.exe privs EtwDnstap SeChangeNotifyPrivilege`, then start again. Run `sc.exe qprivs EtwDnstap` to check the result. |
| DNS Server not running | Event ID 1: `service 'DNS' is not running (process ID 0)`. | Start the DNS Server role service. Check that it is Running. |
| Receiver protocol mismatch | Event ID 1: `connect/negotiation with dnstap receiver` with `did not accept dnstap content type`. | Configure the collector for content type `protobuf:dnstap.Dnstap`. A refused or timed-out connection does not stop startup. See [the receiver section](#the-receiver-is-down-or-unreachable). |
| Receiver certificate not accepted | Event ID 1: `connect/negotiation with dnstap receiver` with `TLS handshake`, or with a TLS alert such as `received fatal alert: CertificateRequired`. | Import the receiver CA into `Cert:\LocalMachine\Root`. Use a certificate that names the receiver IP address, or pass its name in `--tls-server-name`. If the receiver requires a client certificate, turn that requirement off, because the service sends none. See [the receiver certificate step](receiver.md#receiver-certificate). |
| ETW access denied | Event ID 1: `StartTraceW: access denied` or `EnableTraceEx2: access denied`. | For `StartTraceW`, run `scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` with the session name from the message, then restart the computer. Use the exact spelling of `--session-name`, including its case. The session GUID comes from the exact name, so a grant for `etwdnstap` does not cover `EtwDnstap`. For `EnableTraceEx2`, inspect the DNS provider descriptor and do not add a grant on it. See [install.md](install.md#if-an-etw-access-check-is-denied). |
| SCM query of `DNS` denied | Event ID 1: `OpenServiceW(DNS)`, `OpenSCManagerW`, or `QueryServiceStatusEx(DNS)` with `(code 5)`. | The restricted token needs `SC_MANAGER_CONNECT` and `SERVICE_QUERY_STATUS` for Everyone or the service SID. Inspect `sc.exe sdshow DNS`. The query succeeded in the lab on build 26100. |
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

A receiver that is down does not stop the service. With a spool in use, the
service stores frames on disk and sends them when the receiver returns. Without
a spool, the sink counts the frames it cannot deliver, and it retries with
backoff.

1. **At startup:** Event ID 3, `dnstap receiver unavailable at startup; capture
   is running, and frames go to the spool, or are dropped and counted when no
   spool is in use, until it connects (retrying with backoff)`. The service
   reaches `RUNNING`.
2. **During a run:** Event ID 3, `dnstap receiver connection lost`. Then
   `dnstap reconnect failed` appears for the first four failures, then for the 61st,
   121st, and every 60 failures after that. It also appears for the first
   failure of each `cause` field value (`connect`, `timeout`, `tls`, or
   `protocol`) since a session was last established. Causes that alternate add
   at most one extra line per cause in an outage. When a session is
   re-established, Event ID 2 logs `dnstap receiver reconnected`.
3. **Counters:** `receiver_connected` is `false` while down.
   `receiver_disconnects` and `receiver_reconnect_failures` show the failures.
   With a spool, `spool_bytes` and `spool_segments` rise while the receiver is
   down, and `replayed` rises after it returns. Use `spool_bytes` for the spool
   size. A folder listing can show the open segment as 0 bytes until the
   segment is closed. Without a spool, `disconnected_dropped` shows the loss.
   See [Counters](#counters-show-drops).

Frames in flight when a session failed are counted in `disconnected_dropped`
and are never resent. Frames that arrive while the receiver is down go to the
spool when one is in use. Without a spool, they count in `disconnected_dropped`. The sink's retry delay has a base of 250 ms that
doubles up to 30 seconds, and each wait is randomized. See
[Receiver connection](configuration.md#receiver-connection).

**Incompatible receiver after startup.** If the receiver answers with the wrong
content type during a reconnect, each attempt fails. The service does not stop.
Check that `receiver_connected` stays `false` and correct the collector's content
type. Only the first connection at startup stops the service on a protocol
failure.

**The receiver was down at startup and then rejects the connection.** If the
first attempt that reaches the receiver fails the TLS handshake or the Frame
Streams handshake, the service logs one error (Event ID 4) that names the cause,
and then retries with backoff. The error does not stop the service. A TLS
rejection means that the Windows trust store does not trust the receiver
certificate, that the certificate does not match `--tls-server-name` (or the
receiver IP address when the option is not set), or that the receiver requires a
client certificate. This service sends none, so a receiver that requires one
rejects it with a TLS alert such as `received fatal alert: CertificateRequired`.
A Frame Streams rejection means
that the receiver does not accept the content type `protobuf:dnstap.Dnstap`.
The `cause` field of the later `dnstap reconnect failed` lines shows `tls` or
`protocol` too. A session that is established clears the list of causes that
were seen, so each later outage logs each of its causes once again. A lost
session does not clear it, and `dnstap receiver connection lost` marks each
outage.

**Shutdown while disconnected.** The service logs `shutting down while the dnstap
receiver is disconnected; no STOP sent` and stops with exit code 0. Frames still in the
spool stay on disk, and the next start sends them. Without a spool, frames
captured during the outage are in `disconnected_dropped`, not in the collector.

With a spool, check `spool_available`. It should read `true`. `spool_bytes`
should fall after the receiver returns, and `replayed` should rise. If
`spool_bytes` keeps rising while `receiver_connected` is `true`, see
[The receiver is slow](#the-receiver-is-slow).

Check the collector's listener, its port, its content type, and the firewall
between the hosts.

## The receiver is slow

A spool removes most slow-receiver loss while the spool is in use. When the send
queue holds 96 of its 128 blocks, the sender writes new blocks to the spool. The
parser does not wait for the receiver.

Signs: `spooled` and `spool_bytes` rise while `receiver_connected` stays `true`.
`slow_writes` and `write_max_us` rise.

Frames still drop in two cases. A socket write that stalls for longer than the
queue lasts drops frames, and they count in `queue_dropped`. With no spool in
use, the queue fills and drops frames in `queue_dropped` too. Refused blocks
count in `spool_dropped`, and the queue does not fill.

Check these in order:

1. `queue_dropped`. With a spool in use, a rise means that a socket write or a
   disk write blocked the sender thread for longer than the queue lasts. Check
   `slow_writes`, `write_max_us`, and the disk. Check `spool_available` and
   `spool_dropped` too.
2. `spool_bytes` against `--spool-max-mib`. If it grows for the whole run, the
   receiver takes less than the input rate. At the cap, the spool deletes its
   oldest data. `spool_dropped` and the warning
   `dnstap spool deleted its oldest segment unsent` show the loss.
3. `slow_writes` and `write_max_us`. Large values point to the receiver or the
   network path.
4. The disk that holds the spool. At 200,000 queries per second, the spool
   writes about 41 MB per second. See [Spool sizing](tuning.md#spool-sizing).
5. The collector's CPU, its disk, and its input queue.

## Spool warnings

Search the Application log for the text. The fields of each event are listed in
[Spool messages](monitoring.md#spool-messages). None of these events contains
frame data.

- **`spool folder is not usable; capture continues without a spool, so frames are dropped and counted while the receiver is down`** (Event ID 3, at startup). The `reason` field gives the check that failed. Find the reason below and follow its action:
  - ``the spool folder path is not absolute``, ``the spool folder path has a `.` or `..` component``, ``the spool folder path is not on a local drive letter (network, device, and other path forms are not accepted)``: the service checks the shape of the path before this check. A malformed `--spool-dir` fails at command-line parsing, and the default path always has a valid shape. You do not expect these reasons. If you see one, report it.
  - ``the spool folder is not on a fixed local disk (drive type {code})``: the folder is on a removable, network, RAM, or unknown drive. Move it to a fixed local disk, and set `--spool-dir`.
  - ``the spool folder has no parent folder``: the folder is a drive root, such as `C:\`. Choose a folder with a parent.
  - ``the spool folder does not exist``: create the folder. See [Create the spool folder](install.md#create-the-spool-folder) or [Add the spool folder](upgrade.md#add-the-spool-folder).
  - ``the spool folder path is not a folder``: a file has that name. Inspect it.
  - ``the {which} is a reparse point``, where `{which}` is `spool folder` or `parent of the spool folder`: a link or junction is in the path. Inspect it. Do not follow it.
  - ``the spool folder owner is not SYSTEM or Administrators``: run `icacls` on the folder. Do not take ownership of it. If you did not create it, remove it, and create it again with the install steps.
  - ``the spool folder has no access control list, so every account can read and write it``: recreate the folder with the install steps.
  - ``the spool folder permissions let an account other than SYSTEM, Administrators, or a service read or write it``: run `icacls` on the folder. Remove each entry that is not SYSTEM, Administrators, or `NT SERVICE\EtwDnstap`. Then run `Test-SpoolFolder` as in install.md.
  - ``cannot check the {which}: {source}``: a Windows call failed during the check. `{which}` names the check, and `{source}` gives the Windows error. Check the disk and the path.
- **`spool could not be opened; capture continues without a spool, so frames are dropped and counted while the receiver is down`** (Event ID 3, at startup). The folder passed the checks, but the spool could not be opened. The `error` field gives the cause. Check that the disk responds, and that no other program holds `cursor.bin` open.
- **`dnstap spool deleted its oldest segment unsent`** (Event ID 3). The size cap or the free-space floor removed a segment with unsent frames. The `frames` and `bytes` fields give the loss. Check `spool_dropped`, `spool_dropped_bytes`, `--spool-max-mib`, and the free space on the volume. A longer outage than the cap covers needs a larger cap. See [Spool sizing](tuning.md#spool-sizing). The first four warnings are logged, then the 61st, 121st, and so on. The counters are complete.
- **`dnstap spool discarded unreadable segment data`** (Event ID 3). Segment data could not be read. This happens for a corrupt or cut-off tail, a missing START, a read error that failed after retries, and an invalid cursor that dropped the oldest segment. The `frames` and `bytes` fields give the loss. Check `spool_errors` and `spool_dropped`, and the disk. The first four warnings are logged, then the 61st, 121st, and so on.
- **`dnstap spool I/O error`** (Event ID 3). A file operation in the spool failed, such as a create, a write, a read, or a delete. The `error` field gives the cause. The spool pauses for 10 seconds, and the block counts in `spool_dropped`. Check `spool_errors`, the free space and health of the disk, any program that locks files in the folder, and the folder's ACL.
- **`dnstap spool is refusing blocks: the volume is below the free-space floor and no older segment is left to delete`** (Event ID 3). The volume has less free space than `--spool-min-free-mib` plus one segment, and deleting older segments is not an option because only the newest one is left, or because deleting did not free space. The `free_mib` and `needed_mib` fields give the two values, and `pause_s` gives the length of the pause in seconds (10). During the pause the spool refuses every block, and each refused block counts in `spool_dropped`. The spool then checks again. The first four pauses are logged, then the 61st, 121st, and so on. Blocks refused during a pause log nothing. Free space on the volume, or lower `--spool-min-free-mib`, and check what else fills the volume.
- **`could not seal the dnstap spool segment`** (Event ID 3, at shutdown). The last segment closed without its STOP. The next start reads it as an unsealed leftover and replays its frames. No action is needed unless the message repeats. If it repeats, check the disk.
- **`dnstap spool backlog kept on disk for the next run`** (Event ID 2, at shutdown). Not an error. Unsent frames wait on disk. The next start replays them first. The `leftover_segments` field of `spool in use` shows how many segments there are.
- **`spool in use`** (Event ID 2, at startup). Not an error. It shows the folder, the cap, the floor, and any leftovers from an earlier run.

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
| `disconnected_dropped` | Frames lost in the sink: in flight when a connection failed, or arriving while the receiver was down with no spool in use. | Check `spool_available`. With a spool, fix the spool folder. Without one, enable the spool or correct the receiver path. See [the receiver section](#the-receiver-is-down-or-unreachable). |
| `queue_dropped` | The send queue was full. With a spool in use, a socket write or a disk write blocked the sender thread for longer than the queue lasts. | See [The receiver is slow](#the-receiver-is-slow). |
| `spool_dropped`, `spool_dropped_bytes` | The spool removed or refused frames unsent. | See [Spool warnings](#spool-warnings). Raise `--spool-max-mib` if the outage is longer than the cap covers. |
| `spool_errors` | Spool I/O errors. | See [Spool warnings](#spool-warnings). |
| `foreign_pid_rejected` | Records from a process other than the DNS Server. | Normally 0. A nonzero value during a DNS restart is expected. Otherwise investigate. |
| `callback_errors` | Records the callback could not hand off. | Investigate with the Event Log. Not in `intake_dropped`. |

Each frame lost in the sink is in at most one of `queue_dropped`,
`disconnected_dropped`, and `spool_dropped`.

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
`--capacity`), the service logs `received rate is above 22,000 events/s; the default buffer
settings were validated at about 20,000 events/s` once per run, followed by the
advice to raise `--capacity` and `--etw-max-buffers` if `ring_dropped` or
`etw_lost` increase. It logs it at the end of the first measurement window above
22,000 events per second. The warning does not mean that events were lost.

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

- **`rejected ETW event; the first four and then every 60th are logged, all are
  counted`.** The first four rejections are logged, then the 61st, 121st, and so
  on. The `rejected` field of the message is the running count, and the
  `rejected` counter counts every rejection. The message never includes payloads
  or names.
- **`DNS event schema load failed; its events are rejected until the load is
  retried`.** Windows could not return the metadata of an event schema. The
  message ends with the Win32 error from `TdhGetEventInformation`, for example
  because the provider manifest was not available yet. Every event of that
  (event ID, version) key is rejected, and it counts in `schema_rejected` and
  `rejected`. After 65,536 more rejected events of the key, the next event loads
  the schema again. If the load succeeds, the key is cleared and its events
  decode again. If it fails, the key waits for another 65,536 events, and the
  failed load counts in `schema_rejected`. The fields `event_id`, `version`,
  `failures` (loads that failed for the key), and `retry_after_events` (65,536)
  are logged for the first four failures of a key, then the 61st, 121st, and so on. If the
  events stay rejected, read the error text, check that the DNS Server role and
  its ETW provider are healthy, and restart the service to load all schemas
  again.
- **`DNS event schema is unsupported and its events are rejected`.** Logged
  once for each (event ID, version) key, with the reason. The line can appear
  again for the same key after the list of rejected keys is cleared because it
  is full, and when the retry of a load failure (above) fails as unsupported. The metadata loaded,
  but it does not match the single-pass decoder grammar, it is not the expected
  provider or event, or its size is invalid. There is no slower decoding path,
  and the service does not retry the key. Every event for that key is rejected
  and counted in `schema_rejected` and `rejected`. Events after the first are
  rejected without a TDH call. The list of rejected keys holds eight keys. When it
  is full, the service clears it, and a key that it dropped loads again on its
  next event. A new Windows build or DNS Server manifest can cause this. Report
  the warning text. Capture of those events needs a decoder change.

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

If the service tried to reclaim a session and failed, Event ID 1 says
`query existing ETW session '<name>' failed` or `stopping stale ETW session
'<name>' failed`, with the Win32 status.

## Watchdog aborts or stop hangs

Startup has a deadline of 10 seconds plus 1 second per 16,384 channel slots of
`--capacity`, rounded up. The default gives 12 seconds, and 262,144 gives 26.
Shutdown has a 10-second deadline. Inside shutdown, ETW waits up to 5 seconds for
`ProcessTrace` to return before it closes the trace handle. See
[Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines).

If a deadline passes, the service writes Event ID 5 to the Application log and
the process aborts. The message names the phase, `startup` or `shutdown`, for
example `the startup deadline of 12 s expired`. Before the service reads its
arguments, the startup deadline is 10 seconds, and the message says
`the startup deadline of 10 s expired`. The abort does not free resources that
native calls still use. The result can be an incomplete capture. It can also
leave a real-time ETW session. An administrator must then inspect and stop that
session, or restart the computer.

A timeout does not prove that the native operation never completed. Check
`logman query -ets` after an abort. The startup budget covers these steps:
the identity checks, the receiver connection, the spool folder check, the pool
allocation, and the ETW start. The ETW start also has its own 10-second deadline, so the message can
name 10 s. Large `--capacity` values allocate more pool memory, so they can take
longer to reach capture.

Shutdown does not wait for a spool backlog. The sink stops writing data to the
receiver 1 second after shutdown starts. A write in progress and the STOP/FINISH
exchange can take about 4 seconds more.

The service accepts the Stop and Shutdown controls, and only in the `RUNNING`
state. In `START_PENDING` and `STOP_PENDING` it accepts no control, so the SCM
refuses a stop request during startup. Wait for `RUNNING`, or for the startup
deadline. The service treats Shutdown like Stop. It answers Interrogate and rejects every other
control code, such as the user-defined code 200.

## A panic aborts the process

A panic in the service writes Event ID 6 to the Application log, and then the
process aborts. The text starts with `EtwDnstap panicked:` and gives the panic
message and the source location. The abort skips cleanup, as a watchdog abort
does. Check for a leftover ETW session with `logman query -ets`, and report the
Event ID 6 text with the steps that led to it.

## Upgrading

- A removed option (`--output`, `--max-output-bytes`, `--metrics`,
  `--etw-flush-ms`) fails command-line parsing, and the service does not start.
  Remove it from the binary path. To replace `--etw-flush-ms`, use the
  [high-rate settings](tuning.md#high-rate-settings).
- After an upgrade, the service logs `spool folder is not usable` until the spool
  folder exists. Create it as in [upgrade.md](upgrade.md#add-the-spool-folder).
  Register the new counter manifest too, or the spool counters do not appear.
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
- The `output stats` line that shows the spool counters. Do not send the spool
  files. They hold client addresses and query names.
- The commit SHA and whether it is a release or a diagnostic (`diag-timing`) build.

For security issues, follow [SECURITY.md](../../SECURITY.md) rather than a public issue.
