# Slow receiver: link writer and progress liveness

> **Status:** Current. · **Describes:** the sender on this branch. · **Evidence:** source review on 2026-11-05. Lab results are in the [validation record](../history/slow-receiver-validation.md). · **Reviewed:** 2026-11-05

## Summary

Live capture data flows through two threads. The **router** (`dnstap-sender`) owns the spool, the on-disk queue, and never waits on the network. The **link thread** (`dnstap-link`) owns the socket and is the only thread that writes to it. A deep in-memory queue, the **link FIFO**, sits between them. A short stall in either thread costs memory, not frames and not a reconnect.

A session (one connected run of TCP, TLS, and the Frame Streams handshake) ends only when data waits and the receiver accepts no bytes for the stall timeout (default 15 seconds). When a session fails, unsent live data goes back to the router in order, and replay data (frames read back from the spool) stays on disk until the receiver accepts it.

## Units

A query makes two dnstap frames of 103 bytes on average (lab measurement). So 10,000 queries per second is 2.06 MB/s, 50,000 is 10.3 MB/s, and 200,000 is 41.2 MB/s.

## Requirements

- **I1. Order.** While the spool holds a backlog, every new live block goes to the spool. Frames reach the receiver in capture order.
- **I2. One counter per loss.** Each lost frame counts in exactly one of `queue_dropped`, `disconnected_dropped`, and `spool_dropped`.
- **I3. Delivery.** Delivery is at most once, and at least once across a crash or kill. A normal stop repeats nothing. A frame counts as delivered only when the socket accepted its bytes.
- **I4. The parser never blocks.** It draws blocks from a fixed pool and allocates nothing per frame.
- **I5. Shutdown.** Shutdown takes about 5 s at most, inside the 10 s shutdown watchdog. The design target is about 3.2 s.
- **I6. Connect in the background.** The `dnstap-reconnect` helper thread runs connect and the handshakes. The backoff is 250 ms to 30 s.
- **R1. No send timeout on data.** After a blocking send times out, the connection is in an indeterminate state and must be closed (Microsoft Learn). The caller cannot tell whether some bytes were queued, so data writes use no socket timeout.

## Design

### Threads and ownership

| Thread | Owns | Waits on |
| --- | --- | --- |
| `dnstap-parser` | The block it fills. | Nothing. |
| `dnstap-sender` (router) | The parser channel, the front list, the `Spool`, the FIFO sender and a receiver clone, and the return channel. | One `crossbeam_channel::Select`, 50 ms timeout (`SENDER_POLL`). |
| `dnstap-link` | The `NetworkSink`, the FIFO receiver, the return sender, and the block in hand. | FIFO readiness or socket writability, at most 50 ms (`LINK_POLL`). |
| `dnstap-reconnect` | One connect and handshake attempt. | Blocking connect and handshakes, under `IO_TIMEOUT`. |

Code: `src/sender.rs` (shared types and the pool), `src/sender/router.rs`, `src/sender/link.rs`, and `src/network/wire.rs`. In `src/network/wire.rs`, `Uplink` provides reconnecting sessions (`NetworkSink` implements it), and `Wire` is the non-blocking data path of one session.

`LinkShared` holds an `up` flag (the link writes it, the router reads it), the take lock (a `Mutex<()>`), and the shutdown state. Both threads return blocks to one lock-free pool.

At start-up `up` is clear. The link thread enters link mode on the first session, sets `up`, and signals `QueuedSink::start`, which waits for that signal before the router starts. If the first session cannot enter link mode, the router sends blocks to the spool.

The router never waits on the socket, so only disk speed limits it. Each thread waits on one kind of event, and the partial-write state stays in one local loop on the link thread.

### The link FIFO and batched hand-offs

The link FIFO is a bounded channel of 256 blocks (`LINK_FIFO_BLOCKS`, 16 MiB) of `Live` or `Replay` items. The router keeps a receiver clone, so its sends succeed even after the link is gone. It learns of a vanished link from the return channel.

