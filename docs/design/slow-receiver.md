# Slow receiver: link writer and progress liveness

> **Status:** Current. Open item: lab repeat of the B2 replay drain on Windows Server (plan task L). · **Describes:** the sender on this branch. · **Evidence:** source review on 2026-11-05. The B2 throughput numbers come from the build host on 2026-11-04, not from Windows Server. Lab results are in the [plan](../plans/slow-receiver-plan.md). · **Reviewed:** 2026-11-05

## Summary

Live capture data flows through two threads. The **router** (`dnstap-sender`) owns the spool, the on-disk queue, and never waits on the network. The **link thread** (`dnstap-link`) owns the socket and is the only thread that writes to it. A deep in-memory queue, the **link FIFO**, sits between them. A short stall in either thread therefore costs memory, not frames and not a reconnect.

A session (one connected run of TCP, TLS, and the Frame Streams handshake) ends only when the receiver accepts no bytes for the stall timeout, while data waits. The default is 15 seconds.

When a session fails, unsent live data goes back to the router in order. Replay data (frames read back from the spool) stays on disk until the receiver accepts it. So a failure does not lose frames. The one exception is a crash or kill. Then frames that were read ahead are sent again. A normal stop repeats nothing.

## Units

A query makes two dnstap frames of 103 bytes on average (lab measurement). So 10,000 queries per second is 2.06 MB/s, 50,000 is 10.3 MB/s, and 200,000 is 41.2 MB/s.

## Requirements

- **I1. Order.** While the spool holds a backlog, every new live block goes to the spool. Frames reach the receiver in capture order.
- **I2. One counter per loss.** Each lost frame counts in exactly one of `queue_dropped`, `disconnected_dropped`, and `spool_dropped`.
- **I3. Delivery.** Delivery is at most once, except across a crash (B2). A frame counts as delivered only when the socket accepted its bytes.
- **I4. The parser never blocks.** It never allocates per frame. It draws blocks from a fixed pool.
- **I5. Shutdown.** Shutdown takes about 5 s at most, inside the 10 s shutdown watchdog. The design target is about 3.2 s.
- **I6. Connect in the background.** Connect and handshakes run on the `dnstap-reconnect` helper thread. The backoff is 250 ms to 30 s.
- **R1. No send timeout on data.** Microsoft Learn says that after a blocking send times out, the connection is in an indeterminate state and must be closed. It does not say whether some bytes were queued. So a session must never continue after a send that returned a timeout. The design avoids timeouts on data writes altogether.

## Design

### Threads and ownership

| Thread | Owns | Waits on |
| --- | --- | --- |
| `dnstap-parser` | The block it fills. | Nothing. |
| `dnstap-sender` (router) | The parser channel, the front list, the `Spool`, the FIFO sender and a clone of its receiver, and the return channel. | One `crossbeam_channel::Select` with a 50 ms timeout (`SENDER_POLL`). |
| `dnstap-link` | The `NetworkSink`, the FIFO receiver, the return sender, and the block in hand. | FIFO readiness, or socket writability, for at most 50 ms (`LINK_POLL`). |
| `dnstap-reconnect` | One connect and handshake attempt. | Blocking connect and handshakes, under `IO_TIMEOUT`. |

Code: `src/sender.rs` (shared types and the pool), `src/sender/router.rs`, `src/sender/link.rs`, and `src/network/wire.rs`. The link thread reaches the transport through two traits in `src/network/wire.rs`. `Uplink` provides reconnecting sessions, and `NetworkSink` implements it. `Wire` is the non-blocking data path of one session. Tests run the same link code on a scripted transport with a manual clock.

Shared state, in `LinkShared`: an `up` flag (the link thread writes it, the router reads it), the take lock (a `Mutex<()>`), and the shutdown state. Both threads return blocks to one lock-free pool.

*Start-up.* `up` starts clear. The link thread enters link mode on the first session, sets `up`, and signals `QueuedSink::start`, which waits for that signal before the router starts. If the first session cannot enter link mode, it is lost, and the router sends blocks to the spool.

*Why two threads.* The router never waits on the socket, so only disk speed limits it. Each thread waits on one kind of event. No single call waits on both a channel and a socket, so a one-thread design would need short alternating waits. The partial-write state stays in one local loop on the link thread. The one-thread alternative was rejected for these reasons.

### The link FIFO and batched hand-offs

The link FIFO is a bounded channel of 256 blocks (`LINK_FIFO_BLOCKS`, 16 MiB). Its items are `Live` or `Replay` blocks. The router sends, and the link thread receives. The router also keeps a receiver clone, so a send never fails because the link is gone. The router learns of a vanished link from the return channel.

*Why deep and batched.* A thread that waits for work can lose a scheduler quantum before it runs. On a busy host, a shallow FIFO moved only about one quantum's worth of blocks at a time. With a 2-block FIFO, review V2 measured the collapse under load. The fix is depth plus batching, not a higher thread priority. Priority was rejected because the service must not compete with the DNS Server process.

- **Batching.** The link takes the next item as soon as it finishes one. It waits only when the FIFO is empty. Each router wake routes every block it can take and reads replay up to its credit.
- **Passive spill.** The spill happens only when `try_send` returns `Full`. No routing decision reads TCP statistics. `SIO_TCP_INFO` is not used.
- **Take lock.** The link takes an item only while it holds the take lock, with `try_recv`. It holds at most one item at a time. It waits with `Select::ready_timeout`, which takes nothing.
- **Spill.** On `Full`, the router takes the lock and pops every FIFO item into a buffer preallocated for `LINK_FIFO_BLOCKS`. It releases the lock after a few microseconds. Then it appends those blocks to the spool, oldest first, then the block that did not fit. It enters Draining. Under the lock, the router first tries one more send. If room has appeared, it sends and spills nothing.
- **Order at a spill.** Let x1 to xn be the FIFO items when the router takes the lock. The link has finished every older item or holds exactly one. It cannot take an item while the lock is held. So x1 to xn and the new block reach the spool in capture order.
- **Front list (at most one block).** The front list holds blocks that must go out before the FIFO. After a spill, the only live block older than the backlog is the one the link held. In Draining the router sends only replay batches and the front list. So a failure returns at most one live block. The bound is `FRONT_LIST_BLOCKS` = 1.

