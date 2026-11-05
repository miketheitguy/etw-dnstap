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
| `--receiver-stall-seconds SECONDS` | `15` | 2 to 300 | How long the receiver may accept no bytes, while the service has data to send, before the service ends the session and reconnects. A receiver that is slow, or paused for less than this, keeps its session: the service holds the data in memory, then spools it. An idle connection never stalls. With `--no-spool`, a longer stall means more `queue_dropped`. See [Receiver connection](#receiver-connection). |
| `--capacity N` | `131072` | 1,024 to 524,288 events | Event channel size. A full channel drops the newest event and counts it in `ring_dropped` and `intake_dropped`. |
| `--jumbo-capacity N` | `64` | 0 to 256 | Pool slots for padded payloads over 2 KiB. `0` disables jumbo capture. |
| `--duration SECONDS` | none | 1 to 604,800 | Stop after this many seconds. Otherwise capture runs until SCM stops the service or a failure occurs. |
| `--session-name NAME` | `EtwDnstap` | 1 to 128 ASCII letters, digits, underscore, dot, or hyphen. Case-sensitive. The first character must not be a hyphen. | Real-time ETW session name. The ETW grant must use the same spelling, or the start fails with access denied. A new name needs a grant run and a restart. See [Session reclaim](#session-reclaim) and [install.md](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source). |
| `--identity TEXT` | `etw-dnstap` | At most 1,024 UTF-8 bytes | dnstap producer identity. Empty text omits the field. |
| `--etw-buffer-kib KIB` | `32` | 4 to 128, a multiple of 4 | ETW session buffer size. See [Defaults](tuning.md#defaults). |
| `--etw-min-buffers N` | `12` | 0 to 4,096, at most `--etw-max-buffers` | Minimum buffer count, reserved at session start. ETW raises it to at least two per logical processor. `0` lets ETW choose. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). |
| `--etw-max-buffers N` | `64` | 4 to 4,096 | Maximum buffer count, allocated on demand. See [Defaults](tuning.md#defaults) and [Recommended settings for higher rates](tuning.md#recommended-settings-for-higher-rates). |
| `--etw-flush-seconds SECONDS` | `1` | 1 to 60 | Delivery interval, which bounds latency. Longer values use less CPU and make each batch larger. See [Defaults](tuning.md#defaults). |
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
| 8,192 (earlier default) | 64 | 25.22 MiB |
| 24,576 (previous default) | 64 | 59.59 MiB |
| 65,536 (previous default) | 64 | 145.53 MiB |
| 131,072 (default) | 64 | 283.03 MiB |
| 262,144 | 64 | 558.03 MiB |
| 262,144 | 256 | 582.11 MiB |
| 524,288 (maximum) | 64 | 1,108.03 MiB |
| 524,288 (maximum) | 256 (maximum) | 1,132.11 MiB |

The block pool adds about 68.5 MiB at startup (516 blocks of 136 KiB): the send queue of 256
blocks, the 256-block link queue that absorbs a slow receiver before the spool starts, one block
for the front list that waits for the next session after a failure, and one block each in the
hands of the parser, the router, and the link thread. The pool is not in the totals.

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
  from TCP open, covering the TLS handshake, READY, ACCEPT, and START. STOP and FINISH share one
  2-second deadline. Partial socket calls count, so a peer cannot extend a deadline by trickling
  bytes. Certificate verification is not interrupted, but its time counts. A verifier that runs past the remaining time fails the handshake with a timeout. A data write has no
  deadline: see **Stall** under [Receiver connection](#receiver-connection).
- **Socket send buffer:** The service does not set `SO_SNDBUF`. Windows tunes the send buffer
  by itself (dynamic send buffering), and setting the option would turn that off.
- **Flush:** An idle parser sends its partial block once the last send is 100 ms old. A busy parser
  sends at least once a second. The link thread writes each block as soon as it takes it, so no
  further flush follows. The parser reads the clock every 64 events, so the busy bound is
  approximate.
- **Failure:** The link thread abandons the session and hands the unsent live frames back to the
  router (see **Loss** above). No STOP follows a partial frame. Capture continues, and the sink
  reconnects.
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
- **Stall:** The link thread writes data without blocking. When the socket takes no more, it waits
  for the socket to become writable and checks the clock. A session ends only if the socket
  accepted no byte for `--receiver-stall-seconds` while the link had data to write. A write that
  accepts any byte is progress. With TLS only bytes written to the socket count. An idle
  connection never stalls. A receiver that is slow or paused for less than the stall time loses
  nothing and costs no reconnect: the service holds up to about 16 MiB in the link queue, and then
  the spool takes the data.
- **Loss:** A failed write or a stall ends the session. The sink logs a warning and reconnects.
  The link thread gives the live frames that the socket did not fully accept back to the router,
  which puts them at the head of the spool (or at the front of the next session when the spool
  already holds a backlog), and the service sends them again. A frame cut in the middle is sent
  again in full; the receiver saw only a truncated copy, which Frame Streams readers discard.
  With TLS, the frames of the one 16 KiB piece whose delivery is unknown go back too: the
  socket took only part of its record, which the receiver cannot decrypt. Replay frames that
  the connection did not fully accept stay in the spool and are read again. With a spool,
  nothing is counted in `disconnected_dropped` at a failure.
- **Retry:** Each attempt runs on a helper thread, so a blackholed receiver never blocks the parser.
  Each wait is uniform between half of a base delay and the base. The base starts at 250 ms,
  doubles, and caps at 30 seconds. A successful reconnect resets it. Every new session repeats
  READY, ACCEPT, and START.
- **No resend:** A frame that the socket fully accepted is not sent again by a running
  service. Frames that arrive while the receiver is down go to the spool, or count in
  `disconnected_dropped` without one.
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
- **Shutdown while connected:** If the link finished its data, the sink sends STOP and waits for
  FINISH within the 2-second deadline. A STOP or FINISH that does not complete (a stalled
  receiver, a reset, a missing or invalid FINISH) does not fail the stop: the service writes
  nothing about it and stops with success. Every data frame was already accepted by the socket,
  handed back to the spool, or counted, so the exchange only says goodbye. If the 1-second link
  budget ends while the link is writing a block (the receiver is not keeping up), the sink closes
  the connection with no STOP, and the unsent frames stay in the spool.

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
query names. The service encrypts every segment file with a key that only the service, SYSTEM,
and Administrators can use. The folder ACL still keeps other accounts out. The encryption does
not protect an offline copy of a system volume that BitLocker does not protect. See
[Encryption](#encryption) and [SECURITY.md](../../SECURITY.md).

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

If a check fails, the spool key cannot be loaded, or the spool cannot open, the service logs one
warning and runs without a spool. See [troubleshooting.md](troubleshooting.md#spool-warnings) for
the warning text. Files survive restarts and reboots. The check runs only at startup, so the
install ACL must keep other accounts out afterward.

### Encryption

The spool writes no plaintext to disk. Each segment file starts with a 32-byte header. Each block
of frames that the router appends becomes one record, encrypted and authenticated with AES-128-GCM.
A record adds 28 bytes. A sealed segment ends with a 28-byte end record. Frames in memory, in the
send queue, and on the wire are not encrypted; the receiver gets the same dnstap frames as before.

The key:

- The installer creates a non-exportable RSA-3072 machine key, `EtwDnstap Spool Key`, in the
  Microsoft Software Key Storage Provider. Only the service, SYSTEM, and Administrators can use it.
  See [Install](install.md).
- At its first start, the service makes a random AES key, wraps it with the RSA key, and stores the
  wrapped key in the registry value
  `HKLM\SOFTWARE\ETW-DNSTapService\Protect\WrappedKey`. This key is outside the service key, so a
  major upgrade keeps it. Uninstall deletes it.
- At every start, the service unwraps the key once. It keeps the AES key schedule in one locked
  memory page that is never written to the pagefile and is left out of WER crash dumps, and it
  zeroes the page at stop.
- If the key cannot be loaded, the service runs without a spool and logs one warning that names
  the cause. It never writes the spool in plaintext. The service waits at most 3 seconds for the
  key store, so a key problem never holds the service start or stops the service. See
  [Spool key errors](troubleshooting.md#spool-key-errors).

What the encryption protects against:

- A copy or backup of the spool folder alone.
- Other local accounts.
- A disk, image, or backup of a volume that BitLocker protects, with a TPM. The copied-disk case
  needs BitLocker.

What it does not protect against:

- An offline copy of a system volume without BitLocker. The key file of the machine key is
  protected by machine DPAPI. The DPAPI secret is in the SECURITY and SYSTEM registry hives on
  the same volume, and `WrappedKey` is in the SOFTWARE hive. Standard offline DPAPI tools recover
  the RSA key, and with it every segment.
- SYSTEM, Administrators, and code running as the service.
- A VM saved state or memory checkpoint, the hibernation file (`hiberfil.sys`), and a complete
  kernel memory dump. They contain the locked key page. The WER exclusion covers WER crash dumps
  only.

Startup deletes every segment the service cannot read, and counts its bytes in
`spool_dropped_bytes`:

- A plaintext segment from a version before encryption.
- A segment sealed with another key, or a file that is not a segment.
- Every segment, when the service made a new key at this start. The service also resets
  `cursor.bin` then.

One warning, `dnstap spool deleted segments it cannot read at startup` (Event ID 14), gives the
counts. A record that does not authenticate, because a byte changed or the file was cut short, is
counted as unreadable data and is never replayed.

### Routing

For each block of frames, the router thread chooses one destination:

- **Link FIFO:** The receiver is connected, and the spool holds no unsent frames. The link
  FIFO holds 256 blocks (about 16 MiB) in memory, and the link thread takes blocks from it.
- **Spool:** The receiver is down, the spool holds unsent frames, or the link FIFO is full.
  When the FIFO is full, the router moves all of its blocks to the spool, oldest first, and
  then the new block.
- **Dropped:** No spool is in use, and the receiver is down. The frames count in
  `disconnected_dropped`.

If the spool refuses a block, the frames count in `spool_dropped`. The link FIFO absorbs a
slow receiver, not an outage. The send queue between the parser and the router absorbs short
delays of the router. When it is full, the parser drops frames, counted in `queue_dropped`.
With a spool in use, `queue_dropped` means a slow disk held up the router. With no spool, a
full link FIFO stops the router, so the send queue fills and the parser counts `queue_dropped`.

### Order

While the spool holds frames, every new block goes to the spool, even when the receiver is
connected. Frames therefore reach the receiver in capture order. After a reconnect, the router
replays the oldest frames in batches of up to 64 KiB, up to 127 batches at a time, so the link
thread can send about 0.2 seconds of replay at 41 MB/s without waiting for the router. A batch
is larger only when its first frame is. Each router pass first routes the blocks waiting from
the parser, then reads replay, and it reads at most 1 MiB from the spool in one pass. A segment
stays on disk until the connection has taken all of it. Frames from an earlier run replay
first. The file `cursor.bin` records the position after the last frame the connection took,
saved once per router pass. The position is a byte offset in the segment's decrypted frames,
not in the file. Each run writes new segment files and never appends to an earlier run's.

### Size limits and eviction

- `--spool-max-mib` (default 4096) caps segment bytes. The bytes are file bytes, so they include
  the segment header and 28 bytes for each record. When a new block would exceed the cap, the
  oldest segments are removed until it fits.
- `--spool-min-free-mib` (default 2048) keeps free space on the volume. When a new segment starts
  below the floor plus one segment, the spool removes its oldest segments. It stops when a removal
  does not raise free space. If only the newest segment is left, it refuses blocks for 10 seconds,
  counts them in `spool_dropped`, and checks again. Warnings give the free and needed MiB for the
  first four pauses, then the 61st, 121st, and so on. Refused blocks log nothing during a pause.
- A segment holds up to 64 MiB, or one eighth of the cap when that is smaller.
- The oldest data goes first, except the segments that hold any data the service has read and
  the connection has not yet taken: those stay whole until the connection takes the data or a
  failure gives it back. The cap counts them. That data is at most about 9 MiB, but the whole
  segments that hold it can be larger: that data plus two segments, which is 24.7 MiB at the
  smallest cap (64 MiB, with 8 MiB segments) and about 137 MiB with 64 MiB segments (caps of
  512 MiB and above). Each removal counts in `spool_dropped` and `spool_dropped_bytes`. A
  leftover segment from an earlier run adds only bytes, because its frame count is unknown.
  Eviction warnings give the frames and bytes lost.

Size the cap from the outage you must cover. [Spool sizing](tuning.md#spool-sizing) has the
figures.

<a id="delivery-is-at-most-once"></a>

### Delivery after a failure, a stop, and a crash

- A running service does not send a frame again once the socket has fully accepted it. A frame
  that the socket did not fully accept goes back, live frames to the router and replay frames to
  the spool, and is sent in full on the next session. The receiver saw only a truncated copy of a
  cut frame.
- A failed connection with a spool counts no frame. Bytes that the socket accepted and the kernel
  had not yet sent are lost without a count if the connection then fails. Windows sizes the kernel
  send buffer, and it can grow to several MiB on a fast link.
- A normal stop repeats nothing: the next run starts after the last frame the connection took.
- A crash or kill (the process ends without a stop) sends frames again. `cursor.bin` holds the
  position after the last frame the connection took, saved once per router pass. After a crash the
  next run starts there, so it sends again the replay data that the service had read and the
  connection had not taken (up to 127 batches, 8 MiB at 103 bytes a frame), and the frames taken
  since the last save. A receiver can see these frames twice. Delivery across a crash is at least
  once.
- The spool does not flush data to disk. It protects against a receiver outage, not a power loss.

### Shutdown

At shutdown, the router routes the queued blocks, keeps on disk the replay data that the
connection did not take, saves the position after the last frame the connection took in `cursor.bin`,
and seals the active segment. The backlog stays on disk for the next run, and the service does
not send it before exit. For the receiver write cutoff, see
[Startup and shutdown deadlines](#startup-and-shutdown-deadlines).

## Startup and shutdown deadlines

- **Startup:** The deadline is 10 seconds, plus 1 second per 16,384 `--capacity` slots, rounded up.
  The default 131,072 gives 18 seconds, 262,144 gives 26 seconds, and 524,288 gives 42 seconds. The previous default 65,536
  gave 14 seconds, the one before it (24,576) gave 12 seconds, and 8,192 gave 11 seconds. The deadline covers identity checks, argument validation, receiver connection,
  spool open, pool allocation, and capture start. Capture start has its own inner 10-second
  deadline.
- **SCM progress:** The first `START_PENDING` report has a 10-second wait hint. After the arguments
  are read, the service reports the full startup deadline, and it advances the checkpoint after
  every 16,384 allocated pool slots. `sc.exe query EtwDnstap` shows `CHECKPOINT` and `WAIT_HINT`.
  The `STOP_PENDING` wait hint is the shutdown deadline.
- **Shutdown:** The deadline is 10 seconds from the stop request to the final report. After the
  session stops, the service waits up to 5 seconds for `ProcessTrace` to return. Only then does it
  close the trace handle to force the worker out. The router routes queued blocks and seals the
  spool segment, and the link thread runs STOP/FINISH at the same time. The service stops writing
  to the receiver 1 second after shutdown starts and gives the unsent blocks to the spool. The
  link thread stops within 50 ms, and STOP/FINISH can take 2 seconds more, so the sink ends in
  about 3.2 seconds at most. With a stalled receiver there is no STOP, and it ends in about 1.1
  seconds. See [Spool](#spool).
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
