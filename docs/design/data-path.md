# Data path: from an ETW event to a TCP frame

> **Status:** Current · **Describes:** the code on branch `spool-encryption` as of 2026-11-05 · **Evidence dated:** not applicable (reference, from the source and the local tests) · **Reviewed:** 2026-11-05

This page follows one DNS Server event from ETW to the receiver. For each stage it
names the thread, the code, and the loss counters.

![Animation of the data path in six stages. An ETW event moves through intake, a pool slot, a bounded channel, the parser, and the sender to the TCP receiver. In the fifth stage a slow receiver fills the link FIFO, the router spills blocks to the disk spool, and the spool replays them. In the last stage the receiver is down. Frames go to the disk spool, encrypted, and replay decrypts each record, oldest first, after the reconnect.](../assets/pipeline/etw-to-tcp-pipeline.gif)

## Stages

An event passes through five stages. Stages 3 and 5 span more than one thread.

| # | Stage | Thread | Code |
| --- | --- | --- | --- |
| 1 | Produce | `dns.exe` (ETW provider) | `Capture::start` (`src/etw/session.rs`) |
| 2 | Intake | `etw-consumer` | `native_etw_callback`, `copy_record` (`src/etw/intake.rs`) |
| 3 | Buffer | `etw-consumer` (writes), `dnstap-parser` (reads) | `PoolManager` (`src/etw/pool.rs`), the bounded event channel, `native_buffer_callback` |
| 4 | Parse | `dnstap-parser` | `etw::Parser::parse` (`src/etw/schema.rs`), `Encoder::encode` (`src/dnstap.rs`) |
| 5 | Queue and send | `dnstap-parser`, `dnstap-sender`, `dnstap-link`, `dnstap-reconnect` | `QueuedSink` and `Router` (`src/sender.rs`, `src/sender/router.rs`), `LinkThread` (`src/sender/link.rs`), `Spool` (`src/spool.rs`), `NetworkSink` (`src/network.rs`) |

Decoding happens in stage 4, after the buffer. The intake callback only copies bytes.

### 1. Produce