### Writing to the socket

After the handshakes, the link thread sets the socket to non-blocking mode. On `WouldBlock`, which queued nothing, it waits for writability for at most 50 ms. Then it checks the stall timer and the shutdown deadline, and writes again. No data write has a socket timeout, so R1 holds. STOP and FINISH run in blocking mode with a fresh `IO_TIMEOUT`.

Blocks go straight to the transport. The `BufWriter` flush timers do not run on this path. The parser's 100 ms and 1 s partial-block timers bound the delay instead.

**TLS.** The data path calls the rustls connection directly, for two reasons:

- On a non-blocking socket, `write_all` returns `WouldBlock` without the count it wrote. `BufWriter` hides a second offset. `StreamOwned::write` reports a socket error only on the next call. None of these may carry data.
- `conn.writer().write(piece)` returns how much plaintext rustls accepted. `conn.write_tls(&mut tcp)` writes TLS bytes and keeps the unwritten rest inside rustls. So `WouldBlock` never cuts a TLS record.

The writer feeds rustls one piece of at most `TLS_FEED_BYTES` (16 KiB, one record at the default fragment size). The piece ends at a frame boundary. The writer drains `write_tls` until `wants_write()` is false before it feeds the next piece. The unconfirmed piece is the only plaintext whose fate is unknown at a failure. Plaintext sessions know the exact offset.

**TLS record limit (found in review V4).** At its confidentiality limit (2^24 records for TLS 1.2 with AES-GCM), rustls 0.23.46 queues `close_notify`, refuses the record, and still reports the length as written. The link would then see every block as complete, the stall timer would never fire, and frames would drop silently. `tls_offer` (`src/network/wire.rs`) checks the ciphertext rustls queued (`process_new_packets().tls_bytes_to_write()`, which does no I/O) against the plaintext it took. Each record is longer than its plaintext by a 5-byte header and a 16-byte tag. So if the queued bytes are shorter, the data was dropped and the offer fails. The data is then unconfirmed and goes back at the hand-back. The check needs no record-count constant. One gap: a refused piece of 10 bytes or fewer can pass once. The next offer then fails. The encoder never produces a frame that small.

### Liveness from progress (H2)

**Rule.** The link ends a session only when it has bytes to write and the receiver accepted none for the stall timeout. A call that accepts at least one byte is progress. With TLS, only bytes that `write_tls` put on the socket count. rustls taking plaintext is not progress. An idle link never stalls. The rule uses only the byte counts of the link's own calls. It never reads TCP statistics.

**Stall timeout.** `--receiver-stall-seconds`, default 15, range 2 to 300. The minimum equals `IO_TIMEOUT`, so a handshake within its own deadline is never taken for a stall.

**Rejected: a blocking send with `SO_SNDTIMEO` equal to the stall timeout.** A 15-second send cannot stop within the 1-second link budget. Closing the socket under it leaves the queued bytes unknown, so the block could not be split.

**Shutdown timing.** Once shutdown starts, the link stops at the earlier of the stall deadline and the link deadline. The stall timeout never extends shutdown.

| Step | Time |
| --- | --- |
| Final partial block, then the link budget | 1.1 s |
| Write in progress at the deadline, then the hand-back | up to 50 ms |
| STOP/FINISH | up to 2 s |
| Returned block, FIFO, and channel to the spool | up to 513 blocks (about 32 MiB), at the same time as STOP/FINISH |
| Total | about 3.2 s, or longer only if the seal outlasts STOP/FINISH |

A stall shorter than the timeout fills the FIFO and then spills. That costs no loss and no reconnect. With no spool, a full FIFO means `queue_dropped` until the receiver resumes.

### Hand-back at a failure (B1)

B1 covers live data. Replay data is covered by B2. When a session ends (a write error, a stall, or the shutdown deadline):

1. The link ends the session, clears `up`, and stops taking FIFO items.
2. It splits the block in hand by byte counts (`Split`: `sent`, `sent_end`, `unsent`). Fully accepted frames are sent. The cut frame, which the socket accepted only in part, and every later frame go back to the router as `Returned`. With TLS, the frames of the unconfirmed piece go back too. A cut frame is not lost: the receiver saw only a truncated copy, which it discards. A replay batch is not returned. The link reports the accepted prefix with `ReplayDone` (B2).
3. It sends `HandBackDone`. At shutdown it drops the return channel instead. It may reconnect, but it does not set `up` or write until it gets a fence.
4. The router stops sending when it sees `up` false or `HandBackDone`. It drains the FIFO itself. Live blocks follow the returned block. Replay items are released (B2). It takes no parser block until step 5.
5. It places, in order, the returned block, the drained live blocks, and then newer blocks. With no backlog they go to the spool head. With a backlog they go to the front list, which is sent first after the reconnect. With no spool, they count once in `disconnected_dropped`.
6. It sends a fence, with the FIFO empty, and enters Down. Only now may the link set `up`.

**Why it holds.**

