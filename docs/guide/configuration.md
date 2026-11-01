# Configuration reference

> **Status:** Current · **Describes:** `main` at commit `dde7188` with the TLS receiver change (defaults from `CaptureArgs` in `src/pipeline.rs`) · **Reviewed:** 2026-11-01

Every option is set in the service binary path, not in `sc.exe start`. Install
and provisioning steps are in [install.md](install.md). Symptoms are in
[troubleshooting.md](troubleshooting.md).

## Options

Defaults and ranges come from `CaptureArgs` in `src/pipeline.rs`. Values outside
the range are rejected at startup.

| Option | Default | Range or format | Meaning |
| --- | --- | --- | --- |
| `--receiver IP:PORT` | none (required) | Numeric unicast IP and nonzero port. IPv6 as `[addr]:port`. | Frame Streams TCP receiver. The connection is TLS by default and plaintext with `--plaintext`. Hostnames are rejected. The service starts even when the receiver is down and reconnects. See [Receiver connection](#receiver-connection). |
| `--plaintext` | off | Flag | Disables TLS. The connection is plaintext TCP with no peer authentication. Use it only on a trusted network. See [TLS](#tls). |
| `--tls-server-name NAME` | the receiver IP | A DNS name or an IP literal. Cannot be combined with `--plaintext`. | The name that the receiver certificate must match. It is also sent as SNI. Without it, the certificate must carry the receiver IP as an `iPAddress` SAN. |
| `--capacity N` | `8192` | 1,024 to 262,144 events | Event channel capacity. A full channel drops the newest event and increments `ring_dropped` and `intake_dropped`. The startup budget grows with this value. See [Startup and shutdown deadlines](#startup-and-shutdown-deadlines). |
| `--jumbo-capacity N` | `64` | 0 to 256 | Pool slots for records with a padded payload over 2 KiB. `0` disables jumbo capture. |
| `--duration SECONDS` | none | 1 to 604,800 | Stop after this many seconds. Without it, capture runs until SCM stops the service or a failure occurs. |
| `--session-name NAME` | `EtwDnstap` | 1 to 200 UTF-16 units, no NUL | Name of the real-time ETW session. A leaked session with the same name, shape, and session GUID is reclaimed at startup. See [Session reclaim](#session-reclaim). The name also sets the session GUID that the ETW grant names. A new name therefore needs a run of the grant script with that name, and a restart. See [install.md](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source). |
| `--identity TEXT` | `etw-dnstap` | At most 1,024 UTF-8 bytes | dnstap producer identity. Empty text omits the field. |
| `--etw-buffer-kib KIB` | `16` | 4 to 128, a multiple of 4 | ETW session buffer size. |
| `--etw-max-buffers N` | `12` | 4 to 4,096 | Requested maximum ETW buffer count. See [ETW buffer minimums](#etw-buffer-minimums). |
| `--etw-flush-seconds SECONDS` | `1` | 1 to 60 | ETW's own real-time flush interval. It is the delivery bound when `--etw-flush-ms` is `0`. |
| `--etw-flush-ms MS` | `0` (off) | `0`, or 20 to 1,000 | How often the service asks ETW to flush the session. See [Low-rate defaults](#low-rate-defaults). |
| `--stats-interval-seconds N` | `60` | 0 to 86,400 | Seconds between the `capture stats` and `output stats` log events. `0` disables them. The final `capture stopped` and `output stopped` events are always written. See [Log events and counters](#log-events-and-counters). |
| `--help`, `-h` | | | Print help and exit. Capture does not start. |
| `--version`, `-V` | | | Print the package version and exit. Capture does not start. |

### Removed options

`--output`/`-o`, `--max-output-bytes`, and `--metrics` were removed. Arguments
that use them fail command-line parsing. The service writes no capture or
metrics files.

### Diagnostic flags (`diag-timing` builds only)

These options exist only in a binary built with `--features diag-timing`. Do not
use them in production.

| Option | Range | Effect |
| --- | --- | --- |
| `--diag-timer-resolution-ms MS` | 1 to 15 | Calls `timeBeginPeriod` for the whole run and withdraws it on exit. |
| `--diag-thread-priority LEVEL` | `above-normal` or `highest` | Raises the priority of the `dnstap-parser` and `etw-consumer` threads. Failure to set it is logged once and ignored. |

The feature also times the parser's idle parks and socket writes, and adds the
results to the stop report. It is for locating wait time. It does not change
capture behavior.

## ETW buffer minimums

ETW raises the requested buffer counts. It sets the minimum buffers to at least
two per logical processor, and it raises the maximum to at least that minimum.
`--etw-max-buffers 12` is therefore the effective floor on a 6-CPU host, and the
count is higher on larger hosts. Peak buffer memory can exceed the requested
bound. Raising `--etw-max-buffers` only allows more buffers on demand. It does
not allocate them up front.

## Low-rate defaults

This is the canonical statement of the default limits. Other documents link here.

**The defaults are tuned for about 1,000 queries per second or less.** The
low-rate warning below uses its own threshold of 1,500 events per second. Lab
results at higher rates with the defaults follow.

With the periodic flush off (`--etw-flush-ms 0`), ETW delivers real-time events
in batches about `--etw-flush-seconds` (1 second by default) apart. Delivery
latency is therefore up to about 2 seconds, and usually under 1 second. A batch
that holds more events than the channel can take drops the excess unless the
parser drains it while it arrives. The ETW buffer callback wakes the parser for
each delivered buffer, so it does. The lab results on the 6-vCPU VM with all
defaults and the committed build were:

- Authoritative loss was 0 at 1,000 and at 5,000 queries per second.
- Authoritative loss was 0.32% at 10,000 queries per second. An earlier variant
  with an always-on linger lost 1.08%.
- Before this wake, the same 5,000 queries-per-second load lost about 19%.

These figures count channel drops only, in 50-second runs. See
[ETW buffer wake validation](../history/buffer-wake-validation.md).

One lab result predates the buffer wake. The lab used a 6-vCPU VM, Windows
Server 2025, an authoritative workload, and the default 8,192-event channel. The
configuration had the flush off, 64 KiB buffers, and 256 maximum buffers. With
it, 59 to 79% of events were dropped at 10,000 to 20,000 queries per second.

This flush-off configuration has not been measured again with the buffer wake.

For higher rates, use the periodic flush with larger buffers:

```text
--etw-flush-ms 50 --etw-max-buffers 256 --etw-buffer-kib 64
```

With that setting, the same lab reported 0 ring drops at 10,000 and 20,000
queries per second. Tested intervals:

| Queries/s | Flush interval | Result |
| --- | --- | --- |
| 20,000 | 50 ms | 0 ring drops |
| 10,000 | 100 ms | 0 ring drops |
| 50,000 | 100 ms | 0.04 to 0.91% loss. `etw_lost` was 0. |

The 50,000 queries-per-second row needs care. The same source describes loss of
0.3 to 1.7% at 50,000 queries per second **without** the periodic flush. The
flush setting for that comparison run is not recorded in the same document, so
treat the two figures as separate observations. The residual loss tracks host
disturbance according to that source.

These cases were not tested:

- Other machines and vCPU counts.
- 25,000 to 45,000 queries per second.
- Recursive or mixed workloads.
- A 50 ms interval at 50,000 queries per second and above in a release build.

Each delivered batch holds up to about twice the flush interval's worth of
events (2 events per query). At higher rates, lower `--etw-flush-ms` or raise
`--capacity` so that `2 × rate × interval` stays well below the channel
capacity. At 20,000 queries per second, the largest batch was about 6,800
events with a 100 ms interval and about 3,500 with 50 ms.

The evidence is in [ring-buffer.md](../design/ring-buffer.md) and
[singlepass-four-core-test-results.md](../history/singlepass-four-core-test-results.md).
The per-stage lab labels from the source reports are not defined elsewhere and
are not used here.

**Warning.** The service can log a warning (Event ID 3) when the default ETW
settings are in use. It logs the warning at the end of each measurement window
in which the received rate is more than about 1,500 events per second. The window is `--stats-interval-seconds`,
or 60 seconds when snapshots are disabled. The warning repeats while the rate
stays high, and it names the high-rate settings above.

### Flush cost

With `--etw-flush-ms` enabled, the lab run wrote about 5 to 16% more ETW buffers
and cost under 0.1% of a core when idle. The `capture stopped` line reports
`etw_flushes`, `etw_flush_errors`, and `etw_flush_max_us` (the longest flush call,
including scheduling delay on the supervisor). Lab maxima under load were 11 to
46 ms, near multiples of the 15.6 ms timer tick. The idle maximum was about
0.35 ms.

A failed flush logs one warning and is counted. Capture continues. The ETW
buffer callback wakes the parser when the flushed buffers arrive. After each
successful periodic flush, the supervisor also wakes the parser as a backstop.

## Storage

Counted storage at the defaults is 28.22 MiB. At the maximum (262,144 events and
256 jumbo slots) it is 678.14 MiB. These are calculated payload, slot, free-list,
and channel totals. They exclude the allocator, native ETW buffers, TDH caches,
protobuf data, and output buffers. They are not measured process-memory limits.
See [pooled-intake.md](../design/pooled-intake.md) for the budget.

## Subscription

The service enables the `Microsoft-Windows-DNSServer` provider at level 4 in its
own real-time session. It does not enable the Analytical event log. At startup
it resolves the `QUERY_RECEIVED` and `RESPONSE_SUCCESS` keyword bits by name and
sets `MatchAnyKeyword` to them. If the lookup fails, it logs one warning and
uses `0x8000000000000000` instead, the bit associated with the Analytical
channel. The `capture started` line shows the masks in use and whether the
fallback applied.

Only event IDs 256 (query received) and 257 (response success) are accepted,
which produce `CLIENT_QUERY` and `CLIENT_RESPONSE`. Event IDs 258, 260, and 261
are excluded, and excluded events do not increment any counter. See
[README.md](../../README.md#what-is-captured) for the message mapping.

Two more facts from the subscription code:

- The subscription sets `EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0`, so an event
  with keyword 0 does not match.
- ETW also filters event IDs 256 and 257 before events reach the session
  buffers. This reduces buffer use and callback dispatch. It does not reduce
  the DNS Server's cost to generate events that pass the keyword mask.
- Events 284, 287, 597, and 598 share the narrowed keyword bits on Windows
  Server 2025 build 26100. The event-ID check therefore stays in the callback
  as defense in depth.

### DNS process filter

The same provider is also filtered to one process: the running `DNS` service
(the DNS Server role). The subscription adds an `EVENT_FILTER_TYPE_PID` filter
with that process ID, and the callback checks the record's process ID again
before it copies the record. A mismatch is counted in `foreign_pid_rejected`
and is not counted in `received`.

- Startup fails if the `DNS` service is not running. Start it first.
- The process ID is read again every 5 seconds. If it changes, or the service
  stops, EtwDnstap stops with a service failure. SCM then restarts it under the
  recovery policy in [install.md](install.md#5-configure-scm-recovery), and the
  new instance resolves the new process ID.
- Records from a restarted DNS Server are not captured until EtwDnstap has
  restarted. Expect a gap of at least the detection interval plus the SCM
  restart delay.
- The ETW-side filter is expected to keep other processes' records out of the
  callback. Whether it does so on a live host is unverified. A nonzero
  `foreign_pid_rejected` shows that records from another process reached the
  callback.

## Message and transport limits

- **Frame Streams content type:** `protobuf:dnstap.Dnstap`, with protobuf data
  frames. Control payloads up to 512 bytes and content-type fields up to 256
  bytes are accepted.
- **Deadlines:** connection establishment has a 2-second timeout. The whole
  handshake, each buffered write or flush, and STOP/FINISH each have a 2-second
  deadline shared across partial socket calls. A peer cannot extend a deadline
  by trickling bytes. The TLS handshake shares the 2-second connect and
  handshake deadline with READY and ACCEPT. Time spent inside the
  Windows certificate verifier, for example a slow revocation check, is not
  socket time. That deadline does not bound it.
- **Socket send buffer:** the connection asks the kernel for a 1 MiB send
  buffer (`SO_SNDBUF`), so a briefly slow receiver stalls the parser less. If
  the request fails, the connection proceeds with the default buffer.
- **Flush:** when the parser is idle and output is pending, it flushes if the
  last flush is 100 ms or older. While busy, it flushes at least once a
  second. The clock is read every 64 events, so the busy bound is approximate.
- **Failure:** a failed session is abandoned, and its buffered frames are
  discarded. No STOP is sent after a partial frame. Capture continues and the
  sink reconnects. See [Receiver connection](#receiver-connection).
- **Encoded size:** each encoded message is limited to 70 KiB.
- **DNS envelope:** the DNS message must be 12 to 65,535 bytes, and the QR bit must
  match the event's direction. The encoder preserves packet bytes verbatim. It
  does not validate every name or resource record, and it does not strip or add
  a TCP length prefix.
- **Missing or malformed packet data** is rejected. It is never rebuilt from a
  query name.
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

- **Startup.** The first connection is attempted once, synchronously. A network
  failure (refused, timed out, reset) does not stop the service. The service
  logs a warning, starts disconnected, and retries. A protocol failure at
  startup is fatal. A receiver that answers but does not accept the
  `protobuf:dnstap.Dnstap` content type, or that sends a malformed control
  frame, stops the service with an error.
- **Loss.** An established session can fail on a write, a flush, or a
  deadline. The sink then discards its write buffer and the frame in hand. It
  logs a warning and starts to reconnect. Capture continues.
- **Retry.** Each reconnect attempt runs on a helper thread, so a blackholed
  receiver never blocks the parser. Each wait is uniform between half of a base
  delay and the base. The base starts at 250 ms and doubles up to a 30-second
  cap. A successful reconnect resets the base to 250 ms. Every new session
  repeats READY, ACCEPT, and START.
- **No resend.** Frames that were buffered when a session failed, and frames
  that arrive while disconnected, are never sent again. They are counted in
  `disconnected_dropped`.
- **Incompatible receiver after startup.** A reconnect attempt that receives the
  wrong content type counts as a failed attempt. The service keeps running and
  retries. Only the first connection's protocol failure stops the service.
- **Shutdown while disconnected.** No STOP is sent. The service logs a warning
  and stops with success. The frames that were not delivered are counted.
- **Shutdown while connected.** The sink sends STOP and waits for FINISH within
  the 2-second deadline. A missing or invalid FINISH is a shutdown failure: the
  service stops with an error.

### TLS

The Frame Streams session runs inside TLS unless `--plaintext` is given. With
`--plaintext`, the connection is plaintext TCP with no peer authentication, as
in earlier builds. A receiver that speaks plaintext while TLS is on fails the
handshake. That is a protocol failure, as described below.

- **Versions.** TLS 1.2 and TLS 1.3 only. No other version is offered.
- **Library.** rustls with the `ring` crypto provider. Schannel is not used for
  the TLS record layer.
- **Verification.** The receiver certificate is verified by Windows CryptoAPI
  through `rustls-platform-verifier`. It uses the machine store: Trusted Root
  Certification Authorities and intermediates, roots pushed by Group Policy, and
  revocation checking. The certificate must match `--tls-server-name`, or the
  receiver IP as an `iPAddress` SAN when that flag is not set. The same name is
  sent as SNI.
- **Failure class.** A certificate that does not verify is a protocol failure,
  not a network failure. At the first connection it is fatal: the service stops
  with an error (Event ID 1). On a reconnect it is a counted failed attempt, and
  the service keeps retrying. This is the same rule as the content-type failure
  above.
- **No client certificate.** The service does not authenticate itself to the
  receiver.
- **No close_notify.** FINISH ends the Frame Streams session. The TCP close
  follows. No TLS close_notify alert is sent.

The TLS handshake runs under the 2-second connect and handshake deadline. See
[Deadlines](#message-and-transport-limits) for the verifier exception. The
certificate requirements for the receiver are in
[install.md](install.md#receiver-certificate).

## Startup and shutdown deadlines

- **Startup.** The service-level startup deadline is 10 seconds plus 1 second
  per 16,384 channel slots of `--capacity`, rounded up. The default 8,192 gives
  11 seconds, and 262,144 gives 26 seconds. It covers the identity checks,
  argument validation, receiver connection, pool allocation, and capture start.
  Capture start has its own inner 10-second deadline.
- **Shutdown.** The shutdown deadline is 10 seconds from the stop request to the
  final report. Inside it, ETW waits up to 5 seconds for `ProcessTrace` to
  return on its own after the session stops. Only then does it close the trace
  handle to force the worker out. The parser then drains and runs STOP/FINISH
  with its 2-second deadline.
- **Expiry.** The process writes Event ID 5 to the Application log and aborts,
  so SCM recovery applies. See
  [Watchdog aborts](troubleshooting.md#watchdog-aborts-or-stop-hangs).

## Session reclaim

The service has no instance mutex. Only one instance runs, for two reasons.
The SCM starts one process for the service. The identity check refuses a
process that does not run as this service.

On startup, a leaked session with the same name, the same session GUID, and
this program's shape is stopped and replaced. The session GUID is the one that
the service derives from the session name. The shape is real-time only, with no file, circular,
append, new-file, system, buffering, or private logger mode, and no log file.
Buffer size, buffer counts, flush timer, and clock type are not part of the
shape. A different `--etw-buffer-kib` does not block reclaim. The service logs
`stale ETW session used a different buffer size; reclaiming anyway` and
continues. Any other session with that name, including one with a different GUID, is left alone, and startup fails.

Sessions named `etw-dnstap-<pid>` from earlier builds are not reclaimed.
Clean them up once with `logman query -ets`, then `logman stop etw-dnstap-<pid> -ets`.

## Log events and counters

Diagnostics go to the Application event log under source `EtwDnstap` and to
stderr. The service writes no capture, metrics, or log files.

### Log event IDs

| Event ID | Type | Written for |
| --- | --- | --- |
| 1 | Error | A fatal service failure, whether at startup or during the run. The text begins `EtwDnstap failed:`. |
| 2 | Information | Every information-level line: `capture started`, `capture stats`, `output stats`, `capture stopped`, `output stopped`, `dnstap receiver reconnected`, and `diagnostic settings applied` in diagnostic builds. |
| 3 | Warning | Every warning-level line, such as an unavailable receiver, a lost connection, a failed reconnect, a rejected event, the low-rate warning, and a reclaimed session. |
| 4 | Error | An error-level line that does not by itself stop the service, such as the reason for a DNS Server change exit. |
| 5 | Error | Watchdog expiry, written just before the process aborts. |

### Periodic and final reports

Every `--stats-interval-seconds`, the service writes two information events,
`capture stats` and `output stats`. Each snapshot first asks ETW for its own
loss counters. The shutdown always writes `capture stopped` and `output stopped`
with the same fields, even when periodic snapshots are disabled.

`capture started` records the session name, the session GUID (`session_guid`), receiver, `tls`
(`true` unless `--plaintext` is given), `tls_server_name` when set, channel capacity, ETW
buffer size, maximum buffers, flush seconds, `etw_flush_ms`, the keyword masks
as `match_any_keyword` and `match_all_keyword` in hexadecimal, and
`keyword_fallback`.

### Capture counters

These fields appear on `capture stats` and `capture stopped`.

| Counter | Meaning |
| --- | --- |
| `received` | Records of event IDs 256 and 257 that the callback accepted after the process check. Includes records later dropped or rejected. |
| `dequeued` | Records the parser took from the channel. |
| `encoded` | Records the parser encoded and handed to the sink. Includes frames the sink later discarded while disconnected, so it is not a delivery count. |
| `ignored` | Dequeued records the parser did not capture because they are not a supported DNS event. |
| `rejected` | Dequeued records the parser rejected, such as a malformed packet, an invalid field, or a schema failure. Includes `schema_rejected`. |
| `ring_dropped` | Records dropped because the channel was full. Part of `intake_dropped`. |
| `intake_dropped` | Every record lost at admission: `ring_dropped`, `pool_exhausted_regular`, `pool_exhausted_jumbo`, and `receiver_disconnected`. |
| `pool_exhausted_regular` | Regular-class records dropped because no regular pool slot was free. Part of `intake_dropped`. |
| `pool_exhausted_jumbo` | Jumbo-class records (padded payload over 2 KiB) dropped because no jumbo pool slot was free, or because `--jumbo-capacity` is `0`. Part of `intake_dropped`. |
| `foreign_pid_rejected` | Records from a process other than the DNS Server. Not counted in `received`. Normally zero. See [DNS process filter](#dns-process-filter). |
| `receiver_disconnected` | Records dropped because the channel's parser end was gone. This is the event channel, not the TCP receiver. TCP state is in the output counters. |
| `etw_lost` | Events ETW reports as lost in the session, from its last successful query or stop. |
| `etw_buffers_lost` | ETW buffers lost. This is a buffer count, not an event count. Never add it to event counts. |
| `callback_errors` | Records the callback could not copy or hand off, for example malformed native data, or thread-local state that is missing or re-entered. Not part of `intake_dropped`. |
| `schema_rejected` | Records whose event schema failed to load or did not compile into a single-pass decode plan. This includes the first failure for each (event ID, version) key and each later record with that key. Part of `rejected`. |
| `etw_stats_valid` | `true` when the last ETW query or stop succeeded. A flag, not a count. |
| `etw_stats_errors` | Failed ETW statistics queries. |
| `stale_sessions_reclaimed` | Leaked sessions stopped at startup. |
| `etw_flushes` | Successful periodic ETW flush requests. |
| `etw_flush_errors` | Failed flush requests. Only the first is logged. |
| `etw_flush_max_us` | Longest flush call in microseconds, including supervisor scheduling delay. |
| `etw_buffers_delivered` | ETW buffers whose delivery finished (calls of the buffer callback). |
| `parser_wake_requests` | Delivered buffers that enqueued at least one event. Each sends one wake request to the parser. The request costs nothing when the parser is already running. This is therefore not a count of parser wakeups. |

### Output counters

These fields appear on `output stats` and `output stopped`.

| Counter | Meaning |
| --- | --- |
| `output_failed_events` | Dequeued records whose frame write returned an error. The current network sink does not return write errors. It counts failures in the fields below. This stays 0. |
| `output_errors` | Parser output failures. The current sink does not fail a flush, so this increments only when the STOP/FINISH exchange at shutdown fails. |
| `receiver_connected` | `true` while a Frame Streams session is established. A flag, not a count. |
| `receiver_disconnects` | Established sessions that were lost. |
| `receiver_reconnects` | Successful reconnects after the first session. |
| `receiver_reconnect_failures` | Reconnect attempts that failed at connect, handshake, or protocol. |
| `disconnected_dropped` | Frames discarded because the transport was down: frames that arrived while disconnected, plus frames written since the last successful flush when a session failed. Those may have reached the receiver. Never resent. Not part of `intake_dropped`. |
| `slow_writes` | Socket writes that took at least 5 ms. |
| `write_max_us` | Longest single socket write in microseconds. |
| `queue_depth_max` | Deepest the channel was seen, counting the event being processed. Sampled on the first event after each idle poll and on every 256th event, so it is a lower bound. |

### Counter relationships

```text
intake_dropped = ring_dropped + pool_exhausted_regular + pool_exhausted_jumbo
                 + receiver_disconnected
received       = intake_dropped + callback_errors + dequeued + queue backlog
dequeued       = encoded + ignored + rejected + output_failed_events
```

- `ring_dropped` and the two `pool_exhausted_*` fields are parts of
  `intake_dropped`. Never add them to it. There is no logged `pool_exhausted`
  field. Its total is the sum of the two class fields.
- `schema_rejected` is part of `rejected`.
- `disconnected_dropped` is not part of `intake_dropped` or of the identities
  above. Its frames were already counted in `encoded`. Approximately,
  `encoded - disconnected_dropped` is a lower bound on frames that reached the
  socket. It is not a receiver acknowledgment.
- `etw_lost` and `etw_buffers_lost` are ETW's own counters, in ETW's units. They
  are not part of these identities.
- Live samples read separate atomic counters and can disagree briefly. The
  final report, written after capture stops and the parser joins, has no
  backlog, so the identities should close exactly there.

## Related

- [Install and provision](install.md)
- [Troubleshooting](troubleshooting.md)
- [Measurement terms](../design/measurement-terms.md)
