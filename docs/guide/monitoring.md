# Log events and counters

> **Status:** Current · **Describes:** the code at commit `66c3de4` · **Reviewed:** 2026-11-05

The service writes diagnostics to the Application event log, under the source
`EtwDnstap`. Its only files are the spool segments that hold unsent frames. See
[Spool](configuration.md#spool). The [performance counters](#performance-counters)
report many of the same values.

## Log event IDs

| Event ID | Type | Written for |
| --- | --- | --- |
| 1 | Error | A fatal failure at startup or during the run. Text starts with `EtwDnstap failed:`. |
| 2 | Information | `capture started`, `capture stats`, `output stats`, `capture stopped`, `output stopped`, `dnstap receiver reconnected`, `dnstap link is no longer saturated`, the "cleared" event of each risk below, `ETW buffers applied`, `spool in use`, and, in diagnostic builds, `diagnostic settings applied` and `parser timing`. |
| 3 | Warning | Warnings that are not data loss and not a risk of it, such as a rejected event, a failed reconnect, or `performance counters are unavailable`. |
| 4 | Error | Errors, such as `ETW cleanup failed`. A fatal failure uses Event ID 1. |
| 5 and 6 | Error | Watchdog expiry (5) or a panic (6), written just before the process aborts. A panic text starts with `EtwDnstap panicked:` and gives the message and location. |
| 10 | Warning | Data loss in ETW: `etw_lost` or `etw_buffers_lost` rose. |
| 11 | Warning | Data loss at capacity, event channel: `ring_dropped` rose. |
| 12 | Warning | Data loss at capacity, pool slots: `pool_exhausted_regular` or `pool_exhausted_jumbo` rose. |
| 13 | Warning | Data loss on the network path: `disconnected_dropped` (no spool took the frames when a connection failed or the receiver was down) or `queue_dropped` (send queue full) rose. |
| 14 | Warning | Data loss in the spool: `spool_dropped` or `spool_dropped_bytes` rose (size cap, free-space floor, unreadable data, or an I/O error), and the spool lines that report frames removed, skipped, or refused. |
| 15 | Warning | Data loss in decoding: `rejected` or `schema_rejected` rose. |
| 16 | Warning | Data loss inside the service: `callback_errors`, `receiver_disconnected` (the parser end of the event channel, not TCP), or `output_failed_events` rose. |
| 20 | Warning | Risk: the event channel is near full. |
| 21 | Warning | Risk: the ETW buffers are near the maximum. |
| 22 | Warning | Risk: the send queue is near full. |
| 23 | Warning | Risk: `dnstap link is saturated`. |
| 24 | Warning | Risk: the receiver is down or unavailable. |
| 25 | Warning | Risk: the spool is near its size cap, or the volume is near the free-space floor. |
| 26 | Warning | Risk: no spool is in use (`--no-spool`, an unusable folder, a spool key that could not be loaded or was lost after a memory protection failure, or a spool that did not open). |
| 27 | Warning | Risk: a spool I/O error. |
| 28 | Warning | Risk: the ETW statistics query fails, so ETW loss is not visible. |

IDs 10 to 28 are Warnings: filter on the type or the ID, not on Event ID 3. Event text is
cut at 8,192 UTF-16 code units. The service logs Information level and above.

## Periodic and final reports

Every `--stats-interval-seconds`, the service writes `capture stats` and `output stats`,
each after asking ETW for its loss counters. Shutdown always writes `capture stopped` and
`output stopped`, even when periodic snapshots are off.

`capture started` records:

- `version`, from `Cargo.toml`. `(Get-Item <exe>).VersionInfo` shows it as `ProductVersion`;
  `FileVersion` is `X.Y.Z`.
- `session`, `session_guid`, `receiver`, `tls` (`true` unless `--plaintext`), and
  `tls_server_name` when set.
- `capacity`, `etw_buffer_kib`, `etw_flush_seconds`, and the requested
  `etw_min_buffers` and `etw_max_buffers`.
- `receiver_stall_seconds`: how long the receiver may accept no bytes while data waits
  before the service ends the session (default 15). See
  [`--receiver-stall-seconds`](configuration.md#options).
- `match_any_keyword` and `match_all_keyword` (hexadecimal), `keyword_fallback`, and
  `failed_responses_captured`.

`ETW buffers applied` follows once, if the ETW query succeeds, with
`etw_min_buffers_applied`, `etw_max_buffers_applied`, and `etw_buffers_allocated`. See
[ETW buffer minimums](tuning.md#etw-buffer-minimums).

### Spool messages

A repeated message logs its first four occurrences, then every 60th (61st, 121st, and so
on), unless its row says otherwise. See [Spool warnings](troubleshooting.md#spool-warnings).

| Message | Event ID | Fields | When |
| --- | --- | --- | --- |
| `spool in use` | 2 | `spool_dir`, `spool_max_mib`, `spool_min_free_mib`, `new_spool_key`, `leftover_segments`, `leftover_bytes` | Startup. `leftover_*` show what an earlier run left. `new_spool_key` is `true` when this start stored a new spool key. |
| `spool folder is not usable; capture continues without a spool, so frames are dropped and counted while the receiver is down` | 26 | `spool_dir`, `reason` | Startup, when the folder check fails. |
| `spool key is not usable; capture continues without a spool, so frames are dropped and counted while the receiver is down` | 26 | `spool_dir`, `reason` | Startup, when the spool key cannot be loaded, unwrapped, or locked in memory, or the key store does not answer within 3 seconds. `reason` names the failed step and status code. See [Spool key errors](troubleshooting.md#spool-key-errors). |
| `the spool key is unusable after a memory protection failure (CryptProtectMemory, Win32 error N); the spool is suspended until the service restarts; spooled files are kept` (or `(CryptUnprotectMemory returned bytes that fail the key check)` in place of the call and its error) | 26 | none | Once per run, when `CryptProtectMemory` fails twice or the decrypted key fails its check. The spool refuses and counts every append in `spool_dropped`, stops reads, and keeps all files; `spool_available` is false. Live frames still reach a connected receiver. Restart the service to replay the backlog. Not logged after shutdown begins. See [Spool key errors](troubleshooting.md#spool-key-errors). |
| `spool could not be opened; capture continues without a spool, so frames are dropped and counted while the receiver is down` | 26 | `spool_dir`, `error` | Startup, when the folder and key pass but the spool does not open. |
| `spool is disabled (--no-spool); frames are dropped and counted while the receiver is down, and when the send queue is full` | 26 | none | Startup, once, with `--no-spool`. |
| `dnstap spool deleted its oldest segment unsent` | 14 | `frames`, `bytes` | The size cap or free-space floor removed a segment with unsent data. |
| `dnstap spool deleted segments it cannot read at startup: segments of an earlier version or of another spool key` | 14 | `segments`, `bytes`, `earlier_format`, `other_key`, `new_key` | Startup, at most once. Segments the spool cannot decrypt were deleted and their file bytes added to `spool_dropped_bytes`: plaintext or unreadable files (`earlier_format`), another key (`other_key`), or every segment when `new_key` is `true`. |
| `dnstap spool discarded unreadable segment data` | 14 | `frames`, `bytes` | Segment data could not be read, or a record did not authenticate. The data is not replayed. |
| `dnstap spool I/O error` | 27 | `error` | A spool file operation failed. The spool pauses for 10 seconds. |
| `dnstap spool is refusing blocks: the volume is below the free-space floor and no older segment is left to delete` | 14 | `free_mib`, `needed_mib`, `pause_s` | The volume is below `--spool-min-free-mib` plus one segment, and only the newest segment is left. Refused blocks count in `spool_dropped`. `free_mib` rounds down; `needed_mib` rounds up. |

### Receiver, schema, and rate messages

Unless a row says otherwise, the repeat rule in [Spool messages](#spool-messages) applies.
Reconnect logging: [Receiver connection](configuration.md#receiver-connection).

| Message | Event ID | Fields | When |
| --- | --- | --- | --- |
| `dnstap receiver unavailable at startup; capture is running, and frames go to the spool, or are dropped and counted when no spool is in use, until it connects (retrying with backoff)` | 24 | `receiver`, `error` | Startup, when the first connection fails at the network level. |
| `dnstap receiver connection lost; frames go to the spool, or are dropped and counted when no spool is in use, until it reconnects (frames already sent are never resent)` | 24 | `receiver`, `error`, `retry_in_ms` | An established session ended. `dnstap receiver reconnected` (ID 2) ends the outage. |
| `dnstap reconnect failed` | 3 | `receiver`, `error`, `cause`, `failures` | A reconnect attempt failed. `cause` is `connect`, `timeout`, `tls`, or `protocol` (Frame Streams handshake). `failures` counts the run's failed attempts. |
| `dnstap receiver failed the TLS handshake before the first session; check that the Windows trust store trusts the receiver certificate, that --tls-server-name (or the receiver IP address when it is not set) matches the certificate, and that the receiver does not require a client certificate (this client sends none); retrying with backoff` | 4 | `receiver`, `error`, `cause`, `failures` | Once per run, when a startup outage ended in a TLS rejection. Replaces `dnstap reconnect failed` for that attempt. |
| `dnstap receiver refused the Frame Streams handshake before the first session; check that the receiver accepts the dnstap content type; retrying with backoff` | 4 | `receiver`, `error`, `cause`, `failures` | Once per run, for a Frame Streams rejection in the same situation. |
| `could not start the dnstap reconnect thread; retrying with backoff` | 3 | `receiver`, `error`, `failures` | The reconnect thread did not start. |
| `dnstap reconnect thread ended without a result; retrying with backoff` | 4 | `receiver`, `failures` | The reconnect thread ended without a result. |
| `dnstap link is saturated: the spool backlog has grown steadily over the last 10 seconds while the receiver is connected, ...` | 23 | `backlog_bytes`, `oldest_age_ms`, `bytes_sent_per_second` | The [saturated state](#the-saturated-state) set. `bytes_sent_per_second` averages the 10 samples. Logged once per state change. See [The receiver is slow](troubleshooting.md#the-receiver-is-slow). |
| `dnstap link is no longer saturated` | 2 | `backlog_bytes`, `oldest_age_ms`, `bytes_sent_per_second`, `reason` | The state ended. `reason` is `the backlog is empty`, `the backlog stopped growing` (not larger than 10 samples ago), or `the spool is suspended until the service restarts` (the spool key was lost; the state does not set again in this run). |
| `DNS event schema load failed; its events are rejected until the load is retried` | 3 | `event_id`, `version`, `failures`, `retry_after_events` (65,536) | Windows could not return the event schema (a Win32 error from `TdhGetEventInformation` follows the message). Logged per (event ID, version) key. |
| `DNS event schema is unsupported and its events are rejected` | 3 | `event_id`, `version` | The metadata loaded but cannot be decoded. Logged once per key, not retried; it can repeat after the rejected-key list (8 entries) is cleared, or when a retry fails as unsupported. |
| `rejected ETW event; the first four and then every 60th are logged, all are counted` | 3 | `error`, `rejected` | The parser rejected an event. `rejected` is the running count. |
| `received rate is above 22,000 events/s; the default buffer settings were validated at about 20,000 events/s (10,000 queries per second); if ring_dropped or etw_lost increase, use the settings for your rate in the "Recommended settings for higher rates" section of docs/guide/tuning.md, for example --etw-buffer-kib 128 --etw-max-buffers 128 for 50,000 queries per second` | 3 | `events_per_second` | Once per run, with the default buffer settings, at the end of the first measurement window above 22,000 events per second. See [Rate warning](troubleshooting.md#rate-warning). |

## Loss and risk warnings

A loss warning (IDs 10 to 16) reports lost data. A risk warning (IDs 20 to 28) reports likely
or possible loss. An alert rule can act on the ID alone.

- The supervisor checks on its stats tick: every `--stats-interval-seconds`, or every 60
  seconds when that is `0`, after asking ETW for its loss counters. A long interval delays a
  check by up to that interval.
- Each check writes at most one event for each ID. A loss event lists only the counters that
  rose, each as `<counter>_increase` (since the previous check) and `<counter>` (run
  total), plus `seconds`, the time the check covers. The first check covers the time since
  capture started.
- A risk is edge-triggered: the warning is written when the condition starts, and an
  Information event (ID 2) when it ends. A percentage risk clears 10 points below the level
  that raised it.
- A line that reports a loss as it happens (a spool eviction, for example) is written
  immediately, with the repeat rule from [Spool messages](#spool-messages).
- After the service observes the stop, it writes only `capture stopped`, `output stopped`,
  and the lines in [Shutdown results](#shutdown-results). The counters still count.

### Loss warnings

Every row has `seconds`. Causes: [Event IDs for data loss](troubleshooting.md#event-ids-for-data-loss).

| Message starts with | Event ID | Fields | Fix |
| --- | --- | --- | --- |
| `data loss in ETW` | 10 | `etw_lost_increase`, `etw_lost`, `etw_buffers_lost_increase`, `etw_buffers_lost` | Raise `--etw-max-buffers` and `--etw-buffer-kib`; check CPU starvation ([tuning.md](tuning.md#defaults)). |
| `data loss at capacity: the event channel was full` | 11 | `ring_dropped_increase`, `ring_dropped`, `capacity` | Raise `--capacity`. |
| `data loss at capacity: no pool slot was free` | 12 | `pool_exhausted_regular_increase`, `pool_exhausted_regular`, `pool_exhausted_jumbo_increase`, `pool_exhausted_jumbo`, `jumbo_capacity` | Raise `--capacity` (regular) or `--jumbo-capacity` (jumbo). |
| `data loss on the network path` | 13 | `disconnected_dropped_increase`, `disconnected_dropped`, `queue_dropped_increase`, `queue_dropped` | Check the receiver, the network, and `spool_available`. See [receiver down](troubleshooting.md#the-receiver-is-down-or-unreachable) and [receiver slow](troubleshooting.md#the-receiver-is-slow). |
| `data loss in the spool` | 14 | `spool_dropped_increase`, `spool_dropped`, `spool_dropped_bytes_increase`, `spool_dropped_bytes` | Free disk space or raise `--spool-max-mib`. See [Spool warnings](troubleshooting.md#spool-warnings), [Spool sizing](tuning.md#spool-sizing). |
| `data loss in decoding` | 15 | `rejected_increase`, `rejected`, `schema_rejected_increase`, `schema_rejected` | Read the `rejected ETW event` and schema warnings (ID 3). See [Rejected events](troubleshooting.md#rejected-events). |
| `data loss inside the service` | 16 | `callback_errors_increase`, `callback_errors`, `receiver_disconnected_increase`, `receiver_disconnected`, `output_failed_events_increase`, `output_failed_events` | Report it with the steps that led to it. See [restarts](troubleshooting.md#the-service-keeps-restarting). |

An ID 11 or 12 increase is part of `intake_dropped`: do not add it to that counter
([Counter relationships](#counter-relationships)). The spool lines
`dnstap spool deleted its oldest segment unsent`, `dnstap spool deleted segments it cannot
read at startup`, `dnstap spool discarded unreadable segment data`, and
`dnstap spool is refusing blocks` also carry ID 14.

### Risk warnings

| Message starts with | Event ID | Fields | Raised when | Cleared by (ID 2) | Fix |
| --- | --- | --- | --- | --- | --- |
| `the event channel is near full` | 20 | `channel_peak`, `capacity`, `percent` | The deepest channel depth the parser sampled since the previous check is at least 80 percent of `--capacity`. Clears after two checks in a row below 70 percent. | `the event channel is no longer near full` | Raise `--capacity`. Next step: `ring_dropped` (ID 11). |
| `the ETW buffers are near the maximum` | 21 | `etw_buffers_allocated`, `etw_buffers_maximum`, `percent` | Allocated buffers are at least 90 percent of the maximum ETW applied (needs a successful ETW query). Clears below 80 percent. | `the ETW buffers are no longer near the maximum` | Raise `--etw-max-buffers`. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). |
| `the send queue is near full` | 22 | `send_queue_depth_max`, `send_queue_blocks` (256), `percent` | `send_queue_depth_max` (the 60-second peak) is at least 80 percent of the queue: 205 of 256 blocks. Clears below 70 percent. | `the send queue is no longer near full` | Spool disk slow or router thread starved. See [receiver slow](troubleshooting.md#the-receiver-is-slow). |
| `dnstap link is saturated` | 23 | `backlog_bytes`, `oldest_age_ms`, `bytes_sent_per_second` | [The saturated state](#the-saturated-state) sets. | `dnstap link is no longer saturated` | See [receiver slow](troubleshooting.md#the-receiver-is-slow). |
| `dnstap receiver unavailable at startup` and `dnstap receiver connection lost` | 24 | See [Receiver, schema, and rate messages](#receiver-schema-and-rate-messages) | The receiver is not connected; with a spool the frames wait on disk. Written when the outage starts, not on the tick. | `dnstap receiver reconnected` | See [receiver down](troubleshooting.md#the-receiver-is-down-or-unreachable). |
| `the dnstap receiver is down and no spool takes frames` | 24 | `receiver_connected` (`false`), `spool_available` (`false`) | At a check, `receiver_connected` and `spool_available` are both `false`: `--no-spool`, an unusable spool, or a paused spool. **Frames are being lost** and counted in `disconnected_dropped` (ID 13). Once for each outage. | `frames are no longer dropped for a receiver outage` | Restore the receiver; run with a spool. See [Spool warnings](troubleshooting.md#spool-warnings). |
| `the spool is near its limit` | 25 | `spool_mib`, `spool_max_mib`, `free_mib`, `needed_mib` | The spool holds at least 80 percent of `--spool-max-mib`, or the volume's free space is less than `--spool-min-free-mib` plus 2 segments (`needed_mib` is the floor: the minimum plus 1 segment). Clears below 70 percent, and at 3 segments above the minimum. Checked only while a spool is in use. `free_mib` is missing when the probe fails. | `the spool is no longer near its limit` | Free disk space or raise `--spool-max-mib`; at the limit the spool deletes its oldest data (ID 14). See [Spool sizing](tuning.md#spool-sizing). |
| `spool is disabled`, `spool folder is not usable`, `spool key is not usable`, and `spool could not be opened` | 26 | `spool_dir`, `reason` or `error` (absent for `spool is disabled`) | Startup, once per run. Any receiver outage loses frames. | none | Remove `--no-spool`, fix the folder or the key. See [Spool warnings](troubleshooting.md#spool-warnings). |
| `dnstap spool I/O error` | 27 | `error` | A spool file operation failed. The spool pauses for 10 seconds, and blocks in that time may be refused (ID 14). Written immediately, with the repeat rule of [Spool messages](#spool-messages). | none | Check the disk, programs that lock the folder, and its ACL. |
| `the ETW statistics query is failing` | 28 | `etw_stats_errors_increase`, `etw_stats_errors` | A query failed in the interval and the last query did not succeed. Until it works, `etw_lost` and the buffer counts are stale, so ID 10 and ID 21 cannot fire. | `the ETW statistics query works again` | Check that the session runs. See [ETW consumer and session warnings](troubleshooting.md#etw-consumer-and-session-warnings). |

Bytes the kernel accepted before a connection failed are lost without a count; the
`dnstap receiver connection lost` line (ID 24) marks every such failure. The counters give
how much was lost, and the events give why.

## Capture counters

Fields of `capture stats` and `capture stopped`.

| Counter | Meaning |
| --- | --- |
| `received` | DNS records (event IDs 256, 257, and 258) accepted after the process check, including later drops and rejects. |
| `dequeued` | Records the parser took from the channel. |
| `encoded` | Records handed to the sink, including frames the sink later discarded while disconnected. Not a delivery count. |
| `ignored` | Dequeued records that are not a supported DNS event. |
| `rejected` | Dequeued records rejected as malformed or invalid, including `schema_rejected`. |
| `intake_dropped` | Records lost at admission: `ring_dropped`, `pool_exhausted_regular`, `pool_exhausted_jumbo`, and `receiver_disconnected`. |
| `ring_dropped` | Newest event dropped because the channel was full. |
| `pool_exhausted_regular`, `pool_exhausted_jumbo` | Regular: no free regular slot. Jumbo (payload over 2 KiB): no free jumbo slot, or `--jumbo-capacity` is `0`. |
| `receiver_disconnected` | The parser end of the event channel is gone. |
| `etw_lost` | Events ETW reports lost, as of its last successful query or stop. |
| `etw_buffers_lost` | ETW buffers lost (buffers, not events). |
| `callback_errors` | Records the callback could not copy or hand off. Outside `intake_dropped`. |
| `schema_rejected` | Records whose schema failed to load or to compile. |
| `etw_stats_valid`, `etw_stats_errors` | `etw_stats_valid` is `true` when the last ETW query or stop succeeded. `etw_stats_errors` counts failed queries, including the once-per-second queries of the performance counters. |
| `stale_sessions_reclaimed` | Leaked sessions stopped at startup. |
| `etw_buffers_delivered` | ETW buffer callbacks completed. |
| `parser_wake_requests` | Buffers that enqueued an event; each sends one wake request. |
| `cpu_etw_pct`, `cpu_parser_pct` | `capture stats` only. CPU of the `etw-consumer` and `dnstap-parser` threads since the previous `capture stats` line. See [Stage CPU](#stage-cpu). |

## Output counters

Fields of `output stats` and `output stopped`. The three fields in
[Shutdown results](#shutdown-results) appear on `output stopped` only.

| Counter | Meaning |
| --- | --- |
| `output_failed_events` | Dequeued records whose frame could not be queued after the sender stopped. Otherwise 0. |
| `output_errors` | Output failures: one per `output_failed_events`, or a failed queue flush. |
| `receiver_connected` | Flag: `true` while a Frame Streams session exists. |
| `receiver_disconnects` | Established sessions lost: a socket error, or no bytes accepted for `--receiver-stall-seconds` while data waited. A slow receiver keeps the session. |
| `receiver_reconnects` | Successful reconnects after the first session. A startup connection counts too in the performance counter. |
| `receiver_reconnect_failures` | Failed reconnect attempts at connect, handshake, or protocol, plus a reconnect thread that did not start or ended without a result. |
| `disconnected_dropped` | Frames lost in the sink. With no spool: the live frames the link had not handed to the socket in full when a connection failed (the cut frame, the rest of the block in hand, the link queue), and every live frame that arrives while the receiver is down. With a spool: only the one live block handed back at a shutdown that follows a spill. A counted frame did not reach the receiver whole, except in the TLS 1.3 key update window, where the receiver may hold it. Bytes the kernel accepted before a failure are not counted. Outside `intake_dropped`. |
| `slow_writes`, `write_max_us` | Log-report fields only. When the socket takes no more, the link thread waits until it is writable and adds up the waits for each block (about 64 KiB, or one replay batch). `slow_writes` counts blocks whose wait reached 5 ms. `write_max_us` is the longest wait of one block in microseconds, including a wait in progress. Both count since the service started. |
| `write_max_recent_us` | Gauge: the longest such wait in the last 60 seconds, in microseconds. Use it, not `write_max_us`, to see whether the receiver is slow now. See [Windows of the recent peaks](#windows-of-the-recent-peaks). |
| `router_wait_max_recent_us` | Gauge: the longest single wait of the router thread for work, in microseconds, in the last 60 seconds. The wait ends after at most 50 ms; a value far above 50,000 means a late wake (CPU contention in the guest or on the host, or a held process). |
| `router_pass_max_recent_us` | Gauge: the longest router pass outside its wait, in microseconds, in the last 60 seconds. A large value means a call in the pass blocked or ran long, such as a disk write or the hand-back wait. With `router_wait_max_recent_us` it separates a late wake from a blocking call when `queue_dropped` rises. |
| `bytes_sent` | Bytes the receiver socket accepted, live and replayed, including bytes the kernel still held when the connection failed. Plaintext counts framed bytes (4-byte length and payload). TLS counts bytes written to the socket, including record headers and tags (about 22 to 29 bytes for each 16 KiB). Control frames are excluded. |
| `queue_depth_max` | Deepest event channel depth the parser sampled, after each idle poll and every 256th event. A lower bound. This is the channel before the parser. |
| `send_queue_depth_max` | Gauge: the most blocks (about 64 KiB each) in the send queue between the parser and the router in the last 60 seconds, out of 256. A lower bound, sampled each time the router takes a block. A deep queue means a slow spool disk, a router thread that did not run (see `router_wait_max_recent_us`, `router_pass_max_recent_us`), or late ETW events in one burst. A slow receiver does not deepen it: the router sends blocks to the link queue (256 blocks, about 16 MiB) and spills it to the spool only when full. |
| `spool_oldest_age_ms` | Gauge: milliseconds since the oldest unsent spool segment was created, 0 with no backlog. An upper bound on the age of the oldest unsent frame. A segment from an earlier run uses the file creation time. |
| `link_saturated` | Flag: `true` while the spool backlog trends up with the receiver connected. Holds its value while the receiver is down. `false` with no spool, and once the spool key is lost. Rule: [The saturated state](#the-saturated-state). |
| `queue_dropped` | Frames discarded because the send queue was full. Also in `encoded`. |
| `spooled` | Frames stored for later sending. |
| `replayed` | Frames from the spool that the receiver connection fully accepted, including earlier-run leftovers. A frame counts once, when the connection takes it. It counts bytes handed to the socket, not frames the receiver read. |
| `spool_dropped`, `spool_dropped_bytes` | Frames (`spool_dropped`) or file bytes (`spool_dropped_bytes`, including segment headers and the 28-byte overhead of each record) removed, skipped, or refused unsent: eviction, corrupt or cut-off data, segments deleted at startup because they cannot be decrypted, a pause, an I/O error, and every append while the spool is suspended after a lost key (kept files are not counted). Leftover segments count only bytes, because their frame count is unknown. |
| `spool_errors` | Every spool I/O error. |
| `spool_bytes`, `spool_segments` | Gauges: file bytes and segment files now, including files whose removal has not yet succeeded. File bytes include a 32-byte header for each segment and 28 bytes for each record (one record for each block the router appends). |
| `spool_available` | Flag: `true` while the spool accepts data. `false` with `--no-spool`, an unusable folder, or while an I/O error or the free-space floor pauses it. |
| `cpu_sender_pct`, `cpu_link_pct` | `output stats` only. CPU of the `dnstap-sender` and `dnstap-link` threads since the previous `output stats` line. See [Stage CPU](#stage-cpu). |

### Shutdown results

| Field | Meaning |
| --- | --- |
| `stop_sent` | `true` when the service wrote the Frame Streams STOP frame, even if the receiver sends no FINISH. `false` when no session was up at the stop, a data frame was cut and the connection closed with no STOP, the STOP could not be written, the socket could not return to blocking mode, or the final data flush failed. A `false` value leaves the exit code 0. |
| `spool_backlog_kept` | `true` when unsent frames stay in the spool for the next start, also when the seal failed. `false` with an empty spool or no spool. |
| `spool_seal_error` | Present only when the active spool segment could not be sealed: the error text. The next start replays that segment's frames. If it repeats, check the disk. The line is Event ID 2. |

An abnormal stop can still write:

- `ETW consumer did not return within N s of STOP; closing the trace handle` (warning).
- `ETW cleanup failed` (Event ID 4).
- `parser timing`, in diagnostic builds, after the two stop reports.

```text
INFO output stopped output_failed_events=0 output_errors=0 receiver_connected=false receiver_disconnects=1 ... link_saturated=false stop_sent=false spool_backlog_kept=true
```

### Windows of the recent peaks

`write_max_recent_us`, `router_wait_max_recent_us`, `router_pass_max_recent_us`, and
`send_queue_depth_max` are the largest value in the last 60 one-second samples. The router
thread publishes it every second, and the stats log and performance counters read the same
value. With a `--stats-interval-seconds` above 60, a peak older than a minute is missing.
A value appears up to a second after the event. A slow disk append on the router thread can
stretch the window beyond a minute.

### The saturated state

The router thread keeps the last 11 samples of the unread backlog (spool bytes not yet
handed to the link), taken at least 1 second apart, so the trend spans at least 10 seconds.

- **Set.** The backlog is larger than 10 samples ago, and at least 7 of the 10 steps rose,
  with the receiver connected. The service logs `dnstap link is saturated` (Event ID 23).
  Seven of ten tolerates flat steps from 64 KiB replay batches.
- **Clear.** The backlog is empty (at once) or no longer larger than 10 samples ago. The
  service logs `dnstap link is no longer saturated` (Event ID 2).
- **Held.** While the receiver is down, while an I/O error pauses spool reads, and after a
  session was lost since the last sample, the samples start over and the state keeps its
  value. Read `link_saturated` with `receiver_connected`.
- **Silent outage.** A receiver that stops reading without closing the connection looks
  saturated until the stall timeout (default 15 seconds) ends the session. The lab saw the
  flag set 11.4 to 12 seconds into such an outage
  ([lab validation](../history/slow-receiver-validation.md)).

A flapping state logs at most one event for each 10 samples. The state only reports and
leaves routing unchanged. With no spool it is `false`: watch `send_queue_depth_max` and
`queue_dropped` instead.

## Counter relationships

```text
intake_dropped = ring_dropped + pool_exhausted_regular + pool_exhausted_jumbo
                 + receiver_disconnected
received       = intake_dropped + callback_errors + dequeued + queue backlog
dequeued       = encoded + ignored + rejected + output_failed_events
```

- `disconnected_dropped`, `queue_dropped`, and `spool_dropped` are the sink's loss
  counters; a frame is in at most one. `disconnected_dropped` is outside the identities
  because its frames already count in `encoded`.
- A stored frame counts in `spooled`, then in `replayed` when the connection accepts it, or
  in `spool_dropped` if the spool removes it first. A frame on disk at shutdown counts in
  `spooled` only.
- Frames in the send queue at an abnormal sender stop count nowhere; the parser counts the
  failed write in `output_failed_events`.
- Leftover frames count in `replayed`, not in `encoded`. A leftover segment removed unsent
  adds only to `spool_dropped_bytes`. A cut-off final record adds one to `spool_dropped`.
- Capture rate in bytes over a few minutes = `bytes_sent` rate + growth of `spool_bytes` +
  `spool_dropped_bytes` rate. The spool figures include about 28 bytes of encryption
  overhead for each 64 KiB block.
- Only the collector's own count checks delivery.
- Live samples can disagree briefly. The final report, written after the parser joins, has
  no backlog, so the identities close there.

## Performance counters

The service publishes the Windows performance counter set `EtwDnstap`.

- Manifest: [`packaging/perfcounters/etw-dnstap.man`](../../packaging/perfcounters/etw-dnstap.man).
  The provider and the counter set are both named `EtwDnstap`.
- Provider GUID: `{A7F0B1F4-C87D-4267-979F-594A4AB2145D}`.
- Counter set GUID: `{B02EDFA2-B961-4E06-89D2-56C95ECB336C}`.
- Installed copy: `C:\Program Files\EtwDnstap\etw-dnstap.man`. The MSI registers it with
  `lodctr`. In a hand-provisioned install,
  [Install and provision](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute)
  registers it.

Counter names and help text are resources in `etw-dnstap.exe`. Read the counters with Performance Monitor, `typeperf`, `logman` data collector sets, or WMI
(`Win32_PerfRawData_*` classes).

### Instance and timing

The counter set has one instance, named after the ETW session (default `EtwDnstap`), which
exists only while capture runs. Counter paths use `\EtwDnstap(<instance>)\<counter>`.
`--session-name` rules: [Options](configuration.md#options).

Windows reads the rates, totals, and gauges that are plain atomics (IDs 1 to 3, 10 to 21, 32,
51, 52 to 59, 61 to 65, 67, and 68) by reference, so each consumer gets an exact rate for
its own sample interval. The service publishes the other gauges (IDs 30, 31, 33, 34, 40 to
44, 50, 60, 66, and 69 to 80) once per second, and runs the ETW statistics query at the same
rate. ETW gauges read 0 until the first successful query, then keep the last successful
value. The counters add no work to the ETW callback or the parser loop.

### Catalog

The "Log field" column names the field that defines the counter, in
[Capture counters](#capture-counters) or [Output counters](#output-counters). Flags read 1
or 0. All values are 64-bit with scale 0. The reader computes `/sec` rates from the change
over time.

#### Throughput rates

| ID | Counter | Type | Unit | Log field |
| --- | --- | --- | --- | --- |
| 1 | Events Received/sec | `PERF_COUNTER_BULK_COUNT` | events/s | `received` |
| 2 | Frames Sent/sec | `PERF_COUNTER_BULK_COUNT` | frames/s | `encoded` |
| 3 | Events Dropped/sec | `PERF_COUNTER_BULK_COUNT` | events/s | `intake_dropped` |
| 62 | Bytes Sent/sec | `PERF_COUNTER_BULK_COUNT` | bytes/s | `bytes_sent` |

Frames Sent/sec and Frames Sent (11) exclude replays. Add the growth of `spool_bytes` to Bytes Sent/sec for the capture rate in bytes.

#### Totals since the service started

All are `PERF_COUNTER_LARGE_RAWCOUNT`.

| ID | Counter | Unit | Log field |
| --- | --- | --- | --- |
| 10 | Events Received | events | `received` |
| 11 | Frames Sent | frames | `encoded` |
| 12 | Events Dropped | events | `intake_dropped` |
| 13 | Channel Full Drops | events | `ring_dropped` |
| 14 | Pool Exhausted Drops | events | `pool_exhausted_regular` + `pool_exhausted_jumbo` |
| 15 | Jumbo Pool Exhausted Drops | events | `pool_exhausted_jumbo` |
| 16 | ETW Events Lost | events | `etw_lost` |
| 17 | ETW Buffers Lost | buffers | `etw_buffers_lost` |
| 18 | Receiver Down Drops | frames | `disconnected_dropped` |
| 19 | Events Rejected | events | `rejected` |
| 20 | Callback Errors | events | `callback_errors` |
| 21 | Receiver Disconnects | connections | `receiver_disconnects` |
| 52 | Send Queue Drops | frames | `queue_dropped` |
| 53 | Frames Spooled | frames | `spooled` |
| 54 | Frames Replayed | frames | `replayed` |
| 55 | Spool Dropped Frames | frames | `spool_dropped` |
| 56 | Spool Dropped Bytes | bytes | `spool_dropped_bytes` |
| 57 | Spool Errors | errors | `spool_errors` |
| 61 | Bytes Sent | bytes | `bytes_sent` |

Events Dropped (12) includes 13, 14 (which includes 15), and `receiver_disconnected`, which
has no counter of its own. IDs 18, 52, and 55 are sink losses. To count every frame and event
lost, add 12, 16, 18, 52, and 55 (not 17, which counts buffers); the sum misses bytes the
kernel accepted before a connection failed.

#### Gauges

| ID | Counter | Type | Unit | Log field |
| --- | --- | --- | --- | --- |
| 30 | Channel Depth | `PERF_COUNTER_LARGE_RAWCOUNT` | events | None |
| 31 | Channel Capacity | `PERF_COUNTER_LARGE_RAWCOUNT` | events | None |
| 32 | Channel Peak Depth | `PERF_COUNTER_LARGE_RAWCOUNT` | events | `queue_depth_max` |
| 33 | % Channel Used | `PERF_LARGE_RAW_FRACTION` | percent | None |
| 34 | % Channel Used Base (hidden) | `PERF_LARGE_RAW_BASE` | events | None |
| 40 | ETW Buffers Allocated | `PERF_COUNTER_LARGE_RAWCOUNT` | buffers | None |
| 41 | ETW Buffers In Use | `PERF_COUNTER_LARGE_RAWCOUNT` | buffers | None |
| 42 | ETW Buffers Maximum | `PERF_COUNTER_LARGE_RAWCOUNT` | buffers | None |
| 43 | % ETW Buffers Used | `PERF_LARGE_RAW_FRACTION` | percent | None |
| 44 | % ETW Buffers Used Base (hidden) | `PERF_LARGE_RAW_BASE` | buffers | None |
| 50 | Receiver Connected | `PERF_COUNTER_LARGE_RAWCOUNT` | flag | `receiver_connected` |
| 51 | Receiver Reconnects | `PERF_COUNTER_LARGE_RAWCOUNT` | reconnects | `receiver_reconnects` |
| 58 | Spool Bytes | `PERF_COUNTER_LARGE_RAWCOUNT` | bytes | `spool_bytes` |
| 59 | Spool Segments | `PERF_COUNTER_LARGE_RAWCOUNT` | segments | `spool_segments` |
| 60 | Spool Available | `PERF_COUNTER_LARGE_RAWCOUNT` | flag | `spool_available` |
| 63 | Send Queue Peak Depth | `PERF_COUNTER_LARGE_RAWCOUNT` | blocks | `send_queue_depth_max` |
| 64 | Socket Write Peak (microseconds) | `PERF_COUNTER_LARGE_RAWCOUNT` | microseconds | `write_max_recent_us` |
| 65 | Spool Oldest Age (milliseconds) | `PERF_COUNTER_LARGE_RAWCOUNT` | milliseconds | `spool_oldest_age_ms` |
| 66 | Link Saturated | `PERF_COUNTER_LARGE_RAWCOUNT` | flag | `link_saturated` |
| 67 | Router Wait Peak (microseconds) | `PERF_COUNTER_LARGE_RAWCOUNT` | microseconds | `router_wait_max_recent_us` |
| 68 | Router Pass Peak (microseconds) | `PERF_COUNTER_LARGE_RAWCOUNT` | microseconds | `router_pass_max_recent_us` |
| 69 | Stage CPU ETW Consumer | `PERF_LARGE_RAW_FRACTION` | percent | `cpu_etw_pct` |
| 70 | Stage CPU ETW Consumer Base (hidden) | `PERF_LARGE_RAW_BASE` | none | None |
| 71 | Stage CPU Parser | `PERF_LARGE_RAW_FRACTION` | percent | `cpu_parser_pct` |
| 72 | Stage CPU Parser Base (hidden) | `PERF_LARGE_RAW_BASE` | none | None |
| 73 | Stage CPU Sender | `PERF_LARGE_RAW_FRACTION` | percent | `cpu_sender_pct` |
| 74 | Stage CPU Sender Base (hidden) | `PERF_LARGE_RAW_BASE` | none | None |
| 75 | Stage CPU Link | `PERF_LARGE_RAW_FRACTION` | percent | `cpu_link_pct` |
| 76 | Stage CPU Link Base (hidden) | `PERF_LARGE_RAW_BASE` | none | None |
| 77 | Stage CPU Time ETW Consumer (milliseconds) | `PERF_COUNTER_LARGE_RAWCOUNT` | milliseconds | None |
| 78 | Stage CPU Time Parser (milliseconds) | `PERF_COUNTER_LARGE_RAWCOUNT` | milliseconds | None |
| 79 | Stage CPU Time Sender (milliseconds) | `PERF_COUNTER_LARGE_RAWCOUNT` | milliseconds | None |
| 80 | Stage CPU Time Link (milliseconds) | `PERF_COUNTER_LARGE_RAWCOUNT` | milliseconds | None |

Counters without a log field:

- Channel Depth (30): events the parser has not taken, derived from other counts. Channel
  Capacity (31): the `--capacity` value. % Channel Used (33): Channel Depth as a percentage
  of Channel Capacity.
- ETW Buffers Allocated (40): buffers allocated for the session. In Use (41): allocated
  buffers not reported free, never below 0. Maximum (42): the maximum ETW reports, which can
  exceed `--etw-max-buffers` ([ETW buffer minimums](tuning.md#etw-buffer-minimums)).
  % ETW Buffers Used (43): In Use as a percentage of the maximum.
- Stage CPU Time (77 to 80): kernel plus user CPU time each thread has used since the
  service started.

Receiver Reconnects (51) also counts a startup connection. Link Saturated (66) holds its
value through an outage: read it with Receiver Connected (50). The manifest gives the
Advanced detail level to IDs 14, 15, 19, 20, 51, 56, 59, and 77 to 80, and the Standard level
to all others.

#### Stage CPU

`etw-consumer` (the ETW callback and event intake), `dnstap-parser`, `dnstap-sender` (the
router), and `dnstap-link` (the socket writes and TLS) can each use a whole core. IDs 69, 71,
73, and 75 and the `cpu_*_pct` log fields give each thread's CPU share of the last second
(log: since the previous stats line) as a percentage of one logical processor: 100 (100.0 in the log, with one decimal) is one core fully busy. A value near 100
marks the limiting stage. Graph with a sample interval of one second or more. Windows accounts thread time in clock
ticks, so a reading can be a point or two above 100. A thread that has ended reads 0.

### Reading the counters

In Performance Monitor, add counters from the **EtwDnstap** object and choose the instance
that matches the session name. From a command prompt:

```powershell
typeperf -q EtwDnstap
typeperf "\EtwDnstap(*)\*" -sc 5
```

`typeperf -q` lists the counter names. The second command prints five samples per counter.

### If the counters do not register

With an unregistered manifest, `typeperf -q EtwDnstap` lists nothing. Register it with
`lodctr` as in
[install.md](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute),
then restart the service. After an upgrade that changes the manifest, register the new one
([Replace the counter manifest](upgrade.md#replace-the-counter-manifest)).

If a provider call fails at startup, the service writes one Event ID 3 warning:
`performance counters are unavailable; capture continues without performance counters`.
Its `error` field names the call and the Win32 status.

### Lab validation

A 2026-11-01 run on DNSTap-WS2025 matched the `capture stopped` totals. Non-admin use,
remote collection, WMI, and publication CPU cost are untested
([lab results](../history/guide-lab-results-2026-11.md#60-second-counter-run-at-10000-queries-per-second)).

### Suggested alerts

These thresholds are untested on a host.

| Alert | Why |
| --- | --- |
| `% Channel Used` above 75 | In the 200,000 queries-per-second runs, ring drops followed the channel reaching `--capacity`. |
| `% ETW Buffers Used` above 75 | In those runs, `etw_lost` followed buffers in use reaching the maximum. |
| Any increase in `Events Dropped`, `ETW Events Lost`, `Receiver Down Drops`, `Send Queue Drops`, or `Spool Dropped Frames` | Loss totals; each stays at 0 under normal load. |
| Application log Event ID 10 to 16, or 20 to 28 | Loss, or likely loss. See [Loss warnings](#loss-warnings), [Risk warnings](#risk-warnings). |
| `Receiver Connected` at 0 | Session down. `Spool Bytes` rises with a spool; frames count in `Receiver Down Drops` without one. |
| `Spool Available` at 0 | No spool in use, or paused. Frames the receiver does not take count in `Receiver Down Drops` or `Spool Dropped Frames`. |
| `Spool Bytes` near `--spool-max-mib` during an outage | Oldest data is near deletion. See [Spool sizing](tuning.md#spool-sizing). |
| `Link Saturated` at 1 | The receiver or path takes fewer bytes than the service captures. `Spool Oldest Age` shows the delivery delay. See [The receiver is slow](troubleshooting.md#the-receiver-is-slow). |

## Next steps

- [Troubleshooting](troubleshooting.md)
- [Measurement terms](../design/measurement-terms.md)