- *No deadlock.* The link has at most `REPLAY_CREDIT` + 2 messages unread. The return channel holds `RETURN_MESSAGES` (129). The fence channel has capacity 1 and is empty when the router sends. The link waits on it with a timeout and checks the shutdown deadline.
- *No double count.* Each FIFO item is received once. The block in hand belongs to the link. A replay credit returns once.
- *No reorder.* The router stops producing before it drains. The link stopped taking at step 1.
- *Shutdown during a failure.* At the deadline the link does steps 1 and 2 and drops the return channel. The router treats a closed channel like `HandBackDone`.
- *Reconnect during a hand-back.* A session that finishes early stays unused until the fence. Nothing is written before every older block is placed.

### Router states

The router tracks the link as `Up`, `Down`, or `Gone` (the link thread ended). `Router::destination` picks a route for each live block: `Link` (offer to the FIFO), `Spool` (append to the spool), or `Discard` (no spool and the link is down; count in `disconnected_dropped`). A live block goes to the spool when there is a backlog, replay is out, the front list is not empty, or the link is down.

| State | When | Live block | Replay | Leaves when |
| --- | --- | --- | --- | --- |
| Direct | Link up, no backlog | Front list first. Then `try_send` to the FIFO. On `Full`, spill and enter Draining. | None out. | Spill: to Draining. Link down or `HandBackDone`: to HandBack. |
| Draining | Link up, backlog or replay out | Append to the spool. | Front list first. Then, per free credit, read a batch into a pool block and send it to the FIFO. | A read finds nothing and every credit is back: to Direct. With B2 this happens at the last `ReplayDone`. Link down: to HandBack. |
| HandBack | The link ended its session | Wait in the parser channel. | None. | `HandBackDone` or channel close: drain, rewind (B2), place, fence, to Down. |
| Down | Link not up, hand-back done | Append to the spool, or count in `disconnected_dropped` with no spool. | None. | Link up: to Direct or Draining. |
| Closing | Shutdown started, within the link budget | As above. | None. | Budget ends, or input is closed and empty: to Sealing. |
| Sealing | Past the budget, or input closed | Drop the FIFO and fence senders. Apply messages until the return channel closes. Drain the FIFO. Place the blocks in the spool. | None. | Seal and join the link thread: Done. |

Each router turn does the following, in order: take the link's messages, follow the link state, save the delivered position once (B2), serve parser blocks (up to 256 per turn) and then replay (up to 1 MiB per turn), and sample the monitor. The router waits only when nothing moved. The router is the only FIFO producer, and it has at most `REPLAY_CREDIT` replay batches out. With no spool, a full FIFO stops the router, and the parser counts `queue_dropped`.

### Link states

| State | Event | Action | Next |
| --- | --- | --- | --- |
| Disconnected | Every 50 ms | Start a connect attempt on the reconnect helper. | Up and Idle on success. |
| Idle | An item is ready | Take one item under the take lock. Start at offset 0. Set `last_progress`. | Writing |
| Writing | Bytes accepted | Advance the offset and set `last_progress`. At a block end, release a live block or report `ReplayDone`. Take the next item at once. | Writing, or Idle when the FIFO is empty. |
| Writing | `WouldBlock` | Wait for writability, at most 50 ms. | Writing |
| Writing | No progress for the stall timeout, or a write error | End the session and start the backoff, as `connection_lost` does. | HandingBack |
| Writing or Idle | Shutdown deadline | Stop writing. | HandingBack, then Finishing |
| HandingBack | (steps 1 to 3 above) | Clear `up`. Split and return or count the block in hand. Send `HandBackDone`, or drop the return channel at shutdown. | Fenced wait, or Finishing |
| Fenced wait | Fence received | A reconnect may finish meanwhile. Nothing is written. | Up and Idle, or Disconnected |
| Finishing | At a block boundary | Set the socket to blocking with `DeadlineTcp`. Send STOP, then FINISH, under `IO_TIMEOUT`. | Done |
| Finishing | Inside a frame | Close with no STOP. The frame writer refuses further frames, and nothing is logged. | Done |

**Closing rules.** A STOP or FINISH that does not complete never fails the service. `NetworkSink` returns `Ok` and writes no log event and no counter, because every data frame was already accepted, handed back, or counted. The shutdown worst case is a link that finishes just inside the budget, then a closing exchange that runs to its 2-second bound: about 3 s. A stalled shutdown takes about 1.1 s.

### Loss at a failure

| Case | Counted loss |
| --- | --- |
| Failure in Direct | None. |
| Failure in Draining (replay in the FIFO or in hand) | None. The frames are read again. |
| TLS unconfirmed piece, replay | None. The frames are read again. |
| TLS unconfirmed piece, live | None. Returned to the router with the cut frame. |
| Shutdown with batches out | None. The batches stay on disk. |
| Shutdown with a backlog and a front block | At most one block, in `disconnected_dropped`. |
| Crash or kill (not a stop) | None lost. Up to 127 batches and the frames of the last router pass are sent again. The receiver may see them twice. |

Bytes the kernel held at a failure are the only data still lost, and they stay uncounted. A frame counts in at most one counter. A spool frame ends in one of three ways: delivered (`replayed`), evicted or unreadable (`spool_dropped`), or still on disk.

## B2. Spool rewind

B1 gives unsent live data back. B2 does the same for replay data: a replay batch can go back to the spool after a failure.

**Design.** The spool keeps two positions. A *held* frame is one that was read but not yet delivered. Held data stays on disk.

| | Read position R | Delivered position D |
| --- | --- | --- |
| Meaning | End of the last batch the router read. | End of the last frame whose bytes the session fully handed to the kernel. With TLS, the end of the last frame of a piece that `write_tls` fully wrote. |
| Where | In the first segment not yet ended (`read_offset`, `frames_read`). | In the head segment (`done_offset`, `frames_done`). |
| Moves forward | `read_batch`. | `Spool::delivered`, when the router takes a `ReplayDone`. |
| Moves back | `Spool::rewind`, to D. | Never. |

