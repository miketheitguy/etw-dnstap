# Log events and counters

> **Status:** Current · **Describes:** `tcp-sink-spool` at commit `0569136` · **Reviewed:** 2026-11-03

Diagnostics go to the Application event log under source `EtwDnstap` and to
stderr. The service writes no capture or metrics files. The one exception is the
spool, which keeps frames that were not sent yet in segment files. See
[Spool](configuration.md#spool). The Windows performance counters show many of
the same values. See
[Performance counters](#performance-counters).

## Log event IDs

| Event ID | Type | Written for |
| --- | --- | --- |
| 1 | Error | A fatal service failure, whether at startup or during the run. The text starts with `EtwDnstap failed:`. |
| 2 | Information | Every information-level line: `capture started`, `capture stats`, `output stats`, `capture stopped`, `output stopped`, `dnstap receiver reconnected`, `ETW buffers applied`, `spool in use`, `dnstap spool backlog kept on disk for the next run`, and `diagnostic settings applied` in diagnostic builds. |
| 3 | Warning | Every warning-level line, such as an unavailable receiver, a lost connection, a failed reconnect, a rejected event, the rate warning, a reclaimed session, and the spool warnings in [Spool messages](#spool-messages). |
| 4 | Error | Every error-level log line, such as `ETW cleanup failed` or the reason for a stop after a DNS Server process change. A fatal service failure uses Event ID 1. |
| 5 | Error | Watchdog expiry, written just before the process aborts. |

## Periodic and final reports

Every `--stats-interval-seconds`, the service writes two information events,
`capture stats` and `output stats`. Each snapshot first asks ETW for its own
loss counters. The shutdown always writes `capture stopped` and `output stopped`
with the same fields, even when you disable periodic snapshots.

`capture started` records these fields:

- `session` (the ETW session name), `session_guid`, and `receiver`.
- `tls` (`true` unless you set `--plaintext`) and `tls_server_name` when set.
- `capacity`, `etw_buffer_kib`, and `etw_flush_seconds`.
- The requested minimum and maximum buffers (`etw_min_buffers` and
  `etw_max_buffers`).
- The keyword masks `match_any_keyword` and `match_all_keyword`, in hexadecimal.
- `keyword_fallback` and `failed_responses_captured`.

`ETW buffers applied` follows `capture started` once. It records the minimum and
maximum buffer counts and the allocated buffer count that ETW reported after the
session started (`etw_min_buffers_applied`, `etw_max_buffers_applied`, and
`etw_buffers_allocated`). The service writes it only when the ETW query
succeeds. See [ETW buffer minimums](tuning.md#etw-buffer-minimums).

### Spool messages

The spool writes these lines. Each line carries the fields in the table. None
of them logs frame data.

| Message | Event ID | Fields | When |
| --- | --- | --- | --- |
| `spool in use` | 2 | `spool_dir`, `spool_max_mib`, `spool_min_free_mib`, `leftover_segments`, `leftover_bytes` | At startup, when the spool opens. The leftover fields show what an earlier run left. |
| `spool folder is not usable; capture continues without a spool, so frames are dropped and counted while the receiver is down` | 3 | `spool_dir`, `reason` | At startup, when the folder check fails. The service runs without a spool. |
| `spool could not be opened; capture continues without a spool, so frames are dropped and counted while the receiver is down` | 3 | `spool_dir`, `error` | At startup, when the folder check passes but the spool cannot open, for example because `cursor.bin` cannot be read. The service runs without a spool. |
| `dnstap spool deleted its oldest segment unsent` | 3 | `frames`, `bytes` | When the size cap or the free-space floor deletes a segment that still held unsent data. The first four, then every 60th. |
| `dnstap spool discarded unreadable segment data` | 3 | `frames`, `bytes` | Segment data could not be read: a corrupt or cut-off tail, a missing START, a read error after retries, or an invalid cursor that dropped the oldest segment. The first four, then every 60th. |
| `dnstap spool I/O error` | 3 | `error` | A spool file operation failed. The spool pauses for 10 seconds. The first four, then every 60th. `spool_errors` counts every error. |
| `could not seal the dnstap spool segment` | 3 | `error` | At shutdown, when the active segment cannot take its STOP. A later run reads the segment as an unsealed leftover. |
| `dnstap spool backlog kept on disk for the next run` | 2 | none | At shutdown, when unsent frames remain in the spool. |

## Capture counters

These fields appear on `capture stats` and `capture stopped`.

| Counter | Meaning |
| --- | --- |
| `received` | Records of event IDs 256, 257, and 258 that the callback accepted after the process check. Includes records that the service later drops or rejects. |
| `dequeued` | Records the parser took from the channel. |
| `encoded` | Records the parser encoded and handed to the sink. Includes frames the sink later discarded while disconnected, so it is not a delivery count. |
| `ignored` | Dequeued records the parser did not capture because they are not a supported DNS event. |
| `rejected` | Dequeued records the parser rejected, such as a malformed packet, an invalid field, or a schema failure. Includes `schema_rejected`. |
| `ring_dropped` | Records dropped because the channel was full. Part of `intake_dropped`. |
| `intake_dropped` | Every record lost at admission: `ring_dropped`, `pool_exhausted_regular`, `pool_exhausted_jumbo`, and `receiver_disconnected`. |
| `pool_exhausted_regular` | Regular-class records dropped because no regular pool slot was free. Part of `intake_dropped`. |
| `pool_exhausted_jumbo` | Jumbo-class records (padded payload over 2 KiB) dropped because no jumbo pool slot was free, or because `--jumbo-capacity` is `0`. Part of `intake_dropped`. |
| `foreign_pid_rejected` | Records from a process other than the DNS Server. Not counted in `received`. Normally zero. See [DNS process filter](configuration.md#dns-process-filter). |
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

## Output counters

These fields appear on `output stats` and `output stopped`.

| Counter | Meaning |
| --- | --- |
| `output_failed_events` | Dequeued records whose frame could not be queued for the sender, because the sender thread had stopped. Otherwise 0. |
| `output_errors` | Parser output failures. Increments when a frame cannot be queued for the sender thread, because the sender thread has stopped (and so for each `output_failed_events`). Also increments when a flush of the queue fails, when `finish` reports that the sender thread panicked, and when the STOP/FINISH exchange at shutdown fails. |
| `receiver_connected` | `true` while a Frame Streams session exists. A flag, not a count. |
| `receiver_disconnects` | Established sessions that were lost. |
| `receiver_reconnects` | Successful reconnects after the first session. When the receiver is down at startup, the first connection counts here too. |
| `receiver_reconnect_failures` | Reconnect attempts that failed at connect, handshake, or protocol. |
| `disconnected_dropped` | Frames lost in the sink: frames in flight when a connection failed, such as frames written since the last successful flush, and frames that arrived while the receiver was down and no spool was in use. Frames in flight may have reached the receiver. A frame in a failed replay batch is counted here too, after `replayed`. Never resent. Not part of `intake_dropped`. |
| `slow_writes` | Socket writes that took at least 5 ms. |
| `write_max_us` | Longest single socket write in microseconds. |
| `queue_depth_max` | Deepest channel depth that the parser sampled, counting the event it was processing. The parser samples on the first event after each idle poll and on every 256th event, so the value is a lower bound. |
| `queue_dropped` | Frames discarded on the parser thread because the send queue was full. These frames are also in `encoded`. |
| `spooled` | Frames appended to the spool. |
| `replayed` | Frames read from the spool and handed to the receiver connection. Includes frames left by an earlier run. A frame in a failed replay batch is also in `disconnected_dropped`. |
| `spool_dropped` | Frames removed from the spool unsent, or refused by it: eviction at the size cap or the free-space floor, a cut-off or corrupt tail, a pause, or an I/O error. |
| `spool_dropped_bytes` | Bytes removed or skipped unsent, control frames included. Includes leftover segments whose frame count is unknown. |
| `spool_errors` | Spool I/O errors. |
| `spool_bytes` | Gauge: bytes in segment files now, including files waiting for a delete to succeed. |
| `spool_segments` | Gauge: segment files now, including files waiting for a delete to succeed. |
| `spool_available` | Flag: `true` while a spool is in use and accepting data. `false` with `--no-spool`, with an unusable folder, and while the spool is paused by an I/O error or by the free-space floor. |

## Counter relationships

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
- `disconnected_dropped`, `queue_dropped`, and `spool_dropped` are the loss
  counters of the sink. Each frame of a run is in at most one of them.
  `spooled`, `replayed`, and `spool_dropped_bytes` are not loss counters.
  `disconnected_dropped` is not part of `intake_dropped` or of the identities
  above. Its frames were already counted in `encoded`.
- Each frame that reaches the sink ends in one of these places: a write to the
  receiver connection, `queue_dropped`, `disconnected_dropped`, or `spool_dropped`
  (refused by the spool). A frame that the spool stores is counted in `spooled`.
  It then ends in `replayed`, which leads to a write or to `disconnected_dropped`.
  It can also end in `spool_dropped`, when the spool removes it unsent. A frame
  still on disk at shutdown is in `spooled` and in no loss counter. The final
  report shows the files that remain in `spool_bytes` and `spool_segments`.
- One case is in none of these counters: frames already in the send queue when
  the sender thread stops abnormally. The parser counts only the failed write, in
  `output_failed_events`.
- Leftover frames from an earlier run are in `replayed` when they are read. They
  are never in `encoded`. A leftover segment that is deleted unsent adds only
  to `spool_dropped_bytes`, because its frame count is unknown. A cut-off final
  frame in a leftover adds one to `spool_dropped`.
- `spool_dropped_bytes` has no fixed ratio to `spool_dropped`.
- No counter gives the number of frames that the receiver accepted. The
  collector's own count is the only check of delivery.
- `etw_lost` and `etw_buffers_lost` are ETW's own counters, in ETW's units. They
  are not part of these identities.
- Live samples read separate atomic counters and can disagree briefly. The
  final report, written after capture stops and the parser joins, has no
  backlog, so the identities should close exactly there.

## Performance counters

The service publishes a Windows performance counter set named `EtwDnstap`. The
lab tested it on 2026-11-01 (see below). The
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

Windows reads the rates and totals (IDs 1 to 3, 10 to 21, 32, 51, and 52 to 59) by
reference. It reads the live value each time a consumer collects, so a rate is
exact for the consumer's own sample interval. The service publishes the other gauges
(IDs 30, 31, 33, 34, 40 to 44, 50, and 60) once per second.

The ETW gauges come from an ETW statistics query. The service runs that query
once per second, which is more often than `--stats-interval-seconds`. The ETW
gauges read 0 until the first query succeeds. After that, each ETW gauge keeps
the value of the last successful query.

The counters add no work to the ETW callback or the parser loop. The project has
not measured their CPU cost.

### Catalog

Each counter has a numeric ID from the manifest. The log counter column names
the matching field in [Log events and counters](#log-events-and-counters). None
means that the log has no matching field.

#### Throughput rates

| Counter | ID | Meaning | Log counter |
| --- | --- | --- | --- |
| Events Received/sec | 1 | DNS events accepted from ETW each second. | `received` |
| Frames Sent/sec | 2 | Frames encoded and handed to the sink each second. Includes frames later lost in the sink, so this is not a delivery rate. Excludes leftover frames replayed from an earlier run. | `encoded` |
| Events Dropped/sec | 3 | Events dropped at admission each second. | `intake_dropped` |

The `/sec` counters are rates. The reader computes each rate from the change in
the counter over time.

#### Totals since the service started

| Counter | ID | Meaning | Log counter |
| --- | --- | --- | --- |
| Events Received | 10 | DNS events accepted from ETW since the service started. | `received` |
| Frames Sent | 11 | Frames encoded and handed to the sink since the service started. Includes frames later lost in the sink (Send Queue Drops, Receiver Down Drops, Spool Dropped Frames), so it is not a delivery count. Excludes leftover frames replayed from an earlier run. | `encoded` |
| Events Dropped | 12 | Events dropped at admission. It includes IDs 13 and 14. | `intake_dropped` |
| Channel Full Drops | 13 | Events dropped because the channel was full. The service drops the newest event. | `ring_dropped` |
| Pool Exhausted Drops | 14 | Events dropped because no pool slot was free, regular and jumbo together. It includes ID 15. | `pool_exhausted_regular` + `pool_exhausted_jumbo` |
| Jumbo Pool Exhausted Drops | 15 | Jumbo-class events dropped because no jumbo slot was free, or because `--jumbo-capacity` is 0. | `pool_exhausted_jumbo` |
| ETW Events Lost | 16 | Events that ETW reports as lost. | `etw_lost` |
| ETW Buffers Lost | 17 | ETW buffers lost. This is a buffer count, not an event count. | `etw_buffers_lost` |
| Receiver Down Drops | 18 | Frames lost in the sink: the connection failed with them in flight, or they arrived while the receiver was down and no spool was in use. | `disconnected_dropped` |
| Events Rejected | 19 | Dequeued events that the parser rejected. | `rejected` |
| Callback Errors | 20 | Events that the ETW callback could not copy or hand off. | `callback_errors` |
| Receiver Disconnects | 21 | Times the receiver connection was lost since the service started. | `receiver_disconnects` |
| Send Queue Drops | 52 | Frames discarded on the parser thread because the send queue was full. Also counted in Frames Sent. | `queue_dropped` |
| Frames Spooled | 53 | Frames appended to the spool since the service started. | `spooled` |
| Frames Replayed | 54 | Frames read from the spool and handed to the receiver connection since the service started. Includes leftovers from an earlier run. | `replayed` |
| Spool Dropped Frames | 55 | Frames removed from the spool unsent, or refused by it: eviction, corrupt or cut-off data, a pause, or an I/O error. | `spool_dropped` |
| Spool Dropped Bytes | 56 | Bytes removed from the spool unsent, including leftover segments whose frame count is unknown. Advanced detail. | `spool_dropped_bytes` |
| Spool Errors | 57 | Spool I/O errors since the service started. | `spool_errors` |

How the totals relate:

- Events Dropped (12) includes Channel Full Drops (13) and Pool Exhausted Drops
  (14). Jumbo Pool Exhausted Drops (15) is part of Pool Exhausted Drops (14), so
  do not add the two. Events Dropped also includes drops when the channel's
  parser end is gone. Those drops have no performance counter of their own.
- ETW Events Lost (16) and ETW Buffers Lost (17) are separate from Events Dropped.
  They are ETW's own counts.
- Receiver Down Drops (18) is separate from Events Dropped. It counts frames in
  flight when a connection failed, and frames that arrived with no spool in use.
  Frames in flight may have reached the receiver.
- Send Queue Drops (52) and Spool Dropped Frames (55) are losses in the sink,
  like Receiver Down Drops. A frame is in at most one of 18, 52, and 55.
- Frames Replayed (54) is not a loss. A frame in a failed replay batch is in 54
  and in 18.
- To count every frame and event that the service loses, add 12, 16, 18, 52,
  and 55. Leave out ETW Buffers Lost (17), which counts buffers.

#### Gauges

| Counter | ID | Meaning | Log counter |
| --- | --- | --- | --- |
| Channel Depth | 30 | Events in the channel that the parser has not taken yet. The service derives this value from the other counts. | None |
| Channel Capacity | 31 | The `--capacity` value. The `capture started` line also shows it. | None |
| Channel Peak Depth | 32 | The deepest channel depth that the parser sampled, so this is a lower bound. | `queue_depth_max` |
| % Channel Used | 33 | Channel Depth as a percentage of Channel Capacity. | None |
| (base of 33) | 34 | Hidden counter. It is the base for % Channel Used. | None |
| ETW Buffers Allocated | 40 | ETW buffers allocated for the session. | None |
| ETW Buffers In Use | 41 | Allocated buffers that ETW does not report as free. It never goes below 0. | None |
| ETW Buffers Maximum | 42 | The maximum buffer count that ETW reports. It can be higher than `--etw-max-buffers`. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). | None |
| % ETW Buffers Used | 43 | Buffers in use as a percentage of the maximum. | None |
| (base of 43) | 44 | Hidden counter. It is the base for % ETW Buffers Used. | None |
| Receiver Connected | 50 | 1 while a Frame Streams session exists. Otherwise 0. | `receiver_connected` |
| Receiver Reconnects | 51 | Successful reconnects after the first session. When the receiver is down at startup, the first connection counts here too. | `receiver_reconnects` |
| Spool Bytes | 58 | Bytes now held in spool segment files. | `spool_bytes` |
| Spool Segments | 59 | Spool segment files now on disk. Advanced detail. | `spool_segments` |
| Spool Available | 60 | 1 while a spool is in use and accepting data. 0 with `--no-spool`, with an unusable folder, and while the spool is paused by an I/O error or by the free-space floor. | `spool_available` |

The ETW gauges come from the last successful query. They read 0 until the first
query succeeds.

Spool Dropped Bytes (56) and Spool Segments (59) are advanced counters. Perfmon
shows them only at the Advanced detail level.

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
then restart the service. After an upgrade that changes the manifest, register
the new one in the same way. See
[Replace the counter manifest](upgrade.md#replace-the-counter-manifest).

If a provider call fails at startup, the service writes one warning (Event ID 3)
that names the call and the Win32 status. Capture continues without counters.

### Lab validation

The lab tested the counters on 2026-11-01 on the 6-vCPU Windows Server 2025
VM. The service ran as LocalService with a restricted service SID.

- `lodctr` registered the manifest. An administrator's `typeperf -q EtwDnstap`
  listed every counter by name.
- In a 60-second run at 10,000 queries per second, the final totals matched the
  `capture stopped` event. Events Received was 1,200,000. Channel Full Drops was
  10,751. Channel Peak Depth was 8,193. Events Received/sec read 19,700 to
  21,000 in steady seconds, which is 2 events for each query.
- The instance disappeared when the service stopped.

Not tested: a non-admin consumer, remote collection, WMI, and the CPU cost of the
publication alone.

### Suggested alerts

These alerts are suggestions. The project has not tested their thresholds on a host.

| Alert | Why |
| --- | --- |
| `% Channel Used` above 75 | In the 200,000 queries-per-second runs, ring drops followed the channel reaching `--capacity`. |
| `% ETW Buffers Used` above 75 | In those runs, `etw_lost` followed ETW buffers in use reaching the maximum. |
| Any increase in `Events Dropped`, `ETW Events Lost`, `Receiver Down Drops`, `Send Queue Drops`, or `Spool Dropped Frames` | These are the loss totals. Under normal load, each one should stay at 0. |
| `Receiver Connected` at 0 | The receiver session is down. With a spool in use, frames are stored and `Spool Bytes` rises. Without a spool, frames count in `Receiver Down Drops`. |
| `Spool Available` at 0 | No spool is in use, or the spool is paused. Frames that the receiver does not take count in `Receiver Down Drops` or `Spool Dropped Frames`. |
| `Spool Bytes` near `--spool-max-mib` during an outage | The spool is close to deleting its oldest data. Compare the size with [Spool sizing](tuning.md#spool-sizing). |


## Next steps

- [Troubleshooting](troubleshooting.md)
- [Measurement terms](../design/measurement-terms.md)
