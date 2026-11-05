# Spool encryption lab validation

> **Status:** Historical · **Evidence dated:** 2026-11-04 (lab guest local time, UTC-8; the UTC timestamps below read 2026-11-05 from 07:38) · **Commit:** `27a5464` (branch `spool-encryption`) · **Reviewed:** not yet

This record covers task L of the [spool encryption plan](../plans/spool-encryption-plan.md): the
first lab run of the encrypted spool, the key lifecycle in the MSI, and the two upgrade cases. It
ran at 10,000 queries per second only (1 `dns-stress` thread, authoritative mode), with the
default buffer settings of the service: `--etw-buffer-kib 32`, `--etw-min-buffers 12`,
`--etw-max-buffers 64`, `--capacity 131072`. None of those options was passed. The 50,000 and
200,000 QPS throughput runs of the plan were not authorized and were not run. The test of
another local account that tries to unwrap the key was not in the authorized step list and was
not run.

The raw data (Event Log dumps, the receiver `ss` samples, the MSI logs, and the copied segment
files) is lab evidence on the host. It is not in the repository.

## Summary

| Step | Result |
| --- | --- |
| 1. Checkpoint and receiver record | Pass |
| 2. Old build leaves plaintext segments | Pass |
| 3. Upgrade: key, registry, ACLs, dependencies | Pass |
| 4. First start and restart | Pass |
| 5. Encrypted spool, outage and replay, no plaintext on disk | Pass (see the kernel-buffer note) |
| 6. Tamper with one record | Pass |
| 7. Memory lock under the restricted token | Pass (indirect evidence) |
| 8. DNS Server crash and normal stop | Pass |
| 9. Second major upgrade with a backlog | Pass |
| 10. Steady 60 s run | Pass |
| 11. Stop rule | Pass |

No defect was found in the spool encryption. Every Event Log line of the run is Information,
except the expected spool, receiver, and rate warnings that the sections below quote. Event ID 26
(`RISK_SPOOL_DISABLED`) and Event ID 1 never appeared.

One behavior is not new and is worth knowing when you read the delivery numbers: when the receiver
stops answering, the bytes that the guest kernel already accepted on the stalled connection are
lost without a count (about 1 MiB at 10,000 QPS). See step 5.

## Method

| Item | This run |
| --- | --- |
| Guest | `DNSTap-WS2025`, Hyper-V, 12 vCPUs, 8 GiB static. Not resized. Checkpoint `spool-encryption-pre-2026-11-04` taken before any change. No checkpoint was restored. |
| Receiver | `DNSTap-Ubuntu2604`, DNS-collector with the `devnull` logger and `StandardOutput=null` (unit state checked at the start and the end). Nothing was written to a file or the journal. Delivery was counted from `ss -tin` `bytes_received`, sampled every second from the host (the samples went to the host only). |
| Builds | Old: the MSI of `main` at `64b5e91` (plaintext spool, PID filter), executable SHA-256 `651CC66427746733C15A35E19D34C343763FF7C74E01AE9968704861BEB2978A`, ProductCode `{9E02D17F-D5D4-4024-AD76-936A5EF37D0A}`. First new MSI: SHA-256 `CE773A47801553F31EB78C68FD9412072547B0CEE1968016CC86316AA6ED6E79`, ProductCode `{9AB651E1-DA6B-4969-844E-704530DBFCBB}`. Second new MSI: built from the same commit with `scripts/build/Build-Msi.ps1 -SkipCargoBuild` after the MSBuild outputs were cleared (the first run of the script was a no-op and reproduced the first hash), SHA-256 `17BF32EADE1E96C5063DFB67E34AA4E9AC3130A63BD621D323B91D66E1F55950`, ProductCode `{8775DA1E-6CBE-4DCC-ACBA-1FDE5A334D83}`. Both new MSIs carry the same executable, SHA-256 `7A4B32659A25BD85C5D0265F64AD35EE45B8ABA34401F19A2FD839D60ED5A608`. |
| Install command | Always `msiexec /i <msi> /qn /norestart /l*v <log> RECEIVER="172.27.215.60:6000" SESSION_NAME="EtwDnstap-Lab" SERVICE_ARGS="--plaintext --stats-interval-seconds 5"`. Exit 3010 both times, guest restarted. Nothing was uninstalled. |
| Load | `dns-stress` from the host, 1 thread, authoritative, `--qps 10000`, `--inflight 256`. One query makes 2 events and 2 frames. About 103.015 bytes a frame on the wire. |
| Outages | `iptables -I INPUT 1 -s 172.27.215.163 -p tcp --dport 6000 -j DROP` on the receiver, deleted after each use. |
| Counters | The 5-second `capture stats` and `output stats` lines of the Application log, and `typeperf` for step 10. |

