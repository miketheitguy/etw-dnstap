# Data path: from an ETW event to a TCP frame

> **Status:** Current · **Describes:** the code at commit `13972d8` · **Evidence dated:** not applicable (reference, from the source and the local tests) · **Reviewed:** 2026-11-04

This reference follows one DNS Server event through the service. It names the
thread, code, and counters of each stage. Maintainers use it to find the stage that
changes or loses an event. Operators use it to match a counter to a stage.

![Animation of the data path in six stages. An ETW event moves through intake, a pool slot, a bounded channel, the parser, and the sender to the TCP receiver. In the fifth stage a slow receiver fills the link FIFO, the router spills blocks to the disk spool, and the spool replays them. In the last stage the receiver is down. Frames go to the disk spool, and the spool replays them, oldest first, after the reconnect.](../assets/pipeline/etw-to-tcp-pipeline.gif)

The animation slows the events and shows illustrative counter values. It splits stage
5 in two. Tab 05 shows normal sending and a slow receiver: the link FIFO fills, the
router spills to the spool, and the spool replays. Tab 06 shows a receiver outage, the
spool, and the replay. The [stage table](#stages) gives the same information as text.

## Stages

An event passes through five stages. The table names the thread and code of each.
Stage 3 spans two threads. Stage 5 spans three: `dnstap-parser`, `dnstap-sender`
(the router), and `dnstap-link`. The `dnstap-reconnect` helper runs reconnects.

| # | Stage | Thread | Code |
| --- | --- | --- | --- |
| 1 | Produce | `dns.exe`, ETW | `Capture::start` |
| 2 | Intake | `etw-consumer` | `native_etw_callback`, `copy_record` |
| 3 | Buffer | Both | `PoolManager`, the bounded channel, `native_buffer_callback` |
| 4 | Parse | `dnstap-parser` | `Parser::parse`, `Encoder::encode` |
| 5 | Queue and send | `dnstap-parser`, `dnstap-sender`, `dnstap-link` | `QueuedSink`, `Router` (`src/sender/router.rs`), `LinkThread` (`src/sender/link.rs`), `Spool`, `NetworkSink` |

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
2 KiB. The channel holds at most `--capacity` slots, 131,072 by default. Neither part
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

Three threads take part. The `dnstap-parser` thread queues frames. The `dnstap-sender`
thread, the router, routes blocks and owns the spool. The `dnstap-link` thread owns the
socket. The parser never does socket or disk I/O, and the router never waits on the
socket.

On the parser thread, `QueuedSink::write_frame` appends each frame to a block of
complete dnstap frames. The parser sends a block at 64 KiB. It sends a partial block on
its idle and busy timers: 100 ms after the last send when the parser is idle, and at
least once a second when it is busy. The channel to the router holds 256 blocks. A block
that arrives at a full channel is discarded, and its frames count in `queue_dropped`.
The block pool of 516 blocks, 68.5 MiB, is allocated at startup, so the parser does not
allocate per frame.

On the router thread, `Router` takes each block and sends it to one of three places:

1. The link FIFO, a queue of 256 blocks (about 16 MiB) that the link thread takes from.
   The router uses it while the link is up and the spool holds no backlog.
2. The spool, while the link is down, while the spool holds a backlog or a replay batch
   is out (so frames keep their order), and when the FIFO is full.
3. Nowhere, when no spool is in use and the link is down. The frames count in
   `disconnected_dropped`.

When the FIFO is full, the router spills passively. It takes every block out of the FIFO
and appends them to the spool, oldest first, then the block that did not fit. Only a
full FIFO causes a spill. No routing decision reads the state of the socket.

The link thread takes one block at a time from the FIFO and writes it with non-blocking
sends. When the socket accepts no more bytes, the thread waits for the socket to become
writable for at most 50 ms. Then it checks the stall timer and writes again. A session
ends only when the socket accepts no byte for `--receiver-stall-seconds` (15 by default)
while data waits. No data send has a socket timeout, and the service does not set
`SO_SNDBUF`.

While the link is up and the spool holds a backlog, the router replays the oldest frames.
It reads batches of up to 64 KiB from the spool head and sends them into the FIFO. Up to
127 batches can be out at once, and one router pass reads at most 1 MiB. The spool holds
each batch until the link thread reports how much of it the socket accepted. A segment is
deleted only after the link has taken all of its frames. See
[Spool](../guide/configuration.md#spool).

At a link failure, the link thread ends the session and gives back what it did not send.
The router takes the FIFO back and sorts the frames by how far the socket got:

- Live frames that the socket did not fully accept go back to the router. These are the
  cut frame, every later frame, and, with TLS, the frames of the piece whose delivery
  could not be confirmed. They go to the head of the spool, or to a front list that is
  sent first after the reconnect when the spool already holds a backlog. The front list
  holds at most one block.
- Replay frames that the socket did not fully accept stay in the spool. The spool moves
  its read position back to the last frame the socket accepted, so the router reads them
  again.
- Frames that the socket fully accepted are not sent again.

**Allocation on the TLS path.** Every stage up to the socket reuses its buffers, so the data
path makes no heap allocation per event. TLS is the one exception: rustls allocates one buffer
for each record it encrypts (up to 16 KiB of plaintext), and frees it after `write_tls` sends
it. That is about 2,500 allocations a second at 41 MB/s. The rustls unbuffered API
(`UnbufferedClientConnection`) does not remove it: in rustls 0.23.46 its `encrypt` still has
the cipher allocate the record, then copies the record into the caller's buffer, so it adds a
copy and saves nothing. Only a record layer outside rustls (traffic secrets taken with
`dangerous_extract_secrets`, records sealed in place with `ring`) would remove the allocation.
That means writing record framing, nonces, sequence numbers, and key updates ourselves, for an
expected gain below 1 % of one CPU. It was evaluated and dropped on 2026-11-04. The buffered
`ClientConnection` stays.

The sender owns the reconnect logic. Reconnects run on the `dnstap-reconnect` helper,
with a backoff from 250 ms to 30 s. See
[Receiver connection](../guide/configuration.md#receiver-connection).

## Startup and shutdown order

The service starts the stages from the receiver back to the source. The link, spool,
queue, and event channel exist before the first event arrives:

1. Connect the link. A refused connection does not stop the service. A TLS or protocol
   failure does.
2. Check the spool folder and open the spool. A failed check logs one warning, and the
   run continues without a spool.
3. Allocate the block pool and the send channel. Start the `dnstap-link` thread, then
   the `dnstap-sender` router thread. The router starts after the link thread has
   published its first state.
4. Create the event channel and the slot pool.
5. Start the parser thread.
6. Start the ETW session and the `etw-consumer` thread.

Shutdown runs from the source forward:

1. Stop the ETW session and join the `etw-consumer` thread.
2. Publish `producer_done` and wake the parser.
3. The parser sends its last partial block, waiting up to 100 ms for room, closes the
   channel, and waits for the router thread.
4. The router routes the blocks still in the channel and the FIFO to the spool, then
   seals the spool segment. Replay data that the link did not take stays on disk for the
   next run. `cursor.bin` records the delivered position. The link thread stops writing
   1 second after shutdown starts, and hands back its unsent data. It then runs STOP and
   FINISH, within 2 seconds, if the socket is at a frame boundary.

A watchdog ends the process if shutdown takes more than 10 seconds. The sender threads
finish within about 3.2 seconds. See
[Startup and shutdown deadlines](../guide/configuration.md#startup-and-shutdown-deadlines)
and [Completion and resource ownership](ring-buffer.md#completion-and-resource-ownership).

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
| 5 | The channel to the router is full. | `queue_dropped` |
| 5 | No spool is in use, and the receiver is down or the connection fails with live frames the socket did not accept. | `disconnected_dropped` |
| 5 | The spool refuses a block, evicts a segment, or cannot read a frame. | `spool_dropped`, `spool_dropped_bytes` |
| 5 | A spool I/O error. The block is also counted in `spool_dropped`. | `spool_errors` |

Each frame lost in stage 5 is in at most one of `queue_dropped`,
`disconnected_dropped`, and `spool_dropped`.

With a spool, a link failure counts no frame. The frames go back to the router or stay in
the spool, and they are sent again. At a shutdown that follows a spill, the one live
block that comes back from the link is older than the spool backlog, so it cannot go in
front of it. The router counts it in `disconnected_dropped`.

`disconnected_dropped` counts frames that the link did not hand to the socket in full,
and that no spool kept. A cut frame reaches the receiver only as a truncated copy, which
the receiver discards. So a counted frame did not arrive whole. The one exception is the
TLS 1.3 key update window (risk 19 in the slow-receiver design). If the socket took the
whole data record and the connection then failed while a key update was still pending,
the receiver holds that record, and the counter includes its frames. Bytes that the kernel
accepted before a failure are not counted, even when the connection then fails before the
kernel sends them. So the counter can miss loss. It is not an upper bound on loss.

`replayed` and `bytes_sent` count the bytes that the socket accepted. The same kernel
loss applies to them, so `replayed` is not a delivery count.

`intake_dropped` is the sum of the stage 2 and stage 3 admission losses. Do not add it
to those counters. The [Log events and counters](../guide/monitoring.md#counter-relationships)
guide gives the full relationships. The [Tune capture for load](../guide/tuning.md)
guide explains how to reduce each loss.

## Regenerate the animation

The [animation sources](../../scripts/docs/pipeline-animation/README.md) are in the
repository. Change them when a stage changes, and rebuild the GIF.