D never passes R. The spool keeps the data between them on disk.

**Spans and reports.**

- `read_batch` records a span for each batch: the segment, the start and end offsets, and the frame count. A batch comes from one segment. Spans queue in read order, at most `REPLAY_CREDIT` long. The link finishes batches in FIFO order, and live blocks make no span. So each report belongs to the front span.
- `Return::ReplayDone { bytes, frames }` carries the prefix of a batch that the session fully handed to the kernel. The prefix ends at a frame boundary. A whole batch reports its totals. At a failure, the link settles the batch in hand with the same message (`Split::sent`, `Split::sent_end`). The cut frame and the frames after it are not counted and stay in the spool.
- A batch in the FIFO that the link never took is released by the router at the drain.
- `Spool::delivered(bytes, frames)` pops the front span, moves D to the span start plus `bytes`, adds `frames` to `replayed`, and sweeps (below). A report with no span out is ignored. That cannot happen.
- `replayed` counts at delivery, not at the read. A frame read twice after a rewind would otherwise count twice, and a perfmon counter must not fall.
- `Spool::written_off` passes a batch without counting it as replayed. It serves two defensive paths: an item that arrives while the link is down, and a failed offer in `replay_step`. The batch counts once, in `disconnected_dropped`, and is reported with `Return::ReplayCounted`. Neither path can run today.

**Segment deletion.**

- The reader marks a segment `ended` at a STOP, at the end of an unsealed file, at a cut-off or corrupt frame, or at the good length of a segment closed after a write error. A rewind clears `ended`. The mark for a counted loss is kept (see Eviction).
- A sweep deletes the head segment while it has ended and `frames_done == frames_read`. The sweep runs at the end of `delivered`, and at the end of a read that ended a segment. A segment with no frame left to deliver gets no report, so the sweep is needed there.
- The active segment never ends, because more data can arrive. When it is the only segment, R is at the writer position, and `frames_done == frames_read`, the sweep deletes it and closes the writer.
- A deletion writes the cursor only when no span is out. With a span out, the cursor already holds D, and the deletion writes nothing.
- `has_backlog()` is true while any segment remains. It means "data not yet delivered." `has_unread()` means R is before the end of the data. `replay()` tests `has_unread()`. `unread_bytes()` is measured from R, and the monitor reads it.

**Rewind at the hand-back.** The router rewinds only after it has applied everything the link sent and taken the FIFO.

1. The link settles the block in hand: `ReplayDone` with the accepted prefix for a batch, or `Returned` for a live block. Then it sends `HandBackDone`.
2. The router applies each message in channel order. Each `ReplayDone` moves D. Then it takes `HandBackDone` or the channel close.
3. It takes the FIFO. Live items go to the older list. Replay items go back to the pool and give their credit back. They are not counted.
4. `Spool::rewind` drops the spans. It sets every segment's read offset and frame count to its delivered values, and clears `ended`, except the counted-loss mark. It drops the reader, so the next read reopens the file at D. The cursor file needs no write, because it never passes D.
5. It places the older blocks and sends the fence, as in B1.

**Why it holds.**

- *D has every finished batch.* The link sends `ReplayDone` before `HandBackDone`, on one channel. The router rewinds after `HandBackDone` or the close, and the channel delivers every message before it reports a close.
- *Nothing is sent twice.* Frames before D were fully accepted. A frame at or after D was not. The receiver got only a truncated copy of a cut frame, and it discards that copy.
- *Order.* From D on, the spool holds data in capture order: the unsent rest of the batch in hand, the batches that were in the FIFO, the unread data, and the live blocks that Draining appended. The older blocks go to the front list when a backlog exists, and to the spool head when none does. A failure with no span out finds D equal to R, so the rewind does nothing.
- *The front list stays at one block.* Draining sends no live block to the FIFO, whatever the credit.
- *A spill never meets a replay item.* A spill runs only in Direct, where no replay is out.

**Unconfirmed TLS piece (decided 2026-11-04).** The frames of an unconfirmed piece go back with the rest. A replay batch goes to the spool. A live block goes to the router, like the cut frame. Those frames cannot have reached the receiver whole:

- A piece holds at most 16 KiB, one TLS record. The exception is one long first frame. Then the piece is that frame, in several records.
- `pending()` is true while some queued ciphertext has not reached the socket. TCP is in order, so the receiver holds a prefix of the pending bytes. If the last of those bytes is the tail of a data record, the receiver holds a prefix of that record and cannot decrypt it.
- **Exception.** In TLS 1.3, rustls can queue a `KeyUpdate` (about 22 bytes) after a data record. That happens at the AES-GCM record limit or on `refresh_traffic_keys()`. If the socket takes the whole data record, and the connection then resets while the `KeyUpdate` is still pending, the receiver already holds the record, and its frames arrive twice. The window is the time to write about 22 bytes, once per 2^23 or more records. The service never calls `refresh_traffic_keys()`. No code change (risk 19).

**Cursor file (`cursor.bin`).**

