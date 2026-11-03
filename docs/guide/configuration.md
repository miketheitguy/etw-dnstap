# Configuration reference

> **Status:** Current · **Describes:** `tcp-sink-spool` at commit `0569136` · **Reviewed:** 2026-11-03

Set every option in the service binary path. The service ignores arguments that
you give to `sc.exe start`. Install and provisioning steps are in
[install.md](install.md). Symptoms are in
[troubleshooting.md](troubleshooting.md). Values for high query rates are in
[tuning.md](tuning.md). Log events and counters are in
[monitoring.md](monitoring.md).

## Options

Defaults and ranges come from `CaptureArgs` in `src/pipeline/config.rs`. The service
rejects a value outside the range at startup.

| Option | Default | Range or format | Meaning |
| --- | --- | --- | --- |
| `--receiver IP:PORT` | none (required) | Numeric unicast IP and nonzero port. IPv6 as `[addr]:port`. | Frame Streams TCP receiver. The connection is TLS by default and plaintext with `--plaintext`. The option does not accept hostnames. The service starts even when the receiver is down and reconnects. See [Receiver connection](#receiver-connection). |
| `--plaintext` | off | Flag | Disables TLS. The connection is plaintext TCP with no peer authentication. Use it only on a trusted network. See [TLS](#tls). |
| `--tls-server-name NAME` | the receiver IP | A DNS name or an IP literal. Cannot be combined with `--plaintext`. | The name that the receiver certificate must match. It is also sent as SNI. Without it, the certificate must carry the receiver IP as an `iPAddress` SAN. |
| `--capacity N` | `24576` | 1,024 to 262,144 events | Event channel capacity. A full channel drops the newest event and increments `ring_dropped` and `intake_dropped`. The startup budget grows with this value. See [Startup and shutdown deadlines](#startup-and-shutdown-deadlines). |
| `--jumbo-capacity N` | `64` | 0 to 256 | Pool slots for records with a padded payload over 2 KiB. `0` disables jumbo capture. |
| `--duration SECONDS` | none | 1 to 604,800 | Stop after this many seconds. Without it, capture runs until SCM stops the service or a failure occurs. |
| `--session-name NAME` | `EtwDnstap` | 1 to 200 UTF-16 units, no NUL | Name of the real-time ETW session. At startup, the service reclaims a leaked session with the same name, shape, and session GUID. See [Session reclaim](#session-reclaim). The name also sets the session GUID that the ETW grant names. A new name therefore needs a run of the grant script with that name, and a restart of the computer. See [install.md](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source). |
| `--identity TEXT` | `etw-dnstap` | At most 1,024 UTF-8 bytes | dnstap producer identity. Empty text omits the field. |
| `--etw-buffer-kib KIB` | `16` | 4 to 128, a multiple of 4 | ETW session buffer size. |
| `--etw-min-buffers N` | `0` | 0 to 4,096, at most `--etw-max-buffers` | Requested minimum ETW buffer count, reserved when the session starts. `0` lets ETW choose. ETW raises the value to at least two per logical processor. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). |
| `--etw-max-buffers N` | `64` | 4 to 4,096 | Requested maximum ETW buffer count. ETW allocates buffers only on demand, up to this maximum. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). |
| `--etw-flush-seconds SECONDS` | `1` | 1 to 60 | ETW real-time flush interval in seconds. ETW delivers real-time events about once per interval, so this value bounds delivery latency. A longer interval uses less CPU and makes each delivered batch larger. See [Low-rate defaults](tuning.md#low-rate-defaults). |
| `--spool-dir PATH` | `C:\ProgramData\EtwDnstap\spool` | An absolute path that starts with a drive letter, with no `.` or `..` part. Cannot be combined with `--no-spool`. | Folder for frames that wait for the receiver. The service never creates it. It uses the folder only when the folder exists and passes the trust check, which includes a fixed local disk. A failed check logs one warning, and the service runs without a spool. A malformed value fails command-line parsing, and the service does not start. Only the default falls back silently, to `C:\ProgramData`, when the `ProgramData` value is not a plain local path. See [Spool](#spool). |
| `--no-spool` | off | Flag. Cannot be combined with `--spool-dir`, `--spool-max-mib`, or `--spool-min-free-mib`. | Runs without a spool. Frames that arrive while the receiver is down count in `disconnected_dropped`. Frames that the send queue cannot hold count in `queue_dropped`. |
| `--spool-max-mib N` | `4096` | 64 to 1,048,576 MiB. Cannot be combined with `--no-spool`. | Cap on the bytes of spool segment files. At the cap, the spool deletes its oldest segments. See [Size limits and eviction](#size-limits-and-eviction). |
| `--spool-min-free-mib N` | `2048` | 0 to 1,048,576 MiB. Cannot be combined with `--no-spool`. | Free space to keep on the spool volume. Below it, the spool deletes its oldest segments. See [Size limits and eviction](#size-limits-and-eviction). |
| `--stats-interval-seconds N` | `60` | 0 to 86,400 | Seconds between the `capture stats` and `output stats` log events. `0` disables them. The final `capture stopped` and `output stopped` events are always written. See [Log events and counters](monitoring.md). |
| `--help`, `-h` | | | Print help and exit. Capture does not start. |
| `--version`, `-V` | | | Print the package version and exit. Capture does not start. |

### Removed options

The service does not have `--output`/`-o`, `--max-output-bytes`, or `--metrics`.
An argument that uses them fails command-line parsing. The service writes no
capture or metrics files. The one exception is the spool. It keeps frames that
the receiver has not accepted yet, in segment files in the spool folder. See
[Spool](#spool).

This build removes `--etw-flush-ms`. The service no longer asks ETW to flush
the session. A binary path that still has `--etw-flush-ms` fails command-line
parsing, and the service does not start. Remove the option from the binary
path. For high rates, use the settings in [High-rate settings](tuning.md#high-rate-settings).

### Diagnostic flags (`diag-timing` builds only)

These options exist only in a binary built with `--features diag-timing`. Do not
use them in production.

| Option | Range | Effect |
| --- | --- | --- |
| `--diag-timer-resolution-ms MS` | 1 to 15 | Calls `timeBeginPeriod` for the whole run and withdraws it on exit. |
| `--diag-thread-priority LEVEL` | `above-normal` or `highest` | Raises the priority of the `dnstap-parser` and `etw-consumer` threads. If the call fails, the service logs a warning and continues at the default priority. |

The feature also times the parser's idle parks, socket writes, and flushes. It
adds the results to the stop report. It is for locating wait time. It does not change
capture behavior.

## Storage

Counted storage at the defaults (`--capacity 24576` and 64 jumbo slots) is
59.59 MiB. The previous default `--capacity 8192` counts 25.22 MiB. With the
high-rate `--capacity 65536` and the default 64 jumbo slots, it is 145.53 MiB.
With `--capacity 131072` and 64 jumbo slots, it is 283.03 MiB. With
`--capacity 262144` and 64 jumbo slots, it is 558.03 MiB. At the maximum
(262,144 events and 256 jumbo slots) it is 582.11 MiB. These totals are
calculated payload, slot, free-list, and channel sizes. They exclude the
allocator, native ETW buffers, TDH caches, protobuf data, and output buffers.
They are not measured process-memory limits. The send queue adds about 17.3 MiB,
which is 130 blocks of 136 KiB allocated at startup, and a replay buffer of about
0.3 MiB. Neither is in the totals. See
[pooled-intake.md](../design/pooled-intake.md) for the budget.

## Subscription

The service enables the `Microsoft-Windows-DNSServer` provider at level `0xFF`
in its own real-time session. The level is a maximum, so `0xFF` passes every
level. There is no level filter. The service does not enable the Analytical
event log. At startup it resolves the `QUERY_RECEIVED`, `RESPONSE_SUCCESS`, and
`RESPONSE_FAILURE` keyword bits by name and sets `MatchAnyKeyword` to them. On
the lab VM they are `0x1`, `0x2`, and `0x4`, so the mask is `0x7`.

The service logs one warning and uses `0x8000000000000000` instead in these
cases. This is the bit associated with the Analytical channel.

- The keyword lookup fails.
- `QUERY_RECEIVED` or `RESPONSE_SUCCESS` is missing or malformed.
- Any two of the three names share a bit.

A shared bit means the metadata cannot be trusted.

If only `RESPONSE_FAILURE` is missing or malformed, the service narrows the mask
to `QUERY_RECEIVED | RESPONSE_SUCCESS`. The service then does not capture failed
responses. It logs one warning. The warning text starts with "RESPONSE_FAILURE
keyword is not available" and ends with "failed responses are not captured".

The `capture started` line shows the masks in use, whether the fallback applied
(`keyword_fallback`), and whether the service captures failed responses
(`failed_responses_captured`). The last field is `false` only in the partial
case above. In the full-channel fallback, event 258 passes the keyword mask, so
the field is `true`.

The service accepts only event IDs 256 (query received), 257 (response
success), and 258 (response failure). Event 256 produces `CLIENT_QUERY`. Events
257 and 258 produce `CLIENT_RESPONSE`. Event 258 is a failed response, for
example SERVFAIL. The lab observed that its packet was the response sent to the
client for SERVFAIL (Reason `System`) and for a policy REFUSED (Reason `Policy`). The
parser rejects a 258 whose packet is not a response (QR bit clear) and counts it
in `rejected`. NXDOMAIN is a successful response and arrives as event 257. The
service excludes event IDs 259, 260, and 261. Excluded events do not increment
any counter. An IGNORE policy produces event 259 (`IGNORED_QUERY`) with no
packet and no response on the wire, so the service correctly does not capture
it. The service still captures the query itself as 256. See
[README.md](../../README.md#what-is-captured) for the message mapping.

More facts from the subscription code:

- The subscription sets `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0`, so an event
  with keyword 0 does not match.
- ETW also filters event IDs 256, 257, and 258 before events reach the session
  buffers. This reduces buffer use and callback dispatch. It does not reduce
  the DNS Server's cost to generate events that pass the keyword mask.
- The three keywords also enable events that the event-ID filter excludes. They
  include the EDNS variants 284, 285, and 287, `DNSSEC_VALIDATION_FAILURE` (288), and DNS
  over HTTPS (597 to 599). ETW applies the ID filter after the provider builds
  the event, so these events cost `dns.exe` CPU when they occur, but they never
  reach the service.
- The event-ID check stays in the callback as defense in depth.

### DNS process filter

The same provider is also filtered to one process: the running `DNS` service
(the DNS Server role). The subscription adds an `EVENT_FILTER_TYPE_PID` filter
with that process ID, and the callback checks the record's process ID again
before it copies the record. The callback counts a mismatch in
`foreign_pid_rejected`, not in `received`.

- Startup fails if the `DNS` service is not running. Start it first.
- The service reads the process ID again every 5 seconds. If it changes, or the
  service stops, EtwDnstap stops with a service failure. SCM then restarts it under the
  recovery policy in [install.md](install.md#5-configure-scm-recovery), and the
  new instance resolves the new process ID.
- The service does not capture records from a restarted DNS Server until
  EtwDnstap restarts. Expect a gap of at least the detection interval plus the SCM
  restart delay.
- The ETW-side filter should keep other processes' records out of the callback.
  The project has not verified this on a live host. A nonzero
  `foreign_pid_rejected` shows that records from another process reached the
  callback.

## Message and transport limits

- **Frame Streams content type:** `protobuf:dnstap.Dnstap`, with protobuf data
  frames. The service accepts control payloads up to 512 bytes and
  content-type fields up to 256 bytes.
- **Deadlines:** the TCP connection has a 2-second timeout. The handshake has
  one 2-second deadline. It starts after the TCP connection opens. It covers
  the TLS handshake, READY, ACCEPT, and START. Each write call and each flush
  call has its own 2-second deadline. The final flush, STOP, and FINISH share
  one 2-second deadline. A deadline covers partial socket calls, so a peer
  cannot extend it by trickling bytes. The deadline does not interrupt the
  Windows certificate verifier, for example during a revocation check, but it
  counts the verifier's time. A verifier that takes longer than the remaining
  time fails the handshake with a timeout.
- **Socket send buffer:** the connection asks the kernel for a 1 MiB send
  buffer (`SO_SNDBUF`), so a briefly slow receiver stalls the parser less. If
  the request fails, the connection proceeds with the default buffer.
- **Flush:** when the parser is idle and output is pending, it sends its
  partial block if the last send is 100 ms or older. While busy, it sends at
  least once a second. The sender thread flushes the socket at least every
  100 ms while blocks arrive, and at once when the queue is empty. The parser
  reads the clock every 64 events, so the busy bound is approximate.
- **Failure:** the sink abandons a failed session and counts the frames in
  flight. The sink sends no STOP after a partial frame. Capture continues and
  the sink reconnects. See [Receiver connection](#receiver-connection).
- **Encoded size:** each encoded message is at most 70 KiB.
- **DNS envelope:** the DNS message must be 12 to 65,535 bytes, and the QR bit must
  match the event's direction. The encoder preserves packet bytes verbatim. It
  does not check every name or resource record, and it does not strip or add a
  TCP length prefix.
- **Missing or malformed packet data:** the parser rejects it. The service never
  rebuilds a packet from a query name.
- **Standards:** the message layout follows the official
  [dnstap protobuf schema](https://github.com/dnstap/dnstap.pb/blob/main/dnstap.proto)
  (CC0). The Frame Streams control format follows the
  [fstrm reference](https://github.com/farsightsec/fstrm/blob/master/fstrm/control.h).
  The Prost subset keeps the upstream field numbers, enum values, optional-field
  presence, and `fixed32` nanosecond encoding. The project does not require
  `protoc`. [RFC 8618](https://www.rfc-editor.org/info/rfc8618/) defines C-DNS, a
  different CBOR-based format. This program writes dnstap, not C-DNS. Supported
  event names and payload fields follow
  [Microsoft's DNS logging documentation](https://learn.microsoft.com/en-us/windows-server/networking/dns/dns-logging-and-diagnostics).

## Receiver connection

The sink keeps one Frame Streams session to `--receiver`. Network failures
are recoverable. Protocol failures at startup are not.

- **Startup.** The service attempts the first connection once, synchronously. A
  network failure (refused, timed out, reset) does not stop the service. The service
  logs a warning, starts disconnected, and retries. A protocol failure at
  startup is fatal. A receiver that answers but does not accept the
  `protobuf:dnstap.Dnstap` content type, or that sends a malformed control
  frame, stops the service with an error.
- **Loss.** An established session can fail on a write, a flush, or a
  deadline. The sink then counts the frames in flight in `disconnected_dropped`.
  It logs a warning and starts to reconnect. Capture continues.
- **Retry.** Each reconnect attempt runs on a helper thread, so a blackholed
  receiver never blocks the parser. Each wait is uniform between half of a base
  delay and the base. The base starts at 250 ms and doubles up to a 30-second
  cap. A successful reconnect resets the base to 250 ms. Every new session
  repeats READY, ACCEPT, and START.
- **No resend.** The sink never sends a frame twice. Frames in flight when a
  session failed are counted in `disconnected_dropped` and are not resent.
  Frames that arrive while the receiver is down go to the spool when one is in
  use. Without a spool, they count in `disconnected_dropped`. See [Spool](#spool).
- **Incompatible receiver after startup.** A reconnect attempt that receives the
  wrong content type counts as a failed attempt. The service keeps running and
  retries. Only the first connection's protocol failure stops the service.
- **Shutdown while disconnected.** The sink sends no STOP. The service logs a
  warning and stops with success. Frames still in the spool stay on disk for
  the next start. Without a spool, the sink has already counted the frames that
  it did not deliver.
- **Shutdown while connected.** The sink sends STOP and waits for FINISH within
  the 2-second deadline. A missing or invalid FINISH is a shutdown failure: the
  service stops with an error.

### TLS

The Frame Streams session runs inside TLS unless you set `--plaintext`. With
`--plaintext`, the connection is plaintext TCP with no peer authentication, as
in earlier builds. A receiver that speaks plaintext while TLS is on fails the
handshake. That is a protocol failure, as described below.

- **Versions.** TLS 1.2 and TLS 1.3 only. The service offers no other version.
- **Library.** rustls with the `ring` crypto provider. Schannel is not used for
  the TLS record layer.
- **Verification.** Windows CryptoAPI checks the receiver certificate through
  `rustls-platform-verifier`. It uses the machine store: Trusted Root
  Certification Authorities and intermediates, and roots pushed by Group
  Policy. The certificate must match `--tls-server-name`, or the receiver IP
  as an `iPAddress` SAN when that flag is not set. The service sends the same
  name as SNI.
- **Revocation.** The verifier checks revocation for the receiver (end-entity)
  certificate only. It does not check the intermediates. The check is best
  effort. If the verifier cannot get the status, the connection proceeds. One
  cause is an unreachable Online Certificate Status Protocol (OCSP) responder or
  certificate revocation list (CRL) distribution point. The verifier does not
  check revocation for a certificate that names no OCSP responder and no CRL
  distribution point. The verifier rejects a certificate that Windows CryptoAPI
  reports as revoked. The report can come from a CRL, an OCSP responder, or an
  OCSP response that the receiver staples. The verifier gives revocation network
  retrieval a cumulative 10-second limit. The 2-second handshake deadline does
  not interrupt the verifier, but the time it takes counts against the
  deadline. If the verifier returns after the deadline passes, the next socket
  write fails the handshake with a timeout. That is a network failure, and the
  service retries it. See [Deadlines](#message-and-transport-limits). The
  service does not provide hard-fail revocation.
- **Failure class.** A certificate that fails the check is a protocol failure,
  not a network failure. At the first connection it is fatal: the service stops
  with an error (Event ID 1). On a reconnect it is a counted failed attempt, and
  the service keeps retrying. This is the same rule as the content-type failure
  above.
- **No client certificate.** The service does not authenticate itself to the
  receiver.
- **No close_notify.** FINISH ends the Frame Streams session. The TCP close
  follows. The service sends no TLS close_notify alert.

The TLS handshake runs under the 2-second handshake deadline. See
[Deadlines](#message-and-transport-limits) for the verifier exception. The
certificate requirements for the receiver are in
[receiver.md](receiver.md#receiver-certificate).

## Spool

The spool keeps frames on disk while the receiver is down or slow. It is on by
default. When the receiver connects again, the service sends the stored frames
first, oldest first. The spool needs a protected folder, and the service does
not create one. [Create the spool folder](install.md#create-the-spool-folder)
describes the install step.

**The spool stores DNS data.** Each stored frame is a complete dnstap message,
with client addresses and query names. The files are not encrypted. The ACL on
the folder is the only protection. See [SECURITY.md](../../SECURITY.md).

### Requirements

- The default folder is `C:\ProgramData\EtwDnstap\spool`. The service takes the
  `ProgramData` value from the environment. If that value is not a plain path on
  a local drive, it uses `C:\ProgramData`.
- At startup, the service checks the folder once.
- The folder must exist, and it must be a folder.
- The folder and its parent must not be reparse points, such as links or
  junctions. The folder must have a parent folder, so `C:\` is refused.
- The folder must be on a fixed local disk. Removable, network, and RAM disks
  fail the check.
- SYSTEM or Administrators must own the folder.
- Every allow entry that grants read or write access must name SYSTEM,
  Administrators, or a service SID. An inherit-only `CREATOR OWNER` entry is also
  allowed. Deny entries do not count.
- If a check fails, or the spool cannot open, the service logs one warning and
  runs without a spool. It does not stop. The warning text is in
  [troubleshooting.md](troubleshooting.md#spool-warnings).
- Restarts and reboots keep the files. The service checks the folder only at
  startup, so the ACL set at install is what keeps other accounts out later.

### Routing

For each block of frames, the sender thread chooses one destination:

- **Spool.** The receiver is down, the spool holds frames that have not been
  sent, or the send queue holds 96 or more of its 128 blocks. A slow receiver
  therefore spills to disk before the queue fills.
- **Receiver.** The receiver is connected, and the spool is empty or not in use.
- **Dropped.** No spool is in use and the receiver is down. The frames count in
  `disconnected_dropped`.

If the spool refuses a block, the frames count in `spool_dropped`. The send
queue is in memory. It absorbs short delays in the sender thread. It does not
hold an outage, because the sender writes blocks to the spool as soon as the
receiver is down.

The spool removes most slow-receiver loss, but not all of it. A socket write that
stalls for longer than the queue lasts still drops frames, and they count in
`queue_dropped`. With a spool in use, `queue_dropped` means that the sender was
blocked by a slow socket write or a slow disk.

### Order

While the spool holds frames, every new block goes to the spool, even when the
receiver is connected. Frames therefore arrive at the receiver in the order
they were captured. After the receiver reconnects, the sender replays the
oldest frames first, in batches of up to 256 KiB. Before each batch, it moves
the queued blocks to the spool, so the queue does not fill during a replay.

Frames left by an earlier run are replayed first, before any frame from the
current run. The file `cursor.bin` records the read position. Each run writes
its frames to new segment files. It never appends to a file from an earlier
run.

### Size limits and eviction

- `--spool-max-mib` (default 4096) caps the bytes of segment files. When a new
  block would exceed the cap, the spool deletes its oldest segments until the
  block fits.
- `--spool-min-free-mib` (default 2048) keeps free space on the volume. When a
  new segment starts, and free space is below the floor plus one segment, the
  spool deletes its oldest segments. It stops when a delete does not raise the
  free space. If only the newest segment is left, the spool refuses blocks for 10
  seconds, counts them in `spool_dropped`, and checks again.
- A segment holds up to 64 MiB, or one eighth of the cap when that is smaller.
- The oldest data goes first. Each deletion counts in `spool_dropped` and
  `spool_dropped_bytes`. A leftover segment from an earlier run adds only bytes,
  because its frame count is not known.
- The spool logs a warning for each eviction, the first four and then every
  60th. The log line gives the frames and bytes lost. The counters are complete.

Size the cap from the outage you must cover. [Spool sizing](tuning.md#spool-sizing)
has the figures.

### Delivery is at most once

- A frame is never sent twice. The sender moves the read position in
  `cursor.bin` before it writes a batch to the socket. A failure after that
  point loses the batch. The spool does not send it again.
- A frame in a failed batch counts in `replayed` and in `disconnected_dropped`.
- The loss is bounded. A failed connection loses the batch in progress, up to
  256 KiB, or about 2,500 frames at 103 bytes each. The batch counts in
  `replayed` and in `disconnected_dropped`. It also loses the block or chunk
  that was being written, up to 64 KiB or about 636 frames at 103 bytes each,
  and the frames written before it since the last flush. These count in
  `disconnected_dropped`. The lab saw one 64 KiB block per outage at 1,000 and
  50,000 queries per second. Data in the socket send buffer, up to 1 MiB, is
  lost too, and it is not counted.
- The spool does not flush data to disk. It protects against a receiver
  outage, not a power loss. A power loss can lose recent frames.

### Shutdown

At shutdown, the sender routes the queued blocks and seals the active segment.
The read position in `cursor.bin` is already current, because it is saved before
each batch. Only the segment is sealed. A backlog stays on disk for the next run.
The service does not send the backlog before it exits, so the shutdown time does
not depend on the backlog. The service stops writing data to the receiver 1
second after shutdown starts. A write in progress and the STOP/FINISH exchange
can take about 4 seconds more.

## Startup and shutdown deadlines

- **Startup.** The service-level startup deadline is 10 seconds plus 1 second
  per 16,384 channel slots of `--capacity`, rounded up. The default 24,576 gives
  12 seconds, and 262,144 gives 26 seconds. The previous default 8,192 gave
  11 seconds. It covers the identity checks,
  argument validation, receiver connection, spool folder check and open, pool
  allocation, and capture start.
  Capture start has its own inner 10-second deadline.
- **SCM progress.** The service reports the startup deadline to the SCM as the
  `START_PENDING` wait hint. It advances the checkpoint after every 16,384
  allocated pool slots. `sc.exe query EtwDnstap` shows the values as
  `CHECKPOINT` and `WAIT_HINT`. The `STOP_PENDING` wait hint is the shutdown
  deadline.
- **Shutdown.** The shutdown deadline is 10 seconds from the stop request to the
  final report. Inside it, ETW waits up to 5 seconds for `ProcessTrace` to
  return on its own after the session stops. Only then does it close the trace
  handle to force the worker out. The parser then sends its last block. The sender
  thread routes the queued blocks, seals the spool segment, and runs STOP/FINISH.
  It stops writing data to the receiver 1 second after shutdown starts. A write
  in progress and the STOP/FINISH exchange can take about 4 seconds more, so the
  sink ends in about 5 seconds at most. See [Spool](#spool).
- **Expiry.** The process writes Event ID 5 to the Application log and aborts,
  so SCM recovery applies. See
  [Watchdog aborts](troubleshooting.md#watchdog-aborts-or-stop-hangs).

## Session reclaim

The service has no instance mutex. Only one instance runs, for two reasons.
The SCM starts one process for the service. The identity check refuses a
process that does not run as this service.

On startup, the service stops and replaces a leaked session with the same name,
the same session GUID, and this program's shape. The session GUID is the one
that the service derives from the session name. The shape is real-time only,
with no file, circular, append, new-file, system, buffering, or private logger
mode, and no log file.
Buffer size, buffer counts, flush timer, and clock type are not part of the
shape. A different `--etw-buffer-kib` does not block reclaim. The service logs
`stale ETW session used a different buffer size; reclaiming anyway` and
continues. The service does not stop any other session with that name,
including one with a different GUID. Startup fails instead.

The service does not reclaim sessions named `etw-dnstap-<pid>` from earlier
builds. Find them with `logman query -ets`. Then stop each one with
`logman stop etw-dnstap-<pid> -ets`.

## Next steps

- [Tune capture for load](tuning.md)
- [Log events and counters](monitoring.md)
- [Install and provision](install.md)
- [Troubleshooting](troubleshooting.md)
- [Measurement terms](../design/measurement-terms.md)
