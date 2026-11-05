# Slow receiver: link writer and progress liveness

> **Status:** Current. · **Describes:** the sender on this branch. · **Evidence:** source review on 2026-11-05. Lab results are in the [validation record](../history/slow-receiver-validation.md). · **Reviewed:** 2026-11-05

### Quick Start

```text
dnstap-parser --256 blocks--> dnstap-sender (router) --link FIFO, 256 blocks--> dnstap-link --> socket
                                |        ^                                          |
                     spool <----+        +---- return channel: Returned, ReplayDone, HandBackDone
```

- The router owns the spool (on-disk queue) and never waits on the network. Only the link writes the socket. A short stall costs memory, not frames.
- A session (TCP, TLS, Frame Streams handshake) ends only when data waits and the receiver accepts no bytes for the stall timeout (default 15 s).
- On a failure, live data returns to the router in order; replay data (read back from the spool) stays on disk until accepted.

### Steps / Configuration

#### Requirements

- **I1. Order.** With a spool backlog, new live blocks go to the spool. Frames arrive in capture order.
- **I2.** Each lost frame counts once: `queue_dropped`, `disconnected_dropped`, or `spool_dropped`.
- **I3. Delivery.** At most once; at least once across a crash or kill. A normal stop repeats nothing. Delivered means the socket accepted the bytes.
- **I4.** The parser never blocks: fixed block pool, no per-frame allocation.
- **I5.** Shutdown takes about 5 s at most, inside the 10 s watchdog. Target: about 3.2 s.
- **I6.** `dnstap-reconnect` connects and handshakes in the background. Backoff: 250 ms to 30 s.
- **R1.** No send timeout on data. A timed-out blocking send leaves the connection indeterminate and it must be closed (Microsoft Learn).

#### Threads and ownership

| Thread | Owns | Waits on |
| --- | --- | --- |
| `dnstap-parser` | Block it fills | Nothing |
| `dnstap-sender` (router) | Parser channel, front list, `Spool`, FIFO sender and receiver clone, return channel | One `crossbeam_channel::Select`, 50 ms (`SENDER_POLL`) |
| `dnstap-link` | `NetworkSink`, FIFO receiver, return sender, block in hand | FIFO or socket writability, 50 ms (`LINK_POLL`) |
| `dnstap-reconnect` | One connect and handshake attempt | Blocking calls under `IO_TIMEOUT` |