- *Format.* 28 bytes, little-endian: magic `ETDC`, the segment sequence (u64), the byte offset (u64), and a check value (sequence XOR offset XOR a constant). It is one positional write. It is never flushed and never deleted. Segment records are sealed with AES-128-GCM (`src/spool/crypt.rs`). The cursor holds no frame data, so it is not encrypted.
- *Contents.* D, not R.
- *When written.* `Spool::checkpoint` writes D at most once per router pass, after the pass applied every report, and only if D moved. The router calls it in each pass after collecting returns and following the link. It also calls it in `hand_back` and `link_vanished`. The spool writes the cursor when it deletes the first segment, and in `Spool::seal`.
- *What a crash repeats.* The frames the file does not cover: the held data (up to 127 batches) and the frames delivered since the last write. A batch is at most 71,684 bytes. Held data is 8.0 MiB at 103-byte frames and 8.7 MiB in the worst case. The receiver may see these frames twice. A crash still loses the live data in memory (the parser channel and the FIFO, up to 16 MiB) and the bytes the kernel held.
- *A stop repeats nothing.* `Spool::seal` writes D, with or without held batches. The next run starts at the frame after the last one the link took.
- *Write rate.* At most one write per router pass that applied a report: about 630 a second at 41.2 MB/s, and about 8,400 a second at the idle replay rate of 550 MB/s. Each deleted segment adds one write (about one per 64 MiB).
- *A failed write.* The file keeps the older D. A crash then repeats more frames, never fewer. The error counts in `spool_errors` and is logged like any spool I/O error. Reads do not pause for it. The spool retries after the pause (10 s), at the next deletion, and in `seal`.
- *Opening.* `Spool::open` applies D. A cursor that names a missing segment, or one older than every segment, starts reading at the first segment's start, because the segments between were already delivered. A cursor inside an existing segment resumes at its offset, and the segments below it are deleted uncounted. A damaged file (wrong size, magic, or check value, or an offset inside a segment header) is not a crash. The oldest segment is dropped and counted. That limits what a damaged file can repeat.
- *A segment that cannot be deleted.* The deletion writes the cursor first, so the file already names a position past the stray file. The next start deletes the stray file and does not send it again. If that cursor write also fails, a crash can send the stray file again. At-least-once allows that.
- *Why at least once.* A cursor at R, written before each batch leaves, would give at most once. The design resends instead, so a crash loses no replay data.

**Eviction and `--spool-max-mib`.**

- *The cap.* Held segments are on disk. They count in `total_bytes`, `spool_bytes`, and the cap.
- *The victim.* `Spool::evict_oldest(keep_last)` deletes the oldest segment that the reader has not ended and that holds no held data (its read offset is not past its delivered offset). Held segments are skipped. The cap path may delete the newest segment. The free-space floor path keeps the newest segment.
- *Counters.* A victim was never read, so its frames count in `spool_dropped` in full. A leftover segment adds only bytes. Eviction never counts a held frame. A frame the link took counts in `replayed`. Each frame is in one counter, or it is sent.
- *A loss counted once.* A cut-off tail, a corrupt rest, or an unreadable rest counts when the reader first meets it. The segment then keeps a mark: `cut`, `lost_frames`, and `lost_bytes`. After a rewind the reader stops at `cut` and counts nothing more. A read error or a bad header on the read segment ends the segment at its read offset and counts the unread part at once (`Spool::abandon`). The mark survives a rewind.
- *Order (H3a).* The victim is the oldest segment with no held data. It is newer than the held data by at most P + 2S (below). The receiver still gets frames in capture order, with a gap after the held run. H3a keeps its meaning: the oldest data that is not already on its way to the receiver goes first.
- *Minimum cap.* The largest batch is 71,684 bytes. Held bytes are at most P = 127 × 71,684 = 8.7 MiB (8.0 MiB at 103-byte frames). Held segments also include the delivered prefix of D's segment and the unread rest of R's segment. Each is below one segment S = min(64 MiB, cap / 8). So held segments hold at most P + 2S. At `--spool-max-mib 64`, S is 8 MiB, and held segments hold at most 24.7 MiB. A block that needs room arrives when the segments hold more than 63.8 MiB. So at least 39 MiB (five or more segments) is not held, and a victim exists. At any larger cap, the held bytes stay below a quarter of the cap, because P < cap / 4. Tests with a cap below 64 MiB must keep the read-ahead below the cap.
- *Floor path.* It can find only held segments. It then refuses blocks for one pause (10 s) and counts `spool_dropped`. This needs a volume already at its floor and a spool smaller than P + 2S.

**Pool, turn length, and shutdown.**

- A replay batch is a pool block. `replay_step` takes it and offers it to the FIFO at once. In the FIFO it uses one of 256 slots. In the link's hand it replaces a slot, because the link took it from the FIFO. In Draining, the FIFO holds the front block and up to 127 batches: 128 of its 256 slots. A rewind returns blocks to the pool and allocates none.
- `serve` takes up to 256 parser blocks first, then replay. Replay stops after the first batch that reaches `REPLAY_TURN_BYTES`, so a turn reads under 1 MiB + 70 KiB. The turn returns `Moved`, and the next turn serves the parser again.
- The parser channel holds 16 MiB, 0.41 s at 41.2 MB/s. One Server quantum of router pause already uses 7.7 MB of it (118 of 256 blocks). Reading 1 MiB from an uncached disk at 100 MB/s takes 10.5 ms. That is 2.6 % of 0.41 s, and the parser adds about 0.43 MB (7 blocks) meanwhile. The 100 MB/s figure is an assumption that lab task L checks.
- One turn queues about 16 batches (103-byte frames). Eight turns fill the credit. A router wake keeps running turns while they move data, so one wake queues all 127 batches. At 41.2 MB/s the link then needs no router for 127 × 64 KiB / 41.2 MB/s = 0.20 s.
- At shutdown `serve` reads no replay. Batches out finish within the link budget or come back. `Router::seal` then runs. It drops the FIFO and fence senders and applies every message until the return channel closes, so each finished batch moves D. It takes the FIFO, releasing replay items without counting them. It places the live blocks that are older than a backlog, and the front list, as in the hand-back. It sends the parser channel's blocks to the spool. Then `Spool::seal` writes D to the cursor file and STOP to the active segment.
- The held data stays on disk, in order, and the next start reads from D. The router does not rewind at a stop, because nothing reads the spool again. A failed cursor write at stop counts in `spool_errors` and is logged. A stop-time disk fault is not a receiver problem, so the rule that a receiver problem adds no output at stop does not cover it. The shutdown bound (about 3.2 s) does not change.