The service starts a real-time ETW session and enables the DNS Server provider. The
session admits events 256, 257, and 258 from any process. ETW delivers events in
batches, about once per `--etw-flush-seconds`. The provider settings are in
[Subscription and event selection](pooled-intake.md#subscription-and-event-selection).

### 2. Intake

ETW calls `native_etw_callback` on the `etw-consumer` thread for each event. The
callback runs these steps in order:

1. Drop a record from another provider or with an unsupported event ID. A dropped
   record counts nothing.
2. Count the record in `received`.
3. Copy the record into a pool slot. A malformed record counts in `callback_errors`.
   If no slot of the needed class is free, the record counts in
   `pool_exhausted_regular` or `pool_exhausted_jumbo`.
4. Send the slot with `try_send`. If the channel is full, the callback drops the newest
   event and counts `ring_dropped`. If the channel has no receiver, it counts
   `receiver_disconnected`.

Each admission loss also counts in `intake_dropped`. The callback runs without waiting
or allocating. [Ownership and callback lifecycle](pooled-intake.md#ownership-and-callback-lifecycle)
gives the ownership rules.

### 3. Buffer

The pool holds preallocated regular (2 KiB) and jumbo slots. The event channel holds up
to `--capacity` events. Nothing grows after startup. The sizes and the memory formula are
in [Memory budget and configuration](pooled-intake.md#memory-budget-and-configuration).

After each delivered ETW buffer, `native_buffer_callback` counts `etw_buffers_delivered`.
If that buffer enqueued an event, the callback also counts `parser_wake_requests` and
wakes the parser. See [Parser wake and polling](ring-buffer.md#parser-wake-and-polling).

### 4. Parse

The parser takes slots with `try_recv`. For each slot, it:

1. Counts `dequeued`.
2. Decodes the record with a compiled plan that reads each field once. A schema that
   does not compile, or a malformed payload, counts in `rejected`. A schema failure also
   counts in `schema_rejected`.
3. Encodes the record as dnstap. Event 256 becomes `CLIENT_QUERY`. Events 257 and 258
   become `CLIENT_RESPONSE`. A record that fails an encoder check counts in `rejected`.
4. Releases the slot to the pool.
5. Hands the frame to the sink, then counts `encoded`.

The [single-pass decoder](single-pass-decoder.md) gives the grammar, the
[checks](single-pass-decoder.md#checks-after-decoding), and the bounds. When the channel
is empty, the parser polls briefly and then parks.

### 5. Queue and send

The parser (`dnstap-parser`) queues frames. The router (`dnstap-sender`) routes blocks
and owns the spool. The link (`dnstap-link`) owns the socket. The parser does no socket or
disk I/O, and the router never waits on the socket.

**Blocks.** `QueuedSink::write_frame` appends each frame to a block of whole dnstap
frames. The parser sends a block when it holds 64 KiB. It also sends a partial block
after 100 ms of idle time, and at least once a second while it is busy. The channel to
the router holds 256 blocks. If that channel is full, the block is dropped and its
frames count in `queue_dropped`. The block pool of 516 blocks (136 KiB each, 68.5 MiB in
all) is allocated at startup.

**Routing.** The router sends each block to the link FIFO, which holds 256 blocks (about
16 MiB of frames), while the link is up and the spool has no backlog. It sends the block
to the spool while the link is down, while the spool holds a backlog or a replay batch is
out (so frames keep their order), and when the FIFO is full. With no spool and the link
down, the frames count in `disconnected_dropped`. A full FIFO causes a spill: the router
moves every block in the FIFO to the spool, oldest first, and then the block that did not
fit. Routing never reads the state of the socket.

**Link.** The link thread writes one block at a time from the FIFO with non-blocking
sends. When the socket accepts no more bytes, it waits up to 50 ms for the socket to
become writable, checks the stall timer, and writes again. A session ends only when the
socket accepts no bytes for `--receiver-stall-seconds` (default 15, range 2 to 300) while
data waits. The stall timer is the only send timeout, and the socket keeps its default
`SO_SNDBUF`. The [slow-receiver design](slow-receiver.md) gives the link states.

**Replay.** While the link is up and the spool holds a backlog, the router reads batches
of up to 64 KiB from the head of the spool and sends them to the FIFO. At most 127 batches
are out at once, and one router pass reads at most 1 MiB. The spool keeps each batch until
the link reports how much of it the socket accepted, and it deletes a segment only after
the link has taken all of its frames. See [Spool](../guide/configuration.md#spool).

**Reconnects.** The `dnstap-reconnect` helper makes connection attempts. The backoff
runs from 250 ms to 30 s. See [Receiver connection](../guide/configuration.md#receiver-connection).

### Spool encryption

The spool encrypts only the data it writes to disk.
[Encryption](../guide/configuration.md#encryption) describes the key setup, and
[SECURITY.md](../../SECURITY.md) lists what the encryption protects. If the key cannot be
loaded or locked at start, the run continues without a spool. The spool writes only
ciphertext.

**Format.** Each segment starts with a 32-byte file header that holds a random nonce
prefix (`src/spool/segment.rs`). Each appended block is sealed as one AES-128-GCM record
(`src/spool/crypt.rs`), which adds 28 bytes: a 12-byte record header and a 16-byte tag. A sealed
segment ends with an end record.

**Units.** The size cap, the free-space floor, `spool_bytes`, and `spool_dropped_bytes`
count file bytes, which include the header and the record overhead. Read and delivered
positions, and `cursor.bin`, use offsets in the plaintext frame stream. Batch sizes,
frame counts, and `replayed` count plaintext frames.

**Reading.** `SegmentReader` decrypts each record in a reused buffer, and it can read
while the writer still appends. To resume at a saved position, it walks the record
headers without decrypting, then decrypts the record that holds the position. A record
that does not authenticate ends its segment, and the rest counts in `spool_dropped_bytes`.

**Key.** One `NCryptDecrypt` call on the `spool-key-load` thread unwraps the key at each
start (`src/spool/key.rs`). The start waits at most 3 seconds (`KEY_LOAD_TIMEOUT`), and
the installer makes the service depend on `KeyIso`. The key bytes go straight into a locked page
(`src/spool/locked.rs`) that stays out of the pagefile and out of Windows Error Reporting
(WER) crash dumps. They stay encrypted in place with `CryptProtectMemory`
(`CRYPTPROTECTMEMORY_SAME_PROCESS`) except during a seal or an open
(`src/spool/protected.rs`).

**Per use.** Each seal or open takes a mutex, calls `CryptUnprotectMemory`, and checks
the result against a SHA-256 check value taken at load. It builds the `ring` AES-128-GCM
key in a scratch area of the same page, re-protects the key bytes, processes the record,
and zeroes the scratch area. A guard does the re-protect and the zeroing if a panic
unwinds; the release profile sets `panic = "abort"`. The service then overwrites 16 KiB
of stack below the caller's frame.

**Failures.** A failed `CryptUnprotectMemory` fails only that operation, as a spool I/O
error. If `CryptProtectMemory` fails twice, or the decrypted bytes fail the check, the
service zeroes the key and suspends the spool until it restarts, keeping its segments and
cursor. `spool_available` becomes false. Live frames then go straight to the receiver while it is up, which relaxes the
strict-order rule in this case only. [Spool key errors](../guide/troubleshooting.md#spool-key-errors)
gives the messages and the counters.

**At open.** The spool reads the header of each leftover segment. It deletes segments of
another format, other files, and segments sealed with another key. It counts their
bytes in `spool_dropped_bytes` and logs one warning. If the key was created at this
start, the spool deletes every segment and resets `cursor.bin`.

### Link failure

At a link failure, the link thread ends the session and returns its unsent data. The
router takes back the FIFO and sorts the frames by how far the socket got:

- Live frames that the socket did not fully accept go back to the router: the cut frame,
  every later frame, and, with TLS, the frames of the piece whose delivery is unconfirmed.
  They go to the head of the spool. When the spool already holds a backlog, they go to a
  front list (at most one block) that is sent first after the reconnect.
- Replay frames that the socket did not fully accept stay in the spool. The spool moves
  its read position back to the last frame the socket accepted.
- Frames that the socket fully accepted are sent once.

### Allocation on the TLS path

Every stage up to the socket reuses its buffers, so the plaintext data path makes no heap
allocation per event. Under TLS, rustls allocates one buffer for each record it encrypts
(up to 16 KiB of plaintext) and frees it after the record is sent. That is about 2,500
allocations a second at 41 MB/s. The service uses the buffered rustls `ClientConnection`.

## Startup and shutdown order

The services start from the receiver back toward the source. Each step must succeed
before the next one starts.

1. Connect the link. A refused connection lets startup continue, because the link
   retries. A TLS or protocol failure stops the service.
2. Check the spool folder, unwrap the spool key, and open the spool. A failed check or
   key load logs one warning, and the run continues without a spool.
3. Create the send queue and the block pool. Start `dnstap-link`, then `dnstap-sender`.
   The router starts after the link thread publishes its first state.
4. Create the event channel and the slot pool.
5. Start `dnstap-parser`.
6. Start the ETW session and `etw-consumer`.

Shutdown runs from the source toward the receiver:

1. Stop the ETW session and join `etw-consumer`. No callback can enqueue after this point.
2. Publish `producer_done` and wake the parser.
3. The parser sends its last partial block, waiting up to 100 ms for room. It then closes
   the channel and waits for the router.
4. The router routes the blocks still in the channel and the FIFO to the spool, then seals
   the spool segment. Replay data that the link did not take stays on disk for the next
   run. `cursor.bin` records the delivered position.
5. The link stops writing 1 second after shutdown starts, and hands back its unsent
   data. If the deadline did not interrupt a block, it then sends STOP and FINISH, within
   2 seconds.

A 10-second watchdog ends the process if shutdown takes longer.
[Startup and shutdown deadlines](../guide/configuration.md#startup-and-shutdown-deadlines)
gives the time budget of each step. [Completion and resource ownership](ring-buffer.md#completion-and-resource-ownership)
gives the ETW-side ordering.

## Where events are lost

Each loss counter names one stage. [Loss counters](measurement-terms.md#loss-counters-live-service)
define each counter, and [Tune capture for load](../guide/tuning.md) explains how to reduce
each loss.

| Stage | Loss | Counter |
| --- | --- | --- |
| 1 | ETW reports lost events or lost buffers. | `etw_lost`, `etw_buffers_lost` |
| 2 | The record is malformed, or its TLS state is missing or wrong. | `callback_errors` |
| 2 | The pool has no free slot of the record's class. | `pool_exhausted_regular`, `pool_exhausted_jumbo` |
| 3 | The event channel is full. | `ring_dropped` |
| 3 | The event channel has no receiver. | `receiver_disconnected` |
| 4 | The record does not decode or encode. | `rejected` (and `schema_rejected` for a schema failure) |
| 5 | The channel to the router is full. | `queue_dropped` |
| 5 | No spool is in use, and the receiver is down or a connection fails with live frames it did not accept. | `disconnected_dropped` |
| 5 | The spool refuses or evicts a frame, or cannot read or authenticate a record. | `spool_dropped`, `spool_dropped_bytes` |
| 5 | A spool I/O error. | `spool_errors` |

A frame lost in stage 5 counts in at most one of `queue_dropped`, `disconnected_dropped`,
and `spool_dropped`. `intake_dropped` is the sum of the stage 2 and stage 3 admission
losses. Do not add it to those counters. The
[counter relationships](../guide/monitoring.md#counter-relationships) list the identities.

With a spool, a link failure counts no frame, because the frames are sent again
([Link failure](#link-failure)). At a shutdown that follows a spill, the one live block
that comes back from the link is older than the spool backlog. It cannot go ahead of the
backlog, so the router counts it in `disconnected_dropped`.

`disconnected_dropped` counts frames that the link did not hand to the socket in full and
that no spool kept. A cut frame reaches the receiver only as a truncated copy, which the
receiver discards. The one exception is the TLS 1.3 key update window (risk 19 in the
[slow-receiver design](slow-receiver.md)). If the socket took a whole data record and the
connection then failed while a key update was pending, the receiver holds that record, and
the counter includes its frames. Bytes that the kernel accepted before a failure are not
counted, even if the kernel never sent them. So the counter can miss lost frames and can
include frames the receiver holds. It is neither a lower nor an upper bound on loss.

`replayed` and `bytes_sent` count bytes the socket accepted. The same kernel loss applies
to them, so `replayed` is not a delivery count.

## Regenerate the animation

The [animation sources](../../scripts/docs/pipeline-animation/README.md) are in the
repository. Change them when a stage changes, and rebuild the GIF.
