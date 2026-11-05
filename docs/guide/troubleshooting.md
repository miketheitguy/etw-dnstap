# Troubleshooting

> **Status:** Current · **Describes:** the code at commit `66c3de4` · **Reviewed:** 2026-11-05

Use this guide to find the cause of a failed start, restarts, drops, or missing
records. Start with the Application event log and the SCM state. For a log message,
use [Event log messages](#event-log-messages).

## Read the diagnostics

Diagnostics go to the Application log, source `EtwDnstap`. Event ID 1 is a fatal failure,
5 a watchdog abort, 6 a panic, 10 to 16 data loss, and 20 to 28 a risk of loss. See
[Log event IDs](monitoring.md#log-event-ids).

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

A console start exits with code 1 and prints nothing. Read the version from the `version`
field of the `capture started` event, or run `(Get-Item <exe>).VersionInfo`.

## Event log messages

A fatal failure (Event ID 1) starts with `EtwDnstap failed:`. Search this page for the
rest of the text. Messages with no section of their own:

- `ETW keyword narrowing unavailable` (3): the subscription uses the fallback keyword mask.
- `RESPONSE_FAILURE keyword is not available` (3): failed responses are not captured. See
  [configuration.md](configuration.md#subscription).
- `performance counters are unavailable` (3): capture continues. See
  [If the counters do not register](monitoring.md#if-the-counters-do-not-register).
- `data loss in ...` (ETW, at capacity, on the network path, in the spool, in decoding,
  inside the service) is Event ID 10 to 16. `... is near full`, `... near the maximum`,
  `... near its limit`, `the dnstap receiver is down and no spool takes frames`, and
  `the ETW statistics query is failing` are 20 to 22, 24, 25, and 28. See
  [Event IDs for data loss](#event-ids-for-data-loss).

## The service does not reach RUNNING

`sc.exe query EtwDnstap` shows `START_PENDING`, then `STOPPED`, with `WIN32_EXIT_CODE`
1066 and `SERVICE_EXIT_CODE` 1. Each cause below logs an Event ID 1. Check them in order.

| Event ID 1 text | Cause and fix |
| --- | --- |
| `invalid service arguments` | Unknown option or malformed `--spool-dir`. Fix the binary path. `--spool-dir` needs an absolute path with a drive letter and no `.` or `..` part. See [configuration.md](configuration.md). |
| `invalid service arguments`, then `session name must be 1 to 128 ASCII letters, digits, underscore, dot, or hyphen, not starting with a hyphen`, with the reason appended | Invalid session name. Use a valid name. See [configuration.md](configuration.md#options). |
| `must run as NT AUTHORITY\LocalService` | The token user is not S-1-5-19. Run `cmd.exe /c 'sc.exe config EtwDnstap obj= "NT AUTHORITY\LocalService" password= ""'`. Use `cmd.exe`, because Windows PowerShell 5.1 drops the empty password. Then run `sc.exe sidtype EtwDnstap restricted`. |
| `the service SID NT SERVICE\EtwDnstap is not an enabled group` or `the process token is not restricted` | The SID type is none or unrestricted, or the group is disabled or deny-only. Run `sc.exe qsidtype EtwDnstap`. If it reports `NONE`, run `sc.exe sidtype EtwDnstap restricted`. |
| `restricting SIDs do not include the service SID` or `the Write Restricted SID (S-1-5-33)` | The token lacks the service SID or `S-1-5-33`. Run `sc.exe sidtype EtwDnstap restricted`. Compare the token with [SECURITY.md](../../SECURITY.md), and record a different set. |
| `lacks an enabled Performance Log Users` | The service SID is missing from the group, or deny-only. Run `net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add`. |
| `configure SCM RequiredPrivileges to only SeChangeNotifyPrivilege` or `requires only an enabled SeChangeNotifyPrivilege` | Privileges are extra, missing, or disabled. Run `sc.exe privs EtwDnstap SeChangeNotifyPrivilege`, then check with `sc.exe qprivs EtwDnstap`. |
| `connect/negotiation with dnstap receiver` with `did not accept dnstap content type` | Set the collector's content type to `protobuf:dnstap.Dnstap`. |
| `connect/negotiation with dnstap receiver` with `TLS handshake`, or `received fatal alert: CertificateRequired` | The receiver certificate is rejected. Import the receiver CA into `Cert:\LocalMachine\Root`. Name the receiver IP address on its certificate, or set `--tls-server-name`. Drop client certificate requirements. See [the receiver certificate step](receiver.md#receiver-certificate). |
| `StartTraceW: access denied` | The service SID lacks `TRACELOG_CREATE_REALTIME` on the session GUID in the message. Run `scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` with the name from the message (case-sensitive), then restart the computer. See [install.md](install.md#if-an-etw-access-check-is-denied). |
| `EnableTraceEx2: access denied` | The DNS provider GUID has its own descriptor that grants neither group `TRACELOG_GUID_ENABLE`. Do not add a grant on it. See [install.md](install.md#if-an-etw-access-check-is-denied). |
| A session with the same name cannot be reclaimed | See [Stale ETW session](#stale-etw-session-blocks-startup). |
| No Event ID 1. Event ID 5 names the startup deadline. | See [Watchdog aborts](#watchdog-aborts-or-stop-hangs). |

A second `sc.exe start` fails with error 1056 (already running). In `START_PENDING` or
`STOP_PENDING`, a checkpoint that does not advance before the wait hint passes indicates a
hang. See [Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines).

## The receiver is down or unreachable

The service keeps running. With a spool, frames wait on disk. Without one, the sink counts
undeliverable frames and retries with backoff. Timing is in
[Receiver connection](configuration.md#receiver-connection).

- **At startup:** Event ID 24, `dnstap receiver unavailable at startup`. The service
  reaches `RUNNING`.
- **During a run:** Event ID 24, `dnstap receiver connection lost`, then Event ID 3,
  `dnstap reconnect failed` (`cause` is `connect`, `timeout`, `tls`, or `protocol`).
  Event ID 2, `dnstap receiver reconnected`, marks the return. After `could not start
  the dnstap reconnect thread` or `dnstap reconnect thread ended without a result`
  (Event ID 3 or 4), the service retries with backoff.
- **Counters:** `receiver_connected` is `false` while down. `receiver_disconnects` and
  `receiver_reconnect_failures` count the failures. With a spool, `spool_bytes` rises
  while down and `replayed` rises after the return. Read `spool_bytes`, because a folder
  listing shows the open segment as 0 bytes. Without a spool, `disconnected_dropped`
  shows the loss, the service writes Event ID 24 `the dnstap receiver is down and no
  spool takes frames`, then Event ID 13.

**Wrong content type after startup.** Each reconnect fails and `receiver_connected` stays
`false`. Fix the collector's content type. Only the first connection at startup stops the
service.

**Handshake rejected.** The first reachable attempt logs one Event ID 4 error, `dnstap
receiver failed the TLS handshake before the first session` or `dnstap receiver refused
the Frame Streams handshake before the first session`, and retries with backoff.

- TLS causes: the Windows trust store does not trust the certificate, the name does not
  match `--tls-server-name` (or the receiver IP address), or the receiver requires a
  client certificate (`received fatal alert: CertificateRequired`).
- Frame Streams cause: the receiver does not accept `protobuf:dnstap.Dnstap`.

**Shutdown while disconnected.** `output stopped` reads `stop_sent=false`, the service
exits with code 0, and spooled frames stay on disk (`spool_backlog_kept=true`).

**Recovery check.** `spool_available` reads `true`, and `spool_bytes` falls while
`replayed` rises. If `spool_bytes` rises with `receiver_connected` `true`, see
[The receiver is slow](#the-receiver-is-slow). Otherwise, check the collector's listener,
port, content type, and the firewall.

## The receiver is slow

**Symptom:** `spooled` and `spool_bytes` rise while `receiver_connected` stays `true`.
**Cause:** the link queue (256 blocks, about 16 MiB) is full, so the router moves it to
the spool and writes new blocks there. Frames drop (`queue_dropped`) when a socket write
stalls longer than the queue lasts, or when there is no spool and the queue is full. They
drop (`spool_dropped`) when the spool refuses blocks.

Read the `bytes_sent` rate, not the total. The peaks cover the last 60 seconds
([definitions](monitoring.md#output-counters)).

| Value | Reading | Means |
| --- | --- | --- |
| `link_saturated` | `true` | The spool backlog grew over 10 samples (at least 10 seconds), with at least 7 of 10 steps rising, and the receiver is connected. It keeps its value while the receiver is down, so read it with `receiver_connected`. |
| `bytes_sent` | Rate below the capture byte rate | The capture byte rate is the `bytes_sent` rate plus the growth of `spool_bytes` plus the `spool_dropped_bytes` rate. |
| `spool_oldest_age_ms` | Large, and rising | The link now sends data at most this old. At the cap, the spool deletes its oldest segments. |
| `send_queue_depth_max` | Near 256 | The router falls behind: slow spool disk, a starved router thread, or a burst of late ETW events. At 256 the parser drops frames. |
| `write_max_recent_us` | 1,000,000 or more | A socket write blocks now. The session ends when the receiver accepts no bytes for `--receiver-stall-seconds` (default 15) while data waits. (`write_max_us` is the maximum since the start.) |
| `router_wait_max_recent_us` | Far above 50,000 | The router woke late from its wait of at most 50 ms. Look for CPU contention in the guest or on the host. |
| `router_pass_max_recent_us` | Large | A router call blocked or ran long, such as a spool disk write. |

`link_saturated` turning `true` logs `dnstap link is saturated` (Event ID 23) with
`backlog_bytes`, `oldest_age_ms`, and `bytes_sent_per_second`. `dnstap link is no longer
saturated` (Event ID 2) ends it, also when the backlog is empty or the spool is
suspended.

Check in order:

1. `link_saturated` and the `bytes_sent` rate. At the cap, the byte loss is the
   `spool_dropped_bytes` rate and the frame loss is the `spool_dropped` rate. The time to
   the cap is (`--spool-max-mib` minus `spool_bytes`) divided by the growth of `spool_bytes`.
2. `queue_dropped` with a spool: a socket or disk write blocked the sender longer than the
   queue lasts. Check `write_max_recent_us`, `slow_writes`, the disk, `spool_available`, and
   `spool_dropped`. If CPU use stayed low, compare `router_wait_max_recent_us` with
   `router_pass_max_recent_us`.
3. Large `write_max_recent_us` and `slow_writes` point to the receiver or the network
   path. Small values with a saturated state point to the spool disk (about 41 MB per
   second at 200,000 queries per second; see [Spool sizing](tuning.md#spool-sizing)).
4. The collector's CPU, disk, and input queue.

## Spool warnings

Event fields and the repeat rule are in [Spool messages](monitoring.md#spool-messages).

**`spool folder is not usable`** (Event ID 26, at startup). Follow the fix for the `reason`
field:

| `reason` | Fix |
| --- | --- |
| ``the spool folder path is not absolute``, ``the spool folder path has a `.` or `..` component``, or ``the spool folder path is not on a local drive letter (network, device, and other path forms are not accepted)`` | Report it. The default path is always valid. |
| ``the spool folder is not on a fixed local disk (drive type {code})`` | Move it to a fixed local disk, and set `--spool-dir`. |
| ``the spool folder has no parent folder`` | Choose a folder, not a drive root. |
| ``the spool folder does not exist`` | Create it. See [Create the spool folder](install.md#create-the-spool-folder) or [Add the spool folder](upgrade.md#add-the-spool-folder). |
| ``the spool folder path is not a folder`` | Inspect the file with that name. |
| ``the {which} is a reparse point`` (`{which}` is `spool folder` or `parent of the spool folder`) | Inspect the link or junction. Do not follow it. |
| ``the spool folder owner is not SYSTEM or Administrators`` | Run `icacls` on the folder. Recreate the folder with the install steps, without taking ownership. |
| ``the spool folder has no access control list, so every account can read and write it`` | Recreate the folder with the install steps. |
| ``the spool folder permissions let an account other than SYSTEM, Administrators, or a service read or write it`` | Run `icacls` and remove each entry other than SYSTEM, Administrators, and `NT SERVICE\EtwDnstap`. Then run `Test-SpoolFolder` as in [install.md](install.md). |
| ``cannot check the {which}: {source}`` | A Windows call failed (`{source}`). Check the disk and the path. |

Other messages:

- **`spool is disabled (--no-spool)`** (Event ID 26, at startup). Every receiver outage
  loses frames (`disconnected_dropped`). Remove the option.
- **`spool key is not usable`** (Event ID 26, at startup). The service runs without a
  spool. See [Spool key errors](#spool-key-errors).
- **`spool could not be opened`** (Event ID 26, at startup). Check the disk, and that no
  other program holds `cursor.bin` open.
- **`dnstap spool deleted segments it cannot read at startup`** (Event ID 14, once at
  startup). The service deleted segments it cannot decrypt and counted their bytes in
  `spool_dropped_bytes`. The fields are `segments`, `bytes`, `earlier_format` (unreadable
  format, or not segments), and `other_key` (sealed with another spool key).
  `new_key=true` is expected once after you force a new key. Otherwise, check whether
  something replaced `WrappedKey` or copied segment files from another server.
- **`dnstap spool deleted its oldest segment unsent`** (Event ID 14). The cap or the
  free-space floor removed unsent frames. Check `spool_dropped`, `--spool-max-mib`, and
  free space.
- **`dnstap spool discarded unreadable segment data`** (Event ID 14). A record failed
  authentication, was cut off, or had a wrong header, an invalid frame, or an invalid
  cursor. Check `spool_errors`, `spool_dropped`, and the disk.
- **`dnstap spool I/O error`** (Event ID 27). A create, write, read, or delete failed (see
  `error`), and the spool pauses for 10 seconds. Check `spool_errors`, the disk, programs
  that lock files in the folder, and the folder's ACL.
- **`dnstap spool is refusing blocks`** (Event ID 14). Free space is below
  `--spool-min-free-mib` plus one segment, and no older segment can be deleted. See
  `free_mib` and `needed_mib`. Free space, or lower `--spool-min-free-mib`.
- **`the spool is near its limit`** (Event ID 25). The spool holds 80 percent of
  `--spool-max-mib`, or the volume is within 2 segments of the free-space floor. Free
  space, or raise `--spool-max-mib`. See [Spool sizing](tuning.md#spool-sizing).
- **`spool_seal_error` in `output stopped`** (Event ID 2). The last segment closed without
  its end record, and the next start replays its frames. If it repeats, check the disk.
- **`spool in use`** (Event ID 2, at startup). `leftover_segments` and `leftover_bytes`
  show what an earlier run left. `new_spool_key=true` means the service made a new key.

### Spool key errors

The AES spool key lives in the registry value
`HKLM\SOFTWARE\ETW-DNSTapService\Protect\WrappedKey`, wrapped by the installer's CNG key
`EtwDnstap Spool Key`. See [Spool encryption key](install.md#spool-encryption-key). The
`reason` field of `spool key is not usable` names the failed step:

| `reason` starts with | Cause | Fix |
| --- | --- | --- |
| ``cannot open the CNG key `EtwDnstap Spool Key` `` with `0x80090016` | `NTE_BAD_KEYSET`: the key is missing, or the service account cannot use it. | Repair the MSI, which recreates the key and its DACL, then restart the service. If the repair made a new key, the service logs `cannot unwrap the spool key`: force a new key. |
| ``the key store did not answer within 3s`` | The Key Storage Provider was late (for example, `KeyIso` was starting). No spool this run. | Restart the service when `KeyIso` is running. If it repeats, check that `EtwDnstap` depends on `KeyIso`. |
| ``cannot open the CNG key`` with another code, or ``cannot unwrap the spool key`` | The key is unusable, or another CNG key made `WrappedKey`. | Repair the MSI. If the error stays, force a new key. |
| ``cannot open the registry key`` with `Win32 error 2` | The `HKLM\SOFTWARE\ETW-DNSTapService\Protect` key is missing. | Repair the MSI. |
| ``cannot open the registry key``, ``cannot read``, or ``cannot write the `WrappedKey` registry value`` with `Win32 error 5` | The subkey DACL lacks query value and set value for the service SID. | Repair the MSI. |
| ``the `WrappedKey` registry value is not valid`` | The value is damaged or was written by hand. | Force a new key. |
| ``cannot make random spool key bytes`` | The system random number generator failed. | Restart the service. If it repeats, report it. |
| ``cannot allocate a page for the spool key`` or ``cannot lock the spool key page in memory`` | `VirtualAlloc` or `VirtualLock` failed, usually from a used-up working set quota (`Win32 error 1453`). | Restart the service. If it repeats, report it. |

**Force a new key.** Delete the value and restart the service:

```powershell
Stop-Service EtwDnstap
Remove-ItemProperty -Path 'HKLM:\SOFTWARE\ETW-DNSTapService\Protect' -Name WrappedKey
Start-Service EtwDnstap
```

The service makes and stores a new key, deletes every segment (the new key cannot read
them), resets `cursor.bin`, and logs `dnstap spool deleted segments it cannot read at
startup` with `new_key=true`. Frames in the spool are lost, and their bytes count in
`spool_dropped_bytes`.

**Run-time memory protection failures.** The key bytes stay encrypted in memory with
`CryptProtectMemory` (see [Encryption](configuration.md#encryption)).

- **`CryptUnprotectMemory` fails.** The service keeps the key. The append or read fails
  as a transient spool error: it counts in `spool_errors`, logs `dnstap spool I/O error`
  (Event ID 27), and pauses the spool for 10 seconds. Restart the service if it repeats.
- **`CryptProtectMemory` fails twice, or the decrypted bytes fail the key check.** The
  service zeroes the key bytes and suspends the spool until it restarts. It logs one
  warning (Event ID 26), in one of two forms:

  ```text
  the spool key is unusable after a memory protection failure (CryptProtectMemory, Win32 error N); the spool is suspended until the service restarts; spooled files are kept
  the spool key is unusable after a memory protection failure (CryptUnprotectMemory returned bytes that fail the key check); the spool is suspended until the service restarts; spooled files are kept
  ```

  While the spool is suspended:

  - Appends are refused and count in `spool_dropped` only. A refused append deletes no
    segment, except in one rare case: the size cap evicts the active segment and the key
    fails as the block is sealed again.
  - Reads stop and drop nothing. `spool_available` is false.
  - Replay batches the link already took are acknowledged as usual. Nothing else is
    deleted, and the cursor does not move.
  - Live frames go straight to the receiver when it is up. When it is down they are
    refused and counted, and a full send queue drops frames (`queue_dropped`).
  - `link_saturated` clears with the reason `the spool is suspended until the service
    restarts`.

  Restart the service. The next start unwraps the key and replays the whole kept backlog.

## The service keeps restarting

SCM restarts the service under the [recovery policy](install.md#5-configure-scm-recovery)
when it exits with a service-specific error:

- A capture worker ends on its own: Event ID 1, `a capture worker exited before shutdown
  was requested`.
- Any other run failure.
- A panic of the sender or link thread. Release builds abort, so SCM sees a crash.

A STOP/FINISH exchange that does not complete (for example, a stalled receiver) ends with
success and no restart. Unsent frames stay in the spool, or count in
`disconnected_dropped` without one.

Records in flight at a failure are lost. A DNS Server restart is not a failure. Check the
Event ID 1 entries, the policy with `sc.exe qfailure EtwDnstap`, and that
`sc.exe qfailureflag EtwDnstap` reports the non-crash flag as set.

## Capture stops during a run

Find the failure text in the Application log (Event ID 1). See
[The service keeps restarting](#the-service-keeps-restarting). Without a recovery policy,
the service stays down until someone starts it.

## Event IDs for data loss

Fields and rules are in [Loss and risk warnings](monitoring.md#loss-and-risk-warnings).
A restart resets every counter. For the exact count, read the counters (see
[Counters show drops](#counters-show-drops)).

Loss, written after the data is gone:

| Event ID | Cause | Action |
| --- | --- | --- |
| 10 | ETW dropped events or lost buffers (`etw_lost`, `etw_buffers_lost`). | Raise `--etw-max-buffers` and `--etw-buffer-kib`, and check the host for CPU starvation. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). |
| 11 | Event channel full (`ring_dropped`). | Raise `--capacity`. |
| 12 | No pool slot free (`pool_exhausted_regular`, `pool_exhausted_jumbo`). | Raise `--capacity`, or `--jumbo-capacity` when `pool_exhausted_jumbo` rose. With `--jumbo-capacity 0`, every jumbo record counts. |
| 13 | Network path (`disconnected_dropped`, `queue_dropped`). | `disconnected_dropped`: check `spool_available` and see [the receiver is down](#the-receiver-is-down-or-unreachable). `queue_dropped`: see [the receiver is slow](#the-receiver-is-slow). |
| 14 | Spool removed, skipped, or refused data (`spool_dropped`, `spool_dropped_bytes`, `spool_errors`). | Free disk space, or raise `--spool-max-mib` if the outage outlasts the cap. See [Spool warnings](#spool-warnings). |
| 15 | Events rejected (`rejected`, `schema_rejected`). | See [Rejected events](#rejected-events). |
| 16 | Loss inside the service (`callback_errors`, `receiver_disconnected`, `output_failed_events`). | Check the Application log. `receiver_disconnected` counts the event channel, not TCP: check whether the parser stopped, and see [restarts](#the-service-keeps-restarting). Report the steps that led to it. |

Risk, written before data is lost. Each has an Information event (ID 2) when it ends,
except 26 and 27:

| Event ID | Risk | Action |
| --- | --- | --- |
| 20 | Event channel peak at 80 percent of `--capacity`. | Raise `--capacity`. |
| 21 | Allocated ETW buffers at 90 percent of the maximum. | Raise `--etw-max-buffers`. |
| 22, 23 | Send queue peak at 80 percent of its 256 blocks (22), or link saturated (23). | See [the receiver is slow](#the-receiver-is-slow). |
| 24 | Receiver down. With no spool taking frames, frames are being lost. | See [the receiver is down](#the-receiver-is-down-or-unreachable). |
| 25 | Spool at 80 percent of its cap, or the volume near the free-space floor. | Free disk space, or raise `--spool-max-mib`. |
| 26 | No spool in use at start (`--no-spool` or an unusable folder). | Remove `--no-spool`, or fix the folder. |
| 27 | Spool I/O error. The spool pauses for 10 seconds. | Check the disk and the folder. |
| 28 | The ETW statistics query fails, so ETW loss is not visible. | Check that the session runs. See [ETW consumer and session warnings](#etw-consumer-and-session-warnings). |

For settings, see [Defaults](tuning.md#defaults) and [Spool sizing](tuning.md#spool-sizing).

## Counters show drops

The `output stats` and `capture stats` lines (Event ID 2) report counters every
`--stats-interval-seconds`, and `capture stopped` and `output stopped` repeat them at
shutdown. Find the counter in the Event ID tables above. Definitions are in
[Capture counters](monitoring.md#capture-counters) and
[Output counters](monitoring.md#output-counters). Add `etw_lost` and `etw_buffers_lost`
separately, because the units differ. See
[Counter relationships](monitoring.md#counter-relationships).

Compare your collector's record count with the client's. Treat any difference as
unexplained until you read the counters.

## Rate warning

The defaults suit about 20,000 events per second (10,000 queries per second). Above
22,000 events per second the service logs `received rate is above 22,000 events/s` once
per run. The warning alone means no loss.

If `ring_dropped` or `etw_lost` rises in the `capture stats` events, use the settings for
your rate in [Recommended settings for higher rates](tuning.md#recommended-settings-for-higher-rates).
At 50,000 queries per second:

```text
--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128 --capacity 131072
```

At 100,000 queries per second use `--capacity 524288`, and at 200,000 use
`--capacity 262144`. The maximum, 524,288, has a 42-second startup deadline and a working
set of about 1.4 GB. See [Defaults](tuning.md#defaults).

## ETW consumer and session warnings

- **`ETW consumer did not return within 5 s of STOP`.** The service closes the trace
  handle to force the worker out. Events in ETW buffers may be lost without an `etw_lost`
  count. Check the host for overload.
- **`stale ETW session used a different buffer size; reclaiming anyway`.** The service
  reclaims the session. See [Stale ETW session](#stale-etw-session-blocks-startup).

## Rejected events

- **`rejected ETW event`.** The `rejected` field is the running count.
- **`DNS event schema load failed`.** The message ends with the Win32 error, often because
  the provider manifest is not yet available. Events of that (event ID, version) key count
  in `schema_rejected` and `rejected`, and the schema loads again after 65,536 more
  rejected events. If they stay rejected, check that the DNS Server role and its ETW
  provider are healthy, and restart the service. Fields are in
  [Receiver, schema, and rate messages](monitoring.md#receiver-schema-and-rate-messages).
- **`DNS event schema is unsupported and its events are rejected`.** Logged once per
  (event ID, version) key, which is not retried. The schema does not match the
  single-pass decoder grammar, is not the expected provider or event, or has an invalid
  size, for example after a new Windows build or DNS Server manifest. Report the warning
  text. Capturing these events needs a decoder change.

## No records arrive at the collector

Check in order:

1. A `capture started` line (Event ID 2) exists, and the latest `output stats` line shows
   `receiver_connected` `true`.
2. The DNS Server writes Event IDs 256, 257, and 258 (`Microsoft-Windows-DNSServer`).
   Recursive traffic produces no client records.
3. `capture started` shows the keyword masks you expect. A fallback warning means the
   subscription uses `0x8000000000000000`. See
   [configuration.md](configuration.md#subscription).
4. The collector's filter or pipeline matches the example in
   [receiver.md](receiver.md#receiver-configuration).

## Stale ETW session blocks startup

The service reclaims a session left by a crashed or aborted service when its name, shape,
and GUID match (see [Session reclaim](configuration.md#session-reclaim)). If startup
reports `already exists and does not match this program's session shape`, another logger
mode, a log file, or a different GUID owns the session. Find the owner, then stop the
session, using your `--session-name` in place of `EtwDnstap`:

```powershell
logman query -ets
logman stop EtwDnstap -ets
```

A failed reclaim logs Event ID 1 `query existing ETW session '<name>' failed` or
`stopping stale ETW session '<name>' failed`.

## Watchdog aborts or stop hangs

Deadline values are in
[Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines).

A passed deadline writes Event ID 5, starting `EtwDnstap watchdog: the startup deadline`
or `shutdown deadline`, and aborts the process. With the default `--capacity` of 131072
the startup deadline is 18 s and the message says `the startup deadline of 18 s expired`.
The deadline grows with `--capacity`, and is 10 s before the service reads its arguments.

The abort can leave an incomplete capture and a real-time ETW session. Run
`logman query -ets`, then stop that session or restart the computer. Shutdown does not wait
for a spool backlog.

The service accepts Stop and Shutdown only in `RUNNING`, so SCM refuses a stop during
`START_PENDING` and `STOP_PENDING`, and rejects other control codes, such as 200.

## A panic aborts the process

A panic writes Event ID 6, starting with `EtwDnstap panicked:`, and then aborts the
process without cleanup. Check for a leftover ETW session with `logman query -ets`.
Report the Event ID 6 text with the steps that led to it.

## Still stuck

Collect these before asking for help, with the receiver address and query names redacted:

- The Application log entries for source `EtwDnstap`, with the time range.
- The output of `sc.exe qc EtwDnstap`, `sc.exe qsidtype EtwDnstap`,
  `sc.exe qprivs EtwDnstap`, `sc.exe qfailure EtwDnstap`, and
  `sc.exe qfailureflag EtwDnstap`.
- The members of `net localgroup "Performance Log Users"`, and the SDDL printed by
  `scripts\provision\Grant-EtwDnstapEtwAccess.ps1`.
- The `logman query -ets` list.
- The `output stats` line with the spool counters. Spool files hold client addresses and
  query names, so send none.
- The commit SHA, and whether the build is a release or a diagnostic (`diag-timing`)
  build.

For security issues, follow [SECURITY.md](../../SECURITY.md), not a public issue.
