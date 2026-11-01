# Configuration reference

> **Status:** Current · **Describes:** `main` at commit `9172f2d` (defaults from `src/pipeline.rs`, `CaptureArgs`) · **Reviewed:** 2026-11-01

Every option is set in the service binary path, not in `sc.exe start`. Install
and provisioning steps are in [install.md](install.md). Symptoms are in
[troubleshooting.md](troubleshooting.md).

## Options

Defaults and ranges come from `CaptureArgs` in `src/pipeline.rs`. Values outside
the range are rejected at startup.

| Option | Default | Range or format | Meaning |
| --- | --- | --- | --- |
| `--receiver IP:PORT` | none (required) | Numeric unicast IP and nonzero port. IPv6 as `[addr]:port`. | Plaintext Frame Streams TCP receiver. Hostnames are rejected. |
| `--capacity N` | `8192` | 1,024 to 262,144 events | Event channel capacity. A full channel drops the newest event and increments `ring_dropped`. |
| `--jumbo-capacity N` | `64` | 0 to 256 | Pool slots for records with a padded payload over 2 KiB. `0` disables jumbo capture. |
| `--duration SECONDS` | none | 1 to 604,800 | Stop after this many seconds. Without it, capture runs until SCM stops the service or a failure occurs. |
| `--session-name NAME` | `EtwDnstap` | 1 to 200 UTF-16 units, no NUL | Name of the real-time ETW session. A leaked session with the same name and shape is reclaimed at startup. |
| `--identity TEXT` | `etw-dnstap` | At most 1,024 UTF-8 bytes | dnstap producer identity. Empty text omits the field. |
| `--etw-buffer-kib KIB` | `16` | 4 to 128, a multiple of 4 | ETW session buffer size. |
| `--etw-max-buffers N` | `12` | 4 to 4,096 | Requested maximum ETW buffer count. See [ETW buffer minimums](#etw-buffer-minimums). |
| `--etw-flush-seconds SECONDS` | `1` | 1 to 60 | ETW's own real-time flush interval. It is the delivery bound when `--etw-flush-ms` is `0`. |
| `--etw-flush-ms MS` | `0` (off) | `0`, or 20 to 1,000 | How often the service asks ETW to flush the session. See [Low-rate defaults](#low-rate-defaults). |
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

**The defaults are tuned for about 1,000 queries per second or less.** They have
not been tested above 1,000 queries per second with the 16 KiB by 12 buffer
settings.

With the periodic flush off (`--etw-flush-ms 0`), ETW delivers real-time events
in batches about `--etw-flush-seconds` (1 second by default) apart. Delivery
latency is therefore up to about 2 seconds, and usually under 1 second. A batch
that holds more events than the channel can take drops the excess. The
README's estimate for where the 8,192-event default channel starts dropping is
about 2,000 queries per second. That figure is inferred, not measured. A
separate arithmetic check in the 2026-11-01 review gives about 4,000. Neither
has been tested.

Lab evidence (6-vCPU VM, Windows Server 2025, authoritative workload, default
8,192-event channel) for the flush-off configuration with 64 KiB buffers and 256
maximum buffers:

- 59 to 79% of events were dropped at 10,000 to 20,000 queries per second.

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
| 50,000 | 100 ms | 0.04 to 0.91% loss; `etw_lost` was 0 |

The 50,000 queries-per-second row needs care. The same source describes loss of
0.3 to 1.7% at 50,000 queries per second **without** the periodic flush. The
flush setting for that comparison run is not recorded in the same document, so
treat the two figures as separate observations. The residual loss tracks host
disturbance according to that source.

Not tested: other machines and vCPU counts, 25,000 to 45,000 queries per
second, recursive or mixed workloads, and a 50 ms interval at 50,000 queries per
second and above in a release build.

Each delivered batch holds up to about twice the flush interval's worth of
events (2 events per query). At higher rates, lower `--etw-flush-ms` or raise
`--capacity` so that `2 × rate × interval` stays well below the channel
capacity. At 20,000 queries per second, the largest batch was about 6,800
events with a 100 ms interval and about 3,500 with 50 ms.

The evidence is in [ring-buffer.md](../design/ring-buffer.md) and
[singlepass-four-core-test-results.md](../history/singlepass-four-core-test-results.md).
The per-stage lab labels from the source reports are not defined elsewhere and
are not used here.

### Flush cost

With `--etw-flush-ms` enabled, the lab run wrote about 5 to 16% more ETW buffers
and cost under 0.1% of a core when idle. The `capture stopped` line reports
`etw_flushes`, `etw_flush_errors`, and `etw_flush_max_us` (the longest flush call,
including scheduling delay on the supervisor). Lab maxima under load were 11 to
46 ms, near multiples of the 15.6 ms timer tick. The idle maximum was about
0.35 ms.

A failed flush logs one warning and is counted. Capture continues.

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
- Event IDs 256 and 257 are also filtered by ETW before events reach session
  buffers, which reduces buffer use and callback dispatch. This does not reduce
  the DNS Server's cost of generating events that pass the keyword mask. Events
  284, 287, 597, and 598 share the narrowed keyword bits on Windows Server 2025
  build 26100, so the event-ID check stays in the callback as defense in depth.

## Message and transport limits

- **Frame Streams content type:** `protobuf:dnstap.Dnstap`, with protobuf data
  frames. Control payloads up to 512 bytes and content-type fields up to 256
  bytes are accepted.
- **Deadlines:** connection establishment has a 2-second timeout. The whole
  handshake, each buffered write or flush, and STOP/FINISH each have a 2-second
  deadline shared across partial socket calls. A peer cannot extend a deadline
  by trickling bytes.
- **Flush:** the parser flushes buffered TCP output at least once per second
  while active.
- **Failure:** an I/O failure ends capture. There is no reconnect and no resend.
  A poisoned writer does not append a STOP after a partial frame.
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

## Session reclaim

On startup, a leaked session with the same name and the program's shape
(real-time only, no log file, the configured buffer size) is stopped and
replaced. Any other session with that name is left alone, and startup fails.
The buffer-size match is part of the shape. Restarting with a different
`--etw-buffer-kib` does not reclaim a session leaked under the old size. Stop it
manually with `logman stop NAME -ets`.

Sessions named `etw-dnstap-<pid>` from earlier builds are not reclaimed.
Clean them up once with `logman query -ets`, then `logman stop etw-dnstap-<pid> -ets`.

## Related

- [Install and provision](install.md)
- [Troubleshooting](troubleshooting.md)
- [Measurement terms](../design/measurement-terms.md)
