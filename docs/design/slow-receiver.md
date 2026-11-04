# Slow receiver: link writer and progress liveness

> **Status:** Proposed · **Describes:** the `dnstap-sender` routing and the link in `main` at commit `a06f6fd`, and a change to them (tasks H1 and H2 of the [slow and saturated receiver plan](../plans/slow-receiver-plan.md)) · **Evidence dated:** not applicable (source analysis; no lab run) · **Reviewed:** 2026-11-03

This design keeps the sender thread free of socket waits, and ends a session only when
the receiver makes no progress. On a failure, unsent live data goes back to the router
(decision B1). It compares two designs, recommends one, and gives its state machines,
constants, and tests. For a saturated link it keeps today's strict order (H3a, decided
at gate G2).

## Problem

### F1. A blocked write stops the spill

The `dnstap-sender` thread routes blocks and writes the socket (`SenderThread::run`,
`src/sender.rs`). `NetworkSink::write_framed` (`src/network.rs`) splits a block into
chunks of up to 64 KiB. Each chunk has its own 2 s deadline (`IO_TIMEOUT`,
`DeadlineTcp`). A block is 2 or 3 chunks, and a 256 KiB replay batch is 4 or 5. While a
chunk blocks, the thread routes nothing, so the spill rule (`queued >= SPILL_BLOCKS`)
never runs.

The queue holds 128 blocks of about 64 KiB (8 MiB). The spill point leaves 32 blocks
(2 MiB). At the 103-byte lab average:

| Queries per second | R_in | Headroom after the spill point |
| --- | --- | --- |
| 10,000 | 2.06 MB/s | about 1.0 s |
| 50,000 | 10.3 MB/s | about 0.20 s |
| 200,000 | 41.2 MB/s | about 0.05 s |

One chunk that blocks for its full 2 s fills the headroom above about 10,000 queries
per second (2 MiB in 2 s). A block or a replay batch can block for longer. The parser
then counts `queue_dropped` while the spool has room.

### F2. A slow receiver looks dead

A chunk that does not finish in 2 s ends the session. The block in progress and the
`BufWriter` contents (up to 64 KiB) count in `disconnected_dropped`. A reconnect then
waits for the backoff (250 ms to 30 s) and the TLS and Frame Streams handshakes. A
2.5 s collector pause costs frames and a reconnect.

### Invariants to keep

| # | Invariant | Where it holds today |
| --- | --- | --- |
| I1 | Capture order is kept while the spool holds a backlog. Every live block goes to the spool until the backlog is empty. | `SenderThread::destination`, `Spool::has_backlog` |
| I2 | Each lost frame counts in exactly one of `queue_dropped`, `disconnected_dropped`, and `spool_dropped`. | `QueuedSink::send_current`, `NetworkSink::connection_lost`, `Spool::append` |
| I3 | Delivery is at most once. `Spool::read_batch` saves the cursor before it returns a batch. A batch lost on a failed link counts in `disconnected_dropped`. Under B1 a frame cut mid-write is sent again; the receiver saw only a truncated copy, so it never gets the frame twice. | `Spool::read_batch`, `SenderThread::replay_step` |
| I4 | The parser never blocks and never allocates per frame. | `QueuedSink::write_frame`, the block pool |
| I5 | Shutdown takes about 5 s at most, inside the 10 s watchdog. | `SHUTDOWN_LINK_BUDGET`, `IO_TIMEOUT` |
| I6 | Connect and handshakes run on the `dnstap-reconnect` helper, with the backoff unchanged. | `NetworkSink::poll` |

R1 adds a constraint. Microsoft Learn says that when a blocking send times out, the
connection is in an indeterminate state and must be closed. It does not say whether the
call queued some bytes. A session must never continue after a send that returned a
timeout error.

## Options

- **H1a.** A new `dnstap-link` thread owns the `NetworkSink` and writes the socket.
  The sender thread (now the router) keeps the spool and the routing. It hands link
  blocks to a small bounded FIFO.
- **H1b.** One thread. The socket is non-blocking. The sender thread writes for at most
  a short slice, keeps the unwritten tail, and routes blocks between slices.

### Comparison