Byte accounting used below: a new connection starts with 84 bytes of Frame Streams handshake. The
service's `bytes_sent` is the bytes it handed to the socket. The plaintext length of an encrypted
record equals the wire bytes of its frames.

## 1. Checkpoint and receiver record

Checkpoint `spool-encryption-pre-2026-11-04` created before any change (Standard checkpoint of a
running guest). The receiver record (tc qdiscs, iptables, ip6tables, nft ruleset, interfaces, kernel
modules, collector config hash, unit output settings) matched the record of the previous lab
section line for line. The only filter in place is the standing `table inet dnstap_lab` that drops
other sources.

## 2. Old build leaves plaintext segments

The guest ran the `64b5e91` MSI (ProductCode `{9E02D17F-...}`), with no `Parameters` key, no
`Protect` key, no CNG key. 10,000 QPS for 24 s, receiver blocked from 2 s after the start. After the
load the service was stopped while the block was still on:

```
INFO output stopped ... receiver_connected=false ... disconnected_dropped=638 queue_dropped=0 spooled=478016 replayed=0 spool_dropped=0 spool_dropped_bytes=0 ... spool_bytes=49243424 spool_segments=1 ... stop_sent=false spool_backlog_kept=true
```

Spool folder after the stop: `cursor.bin` (28 bytes) and `seg-00000000000000000326.fstrm`
(49,243,424 bytes, plaintext). The block was then removed.

## 3. Upgrade to the first new MSI

Install log (`WixQuietExec` lines):

```
Created key 'EtwDnstap Spool Key' (RSA-3072, not exportable).
Set and checked the DACL of key 'EtwDnstap Spool Key': SYSTEM and Administrators all, service SID S-1-5-80-... GR.
Created registry key SOFTWARE\ETW-DNSTapService (default ACL, not changed).
Created registry key SOFTWARE\ETW-DNSTapService\Protect with its protected DACL.
Set and checked the DACL of SOFTWARE\ETW-DNSTapService\Protect : SYSTEM and Administrators full control, service SID S-1-5-80-... query value, set value, read control.
```

Checks on the guest after the restart (read with .NET `CngKey` in the Microsoft Software KSP and
with `certutil -key`):

| Item | Observed |
| --- | --- |
| CNG key | Present in the machine store, `certutil` lists `EtwDnstap Spool Key`; algorithm RSA, size 3072, machine key, export policy 0; a private-key export is refused ("The requested operation is not supported"). |
| CNG key DACL | `D:P(A;;0xd01f01ff;;;SY)(A;;0xd01f01ff;;;BA)(A;;0x80120089;;;<service SID>)`. Protected. `0xD01F01FF` is the mapped form of generic all for SYSTEM and Administrators; `0x80120089` is the mapped generic read for `NT SERVICE\EtwDnstap`, the minimum right of the Wave 1 findings. |
| `HKLM\SOFTWARE\ETW-DNSTapService\Protect` | Present. SDDL `O:SYG:SYD:PAI(A;;KA;;;SY)(A;;KA;;;BA)(A;;CCDCRC;;;<service SID>)`. Protected (no inheritance). SYSTEM and Administrators full control (`0xF003F`). The service SID has query value, set value, and read control (`0x20003`). The parent `ETW-DNSTapService` key has the default inherited ACL, as the log says. |
| `Services\EtwDnstap\Parameters` | Did not exist before the install and does not exist after it, so there is no ACL change and no `Parameters\SpoolKey` subkey. The ACL of the `Services\EtwDnstap` key is the same before and after. |
| `sc.exe qc EtwDnstap` | `DEPENDENCIES : DNS` and `: KeyIso`; `AUTO_START (DELAYED)`; `NT AUTHORITY\LocalService`. `sc.exe qsidtype`: RESTRICTED. `sc.exe qprivs`: `SeChangeNotifyPrivilege` only. `KeyIso` runs. |
| ImagePath | `"C:\Program Files\EtwDnstap\etw-dnstap.exe" --receiver "172.27.215.60:6000" --session-name "EtwDnstap-Lab" --plaintext --stats-interval-seconds 5` (the three properties and nothing else). |

