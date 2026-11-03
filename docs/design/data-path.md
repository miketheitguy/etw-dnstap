# Data path: from an ETW event to a TCP frame

> **Status:** Current · **Describes:** `main` at commit `907f728` · **Evidence dated:** not applicable (reference, from the source and the local tests) · **Reviewed:** 2026-11-02

This reference follows one DNS Server event through the service. It names the
thread, the code, and the counters of each stage. Maintainers use it to find
the stage that changes or loses an event. Operators use it to connect a counter
to a stage. It does not repeat the details of each stage. The sections that
follow link to the documents that do.

![Animation of the data path. A DNS Server event moves from the ETW session to the intake callback, which checks it and copies it into a pool slot. A bounded channel queues the slot. The parser thread decodes the record, encodes it as dnstap, and releases the slot. The TCP sink buffers the frame and sends it to the receiver.](../assets/pipeline/etw-to-tcp-pipeline.gif)

The animation slows the events and shows illustrative counter values. The
[stage table](#stages) has the same information as text.

## Stages

An event passes through five stages, on three threads. The `etw-consumer`
thread runs stage 2. The `dnstap-parser` thread runs stages 4 and 5. ETW and
the DNS Server run stage 1. The buffer in stage 3 belongs to both threads.

| # | Stage | Thread | Code | What happens |
| --- | --- | --- | --- | --- |
| 1 | Produce | `dns.exe`, ETW | `Capture::start` | The DNS Server writes events 256, 257, and 258 to the real-time ETW session. ETW keeps them in session buffers and delivers them about once per `--etw-flush-seconds`. |
| 2 | Intake | `etw-consumer` | `native_etw_callback`, `copy_record` | The callback checks the provider, the event ID, and the process ID. It copies the record into a pool slot and sends the slot to the channel. |
| 3 | Buffer | Both | `PoolManager`, the bounded channel, `native_buffer_callback` | The channel holds the slots. After each delivered ETW buffer, the buffer callback wakes the parser if that buffer enqueued an event. |
| 4 | Parse | `dnstap-parser` | `Parser::parse`, `Encoder::encode` | The parser decodes the record in one pass, encodes it as a dnstap message, and releases the slot. |
| 5 | Send | `dnstap-parser` | `NetworkSink`, `FrameStreamWriter` | The sink writes the frame to a 64 KiB buffer and flushes it to the receiver over TCP, with TLS by default. |

Decoding happens in stage 4, after the buffer. The intake callback only copies
bytes. It does not decode them.

### 1. Produce

The service creates a real-time ETW session and enables the DNS Server provider
with a keyword mask, an event-ID filter, and a process-ID filter. The filters
admit only events 256, 257, and 258 from the DNS Server process. ETW holds the
events in its own buffers and delivers them in batches. The
[Subscription](../guide/configuration.md#subscription) section lists the
filters.

### 2. Intake

ETW calls `native_etw_callback` on the `etw-consumer` thread for each event.
The callback does these steps in order:

1. Reject a record that is not from the DNS Server provider or that has an
   unsupported event ID. This step counts nothing.
2. Reject a record whose process ID is not the DNS Server process ID. Count it
   in `foreign_pid_rejected`.
3. Count the record in `received`.
4. Copy the record into a pool slot. A malformed record increments
   `callback_errors`. A record with no free slot increments
   `pool_exhausted_regular` or `pool_exhausted_jumbo`.
5. Send the slot to the channel with `try_send`. A full channel drops the newest
   event and increments `ring_dropped`.

The callback never waits and never allocates. The
[pooled intake](pooled-intake.md#ownership-and-callback-lifecycle) reference
explains the ownership rules.

### 3. Buffer

The buffer has two parts. The pool holds preallocated slots of 2 KiB each, and
the `--jumbo-capacity` slots for records over 2 KiB. The bounded channel holds
at most `--capacity` slots, 24,576 by default. Neither part grows after
startup.

ETW calls `native_buffer_callback` after it delivers each buffer. The callback
increments `etw_buffers_delivered`. If that buffer enqueued an event, it also
increments `parser_wake_requests` and unparks the parser. The per-event callback
never wakes the parser. See
[Parser wake and polling](ring-buffer.md#parser-wake-and-polling).

### 4. Parse

The parser takes slots with `try_recv`. For each slot, it does these steps:

1. Increment `dequeued`.
2. Decode the record with a compiled plan that reads each field once. A schema
   that does not compile, or a malformed payload, increments `rejected`. A
   schema failure also increments `schema_rejected`.
3. Encode the record as a dnstap message. Event 256 becomes `CLIENT_QUERY`.
   Events 257 and 258 become `CLIENT_RESPONSE`.
4. Release the slot to the pool.
5. Pass the frame to the sink.

The parser increments `encoded` after it hands the frame to the sink. When the
channel is empty, the parser polls for a short time and then parks. See the
[single-pass decoder](single-pass-decoder.md) for the decoding rules.

### 5. Send

The sink writes each frame to a 64 KiB buffer. A full buffer flushes itself.
The parser also flushes unsent frames after 100 ms of idle time. When the
parser is busy, it flushes if the last flush is over one second old. Each socket
operation has a 2-second deadline.

At connection time, the sink completes the TLS handshake. It then runs the
Frame Streams handshake: READY, ACCEPT, and START. At shutdown, it sends STOP
and waits for FINISH.

If the connection fails, the sink discards the buffered frames and the frames
that arrive while it is down. It counts them in `disconnected_dropped` and never
resends them. A separate `dnstap-reconnect` thread reconnects with a backoff
from 250 ms to 30 s. See
[Receiver connection](../guide/configuration.md#receiver-connection).

## Startup and shutdown order

The service starts the stages from the receiver side back to the source. This
order means that the sink and the buffer exist before the first event arrives:

1. Connect the sink. A refused connection does not stop the service. A TLS or
   protocol failure does.
2. Create the channel and the pool.
3. Start the parser thread.
4. Start the ETW session and the `etw-consumer` thread.

Shutdown runs from the source forward:

1. Stop the ETW session and join the `etw-consumer` thread.
2. Publish `producer_done` and wake the parser.
3. The parser drains the channel, sends STOP, and ends.

A watchdog ends the process if shutdown takes more than 10 seconds. See
[Completion and resource ownership](ring-buffer.md#completion-and-resource-ownership).

## Where events are lost

Each stage has its own loss counters. Use the counter to find the stage.

| Stage | Loss | Counter |
| --- | --- | --- |
| 1 | ETW reports lost events or lost buffers. | `etw_lost`, `etw_buffers_lost` |
| 2 | The record is malformed. | `callback_errors` |
| 2 | The pool has no free slot. | `pool_exhausted_regular`, `pool_exhausted_jumbo` |
| 3 | The channel is full. | `ring_dropped` |
| 4 | The record does not decode. | `rejected`, `schema_rejected` |
| 5 | The receiver is down. | `disconnected_dropped` |

`intake_dropped` is the sum of the stage 2 and stage 3 admission losses. Do not
add it to those counters. The [Log events and counters](../guide/monitoring.md#counter-relationships)
guide gives the full relationships. The
[Tune capture for load](../guide/tuning.md) guide explains how to reduce each
loss.

## Regenerate the animation

The [animation sources](../../scripts/docs/pipeline-animation/README.md) are in
the repository. Change them when a stage changes, and rebuild the GIF.
