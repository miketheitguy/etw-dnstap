# Log events and counters

> **Status:** Current · **Describes:** the code at commit `13972d8` · **Reviewed:** 2026-11-04

The service writes diagnostics to the Application event log under source `EtwDnstap`.
It writes no stdout, stderr, capture, or metrics files, except the spool, which stores
unsent frames in segment files. See [Spool](configuration.md#spool). The
[performance counters](#performance-counters) show many of the same values.

## Log event IDs

| Event ID | Type | Written for |
| --- | --- | --- |
| 1 | Error | A fatal failure at startup or during the run. Text starts with `EtwDnstap failed:`. |
| 2 | Information | `capture started`, `capture stats`, `output stats`, `capture stopped`, `output stopped`, `dnstap receiver reconnected`, `dnstap link is no longer saturated`, `ETW buffers applied`, `spool in use`, `dnstap spool backlog kept on disk for the next run`, and, in diagnostic builds, `diagnostic settings applied` and `parser timing`. |
| 3 | Warning | Warnings, such as a lost connection, a rejected event, or `dnstap link is saturated`. Spool messages are in [Spool messages](#spool-messages). |
| 4 | Error | Errors, such as `ETW cleanup failed`. A fatal failure uses Event ID 1. |
| 5 and 6 | Error | Watchdog expiry (5) or a panic (6), written just before the process aborts. A panic text starts with `EtwDnstap panicked:` and gives the message and location. |

The service logs information level and above, with no debug events. Event text is cut
at 8,192 UTF-16 code units. Capture never stops because an event cannot be written.

## Periodic and final reports

Every `--stats-interval-seconds`, the service writes `capture stats` and `output stats`.
Each snapshot first asks ETW for its loss counters. Shutdown always writes
`capture stopped` and `output stopped`, even when periodic snapshots are off.

`capture started` records:

- `version`, from `Cargo.toml`. `(Get-Item <exe>).VersionInfo` shows it as `ProductVersion`,
  and `FileVersion` is `X.Y.Z`.
- `session`, `session_guid`, `receiver`, `tls` (`true` unless `--plaintext`), and
  `tls_server_name` when set.
- `capacity`, `etw_buffer_kib`, `etw_flush_seconds`, and the requested
  `etw_min_buffers` and `etw_max_buffers`.
- `receiver_stall_seconds`: how long the receiver may accept no bytes, while the service
  has data to send, before the service ends the session. See
  [`--receiver-stall-seconds`](configuration.md#options).
- `match_any_keyword` and `match_all_keyword` (hexadecimal), `keyword_fallback`, and
  `failed_responses_captured`.

`ETW buffers applied` follows `capture started` once, if the ETW query succeeds. It gives
`etw_min_buffers_applied`, `etw_max_buffers_applied`, and `etw_buffers_allocated`. See
[ETW buffer minimums](tuning.md#etw-buffer-minimums).

### Spool messages

None of these lines logs frame data. A repeated message logs its first four occurrences,
then every 60th (the 61st, 121st, and so on), unless its row says otherwise. Causes and
actions are in [Spool warnings](troubleshooting.md#spool-warnings).

| Message | Event ID | Fields | When |
| --- | --- | --- | --- |
| `spool in use` | 2 | `spool_dir`, `spool_max_mib`, `spool_min_free_mib`, `leftover_segments`, `leftover_bytes` | Startup. Leftover fields show what an earlier run left. |
| `spool folder is not usable; capture continues without a spool, so frames are dropped and counted while the receiver is down` | 3 | `spool_dir`, `reason` | Startup, when the folder check fails. |
| `spool could not be opened; capture continues without a spool, so frames are dropped and counted while the receiver is down` | 3 | `spool_dir`, `error` | Startup, when the folder passes the check but the spool does not open. |
| `dnstap spool deleted its oldest segment unsent` | 3 | `frames`, `bytes` | The size cap or free-space floor removed a segment that held unsent data. |
| `dnstap spool discarded unreadable segment data` | 3 | `frames`, `bytes` | Segment data could not be read. |
| `dnstap spool I/O error` | 3 | `error` | A spool file operation failed. The spool pauses for 10 seconds. |
| `dnstap spool is refusing blocks: the volume is below the free-space floor and no older segment is left to delete` | 3 | `free_mib`, `needed_mib`, `pause_s` | The volume is below `--spool-min-free-mib` plus one segment, and only the newest segment is left. Refused blocks count in `spool_dropped`. `free_mib` rounds down, and `needed_mib` rounds up. |
| `could not seal the dnstap spool segment` | 3 | `error` | Shutdown, when the active segment cannot take its STOP. |
| `dnstap spool backlog kept on disk for the next run` | 2 | none | Shutdown, when unsent frames remain in the spool. |

### Receiver, schema, and rate messages

None of these lines logs frame data. Unless a row says otherwise, the repeat rule in
[Spool messages](#spool-messages) applies. Reconnect logging rules are in
[Receiver connection](configuration.md#receiver-connection).

| Message | Event ID | Fields | When |
| --- | --- | --- | --- |
| `dnstap reconnect failed` | 3 | `receiver`, `error`, `cause`, `failures` | A reconnect attempt failed. `cause` is `connect`, `timeout`, `tls`, or `protocol` (Frame Streams handshake). `failures` counts the run's failed attempts. |
| `dnstap receiver failed the TLS handshake before the first session; check that the Windows trust store trusts the receiver certificate, that --tls-server-name (or the receiver IP address when it is not set) matches the certificate, and that the receiver does not require a client certificate (this client sends none); retrying with backoff` | 4 | `receiver`, `error`, `cause`, `failures` | Once per run, after a startup outage ended in a TLS rejection. Replaces `dnstap reconnect failed` for that attempt. |
| `dnstap receiver refused the Frame Streams handshake before the first session; check that the receiver accepts the dnstap content type; retrying with backoff` | 4 | `receiver`, `error`, `cause`, `failures` | Once per run. The same situation as the previous line, for a Frame Streams rejection. |
| `could not start the dnstap reconnect thread; retrying with backoff` | 3 | `receiver`, `error`, `failures` | The thread for a reconnect attempt did not start. |
| `dnstap reconnect thread ended without a result; retrying with backoff` | 4 | `receiver`, `failures` | The reconnect thread ended without a result. |
| `dnstap link is saturated: the spool backlog has grown steadily over the last 10 seconds while the receiver is connected, ...` | 3 | `backlog_bytes`, `oldest_age_ms`, `bytes_sent_per_second` | The spool backlog trended up over 10 samples while the receiver was connected ([rule](#the-saturated-state)): the receiver or the network path takes fewer bytes than the service captures. `bytes_sent_per_second` is the average over those samples. Logged once for each change of state, and not repeated while it holds. See [The receiver is slow](troubleshooting.md#the-receiver-is-slow). |
| `dnstap link is no longer saturated` | 2 | `backlog_bytes`, `oldest_age_ms`, `bytes_sent_per_second`, `reason` | The state ended. `reason` is `the backlog is empty` or `the backlog stopped growing` (not larger than 10 samples ago). |
| `DNS event schema load failed; its events are rejected until the load is retried` | 3 | `event_id`, `version`, `failures`, `retry_after_events` (65,536) | Windows could not return the event schema (a Win32 error from `TdhGetEventInformation` follows the message). Logged per (event ID, version) key. |
| `DNS event schema is unsupported and its events are rejected` | 3 | `event_id`, `version` | The metadata loaded, but this program cannot decode it. Logged once per key and not retried. It can repeat after the rejected-key list (8 entries) is cleared, or when a retry fails as unsupported. |
| `rejected ETW event; the first four and then every 60th are logged, all are counted` | 3 | `error`, `rejected` | The parser rejected an event. `rejected` is the running count. |
| `received rate is above 22,000 events/s; the default buffer settings were validated at about 20,000 events/s; if ring_dropped or etw_lost increase, raise --capacity and --etw-max-buffers, for example --etw-buffer-kib 64 --etw-max-buffers 256 --capacity 65536` | 3 | `events_per_second` | Once per run, with the default buffer settings, at the end of the first measurement window above 22,000 events per second. See [Rate warning](troubleshooting.md#rate-warning). |

## Capture counters

These fields appear on `capture stats` and `capture stopped`.

| Counter | Meaning |
| --- | --- |
| `received` | DNS records (event IDs 256, 257, and 258) accepted after the process check. Includes later drops and rejects. |
| `dequeued` | Records the parser took from the channel. |
| `encoded` | Records handed to the sink. Not a delivery count: includes frames the sink later discarded while disconnected. |
| `ignored` | Dequeued records that are not a supported DNS event. |
| `rejected` | Dequeued records rejected as malformed or invalid. Includes `schema_rejected`. |
| `intake_dropped` | Records lost at admission: `ring_dropped`, `pool_exhausted_regular`, `pool_exhausted_jumbo`, and `receiver_disconnected`. |
| `ring_dropped` | Dropped because the channel was full. Part of `intake_dropped`. |
| `pool_exhausted_regular`, `pool_exhausted_jumbo` | Regular: no free regular slot. Jumbo (payload over 2 KiB): no free jumbo slot, or `--jumbo-capacity` is `0`. Both are part of `intake_dropped`. |
| `foreign_pid_rejected` | From a process other than the DNS Server. Not in `received`. Normally 0. See [DNS process filter](configuration.md#dns-process-filter). |
| `receiver_disconnected` | Parser end of the channel gone. This is the event channel, not the TCP receiver. |
| `etw_lost` | Events ETW reports lost, as of its last successful query or stop. |
| `etw_buffers_lost` | ETW buffers lost. Not an event count. |
| `callback_errors` | Records the callback could not copy or hand off. Not in `intake_dropped`. |
| `schema_rejected` | Records whose schema failed to load or to compile. Part of `rejected`. |
| `etw_stats_valid`, `etw_stats_errors` | `etw_stats_valid` is `true` when the last ETW query or stop succeeded. `etw_stats_errors` counts failed queries, including the once-per-second queries while performance counters run. |
| `stale_sessions_reclaimed` | Leaked sessions stopped at startup. |
| `etw_buffers_delivered` | ETW buffer callbacks completed. |
| `parser_wake_requests` | Buffers that enqueued an event. Each sends one wake request, which costs nothing while the parser runs. Not a count of parser wakeups. |

## Output counters

These fields appear on `output stats` and `output stopped`.

| Counter | Meaning |
| --- | --- |
| `output_failed_events` | Dequeued records whose frame could not be queued after the sender stopped. Otherwise 0. |
| `output_errors` | Output failures: one per `output_failed_events`, or a failed queue flush. A STOP/FINISH exchange that does not complete at a requested stop is not an output error: it does not fail the stop, and the service writes nothing about it. |
| `receiver_connected` | Flag: `true` while a Frame Streams session exists. |
| `receiver_disconnects` | Established sessions lost: a socket error, or no bytes accepted for `--receiver-stall-seconds` while data waited. A receiver that is only slow does not end the session. |
| `receiver_reconnects` | Successful reconnects after the first session. |
| `receiver_reconnect_failures` | Failed reconnect attempts at connect, handshake, or protocol. Also counts a reconnect thread that did not start or ended without a result. |
| `disconnected_dropped` | Frames lost in the sink. With no spool: the live frames that the link did not hand to the socket in full when a connection failed (the cut frame and every later frame of the block in hand, and the frames in the link queue), and every live frame that arrives while the receiver is down. With a spool: only the one live block that comes back from the link at a shutdown that follows a spill, because it is older than the spool backlog and cannot go in front of it. A failed connection with a spool counts nothing here: its unsent live frames go back to the spool, and its unsent replay frames stay in it. This is not an upper bound on loss. Bytes that the kernel accepted before a failure are not counted, even if the connection then loses them. A counted frame did not reach the receiver whole, except in the TLS 1.3 key update window, where the receiver may hold it. Not in `intake_dropped`. |
| `slow_writes`, `write_max_us` | The link thread never blocks in a send. It writes without waiting, and when the socket takes no more it waits for the socket to become writable. For each block (about 64 KiB, or one replay batch) it adds up those waits. `slow_writes` counts blocks whose wait reached 5 ms, and `write_max_us` is the longest wait of one block, in microseconds. Both count since the service started. A wait that goes on is added to `write_max_us` while it lasts. |
| `write_max_recent_us` | Gauge: the longest wait of one block for the socket to become writable, in microseconds, in the last 60 seconds. Use it, not `write_max_us`, to see whether the receiver is slow now. See [Windows of the recent peaks](#windows-of-the-recent-peaks). |
| `bytes_sent` | Bytes that the socket of the receiver connection accepted, live and replayed. With plaintext these are the framed bytes (4-byte length and payload). With TLS they are the bytes written to the socket, so they include the record headers and tags (about 22 to 29 bytes for each 16 KiB). Includes bytes that the kernel still held when the connection failed. Excludes control frames. Take the rate between two lines and compare it with the capture rate. |
| `queue_depth_max` | Deepest event channel depth the parser sampled. A lower bound. The parser samples after each idle poll and every 256th event. This is the channel before the parser, not the send queue. |
| `send_queue_depth_max` | Gauge: the most blocks (about 64 KiB each) waiting in the send queue between the parser and the router in the last 60 seconds, out of 128. The router samples it each time it takes a block. A lower bound. The router never waits on the socket, so a deep queue means the spool (disk) is slow, not the receiver. The router sends blocks to the link queue, which holds 128 blocks (about 8 MiB) while the link is behind, and spills the whole link queue to the spool only when it is full. |
| `spool_oldest_age_ms` | Gauge: milliseconds since the oldest unsent spool segment was created, 0 with no backlog. It is an upper bound on the age of the oldest unsent frame, because a segment is older than any frame in it. For a segment from an earlier run it uses the file creation time. |
| `link_saturated` | Flag: `true` while the spool backlog trends up with the receiver connected (larger than 10 samples ago, with at least 7 of the 10 steps rising). It clears when the backlog is empty or no longer larger than 10 samples ago. Holds its value while the receiver is down. Always `false` with no spool. See [The saturated state](#the-saturated-state). |
| `queue_dropped` | Frames discarded because the send queue was full. Also in `encoded`. |
| `spooled` | Frames stored for later sending. |
| `replayed` | Frames from the spool that the receiver connection fully accepted, including earlier-run leftovers. A frame counts once, when the connection takes it, not when the service reads it from disk. A frame that a failed connection did not take is read again and counts when it is sent. The count is of bytes handed to the socket, not of frames the receiver read: a connection that fails before the kernel sends accepted bytes loses them without a count. |
| `spool_dropped`, `spool_dropped_bytes` | Frames (`spool_dropped`) or bytes (`spool_dropped_bytes`, control frames included) removed, skipped, or refused unsent. Causes: eviction, corrupt or cut-off data, a pause, or an I/O error. Leftover segments count only bytes, because their frame count is unknown. |
| `spool_errors` | Every spool I/O error. |
| `spool_bytes`, `spool_segments` | Gauges: bytes and segment files now, including files whose removal has not yet succeeded. |
| `spool_available` | Flag: `true` while the spool accepts data. `false` with `--no-spool`, with an unusable folder, or while an I/O error or the free-space floor pauses it. |

### Windows of the recent peaks

`write_max_recent_us` and `send_queue_depth_max` are peaks over a window, not totals. The
router thread keeps one bucket for each second of the last 60. Every second it publishes
the largest value in the buckets and starts a new bucket. The window is the same for
every reader, and no reader resets it: the stats log and the performance counters load
the same published value. `--stats-interval-seconds` does not change it. With the
default of 60 seconds, each line shows the peak since the previous one. With a longer
interval, a peak older than a minute is not in the line. A value appears up to a second
after the event.

The window is 60 samples, not 60 seconds. Samples are at least 1 second apart, and a
disk append on the router thread can delay the next sample, so the window can span more
than a minute when the disk is slow.

`write_max_us` and `slow_writes` count since the service started, and they measure the
link thread's wait for the socket to become writable, one block at a time (see above).
Before the link thread existed they measured one blocking socket write.

### The saturated state

The router thread samples the spool on its own clock, at least 1 second apart. It keeps
the last 11 samples of the unread backlog: the spool bytes not yet handed to the link. The
unread backlog falls as the replay reads, unlike `spool_bytes`, which falls only when a
whole segment is deleted. The trend spans 10 samples, so it is at least 10 seconds.

- **Set.** The backlog is larger than 10 samples ago, and at least 7 of the 10 steps
  between samples rose, with the receiver connected. The service logs
  `dnstap link is saturated` (Event ID 3).
- **Clear.** The backlog is empty (at once), or it is no longer larger than 10 samples ago.
  The service logs `dnstap link is no longer saturated` (Event ID 2).
- **Held.** While the receiver is down, while an I/O error pauses spool reads, and after
  a session was lost since the last sample, the 11 samples start over and the state keeps
  its value. Growth during an outage is not a saturated link. Read `link_saturated` with
  `receiver_connected`.
- **Silent outage.** A receiver that stops reading without closing the connection looks
  like a saturated link until the stall timeout ends the session. With the default 15
  seconds, the flag can set first: the lab saw it set 11.4 to 12 seconds into such an
  outage ([lab validation](../history/slow-receiver-validation.md)). Once the session
  ends, `receiver_connected` is 0 and the flag holds.

The rule is a trend, not a run of rises, because the replay reads in 64 KiB batches: with
a capture of 5.5 MB/s and a link of 5.3 MB/s about one sample in five is flat, and a run
would never complete while the spool fills to the cap. Seven of ten allows three flat
steps and rejects one burst. A backlog that drains after an outage is not larger than 10
samples ago, whatever its steps do. A change is not undone within 10 samples, so a state
that flaps logs at most one event for each 10 samples. The state only reports. It does not
change routing. Without a spool it is always `false`: watch `send_queue_depth_max` and
`queue_dropped` instead.

## Counter relationships

```text
intake_dropped = ring_dropped + pool_exhausted_regular + pool_exhausted_jumbo
                 + receiver_disconnected
received       = intake_dropped + callback_errors + dequeued + queue backlog
dequeued       = encoded + ignored + rejected + output_failed_events
```

- `ring_dropped` and the two `pool_exhausted_*` fields are part of `intake_dropped`.
  Do not add them to it. No pool-exhausted total is logged.
- `disconnected_dropped`, `queue_dropped`, and `spool_dropped` are the sink's loss
  counters. Each frame is in at most one of them. `spooled`, `replayed`, and
  `spool_dropped_bytes` are not loss counters. `disconnected_dropped` is outside the
  identities, because its frames already count in `encoded`.
- Each frame that reaches the sink is handed to the socket, counted in `queue_dropped`,
  `disconnected_dropped`, or `spool_dropped`, or still held by the service. A stored frame
  counts in `spooled`, and in `replayed` when the receiver connection accepts it, or in
  `spool_dropped` if the spool removes it first. A frame still on disk at shutdown counts in
  `spooled` and in no loss counter. `spool_bytes` and `spool_segments` show what remains, and
  `spool_bytes` includes the frames that the service has read from a segment and the connection
  has not yet taken.
- Frames in the send queue when the sender stops abnormally count in none of these. The
  parser counts only the failed write, in `output_failed_events`.
- Leftover frames count in `replayed` when the connection takes them, never in `encoded`. A leftover segment
  removed unsent adds only to `spool_dropped_bytes`. A cut-off final frame adds one to
  `spool_dropped`.
- `bytes_sent` counts what the socket accepted (with TLS, the record bytes), not what the receiver read. No counter reports
  the capture rate in bytes. Over a few minutes it is the `bytes_sent` rate, plus the growth
  of `spool_bytes`, plus the `spool_dropped_bytes` rate (bytes removed at the cap). In a steady
  state with no backlog it equals the `bytes_sent` rate. A rising backlog means a saturated
  link, which `link_saturated` reports after at least 10 seconds. The data that the link
  takes now is at most `spool_oldest_age_ms` old, and that age grows while the link stays
  saturated.
- No counter gives the number of frames the receiver accepted. Only the collector's own
  count checks delivery.
- `etw_lost` and `etw_buffers_lost` are ETW's counts, in ETW's units. They are outside
  these identities.
- Live samples read atomic counters and can disagree briefly. The final report, written
  after capture stops and the parser joins, has no backlog, so the identities close there.

## Performance counters

The service publishes the Windows performance counter set `EtwDnstap`. The lab tested it
on 2026-11-01 (see [Lab validation](#lab-validation)).

- Manifest: [`packaging/perfcounters/etw-dnstap.man`](../../packaging/perfcounters/etw-dnstap.man).
  The provider and the counter set are both named `EtwDnstap`.
- Provider GUID: `{A7F0B1F4-C87D-4267-979F-594A4AB2145D}`.
- Counter set GUID: `{B02EDFA2-B961-4E06-89D2-56C95ECB336C}`.
- Installed copy: `C:\Program Files\EtwDnstap\etw-dnstap.man`. The MSI registers it with
  `lodctr`. In a hand-provisioned install,
  [Install and provision](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute)
  registers it.

Counter names and help text are resources in `etw-dnstap.exe`. Read the counters with
Performance Monitor, `typeperf`, `logman` data collector sets, or WMI (`Win32_PerfRawData_*`
classes). See [Reading the counters](#reading-the-counters).

### Instance and timing

The counter set has one instance, named after the ETW session (default `EtwDnstap`).
Counter paths use the form `\EtwDnstap(<instance>)\<counter>`. ETW session names are unique
on a host, so the name is the same after each restart. `--session-name` rules are in
[Options](configuration.md#options). The instance exists only while capture runs.

Windows reads the rates, totals, and gauges that are plain atomics (IDs 1 to 3, 10 to 21, 32,
51, 52 to 59, and 61 to 65) by reference. Each consumer therefore gets an exact rate for its own sample interval. The service
publishes the other gauges (IDs 30, 31, 33, 34, 40 to 44, 50, 60, and 66) once per second.
It runs the ETW statistics query at the same rate. ETW gauges read 0 until the first
successful query, then keep the last successful value.

The counters add no work to the ETW callback or the parser loop. Their CPU cost is not
measured.

### Catalog

Each counter has a numeric ID from the manifest. The log counter column names the matching
field in [Capture counters](#capture-counters) or [Output counters](#output-counters).
`None` means the log has no matching field.

#### Throughput rates

| Counter | ID | Meaning | Log counter |
| --- | --- | --- | --- |
| Events Received/sec | 1 | Accepted DNS events per second. | `received` |
| Frames Sent/sec | 2 | Frames handed to the sink per second. Not a delivery rate. Excludes replays. | `encoded` |
| Events Dropped/sec | 3 | Dropped at admission per second. | `intake_dropped` |
| Bytes Sent/sec | 62 | Bytes per second that the socket of the receiver connection accepted (with TLS, including record headers and tags). Add the growth of `spool_bytes` to get the capture rate in bytes ([relationships](#counter-relationships)). | `bytes_sent` |

The `/sec` values are computed by the reader from the change over time. All values are
64-bit with scale 0. IDs 1 to 3 and 62 are `PERF_COUNTER_BULK_COUNT`. IDs 33 and 43 are
`PERF_LARGE_RAW_FRACTION`, each with a hidden `PERF_LARGE_RAW_BASE` counter. The other
totals and gauges are `PERF_COUNTER_LARGE_RAWCOUNT`.

#### Totals since the service started

| Counter | ID | Meaning | Log counter |
| --- | --- | --- | --- |
| Events Received | 10 | Accepted DNS events. | `received` |
| Frames Sent | 11 | Frames handed to the sink. Not a delivery count. Excludes replays. | `encoded` |
| Events Dropped | 12 | Dropped at admission. Includes IDs 13 and 14. | `intake_dropped` |
| Channel Full Drops | 13 | Channel full. The newest event is dropped. | `ring_dropped` |
| Pool Exhausted Drops | 14 | No free pool slot, regular or jumbo. Includes ID 15. | `pool_exhausted_regular` + `pool_exhausted_jumbo` |
| Jumbo Pool Exhausted Drops | 15 | No free jumbo slot, or `--jumbo-capacity` is 0. | `pool_exhausted_jumbo` |
| ETW Events Lost | 16 | Events ETW reports lost. | `etw_lost` |
| ETW Buffers Lost | 17 | Buffers ETW reports lost. Not an event count. | `etw_buffers_lost` |
| Receiver Down Drops | 18 | Frames lost in the sink: with no spool, the live frames the link had not fully sent at a failure and the frames that arrive while the receiver is down; with a spool, the one live block handed back at a shutdown that follows a spill. With a spool, a failed connection counts nothing here. A counted frame did not reach the receiver whole, except in the TLS 1.3 key update window. Bytes the kernel accepted are not counted. | `disconnected_dropped` |
| Events Rejected | 19 | Dequeued events rejected by the parser. | `rejected` |
| Callback Errors | 20 | Events the callback could not copy or hand off. | `callback_errors` |
| Receiver Disconnects | 21 | Lost receiver connections. | `receiver_disconnects` |
| Send Queue Drops | 52 | Frames discarded, send queue full. Also in Frames Sent. | `queue_dropped` |
| Frames Spooled | 53 | Frames stored for later sending. | `spooled` |
| Frames Replayed | 54 | Frames from the spool that the receiver connection accepted, including leftovers. Handed to the socket, not confirmed received. | `replayed` |
| Spool Dropped Frames (55) / Spool Dropped Bytes (56) | 55 / 56 | Frames (55) or bytes (56) removed, skipped, or refused unsent. Causes as for `spool_dropped`. ID 56 is Advanced detail. | `spool_dropped` / `spool_dropped_bytes` |
| Spool Errors | 57 | Spool I/O errors. | `spool_errors` |
| Bytes Sent | 61 | Bytes the socket of the receiver connection accepted, live and replayed (with TLS, including record headers and tags). ID 62 is its rate. | `bytes_sent` |

How the totals relate:

- Events Dropped (12) includes IDs 13 and 14. Jumbo Pool Exhausted Drops (15) is part of
  ID 14, so do not add them. Events Dropped also includes drops when the channel's parser
  end is gone. Those have no counter of their own.
- IDs 18, 52, and 55 are sink losses, and a frame is in at most one. A frame in 18 did not reach the receiver whole, except in the TLS 1.3 key update window. Frames Replayed (54) is not a loss.
  have reached the receiver. Frames Replayed (54) is not a loss.
- To count every frame and event lost, add 12, 16, 18, 52, and 55. Leave out 17, which counts buffers. The sum misses bytes that the kernel accepted before a connection failed, because they are lost without a count.

#### Gauges

| Counter | ID | Meaning | Log counter |
| --- | --- | --- | --- |
| Channel Depth | 30 | Events the parser has not taken. Derived from other counts. | None |
| Channel Capacity | 31 | The `--capacity` value. | None |
| Channel Peak Depth | 32 | Deepest sampled channel depth. A lower bound. | `queue_depth_max` |
| % Channel Used / % Channel Used Base | 33 / 34 | Channel Depth as a percentage of Channel Capacity (33). ID 34 is the hidden base. | None |
| ETW Buffers Allocated | 40 | Buffers allocated for the session. | None |
| ETW Buffers In Use | 41 | Allocated buffers not reported free. Never below 0. | None |
| ETW Buffers Maximum | 42 | Maximum ETW reports. Can exceed `--etw-max-buffers`. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). | None |
| % ETW Buffers Used / % ETW Buffers Used Base | 43 / 44 | Buffers in use as a percentage of the maximum (43). ID 44 is the hidden base. | None |
| Receiver Connected | 50 | 1 while a Frame Streams session exists, else 0. | `receiver_connected` |
| Receiver Reconnects | 51 | Successful reconnects after the first session. A startup connection counts too. | `receiver_reconnects` |
| Spool Bytes / Spool Segments | 58 / 59 | Bytes and segment files now on disk. ID 59 is Advanced detail. | `spool_bytes` / `spool_segments` |
| Spool Available | 60 | 1 while a spool accepts data, else 0. See `spool_available`. | `spool_available` |
| Send Queue Peak Depth | 63 | Most blocks in the send queue between the parser and the router in the last 60 seconds, out of 128. A lower bound. | `send_queue_depth_max` |
| Socket Write Peak (microseconds) | 64 | Longest wait of one block for the receiver socket to become writable, in the last 60 seconds. The link never blocks in a send. | `write_max_recent_us` |
| Spool Oldest Age (milliseconds) | 65 | Age of the oldest unsent spool segment, 0 with no backlog. An upper bound for the oldest frame. | `spool_oldest_age_ms` |
| Link Saturated | 66 | 1 while the spool backlog trends up with the receiver connected. Holds its value through an outage, so read it with Receiver Connected (50). See [The saturated state](#the-saturated-state). | `link_saturated` |

The manifest gives these counters the Advanced detail level: IDs 14, 15, 19, 20, 51, 56,
and 59. All other counters have the Standard detail level.

### Reading the counters

In Performance Monitor, add the counters from the **EtwDnstap** object and choose the
instance that matches the session name. From a command prompt:

```powershell
typeperf -q EtwDnstap
typeperf "\EtwDnstap(*)\*" -sc 5
```

`typeperf -q` lists the counter names. The second command prints five samples per counter.

### If the counters do not register

An unregistered manifest causes no service error, but consumers cannot find the counter
set. `typeperf -q EtwDnstap` lists nothing. Register the manifest with `lodctr` as in
[install.md](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute),
then restart the service. After an upgrade that changes the manifest, register the new one.
See [Replace the counter manifest](upgrade.md#replace-the-counter-manifest).

If a provider call fails at startup, the service writes one Event ID 3 warning:
`performance counters are unavailable; capture continues without performance counters`.
Its `error` field names the call and the Win32 status.

### Lab validation

A 2026-11-01 run on DNSTap-WS2025 matched the `capture stopped` totals, but non-admin use, remote collection, WMI, and publication CPU cost are untested ([lab results](../history/guide-lab-results-2026-11.md#60-second-counter-run-at-10000-queries-per-second)).

### Suggested alerts

These thresholds are not tested on a host.

| Alert | Why |
| --- | --- |
| `% Channel Used` above 75 | In the 200,000 queries-per-second runs, ring drops followed the channel reaching `--capacity`. |
| `% ETW Buffers Used` above 75 | In those runs, `etw_lost` followed buffers in use reaching the maximum. |
| Any increase in `Events Dropped`, `ETW Events Lost`, `Receiver Down Drops`, `Send Queue Drops`, or `Spool Dropped Frames` | Loss totals. Each stays at 0 under normal load. |
| `Receiver Connected` at 0 | Session down. With a spool, `Spool Bytes` rises. Without one, frames count in `Receiver Down Drops`. |
| `Spool Available` at 0 | No spool in use, or paused. Frames the receiver does not take count in `Receiver Down Drops` or `Spool Dropped Frames`. |
| `Spool Bytes` near `--spool-max-mib` during an outage | Oldest data is near deletion. Compare with [Spool sizing](tuning.md#spool-sizing). |
| `Link Saturated` at 1 | The receiver or the path takes fewer bytes than the service captures. `Spool Oldest Age` shows how late the delivered data is. See [The receiver is slow](troubleshooting.md#the-receiver-is-slow). |

## Next steps

- [Troubleshooting](troubleshooting.md)
- [Measurement terms](../design/measurement-terms.md)
