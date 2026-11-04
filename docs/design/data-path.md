# Data path: from an ETW event to a TCP frame

> **Status:** Current · **Describes:** `tcp-sink-spool` at commit `0569136` · **Evidence dated:** not applicable (reference, from the source and the local tests) · **Reviewed:** 2026-11-03

This reference follows one DNS Server event through the service. It names the
thread, code, and counters of each stage. Maintainers use it to find the stage that
changes or loses an event. Operators use it to match a counter to a stage.

![Animation of the data path in six stages. An ETW event moves through intake, a pool slot, a bounded channel, the parser, and the sender to the TCP receiver. In the last stage the receiver is down. Frames go to the disk spool, and the spool replays them, oldest first, after the reconnect.](../assets/pipeline/etw-to-tcp-pipeline.gif)

The animation slows the events and shows illustrative counter values. It splits stage
5 in two. Tab 05 shows normal sending. Tab 06 shows a receiver outage, the spool, and
the replay. The [stage table](#stages) gives the same information as text.

## Stages

An event passes through five stages. The table names the thread and code of each.
Stage 3 spans two threads. Stage 5 splits between `dnstap-parser` and `dnstap-sender`,
and the `dnstap-reconnect` helper runs reconnects.

| # | Stage | Thread | Code |
| --- | --- | --- | --- |
| 1 | Produce | `dns.exe`, ETW | `Capture::start` |
| 2 | Intake | `etw-consumer` | `native_etw_callback`, `copy_record` |
| 3 | Buffer | Both | `PoolManager`, the bounded channel, `native_buffer_callback` |
| 4 | Parse | `dnstap-parser` | `Parser::parse`, `Encoder::encode` |
| 5 | Queue and send | `dnstap-parser`, then `dnstap-sender` | `QueuedSink`, `SenderThread::route`, `NetworkSink`, `Spool` |

Decoding happens in stage 4, after the buffer. The intake callback only copies bytes.

### 1. Produce

The service creates a real-time ETW session and enables the DNS Server provider with
a keyword mask, an event-ID filter, and a process-ID filter. The filters admit only
events 256, 257, and 258 from the DNS Server process. ETW delivers them about once per
`--etw-flush-seconds`. The
[Subscription](../guide/configuration.md#subscription) section lists the filters.

### 2. Intake

ETW calls `native_etw_callback` on the `etw-consumer` thread for each event. The
callback does these steps in order:

1. Reject a record from another provider, or with an unsupported event ID. This step
   counts nothing.
2. Reject a record whose process ID is not the DNS Server process ID. Count it in
   `foreign_pid_rejected`.
3. Count the record in `received`.
4. Copy the record into a pool slot. Count a malformed record in `callback_errors`.
   Count a record with no free slot in `pool_exhausted_regular` or
   `pool_exhausted_jumbo`.
5. Send the slot with `try_send`. A full channel drops the newest event and increments
   `ring_dropped`. A channel with no receiver drops the event and increments
   `receiver_disconnected`. Both drops also increment `intake_dropped`.

The callback never waits and never allocates. The
[pooled intake](pooled-intake.md#ownership-and-callback-lifecycle) reference explains
the ownership rules.

### 3. Buffer

The pool holds preallocated 2 KiB slots, plus `--jumbo-capacity` slots for records over
2 KiB. The channel holds at most `--capacity` slots, 24,576 by default. Neither part
grows after startup.

After each delivered ETW buffer, `native_buffer_callback` increments
`etw_buffers_delivered`. If that buffer enqueued an event, it also increments
`parser_wake_requests` and unparks the parser. The per-event callback never wakes the
parser. See [Parser wake and polling](ring-buffer.md#parser-wake-and-polling).

### 4. Parse

The parser takes slots with `try_recv`. For each slot:

1. Increment `dequeued`.
2. Decode the record with a compiled plan that reads each field once. A schema that
   does not compile, or a malformed payload, increments `rejected`. A schema failure
   also increments `schema_rejected`.
3. Encode the record as dnstap. Event 256 becomes `CLIENT_QUERY`. Events 257 and 258
   become `CLIENT_RESPONSE`. The encoder rejects a record that has any of these
   properties, and counts it in `rejected`:
   - An unsupported event ID.
   - A DNS message outside 12 to 65,535 bytes.
   - A QR bit that disagrees with the event.
   - Addresses of two families.
   - A timestamp before the Unix epoch.
   - An encoded size over 70 KiB.

   The [dnstap fields](../guide/configuration.md#message-and-transport-limits) are in
   the configuration reference.
4. Release the slot to the pool.
5. Pass the frame to the sink.

The parser increments `encoded` after it hands the frame to the sink. When the channel
is empty, the parser polls briefly and then parks. See the
[single-pass decoder](single-pass-decoder.md) for the decoding rules.

### 5. Queue and send

The `dnstap-parser` thread queues frames, and the `dnstap-sender` thread sends them.
The parser never does socket or disk I/O.

On the parser thread, `QueuedSink::write_frame` appends each frame to a block of
complete dnstap frames. The parser queues a block at 64 KiB. It queues a partial block
on its idle and busy timers, 100 ms and 1 second. The queue is a bounded channel of
128 blocks. A block that arrives at a full queue is discarded, and its frames count in
`queue_dropped`. Blocks come from a pool allocated at startup, so the parser does not
allocate per frame.

On the sender thread, `SenderThread::route` sends each block to one of three places:

1. The spool, when the receiver is down, when the spool already holds frames, or when
   96 of the 128 queued blocks are waiting.
2. The receiver, when it is connected and the spool is empty.
3. Nowhere, when no spool is in use and the receiver is down. The frames count in
   `disconnected_dropped`.

While the receiver is connected and the spool holds frames, the router replays the
oldest frames in batches of up to 64 KiB, up to 127 at a time. The spool keeps each batch
until the link thread has sent it, and a failure gives the unsent rest back, so a failed
connection loses no replay frame. See [Spool](../guide/configuration.md#spool).

The sender thread owns the link (`NetworkSink`). It runs the TLS and Frame Streams
handshakes, sends STOP, and waits for FINISH at shutdown. Reconnects run on the
`dnstap-reconnect` helper, with a backoff from 250 ms to 30 s. See
[Receiver connection](../guide/configuration.md#receiver-connection).

## Startup and shutdown order

The service starts the stages from the receiver back to the source. The link, spool,
queue, and event channel exist before the first event arrives:

1. Connect the link. A refused connection does not stop the service. A TLS or protocol
   failure does.
2. Check the spool folder and open the spool. A failed check logs one warning, and the
   run continues without a spool.
3. Allocate the block pool and the queue, and start the `dnstap-sender` thread.
4. Create the event channel and the slot pool.
5. Start the parser thread.
6. Start the ETW session and the `etw-consumer` thread.

Shutdown runs from the source forward:

1. Stop the ETW session and join the `etw-consumer` thread.
2. Publish `producer_done` and wake the parser.
3. The parser sends its last partial block, closes the queue, and waits for the sender
   thread.
4. The sender thread routes the blocks still in the queue. It seals the spool segment
   and keeps any backlog on disk. It stops writing to the receiver 1 second after
   shutdown starts. A write in progress and the STOP/FINISH exchange can take about 4
   seconds more.

A watchdog ends the process if shutdown takes more than 10 seconds. The sender thread
takes about 5 seconds at most. See
[Completion and resource ownership](ring-buffer.md#completion-and-resource-ownership).

## Where events are lost

Each stage has its own loss counters. Use the counter to find the stage.

| Stage | Loss | Counter |
| --- | --- | --- |
| 1 | ETW reports lost events or lost buffers. | `etw_lost`, `etw_buffers_lost` |
| 2 | The record is malformed. | `callback_errors` |
| 2 | The pool has no free slot. | `pool_exhausted_regular`, `pool_exhausted_jumbo` |
| 3 | The channel is full. | `ring_dropped` |
| 3 | The channel has no receiver. | `receiver_disconnected` |
| 4 | The record does not decode or encode. | `rejected`, `schema_rejected` |
| 5 | The send queue is full. | `queue_dropped` |
| 5 | The receiver is down with no spool in use, or the connection fails with frames in flight. | `disconnected_dropped` |
| 5 | The spool refuses a block, evicts a segment, or cannot read a frame. | `spool_dropped`, `spool_dropped_bytes` |
| 5 | A spool I/O error. The block is also counted in `spool_dropped`. | `spool_errors` |

Each frame lost in stage 5 is in at most one of `queue_dropped`,
`disconnected_dropped`, and `spool_dropped`.

`intake_dropped` is the sum of the stage 2 and stage 3 admission losses. Do not add it
to those counters. The [Log events and counters](../guide/monitoring.md#counter-relationships)
guide gives the full relationships. The [Tune capture for load](../guide/tuning.md)
guide explains how to reduce each loss.

## Regenerate the animation

The [animation sources](../../scripts/docs/pipeline-animation/README.md) are in the
repository. Change them when a stage changes, and rebuild the GIF.
