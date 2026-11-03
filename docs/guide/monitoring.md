# Log events and counters

> **Status:** Current · **Describes:** `main` at commit `dde7188` with the Windows performance counter provider · **Reviewed:** 2026-11-02

Diagnostics go to the Application event log under source `EtwDnstap` and to
stderr. The service writes no capture, metrics, or log files. The Windows
performance counters show many of the same values. See
[Performance counters](#performance-counters).

## Log event IDs

| Event ID | Type | Written for |
| --- | --- | --- |
| 1 | Error | A fatal service failure, whether at startup or during the run. The text begins `EtwDnstap failed:`. |
| 2 | Information | Every information-level line: `capture started`, `capture stats`, `output stats`, `capture stopped`, `output stopped`, `dnstap receiver reconnected`, `ETW buffers applied`, and `diagnostic settings applied` in diagnostic builds. |
| 3 | Warning | Every warning-level line, such as an unavailable receiver, a lost connection, a failed reconnect, a rejected event, the rate warning, and a reclaimed session. |
| 4 | Error | An error-level line that does not by itself stop the service, such as the reason for a DNS Server change exit. |
| 5 | Error | Watchdog expiry, written just before the process aborts. |

## Periodic and final reports

Every `--stats-interval-seconds`, the service writes two information events,
`capture stats` and `output stats`. Each snapshot first asks ETW for its own
loss counters. The shutdown always writes `capture stopped` and `output stopped`
with the same fields, even when periodic snapshots are disabled.

`capture started` records the session name, the session GUID (`session_guid`), receiver, `tls`
(`true` unless `--plaintext` is given), `tls_server_name` when set, channel capacity, ETW
buffer size, the requested minimum and maximum buffers (`etw_min_buffers` and
`etw_max_buffers`), flush seconds, the keyword masks
as `match_any_keyword` and `match_all_keyword` in hexadecimal, and
`keyword_fallback`, and `failed_responses_captured`.

`ETW buffers applied` follows `capture started` once. It records the minimum and
maximum buffer counts and the allocated buffer count that ETW reported after the
session started (`etw_min_buffers_applied`, `etw_max_buffers_applied`, and
`etw_buffers_allocated`). The service writes it only when the ETW query
succeeds. See [ETW buffer minimums](tuning.md#etw-buffer-minimums).

## Capture counters

These fields appear on `capture stats` and `capture stopped`.

| Counter | Meaning |
| --- | --- |
| `received` | Records of event IDs 256, 257, and 258 that the callback accepted after the process check. Includes records later dropped or rejected. Event 258 is counted here since the response failure change. |
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
| ETW Buffers Maximum | 42 | The maximum buffer count that ETW reports. It can be higher than `--etw-max-buffers`. See [ETW buffer minimums](tuning.md#etw-buffer-minimums). | None |
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


## Next steps

- [Troubleshooting](troubleshooting.md)
- [Measurement terms](../design/measurement-terms.md)