A waiting thread can lose a scheduler quantum before it runs. FIFO depth and batching cover that delay, so both threads run at normal priority and leave the DNS Server process its share.

- **Batching.** The link takes the next item as soon as it finishes one and waits only when the FIFO is empty. Each router wake routes every block it can and reads replay up to its credit.
- **Passive spill.** The router spills only when `try_send` returns `Full`. Routing depends only on FIFO state.
- **Take lock.** The link takes an item only under the take lock, with `try_recv`, and holds at most one item. It waits with `Select::ready_timeout`, which takes nothing.
- **Spill.** On `Full`, the router takes the lock and tries one more send. If that fails, it pops every FIFO item into a buffer preallocated for `LINK_FIFO_BLOCKS` and releases the lock after a few microseconds. It appends those blocks to the spool, oldest first, then the block that did not fit, and enters Draining.
- **Order at a spill.** While the router holds the lock, the link has finished every older item or holds exactly one. So the FIFO items and the new block reach the spool in capture order.
- **Front list.** The front list holds blocks that must go out before the FIFO. After a spill, the only live block older than the backlog is the one the link held, and Draining sends only replay batches and the front list. So a failure returns at most one live block (`FRONT_LIST_BLOCKS` = 1).

### Writing to the socket

After the handshakes, the link sets the socket to non-blocking mode. On `WouldBlock`, which queued nothing, it waits up to 50 ms for writability, checks the stall timer and the shutdown deadline, and writes again. STOP and FINISH run in blocking mode with a fresh `IO_TIMEOUT`. Blocks go straight to the transport, past the `BufWriter` flush timers; the parser's 100 ms and 1 s partial-block timers bound the delay.

**TLS.** The data path calls the rustls connection directly. `write_all`, `BufWriter`, and `StreamOwned::write` lose the written count or delay errors on a non-blocking socket, so none of them may carry data. `conn.writer().write(piece)` returns how much plaintext rustls accepted. `conn.write_tls(&mut tcp)` writes TLS bytes and keeps the unwritten rest inside rustls, so `WouldBlock` never cuts a TLS record.

The writer feeds rustls one piece of at most `TLS_FEED_BYTES` (16 KiB, one record at the default fragment size), ending at a frame boundary. It drains `write_tls` until `wants_write()` is false before the next piece. The unconfirmed piece is the only plaintext whose fate is unknown at a failure. Plaintext sessions know the exact offset.

**TLS record limit.** At its confidentiality limit (2^24 records for TLS 1.2 with AES-GCM), rustls 0.23.46 queues `close_notify`, refuses the record, and still reports the length as written. `tls_offer` (`src/network/wire.rs`) compares the ciphertext rustls queued (`process_new_packets().tls_bytes_to_write()`, which does no I/O) with the plaintext it took. Each record adds a 5-byte header and a 16-byte tag, so fewer queued bytes mean rustls dropped the data. The offer then fails, and the data goes back at the hand-back. A refused piece of 10 bytes or fewer can pass once, and the next offer fails. The encoder never produces a frame that small.

### Liveness from progress

The link ends a session only when it has bytes to write and the receiver accepted none for the stall timeout. A call that accepts at least one byte is progress. With TLS, only bytes that `write_tls` put on the socket count. An idle link never stalls. The rule uses only the byte counts of the link's own calls.

The stall timeout is `--receiver-stall-seconds`: default 15, range 2 to 300. The minimum equals `IO_TIMEOUT`, so a handshake within its own deadline is never taken for a stall. A shorter stall fills the FIFO and then spills, with no loss and no reconnect. With no spool, a full FIFO means `queue_dropped` until the receiver resumes.

**Shutdown timing.** Once shutdown starts, the link stops at the earlier of the stall deadline and the link deadline.