Code: `src/sender.rs` (types, pool), `src/sender/router.rs`, `src/sender/link.rs`, `src/network/wire.rs` (`Uplink`: reconnecting sessions, implemented by `NetworkSink`; `Wire`: one session's non-blocking data path).

- `LinkShared` holds the `up` flag (link writes, router reads), the take lock (`Mutex<()>`), and shutdown state. Both threads return blocks to one lock-free pool.
- `up` starts clear. The first session enters link mode, sets `up`, and signals `QueuedSink::start`, which then starts the router. If it can't, the router spools.
- Only disk speed limits the router. Partial-write state stays in one link-thread loop.

#### The link FIFO and batched hand-offs

256 `Live` or `Replay` blocks (`LINK_FIFO_BLOCKS`). The router keeps a receiver clone, so sends succeed after the link is gone; the return channel tells it. Depth and batching absorb a lost scheduler quantum, so both threads run at normal priority and leave DNS Server its share.

- **Batching.** The link takes the next item as soon as it finishes one, waiting only on an empty FIFO. Each router wake routes every block it can and reads replay up to its credit.
- **Take lock.** The link takes items only under it (`try_recv`, one in hand) and waits with `Select::ready_timeout`, which takes nothing.
- **Spill.** Only on `try_send` returning `Full`; routing depends only on FIFO state. The router takes the lock, retries once, pops every item into a buffer preallocated for `LINK_FIFO_BLOCKS`, drops the lock (a few microseconds), spools them oldest first plus the block that didn't fit, and enters Draining. The link has finished every older item or holds one, so order holds.
- **Front list.** Blocks sent before the FIFO. After a spill only the link's held block predates the backlog, and Draining sends only replay and the front list, so a failure returns one live block at most (`FRONT_LIST_BLOCKS`).

#### Writing to the socket

The socket goes non-blocking after the handshakes. On `WouldBlock` (nothing queued) the link waits up to 50 ms for writability, checks the stall timer and shutdown deadline, and retries. STOP/FINISH run blocking with a fresh `IO_TIMEOUT`. Blocks skip the `BufWriter` flush timers; the parser's 100 ms and 1 s partial-block timers bound the delay.

**TLS.** Call rustls directly. `write_all`, `BufWriter`, and `StreamOwned::write` lose the count or delay errors on a non-blocking socket.

- `conn.writer().write(piece)` returns the plaintext accepted. `conn.write_tls(&mut tcp)` keeps unwritten bytes in rustls, so `WouldBlock` never cuts a record.
- A piece is at most `TLS_FEED_BYTES` and ends at a frame boundary. `write_tls` drains until `wants_write()` is false before the next.
- Only the unconfirmed piece has an unknown fate at a failure. Plaintext knows the exact offset.

**TLS record limit.** At 2^24 records (TLS 1.2, AES-GCM), rustls 0.23.46 queues `close_notify`, refuses the record, and still reports it written. `tls_offer` (`src/network/wire.rs`) compares queued ciphertext (`process_new_packets().tls_bytes_to_write()`, no I/O) with plaintext taken. A record adds a 5-byte header and 16-byte tag, so fewer bytes mean a drop: the offer fails and the data goes back at the hand-back. A refused piece of 10 bytes or fewer passes once; the next offer fails. No frame is that small.

#### Liveness from progress

A session ends only when the link has bytes to write and the receiver accepted none for the stall timeout. Any accepted byte is progress (TLS: bytes `write_tls` put on the socket). An idle link never stalls.

`--receiver-stall-seconds`: default 15, range 2 to 300. The minimum equals `IO_TIMEOUT`, so an in-deadline handshake is never a stall. A shorter stall fills the FIFO and spills: no loss, no reconnect. No spool: a full FIFO means `queue_dropped` until the receiver resumes.

**Shutdown timing.** The link stops at the earlier of the stall deadline and the link deadline.

| Step | Time |
| --- | --- |
| Final partial block, link budget | 1.1 s |
| Write in progress, hand-back | up to 50 ms |
| STOP/FINISH | up to 2 s |
| Returned block, FIFO, channel to spool | up to 513 blocks (about 32 MiB), during STOP/FINISH |
| Total | about 3.2 s; more only if the seal outlasts STOP/FINISH |

#### Hand-back at a failure

On a write error, stall, or shutdown deadline:

1. The link ends the session, clears `up`, and stops taking FIFO items.
2. It splits the block in hand by byte counts (`Split`: `sent`, `sent_end`, `unsent`). Accepted frames are sent. Live: the cut frame onward, plus an unconfirmed TLS piece's frames, returns as `Returned`; the receiver discards its truncated copy. Replay: the accepted prefix goes as `ReplayDone`.
3. It sends `HandBackDone` (at shutdown, drops the return channel). It may reconnect but neither sets `up` nor writes before the fence.
4. The router stops sending on `up` false or `HandBackDone`. It applies messages in order (each `ReplayDone` moves D), then drains the FIFO: live items follow the returned block, replay items return to the pool uncounted, with their credit. No parser block until step 6.
5. `Spool::rewind` resets the read position ([Spool rewind](#spool-rewind)).
6. It places returned, drained, then newer blocks: spool head with no backlog, front list with one. No spool: they count once in `disconnected_dropped`.
7. It sends the fence with the FIFO empty and enters Down. Only then may the link set `up`.

Invariants:

- *No deadlock.* `REPLAY_CREDIT` + 2 unread messages at most fit `RETURN_MESSAGES`. The fence channel (capacity 1) is empty when the router sends; the link waits on it with a timeout and checks the shutdown deadline.
- *No double count.* Each FIFO item is received once; a replay credit returns once.
- *No reorder.* The router stops producing before it drains.
- *Shutdown mid-failure.* Steps 1 and 2, then the return channel drops; the router treats the close as `HandBackDone`.
- *Reconnect mid-hand-back.* The new session idles until the fence.

#### Router states

The link is `Up`, `Down`, or `Gone` (thread ended). `Router::destination` picks `Link` (the FIFO), `Spool`, or `Discard` (no spool, link down: `disconnected_dropped`). It picks the spool on a backlog, replay out, a front-list block, or link down.

| State | When | Live block | Replay | Leaves when |
| --- | --- | --- | --- | --- |
| Direct | Link up, no backlog | Front list, then `try_send`; `Full`: spill | None out | Spill: Draining. Link down or `HandBackDone`: HandBack |
| Draining | Link up, backlog or replay out | Spool | Front list, then one batch per free credit | Nothing to read, credits back: Direct. Link down: HandBack |
| HandBack | Link ended its session | Waits in parser channel | None | `HandBackDone` or close: steps 4 to 7, Down |
| Down | Link not up, hand-back done | Spool, or `disconnected_dropped` | None | Link up: Direct or Draining |
| Closing | Shutdown, within link budget | As above | None | Budget ends, or input closed and empty: Sealing |
| Sealing | Past budget, or input closed | `Router::seal` | None | Seal, join link thread: Done |

- **Turn:** take link messages, follow link state, save D once, `serve`, sample the monitor. Wait only when nothing moved.
- **`serve`:** up to 256 parser blocks, then replay until a batch reaches `REPLAY_TURN_BYTES` (under 1 MiB + 70 KiB); none at shutdown. One wake queues all 127 batches; at 41.2 MB/s the link then runs 0.20 s alone.
- **No spool, full FIFO:** the router stops, the parser counts `queue_dropped`.
- **`Router::seal`:** drop the FIFO and fence senders, apply every message until the return channel closes (finished batches move D), release FIFO replay uncounted, place older live blocks and the front list, spool the parser channel's blocks. Then `Spool::seal` writes D and STOP.

#### Link states

| State | Event | Action | Next |
| --- | --- | --- | --- |
| Disconnected | Every 50 ms | Connect on the reconnect helper | Up, Idle |
| Idle | Item ready | Take one under the take lock; set `last_progress` | Writing |
| Writing | Bytes accepted | Set `last_progress`. Block end: release it or report `ReplayDone`, take next | Writing, or Idle |
| Writing | Stall timeout or write error | End session, start backoff, as `connection_lost` does | HandingBack |
| Any | Shutdown deadline | Stop writing | HandingBack, Finishing |
| HandingBack | | Hand-back steps 1 to 3 | Fenced wait, or Finishing |
| Fenced wait | Fence received | A reconnect may finish; nothing written | Idle, or Disconnected |
| Finishing | Block boundary | Blocking `DeadlineTcp`; STOP, FINISH under `IO_TIMEOUT` | Done |
| Finishing | Inside a frame | Close, no STOP; frame writer refuses more; nothing logged | Done |

If STOP or FINISH fails, `NetworkSink` still returns `Ok`, with no log event or counter: every data frame was accepted, handed back, or counted. Worst case about 3 s (link finishes just inside the budget, closing exchange hits 2 s). A stalled shutdown takes about 1.1 s.

#### Spool rewind

| | Read position R | Delivered position D |
| --- | --- | --- |
| Meaning | End of last batch read | End of last frame fully handed to the kernel (TLS: of a piece `write_tls` fully wrote) |
| Where | First segment not ended (`read_offset`, `frames_read`) | Head segment (`done_offset`, `frames_done`) |
| Forward | `read_batch` | `Spool::delivered`, on `ReplayDone` |
| Back | `Spool::rewind`, to D | Never |

*Held* frames lie between D and R. D never passes R; held data stays on disk.

**Spans and reports.**

- `read_batch` records one span per batch (segment, start and end offsets, frame count); a batch comes from one segment. Spans queue in read order, `REPLAY_CREDIT` at most. The link finishes batches in FIFO order, so each report matches the front span.
- `Return::ReplayDone { bytes, frames }`: the prefix fully handed to the kernel, ending at a frame boundary (`Split::sent`, `Split::sent_end` at a failure).
- `Spool::delivered(bytes, frames)` pops the front span, sets D to span start plus `bytes`, adds `frames` to `replayed`, and sweeps. A frame read twice counts once.
- `Spool::written_off` skips a batch on two defensive paths that can't run today: an item arriving while the link is down, a failed offer in `replay_step`. Counted once in `disconnected_dropped`, reported with `Return::ReplayCounted`.

**Segment deletion.**

- `ended` is set at a STOP, the end of an unsealed file, a cut-off or corrupt frame, or the good length of a segment closed after a write error.
- A sweep (end of `delivered`, or of a read that ended a segment) deletes the head segment while it's ended and `frames_done == frames_read`. The active segment never ends; it goes only as the sole segment, with R at the writer position and all frames delivered.
- A deletion writes the cursor only with no span out; otherwise the cursor already holds D.
- `has_backlog()`: any segment remains. `has_unread()` (tested by `replay()`): R before the end of data. `unread_bytes()` counts from R.

**Rewind.** `Spool::rewind` drops the spans, resets each segment's read offset and frame count to its delivered values, clears `ended` (keeping a counted-loss mark), and drops the reader so the next read reopens at D. No cursor write.

- *D has every finished batch:* `ReplayDone` precedes `HandBackDone` on one channel, which delivers every message before a close.
- *Nothing sent twice:* frames before D were accepted, frames from D on weren't.
- *Order from D:* rest of the batch in hand, FIFO batches, unread data, then live blocks Draining appended.
- *No spill meets a replay item:* spills run only in Direct.
- *Pool:* a batch is one pool block in one FIFO slot; Draining uses 128 of 256 slots (front block plus 127 batches). A rewind allocates nothing.
- *Unconfirmed TLS piece:* its frames go back too. It's one 16 KiB record at most, or one long first frame across records. While `pending()` is true, the receiver holds only a prefix of the last record and can't decrypt it. Exception: risk 19.

**Cursor file (`cursor.bin`).**

- 28 bytes, little-endian: magic `ETDC`, segment sequence (u64), byte offset (u64), check value (sequence XOR offset XOR a constant). One positional write, never flushed or deleted. Plaintext, no frame data; segment records are AES-128-GCM sealed (`src/spool/crypt.rs`).
- Holds D, not R: a crash resends replay data instead of losing it.
- `Spool::checkpoint` writes D once per router pass at most, after every report, only if D moved: about 630 writes/s at 41.2 MB/s, 8,400 at the 550 MB/s idle replay rate. Also called in `hand_back` and `link_vanished`; the spool writes it on deleting the first segment and in `Spool::seal`, so a stop repeats nothing.
- `Spool::open` applies D. Missing segment, or older than all: start at the first segment. Inside a segment: resume there, delete lower segments uncounted. Damaged (wrong size, magic, or check value, or offset inside a segment header): drop and count the oldest segment.

**Eviction and `--spool-max-mib`.**

- `Spool::evict_oldest(keep_last)` deletes the oldest segment that isn't reader-ended and holds no held data; its frames count in full in `spool_dropped`. The cap path may delete the newest segment; the free-space floor path keeps it. Held segments count in `total_bytes`, `spool_bytes`, and the cap.
- A cut-off tail, corrupt rest, or unreadable rest counts once, when first read. The mark (`cut`, `lost_frames`, `lost_bytes`) survives a rewind. A read error or bad header ends the segment at its read offset and counts the unread part (`Spool::abandon`).
- *Minimum cap.* Held bytes: at most P = 127 × 71,684 = 8.7 MiB. Held segments add D's delivered prefix and R's unread rest, each under one segment S = min(64 MiB, cap / 8): P + 2S at most. At `--spool-max-mib 64` that's 24.7 MiB, leaving 39 MiB or more (five segments or more) evictable. At larger caps, P < cap / 4.

#### Constants

| Constant | Value | Notes |
| --- | --- | --- |
| `SEND_QUEUE_BLOCKS` | 256 | Parser-to-router, 16 MiB: twice a Server quantum of router pause (118 blocks) |
| `LINK_FIFO_BLOCKS` | 256 | Router-to-link FIFO, 16 MiB |
| `FRONT_LIST_BLOCKS` | 1 | Live blocks sent before the FIFO |
| `POOL_BLOCKS` | 516 | 256 + 256 + 1, plus one each for parser, router, link; 68.5 MiB |
| `BLOCK_CAPACITY` | 136 KiB | Sent at about 64 KiB (`BLOCK_SEND_BYTES`) |
| `REPLAY_BATCH_BYTES` | 64 KiB | Always takes its first frame: 71,684 bytes max |
| `REPLAY_CREDIT` | 127 | Replay batches out at once |
| `REPLAY_TURN_BYTES` | 1 MiB | Most spool read per turn |
| `RETURN_MESSAGES` | 129 | `REPLAY_CREDIT` + 2 |
| `TLS_FEED_BYTES` | 16 KiB | One TLS record at the default fragment size |
| `LINK_POLL`, `SENDER_POLL` | 50 ms | Link readiness wait; router input wait |
| `IO_TIMEOUT` | 2 s | Connect, handshakes, STOP/FINISH; stall minimum |
| `SHUTDOWN_LINK_BUDGET` | 1 s | Link time after shutdown starts |
| Segment size `S` | min(64 MiB, cap / 8) | `segment_bytes_for` |
| Spool pause | 10 s | After a failed cursor write, floor refusal, or transient read error |

Sizing:

- A query makes two 103-byte frames on average (lab): 10,000 queries/s is 2.06 MB/s, 50,000 is 10.3 MB/s, 200,000 is 41.2 MB/s.
- A lost quantum is 31.25 ms on a Windows client, 187.5 ms on Windows Server: 20 and 118 blocks at 41.2 MB/s. The FIFO holds 0.41 s there, two Server quanta plus 8 %. 118 batches cover a Server quantum; the credit of 127 adds 7.6 %.
- A compile-time assert checks that the credit and the front block fit in the FIFO. A larger credit only widens the held data and the crash window.
- Each 16 MiB queue holds about 8.1 s at 10,000 queries/s, 1.6 s at 50,000, 0.41 s at 200,000, plus the autotuned kernel send buffer.

#### Tests

In `src/sender/`, `src/spool/`, and `src/network/wire_tests.rs`.

- `LinkThread::step` and `Router::turn` run one pass without waiting; `serve` and `run` loop over them. The batching test runs both on one thread in a fixed order.
- `LinkHooks::in_take` runs under the take lock. `script.rs`: scripted transport, manual clock. `TlsKit::with_record_limit` lowers the record limit through the cipher suite's setting. `Spool::read_and_deliver` (`src/spool/test_support.rs`) reads a batch and delivers it at once.
- `src/spool/property_tests.rs` and the fuzz target `src/spool/fuzzing.rs` fail the link mid-batch at random and rewind. They check conservation (appended = `replayed` + `spool_dropped` + on disk), order, and no repeats. Several tests assert `pool_fallbacks == 0`.

### Common Gotchas

#### Loss at a failure

| Case | Counted loss |
| --- | --- |
| Failure in Direct | None |
| Failure in Draining | None; replay is read again |
| TLS unconfirmed piece | None; replay read again, live returns with the cut frame |
| Shutdown with batches out | None; batches stay on disk |
| Shutdown with a backlog and a front block | One block at most, `disconnected_dropped` |
| Crash or kill | None; up to 127 batches and the last router pass are resent |

- Only kernel-held bytes are lost, uncounted (risk 4). A spool frame ends `replayed`, `spool_dropped` (evicted or unreadable), or on disk.
- **Crash:** resends held data (8.0 MiB at 103-byte frames, 8.7 MiB worst) and frames delivered since the last cursor write; loses live data in memory (up to 16 MiB) and kernel-held bytes.
- **Failed cursor write:** the file keeps the older D, so a crash repeats more, never fewer. Counted in `spool_errors`, logged; reads continue. Retried after the 10 s pause, at the next deletion, and in `seal`.
- **Undeletable segment:** the cursor is written first, so the next start deletes the stray file unsent. If that write fails too, a crash can resend it.
- **Floor path, only held segments:** refuses blocks for one 10 s pause, counts `spool_dropped`.
- **Tests with a cap below 64 MiB** must keep the read-ahead below the cap.

#### Risks and open questions

Keep these numbers stable: `docs/design/data-path.md` cites risk 19.

1. **Half-open receiver.** Detection waits up to 15 s; with no spool, that's up to 15 s of `queue_dropped`. Accepted.
2. **Disk is the limit.** Appends slower than capture mean `queue_dropped`. Open: the append rate needs a lab measurement.
3. **Write wait counters.** `slow_writes` and `write_max_us` time the readiness wait per block; 5 ms or more counts once as slow. `write_max_recent_us` (counter 64) is the 60-second peak.
4. **Kernel send buffer.** `SO_SNDBUF` is unset; Windows autotunes it. Kernel-held bytes at a failure are lost uncounted: about 0.58 MB idle, 4.2 MB busy (lab). Accepted.
5. **Trickle receiver.** A few bytes before each stall timeout keeps the session forever. The saturated-link state reports it. Open.
6. **Cut frames.** The hand-back relies on the receiver discarding a frame cut by a closed connection. Lab-confirmed for the DNS collector.
7. **rustls API.** Relies on rustls 0.23 (0.23.46 in `Cargo.lock`): `writer()`, `write_tls`, `wants_write`, the default fragment size. On a version change, rerun the TLS tests in `src/network/wire_tests.rs`.
8. **Spool rewind.** See [Spool rewind](#spool-rewind).
9. **Server quantum at full rate.** The FIFO must cover a lost Server quantum. Lab, 12 vCPUs: 22 of 22 runs, no spill, no `queue_dropped`, up to 40.5 MB/s with 24 busy threads.
10. **Replay on a saturated host.** Must drain faster than 41.2 MB/s. Build host: 51 to 368 MB/s with 2 busy threads per CPU. Windows Server guest: 1.46 GB backlog in 2.1 s (about 700 MB/s).
11. **Take lock.** A router preempted while holding it makes the link wait up to one quantum. It covers only a spill's queue pops. Accepted.
12. **Duplicates after a crash.** Held data and the last router pass can arrive twice. Exactly-once receivers must deduplicate. Accepted.
13. **Disk in use.** Held data (P + 2S at most) is outside the cap's reach: 24.7 MiB at the 64 MiB minimum cap, about 137 MiB with 64 MiB segments (caps of 512 MiB and above).
14. **Live data behind read-ahead.** In Draining a live block waits behind up to 8 MiB of FIFO replay: 0.2 s at 41.2 MB/s, 1.7 s at 5 MB/s. Accepted.
15. **Router turn length.** A replay read adds about 10 ms a turn at an assumed 100 MB/s disk read. If `send_queue_depth_max` rises during a drain, lower `REPLAY_TURN_BYTES`. Open.
16. **Counter meaning.** `replayed` counts at delivery. `disconnected_dropped` holds replay frames only on the two defensive paths and with no spool. `bytes_sent` counts a cut frame's accepted prefix again on resend.
17. **Duplicates at a failure.** Only risk 19 can cause one; the rewind tests check this.
18. **Eviction victim.** The oldest segment with no held data, at most P + 2S newer than it. Capture order holds, with a gap after the held run.
19. **TLS 1.3 key update window.** rustls can queue a `KeyUpdate` (about 22 bytes) after a data record, at the AES-GCM record limit or on `refresh_traffic_keys()`. If the socket took the whole record and the connection resets while the `KeyUpdate` is pending, the record's frames arrive twice. The window is once per 2^23 or more records. The service never calls `refresh_traffic_keys()`.
