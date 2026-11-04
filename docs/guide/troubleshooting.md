# Troubleshooting

> **Status:** Current · **Describes:** the code at commit `58fde72` · **Reviewed:** 2026-11-03

Use this guide to find the cause of a failed start, restarts, drops, or missing
records. Start with the Application event log and the SCM state.

## Find your symptom

Find the heading that matches the symptom. For a log message, use
[Event log messages](#event-log-messages).

## Read the diagnostics

The service writes diagnostics only to the Application log, source `EtwDnstap`.
It writes nothing to stdout or stderr. A failed log write never stops capture.
The event IDs are listed in [Log event IDs](monitoring.md#log-event-ids). Event ID 1
is a fatal failure, 5 is a watchdog abort, and 6 is a panic.

Read the stored failure text:

```powershell
$eventFilter = @{LogName='Application'; ProviderName='EtwDnstap'; Id=1}
Get-WinEvent -FilterHashtable $eventFilter -MaxEvents 10 |
    Select-Object TimeCreated, Id,
        @{Name='Details'; Expression={$_.Properties[0].Value}}
```

If there are no events, check the source with
`[System.Diagnostics.EventLog]::SourceExists('EtwDnstap')`. See
[install.md](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source).

## Run from a console

A console start exits with code 1 and prints nothing, whatever arguments you pass.
`--help`, `-h`, `--version`, and `-V` are not options. Read the version from the
`version` field of the `capture started` event, or run `(Get-Item <exe>).VersionInfo`.

## Event log messages

Search the Application log for the message. Each row matches the start of the
message.

| Message | Event ID | Meaning |
| --- | --- | --- |
| `invalid service arguments` | 1 | |
| `must run as NT AUTHORITY\LocalService` | 1 | Token user is not S-1-5-19. |
| `the service SID NT SERVICE\EtwDnstap is not an enabled group` | 1 | SID type is none, or the group is disabled or deny-only. |
| `the process token is not restricted` | 1 | SID type is not restricted. |
| `restricting SIDs do not include` | 1 | The token lacks the service SID or `S-1-5-33`. |
| `lacks an enabled Performance Log Users` | 1 | Service SID missing, or deny-only. |
| `configure SCM RequiredPrivileges` or `requires only an enabled SeChangeNotifyPrivilege` | 1 | Privileges are extra, missing, or disabled. |
| `connect/negotiation with dnstap receiver` | 1 | Startup protocol failure. |
| `service 'DNS' is not running` | 1 | |
| `EnableTraceEx2: access denied` | 1 | DNS provider GUID has its own descriptor, granting neither group `TRACELOG_GUID_ENABLE`. Not seen in the lab. |
| `StartTraceW: access denied` | 1 | The service SID lacks `TRACELOG_CREATE_REALTIME` on the session GUID in the message. |
| `OpenServiceW(DNS)`, `OpenSCManagerW`, or `QueryServiceStatusEx(DNS)` with `(code 5)` | 1 | The restricted token cannot query `DNS`. The message adds `the restricted service token needs SC_MANAGER_CONNECT and SERVICE_QUERY_STATUS`. |
| `already exists and does not match this program's session shape` | 1 | Session cannot be reclaimed. |
| `EtwDnstap watchdog: the startup deadline` or `shutdown deadline` | 5 | Deadline expired. Process aborted. |
| `EtwDnstap panicked:` | 6 | |
| `dnstap receiver unavailable at startup` | 3 | Not fatal. |
| `dnstap receiver connection lost` | 3 | Not fatal. |
| `dnstap reconnect failed` | 3 | `cause` is `connect`, `timeout`, `tls`, or `protocol`. |
| `dnstap receiver failed the TLS handshake before the first session` | 4 | Logged once. |
| `dnstap receiver refused the Frame Streams handshake before the first session` | 4 | Logged once. |
| `could not start the dnstap reconnect thread` or `dnstap reconnect thread ended without a result` | 3, 4 | Retries with backoff. |
| `dnstap receiver reconnected` | 2 | |
| `dnstap link is saturated` | 3 | The spool backlog trended up over 10 samples with the receiver connected. See [The receiver is slow](#the-receiver-is-slow). |
| `dnstap link is no longer saturated` | 2 | The backlog is empty or stopped growing. |
| `shutting down while the dnstap receiver is disconnected` | 3 | No STOP sent. |
| `spool folder is not usable` | 3 | The `reason` field names the failed check. |
| `spool could not be opened` | 3 | The folder passed the checks. |
| `dnstap spool deleted its oldest segment unsent` | 3 | Cap or free-space floor removed unsent frames. |
| `dnstap spool discarded unreadable segment data` | 3 | |
| `dnstap spool I/O error` | 3 | |
| `dnstap spool is refusing blocks` | 3 | No free space, and no older segment to remove. |
| `could not seal the dnstap spool segment` | 3 | At shutdown. |
| `dnstap spool backlog kept on disk for the next run` | 2 | Not an error. |
| `spool in use` | 2 | |
| `the DNS Server service restarted` or `the DNS Server service stopped` | 4, then 1 | |
| `a capture worker exited before shutdown was requested` | 1 | |
| `ETW consumer did not return within 5 s of STOP` | 3 | Trace handle closed to force the worker out. |
| `received rate is above 440,000 events/s` | 3 | |
| `rejected ETW event` | 3 | |
| `DNS event schema load failed` | 3 | |
| `DNS event schema is unsupported and its events are rejected` | 3 | The key is not retried. |
| `stale ETW session used a different buffer size` | 3 | Reclaimed anyway. |
| `ETW keyword narrowing unavailable` | 3 | The subscription uses the fallback keyword mask. See [configuration.md](configuration.md#subscription). |
| `RESPONSE_FAILURE keyword is not available` | 3 | Failed responses are not captured. See [configuration.md](configuration.md#subscription). |
| `performance counters are unavailable` | 3 | Capture continues. See [If the counters do not register](monitoring.md#if-the-counters-do-not-register). |

## The service does not reach RUNNING

`sc.exe query EtwDnstap` shows `START_PENDING`, then `STOPPED`. A failed start
reports `WIN32_EXIT_CODE` 1066 and `SERVICE_EXIT_CODE` 1. Check these causes in
order.

| Cause | Event ID 1 text | Fix |
| --- | --- | --- |
| Invalid option | `invalid service arguments` | Fix the binary path. Removed options fail here. See [configuration.md](configuration.md). |
| Invalid session name | `invalid service arguments`, then `session name must be 1 to 128 ASCII letters, digits, underscore, dot, or hyphen, not starting with a hyphen`, with the reason appended | Use a valid name. See [configuration.md](configuration.md#options). |
| Malformed `--spool-dir` | `invalid service arguments` | Use an absolute path with a drive letter and no `.` or `..` part. |
| Wrong identity | `must run as NT AUTHORITY\LocalService` | Run `cmd.exe /c 'sc.exe config EtwDnstap obj= "NT AUTHORITY\LocalService" password= ""'`. Use `cmd.exe`, because Windows PowerShell 5.1 drops the empty password. Then run `sc.exe sidtype EtwDnstap restricted`. |
| Service SID missing or unrestricted token | `the service SID NT SERVICE\EtwDnstap is not an enabled group` or `the process token is not restricted` | Run `sc.exe qsidtype EtwDnstap`. If it reports `NONE`, or the token is not restricted, run `sc.exe sidtype EtwDnstap restricted`. |
| Restricting SIDs incomplete | `restricting SIDs do not include the service SID` or `the Write Restricted SID (S-1-5-33)` | Run `sc.exe sidtype EtwDnstap restricted`. Compare the token with [SECURITY.md](../../SECURITY.md). Record any different restricting set on this OS build. |
| Not in Performance Log Users | `lacks an enabled Performance Log Users` | Run `net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add`. |
| Privileges | `configure SCM RequiredPrivileges to only SeChangeNotifyPrivilege` or `requires only an enabled SeChangeNotifyPrivilege` | Run `sc.exe privs EtwDnstap SeChangeNotifyPrivilege`. Check with `sc.exe qprivs EtwDnstap`. |
| DNS Server not running | `service 'DNS' is not running (process ID 0)` | Start the DNS Server role service. |
| Receiver protocol mismatch | `connect/negotiation with dnstap receiver` with `did not accept dnstap content type` | Set the collector's content type to `protobuf:dnstap.Dnstap`. |
| Receiver certificate rejected | `connect/negotiation with dnstap receiver` with `TLS handshake`, or `received fatal alert: CertificateRequired` | Import the receiver CA into `Cert:\LocalMachine\Root`. Name the receiver IP address on its certificate, or set `--tls-server-name`. Turn off client certificate requirements. See [the receiver certificate step](receiver.md#receiver-certificate). |
| ETW access denied | `StartTraceW: access denied` or `EnableTraceEx2: access denied` | For `StartTraceW`, run `scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` with the name from the message (case-sensitive), then restart the computer. For `EnableTraceEx2`, add no grant on the DNS provider descriptor. See [install.md](install.md#if-an-etw-access-check-is-denied). |
| SCM query of `DNS` denied | `OpenServiceW(DNS)`, `OpenSCManagerW`, or `QueryServiceStatusEx(DNS)` with `(code 5)` | Grant `SC_MANAGER_CONNECT` and `SERVICE_QUERY_STATUS` to Everyone or the service SID. Inspect with `sc.exe sdshow DNS`. |
| Leaked ETW session | A session with the same name cannot be reclaimed | See [Stale ETW session](#stale-etw-session-blocks-startup). |
| Watchdog abort | No Event ID 1. Event ID 5 names the startup deadline. | See [Watchdog aborts](#watchdog-aborts-or-stop-hangs). |

Only one copy of the service runs. A second `sc.exe start` fails with error 1056
(already running).

While the state is `START_PENDING` or `STOP_PENDING`, repeat `sc.exe query
EtwDnstap`. A checkpoint that does not advance before the wait hint passes
indicates a hang. The progress rules are in
[Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines).

## The receiver is down or unreachable

A down receiver does not stop the service. With a spool, frames wait on disk. Without
one, the sink counts undeliverable frames and retries with backoff. Timing is in
[Receiver connection](configuration.md#receiver-connection).

1. **At startup:** Event ID 3, `dnstap receiver unavailable at startup`. The service
   reaches `RUNNING`.
2. **During a run:** Event ID 3, `dnstap receiver connection lost`, then Event ID 3,
   `dnstap reconnect failed`. Event ID 2 logs `dnstap receiver reconnected` when the
   session returns.
3. **Check the counters.** `receiver_connected` is `false` while down.
   `receiver_disconnects` and `receiver_reconnect_failures` count the failures. With
   a spool, `spool_bytes` rises while down, and `replayed` rises after the return.
   Use `spool_bytes` for the spool size, because a folder listing shows the open
   segment as 0 bytes. Without a spool, `disconnected_dropped` shows the loss.

**Wrong content type after startup.** Each reconnect attempt fails, and the service
keeps running. Check that `receiver_connected` stays `false`, then fix the
collector's content type. Only the first connection at startup stops the service.

**Receiver rejects a down start.** If the first reachable attempt fails the TLS or
Frame Streams handshake, the service logs one Event ID 4 error and retries with backoff.

- A TLS rejection has one of these causes:
  - The Windows trust store does not trust the certificate.
  - The name does not match `--tls-server-name` (or the receiver IP address).
  - The receiver requires a client certificate. The service sends none, so the alert is
    `received fatal alert: CertificateRequired`.
- A Frame Streams rejection means the receiver does not accept `protobuf:dnstap.Dnstap`.

**Shutdown while disconnected.** The service logs `shutting down while the dnstap
receiver is disconnected; no STOP sent` and exits with code 0. Spooled frames stay on
disk for the next start.

With a spool, `spool_available` should read `true`, and `spool_bytes` should fall
after the receiver returns while `replayed` rises. If `spool_bytes` keeps rising while
`receiver_connected` is `true`, see [The receiver is slow](#the-receiver-is-slow).
Otherwise, check the collector's listener, port, content type, and the firewall.

## The receiver is slow

When the link queue (256 blocks, about 16 MiB) is full, the router moves it to the
spool and writes new blocks to the spool. Signs: `spooled` and `spool_bytes` rise while `receiver_connected` stays
`true`.

Frames still drop when a socket write stalls longer than the queue lasts
(`queue_dropped`), or when the spool refuses blocks (`spool_dropped`). With no spool,
a full queue drops frames in `queue_dropped`.

These values show the state now. `bytes_sent` is a total, so read its rate. The two
peaks cover the last 60 seconds ([definitions](monitoring.md#output-counters)).

| Value | Reading | Means |
| --- | --- | --- |
| `link_saturated` | `true` | The spool backlog is larger than 10 samples ago (at least 10 seconds), with at least 7 of the 10 steps rising, and the receiver is connected. The receiver or the network path takes fewer bytes than the service captures. It keeps its value while the receiver is down, so read it with `receiver_connected`. |
| `bytes_sent` | Rate below the capture byte rate | The link takes less than arrives. No counter reports the capture byte rate: it is the `bytes_sent` rate plus the growth of `spool_bytes` plus the `spool_dropped_bytes` rate. |
| `spool_oldest_age_ms` | Large, and rising | The data that the link takes now is at most this old. At the cap, the spool deletes its oldest segments, which hold the oldest data. |
| `send_queue_depth_max` | Near 256 | The router falls behind: the spool disk is slow, the router thread did not run, or late ETW events arrived in one burst. It is not the receiver. At 256 the parser drops frames. |
| `write_max_recent_us` | 1,000,000 or more | A socket write blocks now. A write that stalls for 2 seconds ends the session. `write_max_us` is the maximum since the start, so it cannot show this. |
| `router_wait_max_recent_us` | Far above 50,000 | The router thread woke late from its wait of at most 50 ms. The thread did not get a processor in time: look for CPU contention in the guest or on the host. |
| `router_pass_max_recent_us` | Large | A call in a router pass blocked or ran long, such as a spool disk write. |

When `link_saturated` becomes `true`, the service writes the warning `dnstap link is
saturated` (Event ID 3). It gives `backlog_bytes`, `oldest_age_ms`, and
`bytes_sent_per_second`. The event `dnstap link is no longer saturated` (Event ID 2)
ends it. A receiver that is down does not set the state. See
[the receiver section](#the-receiver-is-down-or-unreachable).

Check these in order:

1. `link_saturated` and the `bytes_sent` rate. If the state is `true`, the link does not
   keep up. At the cap the spool removes its oldest data. In steady state the byte loss
   is the `spool_dropped_bytes` rate, and the frame loss is the `spool_dropped` rate. The
   time to the cap is `--spool-max-mib` minus `spool_bytes`, divided by the growth of
   `spool_bytes`.
   The state does not change how frames are routed.
2. `queue_dropped`. With a spool, a rise means a socket or disk write blocked the
   sender longer than the queue lasts. Check `write_max_recent_us`, `slow_writes`, the
   disk, `spool_available`, and `spool_dropped`.
3. `spool_bytes` against `--spool-max-mib`, and `spool_oldest_age_ms`. If they grow for
   the whole run, the receiver takes less than the input rate. At the cap, the spool
   removes its oldest data, and `spool_dropped` shows the loss.
4. `write_max_recent_us` and `slow_writes`. Large values point to the receiver or the
   network path. Small values with a saturated state point to the spool disk or the
   sender thread, not to a blocked socket.
   If `queue_dropped` rose while the CPU use stayed low, compare `router_wait_max_recent_us`
   (a late wake) with `router_pass_max_recent_us` (a blocking call).
5. The spool disk. At 200,000 queries per second, the spool writes about 41 MB per
   second. See [Spool sizing](tuning.md#spool-sizing).
6. The collector's CPU, disk, and input queue.

## Spool warnings

Event fields, and the repeat rule for messages that log more than once, are in
[Spool messages](monitoring.md#spool-messages). No spool event contains frame data.

- **`spool folder is not usable`** (Event ID 3, at startup). Find the `reason` field
  below and follow its action.

  | `reason` | Fix |
  | --- | --- |
  | ``the spool folder path is not absolute``, ``the spool folder path has a `.` or `..` component``, or ``the spool folder path is not on a local drive letter (network, device, and other path forms are not accepted)`` | Report it. The default path always has a valid shape. |
  | ``the spool folder is not on a fixed local disk (drive type {code})`` | Move it to a fixed local disk, and set `--spool-dir`. |
  | ``the spool folder has no parent folder`` | Choose a folder with a parent, not a drive root. |
  | ``the spool folder does not exist`` | Create it. See [Create the spool folder](install.md#create-the-spool-folder) or [Add the spool folder](upgrade.md#add-the-spool-folder). |
  | ``the spool folder path is not a folder`` | Inspect the file that has that name. |
  | ``the {which} is a reparse point`` (`{which}` is `spool folder` or `parent of the spool folder`) | Inspect the link or junction. Do not follow it. |
  | ``the spool folder owner is not SYSTEM or Administrators`` | Run `icacls` on the folder. Do not take ownership. Recreate the folder with the install steps. |
  | ``the spool folder has no access control list, so every account can read and write it`` | Recreate the folder with the install steps. |
  | ``the spool folder permissions let an account other than SYSTEM, Administrators, or a service read or write it`` | Run `icacls` on the folder. Remove each entry that is not SYSTEM, Administrators, or `NT SERVICE\EtwDnstap`. Then run `Test-SpoolFolder` as in [install.md](install.md). |
  | ``cannot check the {which}: {source}`` | A Windows call failed. `{source}` gives the Windows error. Check the disk and the path. |

- **`spool could not be opened`** (Event ID 3, at startup). Check that the disk
  responds, and that no other program holds `cursor.bin` open.
- **`dnstap spool deleted its oldest segment unsent`** (Event ID 3). The cap or the
  free-space floor removed unsent frames. Check `spool_dropped`, `spool_dropped_bytes`,
  `--spool-max-mib`, and free space.
- **`dnstap spool discarded unreadable segment data`** (Event ID 3). The segment held a
  corrupt or cut-off tail, a missing START, unreadable data, or an invalid cursor. Check
  `spool_errors`, `spool_dropped`, and the disk.
- **`dnstap spool I/O error`** (Event ID 3). A create, write, read, or delete failed.
  The `error` field gives the cause. The spool pauses for 10 seconds. Check
  `spool_errors`, the disk, programs that lock files in the folder, and the folder's ACL.
- **`dnstap spool is refusing blocks`** (Event ID 3). Free space is below
  `--spool-min-free-mib` plus one segment, and no older segment can be deleted. Check
  the `free_mib` and `needed_mib` fields. Free space on the volume, or lower
  `--spool-min-free-mib`.
- **`could not seal the dnstap spool segment`** (Event ID 3, at shutdown). The last
  segment closed without its STOP. The next start replays its frames. If the message
  repeats, check the disk.
- **`dnstap spool backlog kept on disk for the next run`** (Event ID 2, at shutdown).
  Not an error. `leftover_segments` in `spool in use` gives the count.
- **`spool in use`** (Event ID 2, at startup). Not an error. `leftover_segments` and
  `leftover_bytes` show what an earlier run left.

## The service keeps restarting

The service exits with a service-specific error in these cases. SCM restarts it under
the [recovery policy](install.md#5-configure-scm-recovery).

- **The DNS Server process changed or stopped.** Event ID 4 names the reason, for
  example `the DNS Server service restarted (process ID 4242 -> 5151)`. Event ID 1
  follows. The service checks every 5 seconds.
- **A capture worker ended on its own.** Event ID 1: `a capture worker exited before
  shutdown was requested`.
- **A receiver problem at shutdown does not fail the stop.** A STOP/FINISH exchange that does not
  complete, for example because the receiver is stalled at shutdown, does not fail the stop. The
  service writes nothing about it and stops with success, so SCM does not restart it. Unsent
  frames stay in the spool, or count in `disconnected_dropped` when no spool is in use. A panic of
  the sender or link thread ends the process at once (release builds abort on a panic), and SCM
  handles it as a crash.
- **Any other run failure.**

Restart timing and the failure-count reset are in
[5. Configure SCM recovery](install.md#5-configure-scm-recovery). Records in flight at
a failure are lost. After a DNS restart, the gap lasts until EtwDnstap restarts. If
`DNS` is not running at startup, start it.

Check the Event ID 1 entries. Check the policy with
`sc.exe qfailure EtwDnstap`, and check that `sc.exe qfailureflag EtwDnstap` reports the
non-crash flag as set.

## Capture stops during a run

Capture stops only for the reasons in
[The service keeps restarting](#the-service-keeps-restarting), or when the shutdown
exchange fails. A lost receiver connection does not stop it.

1. Find the failure text in the Application log (Event ID 1).
2. For a DNS restart, check that `foreign_pid_rejected` rose during the gap.
3. Check the recovery policy. Without it, the service stays down until someone starts it.

## Counters show drops

The `output stats` and `capture stats` lines (Event ID 2) report counters every
`--stats-interval-seconds`. `capture stopped` and `output stopped` repeat them at
shutdown. A restart resets every counter. The counter definitions are in
[Capture counters](monitoring.md#capture-counters) and
[Output counters](monitoring.md#output-counters).

| Counter | Action |
| --- | --- |
| `ring_dropped` | Raise `--capacity`. See [Defaults](tuning.md#defaults). |
| `pool_exhausted_regular`, `pool_exhausted_jumbo` | Raise `--jumbo-capacity` if `pool_exhausted_jumbo` rises. With `--jumbo-capacity 0`, every jumbo record counts here. |
| `receiver_disconnected` | Check whether the parser stopped. This is the event channel, not TCP. See [restarts](#the-service-keeps-restarting). |
| `disconnected_dropped` | Check `spool_available`. See [the receiver section](#the-receiver-is-down-or-unreachable). |
| `queue_dropped`, `link_saturated` | See [The receiver is slow](#the-receiver-is-slow). |
| `spool_dropped`, `spool_dropped_bytes` | See [Spool warnings](#spool-warnings). Raise `--spool-max-mib` if the outage outlasts the cap. |
| `spool_errors` | See [Spool warnings](#spool-warnings). |
| `foreign_pid_rejected` | Normally 0. Nonzero during a DNS restart is expected. Otherwise investigate. |
| `callback_errors` | Check the Application log. |

Do not add `etw_lost` to `etw_buffers_lost`, because the units differ. The sums and
the loss relationships are in [Counter relationships](monitoring.md#counter-relationships).

Compare your collector's record count with the client's. Treat any difference as
unexplained until you read the counters.

## Rate warning

With the default buffer settings, the service logs `received rate is above
440,000 events/s` once per run. The defaults are sized for about 400,000 events per second
(200,000 queries per second). The warning alone does not mean that events were lost.

If `ring_dropped` or `etw_lost` rises in the `capture stats` events, restart with:

```text
--etw-max-buffers 256 --capacity 262144
```

262,144 is the largest `--capacity`. It uses 558.03 MiB of counted storage at 64 jumbo slots, and the
startup deadline is 26 seconds. The lab ran 200,000 queries per second with it and with the
default 131,072. See [Defaults](tuning.md#defaults). Lab runs are in
[200,000 QPS sizing runs on 12 vCPUs](../history/slow-receiver-validation.md#200000-qps-sizing-runs-on-12-vcpus-2026-11-04)
and [Guide lab results, 2026-11](../history/guide-lab-results-2026-11.md).

## ETW consumer and session warnings

- **`ETW consumer did not return within 5 s of STOP`.** The service closes the trace
  handle to force the worker out. Events still in ETW buffers may not arrive, and
  `etw_lost` may not count them. Check the host for overload.
- **`stale ETW session used a different buffer size; reclaiming anyway`.** Not an
  error. See [Stale ETW session](#stale-etw-session-blocks-startup).

## Rejected events

- **`rejected ETW event`.** The `rejected` field is the running count. The message
  contains no payloads or names.
- **`DNS event schema load failed`.** The message ends with the Win32 error, often because the
  provider manifest is not yet available.
  Events of that (event ID, version) key count in `schema_rejected` and `rejected`.
  The schema loads again after 65,536 more rejected events. The fields are listed in
  [Receiver, schema, and rate messages](monitoring.md#receiver-schema-and-rate-messages).
  If the events stay rejected, check that the DNS Server role and its ETW provider are
  healthy, and restart the service.
- **`DNS event schema is unsupported and its events are rejected`.** Logged once per
  (event ID, version) key. The schema does not match the single-pass decoder grammar,
  is not the expected provider or event, or has an invalid size. The key is not
  retried. A new Windows build or DNS Server manifest can cause this. Report the
  warning text. Capturing these events needs a decoder change.

## No records arrive at the collector

Check the layers in order.

1. **Is the service connected?** Look for a `capture started` line (Event ID 2).
   In the latest `output stats` line, `receiver_connected` should be `true`.
2. **Does the DNS Server write the events?** Event IDs 256, 257, and 258 come from
   `Microsoft-Windows-DNSServer`. Recursive traffic does not produce client records.
3. **Is the DNS process still the one filtered?** A rising `foreign_pid_rejected`
   shows records from a new DNS process being refused until EtwDnstap restarts.
4. **Is the subscription the one you expect?** `capture started` shows the keyword
   masks. A fallback warning means the subscription uses `0x8000000000000000`. See
   [configuration.md](configuration.md#subscription).
5. **Does the collector's filter or pipeline match the example?** See
   [receiver.md](receiver.md#receiver-configuration).

## Stale ETW session blocks startup

A session left by a crashed or aborted service is reclaimed at startup when its name,
shape, and GUID match. Buffer size does not matter. The rules are in
[Session reclaim](configuration.md#session-reclaim).

If startup still reports `already exists and does not match this program's session
shape`, another logger mode, a log file, or a different GUID owns the session. The
service does not stop it. Find the owner, then stop the session:

```powershell
logman query -ets
logman stop EtwDnstap -ets
```

Use your `--session-name` in place of `EtwDnstap`. Stop a session named
`etw-dnstap-<pid>` from an earlier build only if you can identify it as this
service's. If a reclaim fails, Event ID 1 says
`query existing ETW session '<name>' failed` or `stopping stale ETW session '<name>' failed`.

## Watchdog aborts or stop hangs

The deadlines and their values are in
[Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines).

If a deadline passes, the service writes Event ID 5 and aborts the process. The
message names the phase, for example `the startup deadline of 12 s expired`. Before
the service reads its arguments, the message says `the startup deadline of 10 s
expired`.

The abort does not free resources that native calls still use, so the capture can be
incomplete and a real-time ETW session can remain. Check `logman query -ets`, then stop
that session or restart the computer. A timeout does not prove that the native operation
never finished. Shutdown does not wait for a spool backlog.

The service accepts Stop and Shutdown only in `RUNNING`, so SCM refuses a stop during
`START_PENDING` and `STOP_PENDING`. It also rejects other control codes, such as 200.

## A panic aborts the process

A panic writes Event ID 6, starting with `EtwDnstap panicked:`, and then aborts the
process. The abort skips cleanup. Check for a leftover ETW session with
`logman query -ets`. Report the Event ID 6 text with the steps that led to it.

## Upgrading

- A removed option (`--output`, `--max-output-bytes`, `--metrics`, `--etw-flush-ms`)
  fails command-line parsing, and the service does not start. Remove it from the
  binary path. To replace `--etw-flush-ms`, see
  [High-rate settings](tuning.md#high-rate-settings).
- After an upgrade, the service logs `spool folder is not usable` until the spool
  folder exists. Create it as in [upgrade.md](upgrade.md#add-the-spool-folder). Also
  register the new counter manifest, or the spool counters do not appear.
- A service still running as LocalSystem or the virtual account refuses to start.
  Follow the ordered steps in [upgrade.md](upgrade.md#repair-cases).
- Record identity, privilege, and ACL changes against the binary's commit.

## Still stuck

Collect these before asking for help. Redact the receiver address and query names
first.

- The Application log entries for source `EtwDnstap`, with the time range.
- The output of `sc.exe qc EtwDnstap`, `sc.exe qsidtype EtwDnstap`,
  `sc.exe qprivs EtwDnstap`, `sc.exe qfailure EtwDnstap`, and
  `sc.exe qfailureflag EtwDnstap`.
- The members of `net localgroup "Performance Log Users"`, and the SDDL printed by
  `scripts\provision\Grant-EtwDnstapEtwAccess.ps1`.
- The `logman query -ets` list.
- The `output stats` line with the spool counters. Do not send spool files, which hold
  client addresses and query names.
- The commit SHA, and whether the build is a release or a diagnostic (`diag-timing`)
  build.

For security issues, follow [SECURITY.md](../../SECURITY.md), not a public issue.