| Topic | H1a: link thread and FIFO | H1b: time-sliced writes |
| --- | --- | --- |
| Threads and ownership | Router owns the spool, the parser channel, the hold, and the FIFO sender. Link thread owns the session, the reconnect state, and the FIFO receiver. A block has one owner at a time; unsent live blocks come back by a return channel. | One thread owns everything, as today. |
| Waiting | Router waits with one `crossbeam_channel::Select` (parser channel, FIFO room, return channel). Link thread waits on the FIFO or on socket readiness. | The thread must wait for "a block arrived" or "the socket is writable". No call waits on a crossbeam channel and a socket together, so it must alternate short waits. |
| Ordering | One producer (the router) and one FIFO. A block in the FIFO is older than every block routed after it. Replay batches use the same FIFO. | The tail is older than everything else. Routing must not write a new block before the tail is done. |
| Backlog to live | When `read_batch` returns the last data, it deletes the last segment and `has_backlog` turns false. The router puts that batch in the FIFO. The next live block goes behind it. | Same rule, but the last batch can stay a tail for many loop turns. |
| Loss on a link failure | Unsent live frames go back to the router. Only the unsent rest of one replay batch (less than 134 KiB, in Draining) and, with TLS, one unconfirmed piece are lost. | The tail can go back by the same rules; the hand-back is simpler, but the tail stays in the routing loop. |
| How the router learns of a failure | The link thread clears `LinkShared::up`. The router sends a fence and takes the hand-back. | Directly: the write returns the error. |
| Memory | Pool grows by 99 blocks (about 13.1 MiB) for the FIFO and the hold. The 256 KiB replay buffer goes. | No change. The parser channel stays the hold. |
| Shutdown and STOP/FINISH | The link thread checks the deadline at each readiness wait (50 ms), hands back, then runs STOP/FINISH while the router seals. | Same checks on one thread. The seal waits for the link work. |
| TLS partial writes | Needed, but local to one write loop on the link thread. | Needed, and the partial state lives across loop turns and routing decisions. |
| Testability | Router and link writer each have a small state machine and a fake peer. | One loop mixes disk and socket state. Fault tests must script both. |
| Code size (estimate) | New `src/link.rs` of about 350 lines; about 200 changed lines in `src/sender.rs` and 100 in `src/network.rs`. | About 300 changed lines in `src/sender.rs`, and 200 in `src/network.rs` and `src/framestream.rs`. |

### TLS partial writes, both options

`FrameStreamWriter` (`src/framestream.rs`) writes with `write_all` and poisons the stream
on any error. On a non-blocking socket, `write_all` returns `WouldBlock` without the
count it already wrote, and `BufWriter` adds a second hidden offset. rustls
`StreamOwned::write` accepts plaintext, ignores the socket error, and reports it on the
next call. None of these may carry data on a non-blocking socket.

The data path uses the rustls connection directly. `conn.writer().write(piece)` returns
how much plaintext rustls accepted. `conn.write_tls(&mut tcp)` writes TLS bytes and keeps
the unwritten part inside rustls, so a `WouldBlock` never cuts a record. The link writer
feeds rustls one piece of at most `TLS_FEED_BYTES` (16 KiB, one record at the default
fragment size), ending at a frame boundary (or one whole larger frame). It drains
`write_tls` until `wants_write()` is false before it feeds the next piece. A piece is
then the only plaintext whose fate is unknown at a failure. Plaintext sessions use
`TcpStream::write` and know the exact offset.

## Recommendation: H1a with a readiness-driven link writer

Choose H1a. Reasons:

1. The router never waits on the socket. Only disk speed limits it. This fixes F1.
2. Each thread waits on one kind of event. H1b needs a combined wait that the
   standard library and crossbeam do not provide.
3. The partial-write state is a local loop on the link thread. It never crosses a
   routing decision.
4. The spool seal and STOP/FINISH run at the same time at shutdown.
5. On a failure, unsent live data returns to the router (B1). Only the unsent rest of
   one replay batch is lost, against 320 KiB at worst today.

