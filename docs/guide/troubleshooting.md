# Troubleshooting

> **Status:** Current · **Describes:** the code on `main` at commit `592a040` · **Reviewed:** 2026-11-05

### Quick Start

```powershell
# 1. Read the fatal failures (Event ID 1)
$eventFilter = @{LogName='Application'; ProviderName='EtwDnstap'; Id=1}
Get-WinEvent -FilterHashtable $eventFilter -MaxEvents 10 |
    Select-Object TimeCreated, Id,
        @{Name='Details'; Expression={$_.Properties[0].Value}}

# 2. Check the service
sc.exe query EtwDnstap

# 3. Check the counters (output stats, capture stats)
$eventFilter.Id = 2
Get-WinEvent -FilterHashtable $eventFilter -MaxEvents 4 | Select-Object TimeCreated, Message
```

- Event ID 1 is fatal, 5 a watchdog abort, 6 a panic, 10 to 16 data loss, 20 to 28 a risk of loss ([Log event IDs](monitoring.md#log-event-ids)).
- No events? Check `[System.Diagnostics.EventLog]::SourceExists('EtwDnstap')` ([install.md](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source)).
- Fatal text starts `EtwDnstap failed:`. Search this page for the rest.

### Steps / Configuration

#### Event log messages

Message text and fields: [Loss and risk warnings](monitoring.md#loss-and-risk-warnings), [Spool messages](monitoring.md#spool-messages), [Receiver, schema, and rate messages](monitoring.md#receiver-schema-and-rate-messages). Not listed there:

- `ETW keyword narrowing unavailable` (3): fallback keyword mask in use.
- `RESPONSE_FAILURE keyword is not available` (3): failed responses aren't captured ([configuration.md](configuration.md#subscription)).

#### The service does not reach RUNNING

`sc.exe query EtwDnstap` shows `START_PENDING`, then `STOPPED` (`WIN32_EXIT_CODE` 1066, `SERVICE_EXIT_CODE` 1). Each cause logs Event ID 1. Check in order.

| Event ID 1 text | Cause and fix |
| --- | --- |
| `invalid service arguments` | Unknown option or malformed `--spool-dir` (absolute, drive letter, no `.` or `..`). Fix the binary path ([configuration.md](configuration.md)). |
| `invalid service arguments`, then `session name must be 1 to 128 ASCII letters, digits, underscore, dot, or hyphen, not starting with a hyphen`, reason appended | Bad session name ([options](configuration.md#options)). |
| `must run as NT AUTHORITY\LocalService` | Token user isn't S-1-5-19. `cmd.exe /c 'sc.exe config EtwDnstap obj= "NT AUTHORITY\LocalService" password= ""'` (Windows PowerShell 5.1 drops the empty password), then `sc.exe sidtype EtwDnstap restricted`. |
| `the service SID NT SERVICE\EtwDnstap is not an enabled group` or `the process token is not restricted` | SID type none or unrestricted, or group disabled or deny-only. `sc.exe qsidtype EtwDnstap`; on `NONE`, `sc.exe sidtype EtwDnstap restricted`. |
| `restricting SIDs do not include the service SID` or `the Write Restricted SID (S-1-5-33)` | Token lacks the service SID or `S-1-5-33`. `sc.exe sidtype EtwDnstap restricted`. Compare the token with [SECURITY.md](../../SECURITY.md), and record a different set. |
| `lacks an enabled Performance Log Users` | Service SID missing from the group, or deny-only. `net localgroup "Performance Log Users" "NT SERVICE\EtwDnstap" /add`. |
| `configure SCM RequiredPrivileges to only SeChangeNotifyPrivilege` or `requires only an enabled SeChangeNotifyPrivilege` | Privileges extra, missing, or disabled. `sc.exe privs EtwDnstap SeChangeNotifyPrivilege`, check with `sc.exe qprivs EtwDnstap`. |
| `connect/negotiation with dnstap receiver` with `did not accept dnstap content type` | Set the collector's content type to `protobuf:dnstap.Dnstap`. |
| `connect/negotiation with dnstap receiver` with `TLS handshake`, or `received fatal alert: CertificateRequired` | Untrusted certificate, name mismatch, or required client certificate. Import the receiver CA into `Cert:\LocalMachine\Root`. Put the receiver IP address on its certificate, or set `--tls-server-name`. Drop client certificate requirements ([receiver certificate step](receiver.md#receiver-certificate)). |
| `StartTraceW: access denied` | Service SID lacks `TRACELOG_CREATE_REALTIME` on the session GUID in the message. `scripts\provision\Grant-EtwDnstapEtwAccess.ps1 -SessionName <name>` (from the message, case-sensitive), then restart the computer ([install.md](install.md#if-an-etw-access-check-is-denied)). |
| `EnableTraceEx2: access denied` | The DNS provider GUID has its own descriptor that grants neither group `TRACELOG_GUID_ENABLE`. Don't add a grant ([install.md](install.md#if-an-etw-access-check-is-denied)). |
| A session with the same name can't be reclaimed | [Stale ETW session](#stale-etw-session-blocks-startup) |
| No Event ID 1; Event ID 5 names the startup deadline | [Watchdog aborts](#watchdog-aborts-or-stop-hangs) |

#### The receiver is down or unreachable

The service keeps running. A spool holds frames on disk; without one the sink counts them and retries with backoff ([Receiver connection](configuration.md#receiver-connection)). Messages (Event IDs 2 to 4 and 24): [Receiver, schema, and rate messages](monitoring.md#receiver-schema-and-rate-messages).

- **Counters:** `receiver_connected` is `false`; `receiver_disconnects` and `receiver_reconnect_failures` count failures. With a spool, `spool_bytes` rises, then `replayed` rises after the return. Without one, `disconnected_dropped` counts the loss (Event ID 24 `the dnstap receiver is down and no spool takes frames`, then 13).
- **Handshake rejected** (Event ID 4, before the first session): fix with the TLS and content-type rows above. The service retries with backoff.
- **Recovery check:** `spool_available` is `true`, `spool_bytes` falls, `replayed` rises. `spool_bytes` rising with `receiver_connected` `true` means [The receiver is slow](#the-receiver-is-slow). Otherwise check the collector's listener, port, content type, and the firewall.

#### The receiver is slow

**Symptom:** `spooled` and `spool_bytes` rise while `receiver_connected` stays `true`.
**Cause:** the link queue (256 blocks, about 16 MiB) is full, so the router sends new blocks to the spool. `queue_dropped` counts frames when a socket write outlasts the queue, or the queue is full with no spool. `spool_dropped` counts them when the spool refuses blocks.

Read the `bytes_sent` rate, not the total. Peaks cover 60 seconds ([definitions](monitoring.md#output-counters)).

| Value | Reading | Means |
| --- | --- | --- |
| `link_saturated` | `true` | Backlog grew over 10 samples (at least 10 seconds), at least 7 of 10 steps rising, receiver connected ([saturated state](monitoring.md#the-saturated-state)). |
| `bytes_sent` | Rate below capture byte rate | Capture byte rate = `bytes_sent` rate + `spool_bytes` growth + `spool_dropped_bytes` rate. |
| `spool_oldest_age_ms` | Large, rising | The link sends data at most this old. At the cap, the spool deletes its oldest segments. |
| `send_queue_depth_max` | Near 256 | Router behind: slow spool disk, starved router thread, or a burst of late ETW events. At 256 the parser drops frames. |
| `write_max_recent_us` | 1,000,000 or more | A socket write is blocking. The session ends after `--receiver-stall-seconds` (default 15) with no bytes accepted while data waits. `write_max_us` is the maximum since the start. |
| `router_wait_max_recent_us` | Far above 50,000 | Router woke late from its wait of at most 50 ms. Check CPU contention in the guest or on the host. |
| `router_pass_max_recent_us` | Large | A router call blocked or ran long, such as a spool disk write. |

Saturation logs Event ID 23 `dnstap link is saturated`. Event ID 2 `dnstap link is no longer saturated` ends it, also on an empty backlog or a suspended spool.

Check in order:

1. `link_saturated` and the `bytes_sent` rate. At the cap, byte loss is the `spool_dropped_bytes` rate and frame loss the `spool_dropped` rate. Time to the cap = (`--spool-max-mib` minus `spool_bytes`) divided by the growth of `spool_bytes`.
2. `queue_dropped` with a spool: a write blocked the sender longer than the queue lasts. Check `slow_writes`, the disk, `spool_available`, `spool_dropped`. With CPU low, compare `router_wait_max_recent_us` with `router_pass_max_recent_us`.
3. Large `write_max_recent_us` and `slow_writes`: receiver or network path. Small values while saturated: spool disk (about 41 MB per second at 200,000 queries per second; [Spool sizing](tuning.md#spool-sizing)).
4. Collector CPU, disk, input queue.

#### Spool warnings

Fields and repeat rule: [Spool messages](monitoring.md#spool-messages).

**`spool folder is not usable`** (Event ID 26, at startup). Fix by `reason`:

| `reason` | Fix |
| --- | --- |
| ``the spool folder path is not absolute``, ``the spool folder path has a `.` or `..` component``, or ``the spool folder path is not on a local drive letter (network, device, and other path forms are not accepted)`` | Report it. The default path is always valid. |
| ``the spool folder is not on a fixed local disk (drive type {code})`` | Use a fixed local disk, set `--spool-dir`. |
| ``the spool folder has no parent folder`` | Not a drive root. |
| ``the spool folder does not exist`` | Create it ([Create the spool folder](install.md#create-the-spool-folder), [Add the spool folder](upgrade.md#add-the-spool-folder)). |
| ``the spool folder path is not a folder`` | Inspect the file with that name. |
| ``the {which} is a reparse point`` (`{which}`: `spool folder` or `parent of the spool folder`) | Inspect the link or junction. Don't follow it. |
| ``the spool folder owner is not SYSTEM or Administrators`` | `icacls`. Recreate it with the install steps, without taking ownership. |
| ``the spool folder has no access control list, so every account can read and write it`` | Recreate it with the install steps. |
| ``the spool folder permissions let an account other than SYSTEM, Administrators, or a service read or write it`` | `icacls`: remove each entry except SYSTEM, Administrators, and `NT SERVICE\EtwDnstap`. Then compare with the expected `icacls` output in [install.md](install.md#create-the-spool-folder). |
| ``cannot check the {which}: {source}`` | Windows call failed (`{source}`). Check the disk and path. |

Other messages (IDs are Event IDs):

- **`spool is disabled (--no-spool)`** (26). Every receiver outage loses frames (`disconnected_dropped`). Remove the option.
- **`spool key is not usable`** (26). No spool this run ([Spool key errors](#spool-key-errors)).
- **`spool could not be opened`** (26). Check the disk, and that nothing else holds `cursor.bin` open.
- **`dnstap spool deleted segments it cannot read at startup`** (14). `new_key=true` is expected once after you force a new key. Otherwise check for a replaced `WrappedKey` or segment files copied from another server.
- **`dnstap spool deleted its oldest segment unsent`** (14). Check `spool_dropped`, `--spool-max-mib`, free space.
- **`dnstap spool discarded unreadable segment data`** (14). Check `spool_errors`, `spool_dropped`, the disk.
- **`dnstap spool I/O error`** (27). Check `spool_errors`, the disk, programs that lock files in the folder, the folder's ACL.
- **`dnstap spool is refusing blocks`** (14). Free space, or lower `--spool-min-free-mib`.
- **`the spool is near its limit`** (25). Free space, or raise `--spool-max-mib` ([Spool sizing](tuning.md#spool-sizing)).
- **`spool_seal_error` in `output stopped`** (2). The last segment closed without its end record, and the next start replays its frames. If it repeats, check the disk.

#### Spool key errors

The AES spool key lives in `HKLM\SOFTWARE\ETW-DNSTapService\Protect\WrappedKey`, wrapped by the installer's CNG key `EtwDnstap Spool Key` ([Spool encryption key](install.md#spool-encryption-key)). The `reason` of `spool key is not usable` names the failed step:

| `reason` starts with | Cause | Fix |
| --- | --- | --- |
| ``cannot open the CNG key `EtwDnstap Spool Key` `` with `0x80090016` | `NTE_BAD_KEYSET`: key missing, or unusable by the service account. | Repair the MSI (recreates key and DACL), restart. If the repair made a new key, the service logs `cannot unwrap the spool key`: force a new key. |
| ``the key store did not answer within 3s`` | Key Storage Provider late (for example, `KeyIso` starting). No spool this run. | Restart when `KeyIso` runs. If it repeats, check that `EtwDnstap` depends on `KeyIso`. |
| ``cannot open the CNG key`` with another code, or ``cannot unwrap the spool key`` | Key unusable, or another CNG key made `WrappedKey`. | Repair the MSI. If it stays, force a new key. |
| ``cannot open the registry key`` with `Win32 error 2` | `HKLM\SOFTWARE\ETW-DNSTapService\Protect` missing. | Repair the MSI. |
| ``cannot open the registry key``, ``cannot read``, or ``cannot write the `WrappedKey` registry value`` with `Win32 error 5` | Subkey DACL lacks query value and set value for the service SID. | Repair the MSI. |
| ``the `WrappedKey` registry value is not valid`` | Damaged or hand-written value. | Force a new key. |
| ``cannot make random spool key bytes`` | Random number generator failed. | Restart. If it repeats, report it. |
| ``cannot allocate a page for the spool key`` or ``cannot lock the spool key page in memory`` | `VirtualAlloc` or `VirtualLock` failed, usually a used-up working set quota (`Win32 error 1453`). | Restart. If it repeats, report it. |

**Force a new key.** Delete the value and restart:

```powershell
Stop-Service EtwDnstap
Remove-ItemProperty -Path 'HKLM:\SOFTWARE\ETW-DNSTapService\Protect' -Name WrappedKey
Start-Service EtwDnstap
```

The service stores a new key, deletes every segment (the new key can't read them), resets `cursor.bin`, and logs `dnstap spool deleted segments it cannot read at startup` with `new_key=true`. Spooled frames are lost; their bytes count in `spool_dropped_bytes`.

**Run-time memory protection.** The key bytes stay encrypted in memory with `CryptProtectMemory` ([Encryption](configuration.md#encryption)). A `CryptUnprotectMemory` failure is a transient spool error: `spool_errors` counts it, `dnstap spool I/O error` (Event ID 27) logs, and the spool pauses 10 seconds. Restart if it repeats. A permanent failure suspends the spool ([Common Gotchas](#common-gotchas)).

#### The service keeps restarting

SCM restarts the service under the [recovery policy](install.md#5-configure-scm-recovery) when it exits with a service-specific error: a capture worker ends on its own (Event ID 1 `a capture worker exited before shutdown was requested`), any other run failure, or a panic of the sender or link thread (release builds abort, so SCM sees a crash).

Check the Event ID 1 entries, `sc.exe qfailure EtwDnstap`, and that `sc.exe qfailureflag EtwDnstap` reports the non-crash flag as set. Without a recovery policy, a stopped service stays down.

#### Event IDs for data loss

Message text, fields, and fixes for IDs 10 to 16 and 20 to 28: [Loss warnings](monitoring.md#loss-warnings), [Risk warnings](monitoring.md#risk-warnings). Counters: [Capture counters](monitoring.md#capture-counters), [Output counters](monitoring.md#output-counters). Add `etw_lost` and `etw_buffers_lost` separately, because the units differ ([Counter relationships](monitoring.md#counter-relationships)). Treat any gap between the collector's record count and the client's as unexplained until you read the counters.

- **12:** with `--jumbo-capacity 0`, every jumbo record counts in `pool_exhausted_jumbo`.
- **13:** `disconnected_dropped`: check `spool_available`, then [the receiver is down](#the-receiver-is-down-or-unreachable). `queue_dropped`: [the receiver is slow](#the-receiver-is-slow).
- **14, 26, 27:** [Spool warnings](#spool-warnings).
- **15:** [Rejected events](#rejected-events).
- **16:** `receiver_disconnected` counts the event channel, not TCP. Check whether the parser stopped ([restarts](#the-service-keeps-restarting)). Report the steps.
- **22, 23:** [the receiver is slow](#the-receiver-is-slow). **24:** [the receiver is down](#the-receiver-is-down-or-unreachable). **28:** [ETW consumer and session warnings](#etw-consumer-and-session-warnings).

#### Rate warning

Above 22,000 events per second (defaults suit about 20,000, or 10,000 queries per second) the service logs `received rate is above 22,000 events/s` once per run. If `ring_dropped` or `etw_lost` rises in `capture stats`, see [Recommended settings for higher rates](tuning.md#recommended-settings-for-higher-rates). At 50,000 queries per second:

```text
--etw-buffer-kib 128 --etw-min-buffers 12 --etw-max-buffers 128 --capacity 131072
```

At 100,000 use `--capacity 524288`, at 200,000 `--capacity 262144`. The maximum, 524,288, has a 42-second startup deadline and a working set of about 1.4 GB ([Defaults](tuning.md#defaults)).

#### ETW consumer and session warnings

- **`ETW consumer did not return within 5 s of STOP`.** The service closes the trace handle. Events in ETW buffers may be lost without an `etw_lost` count. Check the host for overload.
- **`stale ETW session used a different buffer size; reclaiming anyway`.** The service reclaims the session ([Stale ETW session](#stale-etw-session-blocks-startup)).

#### Rejected events

Messages and fields: [Receiver, schema, and rate messages](monitoring.md#receiver-schema-and-rate-messages).

- **`DNS event schema load failed`.** Often the provider manifest isn't available yet. The schema loads again after 65,536 more rejected events. If they stay rejected, check the DNS Server role and its ETW provider, and restart the service.
- **`DNS event schema is unsupported and its events are rejected`.** Not retried. The schema doesn't match the single-pass decoder grammar, isn't the expected provider or event, or has an invalid size, for example after a new Windows build or DNS Server manifest. Report the text. Capturing these events needs a decoder change.

#### No records arrive at the collector

Check in order:

1. `capture started` (Event ID 2) exists, and the latest `output stats` shows `receiver_connected` `true`.
2. The DNS Server writes Event IDs 256, 257, and 258 (`Microsoft-Windows-DNSServer`).
3. `capture started` shows the keyword masks you expect. A fallback warning means the subscription uses `0x8000000000000000` ([configuration.md](configuration.md#subscription)).
4. The collector's filter or pipeline matches [receiver.md](receiver.md#receiver-configuration).

#### Stale ETW session blocks startup

The service reclaims a crashed or aborted service's session when its name, shape, and GUID match ([Session reclaim](configuration.md#session-reclaim)). If startup reports `already exists and does not match this program's session shape`, another logger mode, a log file, or a different GUID owns it. Find the owner, then stop it, using your `--session-name` in place of `EtwDnstap`:

```powershell
logman query -ets
logman stop EtwDnstap -ets
```

A failed reclaim logs Event ID 1 `query existing ETW session '<name>' failed` or `stopping stale ETW session '<name>' failed`.

#### Watchdog aborts or stop hangs

A passed deadline writes Event ID 5, starting `EtwDnstap watchdog: the startup deadline` or `shutdown deadline`, and aborts the process. With the default `--capacity` of 131072 the startup deadline is 18 s: `the startup deadline of 18 s expired`. It grows with `--capacity`, and is 10 s before the service reads its arguments ([Startup and shutdown deadlines](configuration.md#startup-and-shutdown-deadlines)).

The abort can leave an incomplete capture and a real-time ETW session. Run `logman query -ets`, then stop that session or restart the computer.

#### A panic aborts the process

Event ID 6 starts `EtwDnstap panicked:`, and the process aborts without cleanup. Check `logman query -ets` for a leftover session. Report the text with the steps that led to it.

#### Still stuck

Collect these, redacting the receiver address and query names:

- Application log entries for source `EtwDnstap`, with the time range.
- Output of `sc.exe qc`, `qsidtype`, `qprivs`, `qfailure`, and `qfailureflag` for `EtwDnstap`.
- Members of `net localgroup "Performance Log Users"`, and the SDDL printed by `scripts\provision\Grant-EtwDnstapEtwAccess.ps1`.
- The `logman query -ets` list.
- The `output stats` line with the spool counters. Spool files hold client addresses and query names, so send none.
- The commit SHA, and whether the build is a release or a diagnostic (`diag-timing`) build.

Security issues go through [SECURITY.md](../../SECURITY.md), not a public issue.

### Common Gotchas

- **A restart isn't always a failure.** A DNS Server restart isn't one. A STOP/FINISH exchange that doesn't complete (for example, a stalled receiver) ends with success and no SCM restart. Unsent frames stay in the spool, or count in `disconnected_dropped` without one.
- **In-flight records are lost** at a failure. A restart resets every counter.
- **Console start prints nothing** and exits with code 1. Read the version from the `version` field of `capture started`, or run `(Get-Item <exe>).VersionInfo`.
- **Service control.** A second `sc.exe start` fails with error 1056 (already running). The service accepts Stop and Shutdown only in `RUNNING`: SCM refuses a stop during `START_PENDING` and `STOP_PENDING`, and rejects other control codes, such as 200. A checkpoint that doesn't advance before the wait hint passes means a hang.
- **Shutdown doesn't wait for a spool backlog.** While disconnected, `output stopped` reads `stop_sent=false`, the service exits with code 0, and spooled frames stay on disk (`spool_backlog_kept=true`).
- **Wrong content type after startup.** Each reconnect fails and `receiver_connected` stays `false`. Fix the collector's content type. Only the first connection at startup stops the service.
- **A folder listing shows the open segment as 0 bytes.** Read `spool_bytes`.
- **`link_saturated` keeps its value while the receiver is down.** Read it with `receiver_connected`.
- **The rate warning alone means no loss.** Recursive traffic produces no client records.
- **`new_key=true` is expected once** after you force a new key, which deletes every spooled frame.
- **A watchdog abort or a panic can leave a real-time ETW session** ([Stale ETW session](#stale-etw-session-blocks-startup)).
- **A suspended spool.** If `CryptProtectMemory` fails twice, or the decrypted bytes fail the key check, the service zeroes the key bytes and suspends the spool until it restarts. It logs one Event ID 26 warning that starts `the spool key is unusable after a memory protection failure`, followed by `(CryptProtectMemory, Win32 error N)` or `(CryptUnprotectMemory returned bytes that fail the key check)`. While suspended:
  - Appends are refused and count in `spool_dropped` only. A refused append deletes no segment, except in one rare case: the size cap evicts the active segment and the key fails as the block is sealed again.
  - Reads stop and drop nothing. `spool_available` is false.
  - Replay batches the link already took are acknowledged as usual. Nothing else is deleted, and the cursor doesn't move.
  - Live frames go straight to the receiver when it's up. When it's down they're refused and counted, and a full send queue drops frames (`queue_dropped`).
  - `link_saturated` clears with the reason `the spool is suspended until the service restarts`.

  Restart the service. The next start unwraps the key and replays the whole kept backlog.