**Test seam and fuzzing.** `Spool::read_and_deliver` (`src/spool/test_support.rs`, tests only) reads a batch and reports it delivered at once. Spool-only tests use it. `src/spool/fuzzing.rs` ends up to eight batches in a failed link, at points the input chooses. It delivers part of the frames, then rewinds.

**Throughput evidence.** The review harness is not in the repository. Settings: release profile, loopback, a receiver that counts bytes, 103-byte frames, busy threads at normal priority, 24 logical CPUs, Windows 11 client. Ten runs of each replay case:

| Case | B2 (MB/s) |
| --- | --- |
| Replay, idle | 725 to 787 (median 753) |
| Replay, 1 busy thread per CPU | 320 to 505 (median 361) |
| Replay, 2 busy threads per CPU | 51 to 368 (median 73.9) |
| Live paced at 41.2 MB/s, idle, 1 and 2 busy per CPU | 41.2, 41.0, 40.7; nothing spooled or dropped |

Lab task L repeats the replay case on Windows Server, whose scheduler quantum is longer. Until then the replay result holds on this host only.

## Constants

Values are from the code on this branch. Byte rates use the conversion in Units.

| Constant | Value | Notes |
| --- | --- | --- |
| `SEND_QUEUE_BLOCKS` | 256 | Parser-to-router channel, 16 MiB. It holds one Server quantum of router pause (118 blocks) twice over, and it absorbs late ETW bursts (finding L-7). |
| `LINK_FIFO_BLOCKS` | 256 | Router-to-link FIFO, 16 MiB. A waiting thread can lose a quantum: 2 clock ticks (31.25 ms) on a Windows client, 12 ticks (187.5 ms) on Windows Server. At 41.2 MB/s that is 20 blocks and 118 blocks. Late wakes of 195 to 216 ms, up to 136 blocks, were measured in the lab, and 128 blocks did not cover them. 256 blocks hold 0.41 s at 41.2 MB/s: two Server quanta (236 blocks) with an 8 % margin. |
| `FRONT_LIST_BLOCKS` | 1 | Live blocks that go out before the FIFO. |
| `POOL_BLOCKS` | 516 = 256 + 256 + 1 + 3 | One block each for the parser, the router, and the link thread, plus the front list. Blocks the router takes out of the FIFO, or the link returns, were already counted. 516 × 136 KiB = 68.5 MiB. |
| `BLOCK_CAPACITY` | 136 KiB | Blocks are sent at about 64 KiB (`BLOCK_SEND_BYTES`). |
| `REPLAY_BATCH_BYTES` | 64 KiB limit | `read_batch` takes a frame only if it fits, and always takes the first. So a batch is at most 71,684 bytes (70 KiB + 4). The batch fits `BLOCK_CAPACITY` with no reallocation. |
| `REPLAY_CREDIT` | 127 | Replay batches out at once. The code needs only that the credit and the front block fit in the FIFO, which a compile-time assert checks. 118 batches cover a Server quantum at 41.2 MB/s (41.2 MB/s × 0.1875 s / 64 KiB). 127 gives a 7.6 % margin, up to 44.4 MB/s. A larger credit would widen the held data and the crash window for no gain. |
| `REPLAY_TURN_BYTES` | 1 MiB | Most the router reads from the spool in one pass. |
| `RETURN_MESSAGES` | `REPLAY_CREDIT` + 2 = 129 | Return channel capacity. See "Why it holds" under the hand-back. |
| `TLS_FEED_BYTES` | 16 KiB | One TLS record at the default fragment size. |
| `LINK_POLL`, `SENDER_POLL` | 50 ms each | The link's wait for readiness and the FIFO. It bounds the shutdown reaction, the stall check, the fence wait, and the reconnect poll. The router's wait for input. |
| `IO_TIMEOUT` | 2 s | Connect, TLS handshake, READY/ACCEPT/START, and STOP/FINISH. Never a data write. The stall minimum. |
| `SHUTDOWN_LINK_BUDGET` | 1 s | Time the link may spend after shutdown starts. |
| `--receiver-stall-seconds` | 15 s default, 2 to 300 | At 50,000 queries per second (10.3 MB/s), 15 s spools about 155 MB, about 4 % of the default cap. |
| `--spool-max-mib` | 4,096 default, 64 to 1,048,576 | See [configuration](../guide/configuration.md). |
| Segment size `S` | min(64 MiB, cap / 8) | `segment_bytes_for`. |
| Spool pause | 10 s | After a failed cursor write, a floor refusal, or a transient read error. |
| `cursor.bin` | 28 bytes | Magic, sequence, offset, check value. |
| Link pause held in memory | 16 MiB / rate | About 8.1 s at 10,000 queries per second, 1.6 s at 50,000, and 0.41 s at 200,000. The kernel send buffer adds more, and it autotunes (H4), so these are lower bounds. |
| Router pause held in memory | 16 MiB / rate | The parser channel: 0.41 s at 41.2 MB/s. A replay read adds about 10 ms to a turn (B2). |

## Decisions

