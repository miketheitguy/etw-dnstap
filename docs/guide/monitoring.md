# Log events and counters

> **Status:** Current · **Describes:** the code at commit `66c3de4` · **Reviewed:** 2026-11-05

### Quick Start

Diagnostics go to the Application log, source `EtwDnstap`. The service's only files are spool
segments ([Spool](configuration.md#spool)).

```powershell
Get-WinEvent -FilterHashtable @{LogName='Application'; ProviderName='EtwDnstap'} -MaxEvents 20 |
    Select-Object TimeCreated, Id, Message
typeperf -q EtwDnstap                # list the counters
typeperf "\EtwDnstap(*)\*" -sc 5     # five samples of each
```

Watch these first:

- Event IDs 10 to 16 (data lost) and 20 to 28 (loss likely).
- `Events Dropped` rising.
- `Receiver Connected` at 0, or `Link Saturated` at 1.

More: [Suggested alerts](#suggested-alerts). Terms: [Measurement terms](../design/measurement-terms.md).

### Steps / Configuration

#### Log event IDs

| Event ID | Level | Details |
| --- | --- | --- |
| 1 | Error | Fatal failure; text starts with `EtwDnstap failed:` |
| 2 | Information | [Reports](#periodic-and-final-reports), risk clears, status lines; diagnostic builds add `diagnostic settings applied` |
| 3 | Warning | Neither loss nor risk |
| 4 | Error | Non-fatal error |
| 5 | Error | Watchdog expiry, just before the process aborts |
| 6 | Error | Panic, just before abort; starts with `EtwDnstap panicked:`, then message and location |
| 10 to 16 | Warning | [Loss warnings](#loss-warnings) |
| 20 to 28 | Warning | [Risk warnings](#risk-warnings) |

Text is cut at 8,192 UTF-16 code units. The service logs Information and above.

#### Periodic and final reports

Every `--stats-interval-seconds`: `capture stats` and `output stats`, each after an ETW
loss-counter query. Shutdown always writes `capture stopped` and `output stopped`.

`capture started` fields:

- `version` (from `Cargo.toml`; `ProductVersion` in `(Get-Item <exe>).VersionInfo`, where `FileVersion` is `X.Y.Z`).
- `session`, `session_guid`, `receiver`, `tls` (`true` unless `--plaintext`), `tls_server_name` when set.
- `capacity`, `etw_buffer_kib`, `etw_flush_seconds`, requested `etw_min_buffers` and `etw_max_buffers`.
- `receiver_stall_seconds` (default 15, [`--receiver-stall-seconds`](configuration.md#options)).
- `match_any_keyword`, `match_all_keyword` (hex), `keyword_fallback`, `failed_responses_captured`.

If the ETW query succeeds, `ETW buffers applied` follows once: `etw_min_buffers_applied`,
`etw_max_buffers_applied`, `etw_buffers_allocated` ([ETW buffer minimums](tuning.md#etw-buffer-minimums)).

#### Spool messages

Repeat rule: first four occurrences, then every 60th (61st, 121st, and so on), unless a row
says otherwise. Causes: [Spool warnings](troubleshooting.md#spool-warnings),
[Spool key errors](troubleshooting.md#spool-key-errors).

| Message | ID | Fields | When |
| --- | --- | --- | --- |
| `spool in use` | 2 | `spool_dir`, `spool_max_mib`, `spool_min_free_mib`, `new_spool_key`, `leftover_segments`, `leftover_bytes` | Startup. `new_spool_key` `true`: a new key was stored. |
| `spool folder is not usable; capture continues without a spool, so frames are dropped and counted while the receiver is down` | 26 | `spool_dir`, `reason` | Startup, folder check failed. |
| `spool key is not usable; capture continues without a spool, so frames are dropped and counted while the receiver is down` | 26 | `spool_dir`, `reason` | Startup: key not loaded, unwrapped, or locked, or no key-store answer within 3 seconds. |
| `the spool key is unusable after a memory protection failure (CryptProtectMemory, Win32 error N); the spool is suspended until the service restarts; spooled files are kept` (or `(CryptUnprotectMemory returned bytes that fail the key check)` in place of the call and its error) | 26 | none | Once per run: `CryptProtectMemory` failed twice, or the decrypted key failed its check. Restart to replay. |
| `spool could not be opened; capture continues without a spool, so frames are dropped and counted while the receiver is down` | 26 | `spool_dir`, `error` | Startup: folder and key pass, spool doesn't open. |
| `spool is disabled (--no-spool); frames are dropped and counted while the receiver is down, and when the send queue is full` | 26 | none | Startup, with `--no-spool`. |
| `dnstap spool deleted its oldest segment unsent` | 14 | `frames`, `bytes` | Size cap or free-space floor removed unsent data. |
| `dnstap spool deleted segments it cannot read at startup: segments of an earlier version or of another spool key` | 14 | `segments`, `bytes`, `earlier_format`, `other_key`, `new_key` | Startup, at most once. `earlier_format`: plaintext or unreadable. `other_key`: another key. `new_key` `true`: all segments. |
| `dnstap spool discarded unreadable segment data` | 14 | `frames`, `bytes` | Unreadable data or a record failed authentication; not replayed. |
| `dnstap spool is refusing blocks: the volume is below the free-space floor and no older segment is left to delete` | 14 | `free_mib` (rounded down), `needed_mib` (rounded up), `pause_s` | Volume below `--spool-min-free-mib` plus one segment; only the newest segment left. |

#### Receiver, schema, and rate messages

The [spool repeat rule](#spool-messages) applies unless a row says otherwise. Reconnect
logging: [Receiver connection](configuration.md#receiver-connection).

| Message | ID | Fields | When |
| --- | --- | --- | --- |
| `dnstap receiver unavailable at startup; capture is running, and frames go to the spool, or are dropped and counted when no spool is in use, until it connects (retrying with backoff)` | 24 | `receiver`, `error` | Startup, first connection failed at the network level. |
| `dnstap receiver connection lost; frames go to the spool, or are dropped and counted when no spool is in use, until it reconnects (frames already sent are never resent)` | 24 | `receiver`, `error`, `retry_in_ms` | An established session ended. |
| `dnstap reconnect failed` | 3 | `receiver`, `error`, `cause`, `failures` | `cause`: `connect`, `timeout`, `tls`, or `protocol` (Frame Streams handshake). `failures`: run total. |
| `dnstap receiver failed the TLS handshake before the first session; check that the Windows trust store trusts the receiver certificate, that --tls-server-name (or the receiver IP address when it is not set) matches the certificate, and that the receiver does not require a client certificate (this client sends none); retrying with backoff` | 4 | `receiver`, `error`, `cause`, `failures` | Once per run: a startup outage ends in TLS rejection. Replaces `dnstap reconnect failed`. |
| `dnstap receiver refused the Frame Streams handshake before the first session; check that the receiver accepts the dnstap content type; retrying with backoff` | 4 | `receiver`, `error`, `cause`, `failures` | Same, for a Frame Streams rejection. |
| `could not start the dnstap reconnect thread; retrying with backoff` | 3 | `receiver`, `error`, `failures` | Thread didn't start. |
| `dnstap reconnect thread ended without a result; retrying with backoff` | 4 | `receiver`, `failures` | Thread ended without a result. |
| `DNS event schema load failed; its events are rejected until the load is retried` | 3 | `event_id`, `version`, `failures`, `retry_after_events` (65,536) | Schema not returned; a `TdhGetEventInformation` Win32 error follows. Per (event ID, version) key. |
| `DNS event schema is unsupported and its events are rejected` | 3 | `event_id`, `version` | Once per key, no retry. Repeats after the 8-entry rejected-key list clears or a retry fails as unsupported. |
| `rejected ETW event; the first four and then every 60th are logged, all are counted` | 3 | `error`, `rejected` (running count) | Parser rejected an event. |
| `received rate is above 22,000 events/s; the default buffer settings were validated at about 20,000 events/s (10,000 queries per second); if ring_dropped or etw_lost increase, use the settings for your rate in the "Recommended settings for higher rates" section of docs/guide/tuning.md, for example --etw-buffer-kib 128 --etw-max-buffers 128 for 50,000 queries per second` | 3 | `events_per_second` | Once per run, default buffers, after the first window above 22,000 events per second. [Rate warning](troubleshooting.md#rate-warning). |

#### Loss and risk warnings

- Checked on the stats tick (every `--stats-interval-seconds`, or 60 seconds when `0`), after
  the ETW loss query. At most one event per ID per check.
- A loss event lists only counters that rose: `<counter>_increase` (since the last check),
  `<counter>` (run total), and `seconds` covered (the first check: since capture started).
- Risks are edge-triggered: a warning at start, ID 2 at the end. A percentage risk clears 10 points below its raise level.
- A loss reported as it happens (a spool eviction) is written at once, under the repeat rule.
- After the stop, only `capture stopped`, `output stopped`, and [Shutdown results](#shutdown-results) are written.

#### Loss warnings

Every row has `seconds`. Causes: [Event IDs for data loss](troubleshooting.md#event-ids-for-data-loss).

| Message starts with | ID | Fields | Fix |
| --- | --- | --- | --- |
| `data loss in ETW` | 10 | `etw_lost_increase`, `etw_lost`, `etw_buffers_lost_increase`, `etw_buffers_lost` | Raise `--etw-max-buffers` and `--etw-buffer-kib`; check CPU starvation ([tuning.md](tuning.md#defaults)). |
| `data loss at capacity: the event channel was full` | 11 | `ring_dropped_increase`, `ring_dropped`, `capacity` | Raise `--capacity`. |
| `data loss at capacity: no pool slot was free` | 12 | `pool_exhausted_regular_increase`, `pool_exhausted_regular`, `pool_exhausted_jumbo_increase`, `pool_exhausted_jumbo`, `jumbo_capacity` | Raise `--capacity` (regular) or `--jumbo-capacity` (jumbo). |
| `data loss on the network path` | 13 | `disconnected_dropped_increase`, `disconnected_dropped`, `queue_dropped_increase`, `queue_dropped` | Check the receiver, network, `spool_available`. [Down](troubleshooting.md#the-receiver-is-down-or-unreachable), [slow](troubleshooting.md#the-receiver-is-slow). |
| `data loss in the spool` | 14 | `spool_dropped_increase`, `spool_dropped`, `spool_dropped_bytes_increase`, `spool_dropped_bytes` | Free disk space or raise `--spool-max-mib` ([Spool sizing](tuning.md#spool-sizing)). |
| `data loss in decoding` | 15 | `rejected_increase`, `rejected`, `schema_rejected_increase`, `schema_rejected` | Read the ID 3 reject and schema lines. [Rejected events](troubleshooting.md#rejected-events). |
| `data loss inside the service` | 16 | `callback_errors_increase`, `callback_errors`, `receiver_disconnected_increase`, `receiver_disconnected`, `output_failed_events_increase`, `output_failed_events` | Report it with the steps that led to it. [Restarts](troubleshooting.md#the-service-keeps-restarting). |

#### Risk warnings

| Message starts with | ID | Fields | Raised when | Cleared by (ID 2) | Fix |
| --- | --- | --- | --- | --- | --- |
| `the event channel is near full` | 20 | `channel_peak`, `capacity`, `percent` | Sampled peak depth since the last check at least 80 percent of `--capacity`. Clears after two checks in a row below 70 percent. | `the event channel is no longer near full` | Raise `--capacity`. |
| `the ETW buffers are near the maximum` | 21 | `etw_buffers_allocated`, `etw_buffers_maximum`, `percent` | Allocated at least 90 percent of the applied maximum (needs a working ETW query). Clears below 80 percent. | `the ETW buffers are no longer near the maximum` | Raise `--etw-max-buffers` ([minimums](tuning.md#etw-buffer-minimums)). |
| `the send queue is near full` | 22 | `send_queue_depth_max`, `send_queue_blocks` (256), `percent` | `send_queue_depth_max` at least 80 percent: 205 of 256 blocks. Clears below 70 percent. | `the send queue is no longer near full` | Slow spool disk or starved router ([slow](troubleshooting.md#the-receiver-is-slow)). |
| `dnstap link is saturated: the spool backlog has grown steadily over the last 10 seconds while the receiver is connected, ...` | 23 | `backlog_bytes`, `oldest_age_ms`, `bytes_sent_per_second` (10-sample average); the clear adds `reason` | [The saturated state](#the-saturated-state) sets; once per change. | `dnstap link is no longer saturated` | [Slow](troubleshooting.md#the-receiver-is-slow). |
| [Receiver outage lines](#receiver-schema-and-rate-messages) | 24 | As listed | At outage start, not on the tick. With a spool, frames wait on disk. | `dnstap receiver reconnected` | [Down](troubleshooting.md#the-receiver-is-down-or-unreachable). |
| `the dnstap receiver is down and no spool takes frames` | 24 | `receiver_connected` (`false`), `spool_available` (`false`) | Both `false` at a check. **Frames are being lost** (ID 13). Once per outage. | `frames are no longer dropped for a receiver outage` | Restore the receiver; run with a spool. |
| `the spool is near its limit` | 25 | `spool_mib`, `spool_max_mib`, `free_mib` (absent if the probe fails), `needed_mib` (minimum plus 1 segment) | Spool at least 80 percent of `--spool-max-mib`, or free space under `--spool-min-free-mib` plus 2 segments. | Below 70 percent and 3 segments above the minimum: `the spool is no longer near its limit` | Free disk space or raise `--spool-max-mib` ([Spool sizing](tuning.md#spool-sizing)). |
| [Spool messages](#spool-messages) with ID 26 | 26 | As listed | Once per run. Any receiver outage loses frames. | none | Remove `--no-spool`; fix the folder or key. |
| `dnstap spool I/O error` | 27 | `error` | A spool file operation failed: spool pauses 10 seconds and may refuse blocks (ID 14). Written at once. | none | Check the disk, folder locks, and ACL. |
| `the ETW statistics query is failing` | 28 | `etw_stats_errors_increase`, `etw_stats_errors` | A query failed and the last one didn't succeed. IDs 10 and 21 can't fire. | `the ETW statistics query works again` | Check the session runs. [ETW warnings](troubleshooting.md#etw-consumer-and-session-warnings). |

#### Capture counters

Fields of `capture stats` and `capture stopped`.

| Counter | Meaning |
| --- | --- |
| `received` | DNS records (event IDs 256, 257, 258) past the process check, drops and rejects included. |
| `dequeued` | Taken from the channel by the parser. |
| `encoded` | Handed to the sink, including frames later discarded while disconnected. |
| `ignored` | Dequeued, not a supported DNS event. |
| `rejected` | Dequeued, malformed or invalid; includes `schema_rejected`. |
| `intake_dropped` | Lost at admission ([sum](#counter-relationships)). |
| `ring_dropped` | Newest event dropped, channel full. |
| `pool_exhausted_regular`, `pool_exhausted_jumbo` | No free slot. Jumbo (payload over 2 KiB) also when `--jumbo-capacity` is `0`. |
| `receiver_disconnected` | Parser end of the event channel gone (not TCP). |
| `etw_lost` | Events ETW reports lost, as of the last successful query or stop. |
| `etw_buffers_lost` | ETW buffers (not events) lost. |
| `callback_errors` | Records the callback couldn't copy or hand off. Outside `intake_dropped`. |
| `schema_rejected` | Schema failed to load or compile. |
| `etw_stats_valid`, `etw_stats_errors` | Last ETW query or stop succeeded; failed queries, counter queries included. |
| `stale_sessions_reclaimed` | Leaked sessions stopped at startup. |
| `etw_buffers_delivered` | ETW buffer callbacks completed. |
| `parser_wake_requests` | Buffers that enqueued an event; one wake request each. |
| `cpu_etw_pct`, `cpu_parser_pct` | `capture stats` only. [Stage CPU](#stage-cpu). |

#### Output counters

Fields of `output stats` and `output stopped`; [Shutdown results](#shutdown-results) are on `output stopped` only.

| Counter | Meaning |
| --- | --- |
| `output_failed_events` | Dequeued records whose frame couldn't be queued after the sender stopped. |
| `output_errors` | One per `output_failed_events`, or a failed queue flush. |
| `receiver_connected` | Flag: a Frame Streams session exists. |
| `receiver_disconnects` | Sessions lost: socket error, or no bytes accepted for `--receiver-stall-seconds` while data waited. |
| `receiver_reconnects` | Successful reconnects after the first session. |
| `receiver_reconnect_failures` | Failed connect, handshake, or protocol; reconnect threads that didn't start or ended without a result. |
| `disconnected_dropped` | Frames lost in the sink; outside `intake_dropped`. See Gotchas. |
| `slow_writes`, `write_max_us` | Log only, since start: blocks whose socket wait reached 5 ms; longest block wait, µs. |
| `write_max_recent_us` | Gauge: longest block wait, last 60 s, µs. |
| `router_wait_max_recent_us` | Gauge: longest router wait for work, last 60 s, µs. |
| `router_pass_max_recent_us` | Gauge: longest router pass outside its wait, last 60 s, µs. |
| `bytes_sent` | Bytes the socket accepted, live and replayed; no control frames. |
| `queue_depth_max` | Deepest event-channel depth the parser sampled (idle polls, every 256th event). |
| `send_queue_depth_max` | Gauge: peak blocks (of 256, about 64 KiB each) in the send queue, last 60 s. |
| `spool_oldest_age_ms` | Gauge: ms since the oldest unsent segment was created; 0 with no backlog. |
| `link_saturated` | Flag: [saturated state](#the-saturated-state). `false` with no spool or after key loss. |
| `queue_dropped` | Discarded, send queue full. Also in `encoded`. |
| `spooled` | Stored for later sending. |
| `replayed` | Spool frames, leftovers included, the socket fully accepted; counted once. |
| `spool_dropped`, `spool_dropped_bytes` | Frames, or file bytes, removed, skipped, or refused unsent. |
| `spool_errors` | Every spool I/O error. |
| `spool_bytes`, `spool_segments` | Gauges: file bytes and segment files now, including files pending removal. |
| `spool_available` | Flag: the spool accepts data. |
| `cpu_sender_pct`, `cpu_link_pct` | `output stats` only. [Stage CPU](#stage-cpu). |

#### Shutdown results

| Field | Meaning |
| --- | --- |
| `stop_sent` | `true`: Frame Streams STOP written (FINISH not required). Exit code is 0 either way. |
| `spool_backlog_kept` | `true`: unsent frames stay for the next start, even after a failed seal. |
| `spool_seal_error` | Present when the active segment couldn't be sealed; the next start replays it. |

An abnormal stop can also write `ETW consumer did not return within N s of STOP; closing the trace handle`
(warning), `ETW cleanup failed` (ID 4), and, in diagnostic builds, `parser timing`.

```text
INFO output stopped output_failed_events=0 output_errors=0 receiver_connected=false receiver_disconnects=1 ... link_saturated=false stop_sent=false spool_backlog_kept=true
```

#### Counter relationships

Exact in the final report (written after the parser joins); live samples can briefly disagree.

```text
intake_dropped = ring_dropped + pool_exhausted_regular + pool_exhausted_jumbo + receiver_disconnected
received       = intake_dropped + callback_errors + dequeued + queue backlog
dequeued       = encoded + ignored + rejected + output_failed_events
capture bytes  ≈ bytes_sent rate + spool_bytes growth + spool_dropped_bytes rate
```

#### Performance counters

Provider and counter set `EtwDnstap`, manifest [`packaging/perfcounters/etw-dnstap.man`](../../packaging/perfcounters/etw-dnstap.man),
installed as `C:\Program Files\EtwDnstap\etw-dnstap.man` and registered with `lodctr`
([MSI or by hand](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute)).
Provider GUID `{A7F0B1F4-C87D-4267-979F-594A4AB2145D}`, counter set GUID `{B02EDFA2-B961-4E06-89D2-56C95ECB336C}`.
One instance per ETW session name (default `EtwDnstap`; `--session-name` in [Options](configuration.md#options)),
only while capture runs: `\EtwDnstap(<instance>)\<counter>`. Names and help text live in `etw-dnstap.exe`.
Read with Performance Monitor, `typeperf`, `logman`, or WMI (`Win32_PerfRawData_*`). All values
64-bit, scale 0; flags 1 or 0. Advanced detail level: 14, 15, 19, 20, 51, 56, 59, 77 to 80.

##### Throughput rates

| ID | Counter | Type | Unit | Log field |
| --- | --- | --- | --- | --- |
| 1 | Events Received/sec | `PERF_COUNTER_BULK_COUNT` | events/s | `received` |
| 2 | Frames Sent/sec | `PERF_COUNTER_BULK_COUNT` | frames/s | `encoded` |
| 3 | Events Dropped/sec | `PERF_COUNTER_BULK_COUNT` | events/s | `intake_dropped` |
| 62 | Bytes Sent/sec | `PERF_COUNTER_BULK_COUNT` | bytes/s | `bytes_sent` |

##### Totals since the service started

All `PERF_COUNTER_LARGE_RAWCOUNT`.

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

##### Gauges

An empty Type cell means `PERF_COUNTER_LARGE_RAWCOUNT`.

| ID | Counter | Type | Unit | Log field or meaning |
| --- | --- | --- | --- | --- |
| 30 | Channel Depth | | events | Events the parser hasn't taken (derived) |
| 31 | Channel Capacity | | events | `--capacity` |
| 32 | Channel Peak Depth | | events | `queue_depth_max` |
| 33 | % Channel Used | `PERF_LARGE_RAW_FRACTION` | percent | Depth over Capacity |
| 34 | % Channel Used Base (hidden) | `PERF_LARGE_RAW_BASE` | events | |
| 40 | ETW Buffers Allocated | | buffers | Allocated for the session |
| 41 | ETW Buffers In Use | | buffers | Allocated, not reported free; never below 0 |
| 42 | ETW Buffers Maximum | | buffers | Maximum ETW reports |
| 43 | % ETW Buffers Used | `PERF_LARGE_RAW_FRACTION` | percent | In Use over Maximum |
| 44 | % ETW Buffers Used Base (hidden) | `PERF_LARGE_RAW_BASE` | buffers | |
| 50 | Receiver Connected | | flag | `receiver_connected` |
| 51 | Receiver Reconnects | | reconnects | `receiver_reconnects` |
| 58 | Spool Bytes | | bytes | `spool_bytes` |
| 59 | Spool Segments | | segments | `spool_segments` |
| 60 | Spool Available | | flag | `spool_available` |
| 63 | Send Queue Peak Depth | | blocks | `send_queue_depth_max` |
| 64 | Socket Write Peak (microseconds) | | microseconds | `write_max_recent_us` |
| 65 | Spool Oldest Age (milliseconds) | | milliseconds | `spool_oldest_age_ms` |
| 66 | Link Saturated | | flag | `link_saturated` |
| 67 | Router Wait Peak (microseconds) | | microseconds | `router_wait_max_recent_us` |
| 68 | Router Pass Peak (microseconds) | | microseconds | `router_pass_max_recent_us` |
| 69 | Stage CPU ETW Consumer | `PERF_LARGE_RAW_FRACTION` | percent | `cpu_etw_pct` |
| 70 | Stage CPU ETW Consumer Base (hidden) | `PERF_LARGE_RAW_BASE` | none | |
| 71 | Stage CPU Parser | `PERF_LARGE_RAW_FRACTION` | percent | `cpu_parser_pct` |
| 72 | Stage CPU Parser Base (hidden) | `PERF_LARGE_RAW_BASE` | none | |
| 73 | Stage CPU Sender | `PERF_LARGE_RAW_FRACTION` | percent | `cpu_sender_pct` |
| 74 | Stage CPU Sender Base (hidden) | `PERF_LARGE_RAW_BASE` | none | |
| 75 | Stage CPU Link | `PERF_LARGE_RAW_FRACTION` | percent | `cpu_link_pct` |
| 76 | Stage CPU Link Base (hidden) | `PERF_LARGE_RAW_BASE` | none | |
| 77 | Stage CPU Time ETW Consumer (milliseconds) | | milliseconds | Kernel plus user CPU since start |
| 78 | Stage CPU Time Parser (milliseconds) | | milliseconds | Same |
| 79 | Stage CPU Time Sender (milliseconds) | | milliseconds | Same |
| 80 | Stage CPU Time Link (milliseconds) | | milliseconds | Same |

#### Stage CPU

Percent of one logical processor over the last second (log: since the previous stats line, one
decimal). Near 100 marks the limiting stage.

| Thread | Work | Counter ID | Log field |
| --- | --- | --- | --- |
| `etw-consumer` | ETW callback, intake | 69 | `cpu_etw_pct` |
| `dnstap-parser` | Parsing | 71 | `cpu_parser_pct` |
| `dnstap-sender` | Router | 73 | `cpu_sender_pct` |
| `dnstap-link` | Socket writes, TLS | 75 | `cpu_link_pct` |

#### Suggested alerts

| Alert | Threshold | Action |
| --- | --- | --- |
| `% Channel Used` | above 75 | [ID 20](#risk-warnings) |
| `% ETW Buffers Used` | above 75 | [ID 21](#risk-warnings) |
| `Events Dropped`, `ETW Events Lost`, `Receiver Down Drops`, `Send Queue Drops`, `Spool Dropped Frames` | any increase (normally 0) | [Loss warnings](#loss-warnings) |
| Application log Event ID | 10 to 16, 20 to 28 | [Loss](#loss-warnings), [risk](#risk-warnings) |
| `Receiver Connected` | 0 | [Receiver down](troubleshooting.md#the-receiver-is-down-or-unreachable) |
| `Spool Available` | 0 | [Spool warnings](troubleshooting.md#spool-warnings) |
| `Spool Bytes` | near `--spool-max-mib` during an outage | [Spool sizing](tuning.md#spool-sizing) |
| `Link Saturated` | 1 | [Receiver slow](troubleshooting.md#the-receiver-is-slow); `Spool Oldest Age` is the delay |

### Common Gotchas

- **Filter on level or ID, not Event ID 3.** IDs 10 to 28 are all Warnings.
- **Untested thresholds.** The 75 levels come from 200,000 queries-per-second runs, where ring
  drops and `etw_lost` followed a full channel and maxed buffers. A 2026-11-01 DNSTap-WS2025 run
  matched `capture stopped` totals; non-admin, remote, WMI, and CPU cost are untested
  ([lab results](../history/guide-lab-results-2026-11.md#60-second-counter-run-at-10000-queries-per-second)).
- **Some loss is never counted.** Bytes the kernel accepted before a connection failed (they're
  in `bytes_sent`). `dnstap receiver connection lost` marks each failure. Only the collector's
  count proves delivery.
- **`disconnected_dropped`.** No spool: the cut frame, the rest of the block in hand, the link
  queue, and every live frame while down. With a spool: only the live block handed back at a
  shutdown after a spill. In the TLS 1.3 key update window the receiver may hold a counted frame.
- **Don't double count.** IDs 11 and 12 are inside `intake_dropped`; Events Dropped (12) includes
  13, 14 (which includes 15), and `receiver_disconnected`. A frame is in at most one of
  `disconnected_dropped`, `queue_dropped`, `spool_dropped`. Total loss: 12 + 16 + 18 + 52 + 55 (17 counts buffers).
- **Frames that count nowhere.** Frames in the send queue at an abnormal sender stop; the
  parser counts the failed write in `output_failed_events`.
- **Spool accounting.** Leftovers count in `replayed`, not `encoded`; a leftover segment removed
  unsent adds only bytes. A cut-off final record adds one to `spool_dropped`. Spool bytes include
  a 32-byte segment header and 28 bytes per record (one per 64 KiB block).
- **Spool losses include** eviction, corrupt data, undecryptable segments, pauses, I/O errors,
  and every append after a lost key (reads stop, files stay, live frames still flow; restart to replay).
- **`spool_available` is `false`** with `--no-spool`, an unusable folder, or an I/O-error or free-space pause.
- **Frames Sent excludes replays.** For the capture byte rate, add `spool_bytes` growth to Bytes Sent/sec.
- **`bytes_sent` overhead.** Plaintext: 4-byte length plus payload. TLS: record headers and tags, about 22 to 29 bytes per 16 KiB.
- **Bounds.** `queue_depth_max` and `send_queue_depth_max` are lower bounds; `spool_oldest_age_ms`
  is an upper bound (earlier-run segments use the file creation time).
- **Deep send queue, not a slow receiver.** Blocks go to the 256-block (about 16 MiB) link queue
  and spill to the spool only when it's full. Suspect a slow spool disk, a starved router, or a late ETW burst.
- **Router gauges.** A wait ends within 50 ms, so `router_wait_max_recent_us` far above 50,000
  means a late wake (CPU contention, held process). A large pass means a blocking call (disk write, hand-back wait).
- **`write_max_us` covers the whole run**, including a wait in progress; check `write_max_recent_us` for the current receiver.
- **Receiver Reconnects (51) also counts a startup connection**; the log field doesn't.
- **A slow receiver keeps its session.** Only a socket error or the stall timeout ends it.
- **Counter timing.** IDs 30, 31, 33, 34, 40 to 44, 50, 60, 66, and 69 to 80 update once per
  second; the rest are exact at any interval. ETW gauges read 0 until the first good query, then
  keep the last value; ID 28 means they're stale. ETW Buffers Maximum can exceed `--etw-max-buffers`.
- **Stage CPU can read a point or two above 100** (clock-tick accounting). Ended threads read 0. Sample at 1 second or more.
- **`stop_sent` `false`** means no session, a cut data frame, a failed STOP write, a failed return to blocking mode, or a failed final flush.

#### Windows of the recent peaks

Rule: `write_max_recent_us`, `router_wait_max_recent_us`, `router_pass_max_recent_us`, and
`send_queue_depth_max` are the largest of the last 60 one-second samples, published by the
router thread; the log and the counters read the same value. Values lag up to a second; a slow
disk append can stretch the window past a minute.

Example: with `--stats-interval-seconds 300`, a peak more than 60 seconds before a stats line
never shows in the log.

#### The saturated state

The router samples the unread backlog (spool bytes not yet handed to the link) at least 1
second apart and keeps 11 samples.

- **Set:** backlog larger than 10 samples ago, at least 7 of 10 steps rose (flat 64 KiB replay
  batches are tolerated), receiver connected. ID 23.
- **Clear:** backlog empty (at once) or not larger than 10 samples ago. ID 2, `reason`:
  `the backlog is empty`, `the backlog stopped growing`, or `the spool is suspended until the service restarts` (no re-set this run).
- **Held:** while the receiver is down, a spool I/O error pauses reads, or a session was lost
  since the last sample. Read with `receiver_connected`.
- At most one event per 10 samples. Report only; routing doesn't change. No spool: watch `send_queue_depth_max` and `queue_dropped`.

Example: a receiver stops reading without closing. It looks saturated until the stall timeout
(default 15 seconds) ends the session; the lab saw the flag set 11.4 to 12 seconds in
([lab validation](../history/slow-receiver-validation.md)).

#### If the counters do not register

`typeperf -q EtwDnstap` lists nothing: register the manifest with `lodctr`
([install.md](install.md#3-copy-the-files-register-the-service-and-counters-and-grant-read-and-execute))
and restart the service. After an upgrade, [replace the counter manifest](upgrade.md#replace-the-counter-manifest).

A failed provider call at startup writes one ID 3 warning, `performance counters are unavailable; capture continues without performance counters`,
whose `error` names the call and Win32 status.