| Step | Time |
| --- | --- |
| Final partial block, then the link budget | 1.1 s |
| Write in progress at the deadline, then the hand-back | up to 50 ms |
| STOP/FINISH | up to 2 s |
| Returned block, FIFO, and channel to the spool | up to 513 blocks (about 32 MiB), during STOP/FINISH |
| Total | about 3.2 s, longer only if the seal outlasts STOP/FINISH |

### Hand-back at a failure

This covers live data; [Spool rewind](#spool-rewind) covers replay data. When a session ends (a write error, a stall, or the shutdown deadline):

1. The link ends the session, clears `up`, and stops taking FIFO items.
2. It splits the block in hand by byte counts (`Split`: `sent`, `sent_end`, `unsent`). Fully accepted frames are sent. The cut frame and every later frame go back to the router as `Returned`, with the frames of an unconfirmed TLS piece. The receiver discards its truncated copy of the cut frame. For a replay batch, the link reports the accepted prefix with `ReplayDone`.
3. It sends `HandBackDone`, or at shutdown drops the return channel. It may reconnect, but it neither sets `up` nor writes until it gets a fence.
4. The router stops sending when it sees `up` false or `HandBackDone`, and drains the FIFO itself. Live blocks follow the returned block; replay items are released. It takes no parser block until step 5.
5. It places, in order, the returned block, the drained live blocks, and newer blocks: at the spool head with no backlog, or in the front list with a backlog. With no spool, they count once in `disconnected_dropped`.
6. It sends a fence, with the FIFO empty, and enters Down. Only then may the link set `up`.

**Invariants.**

- *No deadlock.* The link has at most `REPLAY_CREDIT` + 2 messages unread, which fits `RETURN_MESSAGES` (129). The fence channel has capacity 1 and is empty when the router sends. The link waits on it with a timeout and checks the shutdown deadline.
- *No double count.* Each FIFO item is received once, the block in hand belongs to the link, and a replay credit returns once.
- *No reorder.* The router stops producing before it drains, and the link stopped taking at step 1.
- *Shutdown during a failure.* The link does steps 1 and 2 and drops the return channel. The router treats a closed channel like `HandBackDone`.
- *Reconnect during a hand-back.* A new session stays unused until the fence, so every older block is placed first.

### Router states

The router tracks the link as `Up`, `Down`, or `Gone` (the link thread ended). `Router::destination` routes each live block to `Link` (the FIFO), `Spool`, or `Discard` (no spool and the link is down; count in `disconnected_dropped`). A live block goes to the spool when there is a backlog, replay is out, the front list holds a block, or the link is down.

| State | When | Live block | Replay | Leaves when |
| --- | --- | --- | --- | --- |
| Direct | Link up, no backlog | Front list first, then `try_send`. On `Full`, spill. | None out. | Spill: Draining. Link down or `HandBackDone`: HandBack. |
| Draining | Link up, backlog or replay out | Append to the spool. | Front list first, then one batch per free credit to the FIFO. | Nothing to read and every credit back: Direct. Link down: HandBack. |
| HandBack | Link ended its session | Wait in the parser channel. | None. | `HandBackDone` or channel close: drain, rewind, place, fence, Down. |
| Down | Link not up, hand-back done | Spool, or `disconnected_dropped` with no spool. | None. | Link up: Direct or Draining. |
| Closing | Shutdown, within the link budget | As above. | None. | Budget ends, or input closed and empty: Sealing. |
| Sealing | Past the budget, or input closed | Drop FIFO and fence senders, apply messages until the return channel closes, drain the FIFO, place blocks in the spool. | None. | Seal and join the link thread: Done. |

Each router turn, in order: take the link's messages, follow the link state, save the delivered position once, serve up to 256 parser blocks and then up to 1 MiB of replay, and sample the monitor. The router waits only when nothing moved. With no spool, a full FIFO stops the router, and the parser counts `queue_dropped`.

### Link states

| State | Event | Action | Next |
| --- | --- | --- | --- |
| Disconnected | Every 50 ms | Start a connect attempt on the reconnect helper. | Up and Idle on success. |
| Idle | An item is ready | Take one item under the take lock, at offset 0. Set `last_progress`. | Writing |
| Writing | Bytes accepted | Advance the offset, set `last_progress`. At a block end, release a live block or report `ReplayDone`, and take the next item. | Writing, or Idle when the FIFO is empty. |
| Writing | `WouldBlock` | Wait for writability, at most 50 ms. | Writing |
| Writing | Stall timeout, or a write error | End the session and start the backoff, as `connection_lost` does. | HandingBack |
| Writing or Idle | Shutdown deadline | Stop writing. | HandingBack, then Finishing |
| HandingBack | Steps 1 to 3 above | Clear `up`. Split the block in hand. Send `HandBackDone`, or drop the return channel at shutdown. | Fenced wait, or Finishing |
| Fenced wait | Fence received | A reconnect may finish meanwhile. Nothing is written. | Up and Idle, or Disconnected |
| Finishing | At a block boundary | Set blocking mode with `DeadlineTcp`. Send STOP, then FINISH, under `IO_TIMEOUT`. | Done |
| Finishing | Inside a frame | Close with no STOP. The frame writer refuses further frames. Nothing is logged. | Done |

If STOP or FINISH does not complete, `NetworkSink` still returns `Ok` and writes no log event and no counter, because every data frame was already accepted, handed back, or counted. The worst case is a link that finishes just inside the budget, then a closing exchange that runs to its 2 s bound: about 3 s. A stalled shutdown takes about 1.1 s.

### Loss at a failure

| Case | Counted loss |
| --- | --- |
| Failure in Direct | None. |
| Failure in Draining | None. Replay frames are read again. |
| TLS unconfirmed piece | None. Replay frames are read again; live frames return with the cut frame. |
| Shutdown with batches out | None. The batches stay on disk. |
| Shutdown with a backlog and a front block | At most one block, in `disconnected_dropped`. |
| Crash or kill | None. Up to 127 batches and the frames of the last router pass are sent again. |

Bytes the kernel held at a failure are the only data lost, and they stay uncounted. A spool frame ends delivered (`replayed`), evicted or unreadable (`spool_dropped`), or still on disk.

## Spool rewind

After a failure, a replay batch goes back to the spool, as live data goes back to the router.

| | Read position R | Delivered position D |
| --- | --- | --- |
| Meaning | End of the last batch the router read. | End of the last frame the session fully handed to the kernel. With TLS, the last frame of a piece that `write_tls` fully wrote. |
| Where | First segment not yet ended (`read_offset`, `frames_read`). | Head segment (`done_offset`, `frames_done`). |
| Moves forward | `read_batch`. | `Spool::delivered`, on a `ReplayDone`. |
| Moves back | `Spool::rewind`, to D. | Never. |

A *held* frame lies between D and R: read but not yet delivered. D never passes R, and held data stays on disk.

**Spans and reports.**

- `read_batch` records a span per batch: segment, start and end offsets, and frame count. A batch comes from one segment. Spans queue in read order, at most `REPLAY_CREDIT` long, and the link finishes batches in FIFO order, so each report belongs to the front span.
- `Return::ReplayDone { bytes, frames }` carries the prefix of a batch that the session fully handed to the kernel, ending at a frame boundary. At a failure, the link settles the batch in hand the same way (`Split::sent`, `Split::sent_end`).
- `Spool::delivered(bytes, frames)` pops the front span, moves D to the span start plus `bytes`, adds `frames` to `replayed`, and sweeps. `replayed` counts at delivery, so a frame read twice after a rewind counts once.
- `Spool::written_off` passes a batch without counting it as replayed, on two defensive paths that cannot run today: an item that arrives while the link is down, and a failed offer in `replay_step`. The batch counts once in `disconnected_dropped` and is reported with `Return::ReplayCounted`.

**Segment deletion.**

- The reader marks a segment `ended` at a STOP, at the end of an unsealed file, at a cut-off or corrupt frame, or at the good length of a segment closed after a write error.
- A sweep deletes the head segment while it has ended and `frames_done == frames_read`. It runs at the end of `delivered` and at the end of a read that ended a segment. The active segment never ends; the sweep deletes it only when it is the only segment, R is at the writer position, and all its frames are delivered.
- A deletion writes the cursor only when no span is out. With a span out, the cursor already holds D.
- `has_backlog()` is true while any segment remains. `has_unread()` means R is before the end of the data, and `replay()` tests it. `unread_bytes()` is measured from R.

**Rewind at the hand-back.**

1. The link settles the block in hand (`ReplayDone` or `Returned`), then sends `HandBackDone`.
2. The router applies each message in channel order, so each `ReplayDone` moves D, and then takes `HandBackDone` or the close.
3. It takes the FIFO. Live items go to the older list. Replay items return to the pool, uncounted, with their credit.
4. `Spool::rewind` drops the spans, resets each segment's read offset and frame count to its delivered values, and clears `ended` (keeping a counted-loss mark). It drops the reader, so the next read reopens at D. The cursor needs no write, because it never passes D.
5. It places the older blocks and sends the fence, as in the hand-back.

**Invariants.**

- *D has every finished batch.* `ReplayDone` precedes `HandBackDone` on one channel, and the channel delivers every message before it reports a close.
- *Nothing is sent twice.* Frames before D were fully accepted; a frame at or after D was not.
- *Order.* From D on, the spool holds the rest of the batch in hand, the FIFO batches, the unread data, and the live blocks Draining appended, in capture order.
- *A spill never meets a replay item.* A spill runs only in Direct, where no replay is out.

**Unconfirmed TLS piece.** Its frames go back with the rest: a replay batch to the spool, a live block to the router. A piece is at most one 16 KiB record, or one long first frame in several records. While `pending()` is true, the receiver holds only a prefix of the last data record and cannot decrypt it. The exception is risk 19.

**Cursor file (`cursor.bin`).**

- *Format.* 28 bytes, little-endian: magic `ETDC`, segment sequence (u64), byte offset (u64), and a check value (sequence XOR offset XOR a constant). One positional write, never flushed and never deleted. It holds no frame data and is stored in plaintext; segment records are sealed with AES-128-GCM (`src/spool/crypt.rs`).
- *Contents.* D, not R, so a crash resends replay data rather than losing it.
- *When written.* `Spool::checkpoint` writes D at most once per router pass, after the pass applied every report, and only if D moved: about 630 writes a second at 41.2 MB/s and 8,400 at the idle replay rate of 550 MB/s. The router also calls it in `hand_back` and `link_vanished`. The spool writes the cursor when it deletes the first segment and in `Spool::seal`, so a stop repeats nothing.
- *Crash.* The held data (up to 127 batches of at most 71,684 bytes: 8.0 MiB at 103-byte frames, 8.7 MiB at worst) and the frames delivered since the last write are sent again. A crash loses the live data in memory (up to 16 MiB) and the bytes the kernel held.
- *Failed write.* The file keeps the older D, so a crash repeats more frames, never fewer. The error counts in `spool_errors` and is logged. Reads continue. The spool retries after the 10 s pause, at the next deletion, and in `seal`.
- *Opening.* `Spool::open` applies D. A cursor naming a missing segment, or one older than every segment, starts at the first segment. A cursor inside a segment resumes at its offset and deletes the segments below it uncounted. A damaged file (wrong size, magic, or check value, or an offset inside a segment header) drops and counts the oldest segment.
- *Undeletable segment.* The deletion writes the cursor first, so the next start deletes the stray file without sending it. If that cursor write also fails, a crash can resend the file.

**Eviction and `--spool-max-mib`.**

- *The victim.* `Spool::evict_oldest(keep_last)` deletes the oldest segment that the reader has not ended and that holds no held data. Its frames count in `spool_dropped` in full. The cap path may delete the newest segment; the free-space floor path keeps it. Held segments count in `total_bytes`, `spool_bytes`, and the cap.
- *Order.* The victim is newer than the held data by at most P + 2S. The receiver still gets frames in capture order, with a gap after the held run.
- *A loss counted once.* A cut-off tail, corrupt rest, or unreadable rest counts when the reader first meets it. The segment keeps a mark (`cut`, `lost_frames`, `lost_bytes`) that survives a rewind. A read error or bad header ends the read segment at its read offset and counts the unread part (`Spool::abandon`).
- *Minimum cap.* Held bytes are at most P = 127 × 71,684 = 8.7 MiB. Held segments add the delivered prefix of D's segment and the unread rest of R's, each under one segment S = min(64 MiB, cap / 8), so they hold at most P + 2S. At `--spool-max-mib 64`, that is 24.7 MiB, which leaves at least 39 MiB (five or more segments) evictable. At larger caps, P < cap / 4. Tests with a cap below 64 MiB must keep the read-ahead below the cap.
- *Floor path.* If it finds only held segments, it refuses blocks for one 10 s pause and counts `spool_dropped`.

**Pool, turn length, and shutdown.**

- A replay batch is a pool block that takes one FIFO slot. In Draining the FIFO holds the front block and up to 127 batches: 128 of 256 slots. A rewind allocates no blocks.
- `serve` takes up to 256 parser blocks, then replay until a batch reaches `REPLAY_TURN_BYTES`, so a turn reads under 1 MiB + 70 KiB. One router wake queues all 127 batches, and at 41.2 MB/s the link then runs 0.20 s without the router.
- At shutdown `serve` reads no replay. `Router::seal` drops the FIFO and fence senders and applies every message until the return channel closes, so each finished batch moves D. It releases FIFO replay items uncounted, places older live blocks and the front list, and sends the parser channel's blocks to the spool. Then `Spool::seal` writes D and STOP.

## Constants

| Constant | Value | Notes |
| --- | --- | --- |
| `SEND_QUEUE_BLOCKS` | 256 | Parser-to-router channel, 16 MiB: twice one Server quantum of router pause (118 blocks). |
| `LINK_FIFO_BLOCKS` | 256 | Router-to-link FIFO, 16 MiB. |
| `FRONT_LIST_BLOCKS` | 1 | Live blocks that go out before the FIFO. |
| `POOL_BLOCKS` | 516 | 256 + 256 + 1, plus one each for the parser, router, and link. 68.5 MiB. |
| `BLOCK_CAPACITY` | 136 KiB | Blocks are sent at about 64 KiB (`BLOCK_SEND_BYTES`). |
| `REPLAY_BATCH_BYTES` | 64 KiB | A batch always takes its first frame, so it is at most 71,684 bytes. |
| `REPLAY_CREDIT` | 127 | Replay batches out at once. |
| `REPLAY_TURN_BYTES` | 1 MiB | Most the router reads from the spool in one turn. |
| `RETURN_MESSAGES` | 129 | `REPLAY_CREDIT` + 2. |
| `TLS_FEED_BYTES` | 16 KiB | One TLS record at the default fragment size. |
| `LINK_POLL`, `SENDER_POLL` | 50 ms | Link readiness wait; router input wait. |
| `IO_TIMEOUT` | 2 s | Connect, handshakes, STOP/FINISH. The stall minimum. |
| `SHUTDOWN_LINK_BUDGET` | 1 s | Time the link may spend after shutdown starts. |
| Segment size `S` | min(64 MiB, cap / 8) | `segment_bytes_for`. |
| Spool pause | 10 s | After a failed cursor write, a floor refusal, or a transient read error. |

A waiting thread can lose a quantum: 31.25 ms on a Windows client and 187.5 ms on Windows Server, or 20 and 118 blocks at 41.2 MB/s. The 256-block FIFO holds 0.41 s at that rate: two Server quanta with an 8 % margin. 118 replay batches cover a Server quantum at 41.2 MB/s; the credit of 127 adds a 7.6 % margin. A compile-time assert checks that the credit and the front block fit in the FIFO. A larger credit only widens the held data and the crash window.

Each 16 MiB queue holds about 8.1 s at 10,000 queries per second, 1.6 s at 50,000, and 0.41 s at 200,000. The autotuned kernel send buffer adds more.

## Risks and open questions

Other documents cite these numbers (risk 19 in `docs/design/data-path.md`). Keep them stable.

1. **Half-open receiver.** The stall timeout delays detection by up to 15 s. Without a spool, that is up to 15 s of `queue_dropped`. Accepted.
2. **Disk is the limit.** `queue_dropped` occurs if spool appends are slower than the capture input. Open: the append rate needs a lab measurement.
3. **Write wait counters.** `slow_writes` and `write_max_us` count the readiness wait per block; a wait of 5 ms or more counts once as slow. `write_max_recent_us` (counter 64) is the 60-second peak.
4. **Kernel send buffer.** The service leaves `SO_SNDBUF` unset, so Windows autotunes it. Bytes the kernel holds at a failure are lost uncounted: about 0.58 MB idle and 4.2 MB on a busy host in the lab. Accepted.
5. **Trickle receiver.** A receiver that accepts a few bytes before each stall timeout keeps the session forever. The saturated-link state reports it. Open.
6. **Cut frames.** The hand-back relies on the receiver discarding a frame cut by a closed connection. The lab confirmed this for the DNS collector.
7. **rustls API.** The data path relies on rustls 0.23 (0.23.46 in `Cargo.lock`): `writer()`, `write_tls`, `wants_write`, and the default fragment size. On a version change, rerun the TLS tests in `src/network/wire_tests.rs`.
8. **Spool rewind.** See [Spool rewind](#spool-rewind).
9. **Server quantum at full rate.** The FIFO must cover a lost Windows Server quantum. In the lab on 12 vCPUs, 22 of 22 runs had no spill and no `queue_dropped`, up to 40.5 MB/s with 24 busy threads.
10. **Replay on a saturated host.** Replay must drain faster than 41.2 MB/s. On the build host it ran at 51 to 368 MB/s with 2 busy threads per CPU. On a Windows Server guest it drained a 1.46 GB backlog in 2.1 s (about 700 MB/s).
11. **Take lock.** A router preempted while it holds the take lock makes the link wait up to one quantum. The lock covers only the queue pops of a spill. Accepted.
12. **Duplicates after a crash.** The receiver can see the held data and the last router pass twice. A receiver that needs exactly-once must deduplicate. Accepted.
13. **Disk in use.** Held data, up to P + 2S, is outside the cap's reach: 24.7 MiB at the 64 MiB minimum cap and about 137 MiB with 64 MiB segments (caps of 512 MiB and above).
14. **Live data behind read-ahead.** In Draining, a live block waits behind up to 8 MiB of FIFO replay: 0.2 s at 41.2 MB/s and 1.7 s at 5 MB/s. Accepted.
15. **Router turn length.** A replay read adds about 10 ms to a turn at an assumed 100 MB/s disk read rate. If `send_queue_depth_max` rises during a drain, lower `REPLAY_TURN_BYTES`. Open.
16. **Counter meaning.** `replayed` counts at delivery. `disconnected_dropped` holds replay frames only on the two defensive paths and with no spool. `bytes_sent` counts the accepted prefix of a cut frame again when the frame is resent.
17. **Duplicates at a failure.** Only risk 19 can cause one, and the rewind tests check this.
18. **Eviction victim.** The victim is the oldest segment with no held data, newer than the held data by at most P + 2S.
19. **TLS 1.3 key update window.** rustls can queue a `KeyUpdate` (about 22 bytes) after a data record, at the AES-GCM record limit or on `refresh_traffic_keys()`. If the socket took the whole record and the connection resets while the `KeyUpdate` is pending, the record's frames arrive twice. The window is once per 2^23 or more records. The service never calls `refresh_traffic_keys()`.

## Tests

Tests live in `src/sender/`, `src/spool/`, and `src/network/wire_tests.rs`. Test seams:

- `LinkThread::step` and `Router::turn` each run one pass without waiting; `serve` and `run` loop over them. The batching test runs both on one thread in a fixed order.
- `LinkHooks::in_take` runs on the link thread while it holds the take lock.
- `script.rs` provides a scripted transport with a manual clock. `TlsKit::with_record_limit` lowers the record limit through the cipher suite's own setting.
- `Spool::read_and_deliver` (`src/spool/test_support.rs`) reads a batch and reports it delivered at once.

Named tests for the main behaviors, by file:

- `link_tests.rs`: `a_failure_returns_only_the_block_in_hand_and_leaves_the_fifo_to_the_router`, `a_tls_piece_whose_delivery_is_unconfirmed_goes_back_with_the_frames_after_it`, `a_cut_replay_batch_reports_the_frames_taken_and_counts_nothing`, `a_link_that_waits_for_the_fence_ends_at_the_shutdown_deadline`.
- `fault_tests.rs`: `a_spill_while_the_link_holds_the_take_lock_keeps_capture_order_and_one_front_block`, `a_pause_of_one_server_quantum_never_touches_the_spool` (the two-quantum case is next to it), `a_frame_cut_by_a_failure_is_sent_again_in_full_on_the_new_session_and_arrives_once`, `a_tls_piece_whose_delivery_is_unconfirmed_goes_back_and_arrives_once`.
- `saturation_tests.rs`: `a_receiver_that_accepts_nothing_for_the_stall_timeout_loses_the_session_and_no_frame`.
- `router_tests.rs`: `a_hand_back_with_a_cut_replay_batch_reads_again_from_the_cut_frame`, `one_router_wake_queues_the_whole_credit_and_the_link_drains_it_without_the_router`, `a_batch_that_cannot_enter_the_fifo_is_counted_once_and_not_read_again`, `a_link_thread_that_vanishes_with_batches_out_gives_them_back_to_the_spool`.
- `spool_tests.rs`: `a_rewind_reads_the_held_batches_again_in_order_across_a_segment_end`, `a_rewind_after_a_partial_report_starts_at_the_first_frame_not_taken`, `a_rewind_needs_no_cursor_write_and_a_stop_after_it_repeats_nothing`, `a_cursor_write_failure_leaves_an_older_position_and_does_not_pause_reads`, `eviction_skips_held_segments_and_counts_only_what_it_deletes`.
- `wire_tests.rs`: `a_tls_session_at_its_record_limit_fails_instead_of_dropping_data`, `a_piece_of_the_feed_size_is_one_tls_record`.
- `e2e_tests.rs`: `a_run_stopped_in_the_middle_of_a_replay_leaves_a_cursor_the_next_run_resumes_from`, `a_link_that_fails_mid_replay_loses_only_what_the_kernel_held_and_sends_nothing_twice`.
- `sender_tests.rs`: `a_receiver_that_never_answers_finish_does_not_fail_the_shutdown`.

The spool property tests (`src/spool/property_tests.rs`) and the fuzz target (`src/spool/fuzzing.rs`) end batches in a failed link at random points and rewind. They check conservation (appended = `replayed` + `spool_dropped` + on disk), order, and no repeats. Several tests assert `pool_fallbacks == 0`.