## 4. First start and restart

The delayed-start service came up by itself after the restart. Event Log, first start (UTC):

```
23:43:22 ID14 WARN dnstap spool deleted segments it cannot read at startup: segments of an earlier version or of another spool key segments=1 bytes=49243424 earlier_format=1 other_key=0 new_key=true
23:43:22 ID2  INFO spool in use spool_dir=C:\ProgramData\EtwDnstap\spool spool_max_mib=4096 spool_min_free_mib=2048 new_spool_key=true leftover_segments=0 leftover_bytes=0
23:43:22 ID2  INFO capture started session="EtwDnstap-Lab" ... receiver=172.27.215.60:6000 tls=false capacity=131072 etw_buffer_kib=32 etw_min_buffers=12 etw_max_buffers=64 etw_flush_seconds=1 ... version="0.1.0"
23:43:27 ID14 WARN data loss in the spool: ... spool_dropped_bytes_increase=49243424 spool_dropped_bytes=49243424 seconds=5
```

(The clock column above is the guest's local time, UTC-8.) The old plaintext segment (49,243,424
bytes) was deleted and counted in `spool_dropped_bytes`, as designed. `WrappedKey` existed under
`Protect` after this start: `REG_BINARY`, 396 bytes, version 1, key id `b1c37f136a264270`,
algorithm 1, wrapped length 384. There was no Event ID 26. `capture started` shows the defaults
32 / 12 / 64 / 131072.

After one more stop and start: `spool in use ... new_spool_key=false leftover_segments=0`, no
deletion warning, and the `WrappedKey` header (version and key id) unchanged.

## 5. Encrypted spool under 10,000 QPS

50 s of load. Receiver blocked 3 s after the start and released 40 s later (the block was longer
than the planned 30 s because two segment copies ran during it).

| Time (UTC) | Event |
| --- | --- |
| 07:46:11.8 | Block on. |
| 07:46:23 | `spooled=211949 spool_bytes=21843176 spool_segments=1` |
| 07:46:28 | `dnstap receiver connection lost ... the receiver accepted no bytes for 15001 ms while data waited` (ID 24) |
| 07:46:38 | `spooled=522121 spool_bytes=53809634` |
| 07:46:49 | `spooled=710434 spool_bytes=73217794 spool_segments=2` (the first segment rotated at about 64 MiB) |
| 07:46:51.6 | Block off. |
| 07:46:52.5 | `dnstap receiver reconnected` (ID 2). |
| 07:46:54 | `spooled=791787 replayed=791787 spool_dropped=0 spool_dropped_bytes=0 spool_bytes=0 spool_segments=0`. The replay of the 73 MB took under 3 s. |

The spool grew at about 2.1 MB a second (20,000 frames a second). The sender thread used 0.0 to
0.3 percent of a logical processor while it spilled, and 1.9 percent in the replay interval (the
counter has clock-tick resolution).

**Delivery.** Final counters: `received=999998 encoded=999998 ring_dropped=0 etw_lost=0`,
`spooled=791787`, `replayed=791787`, `spool_dropped=0`, `disconnected_dropped=0`,
`bytes_sent=103014994` (exactly 103.015 bytes for each of the 999,998 frames).

| Receiver connection | Service `bytes_sent` | Receiver `bytes_received` (minus 84 handshake) |
| --- | --- | --- |
| Before the block (stalled) | 5,191,569 | 4,141,872 |
| After the reconnect (replay and live) | 97,823,425 | 97,823,425 (exact) |