- **Stall timeout.** The 15 s default and the name `--receiver-stall-seconds` were accepted on 2026-11-03. The MSI has properties only for `RECEIVER` and `SESSION_NAME`. Other options, including this one and `--spool-max-mib`, go in `SERVICE_ARGS`, which the MSI passes to the service command line.
- **Unsent data at a failure.** B1, on 2026-11-03: unsent live data goes back to the router. B2 does the same for replay data.
- **Parser channel.** 256 blocks, on 2026-11-04, with the FIFO and risk 9.
- **Batched hand-offs.** Decision D, on 2026-11-03. Thread priority was rejected. Above-normal priority fixed every case in review V2, but the service must not compete with the DNS Server process.
- **Saturated link.** H3a, on 2026-11-03: capture order is kept, and the oldest spool data is deleted at the cap. H4: the service does not set `SO_SNDBUF`, so Windows send autotuning stays on.
- **Spool rewind (B2).** Decided on 2026-11-04, with four points. (1) Across a crash or kill, the read-ahead is sent again, so delivery is at least once. A stop repeats nothing. (2) `replayed` counts a frame when the connection accepts it. (3) The unconfirmed TLS piece of a live block goes back to the router, like the cut frame. (4) While batches are in flight, eviction deletes the oldest segment that has not been read. This is still H3a.

## Risks and open questions

Numbers are stable. Other documents refer to them (risk 19 in `docs/design/data-path.md`).

1. **Half-open receiver.** The 15 s stall timeout delays detection. With a spool, the disk absorbs the delay. Without a spool, up to 15 s of `queue_dropped`. Accepted.
2. **Disk is the new limit.** `queue_dropped` can still occur if spool appends are slower than the capture input. Open. Lab task L should measure the append rate.
3. **Counter meaning.** Resolved. `slow_writes` and `write_max_us` count the readiness wait per block: a block whose wait reaches 5 ms counts once as slow. `write_max_recent_us` (counter 64) is the 60-second peak.
4. **H4, kernel send buffer.** Built. The service does not set `SO_SNDBUF`, so Windows grows the buffer with the link (dynamic send buffering). Bytes the kernel holds at a failure grow with the buffer, and they stay uncounted. The lab measured about 0.58 MB on an idle host and about 4.2 MB on a busy host. Accepted. No test assumes a buffer size.
5. **Trickle receiver.** A receiver that accepts a few bytes before each stall timeout keeps the session forever. The liveness rule does not detect it. The saturated-link state (plan detection D4) reports it. Open.
6. **Receiver and cut frames.** B1 assumes the receiver discards a frame cut by a closed connection. The lab confirmed this for the DNS collector (plan, results for L).
7. **rustls API.** The data path relies on rustls 0.23 (0.23.46 in `Cargo.lock`): `writer()`, `write_tls`, `wants_write`, and the default fragment size. A version change reruns the TLS tests in `src/network/wire_tests.rs`.
8. **B2.** See the B2 section.
9. **Server quantum at full rate.** Lab task S, on 12 vCPUs: With the 256-block FIFO, 22 of 22 runs had no spill and no `queue_dropped`. That included 40.6 MB/s with 12 busy threads, and 40.5 MB/s with 24 busy threads and a router wait of 198 ms.
10. **Replay on a saturated host.** On the build host, replay ran at 51 to 368 MB/s at 2 busy threads per CPU, in 10 of 10 runs, above the 41.2 MB/s capture rate. Open for Windows Server (lab task L).
11. **Take lock.** If the router is preempted while it holds the take lock, the link waits up to one quantum. The lock is held only for queue pops during a spill. Accepted.
12. **Duplicates after a crash.** Delivery across a crash or kill is at least once (decided 2026-11-04). The receiver can see up to 127 replay batches twice (8.0 MiB, 8.7 MiB at worst), and the frames of the last router pass. A receiver that cannot tolerate a repeat must deduplicate, as for any TCP restart. Accepted.
13. **Disk in use.** Held data stays on disk until delivered. Eviction spares any segment with held data, so the bytes the cap cannot delete reach P + 2S. That is 24.7 MiB at the minimum cap of 64 MiB (39 % of it), and about 137 MiB with 64 MiB segments (caps of 512 MiB and above). `spool_bytes` includes them. At the minimum cap an eviction victim always exists.
14. **Live data behind read-ahead.** In Draining, a live block goes through the spool and then waits behind up to 8 MiB of replay in the FIFO. That is 0.2 s at 41.2 MB/s and 1.7 s at 5 MB/s. Accepted, because the backlog delay of Draining is already the larger cost.
15. **Router turn length.** A replay read adds under 1 MiB + 70 KiB to a turn, about 10 ms on a disk that reads 100 MB/s. The 100 MB/s figure is an assumption. If the lab shows `send_queue_depth_max` rising during a drain, lower `REPLAY_TURN_BYTES`. Open.
16. **Counter meaning.** Resolved in the guides (plan task W). `replayed` counts at delivery. `disconnected_dropped` holds replay frames or the unconfirmed TLS piece only on the two defensive paths and with no spool. `bytes_sent` counts the accepted prefix of a cut frame again when the frame is sent again, as it does for live data.
17. **Duplicates at a failure.** None are expected, except in risk 19. D is the end of the last frame the kernel fully accepted, and an unconfirmed TLS piece cannot have arrived whole. An error in this argument would show as a frame twice at the receiver. The B2 tests check it.
18. **Eviction victim.** B2 deletes the oldest segment with no held data. It is newer than the held data by at most P + 2S. Decided as H3a on 2026-11-04.
19. **TLS 1.3 key update window.** A duplicate can occur if rustls queues a `KeyUpdate` after a data record and the connection resets while it is pending. The window is about 22 bytes of writing, once per 2^23 or more records. The service never asks for a key update. No code change.

Documents that depend on this design: `docs/guide/monitoring.md` (counter meaning), `docs/guide/configuration.md` (order and delivery), `docs/guide/tuning.md` (spool sizing, including held data), `docs/design/measurement-terms.md`, `docs/design/data-path.md`, the comments in `src/pipeline/supervisor.rs` and `src/sender.rs`, and `CHANGELOG.md`.

