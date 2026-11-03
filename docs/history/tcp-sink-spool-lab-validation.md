# TCP sink spool lab validation

> **Status:** Historical · **Evidence dated:** 2026-11-03 (timestamps in UTC) · **Commit:** `e6b744f` (lab build) · **Reviewed:** 2026-11-03

This record covers step 10 of the
[TCP sink spool plan](tcp-sink-spool-plan.md): the first lab run of the disk
spool. It answers three questions:

- Do the install and upgrade steps for the spool folder work as written, and
  does the folder grant work under the restricted service token?
- Does a 60-second receiver outage at 1,000 and at 50,000 queries per second
  (QPS) lose frames beyond the documented in-flight bound?
- Does a service restart with a backlog on disk replay the leftover frames?

The raw run data is lab evidence. It is not in the repository.

## Environment

| Item | Value |
| --- | --- |
| Guest | DNSTap-WS2025 (`24bafedb-0998-4430-a0c6-7e90350cd9ac`): 6 vCPUs, 4 GiB, Windows Server 2025 (10.0.26100). Checkpoint `spool-pre-2026-11-03` taken before any change. |
| Service identity | `NT AUTHORITY\LocalService`, SID type restricted, `SeChangeNotifyPrivilege` only. Not changed by this run. |
| Service build | `e6b744f`, `cargo build --locked --release`, SHA-256 `D5D51704B39564F2BFD74895206E6C58C02EB43646AB954EEFC4E1D466DB801B`. `--version` prints `etw-dnstap 0.1.0`. |
| Service options | `--receiver 172.27.215.60:6000 --session-name EtwDnstap-Lab --plaintext --stats-interval-seconds 5`. Spool options at their defaults: `C:\ProgramData\EtwDnstap\spool`, 4096 MiB cap, 2048 MiB free-space floor. Run B added intake settings (see [Run B](#run-b-50000-qps)). |
| Previous build on the guest | SHA-256 `A37729CBDE5809566F7C3104597B6D00C69D544BB0B0A23180F97948EF6956B0`, no spool, and its counter manifest. |
| Receiver | DNS-collector 3.2.3 on DNSTap-Ubuntu2604, plaintext. Its pipeline routed to the `devnull` logger, and its stdout and stderr went to null. It wrote nothing to disk. The config hash was checked before each run. |
| Client | `dns-stress` built from `e6b744f` (the source equals `2073411`; only its README changed since) on the host. Authoritative load, 1,024 in flight per thread. |

## Method

1. Recorded the guest state: the binary and manifest hashes, the service
   configuration, the SID type, the privileges, the service descriptor, the
   install folder, and the `EtwDnstap` folder under `ProgramData`. The spool
   folder did not exist.
2. Copied the new executable and manifest to a staging folder on the guest,
   in the same relative layout as a checkout (`target\...\release` and
   `packaging\perfcounters`). Then ran the code blocks of the guides exactly as
   written, in one PowerShell session, in this order:
   - [Create a protected snapshot folder](../guide/install.md#create-a-protected-snapshot-folder)
     and [Snapshot before a change](../guide/upgrade.md#snapshot-before-a-change);
   - [Create the spool folder](../guide/install.md#create-the-spool-folder),
     with `Test-SpoolFolder`;
   - [Replace the executable](../guide/upgrade.md#replace-the-executable), with
     [Replace the counter manifest](../guide/upgrade.md#replace-the-counter-manifest)
     between the stop and the start, as
     [Add the spool folder](../guide/upgrade.md#add-the-spool-folder) says.
3. Ran two calibration runs without an outage, 30 s each, at 1,000 and
   50,000 QPS. They give the receiver bytes for each encoded frame.
4. Ran A, B, and C. The service restarted before each run, so its counters
   started at zero. The collector was stopped and started with `systemctl`.
5. Polled every 5 s: the performance counters, the spool folder listing, and
   the receiver's `ss -tin` `bytes_received`. Stopping the collector ends the
   TCP connection, so a loop on the receiver ran `ss -tin` about every 7 to 10 ms
   while the collector stopped. The last poll that still showed the connection
   gives that connection's bytes.
6. After the load stopped, waited until `spool_bytes` was 0 and no segment
   file remained. Then read the receiver bytes, stopped the service, and took
   the counters from `capture stopped` and `output stopped`.

Delivered frames are the receiver bytes, less 84 bytes of handshake on each
connection, divided by the bytes for each frame. The expected count is
`encoded - (disconnected_dropped + queue_dropped + spool_dropped)`.

## Calibration

| Item | 1,000 QPS | 50,000 QPS |
| --- | --- | --- |
| Queries sent and answered | 30,000 | 1,500,000 |
| `encoded` | 60,000 | 2,960,966 |
| Receiver bytes | 6,177,446 | 305,017,041 |
| Bytes for each frame | 102.956 | 103.013 |
| `ring_dropped` | 0 | 39,034 (1.3%) |

- Every encoded frame reached the receiver in both runs.
- The bytes for each frame differ by 0.06 between the two rates. The earlier
  figure of about 103.01 matches the 50,000 QPS run. The gaps below use the
  calibration of the same rate, and the 103.01 figure in brackets.
- At 50,000 QPS, the default intake settings filled the 24,576-event channel
  (`queue_depth_max` 24,577), and the service logged the rate warning. This
  loss is at intake, before the sink, and it is not related to the spool.
  [Tuning](../guide/tuning.md#high-rate-settings) describes it.

## Provisioning results

Every guide step ran without an error, and the folder grant worked under the
restricted token.

- `Test-SpoolFolder` printed `Spool folder: as expected.` after the grant and
  again after the service started.
- `icacls` showed the four expected entries, with no `(I)`:
  `NT SERVICE\EtwDnstap:(RX,W)`, `NT SERVICE\EtwDnstap:(OI)(CI)(IO)(M)`,
  `BUILTIN\Administrators:(OI)(CI)(F)`, and `NT AUTHORITY\SYSTEM:(OI)(CI)(F)`.
- The new executable showed `NT SERVICE\EtwDnstap:(I)(RX)`.
- `unlodctr` and `lodctr` succeeded. `typeperf -q EtwDnstap` listed the nine
  new counters: Send Queue Drops, Frames Spooled, Frames Replayed, Spool
  Dropped Frames, Spool Dropped Bytes, Spool Errors, Spool Bytes, Spool
  Segments, and Spool Available.
- The first start logged `spool in use spool_dir=C:\ProgramData\EtwDnstap\spool
  spool_max_mib=4096 spool_min_free_mib=2048 leftover_segments=0
  leftover_bytes=0`, with no spool warning.
- In the runs, the service created, wrote, sealed, read, and deleted segment
  files, and rewrote `cursor.bin`. `spool_errors` stayed 0. `cursor.bin` had
  owner `NT AUTHORITY\LOCAL SERVICE` and the entries
  `NT SERVICE\EtwDnstap:(I)(M)`, `BUILTIN\Administrators:(I)(F)`, and
  `NT AUTHORITY\SYSTEM:(I)(F)`.

## Run A: 1,000 QPS

Load: 4 threads, 150 s, 150,000 queries, all answered, 0 timeouts.

| Item | Value |
| --- | --- |
| Collector stopped | 09:04:58.05. The service logged `dnstap receiver connection lost` (os error 10053) at 09:04:58.43. |
| Collector started | 09:05:58.55. The service logged `dnstap receiver reconnected` at 09:06:05.49, after the reconnect backoff. |
| Spool empty | By the next poll, at 09:06:10. |
| `received` = `dequeued` = `encoded` | 300,000 |
| `ring_dropped`, `intake_dropped`, `etw_lost` | 0 |
| `receiver_disconnects`, `receiver_reconnects` | 1, 1 (`receiver_reconnect_failures` 7) |
| `disconnected_dropped` | 636 |
| `queue_dropped`, `spool_dropped`, `spool_errors` | 0 |
| `spooled` = `replayed` | 134,915 |
| Spool at peak (09:06:05) | 13,895,749 bytes in one segment, `seg-00000000000000000001.fstrm`, and `cursor.bin` |
| Spool at the end | `cursor.bin` (28 bytes) only |
| Receiver bytes | 6,059,204 on the first connection, 24,755,545 on the second |
| Expected frames | 299,364 |
| Delivered frames | 299,298 (299,142) |
| Gap | 66 frames, 6.7 KB (222 frames, 23 KB) |

## Run B: 50,000 QPS

Load: 16 threads, 150 s, 7,500,000 queries, all answered, 0 timeouts.

For this run only, the binary path also had
`--etw-buffer-kib 64 --etw-max-buffers 256 --capacity 262144`, so that intake
loss would not hide sink loss. The path was set back after the run.

| Item | Value |
| --- | --- |
| Collector stopped | 09:08:44.06. The service logged `dnstap receiver connection lost` at 09:08:44.06. |
| Collector started | 09:09:44.66. The service logged `dnstap receiver reconnected` at 09:09:51.22. |
| Spool empty | By the next poll, at 09:09:57. About 700 MB replayed in at most 6 s. |
| `received` = `dequeued` = `encoded` | 15,000,000 |
| `ring_dropped`, `intake_dropped`, `etw_lost` | 0 (`queue_depth_max` 56,292) |
| `receiver_disconnects`, `receiver_reconnects` | 1, 1 (`receiver_reconnect_failures` 7) |
| `disconnected_dropped` | 640 |
| `queue_dropped`, `spool_dropped`, `spool_errors` | 0 |
| `spooled` = `replayed` | 6,783,646 |
| Spool at the last sample before replay (09:09:46) | 644,271,704 bytes in 10 segment files: 9 sealed files of 67,043,694 to 67,107,133 bytes (`seg-…03` to `seg-…11`), the active `seg-…12`, and `cursor.bin` |
| Spool at the end | `cursor.bin` only |
| Receiver bytes | 318,392,155 on the first connection, 1,226,562,446 on the second |
| Expected frames | 14,999,360 |
| Delivered frames | 14,997,716 (14,998,102) |
| Gap | 1,645 frames, 169 KB (1,259 frames, 130 KB) |

- `slow_writes` was 6 and `write_max_us` 24,547.
- The sequence numbers start at 3, because `next_sequence` stays above the
  cursor's sequence (an accepted C1 deviation).

## Run C: restart with leftovers, 1,000 QPS

Load: 4 threads, 120 s, 120,000 queries, all answered, 0 timeouts. The load
ran during the whole test. The 4,372 queries sent while the service restarted,
about 4.4 s, were not captured.

Order of events:

1. 09:12:02.74: collector stopped. Connection lost logged at 09:12:02.93.
2. 09:12:32.77: service stopped, in 0.31 s. It logged `dnstap spool backlog
   kept on disk for the next run` and `shutting down while the dnstap receiver
   is disconnected; no STOP sent`. One segment stayed: 6,493,432 bytes.
3. 09:12:35.14: service started with the collector still down. It logged
   `dnstap receiver unavailable at startup` and then `spool in use … leftover_segments=1
   leftover_bytes=6493432`.
4. 09:12:53.31: collector started. Reconnected at 09:12:59.29. The spool was
   empty at the 09:12:59 poll.

| Item | First service run | Second service run |
| --- | --- | --- |
| `received` = `encoded` | 102,152 | 129,104 |
| `ring_dropped`, `intake_dropped`, `etw_lost` | 0 | 0 |
| `receiver_disconnects`, `receiver_reconnects` | 1, 0 | 0, 1 |
| `disconnected_dropped` | 632 | 0 |
| `queue_dropped`, `spool_dropped`, `spool_errors` | 0 | 0 |
| `spooled` | 63,066 | 40,480 |
| `replayed` | 0 | 103,546 |
| `spool_bytes` in `output stopped` | 6,493,432 (1 segment) | 0 |
| Receiver bytes | 3,951,820 | 19,796,124 |

- `replayed` in the second run is 103,546 = 63,066 leftover frames + 40,480
  frames spooled in that run. Every leftover frame was replayed.
- Spool at peak (09:12:54): the leftover `seg-…15.fstrm` (6,493,432 bytes),
  the active `seg-…16.fstrm`, and `cursor.bin`. At the end: `cursor.bin` only.
- Expected frames: 102,152 − 632 + 129,104 = 230,624. Delivered: 230,659
  (230,539). Gap: −35 frames (86 frames).

## Checks

| Check | Run A | Run B | Run C |
| --- | --- | --- | --- |
| `spool_dropped`, `queue_dropped`, `ring_dropped`, `intake_dropped` are 0 | Pass | Pass, with the intake settings above | Pass |
| `spooled` > 0 and `replayed` = `spooled` | Pass | Pass | Pass: `replayed` = leftovers + `spooled` |
| Leftovers replayed (`leftover_segments` > 0, `replayed` > `spooled`) | n/a | n/a | Pass |
| Spool empties, no segment file remains | Pass | Pass | Pass |
| One disconnect and one reconnect | Pass | Pass | Pass, over the two service runs |
| Gap within the in-flight bound | Pass | Pass | Pass |

**The gap.** The counted in-flight loss in each outage was one block of about
64 KiB: 636, 640, and 632 frames. The uncounted gap was 6.7 KB, 169 KB, and
−3.6 KB. All are far below the 1 MiB socket send buffer (about 10,180
frames), which the guides name as the uncounted part of the bound. Two
measurement limits apply:

- The receiver loop polled every 7 to 10 ms. At 50,000 QPS that is up to about
  95 KB of frames that the last poll did not see, so part of run B's gap is
  measurement.
- The bytes for each frame differ by about 0.05% between the two calibration
  rates. That is about 150 frames on runs A and C. The negative gap of run C is inside this limit. The
  guides also say that frames counted in `disconnected_dropped` may have
  reached the receiver.

## Guide steps that need changes

The guide steps worked. These points are wrong or unclear:

1. **The counted in-flight loss is one block, not about 100 ms.**
   [Delivery is at most once](../guide/configuration.md#delivery-is-at-most-once)
   says a failed connection loses "the frames written since the last flush,
   about 100 ms of frames". Each outage counted 632 to 640 frames at both
   rates: the 64 KiB block that was being written. At 1,000 QPS one block is
   about 320 ms of frames. Fix: say that the loss is the block being written,
   up to 64 KiB (about 636 frames at 103 bytes), plus the frames written since
   the last flush.
2. **The source paths assume a build checkout.**
   [Replace the executable](../guide/upgrade.md#replace-the-executable) copies
   from `.\target\x86_64-pc-windows-msvc\release\etw-dnstap.exe`, and
   [Replace the counter manifest](../guide/upgrade.md#replace-the-counter-manifest)
   from `.\packaging\perfcounters\etw-dnstap.man`. A server normally has
   neither folder. The lab staged the files in that layout. Fix: say that the
   commands run from the folder that holds the new files, or use a placeholder.
3. **`receiver_reconnects` counts a first connection.** When the receiver is
   down at startup, the first successful connection counts as a reconnect (run
   C, second service run: `receiver_reconnects=1`, `receiver_disconnects=0`).
   [Output counters](../guide/monitoring.md#output-counters) and the Receiver
   Reconnects row say "after the first session". The behavior is older than
   the spool. Fix: say that it also counts the first connection when the
   receiver was down at startup.
4. **The active segment shows 0 bytes in a listing.** `Get-ChildItem` showed
   the open segment as 0 bytes (for example `seg-…12.fstrm 0` in run B), while
   `spool_bytes` gave the real total. NTFS updates the directory entry when the
   file closes. Fix: add a note to [Spool](../guide/configuration.md#spool) or
   to troubleshooting that `spool_bytes` is the reliable size.
5. **Small points.** `sc.exe stop` on a stopped service prints
   `FAILED 1062` in [Replace the executable](../guide/upgrade.md#replace-the-executable);
   the uninstall guide says that this is fine, and the upgrade guide does not.
   `icacls` lists `BUILTIN\Administrators` before `NT AUTHORITY\SYSTEM`; the
   install guide lists them the other way round.

## Lab state after the run

- The previous executable is back, with the rollback in
  [Replace the executable](../guide/upgrade.md#replace-the-executable). The
  previous counter manifest is registered again. The first attempt copied no
  file because of an error in the lab script, so `lodctr` registered the new
  manifest once more. The second attempt registered the previous manifest.
- The binary path, account, SID type, privileges, service descriptor, start
  type, and executable ACL match the recorded state. The service is stopped,
  as before.
- The spool folder was deleted with step 7 of the
  [uninstall guide](../guide/uninstall.md#7-delete-the-spool-folder). It held
  only `cursor.bin`. The snapshot folder that the guide created, and the
  staging folder, were deleted.
- The collector config is unchanged, with its output to null. The collector
  is running.
- The checkpoint `spool-pre-2026-11-03` is kept.

## Not tested

- Eviction at the size cap, the free-space floor, and its pause.
- Spool I/O errors, a corrupt `cursor.bin`, and a cut-off or unsealed segment.
- A slow but connected receiver, which spills to the spool at 96 queued blocks.
- A connection failure during a replay batch.
- A restart while a replay is in progress.
- `--no-spool`, `--spool-dir`, and the spool folder warnings.
- Frame order and duplicates at the receiver. The collector wrote nothing, so
  the check is on byte counts only.
- A TLS receiver, outages longer than 60 s, and rates other than 1,000 and
  50,000 QPS.
- The other steps of the uninstall guide.