Everything the service sent on the new connection arrived, byte for byte. The first connection
received 1,049,697 bytes (about 10,190 frames) less than the service counted as sent. Those are
the bytes that the guest kernel had accepted into its send buffer when the stall began; they are
lost without a count, as [monitoring.md](../guide/monitoring.md) states ("Bytes that the kernel
accepted before a connection failed are lost without a count") and the
[slow receiver design](../design/slow-receiver.md) records. The plaintext build showed the same
effect in step 2: it counted 137,701 bytes sent just before its block, and the receiver's
connection ended at 84 bytes. This loss does not come from the encryption. It is the reason the
delivered frames from `ss` (989,810) are below `encoded` (999,998) in a hard outage.

**Segment on disk.** One segment was copied off the guest while the spool was active (53,809,634
bytes, seq 328, during the outage) and again earlier in the outage (28,258,652 bytes). Both:

- Start with magic `EDSE`, version 1, flags 0, key id `b1c37f136a264270` (the key id of
  `WrappedKey`).
- Walk cleanly as 32-byte header plus records: 848 data records of at most 65,649 plaintext
  bytes each; no record header out of order; sizes add up to the file length.
- Contain no `dnstap`, `Dnstap`, `protobuf`, `stress`, `auth`, or `test` byte string, and not the
  DNS wire form of the name `www.auth.stress.test`. The three-byte string `www` occurs twice in
  53.8 MB, as chance predicts. The same searches find all three strings in a plaintext sample
  (positive control). Byte entropy after the file header is 7.99999 bits a byte, and the longest
  printable ASCII run is 16 bytes.

The spool folder held `cursor.bin` only after the drain.

## 6. Tamper with one record

A backlog of two sealed segments was made again (40 s of load, receiver blocked, service stopped
while blocked, which sealed the last segment): `seg-...330.fstrm` 67,064,744 bytes (1,058 data
records and the end record, 67,035,060 plaintext bytes) and `seg-...331.fstrm` 4,765,868 bytes (77
data records, 4,763,652 plaintext bytes). A copy of the first segment was kept. One byte in the
middle of the ciphertext of record 529 (file offset 33,566,042) was changed from `0x7a` to `0x7b`
with the service stopped. The service was started with the receiver still blocked, and the block
was removed 8 s later.

```
ID2  INFO spool in use ... new_spool_key=false leftover_segments=2 leftover_bytes=71830612
ID24 WARN dnstap receiver unavailable at startup; capture is running, and frames go to the spool, ... error=connection timed out
ID2  INFO dnstap receiver reconnected receiver=172.27.215.60:6000
ID14 WARN dnstap spool discarded unreadable segment data frames=0 bytes=33531502
ID2  INFO output stats ... replayed=371605 spool_dropped=0 spool_dropped_bytes=33531502 spool_bytes=0 spool_segments=0 ... bytes_sent=38282050
ID14 WARN data loss in the spool: ... spool_dropped_bytes_increase=33531502 spool_dropped_bytes=33531502 seconds=5
```

The numbers match the prediction from the copy of the intact segment exactly:

| Quantity | Predicted | Observed |
| --- | --- | --- |
| Plaintext of records 0 to 528 of segment 330 | 33,518,398 | |
| Plaintext of segment 331 | 4,763,652 | |
| Sum, replayed | 38,282,050 | `bytes_sent=38,282,050`; receiver delta 38,282,134 (84 handshake bytes more) |
| File bytes from the header of record 529 to the end of segment 330 | 33,531,502 | `bytes=33531502`, `spool_dropped_bytes=33531502` |

The tampered record was not replayed. By design (the plan: "the rest of a corrupt segment is
dropped and counted"), the 529 records after it in the same segment were dropped with it; the
other segment replayed whole. `frames=0` and `spool_dropped=0` are the documented result for a
leftover segment: only bytes are counted, because the frame count of a leftover is unknown. The
spool folder held `cursor.bin` only afterwards.

## 7. Memory lock under the restricted token

The service runs with a restricted service SID and only `SeChangeNotifyPrivilege` (step 3). Across
all seven service starts of the new build, `spool in use` appeared each time (checked in the Event
Log: one `new_spool_key=true`, six `false`), with the spool filled and drained in steps 5, 6, and 9, and no Event ID 26 (`spool key is not usable`, with a
`cannot lock the spool key page in memory` reason) was written. The key page is therefore locked
with `VirtualLock` under that token. No Process Explorer check was made (optional).

## 8. DNS Server restart

**(a) Crash.** 50 s of load; at 07:55:17 `taskkill /f /im dns.exe` (dns.exe PID 2868). The DNS
service recovery policy waits 120 s before the first restart (`sc.exe qfailure DNS`), so after 15 s
`Start-Service DNS` was run (new dns.exe PID 2144). `EtwDnstap` was not touched: PID 1268 before
and after, `Running` throughout.

| 5 s window ends (UTC) | `received` added |
| --- | --- |
| 07:55:13 | +101,277 |
| 07:55:18 | +98,919 (dns.exe killed at the start of this window's last second) |
| 07:55:23 to 07:55:33 | 0 (DNS down) |
| 07:55:38 | +64,496 (DNS back at 07:55:33.8) |
| 07:55:43 | +120,618 |
| 07:55:48 | +103,140 |

Delivery stayed live: the service's `bytes_sent` rose by 70,035,656 and the receiver by the same
70,035,656 (`encoded` 679,866; `ring_dropped=0 etw_lost=0 disconnected_dropped=0 spooled=0`). One
warning followed the restart, from the catch-up burst of the load client:

```
WARN received rate is above 22,000 events/s; the default buffer settings were validated at about 20,000 events/s (10,000 queries per second); ... events_per_second=24020.0
```

**(b) Normal stop.** `Stop-Service DNS -Force` stopped `EtwDnstap` too (dependency): both
`Stopped`, no `etw-dnstap` process, and the service wrote its two stop lines (`capture stopped` and
`output stopped`, `stop_sent=true spool_backlog_kept=false`). Then `Start-Service DNS` and
`Start-Service EtwDnstap`: `Running`, connected, `spool in use ... new_spool_key=false`, `capture
started` with the defaults. A 25 s load followed: `encoded=499994`, `bytes_sent=51506958`,
`ring_dropped=0`, `etw_lost=0`, `disconnected_dropped=0`.

## 9. Second major upgrade with a backlog

Receiver blocked under 24 s of load, load stopped, backlog kept: `spooled=377788 spool_bytes=38935733`
in one segment, service running. The second MSI (ProductCode `{8775DA1E-...}`) was installed over
the first with the three properties while the receiver was still blocked, the guest restarted, and
the block removed 1.5 min after the start of the service (4.5 min after it was set). (The service was still stopped about 100
s after the boot and my script started it, so this start was manual. The first install's start was
automatic.)

Key state before and after the upgrade (read on the guest; the diff of the two outputs differs in
the product line, and in the key file's modification time only, because the installer reapplied
the DACL):

| Item | Before | After |
| --- | --- | --- |
| `WrappedKey` | length 396, header `01b1c37f136a264270`, first 8 bytes of its SHA-256 `dd4459e7f4ef182a` | identical |
| CNG key unique name | `38228a9d...b25d_756fd798-914b-4170-b439-10c88c333cd0` | identical; file size 2087 |
| CNG key DACL | `D:P(A;;0xd01f01ff;;;SY)(A;;0xd01f01ff;;;BA)(A;;0x80120089;;;<service SID>)` | identical |
| `Protect` SDDL | as in step 3 | identical |
| `Parameters` key | absent | absent |
| Product | `{9AB651E1-...}` | `{8775DA1E-...}` |

Install log: `Accepted existing key 'EtwDnstap Spool Key' (RSA-3072, not exportable).` and
`Accepted existing registry key SOFTWARE\ETW-DNSTapService\Protect (an existing WrappedKey value is kept).`

Event Log of the first start of the second build:

```
INFO spool in use ... new_spool_key=false leftover_segments=1 leftover_bytes=38935761
WARN dnstap receiver unavailable at startup; ... error=connection timed out
INFO dnstap receiver reconnected receiver=172.27.215.60:6000
INFO output stats ... replayed=377788 spool_dropped=0 spool_dropped_bytes=0 spool_bytes=0 spool_segments=0 ... bytes_sent=38918453
```

No deletion warning. The old process had written `spooled=377788` before the upgrade, and the new
process replayed `replayed=377788`. The plaintext of the copied leftover segment is 38,918,453
bytes (617 records, sealed), `bytes_sent` is 38,918,453, and the receiver got 38,918,537 (84
handshake bytes more). No loss.

## 10. Steady 60 s run

Fresh service start, receiver up, 60 s at 10,000 QPS, `typeperf` once a second:

| Measure | Mean over the 58 load seconds | Maximum |
| --- | --- | --- |
| Events Received/sec | 20,406 | 38,598 (one second) |
| `dns.exe` CPU (percent of one logical processor) | 16.2 | 30.8 |
| `etw-dnstap` process CPU | 0.77 | 6.2 |
| Stage CPU, ETW consumer | 0.34 | 1.54 |
| Stage CPU, parser | 0.34 | 1.54 |
| Stage CPU, sender | 0.03 | 1.55 |
| Stage CPU, link | 0.05 | 1.55 |
| Total system CPU (12 vCPUs) | 2.7 percent | 5.3 percent |
| Working set / private bytes of the service | 356 MB / 392 MB | 367 MB / 398 MB |

`received=1199998 encoded=1199998 ring_dropped=0 etw_lost=0 disconnected_dropped=0
queue_dropped=0 spooled=0 spool_errors=0`; `bytes_sent=123,618,034` and the receiver got the same
number plus the 84 handshake bytes. Of the 51 Event Log entries in the two minutes of the run, all were Information
(the start lines and the 5-second stats lines). No warning. The spool was not used, so this run
measures the idle cost of the encrypted spool, which is none.

## 11. Stop rule

With the receiver up and connected, `Stop-Service EtwDnstap` wrote `capture stopped` and `output
stopped` (`stop_sent=true spool_backlog_kept=false`) and nothing else, apart from two regular
5-second stats lines that fell in the same second. The spool held `cursor.bin` only.

## Deviations

- The block in step 5 lasted about 40 s, not 30 s (two segment copies ran inside it). Step 2 and
  step 6 backlogs came from 24 s and 40 s loads.
- A driver error: the service restart that was meant to precede the loads of steps 5, 6, and 8a
  ran on the host and did nothing (a host has no `EtwDnstap` service). For step 5 the counters
  were still zero (the service had been restarted for step 4). For steps 6 and 8a, the numbers
  above are differences of the cumulative counters (`bytes_sent`, `received`) or come from the
  counters of a start that followed a deliberate stop (step 6). Fresh starts were used for steps 8b,
  9 (after the upgrade), and 10.
- In step 9 the service was started by hand about 100 s after the boot.
- The receiver's `iptables -I` created an empty `table ip filter` in nft. It was deleted so the
  final record matches the first line for line.
- Not run: the other-local-account unwrap test, the 50,000 and 200,000 QPS throughput runs, and a
  Process Explorer check of the locked page.

## Lab state after the run

- `DNSTap-WS2025` runs the second MSI (ProductCode `{8775DA1E-6CBE-4DCC-ACBA-1FDE5A334D83}`,
  SHA-256 `17BF32EA...F55950`) with the three properties. `EtwDnstap` is `Running`, connected to
  `172.27.215.60:6000`. `C:\ProgramData\EtwDnstap\spool` holds `cursor.bin` only. The CNG key and
  `WrappedKey` are the ones from step 3. The checkpoint `spool-encryption-pre-2026-11-04` exists;
  nothing was restored.
- `DNSTap-Ubuntu2604`: no iptables rule of the run is left. The record of tc, iptables, ip6tables,
  and nft is identical to the first record, line for line. The collector still has the `devnull`
  output and `StandardOutput=null`.
