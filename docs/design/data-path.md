# Data path: from an ETW event to a TCP frame

> **Status:** Current · **Describes:** the code on branch `spool-encryption` as of 2026-11-05 · **Evidence dated:** not applicable (reference, from the source and the local tests) · **Reviewed:** 2026-11-05

This page follows one DNS Server event from ETW to the receiver. For each stage it
names the thread, the code, and the counters. Maintainers use it to find the stage
that changes or loses an event. Operators use it to match a counter to a stage.

![Animation of the data path in six stages. An ETW event moves through intake, a pool slot, a bounded channel, the parser, and the sender to the TCP receiver. In the fifth stage a slow receiver fills the link FIFO, the router spills blocks to the disk spool, and the spool replays them. In the last stage the receiver is down. Frames go to the disk spool, encrypted, and replay decrypts each record, oldest first, after the reconnect.](../assets/pipeline/etw-to-tcp-pipeline.gif)

The animation slows the events and shows sample counter values. It splits stage 5 into
two tabs. Tab 05 shows normal sending and a slow receiver. Tab 06 shows a receiver
outage, the encrypted spool, and the replay. The [stage table](#stages) gives the same
information as text.

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
session admits only events 256, 257, and 258, from any process that writes them. ETW delivers events about once per
`--etw-flush-seconds` (default 1). The [Subscription](../guide/configuration.md#subscription)
section lists the filters. The exact provider settings are in
[Subscription and event selection](pooled-intake.md#subscription-and-event-selection).

### 2. Intake

ETW calls `native_etw_callback` on the `etw-consumer` thread for each event. The
callback runs these steps in order:

1. Drop a record from another provider or with an unsupported event ID. A dropped
   record counts nothing.
2. Count the record in `received`. The callback does not check the process ID.
3. Copy the record into a pool slot. A malformed record counts in `callback_errors`.
   If no slot of the needed class is free, the record counts in
   `pool_exhausted_regular` or `pool_exhausted_jumbo`.
4. Send the slot with `try_send`. If the channel is full, the callback drops the newest
   event and counts `ring_dropped`. If the channel has no receiver, it counts
   `receiver_disconnected`.

Each admission loss also counts in `intake_dropped`. The callback never waits and never
allocates. [Ownership and callback lifecycle](pooled-intake.md#ownership-and-callback-lifecycle)
gives the ownership rules.

### 3. Buffer

The pool holds preallocated 2 KiB regular slots (`--capacity` + 2 of them) and
`--jumbo-capacity` slots (default 64) for records that do not fit in 2 KiB. The event
channel holds up to `--capacity` events (default 131,072). Nothing grows after startup.
The sizes and the memory formula are in
[Memory budget and configuration](pooled-intake.md#memory-budget-and-configuration).

After each delivered ETW buffer, `native_buffer_callback` counts `etw_buffers_delivered`.
If that buffer enqueued an event, the callback also counts `parser_wake_requests` and
wakes the parser. The per-event callback never wakes the parser. See
[Parser wake and polling](ring-buffer.md#parser-wake-and-polling).

### 4. Parse

The parser takes slots with `try_recv`. For each slot, it:

1. Counts `dequeued`.
2. Decodes the record with a compiled plan that reads each field once. A schema that
   does not compile, or a malformed payload, counts in `rejected`. A schema failure also
   counts in `schema_rejected`. The [single-pass decoder](single-pass-decoder.md) has the rules.
3. Encodes the record as dnstap. Event 256 becomes `CLIENT_QUERY`. Events 257 and 258
   become `CLIENT_RESPONSE`. The encoder rejects the record, and counts it in `rejected`,
   when any of these is true:
   - The event ID is unsupported.
   - The DNS message is outside 12 to 65,535 bytes.
   - The DNS QR bit disagrees with the event.
   - The local and peer addresses are in different families.
   - The timestamp is before the Unix epoch.
   - The identity is longer than 1,024 bytes.
   - The encoded frame is larger than 70 KiB (71,680 bytes).
4. Releases the slot to the pool.
5. Hands the frame to the sink, then counts `encoded`.

When the channel is empty, the parser polls briefly and then parks. The
[dnstap field limits](../guide/configuration.md#message-and-transport-limits) are in the
configuration reference.

### 5. Queue and send

Three threads share this stage. The parser (`dnstap-parser`) queues frames. The router
(`dnstap-sender`) routes blocks and owns the spool. The link (`dnstap-link`) owns the
socket. The parser never does socket or disk I/O, and the router never waits on the socket.

**Blocks.** `QueuedSink::write_frame` appends each frame to a block of whole dnstap
frames. The parser sends a block when it holds 64 KiB. It also sends a partial block
after 100 ms of idle time, and at least once a second while it is busy. The channel to
the router holds 256 blocks. If that channel is full, the block is dropped and its
frames count in `queue_dropped`. The block pool of 516 blocks (136 KiB each, 68.5 MiB in
all) is allocated at startup.

**Routing.** The router sends each block to one of three places:

1. The link FIFO, which holds 256 blocks (about 16 MiB of frames). The router uses it
   while the link is up and the spool has no backlog.
2. The spool, while the link is down, while the spool holds a backlog or a replay batch
   is out (so frames keep their order), and when the FIFO is full.
3. Nowhere, when no spool is in use and the link is down. The frames count in
   `disconnected_dropped`.

**Spill.** A full FIFO causes a spill. The router moves every block in the FIFO to the
spool, oldest first, and then the block that did not fit. Routing never reads the state
of the socket.

**Link.** The link thread takes one block at a time from the FIFO and writes it with
non-blocking sends. When the socket accepts no more bytes, the thread waits up to 50 ms
for the socket to become writable. Then it checks the stall timer and writes again. A
session ends only when the socket accepts no bytes for `--receiver-stall-seconds`
(default 15, range 2 to 300) while data waits. No data send has a socket timeout, and the
service does not set `SO_SNDBUF`.

**Replay.** While the link is up and the spool holds a backlog, the router reads batches
of up to 64 KiB from the head of the spool and sends them to the FIFO. At most 127 batches
can be out at once. One router pass reads at most 1 MiB. The spool keeps each batch until
the link reports how much of it the socket accepted. It deletes a segment only after the
link has taken all of its frames. See [Spool](../guide/configuration.md#spool).

**Reconnects.** The `dnstap-reconnect` helper makes connection attempts. The backoff
runs from 250 ms to 30 s. See [Receiver connection](../guide/configuration.md#receiver-connection).

### Spool encryption

The spool encrypts only what it writes to disk. Blocks in memory, in the send queue, in
the FIFO, and on the wire are not changed by the spool. TLS, when configured, protects
the wire.

**File format.** Each segment starts with a 32-byte file header that holds a random
nonce prefix (`src/spool/segment.rs`). Each appended block is sealed as one AES-128-GCM
record (`src/spool/crypt.rs`). A record adds 28 bytes: a 12-byte record header and a
16-byte tag. A sealed segment ends with an end record.

**Units.** The size cap, the free-space floor, `spool_bytes`, and `spool_dropped_bytes`
count file bytes, which include the header and the record overhead. Read and delivered
positions, and `cursor.bin`, use offsets in the plaintext frame stream. Batch sizes,
frame counts, and `replayed` count plaintext frames.

**Reading.** `SegmentReader` decrypts each record in a reused buffer. It can read while
the writer still appends. To resume at a saved position, it walks the record headers
without decrypting, decrypts the record that holds the position, and starts at the frame
boundary inside it. A record that does not authenticate ends its segment. The rest of
that segment is counted in `spool_dropped_bytes` and never replayed.

**Key handling.** The key is unwrapped once per start by one `NCryptDecrypt` call
(`src/spool/key.rs`). The unwrap runs on the `spool-key-load` helper thread, and the
start waits at most 3 seconds (`KEY_LOAD_TIMEOUT`). If the key store does not answer,
the run continues without a spool, with one warning. The key bytes go straight into a
locked page (`src/spool/locked.rs`). The page is never written to the pagefile, and
Windows Error Reporting (WER) leaves it out of crash dumps. The bytes are encrypted in
place with `CryptProtectMemory` (`CRYPTPROTECTMEMORY_SAME_PROCESS`) at all times except
during a seal or an open (`src/spool/protected.rs`). The installer makes the service
depend on `KeyIso`.

**Per use.** Each seal or open takes a mutex and calls `CryptUnprotectMemory`. It checks
the result against a SHA-256 check value taken when the key loaded. It builds the `ring`
AES-128-GCM key in a scratch area of the same page, and re-protects the key bytes. It
then seals or opens the record and zeroes the scratch area. A guard does the re-protect
and the zeroing when a panic unwinds. The release profile sets `panic = "abort"`, so no
unwind happens there. After each use, the service overwrites 16 KiB of stack below the
caller's frame. The [spool encryption plan](../plans/spool-encryption-plan.md) records the
per-record costs.

**Failures.**

- If `CryptUnprotectMemory` fails, nothing changes and the service keeps the key. An
  append fails as an I/O error. It counts in `spool_errors`, logs Event ID 27
  (`dnstap spool I/O error`), and pauses the spool for 10 seconds. A failed read is
  counted, reads pause for 10 seconds, and the segment and cursor are kept. A later
  operation tries again.
- If `CryptProtectMemory` fails, the service tries once more. If that also fails, or the
  decrypted bytes fail the check, the key bytes are zeroed and the spool suspends until
  the service restarts:
  - Appends are refused. Their frames count in `spool_dropped`. In rare cases a segment is
    evicted before the failure is seen.
  - Reads stop. Nothing is dropped.
  - No segment is deleted and the cursor does not move. Replay batches that the link
    already has are still acknowledged in the usual way.
  - `spool_available` becomes false. The service logs one warning, Event ID 26, unless
    shutdown has begun.
  - Live frames go straight to the receiver while it is up. This relaxes the strict-order
    rule in this case only. While the receiver is down, live frames are refused and count
    in `spool_dropped`. A full send queue then stops intake, and the parser counts
    `queue_dropped`.
  - `link_saturated` clears with the reason `the spool is suspended until the service
    restarts`. It does not set again in this run.
  - The next start unwraps the key again and replays the kept backlog.

**Limits.** The encryption protects a copy of the spool folder on its own, other local
accounts, and a disk or image of a BitLocker-protected volume. It does not protect:

- An offline copy of an unencrypted system volume. The machine key material sits on the
  same volume.
- SYSTEM and Administrators, or code running in the service process.
- A full kernel memory capture. Such a capture may let an analyst recover the key. This
  is likely, but Microsoft does not document where the key lives.
- Register contents, round keys left in XMM registers, and stack below the 16 KiB scrub.
  A dump taken during a seal or open can contain these.

The WER exclusion covers WER crash dumps only. If the key cannot be loaded or locked at
start, the run continues without a spool. The spool never writes plaintext. See
[SECURITY.md](../../SECURITY.md).

**At open.** The spool reads the header of each leftover segment. It deletes segments of
another format, other files, and segments sealed with another key. It counts their
bytes in `spool_dropped_bytes` and logs one warning. If the key was created at this
start, the spool deletes every segment and resets `cursor.bin`.

### Link failure

At a link failure, the link thread ends the session and returns the data it did not send.
The router takes back the FIFO and sorts the frames by how far the socket got:

- Live frames that the socket did not fully accept go back to the router. These are the
  cut frame, every later frame, and, with TLS, the frames of the piece whose delivery
  could not be confirmed. They go to the head of the spool, or to a front list that is
  sent first after the reconnect when the spool already holds a backlog. The front list
  holds at most one block.
- Replay frames that the socket did not fully accept stay in the spool. The spool moves
  its read position back to the last frame the socket accepted, so the router reads them
  again.
- Frames that the socket fully accepted are not sent again.

### Allocation on the TLS path

Every stage up to the socket reuses its buffers, so the data path makes no heap
allocation per event. TLS is the exception. rustls allocates one buffer for each record it
encrypts (up to 16 KiB of plaintext), and frees it after the record is sent. That is
about 2,500 allocations a second at 41 MB/s. The unbuffered rustls API does not avoid
this: in rustls 0.23.46 its `encrypt` still allocates the record and then copies it.
Removing the allocation would mean writing a record layer outside rustls, for an expected
gain below 1 % of one CPU. That was evaluated and dropped on 2026-11-04. The buffered
`ClientConnection` stays.

## Startup and shutdown order

The services start from the receiver back toward the source. Each step must succeed
before the next one starts.

1. Connect the link. A refused connection does not stop the service, because the link
   retries with backoff. A TLS or protocol failure does stop it.
2. Check the spool folder, unwrap the spool key once, and open the spool. A failed check or
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

A 10-second watchdog ends the process if shutdown takes longer. The sender threads finish
in about 3.2 s: the last partial block (0.1 s), the link budget (1 s), one link poll, and
the STOP/FINISH exchange (2 s), which overlaps the spool seal. See
[Startup and shutdown deadlines](../guide/configuration.md#startup-and-shutdown-deadlines)
and [Completion and resource ownership](ring-buffer.md#completion-and-resource-ownership).

## Where events are lost

Each loss counter names one stage. Find the stage where the event disappeared, then read
its counter. [Loss counters](measurement-terms.md#loss-counters-live-service) define each
counter. The [Tune capture for load](../guide/tuning.md) guide explains how to reduce each loss.

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

With a spool, a link failure counts no frame. The frames go back to the router or stay in
the spool, and they are sent again. At a shutdown that follows a spill, the one live block
that comes back from the link is older than the spool backlog. It cannot go ahead of the
backlog, so the router counts it in `disconnected_dropped`.

`disconnected_dropped` counts frames that the link did not hand to the socket in full and
that no spool kept. A cut frame reaches the receiver only as a truncated copy, which the
receiver discards. The one exception is the TLS 1.3 key update window (risk 19 in the
[slow-receiver design](slow-receiver.md)). If the socket took a whole data record and the
connection then failed while a key update was pending, the receiver holds that record, and
the counter includes its frames. Bytes that the kernel accepted before a failure are not
counted, even if the kernel never sent them. So the counter can miss loss, and it is not an
upper bound on loss.

`replayed` and `bytes_sent` count bytes the socket accepted. The same kernel loss applies
to them, so `replayed` is not a delivery count.

## Regenerate the animation

The [animation sources](../../scripts/docs/pipeline-animation/README.md) are in the
repository. Change them when a stage changes, and rebuild the GIF.
