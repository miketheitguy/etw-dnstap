# Configuration reference

> **Status:** Current · **Describes:** the code at commit `58fde72` · **Reviewed:** 2026-11-03

Set every option in the service binary path. The service ignores arguments to `sc.exe start`.
An MSI install builds the path from MSI properties. See
[Install with the MSI](install.md#install-with-the-msi).

Related: [install.md](install.md), [troubleshooting.md](troubleshooting.md),
[tuning.md](tuning.md), [monitoring.md](monitoring.md).

## Options

Defaults and ranges come from `CaptureArgs` in `src/pipeline/config.rs`. An out-of-range value
fails at startup. Repeating an option is an error, so the last value does not win.

| Option | Default | Range or format | Meaning |
| --- | --- | --- | --- |
| `--receiver IP:PORT` | none (required) | Numeric unicast IP and nonzero port. IPv6 as `[addr]:port`. | Frame Streams receiver. TLS unless `--plaintext`. See [Receiver connection](#receiver-connection). |
| `--plaintext` | off | Flag | Disables TLS and peer authentication. Use only on a trusted network. See [TLS](#tls). |
| `--tls-server-name NAME` | the receiver IP | A DNS name or an IP literal. Cannot be combined with `--plaintext`. | Name the receiver certificate must match. See [TLS](#tls). |
| `--capacity N` | `24576` | 1,024 to 262,144 events | Event channel size. A full channel drops the newest event and counts it in `ring_dropped` and `intake_dropped`. |
| `--jumbo-capacity N` | `64` | 0 to 256 | Pool slots for padded payloads over 2 KiB. `0` disables jumbo capture. |
| `--duration SECONDS` | none | 1 to 604,800 | Stop after this many seconds. Otherwise capture runs until SCM stops the service or a failure occurs. |
| `--session-name NAME` | `EtwDnstap` | 1 to 128 ASCII letters, digits, underscore, dot, or hyphen. Case-sensitive. The first character must not be a hyphen. | Real-time ETW session name. The ETW grant must use the same spelling, or the start fails with access denied. A new name needs a grant run and a restart. See [Session reclaim](#session-reclaim) and [install.md](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source). |
| `--identity TEXT` | `etw-dnstap` | At most 1,024 UTF-8 bytes | dnstap producer identity. Empty text omits the field. |
| `--etw-buffer-kib KIB` | `16` | 4 to 128, a multiple of 4 | ETW session buffer size. |
| `--etw-min-buffers N` | `0` | 0 to 4,096, at most `--etw-max-buffers` | Minimum buffer count, reserved at session start. `0` lets ETW choose. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). |
| `--etw-max-buffers N` | `64` | 4 to 4,096 | Maximum buffer count, allocated on demand. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). |
| `--etw-flush-seconds SECONDS` | `1` | 1 to 60 | Delivery interval, which bounds latency. Longer values use less CPU and make each batch larger. See [Low-rate defaults](tuning.md#low-rate-defaults). |
| `--spool-dir PATH` | `C:\ProgramData\EtwDnstap\spool` | An absolute path that starts with a drive letter, with no `.` or `..` part. Cannot be combined with `--no-spool`. | Folder for frames that wait for the receiver. The service never creates it. A failed trust check runs without a spool. A malformed value fails parsing. See [Requirements](#requirements). |
| `--no-spool` | off | Flag. Cannot be combined with `--spool-dir`, `--spool-max-mib`, or `--spool-min-free-mib`. | Runs without a spool. Lost frames count in `disconnected_dropped` or `queue_dropped`. |
| `--spool-max-mib N` | `4096` | 64 to 1,048,576 MiB. Cannot be combined with `--no-spool`. | Cap on spool segment bytes. The oldest segments go first. See [Size limits and eviction](#size-limits-and-eviction). |
| `--spool-min-free-mib N` | `2048` | 0 to 1,048,576 MiB. Cannot be combined with `--no-spool`. | Free space to keep on the spool volume. The oldest segments go first below it. See [Size limits and eviction](#size-limits-and-eviction). |
| `--stats-interval-seconds N` | `60` | 0 to 86,400 | Seconds between `capture stats` and `output stats` events. `0` disables them. Final stop events are always written. See [Log events and counters](monitoring.md). |

The binary has no help or version output. Starting it from a console exits with code 1 and
prints nothing. For the file version, see [build.md](build.md#build-the-executable). For the
running version, see the `version` field of `capture started` in
[monitoring.md](monitoring.md#periodic-and-final-reports).

### Removed options

The service rejects `--output`/`-o`, `--max-output-bytes`, `--metrics`, `--help`, `-h`,
`--version`, `-V`, and `--etw-flush-ms` at parsing. This build removes `--etw-flush-ms`. A binary
path with a rejected option does not start. The service writes no capture or metrics files, except
spool segments.

### Diagnostic flags (`diag-timing` builds only)

These options exist only in a binary built with `--features diag-timing`. Do not use them in
production.

| Option | Range | Effect |
| --- | --- | --- |
| `--diag-timer-resolution-ms MS` | 1 to 15 | Calls `timeBeginPeriod` for the whole run and withdraws it on exit. |
| `--diag-thread-priority LEVEL` | `above-normal` or `highest` | Raises the priority of the `dnstap-parser` and `etw-consumer` threads. If the call fails, the service logs a warning and keeps the default priority. |

The feature also times parser idle parks, socket writes, and flushes, and adds the results to
the stop report.

## Storage

Counted storage covers payload, slots, free list, and channel. It excludes the allocator, native
ETW buffers, TDH caches, protobuf data, and output buffers. It is not a measured memory limit.
See [pooled-intake.md](../design/pooled-intake.md).

| `--capacity` | Jumbo slots | Counted storage |
| --- | --- | --- |
| 8,192 (previous default) | 64 | 25.22 MiB |
| 24,576 (default) | 64 | 59.59 MiB |
| 65,536 | 64 | 145.53 MiB |
| 131,072 | 64 | 283.03 MiB |
| 262,144 | 64 | 558.03 MiB |
| 262,144 | 256 (maximum) | 582.11 MiB |

The send queue adds about 17.3 MiB at startup (130 blocks of 136 KiB). The replay buffer adds
about 0.3 MiB. Neither is in the totals.

## Subscription

The service enables `Microsoft-Windows-DNSServer` at level `0xFF` in its own real-time session.
The level is a maximum, so every level passes. The Analytical log is not enabled.

At startup, the service resolves the `QUERY_RECEIVED`, `RESPONSE_SUCCESS`, and
`RESPONSE_FAILURE` keyword bits by name and sets `MatchAnyKeyword`. On the lab VM they are
`0x1`, `0x2`, and `0x4`, so the mask is `0x7`.

If the lookup fails, or if `QUERY_RECEIVED` or `RESPONSE_SUCCESS` is missing or malformed, the
service logs one warning. It then uses `0x8000000000000000`, the Analytical channel bit. The same
warning and mask apply when two names share a bit, which means the metadata cannot be trusted. If
only `RESPONSE_FAILURE` is missing or malformed, the mask becomes `QUERY_RECEIVED |
RESPONSE_SUCCESS`, and failed responses are not captured. The warning starts with "RESPONSE_FAILURE
keyword is not available" and ends with "failed responses are not captured".

`capture started` shows the masks, `keyword_fallback`, and `failed_responses_captured`. The last
field is `false` only in the partial case. In the full-channel fallback, event 258 passes the
mask, so the field is `true`.

The service accepts only these events:

| Event ID | Meaning | dnstap message type |
| --- | --- | --- |
| 256 | Query received | `CLIENT_QUERY` |
| 257 | Response success | `CLIENT_RESPONSE` |
| 258 | Response failure, for example SERVFAIL | `CLIENT_RESPONSE` |

SERVFAIL and policy REFUSED arrive as 258 (Reason `System` and `Policy`). A 258 whose packet has
the QR bit clear counts in `rejected`. NXDOMAIN arrives as 257. Events 259, 260, and 261 are
excluded and count nowhere. An IGNORE policy produces 259 (`IGNORED_QUERY`) with no packet, but
its query arrives as 256. See the [README message mapping](../../README.md#what-is-captured).

- `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0` is set, so keyword 0 events never match. ETW filters
  IDs 256 to 258 before the session buffers. That cuts buffer use and callbacks, not the DNS
  Server's cost to generate events.
- The keywords also enable EDNS variants 284, 285, and 287, `DNSSEC_VALIDATION_FAILURE` (288),
  and DNS over HTTPS (597 to 599). They cost `dns.exe` CPU but never reach the service. The
  callback repeats the event-ID check as defense in depth.

### DNS process filter

The subscription also filters to the running `DNS` service by process ID
(`EVENT_FILTER_TYPE_PID`). A mismatch counts in `foreign_pid_rejected`, not `received`.

- Startup fails if the `DNS` service is not running.
- Every 5 seconds the service rereads the process ID. If it changes, or the DNS service stops,
  EtwDnstap stops with a service failure. SCM restarts it under the [recovery
  policy](install.md#5-configure-scm-recovery). Capture does not resume for a restarted DNS Server
  until EtwDnstap restarts. Expect a gap of at least the detection interval plus the SCM restart
  delay.
- The project has not verified on a live host that the ETW-side filter keeps other processes'
  records out. A nonzero `foreign_pid_rejected` shows that they reached the callback.

## Message and transport limits

Event 256 sets the envelope type `CLIENT_QUERY`. Events 257 and 258 set `CLIENT_RESPONSE`. A field
is set only when the provider reports it.

| Field | Value |
| --- | --- |
| `socket_protocol` | `UDP` or `TCP`, from the event |
| `socket_family` | `INET` or `INET6`, from the client or local address. The encoder rejects a record whose two addresses differ in family. |
| `query_address`, `query_port` | The client's address and port |
| `response_address` | The DNS Server's local interface address. `response_port` is never set. A wildcard address and port 0 count as not reported. |
| `query_time_sec`, `query_time_nsec` | Set on a query |
| `response_time_sec`, `response_time_nsec` | Set on a response. A response has no query time. |
| `query_message` or `response_message` | The DNS packet. Event 256 sets `query_message`. Events 257 and 258 set `response_message`. |
| `identity` | The `--identity` text. Absent when empty. |

Times are ETW event timestamps in UTC, at 100 ns resolution. The encoder rejects a timestamp before
the Unix epoch. The service never sets `version` or `extra`. The parser counts a rejected record
in `rejected`.

- **Encoding:** Content type `protobuf:dnstap.Dnstap`. Control payloads up to 512 bytes,
  content-type fields up to 256 bytes, and encoded messages up to 70 KiB.
- **Deadlines:** The TCP connect has a 2-second timeout. The handshake has one 2-second deadline
  from TCP open, covering the TLS handshake, READY, ACCEPT, and START. Each write and flush has its
  own 2-second deadline, and the final flush, STOP, and FINISH share one. Partial socket calls
  count, so a peer cannot extend a deadline by trickling bytes. Certificate verification is not interrupted, but its time counts. A verifier that runs past the remaining time fails the handshake with a timeout.
- **Socket send buffer:** The connection requests a 1 MiB `SO_SNDBUF`, or the default buffer if
  that request fails.
- **Flush:** An idle parser sends its partial block once the last send is 100 ms old. A busy parser
  sends at least once a second, and the sender flushes at least every 100 ms while blocks arrive.
  The parser reads the clock every 64 events, so the busy bound is approximate.
- **Failure:** The sink abandons the session and counts the frames in flight. No STOP follows a
  partial frame. Capture continues, and the sink reconnects.
- **DNS envelope:** The DNS message is 12 to 65,535 bytes, and its QR bit must match the event
  direction. Packet bytes are kept verbatim. Names and resource records are not checked. No TCP
  length prefix is added or stripped. A malformed packet is rejected, and the service never
  rebuilds a packet from a query name.
- **Standards:** The layout follows the
  [dnstap protobuf schema](https://github.com/dnstap/dnstap.pb/blob/main/dnstap.proto) (CC0) and
  the [fstrm control reference](https://github.com/farsightsec/fstrm/blob/master/fstrm/control.h).
  The Prost subset keeps upstream field numbers, enum values, optional-field presence, and
  `fixed32` nanosecond encoding, and `protoc` is not required. This program writes dnstap, not the
  C-DNS format in [RFC 8618](https://www.rfc-editor.org/info/rfc8618/). Event names and payload
  fields follow
  [Microsoft's DNS logging documentation](https://learn.microsoft.com/en-us/windows-server/networking/dns/dns-logging-and-diagnostics).

## Receiver connection

The sink keeps one Frame Streams session to `--receiver`. Network failures are recoverable.
Protocol failures at startup are not.

- **Startup:** The first connection is attempted once, synchronously. A network failure (refused,
  timed out, or reset) logs a warning, and the service starts disconnected and retries. A protocol
  failure stops the service, including a receiver that rejects the `protobuf:dnstap.Dnstap`
  content type or sends a malformed control frame.
- **Loss:** A failed write, flush, or deadline counts the frames in flight in
  `disconnected_dropped`. The sink logs a warning and reconnects.
- **Retry:** Each attempt runs on a helper thread, so a blackholed receiver never blocks the parser.
  Each wait is uniform between half of a base delay and the base. The base starts at 250 ms,
  doubles, and caps at 30 seconds. A successful reconnect resets it. Every new session repeats
  READY, ACCEPT, and START.
- **No resend:** The sink never sends a frame twice. Frames that arrive while the receiver is down
  go to the spool, or count in `disconnected_dropped` without one.
- **Wrong content type after startup:** The attempt counts as failed, and the service keeps
  retrying. If the receiver was down at startup, the first attempt that reaches it logs one
  Event ID 4 error that names the cause. See [Failure class](#tls).
- **Reconnect log:** Each failure counts in `receiver_reconnect_failures`. The service logs
  `dnstap reconnect failed` for the first four failures, then the 61st, 121st, and so on. It also
  logs the first failure of each `cause` (`connect`, `timeout`, `tls`, or `protocol`) since the last established session. A lost session does not reset that list. `dnstap receiver connection lost` marks each outage. A failed startup
  connection adds its cause, because the startup warning already reported it. A reconnect thread
  that cannot start, or ends without a result, is counted and logged. See
  [Log events and counters](monitoring.md#receiver-schema-and-rate-messages).
- **Shutdown while disconnected:** The sink sends no STOP. The service logs a warning and stops
  with success. Spooled frames stay on disk for the next start.
- **Shutdown while connected:** The sink sends STOP and waits for FINISH within the 2-second
  deadline. A missing or invalid FINISH stops the service with an error.

### TLS

TLS wraps the Frame Streams session unless `--plaintext` is set. With `--plaintext`, the connection
is plaintext TCP with no peer authentication, as in earlier builds. A receiver that answers the
handshake with plaintext fails it as a protocol failure. A receiver that stays silent or closes
during the handshake is a network failure, and the service retries.

- **Versions and library:** TLS 1.2 and TLS 1.3 only, through rustls with the `ring` provider.
  Schannel does not handle the record layer.
- **Verification:** Windows CryptoAPI checks the certificate through `rustls-platform-verifier`,
  against the machine store, including roots pushed by Group Policy. The certificate must match
  `--tls-server-name`, or the receiver IP when that flag is unset. SNI is sent only for a DNS name,
  so the default name and an IP literal send none. For an IP address, the certificate should carry
  an `iPAddress` SAN. Whether a `dNSName` SAN or the common name also matches an IP address is not
  verified.
- **Revocation:** The verifier checks only the receiver (end-entity) certificate, on a best-effort
  basis. If the status cannot be retrieved, the connection proceeds. Causes include an unreachable
  Online Certificate Status Protocol (OCSP) responder or certificate revocation list (CRL)
  distribution point. A certificate that names neither is not checked. A certificate that
  CryptoAPI reports as revoked is rejected. Network retrieval has a cumulative 10-second limit. A
  late verifier fails the next socket write with a timeout, which is a network failure and is
  retried. The service does not provide hard-fail revocation.
- **Failure class:** A failed certificate check is a protocol failure. On the first connection it
  stops the service with Event ID 1. On a reconnect it is a counted failed attempt, and the service
  keeps retrying. A TLS alert after the handshake follows the same rule, with `cause` `tls`. A
  TLS 1.3 receiver that requires a client certificate sends
  `received fatal alert: CertificateRequired`. If the receiver was down at startup, the first
  attempt that reaches it and fails the handshake logs one Event ID 4 error. Check that:
  - The Windows trust store trusts the receiver certificate.
  - `--tls-server-name`, or the receiver IP when unset, matches the certificate.
  - The receiver does not require a client certificate.

  Later `dnstap reconnect failed` lines show `cause` `tls`.
- **Client certificate and close_notify:** The service presents no client certificate, so a
  receiver that requires one fails the connection with a `tls` failure. FINISH ends the Frame
  Streams session, then TCP closes. The service sends no TLS close_notify alert.

The handshake runs under the 2-second deadline. See
[Deadlines](#message-and-transport-limits). Receiver certificate requirements are in
[receiver.md](receiver.md#receiver-certificate).

## Spool

The spool keeps frames on disk while the receiver is down or slow. It is on by default. On
reconnect, the service sends stored frames first, oldest first. The spool needs a protected folder
that the service does not create. See [Create the spool folder](install.md#create-the-spool-folder).

**The spool stores DNS data.** Each frame is a complete dnstap message with client addresses and
query names. The files are not encrypted, so the folder ACL is the only protection. See
[SECURITY.md](../../SECURITY.md).

### Requirements

The service checks the folder once, at startup. The default is `C:\ProgramData\EtwDnstap\spool`,
from the `ProgramData` environment value. If that is not a plain local path, the service uses
`C:\ProgramData`. The folder must:

- Exist, and be a folder.
- Not be a reparse point, such as a link or junction. Neither may its parent. The drive root is
  refused.
- Be on a fixed local disk. Removable, network, and RAM disks fail.
- Be owned by SYSTEM or Administrators.
- Grant read and write only to SYSTEM, Administrators, or a service SID. An inherit-only
  `CREATOR OWNER` allow entry is also allowed. Deny entries do not count.

If a check fails, or the spool cannot open, the service logs one warning and runs without a spool.
See [troubleshooting.md](troubleshooting.md#spool-warnings) for the warning text. Files survive
restarts and reboots. The check runs only at startup, so the install ACL must keep other accounts
out afterward.

### Routing

For each block of frames, the sender thread chooses one destination:

- **Spool:** The receiver is down, the spool holds unsent frames, or the send queue holds 96 or
  more of its 128 blocks. A slow receiver spills to disk before the queue fills.
- **Receiver:** The receiver is connected, and the spool is empty or not in use.
- **Dropped:** No spool is in use, and the receiver is down. The frames count in
  `disconnected_dropped`.

If the spool refuses a block, the frames count in `spool_dropped`. The send queue is in memory and
absorbs short sender delays, not an outage. A socket write that stalls longer than the queue lasts
drops frames, counted in `queue_dropped`. With a spool in use, `queue_dropped` means a slow socket
write or slow disk blocked the sender.

### Order

While the spool holds frames, every new block goes to the spool, even when the receiver is
connected. Frames therefore reach the receiver in capture order. After a reconnect, the sender replays the oldest frames in batches of up to 256 KiB. Before each batch, it moves queued blocks to the spool. Frames
from an earlier run replay first. The file `cursor.bin` records the read position. Each run writes
new segment files and never appends to an earlier run's.

### Size limits and eviction

- `--spool-max-mib` (default 4096) caps segment bytes. When a new block would exceed the cap, the
  oldest segments are removed until it fits.
- `--spool-min-free-mib` (default 2048) keeps free space on the volume. When a new segment starts
  below the floor plus one segment, the spool removes its oldest segments. It stops when a removal
  does not raise free space. If only the newest segment is left, it refuses blocks for 10 seconds,
  counts them in `spool_dropped`, and checks again. Warnings give the free and needed MiB for the
  first four pauses, then the 61st, 121st, and so on. Refused blocks log nothing during a pause.
- A segment holds up to 64 MiB, or one eighth of the cap when that is smaller.
- The oldest data goes first. Each removal counts in `spool_dropped` and `spool_dropped_bytes`. A
  leftover segment from an earlier run adds only bytes, because its frame count is unknown.
  Eviction warnings give the frames and bytes lost.

Size the cap from the outage you must cover. [Spool sizing](tuning.md#spool-sizing) has the
figures.

### Delivery is at most once

- A frame is never sent twice. The sender moves the `cursor.bin` read position before it writes a
  batch, so a failure after that point loses the batch.
- Loss per failed connection is bounded:
  - The batch in progress, up to 256 KiB (about 2,500 frames at 103 bytes each). It counts in
    `replayed` and `disconnected_dropped`.
  - The block being written, up to 64 KiB (about 636 frames), and the frames written since the
    last flush. These count in `disconnected_dropped`.

  The lab saw one 64 KiB block lost per outage at 1,000 and 50,000 queries per second. Up to 1 MiB
  in the socket send buffer is also lost, uncounted.
- The spool does not flush data to disk. It protects against a receiver outage, not a power loss.

### Shutdown

At shutdown, the sender routes the queued blocks and seals the active segment. The `cursor.bin`
read position is already current. The backlog stays on disk for the next run, and the service does
not send it before exit. For the receiver write cutoff, see
[Startup and shutdown deadlines](#startup-and-shutdown-deadlines).

## Startup and shutdown deadlines

- **Startup:** The deadline is 10 seconds, plus 1 second per 16,384 `--capacity` slots, rounded up.
  The default 24,576 gives 12 seconds, and 262,144 gives 26 seconds. The previous default 8,192
  gave 11 seconds. The deadline covers identity checks, argument validation, receiver connection,
  spool open, pool allocation, and capture start. Capture start has its own inner 10-second
  deadline.
- **SCM progress:** The first `START_PENDING` report has a 10-second wait hint. After the arguments
  are read, the service reports the full startup deadline, and it advances the checkpoint after
  every 16,384 allocated pool slots. `sc.exe query EtwDnstap` shows `CHECKPOINT` and `WAIT_HINT`.
  The `STOP_PENDING` wait hint is the shutdown deadline.
- **Shutdown:** The deadline is 10 seconds from the stop request to the final report. After the
  session stops, the service waits up to 5 seconds for `ProcessTrace` to return. Only then does it
  close the trace handle to force the worker out. The sender routes queued blocks, seals the spool
  segment, and runs STOP/FINISH. The service stops writing to the receiver 1 second after shutdown
  starts. A write in progress and STOP/FINISH can take about 4 seconds more, so the sink ends in
  about 5 seconds at most. See [Spool](#spool).
- **Expiry:** The process writes Event ID 5 to the Application log and aborts, so SCM recovery
  applies. See [Watchdog aborts](troubleshooting.md#watchdog-aborts-or-stop-hangs).

## Session reclaim

Only one instance runs. SCM starts one process for the service, and the identity check refuses any
process that does not run as it.

At startup, the service stops and replaces a leaked session with the same name, the same session
GUID, and this program's shape. The shape is real-time only, with no file, circular, append,
new-file, system, buffering, or private logger mode, and no log file. Buffer size, buffer counts,
flush timer, and clock type are not part of the shape. A different `--etw-buffer-kib` does not
block reclaim. The service logs `stale ETW session used a different buffer size; reclaiming anyway`
and continues. It never stops another session with that name, including one with a different GUID.
Startup fails instead.

The GUID comes from the exact spelling of `--session-name`, but ETW ignores case.

The service does not reclaim `etw-dnstap-<pid>` sessions from earlier builds. Find them with
`logman query -ets`, and stop each with `logman stop etw-dnstap-<pid> -ets`.

## Next steps

- [Tune capture for load](tuning.md)
- [Log events and counters](monitoring.md)
- [Install and provision](install.md)
- [Troubleshooting](troubleshooting.md)
- [Measurement terms](../design/measurement-terms.md)