Unlike the plan's H1a, socket I/O is not blocking, because of the R1 constraint. See
[H2](#h2-liveness-from-progress).

### Threads and ownership

| Thread | Owns | Waits on |
| --- | --- | --- |
| `dnstap-parser` | The block it fills. | Nothing (I4 unchanged). |
| `dnstap-sender` (router) | Parser channel receiver, the hold, the front list, `Spool`, FIFO sender, return receiver. | One `Select`, with the `SENDER_POLL` timeout. |
| `dnstap-link` (new) | `NetworkSink`, FIFO receiver, return sender, the block being written. | FIFO `recv_timeout(LINK_POLL)`, or socket writability for at most `LINK_POLL`. |
| `dnstap-reconnect` | One attempt, as today. | Blocking connect and handshakes under `IO_TIMEOUT` (I6 unchanged). |

`LinkShared` holds `up: AtomicBool` and the existing `Shutdown`. The link thread writes
`up`; the router reads it. A FIFO item is a `Block` tagged `Live` or `Replay`, or a
`Fence`. The return channel (capacity 4) carries `Returned(Block)`, `ReplayDone`, and
`HandBackDone`. Both threads release blocks to the one lock-free pool.

### The hold

The hold keeps link jitter in memory, as the 96 queued blocks before `SPILL_BLOCKS` do
today. It is a `VecDeque<Block>` on the router, with room for `HOLD_SPILL_BLOCKS` (96)
allocated at startup. When the FIFO is full, a live block goes to the back of the hold.
When the FIFO has room, the router moves the front of the hold to it. A live block goes
straight to the FIFO only while the hold is empty.

Only when the hold reaches 96 blocks (about 6 MiB) does the router spill: it appends the
whole hold to the spool, oldest first, then the arriving block, and enters Draining. The
spill is passive: link backpressure (FIFO full, then hold depth) is its only trigger.
No routing decision reads `SIO_TCP_INFO`; that stays an optional diagnostic (task T).

The FIFO holds older blocks than the hold, and the hold older blocks than the parser
channel. Hold to FIFO takes the front; hold to spool appends the front first. Both keep
capture order. While the hold is not empty, the router's `Select` also waits for FIFO
room; the timeout only rechecks `up` and the shutdown deadline.

### Hand-back at a link failure (B1)

1. The link thread ends the session (write error, stall, or shutdown deadline) and
   clears `up`. It does not set `up` again until step 4, even if a reconnect finishes.
2. The router sees `up` false and sends one `Fence` into the FIFO. It now takes nothing
   from the parser channel and appends nothing to the spool.
3. The link thread splits the block in hand. Frames the socket fully accepted are sent.
   The cut frame and every later frame move to the start of the same buffer
   (`copy_within`, no allocation) and go back as `Returned`. With TLS, the frames of the
   unconfirmed piece count in `disconnected_dropped`; later frames go back. A replay
   batch is never returned: its unsent rest counts in `disconnected_dropped`, then
   `ReplayDone`. The link thread then takes FIFO items in order: a live block goes back
   whole, a replay batch counts whole, and the `Fence` ends the hand-back.
4. The link thread sends `HandBackDone` and may now publish `up` after a reconnect.
5. The router places the returned blocks first, then the hold, then newer blocks:
   - No backlog: it appends them to the spool in that order.
   - A backlog exists (the failure came soon after a spill): the returned blocks are
     older than the backlog, so the router keeps them on the front list (at most 3
     blocks) and sends them first after the reconnect, before any replay. The hold and
     newer blocks go to the spool.
   - No spool: their frames count once in `disconnected_dropped`.

The router is the only spool writer, and it writes nothing between steps 2 and 5, so no
newer block can pass a returned one. The hand-back is memory work on at most 3 blocks.
The router waits for it in the same `Select`, so the return channel never blocks the
link thread for long, and the parser channel (128 blocks) covers the wait.

### Router state machine

| State | Condition | On a live block | Replay | Next state |
| --- | --- | --- | --- | --- |
| Direct | `up`, no backlog, hold empty | Front list first. Then `try_send` to the FIFO; `Full`: push to the hold. | None. | Hold not empty → Holding. `up` false → Fencing. |
| Holding | `up`, no backlog, hold not empty | Push to the hold. On FIFO room, move the hold front to the FIFO. | None. | Hold empty → Direct. Hold reaches 96: spill → Draining. `up` false → Fencing. |
| Draining | `up`, backlog, hold empty | Append to the spool. | Front list first. Then, with no replay batch out: `read_batch` into a pool block (64 KiB), `send` it to the FIFO. | Last batch read → Direct. `up` false → Fencing. |
| Fencing | `up` just went false | Not taken: blocks wait in the parser channel. | None. | Send `Fence`, collect `Returned` until `HandBackDone`, place them (step 5) → Down. |
| Down | not `up`, hand-back done | Append to the spool. With no spool, count in `disconnected_dropped`. | None. | `up` true → Direct or Draining. |
| Closing | Shutdown started, within the link budget | As in the states above, but no replay. | None. | Budget ends, or channel closed and empty and hold sent → Sealing. |
| Sealing | Past the budget, or the channel closed and empty | Drop the FIFO sender (the fence). Collect `Returned` until the return channel closes. Then returned blocks, the hold, the channel: to the spool. | None. | `seal`, join the link thread → Done. |

The router is the only FIFO producer, and it sends at most one replay batch at a time:
it waits for `ReplayDone` before it reads the next (the replay credit). When `Select`
reports room, the replay `send` cannot block, so a batch whose cursor moved always
reaches the link thread (I3). Before each replay step the router moves every queued live
block to the spool, as today. With no spool, a full hold makes the router stop receiving;
the parser channel fills, and the parser counts `queue_dropped`, as today.
`SPILL_BLOCKS` goes; `HOLD_SPILL_BLOCKS` takes its place on the hold.

In Sealing, returned blocks normally become the head of the spool. If a backlog exists,
they are older than it and cannot go first, so their frames count in
`disconnected_dropped` (at most 3 blocks; this needs a failure soon after a spill and a
shutdown before the reconnect).

### Link writer state machine

| State | Event | Action | Next state |
| --- | --- | --- | --- |
| Disconnected | Each `LINK_POLL` | `NetworkSink::poll` (helper thread). | Attempt succeeds → set `up`, Idle. |
| Idle | Block arrives | Start at offset 0; `last_progress = now`. | Writing |
| Idle | FIFO closed and empty | Drop the return sender, so the router stops waiting. | Finishing |
| Writing | `write` or `write_tls` accepts bytes | Move the offset; `last_progress = now`. | Block done (live: release; replay: `ReplayDone`) → Idle |
| Writing | `WouldBlock` | Wait for writability, at most `LINK_POLL`. | Writing |
| Writing | `now - last_progress >= stall_timeout`, or a write error | End the session; start the backoff, as `connection_lost` does. | HandingBack |
| Writing or Idle | Shutdown deadline passes | Stop writing. | HandingBack, then Finishing |
| HandingBack | — | Clear `up`. Split the block in hand; return or count it (step 3). Take FIFO items until `Fence` or until the FIFO closes. Send `HandBackDone`; at shutdown, drop the return sender instead. | Disconnected (or Finishing) |
| Finishing | At a block boundary | Socket back to blocking with `DeadlineTcp`; STOP, then FINISH, under `IO_TIMEOUT`. | Done |
| Finishing | Mid-frame | Close with no STOP (the frame writer poison rule); log the warning. | Done |

Blocks go straight to the transport, so the `BufWriter`, `FLUSH_INTERVAL`, and `dirty`
go. The parser's 100 ms and 1 s partial-block timers still bound the delay.

### Loss accounting on a link failure

A block is "handed to the link" when the router's `try_send` or `send` returns `Ok`.
Each frame in it then ends in exactly one way:

| End | Counted by | Counter |
| --- | --- | --- |
| The kernel accepted all its bytes. | nobody | none: sent, as today. Kernel-buffered bytes are the only live data still lost at a failure, and they stay uncounted. |
| Live, cut or not yet accepted. | the router, after the hand-back, as an ordinary block | `spool_dropped` if the spool refuses it; `disconnected_dropped` with no spool or in the Sealing case |
| Live, in the unconfirmed TLS piece. | the link thread | `disconnected_dropped` |
| Replay, not fully accepted. | the link thread | `disconnected_dropped` |

The router also counts the hold and the front list. The parser counts only a full channel
(`queue_dropped`). Each block has one owner at a time, so I2 holds.

### Constants

| Constant | Value | Arithmetic |
| --- | --- | --- |
| `LINK_FIFO_BLOCKS` | 2 (was 4) | 2 × 64 KiB = 128 KiB queued. With the replay credit, at most one replay batch is in the FIFO or in hand. |
| Loss at a failure | less than 134 KiB, only around Draining | The unsent rest of one replay batch: below 64 KiB + 70 KiB + 4 bytes. With TLS, add one piece of at most 16 KiB, or one frame of up to 70 KiB. Today the worst case is 320 KiB. |
| `TLS_FEED_BYTES` | 16 KiB | One TLS record at the default fragment size, so the unconfirmed plaintext is one piece. |
| `HOLD_SPILL_BLOCKS` | 96 | 96 × 64 KiB = 6 MiB, today's slack before `SPILL_BLOCKS`. |
| Link pause held in memory | — | (hold 6 MiB + FIFO 128 KiB + `SO_SNDBUF` 1 MiB) / R_in: about 3.6 s at 10,000, 0.73 s at 50,000, and 0.18 s at 200,000 queries per second. Today's figure is about the same. |
| Router pause before the link idles | — | (FIFO 128 KiB + `SO_SNDBUF` 1 MiB) / R_link: about 29 ms at 41 MB/s and 12 ms at 100 MB/s. This costs throughput, not frames: the parser channel absorbs the pause. |
| Spill threshold | Hold reaches 96 | The FIFO is full (`try_send` returns `Full`) and the hold has filled behind it. |
| `REPLAY_BATCH_BYTES` | 64 KiB (was 256 KiB) | `read_batch` stops after the frame that crosses the limit, so a batch is below 64 KiB + 70 KiB + 4 bytes. It fits `BLOCK_CAPACITY` (136 KiB) with no reallocation. |
| Replay credit | 1 batch | The next batch is read after `ReplayDone`. The kernel buffer (1 MiB) keeps the wire busy across the router's turn. |
| `SEND_QUEUE_BLOCKS` | 128, unchanged | User decision: the parser channel stays at 128 blocks. |
| `POOL_BLOCKS` | `SEND_QUEUE_BLOCKS + HOLD_SPILL_BLOCKS + LINK_FIFO_BLOCKS + 3` = 229 | One block each for the parser, the router, and the link thread. The front list and returned blocks reuse the FIFO and in-hand blocks. 229 × 136 KiB = 30.4 MiB, against 17.3 MiB today. |
| `LINK_POLL` | 50 ms | The readiness and FIFO wait. It bounds the reaction to shutdown and to the stall check, and the reconnect poll (today `SENDER_POLL`, 50 ms). |
| Stall timeout | 15 s default | `--receiver-stall-seconds`, 2 to 300 (accepted by the user). At 50,000 queries per second the spool takes 15 s × 10.3 MB/s = 155 MB, about 4 % of the 4,096 MiB default. |
| `IO_TIMEOUT` | 2 s, unchanged | Connect, TLS handshake, READY/ACCEPT/START, and STOP/FINISH only. Never a data write. |
| `SHUTDOWN_LINK_BUDGET` | 1 s, unchanged | — |

### H2. Liveness from progress

**Rule.** The link thread ends a session only when it has bytes to write and the
receiver accepted none for the stall timeout. A write call that accepts at least one
byte is progress. With TLS, only `write_tls` counts; rustls taking plaintext is not
progress. An idle link never stalls. The rule uses only the byte counts the link writer
sees from its own calls. It does not read `SIO_TCP_INFO` or any other kernel state.

**Mechanism: non-blocking sends with readiness, on the link thread.** After the helper
finishes the handshakes, the link thread calls `set_nonblocking(true)`. A data write is
a non-blocking `write` or `write_tls`. On `WouldBlock` the thread waits for writability
with `select` (or `WSAPoll`) on the one socket, for at most `LINK_POLL`. It then checks
the stall timer and the shutdown deadline, and writes again. No data write uses
`SO_SNDTIMEO`, so no send times out, and the R1 rule holds. `WouldBlock` is a defined
result that queued nothing. The wait needs a small `unsafe` call in
`src/network/sockopt.rs`; the `Win32_Networking_WinSock` feature is already on.

**Rejected: a blocking send with `SO_SNDTIMEO` equal to the stall timeout.** It is sound
for liveness, because a timeout ends the session. It breaks I5: a send that blocks for
15 s cannot stop within the 1 s link budget. To stop it early, another thread must close
the socket under the call. The bytes that call queued are then unknown, so B1 could not
split the block, and the link thread can outlive the 5 s bound.

**Interaction with `IO_TIMEOUT`.** Connect and the handshakes stay blocking on the
`dnstap-reconnect` helper, with `DeadlineTcp` and `IO_TIMEOUT`. A timeout there ends an
attempt, not a live session. For STOP/FINISH the link thread puts the socket back in
blocking mode with a fresh `IO_TIMEOUT` deadline. A timeout there ends a session that is
ending anyway. The stall timeout is never shorter than `IO_TIMEOUT`.

**Interaction with shutdown.** Once shutdown starts, the link thread stops at the
earlier of the stall deadline and the link deadline. The stall timeout never extends
shutdown. The hand-back runs before STOP/FINISH and touches only memory, so it adds at
most one `LINK_POLL`.

| Step (from `QueuedSink::finish`) | Today | This design |
| --- | --- | --- |
| Final partial block | 0.1 s | 0.1 s |
| Link budget | 1 s | 1 s |
| Write in progress at the deadline, then the hand-back | up to 2 s | up to `LINK_POLL`, 0.05 s |
| STOP/FINISH | up to 2 s | up to 2 s |
| Returned blocks, hold, and channel to the spool, then seal | up to 128 blocks, after the link | up to 227 blocks (14 MiB), at the same time as STOP/FINISH |
| Total | about 5 s | about 3.2 s, or longer only if the seal takes longer than STOP/FINISH |

**Interaction with the spool.** A stall shorter than the timeout fills the kernel
buffer, the FIFO, and the hold; the router spills; nothing is lost and no reconnect
happens. With no spool, the loss is `queue_dropped` until the receiver resumes.

## Tests

### Characterization tests to flip (C1)

| Behavior today | Assertion after task I1 |
| --- | --- |
| F1: a write stall drops frames in `queue_dropped` while the spool has room. | `queue_dropped` is 0. Every frame arrives in capture order after the stall. |
| F2: a stall longer than 2 s disconnects and counts in-flight frames in `disconnected_dropped`. | A stall shorter than the stall timeout: `disconnects`, `reconnects`, and `disconnected_dropped` are 0, and every frame arrives in order. With the timeout set to 2 s it disconnects, the unsent live frames go to the spool, and `disconnected_dropped` is 0. |
| F3: a sustained slow receiver keeps every block on the spool path. | Unchanged: gate G2 chose H3a. |

These existing tests must still pass, some with new constants:
`shutdown_with_a_stalled_link_and_queued_blocks_is_bounded_and_accounted`,
`a_link_that_fails_mid_replay_loses_the_failed_batch_and_sends_nothing_twice`,
`live_data_does_not_wait_behind_a_replay`,
`a_slow_receiver_spills_to_the_spool_instead_of_dropping`, and
`a_slowly_reading_receiver_makes_blocks_spool_and_then_drain_in_order`.
`routing_follows_the_backlog_the_link_and_the_queue_depth` changes from queue depth to
hold depth. `receiver_that_stops_reading_causes_one_bounded_stall_then_counted_drops`
sets a short stall timeout and now expects the hand-back.

### New tests

All fault tests check produced = delivered + `queue_dropped` + `disconnected_dropped` +
`spool_dropped` + still spooled, no complete frame twice at the receiver, and delivered
frames in capture order.

1. **Hand-back at the end of a drain.** Plaintext. The link fails after N bytes with the
   last replay batch in hand (cut) and 2 live blocks in the FIFO. Assert:
   `disconnected_dropped` equals the unsent rest of that replay batch only; the 2 live
   blocks are the spool head, before the hold and newer blocks; after the reconnect they
   arrive first, in order.
2. **Hand-back soon after a spill.** A live block in hand (cut) and the FIFO holding a
   live block and a replay batch, with a backlog. Assert: `disconnected_dropped` equals
   that replay batch only; the live rest goes to the front list and arrives first after
   the reconnect, then the backlog from the cursor, then newer frames.
3. **Fence race.** The router routes while the link fails. A barrier holds the link
   thread after it clears `up`. Assert: the router appends nothing to the spool before
   `HandBackDone`; each frame ends once.
4. **Cut frame resent.** The transport accepts K bytes that end inside frame F, then
   resets. The old session's receiver sees a truncated F and discards it. The new session
   delivers F in full, once.
5. **TLS unconfirmed piece.** A TLS session (the `rcgen` test receiver) fails while
   `write_tls` is part-way through a piece. Its frames count in `disconnected_dropped`;
   later frames go back; nothing arrives twice.
6. **Partial writes.** A transport that accepts 1 byte, then returns `WouldBlock`, in
   turn. Plaintext and TLS: every frame arrives intact.
7. **Stall timer.** With a 2 s timeout, 1 byte accepted each second keeps the session.
   No bytes for 2 s ends it, with one `disconnects`.
8. **Router stays free.** A transport that is never writable: the parser channel stays
   short, the spool grows, and `queue_dropped` stays 0.
9. **Shutdown under a stall.** FIFO full of live blocks, link never writable: `finish`
   returns in less than 4 s; the returned blocks are on disk before the hold;
   `disconnected_dropped` is 0; the next run replays them in order.
10. **STOP rules.** Shutdown at a block boundary runs STOP/FINISH. Shutdown mid-frame
    sends no STOP, and the cut frame is on disk.
11. **Backlog to live.** A property test (`proptest`) over stall, failure, and resume
    sequences: delivered order equals capture order.
12. **Pause shorter than the hold.** At 50,000 queries per second, the link accepts no
    bytes for 300 ms (less than the 0.73 s the hold covers). `spooled` stays 0, every loss
    counter stays 0, and every frame arrives in order.
13. **No spool.** A full hold backs up into `queue_dropped` only; a failure counts the
    returned frames once in `disconnected_dropped`.
14. **Pool, flag, reconnect.** The parser never takes the `Block::new` fallback (a
    test-only counter). `--receiver-stall-seconds` accepts 2 and 300, rejects 1 and 301.
    A stall timeout starts the backoff and a `dnstap-reconnect` attempt.

## Decisions, risks, and open questions

### Decisions

- **Stall timeout.** The user accepted the 15 s default and the name
  `--receiver-stall-seconds` on 2026-11-03. The MSI needs a matching property.
- **Unsent data at a failure.** User chose B1 on 2026-11-03: unsent live data goes back
  to the router, and only replay data whose cursor moved can be lost.
- **Parser channel.** It stays at 128 blocks.
- **Saturated link.** User chose H3a on 2026-11-03: strict capture order, with the
  oldest spool data deleted at the cap. With H4, the fixed `SO_SNDBUF` goes, so Windows
  send autotuning stays on. H3b, H3c, and the other shedding options are not built.

### Risks and open questions

1. **Half-open receiver.** 15 s delays its detection from 2 s. With a spool this costs
   disk, not frames. Without a spool it is up to 15 s of `queue_dropped`.
2. **Disk is the new limit.** `queue_dropped` can still occur if spool appends are slower
   than R_in. Lab task L should measure the append rate.
3. **Counter meaning.** Until task I1, task D keeps today's meaning of `write_max_us` and
   `slow_writes`: one blocking socket write. Task I1 redefines them as the readiness wait
   per block, because no send blocks after it.
4. **H4.** Without the fixed `SO_SNDBUF`, the kernel buffer can grow. More bytes then
   count as sent and can be lost unseen at a failure, as the 1 MiB buffer is today.
5. **Trickle receiver.** A receiver that accepts a few bytes before each stall timeout
   keeps the session forever. Liveness does not detect it. Plan detection D4 (link
   saturated) reports it.
6. **Receiver handling of a cut frame.** B1 assumes that a receiver drops a frame cut by
   a closed connection. fstrm readers do. Lab task L should confirm it for the lab
   collector.
7. **rustls API.** The data path relies on `writer()`, `write_tls`, and `wants_write` in
   rustls 0.23, and on the default fragment size. A version change must rerun tests 5
   and 6.
8. **Future work, B2.** Re-read replay batches after a failure: an in-memory rewind of
   the spool reader, with segment deletion deferred until the batch is confirmed. A
   cheaper step is to put the unsent rest of the replay batch on the front list. Both
   are deferred.
9. **Saturated link (H3).** Decided: H3a. The other choices stay on record in case
   operators ask later for fresh data over complete data. How each fits this design:
   - **H3a, strict order.** This design as written. The backlog grows, and the spool
     evicts the oldest data into `spool_dropped`.
   - **H3b, live first.** Only the router's Draining row changes: live blocks go to the
     FIFO when it has room, and replay uses the slot left. The hold and the hand-back do
     not change. Order across the backlog is lost, and I1 must be restated.
   - **H3c, shed by type.** Blocks are opaque bytes, so the router cannot shed one frame.
     The router publishes a `saturated` flag. The parser reads it (one atomic load per
     frame) and drops `CLIENT_QUERY` frames into a new counter. I2 names four counters.
