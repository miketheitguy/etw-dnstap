# Configuration reference

> **Status:** Current · **Describes:** `main` at commit `dde7188` with the TLS receiver change, the removal of the explicit ETW flush (defaults from `CaptureArgs` in `src/pipeline.rs`), the Windows performance counter provider (new in this build, not yet tested in the lab), and the new defaults `--etw-max-buffers 64` and `--capacity 24576` (not lab-tested as a set) · **Reviewed:** 2026-11-01

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
| `--capacity N` | `24576` | 1,024 to 262,144 events | Event channel capacity. A full channel drops the newest event and increments `ring_dropped` and `intake_dropped`. The startup budget grows with this value. See [Startup and shutdown deadlines](#startup-and-shutdown-deadlines). |
| `--jumbo-capacity N` | `64` | 0 to 256 | Pool slots for records with a padded payload over 2 KiB. `0` disables jumbo capture. |
| `--duration SECONDS` | none | 1 to 604,800 | Stop after this many seconds. Without it, capture runs until SCM stops the service or a failure occurs. |
| `--session-name NAME` | `EtwDnstap` | 1 to 200 UTF-16 units, no NUL | Name of the real-time ETW session. A leaked session with the same name, shape, and session GUID is reclaimed at startup. See [Session reclaim](#session-reclaim). The name also sets the session GUID that the ETW grant names. A new name therefore needs a run of the grant script with that name, and a restart. See [install.md](install.md#4-set-the-service-identity-privileges-etw-access-and-event-source). |
| `--identity TEXT` | `etw-dnstap` | At most 1,024 UTF-8 bytes | dnstap producer identity. Empty text omits the field. |
| `--etw-buffer-kib KIB` | `16` | 4 to 128, a multiple of 4 | ETW session buffer size. |
| `--etw-min-buffers N` | `0` | 0 to 4,096, at most `--etw-max-buffers` | Requested minimum ETW buffer count, reserved when the session starts. `0` lets ETW choose. ETW raises the value to at least two per logical processor. See [ETW buffer minimums](#etw-buffer-minimums). |
| `--etw-max-buffers N` | `64` | 4 to 4,096 | Requested maximum ETW buffer count. ETW allocates buffers only on demand, up to this maximum. See [ETW buffer minimums](#etw-buffer-minimums). |
| `--etw-flush-seconds SECONDS` | `1` | 1 to 60 | ETW real-time flush interval in seconds. ETW delivers real-time events about once per interval, so this value bounds delivery latency. A longer interval uses less CPU and makes each delivered batch larger. See [Low-rate defaults](#low-rate-defaults). |
| `--stats-interval-seconds N` | `60` | 0 to 86,400 | Seconds between the `capture stats` and `output stats` log events. `0` disables them. The final `capture stopped` and `output stopped` events are always written. See [Log events and counters](#log-events-and-counters). |
| `--help`, `-h` | | | Print help and exit. Capture does not start. |
| `--version`, `-V` | | | Print the package version and exit. Capture does not start. |

### Removed options

`--output`/`-o`, `--max-output-bytes`, and `--metrics` were removed. Arguments
that use them fail command-line parsing. The service writes no capture or
metrics files.

This build removes `--etw-flush-ms`. The service no longer asks ETW to flush
the session. A binary path that still has `--etw-flush-ms` fails command-line
parsing, and the service does not start. Remove the option from the binary
path. For high rates, use the settings in [High-rate settings](#high-rate-settings).

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

`--etw-min-buffers` sets the requested minimum buffer count. ETW allocates the
minimum when the session starts and keeps those buffers for the whole session.
The reserved memory is the minimum times `--etw-buffer-kib`. ETW allocates more
buffers only on demand, up to the maximum, and that allocation happens during
the burst that needs the buffers. Raising `--etw-max-buffers` only allows more
buffers on demand. It does not allocate them up front.

The default is `0`, which lets ETW choose. The service holds no fixed minimum.
ETW raises the minimum to at least two buffers per logical processor, and it
may reduce a minimum that exceeds its own limit. It raises the maximum to at
least the adjusted minimum. The default maximum is 64. On a host with more than
32 logical processors, ETW's minimum is above 64, and ETW raises the maximum to
match. With the previous default maximum of 12, the maximum was the effective
floor on a 6-CPU host: ETW's minimum and maximum were both 12 on the lab VM.
Peak buffer memory can exceed the requested bound. With the default, a host with 1 logical
processor reserves 2 buffers. A host with 2 or more logical processors reserves
two per processor.

The minimum must not exceed `--etw-max-buffers`. Otherwise the service does not
start and reports `ETW minimum buffers (65) must not exceed ETW maximum buffers
(64)`, with the two values given. ETW cannot change the minimum while the
session runs, so a new value takes effect at the next service start.

Microsoft advises starting to tune a session by setting the minimum and the
maximum to the same value. Raise the maximum if the session drops events during
peaks.

ETW can apply different values than the ones requested. After the session
starts, the service queries it once and writes the information event `ETW
buffers applied`. It has `etw_min_buffers_applied`, `etw_max_buffers_applied`,
and `etw_buffers_allocated`, as ETW reports them. If the query fails, the
service writes no such event. The `ETW Buffers Allocated` counter also shows the
effective minimum just after the session starts.

## Low-rate defaults

This is the canonical statement of the default limits. Other documents link here.

The defaults are `--etw-buffer-kib 16 --etw-max-buffers 64 --capacity 24576`:
16 KiB buffers, at most 64 of them, and a channel of 24,576 events.

**The lab validated the current defaults in one 60-second run at 10,000 queries
per second** (about 20,000 events per second) on 2026-11-01. Every event was
captured, with 0 ring drops and 0 `etw_lost`. ETW delivered late four times; each
time about 35,000 to 41,000 events arrived in one second, and the channel peaked
at 8,800 events. ETW had at most 5 buffers in use and allocated at most 18. The
rate warning below uses a threshold of 22,000 events per second, just above
this validated rate.

The tables below were measured with the previous defaults: 16 KiB buffers, a
maximum of 12, and a channel of 8,192 events. Read them as results for those
settings.

ETW delivers real-time events in batches about `--etw-flush-seconds` (1 second
by default) apart. Delivery latency is therefore up to about 2 seconds, and
usually under 1 second. The service does not ask ETW for more flushes. A batch
that holds more events than the channel can take drops the excess unless the
parser drains it while it arrives. The ETW buffer callback wakes the parser for
each delivered buffer, so the parser drains each batch while it arrives.

The lab used one 6-vCPU, 4 GiB VM with Windows Server 2025, an authoritative
load, and 50-second runs. The rates are the rates that the client achieved. The
results with all of the previous defaults were:

| Queries per second | Ring drops | `etw_lost` | Build |
| --- | --- | --- | --- |
| 1,000 | 0 | 0 | Buffer wake, and this build (959 queries per second) |
| 5,000 | 0 | 0 | Buffer wake |
| 7,300 to 7,500 | 0 to 0.12%. 2 of 4 runs dropped events | 0 | This build |
| 9,000 to 10,000 | 0.28 to 0.58% | 0 | This build. The buffer-wake build lost 0.32% |
| 14,700 | 1.63% | 0 | This build |
| 30,000 | 12.3 to 16.6% | 0.10 to 0.16% | This build, two clients. See the note below |
| 44,000 | 2.23% | 0.135% | This build |
| 49,500 to 50,000 | 0.34 to 4.39% | 0.199 to 0.215% | This build, two clients. The buffer-wake diag build had 0 ring drops and 0.03 to 0.11% `etw_lost` |

- The 8,192-event channel was full in every run at about 9,000 queries per
  second and above.
- `etw_lost` is loss inside the previous default of 16 KiB × 12 ETW buffers.
- The runs at about 30,000 queries per second lost more than the runs at
  44,000 to 50,000. They also used less service CPU: 1.8 to 1.9 seconds against
  6.6 to 8.7 seconds. Two clients with different pacing gave the same loss, so
  the client is not the cause. The cause in the service or in ETW is not known.
- At about 50,000 queries per second, the loss depended on the client: 0.34 to
  1.24% with 54 client threads, and 4.39% with 4 threads and steady pacing.
- Before the buffer wake, a 5,000 queries-per-second load lost about 19%.

See [ETW buffer wake validation](../history/buffer-wake-validation.md) and
[ETW flush removal validation](../history/flush-removal-validation.md).

The lab did not test these cases with the previous defaults:

- Other machines and vCPU counts.
- Recursive or mixed workloads.
- Runs longer than 50 seconds.
- A TLS receiver. The lab used a plaintext receiver.

Later lab runs on 2026-11-01 used other values:

- With `--etw-buffer-kib 16`, `--etw-max-buffers 32`, and `--capacity 8192` at
  10,000 queries per second, two 60-second runs dropped 10,751 and 12,747
  events at the channel. Each drop followed a late ETW real-time delivery: a
  second with about 1,500 events, then about 40,000 the next second. ETW lost
  nothing.
- In a 10-minute run at 10,000 queries per second with 16 KiB buffers, ETW had
  up to 16 buffers in use and allocated up to 21.
- With the previous default maximum of 12 buffers on the 6-vCPU lab VM, ETW's
  minimum and maximum were both 12.

The current defaults raise the channel to 24,576 events and the maximum to 64
buffers. The lab has not run them as a set, so these results do not show that
they avoid drops at 10,000 queries per second.

### High-rate settings

For rates above about 1,000 queries per second, use larger ETW buffers and a
larger channel:

```text
--etw-max-buffers 256 --etw-buffer-kib 64 --capacity 65536
```

The lab tested this combination on the same VM as the previous defaults. The load was
authoritative, each run lasted 50 seconds, and the receiver was plaintext. The
rates are the rates that the client achieved:

| Queries per second | Runs | Ring drops | `etw_lost` | Peak queue |
| --- | --- | --- | --- | --- |
| 7,300 to 7,500 | 4 | 0 | 0 | 8,201 to 8,876 |
| 9,000 to 10,000 | 4 | 0 | 0 | 9,375 to 10,627 |
| 14,700 | 1 | 0 | 0 | 16,776 |
| 30,000 | 2 | 0 | 0 | 27,329 and 28,226 |
| 44,000 | 1 | 0 | 0 | 30,643 |
| 49,400 | 2 | 0 and 0.28% | 0 | 46,408, and 65,536 (full) |

The peak queue is `queue_depth_max`, a sampled lower bound.

- ETW lost no events in any run.
- The channel dropped no events at every rate up to about 44,000 queries per
  second. At 44,000 the peak queue was 30,643 of 65,536.
- At about 49,400 queries per second, one of two runs filled the 65,536-event
  channel and dropped 0.28% of events. The other run peaked at 46,408.
- At about 49,500 queries per second, the service used about 4.4 seconds of
  CPU in each 50-second run. With the previous defaults it used 8.4 to 8.7
  seconds.
- Private bytes rose from about 31 MiB with the previous defaults to about
  179 MiB.

**65,536 is enough up to about 44,000 queries per second on this VM. It is
marginal at a sustained 50,000.** For sustained rates near 50,000 queries per
second, raise `--capacity` further, for example to 131,072. The lab did not
test 131,072.

With the default `--jumbo-capacity 64`, the counted storage and the startup
deadline are:

| `--capacity` | Counted storage | Startup deadline | Lab-tested |
| --- | --- | --- | --- |
| 65,536 | 177,772,880 bytes (169.54 MiB) | 14 seconds | Yes |
| 131,072 | 347,117,904 bytes (331.04 MiB) | 18 seconds | No |

See [Storage](#storage) and
[Startup and shutdown deadlines](#startup-and-shutdown-deadlines).

Use these rules to choose the values:

- ETW delivers events about once per `--etw-flush-seconds`. Each query makes 2
  events. One delivery therefore holds up to about
  `2 × queries/s × --etw-flush-seconds` events. Keep `--capacity` well above
  this size.
- The measured peak queue was far below that size, because the parser drains
  the channel while a delivery arrives. At 44,000 queries per second the rule
  gives 88,000 events, and the peak queue was 30,643. Use the rule as a safe
  upper limit, not as a prediction.
- A longer `--etw-flush-seconds` makes each delivery larger. If you raise it,
  raise `--capacity` by the same factor.
- Larger ETW buffers reduce `etw_lost`. In the lab, 64 KiB × 256 buffers
  removed the `etw_lost` that the previous defaults had at 30,000 queries per
  second and above.

The lab results apply to one VM, an authoritative load only, and 50-second
runs. The lab did not test other machines, recursive or mixed workloads, longer
runs, a TLS receiver, or an `--etw-flush-seconds` value other than 1. See
[ETW flush removal validation](../history/flush-removal-validation.md).

After you change these settings, monitor `ring_dropped` and `etw_lost` under
the real load.

Two earlier lab results do not apply to this build:

- An explicit periodic flush, which the service no longer has, gave 0 ring
  drops at 10,000 and 20,000 queries per second with 64 KiB × 256 buffers. See
  [Removed periodic flush](../design/ring-buffer.md#removed-periodic-flush).
- With 64 KiB × 256 buffers, the previous default 8,192-event channel, and no flush,
  the channel dropped 59 to 79% of events at 10,000 to 20,000 queries per
  second. That result predates the buffer wake. The lab did not repeat it with
  the buffer wake.

**Warning.** The service can log one warning (Event ID 3) per run when the
default buffer settings are in use: the default ETW settings and the default
`--capacity`. It logs it at the end of the first measurement window in which the
received rate is more than 22,000 events per second, which is above the rate
that the defaults were validated for. The window is `--stats-interval-seconds`,
or 60 seconds when snapshots are disabled. The warning does not mean that events
were lost. It tells you to watch `ring_dropped` and `etw_lost`, and to raise
`--capacity` and `--etw-max-buffers` if they increase, for example to
`--etw-buffer-kib 64 --etw-max-buffers 256 --capacity 65536`.

### Settings for 200,000 queries per second

For 200,000 queries per second, use:

```text
--etw-buffer-kib 128 --etw-max-buffers 32 --capacity 24576
```

Each value is a small margin above the largest value that the lab observed at
200,000 queries per second:

| Setting | Largest observed | Recommended |
| --- | --- | --- |
| ETW buffer size (`--etw-buffer-kib`) | 128 KiB, no loss. About 373 events in each buffer. | 128 |
| Maximum ETW buffers (`--etw-max-buffers`) | 29 allocated (3.6 MiB). At most 7 in use at the same time. | 32 (4 MiB) |
| Minimum ETW buffers (`--etw-min-buffers`) | 7 in use of ETW's minimum of 12 (2 per logical processor on the 6-vCPU VM) | `0`, the default. ETW chooses its minimum. |
| Channel (`--capacity`) | Peak queue 21,622 | 24,576 |

The lab ran this load twice for 50 seconds each, on the same VM as the other
results and with a plaintext receiver. The runs used `--etw-max-buffers 256`
and `--capacity 262144`. Both runs had 0 ring drops, 0 `etw_lost`, and 0 lost
ETW buffers. Every encoded frame reached the receiver. The service used about
0.34 of one CPU. The DNS server used about 2.9 CPUs.

Why these values:

- **Maximum buffers.** 32 is 3 buffers above the peak of 29. ETW allocates
  buffers only on demand, so the maximum limits how far ETW can grow. On a host
  with more logical processors, ETW's minimum is higher. ETW raises the maximum
  to at least that minimum.
- **Minimum buffers.** ETW gave one buffer in use per logical processor, plus
  at most one buffer that waited for delivery. Its minimum of 2 per logical
  processor was enough.
- **Channel.** 24,576 is 14% above the peak queue of 21,622. The parser samples
  the queue depth, so the true peak can be slightly higher. The margin covers
  that difference.

Costs:

- Counted storage with `--capacity 24576` and the default 64 jumbo slots is
  71,932,240 bytes (68.60 MiB). The service allocates it at startup. With
  `--capacity 262144`, the counted storage is 654.04 MiB, and the measured
  private bytes were about 684 MiB.
- The startup deadline at this capacity is 12 seconds.
- ETW buffer memory is at most 4 MiB with 32 buffers of 128 KiB.

The lab did not run these exact values. It also did not measure 128 KiB
buffers at 30,000 to 50,000 queries per second. In that range ETW delivered
events in batches in earlier runs. After you apply these settings, monitor
`ring_dropped` and `etw_lost`. If either is not 0, raise `--capacity` or
`--etw-max-buffers`.

**Later lab runs lost events with these values.** The default
`--etw-max-buffers` is now 64, which is above the 32 recommended here. On
2026-11-01 the lab also ran 32 buffers with `--capacity 24576` at 200,000
queries per second. One run had 37,133 ring drops and 351 `etw_lost`. Another
had 3,219 `etw_lost`. See the lab notes. Treat 32 and 24,576 as the earlier
recommendation, not as a setting that is known to lose nothing. The recorded
results above stay as measured.

See [200,000 QPS validation](../history/200k-qps-validation.md).

## Storage

Counted storage at the defaults (`--capacity 24576` and 64 jumbo slots) is
68.60 MiB. The previous default `--capacity 8192` counted 28.22 MiB. With the
high-rate `--capacity 65536` and the default 64 jumbo slots, it is 169.54 MiB.
With `--capacity 131072` and 64 jumbo slots, it is 331.04 MiB. With
`--capacity 262144` and 64 jumbo slots, it is 654.04 MiB. At the
maximum (262,144 events and 256 jumbo slots) it is 678.14 MiB. These are
calculated payload, slot, free-list, and channel totals. They exclude the allocator, native ETW buffers, TDH caches,
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
  per 16,384 channel slots of `--capacity`, rounded up. The default 24,576 gives
  12 seconds, and 262,144 gives 26 seconds. The previous default 8,192 gave
  11 seconds. It covers the identity checks,
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
stderr. The service writes no capture, metrics, or log files. The Windows
performance counters show many of the same values. See
[Performance counters](#performance-counters).

### Log event IDs

| Event ID | Type | Written for |
| --- | --- | --- |
| 1 | Error | A fatal service failure, whether at startup or during the run. The text begins `EtwDnstap failed:`. |
| 2 | Information | Every information-level line: `capture started`, `capture stats`, `output stats`, `capture stopped`, `output stopped`, `dnstap receiver reconnected`, `ETW buffers applied`, and `diagnostic settings applied` in diagnostic builds. |
| 3 | Warning | Every warning-level line, such as an unavailable receiver, a lost connection, a failed reconnect, a rejected event, the rate warning, and a reclaimed session. |
| 4 | Error | An error-level line that does not by itself stop the service, such as the reason for a DNS Server change exit. |
| 5 | Error | Watchdog expiry, written just before the process aborts. |

### Periodic and final reports

Every `--stats-interval-seconds`, the service writes two information events,
`capture stats` and `output stats`. Each snapshot first asks ETW for its own
loss counters. The shutdown always writes `capture stopped` and `output stopped`
with the same fields, even when periodic snapshots are disabled.

`capture started` records the session name, the session GUID (`session_guid`), receiver, `tls`
(`true` unless `--plaintext` is given), `tls_server_name` when set, channel capacity, ETW
buffer size, the requested minimum and maximum buffers (`etw_min_buffers` and
`etw_max_buffers`), flush seconds, the keyword masks
as `match_any_keyword` and `match_all_keyword` in hexadecimal, and
`keyword_fallback`.

`ETW buffers applied` follows `capture started` once. It records the minimum and
maximum buffer counts and the allocated buffer count that ETW reported after the
session started (`etw_min_buffers_applied`, `etw_max_buffers_applied`, and
`etw_buffers_allocated`). The service writes it only when the ETW query
succeeds. See [ETW buffer minimums](#etw-buffer-minimums).

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
| `etw_stats_errors` | Failed ETW statistics queries. While the performance counters run, the service also queries once per second, and those failures count here too. |
| `stale_sessions_reclaimed` | Leaked sessions stopped at startup. |
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

## Performance counters

The service publishes a Windows performance counter set named `EtwDnstap`. The
lab validated it on 2026-11-01 (see below). The
manifest is [`packaging/perfcounters/etw-dnstap.man`](../../packaging/perfcounters/etw-dnstap.man).
The installed copy is `C:\Program Files\EtwDnstap\etw-dnstap.man`.
[Install and provision](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute)
registers it.

You can read the counters in Performance Monitor, with `typeperf`, or with
`logman` data collector sets. WMI (Windows Management Instrumentation) reads them
through the `Win32_PerfRawData_*` classes. See [Reading the counters](#reading-the-counters).

### Instance and timing

The counter set has one instance. The instance name is the ETW session name, so
the default instance is `EtwDnstap`. A different `--session-name` changes the
instance name. ETW session names are unique on a host, so the name stays the same
after each restart. Counter paths use the form `\EtwDnstap(<instance>)\<counter>`,
so a session name with `(`, `)`, `#`, `/`, or `\` makes the instance hard to
address. Use a session name without those characters.

The instance exists only while capture runs. Counters start from zero when the
service starts. A restart resets them.

The rates and totals (IDs 1 to 3, 10 to 21, 32, and 51) are read by reference.
Windows reads the live value each time a consumer collects, so a rate is exact
for the consumer's own sample interval. The service publishes the other gauges
(IDs 30, 31, 33, 34, 40 to 44, and 50) once per second.

The ETW gauges come from an ETW statistics query. The service runs that query
once per second, which is more often than `--stats-interval-seconds`. The ETW
gauges read 0 until the first query succeeds. After that, each ETW gauge keeps
the value of the last successful query.

The counters add no work to the ETW callback or the parser loop. Their CPU cost
has not been measured.

### Catalog

Each counter has a numeric ID from the manifest. The log counter column names
the matching field in [Log events and counters](#log-events-and-counters). None
means that the log has no matching field.

#### Throughput rates

| Counter | ID | Meaning | Log counter |
| --- | --- | --- | --- |
| Events Received/sec | 1 | DNS events accepted from ETW each second. | `received` |
| Frames Sent/sec | 2 | Frames encoded and handed to the sink each second. This is not a delivery count. | `encoded` |
| Events Dropped/sec | 3 | Events dropped at admission each second. | `intake_dropped` |

The `/sec` counters are rates. The reader computes each rate from the change in
the counter over time.

#### Totals since the service started

| Counter | ID | Meaning | Log counter |
| --- | --- | --- | --- |
| Events Received | 10 | DNS events accepted from ETW since the service started. | `received` |
| Frames Sent | 11 | Frames encoded and handed to the sink since the service started. It includes frames dropped while the receiver is down (Receiver Down Drops), so it is not a delivery count. | `encoded` |
| Events Dropped | 12 | Events dropped at admission. It includes IDs 13, 14, and 15. | `intake_dropped` |
| Channel Full Drops | 13 | Events dropped because the channel was full. The newest event is dropped. | `ring_dropped` |
| Pool Exhausted Drops | 14 | Events dropped because no pool slot was free, regular and jumbo together. | `pool_exhausted_regular` + `pool_exhausted_jumbo` |
| Jumbo Pool Exhausted Drops | 15 | Jumbo-class events dropped because no jumbo slot was free, or because `--jumbo-capacity` is 0. | `pool_exhausted_jumbo` |
| ETW Events Lost | 16 | Events that ETW reports as lost. | `etw_lost` |
| ETW Buffers Lost | 17 | ETW buffers lost. This is a buffer count, not an event count. | `etw_buffers_lost` |
| Receiver Down Drops | 18 | Frames discarded while the receiver was down. | `disconnected_dropped` |
| Events Rejected | 19 | Dequeued events that the parser rejected. | `rejected` |
| Callback Errors | 20 | Events that the ETW callback could not copy or hand off. | `callback_errors` |
| Receiver Disconnects | 21 | Times the receiver connection was lost since the service started. | `receiver_disconnects` |

How the totals relate:

- Events Dropped (12) includes Channel Full Drops (13), Pool Exhausted Drops (14),
  and Jumbo Pool Exhausted Drops (15). It also includes drops when the channel's
  parser end is gone. Those drops have no counter of their own.
- ETW Events Lost (16) and ETW Buffers Lost (17) are separate from Events Dropped.
  They are ETW's own counts.
- Receiver Down Drops (18) is separate from Events Dropped. It also counts frames
  written since the last successful flush when a session failed. Those frames may
  have reached the receiver.
- To count every loss that the service can count, add 12, 16, and 18.

#### Gauges

| Counter | ID | Meaning | Log counter |
| --- | --- | --- | --- |
| Channel Depth | 30 | Events in the channel that the parser has not taken yet. The service derives this value from the other counts. | None |
| Channel Capacity | 31 | The `--capacity` value. The `capture started` line also shows it. | None |
| Channel Peak Depth | 32 | The deepest the channel was seen. The parser samples the depth, so this is a lower bound. | `queue_depth_max` |
| % Channel Used | 33 | Channel Depth as a percentage of Channel Capacity. | None |
| (base of 33) | 34 | Not displayed. It is the base for % Channel Used. | None |
| ETW Buffers Allocated | 40 | ETW buffers allocated for the session. | None |
| ETW Buffers In Use | 41 | Allocated buffers that ETW does not report as free. It never goes below 0. | None |
| ETW Buffers Maximum | 42 | The maximum buffer count that ETW reports. It can be higher than `--etw-max-buffers`. See [ETW buffer minimums](#etw-buffer-minimums). | None |
| % ETW Buffers Used | 43 | Buffers in use as a percentage of the maximum. | None |
| (base of 43) | 44 | Not displayed. It is the base for % ETW Buffers Used. | None |
| Receiver Connected | 50 | 1 while a Frame Streams session is established. Otherwise 0. | `receiver_connected` |
| Receiver Reconnects | 51 | Successful reconnects after the first session. | `receiver_reconnects` |

The ETW gauges come from the last successful query. They read 0 until the first
query succeeds.

### Reading the counters

In Performance Monitor, add the counters from the **EtwDnstap** object. Choose
the instance that matches the session name.

```powershell
typeperf -q EtwDnstap
typeperf "\EtwDnstap(*)\*" -sc 5
```

`typeperf -q` lists the counter names. The second command prints five samples
of every counter in every instance.

Counters reset when the service restarts.

### If the counters do not register

An unregistered manifest causes no error in the service. Windows accepts the
provider calls, but consumers cannot find the counter set: `typeperf -q
EtwDnstap` lists nothing, and Performance Monitor has no **EtwDnstap** object. To
fix it, register the manifest with `lodctr` as in
[install.md](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute),
then restart the service.

If a provider call fails at startup, the service writes one warning (Event ID 3)
that names the call and the Win32 status. Capture continues without counters.

### Lab validation

The lab validated the counters on 2026-11-01 on the 6-vCPU Windows Server 2025
VM. The service ran as LocalService with a restricted service SID.

- `lodctr` registered the manifest. An administrator's `typeperf -q EtwDnstap`
  listed every counter by name.
- In a 60-second run at 10,000 queries per second, the final totals matched the
  `capture stopped` event: Events Received 1,200,000, Channel Full Drops 10,751,
  and Channel Peak Depth 8,193. Events Received/sec read 19,700 to 21,000 in
  steady seconds, which is 2 events for each query.
- The instance disappeared when the service stopped.

Not tested: a non-admin consumer, remote collection, WMI, and the CPU cost of the
publication alone.

### Suggested alerts

These alerts are suggestions. Their thresholds have not been tested on a host.

| Alert | Why |
| --- | --- |
| `% Channel Used` above 75 | In the 200,000 queries-per-second runs, ring drops followed the channel reaching `--capacity`. |
| `% ETW Buffers Used` above 75 | In those runs, `etw_lost` followed ETW buffers in use reaching the maximum. |
| Any increase in `Events Dropped`, `ETW Events Lost`, or `Receiver Down Drops` | These are the loss totals. Under normal load, each one should stay at 0. |
| `Receiver Connected` at 0 | The receiver session is down. Frames discarded while it is 0 count in `Receiver Down Drops`. |

## Related

- [Install and provision](install.md)
- [Troubleshooting](troubleshooting.md)
- [Measurement terms](../design/measurement-terms.md)
