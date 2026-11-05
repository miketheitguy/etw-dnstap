# Configuration reference

> **Status:** Current · **Describes:** the code at commit `66c3de4` · **Reviewed:** 2026-11-05

This page lists every option, its default, and its allowed range. Set options in the service
binary path. The service ignores arguments to `sc.exe start`. An MSI install builds the path
from MSI properties. See [Install with the MSI](install.md#install-with-the-msi).

Related: [install.md](install.md), [troubleshooting.md](troubleshooting.md),
[tuning.md](tuning.md), [monitoring.md](monitoring.md).

## Options

The defaults and ranges are set in `CaptureArgs` in `src/pipeline/config.rs`. A value outside
its range, a repeated option, or an unlisted option stops the service from starting.

| Option | Default | Range or format | Meaning |
| --- | --- | --- | --- |
| `--receiver IP:PORT` | none (required) | Numeric unicast IP and nonzero port. IPv6 as `[addr]:port`. | Frame Streams receiver. TLS unless `--plaintext`. |
| `--plaintext` | off | Flag | Disables TLS and peer authentication. Use only on a trusted network. |
| `--tls-server-name NAME` | the receiver IP | A DNS name or an IP literal. Cannot be combined with `--plaintext`. | Name the receiver certificate must match. See [TLS](#tls). |
| `--receiver-stall-seconds SECONDS` | `15` | 2 to 300 | Seconds the receiver may accept no bytes, with data waiting, before reconnect. See [Receiver connection](#receiver-connection). |
| `--capacity N` | `131072` | 1,024 to 524,288 events | Event channel size. A full channel drops the newest event (`ring_dropped`, `intake_dropped`). |
| `--jumbo-capacity N` | `64` | 0 to 256 | Pool slots for payloads over 2 KiB. `0` disables jumbo capture. |
| `--duration SECONDS` | none | 1 to 604,800 | Stop after this many seconds. |
| `--session-name NAME` | `EtwDnstap` | 1 to 128 ASCII letters, digits, underscore, dot, or hyphen. The first character must not be a hyphen. | Real-time ETW session name. A new name needs a new ETW grant and a restart. See [Session reclaim](#session-reclaim) and [install.md](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source). |
| `--identity TEXT` | `etw-dnstap` | At most 1,024 UTF-8 bytes | dnstap producer identity. Empty text omits the field. |
| `--etw-buffer-kib KIB` | `32` | 4 to 128, a multiple of 4 | ETW session buffer size. See [Defaults](tuning.md#defaults). |
| `--etw-min-buffers N` | `12` | 0 to 4,096, at most `--etw-max-buffers` | Buffers reserved at session start. `0` lets ETW choose. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). |
| `--etw-max-buffers N` | `64` | 4 to 4,096 | Maximum buffers, allocated on demand. See [Recommended settings for higher rates](tuning.md#recommended-settings-for-higher-rates). |
| `--etw-flush-seconds SECONDS` | `1` | 1 to 60 | ETW delivery interval, which bounds latency. Longer values use less CPU and make batches larger. |
| `--spool-dir PATH` | `C:\ProgramData\EtwDnstap\spool` | An absolute path that starts with a drive letter, with no `.` or `..` part. Cannot be combined with `--no-spool`. | Folder for frames that wait for the receiver. You create it. See [Requirements](#requirements). |
| `--no-spool` | off | Flag. Cannot be combined with `--spool-dir`, `--spool-max-mib`, or `--spool-min-free-mib`. | Runs without a spool. Lost frames count in `disconnected_dropped` or `queue_dropped`. |
| `--spool-max-mib N` | `4096` | 64 to 1,048,576 MiB. Cannot be combined with `--no-spool`. | Cap on spool segment bytes. See [Size limits and eviction](#size-limits-and-eviction). |
| `--spool-min-free-mib N` | `2048` | 0 to 1,048,576 MiB. Cannot be combined with `--no-spool`. | Free space to keep on the spool volume. See [Size limits and eviction](#size-limits-and-eviction). |
| `--stats-interval-seconds N` | `60` | 0 to 86,400 | Seconds between `capture stats` and `output stats` events. `0` disables them. See [Log events and counters](monitoring.md). |

The binary prints no help or version text. For the file version, see
[build.md](build.md#build-the-executable). For the running version, see the `version` field of
`capture started` in [monitoring.md](monitoring.md#periodic-and-final-reports).

### Diagnostic flags (`diag-timing` builds only)

These options exist only in a binary built with `--features diag-timing`. Do not use them in
production. The feature also adds parser park, socket write, and flush timings to the stop report.

| Option | Range | Effect |
| --- | --- | --- |
| `--diag-timer-resolution-ms MS` | 1 to 15 | Holds `timeBeginPeriod` for the run. |
| `--diag-thread-priority LEVEL` | `above-normal` or `highest` | Raises the `dnstap-parser` and `etw-consumer` thread priority. A failed call logs a warning. |

## Storage

Counted storage covers payload, slots, free list, and channel. It excludes the allocator, native
ETW buffers, TDH caches, protobuf data, and output buffers. See
[pooled-intake.md](../design/pooled-intake.md).

| `--capacity` | Jumbo slots | Counted storage |
| --- | --- | --- |
| 131,072 (default) | 64 | 283.03 MiB |
| 262,144 | 64 | 558.03 MiB |
| 524,288 (maximum) | 256 (maximum) | 1,132.11 MiB |

The block pool adds about 68.5 MiB at startup (516 blocks of 136 KiB) and is not in these totals.

## Subscription

The service enables `Microsoft-Windows-DNSServer` at level `0xFF` in its own real-time session. It
resolves the `QUERY_RECEIVED`, `RESPONSE_SUCCESS`, and `RESPONSE_FAILURE` keyword bits by name and
sets `MatchAnyKeyword` (`0x7` on the lab VM).

- If the lookup fails, if `QUERY_RECEIVED` or `RESPONSE_SUCCESS` is missing or malformed, or if
  two names share a bit, the service logs one warning and uses `0x8000000000000000`, the
  Analytical channel bit.
- If only `RESPONSE_FAILURE` is missing or malformed, the mask becomes `QUERY_RECEIVED |
  RESPONSE_SUCCESS`, and failed responses are not captured. The warning starts with
  "RESPONSE_FAILURE keyword is not available" and ends with "failed responses are not captured".

`capture started` shows the masks, `keyword_fallback`, and `failed_responses_captured`. The last
field is `false` only in the partial case.

The service accepts only these events:

| Event ID | Meaning | dnstap message type |
| --- | --- | --- |
| 256 | Query received | `CLIENT_QUERY` |
| 257 | Response success | `CLIENT_RESPONSE` |
| 258 | Response failure, for example SERVFAIL | `CLIENT_RESPONSE` |

SERVFAIL and policy REFUSED arrive as 258. A 258 whose packet has the QR bit clear counts in
`rejected`. NXDOMAIN arrives as 257. An IGNORE policy produces 259 (`IGNORED_QUERY`) with no
packet, but its query arrives as 256. The service counts events 259, 260, and 261 nowhere. See the
[README message mapping](../../README.md#what-is-captured).

ETW filters IDs 256 to 258 before the session buffers (keyword 0 events never match), which cuts
buffer use and callbacks but not the DNS Server's cost to generate events. The keywords also
enable EDNS variants 284, 285, and 287, `DNSSEC_VALIDATION_FAILURE` (288), and DNS over HTTPS
(597 to 599). They cost `dns.exe` CPU, and the callback drops them.

### No process filter

The subscription accepts `Microsoft-Windows-DNSServer` events from any process, so capture follows
a DNS Server crash and recovery restart: the new `dns.exe` is captured when it writes events.

- The service depends on the `DNS` service, so a normal stop of the DNS Server also stops
  EtwDnstap. A crash that the recovery policy restarts does not.
- Starting EtwDnstap does not check that `DNS` is running.
- Any local process that can register the provider GUID can write events that the service
  captures. See [SECURITY.md](../../SECURITY.md).

## Message and transport limits

Event 256 sets the envelope type `CLIENT_QUERY`. Events 257 and 258 set `CLIENT_RESPONSE`. A field
is set only when the provider reports it.

| Field | Value |
| --- | --- |
| `socket_protocol` | `UDP` or `TCP`, from the event |
| `socket_family` | `INET` or `INET6`. The encoder rejects a record whose two addresses differ in family. |
| `query_address`, `query_port` | The client's address and port |
| `response_address` | The DNS Server's local interface address. `response_port` is never set. A wildcard address and port 0 count as not reported. |
| `query_time_sec`, `query_time_nsec` | Set on a query |
| `response_time_sec`, `response_time_nsec` | Set on a response, which has no query time |
| `query_message`, `response_message` | The DNS packet: `query_message` for event 256, `response_message` for 257 and 258 |
| `identity` | The `--identity` text. Absent when empty. |

Times are ETW event timestamps in UTC, at 100 ns resolution. The encoder rejects a timestamp before
the Unix epoch, and the parser counts a rejected record in `rejected`. The service leaves `version`
and `extra` unset.

- **Encoding:** Content type `protobuf:dnstap.Dnstap`. Limits: 512 bytes for control payloads, 256
  bytes for content-type fields, 70 KiB for encoded messages.
- **Deadlines:** The TCP connect has a 2-second timeout. The handshake (TLS, READY, ACCEPT, START)
  has one 2-second deadline from TCP open. STOP and FINISH share one 2-second deadline. Trickled
  bytes do not extend a deadline, and certificate verification time counts. A data write has no
  deadline: see **Stall** under [Receiver connection](#receiver-connection).
- **Socket send buffer:** The service leaves `SO_SNDBUF` unset so Windows keeps tuning it.
- **Flush:** An idle parser sends its partial block once the last send is 100 ms old. A busy parser
  sends at least once a second, checking the clock every 64 events.
- **DNS envelope:** The DNS message is 12 to 65,535 bytes, and its QR bit must match the event
  direction. Packet bytes are kept verbatim, with no TCP length prefix. A malformed packet is
  rejected.
- **Standards:** The layout follows the
  [dnstap protobuf schema](https://github.com/dnstap/dnstap.pb/blob/main/dnstap.proto) (CC0) and
  the [fstrm control reference](https://github.com/farsightsec/fstrm/blob/master/fstrm/control.h).
  Event names and payload fields follow
  [Microsoft's DNS logging documentation](https://learn.microsoft.com/en-us/windows-server/networking/dns/dns-logging-and-diagnostics).

## Receiver connection

The sink keeps one Frame Streams session to `--receiver`. Network failures are recoverable.
Protocol failures at startup are not.

- **Startup:** The first connection is attempted once. A network failure (refused, timed out, or
  reset) logs a warning, and the service starts disconnected and retries. A protocol failure
  stops the service, such as a receiver that rejects the `protobuf:dnstap.Dnstap` content type or
  sends a malformed control frame.
- **Stall:** A session ends when the socket accepted no byte for `--receiver-stall-seconds` while
  the link had data to write. An idle connection never stalls. A receiver that is slow or paused
  for less than the stall time keeps its session: the service holds up to about 16 MiB in the
  link queue, then the spool takes the data. With `--no-spool`, a longer stall means more
  `queue_dropped`.
- **Loss:** A failed write or a stall ends the session. The sink logs a warning and reconnects.
  Frames that the socket did not fully accept go back to the head of the spool (or the front of
  the next session when the spool holds a backlog) and are sent again in full. Frame Streams
  readers discard the truncated copy. With TLS, the whole 16 KiB piece whose delivery is unknown
  goes back too. With a spool, nothing counts in `disconnected_dropped` at a failure.
- **Retry:** Each attempt runs on a helper thread. Each wait is uniform between half of a base
  delay and the base. The base starts at 250 ms, doubles, caps at 30 seconds, and resets after a
  successful reconnect. Every new session repeats READY, ACCEPT, and START.
- **While down:** Frames go to the spool, or count in `disconnected_dropped` without one.
- **Wrong content type after startup:** The attempt counts as failed, and the service keeps
  retrying. If the receiver was down at startup, the first attempt that reaches it logs one
  Event ID 4 error that names the cause.
- **Reconnect log:** Each failure counts in `receiver_reconnect_failures`. The service logs
  `dnstap reconnect failed` for the first four failures, then the 61st, 121st, and so on, plus the
  first failure of each `cause` (`connect`, `timeout`, `tls`, or `protocol`) since the last
  established session. `dnstap receiver connection lost` marks each outage. See
  [Log events and counters](monitoring.md#receiver-schema-and-rate-messages).
- **Shutdown:** While disconnected, the sink sends no STOP, and spooled frames stay on disk. While
  connected, it sends STOP and waits for FINISH within the 2-second deadline. A STOP or FINISH that
  does not complete does not fail the stop. If the 1-second link budget ends while the link is
  writing a block, the sink closes the connection with no STOP, and the unsent frames stay in the
  spool.

### TLS

TLS wraps the session unless `--plaintext` is set. With `--plaintext`, the connection is plaintext
TCP with no peer authentication. A receiver that answers the TLS handshake with plaintext fails it
as a protocol failure. A receiver that stays silent or closes during the handshake is a network
failure, and the service retries.

- **Versions:** TLS 1.2 and TLS 1.3, through rustls with the `ring` provider.
- **Verification:** Windows CryptoAPI checks the certificate against the machine store, including
  roots that Group Policy adds. The certificate must match `--tls-server-name`, or the receiver IP
  address when that flag is unset. The service sends SNI only for a DNS name. For an IP address,
  the certificate should carry an `iPAddress` SAN.
- **Revocation:** The verifier checks only the receiver certificate, on a best-effort basis. If the
  status is unavailable (an unreachable OCSP responder or CRL endpoint), the connection continues.
  A certificate reported as revoked is rejected. Hard-fail revocation is not enforced.
- **Failure class:** A failed certificate check or a TLS alert after the handshake is a protocol
  failure, with `cause` `tls`. On the first connection it stops the service with Event ID 1. On a
  reconnect it is a counted failed attempt, and the service keeps retrying. If the receiver was
  down at startup, the first attempt that fails the handshake logs one Event ID 4 error. Check
  that:
  - The Windows trust store trusts the receiver certificate.
  - `--tls-server-name`, or the receiver IP when unset, matches the certificate.
  - The receiver requires no client certificate. The service presents none, and a TLS 1.3
    receiver that requires one sends `received fatal alert: CertificateRequired`.

FINISH ends the Frame Streams session, then TCP closes, with no TLS close_notify alert. Receiver
certificate requirements are in [receiver.md](receiver.md#receiver-certificate).

## Spool

The spool keeps frames on disk while the receiver is down or slow. It is on by default. After a
reconnect, the service sends stored frames first, oldest first. It needs a protected folder that
you create. See [Create the spool folder](install.md#create-the-spool-folder).

**The spool stores DNS data.** Each frame is a complete dnstap message with client addresses and
query names. The service encrypts every segment file, and the folder ACL keeps other accounts out.
The encryption does not protect an offline copy of a system volume that BitLocker does not protect.
See [Encryption](#encryption) and [SECURITY.md](../../SECURITY.md).

### Requirements

The service checks the folder once, at startup. If `ProgramData` is not a plain local path, the
default folder moves to `C:\ProgramData`. The folder must:

- Exist, and be a folder.
- Be a plain folder: neither it nor its parent may be a reparse point such as a link or junction,
  and the drive root is refused.
- Be on a fixed local disk. Removable, network, and RAM disks fail.
- Be owned by SYSTEM or Administrators.
- Grant read and write only to SYSTEM, Administrators, or a service SID. An inherit-only
  `CREATOR OWNER` allow entry is also allowed. Deny entries do not count.

If a check fails, the spool key cannot be loaded, or the spool cannot open, the service logs one
warning and runs without a spool. See [troubleshooting.md](troubleshooting.md#spool-warnings) for
the warning text. Files survive restarts and reboots. The check runs only at startup, so the
install ACL must keep other accounts out afterward.

### Encryption

Each block of frames that the router appends becomes one record, encrypted and authenticated with
AES-128-GCM. A segment file adds a 32-byte header, each record adds 28 bytes, and a sealed segment
ends with a 28-byte end record. Frames in memory, in the send queue, and on the wire are
unencrypted.

- **Machine key:** The installer creates a non-exportable RSA-3072 key, `EtwDnstap Spool Key`, in
  the Microsoft Software Key Storage Provider. Only the service, SYSTEM, and Administrators can use
  it. See [Install](install.md).
- **Wrapped AES key:** At its first start, the service makes a random AES key, wraps it with the
  RSA key, and stores it in `HKLM\SOFTWARE\ETW-DNSTapService\Protect\WrappedKey`. A major upgrade
  keeps it, and uninstall deletes it.
- **In memory:** The service unwraps the key at each start into a locked page that stays out of the
  pagefile and WER crash dumps. The key is encrypted there between records, at about 75 µs of CPU a
  second at 10,000 queries per second.
- **Memory-protection failure:** If `CryptUnprotectMemory` fails, the operation fails (Event ID 27
  on a write) and a later one tries again. If `CryptProtectMemory` fails twice, or the decrypted
  key fails its check, the service zeroes the key and suspends the spool until restart. It
  refuses every append, counts the frames in `spool_dropped`, and logs one warning (Event ID 26)
  that names the failed call and its Win32 error. Live frames go straight to the receiver when it
  is up. The next start replays the kept backlog.
- **Key load failure:** The service runs without a spool and logs one warning that names the cause.
  It waits at most 3 seconds for the key store, so a key problem never holds up the service start
  or stop.

For recovery steps, see [Spool key errors](troubleshooting.md#spool-key-errors).

The encryption protects a copy of the spool folder alone, other local accounts, the idle key in a
process dump or the pagefile, and a disk or backup of a BitLocker-protected volume. It does not
protect against:

- An offline copy of a system volume without BitLocker. Offline DPAPI tools recover the RSA key,
  and with it every segment.
- SYSTEM, Administrators, and code running in the service process.
- A full kernel memory capture (a memory dump, `hiberfil.sys`, or a VM saved state), or a dump
  taken during a seal or open.

See [SECURITY.md](../../SECURITY.md) for the full threat analysis.

Startup deletes every segment the service cannot read and counts its bytes in
`spool_dropped_bytes`: a plaintext segment from a version before encryption, a segment sealed with
another key, a file that is not a segment, and every segment when the service made a new key at
this start (which also resets `cursor.bin`). One warning, `dnstap spool deleted segments it cannot
read at startup` (Event ID 14), gives the counts. A record that does not authenticate is counted as
unreadable data and is never replayed.

### Routing

For each block of frames, the router thread chooses one destination:

- **Link FIFO:** The receiver is connected, and the spool holds no unsent frames. The FIFO holds
  256 blocks (about 16 MiB) in memory, and the link thread takes blocks from it.
- **Spool:** The receiver is down, the spool holds unsent frames, or the link FIFO is full. A
  full FIFO moves all its blocks to the spool, oldest first, then the new block.
- **Dropped:** No spool is in use, and the receiver is down. The frames count in
  `disconnected_dropped`.

If the spool refuses a block, the frames count in `spool_dropped`. The link FIFO absorbs a slow
receiver, not an outage. The send queue between the parser and the router absorbs short router
delays. When it is full, the parser drops frames, counted in `queue_dropped`. With a spool, that
means a slow disk held up the router. With no spool, a full link FIFO stops the router, so the
send queue fills.

### Order

While the spool holds frames, every new block goes to the spool, even when the receiver is
connected, so frames reach the receiver in capture order. After a reconnect, the router replays the
oldest frames in batches of up to 64 KiB, up to 127 batches at a time, and reads at most 1 MiB
from the spool per pass. A segment stays on disk until the connection has taken all of it. Frames
from an earlier run replay first, and each run writes new segment files.

`cursor.bin` records the position after the last frame the connection took, saved once per router
pass. The position is a byte offset in the segment's decrypted frames.

### Size limits and eviction

- `--spool-max-mib` caps segment bytes. The bytes are file bytes, so they include the segment
  header and 28 bytes for each record. When a new block would exceed the cap, the oldest segments
  are removed until it fits.
- `--spool-min-free-mib` keeps free space on the volume. When a new segment starts below the floor
  plus one segment, the spool removes its oldest segments, and stops when a removal does not raise
  free space. If only the newest segment is left, it refuses blocks for 10 seconds, counts them
  in `spool_dropped`, and checks again. Warnings give the free and needed MiB for the first four
  pauses, then the 61st, 121st, and so on.
- A segment holds up to 64 MiB, or one eighth of the cap when that is smaller.
- Segments that hold data the service has read and the connection has not yet taken stay whole
  until the connection takes the data or a failure gives it back, and the cap counts them. That
  data is at most about 9 MiB. With two segments the total is 24.7 MiB at the smallest cap and
  about 137 MiB with 64 MiB segments (caps of 512 MiB and above).
- Each removal counts in `spool_dropped` and `spool_dropped_bytes`, and the eviction warning gives
  the frames and bytes lost. A leftover segment from an earlier run adds only bytes.

Size the cap from the outage you must cover. [Spool sizing](tuning.md#spool-sizing) has the
figures.

<a id="delivery-is-at-most-once"></a>

### Delivery after a failure, a stop, and a crash

- A running service sends a frame again only when the socket did not fully accept it (see **Loss**
  under [Receiver connection](#receiver-connection)).
- Bytes that the socket accepted and the kernel had not yet sent are lost without a count if the
  connection then fails. The kernel send buffer can grow to several MiB on a fast link.
- A normal stop repeats nothing: the next run starts after the last frame the connection took.
- A crash or kill sends frames again. The next run starts at the `cursor.bin` position, so it
  repeats the replay data that the service had read and the connection had not taken (up to 127
  batches, 8 MiB at 103 bytes a frame) and the frames taken since the last save. A receiver can see
  these frames twice. Delivery across a crash is at least once.
- The spool relies on the OS to write data to disk. It covers a receiver outage; a power loss can
  lose recent frames.

### Shutdown

At shutdown, the router routes the queued blocks, saves the `cursor.bin` position, and seals the
active segment. The backlog stays on disk for the next run. For the receiver write cutoff, see
[Startup and shutdown deadlines](#startup-and-shutdown-deadlines).

## Startup and shutdown deadlines

- **Startup:** The deadline is 10 seconds, plus 1 second for each 16,384 `--capacity` slots,
  rounded up. It covers identity checks, argument validation, the receiver connection, spool open,
  pool allocation, and capture start (which has its own inner 10-second deadline). Examples:

  | `--capacity` | Startup deadline |
  | --- | --- |
  | 24,576 | 12 seconds |
  | 131,072 (default) | 18 seconds |
  | 524,288 | 42 seconds |

- **SCM progress:** The first `START_PENDING` report has a 10-second wait hint. After the arguments
  are read, the service reports the full startup deadline and advances the checkpoint after every
  16,384 allocated pool slots. `sc.exe query EtwDnstap` shows `CHECKPOINT` and `WAIT_HINT`. The
  `STOP_PENDING` wait hint is the shutdown deadline.
- **Shutdown:** The deadline is 10 seconds from the stop request to the final report. After the
  session stops, the service waits up to 5 seconds for `ProcessTrace` to return, then closes the
  trace handle. The router seals the spool segment while the link thread runs STOP/FINISH. The
  service stops writing to the receiver 1 second after shutdown starts and gives the unsent blocks
  to the spool. The sink ends in about 3.2 seconds at most, or about 1.1 seconds with a stalled
  receiver.
- **Expiry:** The process writes Event ID 5 to the Application log and aborts, so SCM recovery
  applies. See [Watchdog aborts](troubleshooting.md#watchdog-aborts-or-stop-hangs).

## Session reclaim

SCM starts one process, and the identity check refuses any other.

At startup, the service stops and replaces a leaked session with the same name, the same session
GUID, and this program's shape: real-time only, with no file, circular, append, new-file, system,
buffering, or private logger mode. Buffer size, buffer counts, flush timer, and clock type are not
part of the shape. A different `--etw-buffer-kib` does not block reclaim: the service logs `stale
ETW session used a different buffer size; reclaiming anyway` and continues. A session with the
same name and a different GUID is left alone, and startup fails.

The GUID comes from the exact spelling of `--session-name`, but ETW ignores case. Use the same
spelling in the ETW grant, or the start fails with access denied.

## Next steps

- [Tune capture for load](tuning.md)
- [Log events and counters](monitoring.md)
- [Install and provision](install.md)
- [Troubleshooting](troubleshooting.md)
- [Measurement terms](../design/measurement-terms.md)