## Tests

The tests live in `src/sender/` (link, router, fault, end-to-end, and saturation tests), `src/spool/` (spool, property, and fuzz tests), and `src/network/wire_tests.rs`. Test seams:

- `LinkThread::step` and `Router::turn` each run one pass without waiting. `serve` and `run` loop over them. The batching test runs both on one thread in a fixed order, which is a manual scheduler.
- `LinkHooks::in_take` runs on the link thread while it holds the take lock.
- `script.rs` provides a scripted transport. `TlsKit::with_record_limit` lowers the record limit through the cipher suite's own setting, so product code has no test seam for it.

Named tests for the main behaviors (file in parentheses):

| Behavior | Test |
| --- | --- |
| A failure returns only the block in hand; the router takes the FIFO | `a_failure_returns_only_the_block_in_hand_and_leaves_the_fifo_to_the_router` (`link_tests.rs`) |
| A spill while the link holds the take lock keeps capture order | `a_spill_while_the_link_holds_the_take_lock_keeps_capture_order_and_one_front_block` (`fault_tests.rs`) |
| A pause of one Server quantum never touches the spool | `a_pause_of_one_server_quantum_never_touches_the_spool` (`fault_tests.rs`); the two-quantum case is next to it |
| A stall past the timeout ends the session and loses no frame | `a_receiver_that_accepts_nothing_for_the_stall_timeout_loses_the_session_and_no_frame` (`saturation_tests.rs`) |
| A cut frame is sent again in full and arrives once | `a_frame_cut_by_a_failure_is_sent_again_in_full_on_the_new_session_and_arrives_once` (`fault_tests.rs`) |
| An unconfirmed TLS piece goes back and arrives once | `a_tls_piece_whose_delivery_is_unconfirmed_goes_back_and_arrives_once` (`fault_tests.rs`); `a_tls_piece_whose_delivery_is_unconfirmed_goes_back_with_the_frames_after_it` (`link_tests.rs`) |
| A cut replay batch reports what was taken and counts nothing | `a_cut_replay_batch_reports_the_frames_taken_and_counts_nothing` (`link_tests.rs`) |
| A hand-back with a cut replay batch reads again from the cut frame | `a_hand_back_with_a_cut_replay_batch_reads_again_from_the_cut_frame` (`router_tests.rs`) |
| One router wake queues the whole credit | `one_router_wake_queues_the_whole_credit_and_the_link_drains_it_without_the_router` (`router_tests.rs`) |
| A batch that cannot enter the FIFO is counted once and not read again | `a_batch_that_cannot_enter_the_fifo_is_counted_once_and_not_read_again` (`router_tests.rs`) |
| A rewind reads the held batches again, in order, across a segment end | `a_rewind_reads_the_held_batches_again_in_order_across_a_segment_end` (`spool_tests.rs`) |
| A rewind after a partial report starts at the first frame not taken | `a_rewind_after_a_partial_report_starts_at_the_first_frame_not_taken` (`spool_tests.rs`) |
| A rewind writes no cursor, and a stop after it repeats nothing | `a_rewind_needs_no_cursor_write_and_a_stop_after_it_repeats_nothing` (`spool_tests.rs`) |
| A failed cursor write keeps the older position and does not pause reads | `a_cursor_write_failure_leaves_an_older_position_and_does_not_pause_reads` (`spool_tests.rs`) |
| Eviction skips held segments and counts only what it deletes | `eviction_skips_held_segments_and_counts_only_what_it_deletes` (`spool_tests.rs`) |
| A TLS session at its record limit fails instead of dropping data | `a_tls_session_at_its_record_limit_fails_instead_of_dropping_data` (`wire_tests.rs`) |
| A TLS piece of the feed size is one record | `a_piece_of_the_feed_size_is_one_tls_record` (`wire_tests.rs`) |
| A run stopped in the middle of a replay resumes from the cursor | `a_run_stopped_in_the_middle_of_a_replay_leaves_a_cursor_the_next_run_resumes_from` (`e2e_tests.rs`) |
| A link that fails mid-replay sends nothing twice | `a_link_that_fails_mid_replay_loses_only_what_the_kernel_held_and_sends_nothing_twice` (`e2e_tests.rs`) |
| A link thread that vanishes gives its batches back to the spool | `a_link_thread_that_vanishes_with_batches_out_gives_them_back_to_the_spool` (`router_tests.rs`) |
| The fence wait ends at the shutdown deadline | `a_link_that_waits_for_the_fence_ends_at_the_shutdown_deadline` (`link_tests.rs`) |
| A receiver that never answers FINISH does not fail the shutdown | `a_receiver_that_never_answers_finish_does_not_fail_the_shutdown` (`sender_tests.rs`) |

**Other tests.** The spool property tests (`src/spool/property_tests.rs`) and the fuzz target (`src/spool/fuzzing.rs`) rewind at random points. They check conservation (appended = `replayed` + `spool_dropped` + on disk), order, and no repeats. Several tests assert `pool_fallbacks == 0`.

**Characterization tests.** A stall shorter than the timeout causes no `queue_dropped`, no reconnect, and no `disconnected_dropped`. A stall at a 2-second timeout ends the session. Its unsent live frames go back to the spool, and B2 counts nothing.

**Throughput acceptance.** From the review harness: a 41 MB/s paced capture is delivered at 40 MB/s or more, at one and at two busy threads per CPU, with under 1 % of frames spooled. Replay drains faster than 41 MB/s at two busy threads per CPU in every run. The build-host result is in the B2 section. Lab task L repeats it on Windows Server.
