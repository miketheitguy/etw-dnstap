# Configuration reference

> **Status:** Current · **Describes:** the code at commit `66c3de4` · **Reviewed:** 2026-11-05

### Quick Start

Set options in the service binary path. The service ignores arguments to `sc.exe start`. The MSI builds the path from MSI properties ([Install with the MSI](install.md#install-with-the-msi)).

```text
"C:\Program Files\EtwDnstap\etw-dnstap.exe" --receiver 192.0.2.10:6000 --capacity 131072
```

Only `--receiver` is required. Add `--plaintext` for a receiver without TLS.

### Steps / Configuration

#### Options

| Option | Default | Range or format | Meaning |
| --- | --- | --- | --- |
| `--receiver IP:PORT` | required | Numeric unicast IP, nonzero port; IPv6 `[addr]:port` | Frame Streams receiver |
| `--plaintext` | off | Flag | No TLS, no peer authentication |
| `--tls-server-name NAME` | receiver IP | DNS name or IP; not with `--plaintext` | Certificate name ([TLS](#tls)) |
| `--receiver-stall-seconds SECONDS` | `15` | 2 to 300 | Stall limit before reconnect |
| `--capacity N` | `131072` | 1,024 to 524,288 events | Event channel size |
| `--jumbo-capacity N` | `64` | 0 to 256 | Slots over 2 KiB; `0` disables |
| `--duration SECONDS` | none | 1 to 604,800 | Stop after this long |
| `--session-name NAME` | `EtwDnstap` | 1 to 128 ASCII letters, digits, `_`, `.`, `-`; no leading hyphen | ETW session name |
| `--identity TEXT` | `etw-dnstap` | Up to 1,024 UTF-8 bytes | dnstap identity; empty omits |
| `--etw-buffer-kib KIB` | `32` | 4 to 128, multiple of 4 | ETW buffer size ([Defaults](tuning.md#defaults)) |
| `--etw-min-buffers N` | `12` | 0 to 4,096, at most `--etw-max-buffers` | Reserved at start; `0` lets ETW choose ([ETW buffer minimums](tuning.md#etw-buffer-minimums)) |
| `--etw-max-buffers N` | `64` | 4 to 4,096 | On-demand maximum ([Recommended settings for higher rates](tuning.md#recommended-settings-for-higher-rates)) |
| `--etw-flush-seconds SECONDS` | `1` | 1 to 60 | ETW delivery interval |
| `--spool-dir PATH` | `C:\ProgramData\EtwDnstap\spool` | Absolute, drive letter, no `.` or `..`; not with `--no-spool` | Spool folder ([Requirements](#requirements)) |
| `--no-spool` | off | Flag; not with `--spool-dir`, `--spool-max-mib`, `--spool-min-free-mib` | No spool |
| `--spool-max-mib N` | `4096` | 64 to 1,048,576 MiB; not with `--no-spool` | Segment byte cap |
| `--spool-min-free-mib N` | `2048` | 0 to 1,048,576 MiB; not with `--no-spool` | Free space floor |
| `--stats-interval-seconds N` | `60` | 0 to 86,400 | `capture stats` and `output stats` interval; `0` disables |

- A full channel drops the newest event (`ring_dropped`, `intake_dropped`). A longer `--etw-flush-seconds` bounds latency less and uses less CPU.
- A new `--session-name` needs a new ETW grant and a restart ([install.md](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source)).
- With `--no-spool`, lost frames count in `disconnected_dropped` or `queue_dropped`.
- No help or version output. File version: [build.md](build.md#build-the-executable). Running version: `version` in `capture started` ([monitoring.md](monitoring.md#periodic-and-final-reports)).

#### Diagnostic flags (`diag-timing` builds only)

Only in a `--features diag-timing` binary; never in production. Adds parser park, socket write, and flush timings to the stop report.

| Option | Range | Effect |
| --- | --- | --- |
| `--diag-timer-resolution-ms MS` | 1 to 15 | Holds `timeBeginPeriod` |
| `--diag-thread-priority LEVEL` | `above-normal` or `highest` | Raises `dnstap-parser`, `etw-consumer` priority; failure logs a warning |

#### Storage

Counted storage is payload, slots, free list, and channel ([pooled-intake.md](../design/pooled-intake.md)).

| `--capacity` | Jumbo slots | Counted storage |
| --- | --- | --- |
| 131,072 (default) | 64 | 283.03 MiB |
| 262,144 | 64 | 558.03 MiB |
| 524,288 (maximum) | 256 (maximum) | 1,132.11 MiB |

The 516-block pool (136 KiB each) adds about 68.5 MiB at startup. Allocator, native ETW buffers, TDH caches, protobuf data, and output buffers aren't counted.

#### Subscription

`Microsoft-Windows-DNSServer`, level `0xFF`, own real-time session. The service resolves the `QUERY_RECEIVED`, `RESPONSE_SUCCESS`, and `RESPONSE_FAILURE` keyword bits by name and sets `MatchAnyKeyword` (`0x7` on the lab VM).

- Lookup fails, `QUERY_RECEIVED` or `RESPONSE_SUCCESS` missing or malformed, or two names share a bit: one warning, mask `0x8000000000000000` (Analytical channel bit).
- Only `RESPONSE_FAILURE` missing or malformed: mask `QUERY_RECEIVED | RESPONSE_SUCCESS`, no failed responses. Warning: starts "RESPONSE_FAILURE keyword is not available", ends "failed responses are not captured".
- `capture started` shows the masks, `keyword_fallback`, and `failed_responses_captured` (`false` only in the partial case).

| Event ID | Meaning | dnstap message type |
| --- | --- | --- |
| 256 | Query received | `CLIENT_QUERY` |
| 257 | Response success | `CLIENT_RESPONSE` |
| 258 | Response failure | `CLIENT_RESPONSE` |

- SERVFAIL and policy REFUSED arrive as 258. A 258 with the QR bit clear counts in `rejected`. NXDOMAIN arrives as 257.
- IGNORE policy produces 259 (`IGNORED_QUERY`, no packet), but its query arrives as 256. Events 259, 260, and 261 count nowhere ([README message mapping](../../README.md#what-is-captured)).
- ETW filters IDs 256 to 258 before buffering (keyword 0 never matches). The keywords also enable EDNS 284, 285, 287, `DNSSEC_VALIDATION_FAILURE` (288), and DNS over HTTPS (597 to 599): they cost `dns.exe` CPU and the callback drops them.

#### No process filter

Events from any process are accepted, so capture follows a DNS Server crash restart.

- The service depends on `DNS`: a normal DNS Server stop stops EtwDnstap; a crash restart doesn't.
- Starting EtwDnstap doesn't check that `DNS` runs.
- Any local process that can register the provider GUID can write captured events ([SECURITY.md](../../SECURITY.md)).

#### Message and transport limits

Event 256 sets `CLIENT_QUERY`; 257 and 258 set `CLIENT_RESPONSE`. A field is set only when the provider reports it.

- **Fields:** `socket_protocol` (`UDP`/`TCP`); `socket_family` (`INET`/`INET6`; mixed-family addresses rejected); `query_address`, `query_port`; `response_address` (wildcard and port 0 count as not reported); `query_time_sec`/`query_time_nsec` (query only); `response_time_sec`/`response_time_nsec` (response only); `query_message` (256); `response_message` (257, 258); `identity` (absent when empty). `response_port`, `version`, `extra` stay unset.
- **Times:** ETW UTC, 100 ns. Before the Unix epoch: rejected, counted in `rejected`.
- **Encoding:** content type `protobuf:dnstap.Dnstap`. Limits: 512 bytes control payloads, 256 content-type fields, 70 KiB encoded messages.
- **Deadlines:** TCP connect 2 seconds; handshake (TLS, READY, ACCEPT, START) 2 seconds from TCP open; STOP and FINISH share 2 seconds. Trickled bytes don't extend one, and certificate verification time counts. Data writes have none (see **Stall**).
- **Send buffer:** `SO_SNDBUF` unset. **Flush:** idle parser sends at 100 ms since the last send; busy parser at least once a second, checking every 64 events.
- **DNS envelope:** 12 to 65,535 bytes, QR bit matching direction, verbatim, no TCP length prefix. Malformed is rejected.

#### Receiver connection

One Frame Streams session. Network failures recover; protocol failures at startup don't.

- **Startup:** one attempt. Network failure (refused, timed out, reset): warning, start disconnected, retry. Protocol failure (rejected `protobuf:dnstap.Dnstap` content type, malformed control frame) stops the service.
- **Stall:** no byte accepted for `--receiver-stall-seconds` with data waiting ends the session. Idle never stalls. Shorter stalls queue about 16 MiB, then spool; with `--no-spool`, more `queue_dropped`.
- **Loss:** a failed write or stall reconnects. Frames not fully accepted return to the spool head (or the next session's front with a backlog) and resend in full; readers discard the truncated copy. With TLS the whole 16 KiB piece returns. With a spool, `disconnected_dropped` stays flat.
- **Retry:** wait uniform between half the base and the base. Base 250 ms, doubling, cap 30 seconds, reset on success. Each session repeats READY, ACCEPT, START.
- **While down:** spool, or `disconnected_dropped` without one. A wrong content type after startup is a failed attempt; if the receiver was down at startup, the first attempt that reaches it logs one Event ID 4 error.
- **Reconnect log:** failures count in `receiver_reconnect_failures`. `dnstap reconnect failed` logs the first four, then the 61st, 121st, and so on, plus the first of each `cause` (`connect`, `timeout`, `tls`, `protocol`) since the last session. `dnstap receiver connection lost` marks each outage ([monitoring.md](monitoring.md#receiver-schema-and-rate-messages)).
- **Shutdown:** disconnected sends no STOP. Connected sends STOP and waits up to 2 seconds for FINISH; failure doesn't fail the stop. If the 1-second link budget ends mid-block, the sink closes without STOP and unsent frames stay spooled.

#### TLS

On unless `--plaintext`. A receiver that answers the handshake with plaintext is a protocol failure; one that stays silent or closes is a network failure (retry).

- **Versions:** TLS 1.2 and 1.3, rustls with `ring`.
- **Verification:** Windows CryptoAPI against the machine store, including Group Policy roots. The certificate must match `--tls-server-name`, or the receiver IP. SNI only for a DNS name; for an IP, use an `iPAddress` SAN.
- **Revocation:** best effort, receiver certificate only. OCSP or CRL unreachable: continue. Revoked: reject.
- **Failure class:** a failed certificate check or post-handshake alert is a protocol failure, `cause` `tls`: Event ID 1 and stop on first connection, a counted failed attempt on reconnect, one Event ID 4 error on the first failed handshake after a receiver-down start.

TCP closes after FINISH with no TLS close_notify. Certificate requirements: [receiver.md](receiver.md#receiver-certificate).

#### Spool

Frames wait on disk while the receiver is down or slow. On by default; stored frames go first, oldest first. You create the folder ([Create the spool folder](install.md#create-the-spool-folder)).

**The spool stores DNS data** (client addresses, query names). Segments are encrypted and the folder ACL keeps other accounts out ([Encryption](#encryption), [SECURITY.md](../../SECURITY.md)).

##### Requirements

Checked once, at startup (default moves to `C:\ProgramData` if `ProgramData` isn't a plain local path). The folder must:

- Exist, and be a plain folder: it and its parent aren't reparse points, and the drive root is refused.
- Be on a fixed local disk (not removable, network, or RAM).
- Be owned by SYSTEM or Administrators.
- Grant read and write only to SYSTEM, Administrators, or a service SID. An inherit-only `CREATOR OWNER` allow entry is allowed. Deny entries don't count.

A failed check, an unloadable key, or a failed open logs one warning and runs without a spool ([warning text](troubleshooting.md#spool-warnings)). Files survive restarts and reboots. Nothing rechecks, so the install ACL must keep other accounts out.

##### Encryption

Each appended block is one AES-128-GCM record. A segment adds a 32-byte header, each record 28 bytes, a sealed segment a 28-byte end record. Frames in memory, the send queue, and on the wire are unencrypted.

- **Machine key:** non-exportable RSA-3072 `EtwDnstap Spool Key` in the Microsoft Software Key Storage Provider, made by the installer. Only the service, SYSTEM, and Administrators can use it ([Install](install.md)).
- **Wrapped AES key:** made at first start, wrapped by the RSA key, stored in `HKLM\SOFTWARE\ETW-DNSTapService\Protect\WrappedKey`. A major upgrade keeps it; uninstall deletes it.
- **In memory:** locked page outside the pagefile and WER dumps, encrypted between records (about 75 µs of CPU a second at 10,000 queries per second).
- **Memory-protection failure:** `CryptUnprotectMemory` failing fails that operation (Event ID 27 on a write); later ones retry. `CryptProtectMemory` failing twice, or a bad decrypted key, zeroes the key and suspends the spool until restart: appends refused and counted in `spool_dropped`, one warning (Event ID 26) naming the call and Win32 error. Live frames still reach an up receiver; the next start replays the backlog.
- **Key load failure:** no spool, one warning naming the cause. The key store wait is at most 3 seconds ([Spool key errors](troubleshooting.md#spool-key-errors)).

Doesn't protect against: an offline copy of a system volume without BitLocker (offline DPAPI tools recover every segment), SYSTEM, Administrators, code in the service process, a full kernel memory capture (dump, `hiberfil.sys`, VM saved state), or a dump during a seal or open ([SECURITY.md](../../SECURITY.md)).

Startup deletes unreadable segments and counts the bytes in `spool_dropped_bytes`: plaintext from before encryption, another key's segment, a non-segment file, and all segments when this start made a new key (also resets `cursor.bin`). One warning, `dnstap spool deleted segments it cannot read at startup` (Event ID 14), gives the counts. A record that fails authentication is unreadable and never replays.

##### Routing

Per block, the router thread picks one destination:

- **Link FIFO:** receiver connected, spool empty. 256 blocks (about 16 MiB), in memory.
- **Spool:** receiver down, spool holds frames, or FIFO full. A full FIFO moves all its blocks to the spool, oldest first, then the new block.
- **Dropped:** no spool, receiver down: `disconnected_dropped`.

A refused spool block counts in `spool_dropped`. The FIFO absorbs a slow receiver, not an outage. When the parser-to-router send queue is full, the parser drops frames (`queue_dropped`): a slow disk with a spool, a full link FIFO without one.

##### Order

While the spool holds frames, every new block goes to it, so frames arrive in capture order. After a reconnect, the router replays the oldest frames in batches of up to 64 KiB, up to 127 at a time, at most 1 MiB read per pass. A segment stays until the connection has taken all of it. Earlier-run frames replay first; each run writes new segments. `cursor.bin` holds the position after the last frame taken (a byte offset in the segment's decrypted frames), saved once per router pass.

##### Size limits and eviction

- `--spool-max-mib` caps segment file bytes (header and 28 bytes per record included). Over the cap, the oldest segments go.
- `--spool-min-free-mib`: a new segment starting below the floor plus one segment removes the oldest segments, stopping when removal doesn't raise free space. With only the newest left, blocks are refused for 10 seconds (`spool_dropped`), then rechecked. Warnings give free and needed MiB for the first four pauses, then the 61st, 121st, and so on.
- A segment holds up to 64 MiB, or one eighth of the cap when smaller.
- Data read but not yet taken (at most about 9 MiB) keeps its segments whole and counts toward the cap: 24.7 MiB total at the smallest cap, about 137 MiB with 64 MiB segments (caps of 512 MiB and above).
- Each removal counts in `spool_dropped` and `spool_dropped_bytes`, and the warning gives both. A leftover earlier-run segment adds only bytes.

Sizing: [Spool sizing](tuning.md#spool-sizing).

<a id="delivery-is-at-most-once"></a>

##### Delivery after a failure, a stop, and a crash

- A running service resends only frames the socket didn't fully accept (**Loss**, [Receiver connection](#receiver-connection)).
- Bytes the socket accepted but the kernel hadn't sent are lost uncounted if the connection fails (send buffer: several MiB).
- A normal stop repeats nothing.
- A crash or kill resends: the next run starts at `cursor.bin`, repeating replay data read but not taken (up to 127 batches, 8 MiB at 103 bytes a frame) and frames taken since the last save. A receiver can see them twice: at least once.
- The OS writes to disk; a power loss can lose recent frames.

##### Shutdown

The router routes queued blocks, saves `cursor.bin`, and seals the active segment. The backlog stays. Write cutoff: [Startup and shutdown deadlines](#startup-and-shutdown-deadlines).

#### Startup and shutdown deadlines

- **Startup:** 10 seconds plus 1 per 16,384 `--capacity` slots, rounded up (24,576: 12; 131,072: 18; 524,288: 42). Covers identity checks, argument validation, receiver connection, spool open, pool allocation, and capture start (own inner 10 seconds).
- **SCM progress:** first `START_PENDING` hint 10 seconds, then the full deadline with the checkpoint advancing every 16,384 pool slots (`sc.exe query EtwDnstap`: `CHECKPOINT`, `WAIT_HINT`). The `STOP_PENDING` hint is the shutdown deadline.
- **Shutdown:** 10 seconds from stop request to final report. Up to 5 seconds for `ProcessTrace`, then the trace handle closes. The router seals the segment while the link thread runs STOP/FINISH. Receiver writes stop 1 second in; unsent blocks go to the spool. The sink ends in about 3.2 seconds at most, about 1.1 with a stalled receiver.
- **Expiry:** Event ID 5, then abort, so SCM recovery applies ([Watchdog aborts](troubleshooting.md#watchdog-aborts-or-stop-hangs)).

#### Session reclaim

SCM starts one process; the identity check refuses others. At startup the service replaces a leaked session with the same name, GUID, and shape (real-time only, no file, circular, append, new-file, system, buffering, or private logger mode). Buffer size, counts, flush timer, and clock type aren't in the shape: a different `--etw-buffer-kib` logs `stale ETW session used a different buffer size; reclaiming anyway` and continues.

### Common Gotchas

- A value outside its range, a repeated option, or an unlisted option stops the service from starting.
- A session with the same name and a different GUID is left alone, and startup fails. The GUID comes from the exact spelling of `--session-name`, though ETW ignores case: use the same spelling in the ETW grant or start fails with access denied.
- TLS failures: check the Windows trust store, the match with `--tls-server-name` (or receiver IP), and that the receiver requires no client certificate. A TLS 1.3 receiver that requires one sends `received fatal alert: CertificateRequired`.
- A bad spool folder, unloadable key, or failed open means no spool.
- A crash resends frames: delivery is at least once.
