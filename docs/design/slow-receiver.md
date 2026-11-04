# Slow receiver: link writer and progress liveness

> **Status:** Built (H1, H2, B1, D, and B2) · **Describes:** the `dnstap-sender` routing and the link at `main` commit `a06f6fd`, and the change to them on branch `slow-receiver` (tasks H1 and H2 of the [slow and saturated receiver plan](../plans/slow-receiver-plan.md)) · **Evidence dated:** B2 throughput on the build host, 2026-11-04; otherwise not applicable (source analysis; no lab run) · **Reviewed:** 2026-11-03; B2 on 2026-11-04 (review V4)

This design keeps the sender thread free of socket waits, and ends a session only when
the receiver makes no progress. On a failure, unsent live data goes back to the router
(decision B1). Hand-offs between threads are batched through a deep FIFO, so the design
keeps its throughput on a CPU-saturated host (decision D). A spool rewind (task B2)
returns unsent replay data to the spool, so a failure counts no replay frame.
It compares two designs, recommends one, and gives its state machines, constants, and
tests. For a saturated link it keeps today's strict order (H3a, decided at gate G2).

## Problem

### F1. A blocked write stops the spill

The `dnstap-sender` thread routes blocks and writes the socket (`SenderThread::run`,
`src/sender.rs`). `NetworkSink::write_framed` (`src/network.rs`) splits a block into
chunks of up to 64 KiB. Each chunk has its own 2 s deadline (`IO_TIMEOUT`,
`DeadlineTcp`). A block is 2 or 3 chunks, and a 256 KiB replay batch is 4 or 5. While a
chunk blocks, the thread routes nothing, so the spill rule (`queued >= SPILL_BLOCKS`)
never runs.

The queue holds 128 blocks of about 64 KiB (8 MiB); the spill point leaves 2 MiB. One
chunk that blocks for its full 2 s fills that above about 10,000 queries per second at
the 103-byte lab average (2 MiB in 2 s); at 50,000 it lasts about 0.2 s. The parser then
counts `queue_dropped` while the spool has room.

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
| I3 | Delivery is at most once. `Spool::read_batch` saves the cursor before it returns a batch. A batch lost on a failed link counts in `disconnected_dropped`. Under B1 a frame cut mid-write is sent again; the receiver saw only a truncated copy, so it never gets the frame twice. B2: the cursor still moves before a batch is handed out, and a rewind sets it back to the delivered position, so unsent replay data is read again, not lost. A crash loses only the read-ahead. | `Spool::read_batch`, `SenderThread::replay_step` |
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
| Threads and ownership | Router owns the spool, the parser channel, and the FIFO sender. Link thread owns the session, the reconnect state, and the FIFO receiver. A block has one owner at a time; unsent live blocks come back by a return channel. | One thread owns everything, as today. |
| Waiting | Router waits with one `crossbeam_channel::Select` (parser channel, return channel). Link thread waits on FIFO readiness or socket readiness. Each drains all it can per wake (D). | The thread must wait for "a block arrived" or "the socket is writable". No call waits on a crossbeam channel and a socket together, so it must alternate short waits. |
| Loss on a link failure | Unsent live frames go back to the router. Only the unsent rest of two replay batches (less than 268 KiB, in Draining) and, with TLS, one unconfirmed piece are lost (B2: no replay frame is lost). | The tail can go back by the same rules; the hand-back is simpler, but the tail stays in the routing loop. |
| How the router learns of a failure | The link thread returns its block in hand and sends `HandBackDone`. The router drains the FIFO itself. | Directly: the write returns the error. |
| Memory | Pool grows by 130 blocks (about 17.3 MiB): the FIFO (128 blocks when designed, 256 now), the front list, and a block for the link thread. | No change. |

### TLS partial writes, both options

On a non-blocking socket, `write_all` (used by `FrameStreamWriter`) returns `WouldBlock`
without the count it wrote, `BufWriter` hides a second offset, and rustls
`StreamOwned::write` reports a socket error only on the next call. None may carry data.

The data path uses the rustls connection directly. `conn.writer().write(piece)` returns
how much plaintext rustls accepted. `conn.write_tls(&mut tcp)` writes TLS bytes and keeps
the unwritten part inside rustls, so a `WouldBlock` never cuts a record. The writer feeds
one piece of at most `TLS_FEED_BYTES` (16 KiB, one record), ending at a frame boundary,
and drains `write_tls` until `wants_write()` is false before the next. That piece is the
only plaintext whose fate is unknown at a failure. Plaintext sessions know the offset.

> **As built.** rustls limits what one `write` takes to its 64 KiB buffer, so a frame larger
> than that is fed in more than one call. A frame that rustls has not taken whole cannot have
> arrived whole, so it is returned and sent again, like the cut frame of a plaintext session.
> Only frames that rustls took whole and the socket did not confirm count in
> `disconnected_dropped`. `write_tls` is drained to empty before each next piece, as designed.
> A unit test checks that a piece of 16 KiB is one record (risk 7).

## Recommendation: H1a with a readiness-driven link writer

Choose H1a. Reasons:

1. The router never waits on the socket. Only disk speed limits it. This fixes F1.
2. Each thread waits on one kind of event; H1b needs a channel-and-socket wait that
   neither the standard library nor crossbeam provides.
3. The partial-write state is a local loop on the link thread.
4. The spool seal and STOP/FINISH run at the same time at shutdown.
5. Unsent live data returns to the router (B1); only two replay batches are at risk, and
   B2 removes that risk.

Unlike the plan's H1a, socket I/O is not blocking, because of the R1 constraint. See
[H2](#h2-liveness-from-progress).

### Threads and ownership

| Thread | Owns | Waits on |
| --- | --- | --- |
| `dnstap-parser` | The block it fills. | Nothing (I4 unchanged). |
| `dnstap-sender` (router) | Parser channel receiver, the front list, `Spool`, FIFO sender, a FIFO receiver clone, return receiver. | One `Select`, with the `SENDER_POLL` timeout. |
| `dnstap-link` (new) | `NetworkSink`, FIFO receiver, return sender, the block being written. | FIFO readiness (`Select::ready_timeout(LINK_POLL)`), or socket writability for at most `LINK_POLL`. |
| `dnstap-reconnect` | One attempt, as today. | Blocking connect and handshakes under `IO_TIMEOUT` (I6 unchanged). |

> **As built.** The two threads live in `src/sender/link.rs` and `src/sender/router.rs`, not in a
> new `src/link.rs`, because they share `Block`, the pool, and `Shutdown` with `QueuedSink`. The
> link thread drives the transport through two traits in `src/network/wire.rs`: `Uplink` (the
> reconnecting session provider, implemented by `NetworkSink`) and `Wire` (one session's
> non-blocking data path). Tests run the same link code on a scripted transport and a manual
> clock. `NetworkSink` keeps its blocking `FrameSink` implementation for the parser and reconnect
> tests; the service does not use it.

`LinkShared` holds `up: AtomicBool` and the existing `Shutdown`. The link thread writes
`up`; the router reads it. `LinkShared` also holds the take lock (a `Mutex<()>`); see
[the spill](#batched-hand-offs-and-the-passive-spill-d). A FIFO item is a `Block` tagged
`Live` or `Replay`. The `Fence` is a message on its own channel (As built, 6). The return
channel (capacity 129 with B2; 4 before) carries `Returned(Block)`, `ReplayDone`, and
`HandBackDone`. Both
threads release blocks to the one lock-free pool.

### Batched hand-offs and the passive spill (D)

On a CPU-saturated host, a thread that waits for a block can wait a scheduler quantum
before it runs, so a shallow FIFO moves about (depth × 64 KiB) per quantum. Review V2
measured the collapse (see [Decisions](#decisions)). The old single sender drained up to
128 blocks per wake. Decision D restores that without a priority change:

- **Batching.** The link thread takes the next FIFO item as soon as it finishes a block,
  and waits only when the FIFO is empty. The router, per wake, routes every block it can
  take from the parser channel and reads replay batches up to its credit before it waits.
- **No hold.** The deep FIFO is the in-memory slack. The spill stays passive: it happens
  only when `try_send` returns `Full`. No routing decision reads `SIO_TCP_INFO` (task T).
- **Take lock.** The link takes a FIFO item only under the take lock, with `try_recv`,
  and holds at most one taken item. It waits with `Select::ready_timeout`, which takes
  nothing. The channel is MPMC: the router's receiver clone and the link see one queue,
  and each item goes to exactly one of them.
- **Spill drains the FIFO.** On `Full`, the router takes the lock, pops every FIFO item
  into a buffer preallocated for `LINK_FIFO_BLOCKS`, and releases the lock (a few
  microseconds). It appends those blocks to the spool, oldest first, then the block that
  did not fit, and enters Draining.

**Order at a spill.** Let x1..xn be the FIFO items when the router takes the lock. The
link has finished every older item or holds exactly one. It cannot take an xi while the
router holds the lock, so x1..xn and the new block reach the spool in capture order. The
link writes its held block before it takes the next item, a replay batch of x1 onward.

**Front list bound: 1 block (134 KiB).** After a spill, the only live block older than
the backlog is the one the link held. In Draining the router sends only replay batches
and the front list, so a failure returns at most that one block. The router leaves
Draining only when a read finds nothing and both credits are back, so the FIFO holds no
replay batch in Direct.

### Hand-back at a link failure (B1, with D)

The link returns only its block in hand; the router drains the FIFO itself.

1. The link thread ends the session (write error, stall, or shutdown deadline), clears
   `up`, and stops taking FIFO items.
2. It splits the block in hand. Fully accepted frames are sent. The cut frame and later
   frames move to the buffer start (`copy_within`) and go back as `Returned`. With TLS,
   the frames of the unconfirmed piece go back too (B2; before B2 the piece of a live block
   counted in `disconnected_dropped`). A replay batch is never returned. Before B2 its unsent
   rest counted there, then `ReplayDone`. With B2 the link counts nothing: `ReplayDone`
   reports the accepted prefix, and the unsent rest stays in the spool
   ([B2](#b2-spool-rewind)).
3. It sends `HandBackDone` (at shutdown it drops the return sender instead). It may
   reconnect, but it neither publishes `up` nor writes until it receives a `Fence`.
4. The router stops sending when it sees `up` false or `HandBackDone`. After
   `HandBackDone` it drains the FIFO with its clone: live blocks follow the returned one;
   a replay batch returns its credit and counts in `disconnected_dropped` (B2: it counts
   nowhere, and the router rewinds the spool after the drain). It takes no parser block and
   appends nothing to the spool until step 5.
5. It places the returned block, the drained live blocks, then newer blocks: on the
   spool with no backlog; on the front list (at most 1 block) with a backlog, sent
   first after the reconnect; counted once in `disconnected_dropped` with no spool.
6. It sends a `Fence` (its own channel, As built 6), with the FIFO empty, and enters Down.
   The link may publish `up`.

**Proofs.**

- **No deadlock.** The link has at most `REPLAY_CREDIT` + 2 unread messages out
  (`REPLAY_CREDIT` `ReplayDone`, 1 `Returned`, 1 `HandBackDone`: 4 before B2, 129 with B2),
  and the return channel holds that many. The `Fence` channel
  holds one message and is empty when the router sends it. The link waits for it with a
  timeout and checks the shutdown deadline.
- **No double count.** Each FIFO item is received once (MPMC). The block in hand is the
  link's alone. A credit comes back once: by `ReplayDone` or by the router's drain.
- **No reorder.** The router stops producing before it drains, and the link stopped
  taking at step 1, so the drain sees every untaken item. The returned block is older
  than all of them, and nothing newer is placed before step 5.
- **Shutdown racing a failure.** At the deadline the link does steps 1 and 2 and drops
  the return sender. The router treats the closed channel like `HandBackDone`, drains
  the FIFO, and seals. Either order of the two events gives the same steps.
- **Reconnect racing the hand-back.** A session finished early stays unused until the
  `Fence`, so nothing is written before every older block is placed. An idle link does
  not run the stall timer.

### Router state machine

| State | Condition | On a live block | Replay | Next state |
| --- | --- | --- | --- | --- |
| Direct | `up`, no backlog | Front list first. Then `try_send` to the FIFO. `Full`: spill (drain the FIFO to the spool, then this block). | None. | Spill → Draining. `up` false or `HandBackDone` → HandBack. |
| Draining | `up`, backlog | Append to the spool. | Front list first. Then, per free credit (2; 127 with B2, at most `REPLAY_TURN_BYTES` per turn): `read_batch` into a pool block (64 KiB), `send` it to the FIFO. | A read finds nothing and every credit is back → Direct (B2: the last `ReplayDone` deletes the last segment, so this happens at that `ReplayDone`). `up` false or `HandBackDone` → HandBack. |
| HandBack | The link ended its session | Not taken: blocks wait in the parser channel. | None. | Wait for `HandBackDone`, drain the FIFO, rewind the spool (B2), place the blocks (steps 4 to 5), send `Fence` → Down. |
| Down | not `up`, hand-back done | Append to the spool. With no spool, count in `disconnected_dropped`. | None. | `up` true → Direct or Draining. |
| Closing | Shutdown started, within the link budget | As in the states above, but no replay. | None. | Budget ends, or channel closed and empty → Sealing. |
| Sealing | Past the budget, or the channel closed and empty | Drop the FIFO sender. Wait until the return channel closes. Drain the FIFO. Then the returned block, the FIFO blocks, and the channel: to the spool. | None. | `seal`, join the link thread → Done. |

The router is the only FIFO producer, with at most `REPLAY_CREDIT` replay batches out (2;
127 with B2). In Draining the FIFO holds only those and the front list, so a replay `send`
always has a slot. Before B2 a batch whose cursor moved had to reach the link (I3). With B2
a batch that does not is read again. With no spool, a full FIFO stops the
router, and the parser counts `queue_dropped`, as today. `SPILL_BLOCKS` goes. In Sealing
with a backlog, live blocks older than it count in `disconnected_dropped`: at most 1
block (134 KiB) by the front-list bound.

As-built notes 1 to 3 were written before decision D and are corrected below for it:
note 2's bound is 1 block, and note 3's hand-back is the one in steps 1 to 6. Notes 6 to
11 record what the build under D does differently from the text above. B2 changes notes
1, 2, and 9; each says how.

> **As built, 1: the end of a drain.** `Spool::read_batch` does not delete the last segment in
> the call that returns the last data. The next call returns 0 and deletes it, and the router
> makes that call only after `ReplayDone` (the replay credit). Live blocks that arrive in
> between go to the spool, in order, and the router reads them in that next call, so Draining
> turns into Direct only at a read that finds nothing. Order is kept, but a state with a replay
> batch in hand and live blocks behind it in the FIFO cannot arise. Test 1 below therefore
> covers the hand-back without a backlog (returned blocks at the head of the spool, before the
> blocks that were in the FIFO) and the end of a drain (only the unsent rest of the last batch is lost) as two tests.
> **B2 changes this.** The reader no longer deletes a segment. `Spool::delivered` does, at
> the last `ReplayDone` ([Segment deletion](#segment-deletion)).
>
> **As built, 2: Sealing with a backlog.** The loss in the paragraph above needs only a spill
> and a shutdown (a stalled link at shutdown), not also a failure. Under D the bound is 1
> block (about 134 KiB), counted once in `disconnected_dropped`: a spill empties the FIFO into
> the spool, so the only live block older than the backlog is the one the link thread held.
> (Before D it was the block in hand and two FIFO blocks, at most 3.) The replay batches that
> the router has out at that moment, at most two, are counted too, as at any failure in
> Draining. **B2 changes this.** Those batches are not counted. They are read again
> ([Rewind](#rewind-at-the-hand-back)).
>
> **As built, 3: shutdown hand-back.** The link thread stops at the shutdown deadline, clears
> `up`, returns its block in hand, drops the return channel, and ends. It does not wait for
> the FIFO. The router treats the end of the channel as `HandBackDone`, takes the FIFO, and
> seals. A router that finishes a hand-back after its own deadline closes the fence channel
> instead of sending a fence, which ends the link thread's wait the same way.
>
> **As built, 4: the closing exchange.** Windows non-blocking sends take a whole block or
> nothing, so a stalled receiver usually leaves the link at a block boundary. STOP then
> waits behind a full send buffer, and its 2 s timeout used to end the service with an
> error, which SCM restarts. Two rules follow. When the shutdown deadline interrupts a
> block, the link closes with no STOP, because the receiver is not keeping up; a stalled
> shutdown takes about 1.1 s. And a closing STOP/FINISH that does not complete never fails
> the service: `NetworkSink` returns `Ok` and writes nothing about it (no log event, no
> counter), because every data frame was already accepted, handed back, or counted. The
> same holds for a close with no STOP. The worst case is a link that finishes just inside the budget followed by
> a closing exchange at its 2 s bound: about 3 s.
>
> **As built, 5: start-up.** `up` starts clear. The link thread enters link mode on the first
> session, publishes `up`, and signals `QueuedSink::start`, which waits for the signal before
> it starts the router. A first session that cannot enter link mode is lost, and the router
> sends blocks to the spool.
>
> **As built, 6: the fence is its own channel.** The fence is a message on a channel of
> capacity 1, not a FIFO item. After `HandBackDone` the router drains the FIFO, and the link
> thread must not look into the FIFO until that is done: a `try_recv` that looked for the
> fence could take a live block that the router is about to place, ahead of the blocks
> older than it. On its own channel the link waits for the fence with a plain timed receive.
> The close of the channel (the router sealing) ends the wait. `Item` is `Live` or `Replay`
> only.
>
> **As built, 7: the spill sends again under the lock.** The link thread may take FIFO items
> between the failed send and the moment the router gets the take lock. With the lock held,
> one more `try_send` settles whether the FIFO is still full. If it has room, the block goes
> in and nothing is spilled. The router is the only producer, so no one can fill the room.
>
> **As built, 8: the router's receiver clone.** The router holds a receiver of the FIFO, so
> a send never reports that the link thread is gone. The router learns it from the return
> channel, which the link thread drops when it ends. The router then takes the FIFO and
> places the blocks, as at a hand-back.
>
> **As built, 9: leaving Draining.** `Spool::read_batch` deletes the last segment in the call
> that returns 0 (note 1). With credit 2, that call can come while the other batch is still
> out. The router therefore sends live blocks to the spool while a replay batch is out, even
> when the spool no longer holds a backlog, and goes back to Direct only when every credit is
> back. The FIFO then holds no replay batch in Direct, so a spill never meets one. (The spill
> and the hand-back still count a replay batch if they find one.) **B2 changes this.** A
> segment with frames out is not deleted, so `has_backlog` stays true while a batch is out.
> The rule stays as a guard. The spill never finds a replay batch, and the hand-back
> rewinds instead of counting.
>
> **As built, 10: the pool counts the front list.** `POOL_BLOCKS` has a term of its own for
> the front list (`FRONT_LIST_BLOCKS`, 1), although the block on it was in the link thread's
> hand. The pool bound then does not depend on that argument. A test probe records the
> largest size the front list reaches, and the tests assert that it is at most 1.
>
> **As built, 11: test seams.** `LinkThread::step` and `Router::turn` each do one pass
> without waiting, and `serve` and `run` are loops over them. The batching test runs both on
> one thread in a fixed order, which is the manual scheduler. `LinkHooks::in_take` runs on
> the link thread while it holds the take lock.

### Link writer state machine

| State | Event | Action | Next state |
| --- | --- | --- | --- |
| Disconnected | Each `LINK_POLL` | `NetworkSink::poll` (helper thread). | Attempt succeeds → set `up`, Idle. |
| Idle | FIFO ready | Take one item under the take lock (`try_recv`); start at offset 0; `last_progress = now`. | Writing |
| Idle | FIFO closed and empty | Drop the return sender, so the router stops waiting. | Finishing |
| Writing | `write` or `write_tls` accepts bytes | Move the offset; `last_progress = now`. | Block done (live: release; replay: `ReplayDone`). Next item at once if any → Writing; else → Idle |
| Writing | `WouldBlock` | Wait for writability, at most `LINK_POLL`. | Writing |
| Writing | `now - last_progress >= stall_timeout`, or a write error | End the session; start the backoff, as `connection_lost` does. | HandingBack |
| Writing or Idle | Shutdown deadline passes | Stop writing. | HandingBack, then Finishing |
| HandingBack | — | Clear `up`; stop taking FIFO items. Split the block in hand; return or count it (step 2). Send `HandBackDone`; at shutdown, drop the return sender instead. | Fenced wait (or Finishing) |
| Fenced wait | `Fence` received | A reconnect may finish meanwhile; nothing is written. | Session ready → set `up`, Idle; else Disconnected |
| Finishing | At a block boundary | Socket back to blocking with `DeadlineTcp`; STOP, then FINISH, under `IO_TIMEOUT`. | Done |
| Finishing | Mid-frame | Close with no STOP (the frame writer poison rule); nothing is logged. | Done |

Blocks go straight to the transport, so the `BufWriter`, `FLUSH_INTERVAL`, and `dirty`
go. The parser's 100 ms and 1 s partial-block timers still bound the delay.

### Loss accounting on a link failure

A block is "handed to the link" when the router's `try_send` or `send` returns `Ok`.
Each frame in it then ends in exactly one way:

| End | Counted by | Counter |
| --- | --- | --- |
| The kernel accepted all its bytes. | nobody | none: sent, as today. Kernel-buffered bytes (live or replay) are the only data still lost at a failure, and they stay uncounted. |
| Live, cut, not yet accepted, or still in the FIFO. | the router, after the hand-back, as an ordinary block | `spool_dropped` if the spool refuses it; `disconnected_dropped` with no spool or in the Sealing case |
| Live, in the unconfirmed TLS piece. | the router (B2: returned with the cut frame, like the frames after it) | as the other returned frames. Before B2: the link thread counted them in `disconnected_dropped`. |
| Replay, not fully accepted: the cut frame, the unconfirmed TLS frames, the rest of the batch in hand, the batches in the FIFO. | nobody (B2) | none: the spool goes back to the end of the last accepted frame, and the frames are read and sent again. Before B2 the link thread (in hand) or the router (in the FIFO) counted them in `disconnected_dropped`. |

The router also counts the front list. The parser counts only a full channel
(`queue_dropped`). Each block has one owner at a time, so I2 holds. With B2 a spool frame
ends in `replayed` (delivered), in `spool_dropped` (evicted or unreadable, never read), or on
disk; never in two of these ([Eviction](#eviction-and---spool-max-mib)).

### Constants

| Constant | Value | Arithmetic |
| --- | --- | --- |
| `LINK_FIFO_BLOCKS` | 256 (was 128, and 2 before D) | A thread that waits can lose one quantum: 2 clock ticks (31.25 ms) on Windows client, 12 ticks (187.5 ms) on Windows Server. At 41.2 MB/s that is 1.29 MB (20 blocks) and 7.7 MB (118 blocks). The lab measured late wakes of 195 to 216 ms on a busy guest (up to 136 blocks), so 128 blocks left no margin (risk 9). 256 blocks (16 MiB of frames) hold 0.41 s at 41.2 MB/s, which is two Server quanta (236 blocks) with a 8 % margin, and 12 client quanta. They cover the measured late wakes with room for a second one. No hold remains, so this is the only in-memory slack. |
| Loss at a failure | B2: none counted; the kernel-buffered bytes are lost uncounted. Before B2: less than 140 KiB, only in Draining | Before B2: the unsent rest of two replay batches, each at most 71,684 bytes, and with TLS one piece of at most 16 KiB. Live blocks in the FIFO cost memory, not loss: they go back. With B2 the replay batches go back too (to the spool), and so does the unconfirmed TLS piece of a live block. |
| `TLS_FEED_BYTES` | 16 KiB | One TLS record at the default fragment size, so the unconfirmed plaintext is one piece. |
| Link pause held in memory | — | FIFO 16 MiB / R_in: about 8.1 s at 10,000, 1.6 s at 50,000, and 0.41 s at 200,000 queries per second. The kernel send buffer adds more; it is autotuned (as built, H4), so these are lower bounds. |
| Router pause held in memory | — | Parser channel 16 MiB / R_in: 0.41 s at 41.2 MB/s, two Server quanta. B2: a replay read adds about 10 ms to a turn ([Turn length](#credit-batch-size-and-turn-length)). |
| `REPLAY_BATCH_BYTES` | 64 KiB | `read_batch` takes a frame only if it fits the limit, and always takes the first, so a batch is at most max(64 KiB, 70 KiB + 4 bytes) = 71,684 bytes. (Earlier text said that the frame that crosses the limit is taken too, which gave 134 KiB.) It fits `BLOCK_CAPACITY` (136 KiB) with no reallocation. |
| Replay credit | B2: 127 batches. Before B2: 2 (was 1) | V2: credit 2 made replay about 6 times faster on a saturated host, and bounded the replay loss above. With B2 nothing is lost. 127 was `LINK_FIFO_BLOCKS` - `FRONT_LIST_BLOCKS` while the FIFO had 128 blocks. It stays 127 with a FIFO of 256 ([Credit](#credit-batch-size-and-turn-length)). |
| `REPLAY_TURN_BYTES` | B2: 1 MiB | The most the router reads from the spool in one pass. |
| `RETURN_MESSAGES` | `REPLAY_CREDIT` + 2 | 4 before B2, 129 with B2. |
| `SEND_QUEUE_BLOCKS` | 256 (was 128) | User decision, with the risk 9 fix. 16 MiB, 0.41 s at 41.2 MB/s. It holds one Server quantum of router pause (118 blocks) twice over, and the burst of late ETW events (finding L-7), which overflowed the 128-block channel once at 50,000 queries per second. |
| `POOL_BLOCKS` | `SEND_QUEUE_BLOCKS + LINK_FIFO_BLOCKS + FRONT_LIST_BLOCKS + 3` = 516 | One block each for the parser, the router, and the link thread, and the front list (1 block, finding m3). The blocks the router takes out of the FIFO, for a spill or a hand-back, were in the FIFO, and the block the link thread returns was in its hand, so they add none. 516 × 136 KiB = 68.5 MiB (34.5 MiB with both queues at 128), against 17.3 MiB at `a06f6fd`. B2 adds none: a replay batch is a FIFO item. |
| `LINK_POLL` | 50 ms | The readiness and FIFO wait. It bounds the reaction to shutdown, the stall check, the `Fence` wait, and the reconnect poll. |
| Stall timeout | 15 s default | `--receiver-stall-seconds`, 2 to 300. At 50,000 queries per second the spool takes 15 s × 10.3 MB/s = 155 MB, about 4 % of the 4,096 MiB default. |
| `IO_TIMEOUT` | 2 s, unchanged | Connect, TLS handshake, READY/ACCEPT/START, and STOP/FINISH only. Never a data write. |
| `SHUTDOWN_LINK_BUDGET` | 1 s, unchanged | — |

### H2. Liveness from progress

**Rule.** The link thread ends a session only when it has bytes to write and the
receiver accepted none for the stall timeout. A call that accepts at least one byte is
progress; with TLS only `write_tls` counts. An idle link never stalls. The rule uses only
the byte counts of the writer's own calls, never `SIO_TCP_INFO`.

**Mechanism: non-blocking sends with readiness, on the link thread.** After the helper
finishes the handshakes, the link thread calls `set_nonblocking(true)`. On `WouldBlock`
it waits for writability with `select` (or `WSAPoll`) for at most `LINK_POLL`, checks
the stall timer and the shutdown deadline, and writes again. No data write uses
`SO_SNDTIMEO`, so no send times out (R1). `WouldBlock` is a defined result that queued
nothing.

**Rejected: a blocking send with `SO_SNDTIMEO` equal to the stall timeout.** A 15 s send
cannot stop within the 1 s link budget (I5). Closing the socket under it leaves the
queued bytes unknown, so B1 could not split the block.

**Interaction with `IO_TIMEOUT`.** Connect and the handshakes stay blocking on the
`dnstap-reconnect` helper under `IO_TIMEOUT`; a timeout ends an attempt. STOP/FINISH runs
in blocking mode with a fresh `IO_TIMEOUT`; a timeout ends a session that is ending
anyway. The stall timeout is never shorter than `IO_TIMEOUT`.

**Interaction with shutdown.** Once shutdown starts, the link thread stops at the
earlier of the stall deadline and the link deadline. The stall timeout never extends
shutdown. The link's part of the hand-back is one block of memory work before
STOP/FINISH. The router drains the FIFO while STOP/FINISH runs.

| Step (from `QueuedSink::finish`) | Today | This design |
| --- | --- | --- |
| Final partial block, then the link budget | 1.1 s | 1.1 s |
| Write in progress at the deadline, then the hand-back | up to 2 s | up to `LINK_POLL`, 0.05 s |
| STOP/FINISH | up to 2 s | up to 2 s |
| Returned block, FIFO, and channel to the spool, then seal | up to 128 blocks, after the link | up to 513 blocks (about 32 MiB), at the same time as STOP/FINISH. At 100 MB/s that is 0.3 s of writes, an assumption that the lab checks. |
| Total | about 5 s | about 3.2 s, or longer only if the seal takes longer than STOP/FINISH |

**Interaction with the spool.** A shorter stall fills the FIFO and spills: no loss, no
reconnect. With no spool, the loss is `queue_dropped` until the receiver resumes.

## B2. Spool rewind

> **Status:** Built on 2026-11-04, after the user approved the design the same day. The
> sections above describe the code at `7ac0c1c`. Where B2 changes one of them, that section
> says so and keeps the old value. [As built (B2)](#as-built-b2) lists what differs from
> this design.

B1 gives unsent live data back to the router. A replay batch cannot go back, because
`Spool::read_batch` saves the cursor and may delete the segment before the link has sent the
batch (I3). That is why the replay credit is 2: a failure loses at most two batches (under
140 KiB; earlier text said 268 KiB, which assumed a batch could cross the limit). It is
also why replay is slow on a saturated host: each batch needs a router wake (review V3: a
drain of 5.6 to 40.7 MB/s at 2 busy threads per CPU, and 11 to 19 MB/s with a credit of 4).

B2 keeps what the link has not taken. The spool tracks a read position and a delivered
position. At a failure the read position goes back to the delivered position, and the unsent
data is read again. No replay frame is lost, so the credit can be large (127 batches), and one
router wake queues enough batches for the link to run without the router.

| # | Decision | Section |
| --- | --- | --- |
| 1 | Two positions, read (R) and delivered (D). The spool keeps the batches between them. | [Positions](#positions-and-reports) |
| 2 | A segment is deleted when D has passed it, not when R has. | [Segment deletion](#segment-deletion) |
| 3 | A failure sets R back to D. Replay data in the FIFO and in the link's hand is read again. | [Rewind](#rewind-at-the-hand-back) |
| 4 | `cursor.bin` holds D, saved once per router pass. Across a crash the held data is sent again (at least once). A stop repeats nothing. | [Cursor](#cursorbin-and-a-crash) |
| 5 | Held segments count toward the cap. Eviction skips them. | [Eviction](#eviction-and---spool-max-mib) |
| 6 | Credit 127, at most 1 MiB read per router turn, batch size 64 KiB. | [Credit](#credit-batch-size-and-turn-length) |
| 7 | Shutdown leaves the held data on disk, and `seal` saves D. | [Shutdown](#shutdown-and-sealing) |
| 8 | A failure counts no frame, live or replay, except in a spool-less run. | [Loss](#loss-after-b2) |
| 9 | The unconfirmed TLS piece of a live block goes back with the cut frame. | [TLS](#rewind-at-the-hand-back) |

### Positions and reports

| | Read position R | Delivered position D |
| --- | --- | --- |
| Meaning | The end of the last batch the router read. | The end of the last frame whose bytes the session fully handed to the kernel. With TLS, the end of the last frame in a piece that `write_tls` has fully written. |
| Lives in | `Spool`. R is in the first segment that has not `ended` (the reader has not finished it): `read_offset` and `frames_read` of that segment. | `Spool`. D is in the head segment: `done_offset` and `frames_done`. |
| Moves forward | `read_batch`. | `Spool::delivered`, when the router takes a `ReplayDone`. |
| Moves back | `Spool::rewind`, to D. | Never. |

D never passes R. Data between them is **held**: read, not delivered. The spool keeps it on
disk. `delivered`, `rewind`, `checkpoint`, and `has_unread` are methods of `Spool`.

**Spans.** `read_batch` records a span for each batch it returns: the segment sequence, the
start and end offsets, and the frame count. A batch comes from one segment. The spool keeps
the spans in read order, in a queue that grows to at most `REPLAY_CREDIT` entries. The link
finishes batches in FIFO order, and a live block makes no span, so each report belongs to
the front span. The router tracks no positions and no batch numbers.

**Reports.** `Return::ReplayDone` carries `bytes` and `frames`: the prefix of the batch that
the session fully handed to the kernel, ending at a frame boundary.

- A whole batch reports its totals.
- At a failure the link settles the batch in hand with the same message. It reports the
  frames of `Split::sent` and the end offset of the last of them (a new `Split::sent_end`).
  The cut frame, the unconfirmed TLS frames, and every later frame are not counted and not
  returned. They stay in the spool. A live block is settled the same way, except that its
  unsent frames go back as `Returned` (B1). Then the link sends `HandBackDone`.
- A batch in the FIFO that the link never took is released by the router at the drain.

`Spool::delivered(bytes, frames)` pops the front span, sets D to the span start plus `bytes`,
adds `frames` to the `replayed` counter, and sweeps (see below). The spool ignores a report
when no span is out and clamps `bytes` to the span; neither can happen.

`replayed` moves from the read to the delivery. A frame that is read twice (a rewind) would
count twice, and a perfmon counter must not fall. After B2, `replayed` is the number of
spool frames the connection fully accepted.

**Defensive paths.** Two paths cannot run today: `reject` (an item arrives while the link is
down) and a failed `offer` in `replay_step` (the FIFO had room). Both count the batch in
`disconnected_dropped` and then pass it with `Spool::written_off`, which moves D past the batch
without adding to `replayed`. `reject` reports it with `Return::ReplayCounted`. A counted
frame is always one that D has passed, so it is never read again, and it is in one counter.

### Segment deletion

Today `read_batch` deletes a segment when the reader finishes it. After B2 the reader only
marks the segment `ended` and goes on to the next one. A separate sweep deletes.

- **Ended.** The reader ends a segment at STOP, at the end of an unsealed file, at a cut-off
  or corrupt frame, or at the good length of a segment closed after a write error. A rewind
  clears `ended`, because the reader must read the segment again from D. A segment whose
  rest was counted as dropped also keeps a separate mark that a rewind does not clear (see
  [Eviction](#eviction-and---spool-max-mib)).
- **Sweep.** While the head segment has ended and `frames_done == frames_read`, the spool
  deletes it. This is true when the link took everything read from it. The sweep runs at the
  end of `delivered`, and at the end of a `read_batch` that ended a segment, because a
  segment with no frame left to deliver gets no report.
- **The active segment** never ends (more data can arrive). When it is the only segment, R is
  at its writer position, and `frames_done == frames_read`, the sweep deletes it and closes
  the writer. This is the case that the call returning 0 handles today.
- **The cursor.** A deletion writes the cursor only when no span is out, exactly as today
  (the next segment's start, or the next unused sequence). With a span out the cursor already
  holds R, which is ahead, and a deletion writes nothing.

`has_backlog()` stays true while any segment remains. It now means "data not yet delivered".
A new `has_unread()` means "R is before the end of the data". `replay()` tests `has_unread()`:
with the whole backlog held, `has_backlog()` is true but a read would return nothing.
`unread_bytes()` is measured from R; the monitor reads it and nothing in it changes. A
rewind raises it, and the monitor already restarts its window when `disconnects` changes.

**As-built note 1 changes.** The call that returns 0 no longer deletes the last segment.
`delivered` does, when the last `ReplayDone` arrives. The router then leaves Draining at that
`ReplayDone`, not at the next read. Direct still needs no backlog and every credit back.

### Rewind at the hand-back

The router rewinds after it has taken everything the link sent and the FIFO.

1. The link settles the block in hand (a replay batch: `ReplayDone` with the accepted prefix;
   a live block: `Returned`) and sends `HandBackDone`.
2. The router applies each message in channel order. Every `ReplayDone` moves D. Then it
   takes `HandBackDone`, or the close of the channel.
3. `take_fifo`: live items go to `older`. Replay items go back to the pool, give their credit
   back, and are not counted.
4. `Spool::rewind`: the spans are dropped; every segment's `read_offset` and `frames_read`
   are set to its `done_offset` and `frames_done`; `ended` is cleared on every segment (the
   mark for a counted loss is kept); the reader is dropped, and the next read reopens the
   file at D. The cursor file needs no write: it never passes D.
5. `place_older` and the fence, as in B1.

**Proofs.**

- **D has every finished batch.** The link sends `ReplayDone` before `HandBackDone` on one
  channel. The router rewinds after `HandBackDone` or the close, and the channel delivers
  every message before it reports a close. The same holds in `link_vanished` and in the
  Sealing loop.
- **Nothing is sent twice.** The frames before D were fully accepted. A frame at or after D
  was not: a cut frame left the receiver only a truncated copy, which it discards (the B1
  rule). So reading again from D repeats no complete frame. Bytes that the kernel took are
  counted as accepted, so they are before D.
- **Order.** From D on, the spool holds in capture order: the unsent rest of the batch in
  hand, the batches that were in the FIFO, the unread data, and the live blocks that
  Draining appended. The blocks older than all of it (the returned block, live FIFO items,
  the front list) go to the front list when a backlog exists, and to the head of the spool
  when none does. That is B1. In Direct no span is out (as-built note 9), so a failure
  finds D = R and the rewind does nothing.
- **The front list stays at 1 block.** Draining sends no live block to the FIFO, whatever the
  credit. The only live block that can be in the FIFO or in hand is the front block itself.
- **The spill never meets a replay item.** A spill runs only in Direct, where `replay_out` is
  0. It does not rewind.
- **A batch read after `up` cleared** reaches the FIFO and is released at the drain. The
  `up` check in `replay()` stays as an optimization. It is not needed for correctness any
  more.
- **No deadlock.** At most `REPLAY_CREDIT` `ReplayDone`, one `Returned`, and one
  `HandBackDone` are unread, so the return channel holds `REPLAY_CREDIT + 2` (129).

**TLS, live and replay.** The frames of the unconfirmed piece go back with the rest: a replay
batch to the spool, a live block to the router (decided 2026-11-04; B1 counted the piece of a
live block in `disconnected_dropped`). They cannot have reached the receiver whole:

- A piece holds at most 16 KiB, which is one TLS record (`TLS_FEED_BYTES`). The exception is
  the first frame of a piece when it is longer: the piece is then that frame (or its rest)
  alone, in several records.
- `pending()` is true when some ciphertext that `tls_offer` queued is not on the socket.
  `tls_drain` returns an error only while `wants_write()` is true. TCP is in order, so the
  receiver holds a prefix of the pending bytes. When the last of them is the tail of the data
  record, it holds a prefix of that record and cannot decrypt it.
- Every unknown frame ends inside the pending bytes. None is complete at the receiver, with
  one exception.

**The exception.** In TLS 1.3 rustls can queue a `KeyUpdate` (about 22 bytes) after a data
record, at the AES-GCM record limit or on `refresh_traffic_keys()`. If the socket takes the
whole data record and the connection resets while the `KeyUpdate` is still pending, the piece
stays unconfirmed, although the receiver holds it, and its frames arrive twice. The earlier
claim that an error never hides a record that was fully written is false in that window. The
window is the time to write about 22 bytes, once per 2^23 or more records. The service does
not call `refresh_traffic_keys()`. No code changes for it (risk 19).

So the return makes no duplicate except in that window, and the last counted loss at a failure
is gone. The link
tests use a real TLS server on the scripted socket, which decrypts whole records only, so they
check the argument: the receiver holds the frames before the piece and none from it.

### `cursor.bin` and a crash

Decided by the user on 2026-11-04: across a crash or kill, the read-ahead is sent again.
Delivery across a crash is at least once. `cursor.bin` holds the delivered position D, not
the read position. A normal stop repeats nothing.

- **Rule.** `read_batch` does not write the file. `Spool::checkpoint` writes D, at most once
  per router pass, after the pass has applied every `ReplayDone`, and only if D moved. The
  spool also writes it when it deletes the first segment (the start of the next segment, or
  the next unused sequence, as today), and `Spool::seal` writes it. A rewind needs no write:
  the file never passes D, so setting R back to D leaves it valid.
- **What a crash repeats.** The frames the file does not cover: the held data (up to 127
  batches) and the frames delivered since the last write (those of one pass). A batch is at
  most max(64 KiB, 70 KiB + 4 bytes) = 71,684 bytes, because `read_batch` never takes a frame
  that crosses the limit. The held data is 8.0 MiB at 103-byte frames and 8.7 MiB at the
  worst. The receiver can see these frames twice. Nothing else is added: a crash still loses
  the live data in memory (the parser channel and the FIFO, up to 16 MiB) and the bytes the
  kernel held.
- **A stop repeats nothing.** `Spool::seal` writes D, with or without held batches. The next
  run starts at the frame after the last one the link took.
- **Write rate.** One 28-byte positional write per router pass that applied a report, so never
  more than one per batch: at most about 630 a second at 41.2 MB/s and about 8,400 a second at
  the idle replay rate (550 MB/s, review V2), the same bound as the write before each batch
  today. A pass that applies many reports writes once, so a busy host writes less. Add one write
  for each segment deleted (about one per 64 MiB). The file is never flushed.
- **A failed write.** The file keeps the older D, so a crash repeats more frames and never
  fewer. The error is counted in `spool_errors` and logged as any spool I/O error. Reads do
  not pause: replay serves the receiver, and the file only bounds what a crash repeats. Before
  B2 the file had to be written before the send, so a failed write had to stop the read. The
  spool tries again after the pause (10 s), at a deletion at once, and in `seal`.
- **Open.** `Spool::open` applies D as before, and the lag of the file adds two cases that
  need nothing new. A cursor that names a segment that is gone, or that is older than every
  segment, makes reading start at the first segment's START: the segments between were
  delivered, because the file never passes D. A cursor in a segment that exists resumes at its
  offset, and the segments below it are deleted uncounted. A damaged cursor file (wrong size,
  magic, or check value, or an offset inside START) is not a crash. It is unchanged: the
  oldest segment is dropped and counted, which bounds what a damaged file can repeat.
- **A segment that could not be deleted.** The deletion writes the cursor first, so the file
  names a position past the stray file. The next start deletes the stray file and does not
  send it again (the property that `src/spool.rs` documents). If that write fails too, a
  crash can send the stray file again, which at-least-once allows.
- **Rejected: the cursor at R, written before the batch leaves (at most once).** It was the
  earlier proposal. The user chose to resend: a crash then loses no replay data.

### Eviction and `--spool-max-mib`

- **The cap.** Held segments are on disk, so they count in `total_bytes`, `spool_bytes`, and
  the cap.
- **The victim.** The cap path and the free-space floor path both delete the oldest segment
  that has no read byte (R has not entered it). They skip held segments. The floor path
  also keeps the newest segment, as today.
- **Counters.** A victim was never read, so its `frames_read` is 0 and `spool_dropped` takes
  all its frames, as now. A leftover segment adds only bytes. A held frame is never counted
  by eviction. A frame the link took is counted in `replayed`. A frame is in one counter or
  is sent: held segments are not evicted, and an evicted segment was not read.
- **A loss counted once.** A cut-off tail, a corrupt rest, or an unreadable rest is counted
  when the reader first meets it. The segment keeps a mark. After a rewind the reader meets
  the same end and counts nothing. A read error or a bad START on R's segment ends the
  segment at its read offset and counts the unread part at once, as today; the file stays
  until D passes the read part. The mark is not cleared by a rewind.
- **The order guarantee (H3a).** Today the victim is the head segment, which holds the
  oldest unread data. After B2 the victim is the first segment with no read byte, which is
  newer than the held data by at most P + 2S (see the minimum cap). The receiver still gets
  frames in capture order, with a gap after the held run. H3a keeps its meaning: the oldest
  data that is not already on its way to the receiver goes first.
- **The minimum cap.** The largest batch is 71,684 bytes (70 KiB + 4 bytes; `read_batch` takes
  a frame only if it fits the 64 KiB limit, or if it is the first). Held bytes are at most
  P = 127 × 71,684 = 8.7 MiB (8.0 MiB at 103-byte frames).
  Held segments also include the delivered prefix of D's segment and the unread rest of R's,
  each below one segment S = min(64 MiB, cap / 8). So held segments hold at most P + 2S.
  At `--spool-max-mib 64`, S is 8 MiB and held segments hold at most 24.7 MiB. A block that
  needs room arrives when the segments hold more than 63.8 MiB, so at least 39 MiB of
  segments (at least five) are not held, and a victim exists. At any larger cap the held bytes are
  below three quarters of the cap, because P < cap / 4. If no victim existed, the loop would
  end as it does today when no segment is left, and the block would be written over the cap.
  Tests that use a cap below 64 MiB must keep the read-ahead below it.
- **The floor path** can find only held segments to delete. It then refuses blocks for one
  pause (10 s) and counts `spool_dropped`, as it does today when only the newest segment is
  left. This needs a volume that is already at its floor and a spool smaller than P + 2S.

### Credit, batch size, and turn length

| Constant | Value | Arithmetic |
| --- | --- | --- |
| `REPLAY_CREDIT` | 127 (was 2) | It was `LINK_FIFO_BLOCKS - FRONT_LIST_BLOCKS` while the FIFO had 128 blocks. The code needs only that the credit and the front block fit in the FIFO, so that a send that the router has checked never waits (the compile-time assert). With 256 blocks that holds with 128 to spare, and the credit stays 127. The held data and the crash window depend on the credit (P = 127 × 71,684 bytes), so a larger credit would widen them for no gain. To keep replay at the capture rate through a router that runs once per Server quantum (187.5 ms), the link needs 41.2 MB/s × 0.1875 s / 64 KiB = 118 batches. 127 gives a margin of 7.6 % (up to 44.4 MB/s), on the same footing as the FIFO for live data. |
| `REPLAY_BATCH_BYTES` | 64 KiB, unchanged | A larger batch needs a larger `BLOCK_CAPACITY` in all 516 pool blocks (about 32 MiB for each 64 KiB), and it widens the held bytes and the crash window. After B2 the batch size no longer limits the drain, because a wake queues many batches. A smaller batch makes more messages and cursor writes. |
| `REPLAY_TURN_BYTES` | 1 MiB (new) | The most the router reads from the spool in one `serve` call. Replay stops after the first batch that reaches it, so a turn reads under 1 MiB + 70 KiB. See below. |
| `RETURN_MESSAGES` | `REPLAY_CREDIT + 2` = 129 (was 4) | See "No deadlock" above. |
| `LINK_FIFO_BLOCKS` | 256 (was 128) | See the constants above. |
| `POOL_BLOCKS` | 516 (was 260) | See below. |

**Pool, no new blocks.**

- A replay batch is a pool block that `replay_step` takes and offers to the FIFO at once. In
  the FIFO it uses one of the 256 slots. In the link's hand it replaces a slot, because the
  link took it from the FIFO. The router's hand holds one block while it reads.
- In Draining the FIFO holds at most the front block and 127 batches, which is 128 of its 256 slots. These are
  the slots and hands that the `POOL_BLOCKS` term already counts for live blocks.
- A rewind returns blocks to the pool. It allocates none, and it reads again into pool
  blocks.
- The new tests assert `pool_fallbacks == 0`.

**Turn length.**

- `serve` takes the parser blocks first (up to 256), then replay. Replay stops after
  `REPLAY_TURN_BYTES`, and the turn returns `Moved`, so the next turn serves the parser
  channel again.
- The parser channel holds 16 MiB: 0.41 s at 41.2 MB/s. One Server quantum of router pause
  already uses 7.7 MB of it (118 of 256 blocks). The read at the end of a turn must cost a
  small part of the rest. 1 MiB from a disk that is not in the page cache, at 100 MB/s, takes
  10.5 ms. That is 2.6 % of 0.41 s, and the parser adds 0.43 MB (7 blocks) meanwhile. From the
  page cache a read is faster, and this figure is an assumption that lab task L checks.
- One turn queues about 16 batches (103-byte frames). Eight back-to-back turns fill the
  credit. A router wake runs turns while they move something, so one wake queues all 127
  batches. At 41.2 MB/s the link then needs no router for 127 × 64 KiB / 41.2 MB/s = 0.20 s.

### Shutdown and Sealing

At shutdown `serve` reads no replay (as today). Batches out finish within the link budget or
come back. `Router::seal` is unchanged except for what the spool does:

1. Drop the FIFO sender and the fence sender. Apply every message until the return channel
   closes. This moves D for each batch the link finished and settles the batch in hand.
2. `take_fifo`: replay items are released. Nothing is counted.
3. Live blocks older than a backlog and the front list: as today.
4. The blocks left in the parser channel go to the spool. Then `Spool::seal` writes D to the
   cursor file and STOP to the active segment.

The held data stays on disk, in order, and the next start reads from D. The router does not
rewind at a stop: nothing reads the spool again, and the file never holds the read position.
(The first version of this note had a rewind here. It was redundant.) `seal` writes the cursor
file as it writes STOP. A write that fails is counted in `spool_errors` and logged, as a STOP
write that fails always was: a disk fault at the stop is not a receiver problem, and the
rule that a receiver problem adds no output at stop does not cover it. Nothing else is added
at stop. The shutdown bound (about 3.2 s) does not change.

### Loss after B2

| Case | Counted loss before B2 | After B2 |
| --- | --- | --- |
| Failure in Direct | none | none |
| Failure in Draining (replay in the FIFO and in hand) | under 140 KiB, `disconnected_dropped` | none counted. The kernel-buffered bytes are uncounted, as for live data. |
| TLS unconfirmed piece, replay | at most 16 KiB, `disconnected_dropped` | none: the frames are read again |
| TLS unconfirmed piece, live | at most 16 KiB, `disconnected_dropped` | none: returned to the router with the cut frame |
| Shutdown with batches out | counted | none: they stay on disk |
| Shutdown with a backlog and a front block | at most 1 block, `disconnected_dropped` | unchanged |
| Crash or kill (not a stop) | under 140 KiB of replay, uncounted | none lost; up to 127 batches (8.0 MiB, 8.7 MiB at the worst) and the frames of the last pass are sent again, so the receiver can see them twice |

### As built (B2)

What the build does differently from the design above, in the order of the code.

1. **Names.** `Spool::delivered(bytes, frames)`, `Spool::rewind()`, `Spool::checkpoint()`, and
   `Spool::has_unread()`. `Return::ReplayDone` carries `{ bytes, frames }`. `Split` has `sent`,
   `sent_end`, and `unsent`; the `unknown` and `kept` fields are gone, because a live block and a
   replay batch both settle by the confirmed bytes alone. `REPLAY_TURN_BYTES` is in
   `src/sender.rs`. The pool, the FIFO, and the front list are unchanged.
2. **The spans** are a `VecDeque` that grows to at most `REPLAY_CREDIT` entries on the router
   thread. They are not allocated at start, because the spool does not know the credit.
3. **The reader ends a segment in the call that returns its last frames.** The design said
   that the call after that one ends it. A segment with held frames stays until `delivered`
   passes it. A segment with no frames to deliver (a bad START, or nothing but control frames)
   is deleted by the `sweep` that the read runs. The active segment carries an `at_end` flag
   that an append clears.
4. **The mark for a counted loss** is `cut`, the offset where the readable data ends, with
   `lost_frames` and `lost_bytes`. After a rewind the reader stops at `cut` and counts
   nothing. An eviction subtracts `lost_frames` and `lost_bytes`. A read error, a bad START,
   and an invalid cursor use the same mark (`Spool::abandon`), so they delete the segment at
   once when nothing in it is held, as before.
5. **Eviction** (`Spool::evict_oldest(keep_last)`) takes the oldest segment that the reader has
   not ended and whose read offset is not past its delivered offset. The first segment
   qualifies when nothing is held in it, so a spool with nothing held evicts as before.
6. **The cursor.** `Spool::checkpoint` writes D when it moved. The router calls it once per pass
   after `collect_returns` and `follow_link`, and after a hand-back and a vanished link. A deletion
   of the first segment writes the cursor, and `Spool::seal` writes it. After a failed write
   `checkpoint` waits one `PAUSE` (10 s). Reads never pause for it: `read_paused_until` now
   covers only a transient read error. An invalid cursor file is handled as before.
7. **The router** releases a replay item that it finds in the FIFO (`drain_fifo`, which leaves
   the credit to `place_older`) and rewinds the spool in `hand_back` and `link_vanished`. It does
   not rewind in `seal`, because `Spool::seal` saves D and nothing reads the spool again. The `up`
   check in `replay()` stays. The `front.is_empty()` guard in `replay()` is gone, because
   `serve` sends the front list first and the FIFO has room for it and for the whole credit.
   A batch that cannot be sent to the FIFO (it cannot happen) and a replay item that the link
   rejects are counted and passed with `Spool::written_off`, which does not add to `replayed`
   (review V4, m2). The router saves D with `Spool::checkpoint` once per pass, and in
   `hand_back` and `link_vanished`.
8. **Batch size.** `read_batch` takes a frame only if it fits the limit, and always takes the
   first. A batch is therefore at most 71,684 bytes, not 134 KiB. The earlier text, the code
   comments, and the guides had the larger figure; the 268 KiB loss bound was 140 KiB at most.
9. **Test seam.** `Spool::read_and_deliver` (`src/spool/test_support.rs`, tests only) reads a batch
   and reports it delivered at once, for the tests of the spool alone that do not model a link.
10. **The perfcounter manifest** (`packaging/perfcounters/etw-dnstap.man`) changed the help text of
    counters 18 and 54, and `etw-dnstap-perf.rc` was regenerated with CTRPP (two strings changed).
    The updated manifest has to be registered again on upgrade, as the unreleased version
    requires already.
11. **Throughput.** The V2 harness is not in the repository, so a harness was rebuilt from the
    review notes (a temporary test, removed after the run): release profile, loopback, a receiver
    that counts bytes, 103-byte frames on the wire, busy threads at normal priority, 24 logical
    CPUs, Windows 11. Replay drains a backlog of 400,000 frames (41 MB). Live is a paced 41.2 MB/s
    capture for 4 s. Ten runs of each replay case:

    | Case | B2 | Before B2 (`7ac0c1c`) |
    | --- | --- | --- |
    | Replay, idle | 725 to 787 MB/s (median 753) | 600 to 914 MB/s (median 789) |
    | Replay, 1 busy thread per CPU | 320 to 505 MB/s (median 361) | 12.8 to 41.9 MB/s (median 27.4) |
    | Replay, 2 busy threads per CPU | 51 to 368 MB/s (median 73.9) | 4.7 to 9.0 MB/s (median 5.3) |
    | Live, idle / 1 / 2 busy per CPU | 41.2 / 41.0 / 40.7 MB/s, 0 % spooled, 0 dropped | 41.2 / 41.2 / 40.7 MB/s, 0 % spooled; 2,548 frames dropped at 2 busy |

    A first set of 5 runs agreed (B2 at 2 busy per CPU: 56 to 85 MB/s). V3 had measured 5.6 to
    40.7 MB/s before B2 on its host, and the same load gave 4.7 to 9.0 MB/s here.

12. **The TLS record limit (review V4, M1; found there, older than B2).** At the confidentiality
    limit of its cipher (2^24 records for TLS 1.2 with AES-GCM), rustls 0.23.46 queues
    `close_notify`, refuses the record, and still returns the length from `Writer::write`. The
    link then saw every block complete with no socket write: the stall timer never fired, `up`
    stayed true, and frames were discarded silently. Before B2 they were lost; with B2 they would
    also count in `replayed` and be deleted. `tls_offer` (`src/network/wire.rs`) now compares the
    ciphertext that rustls queued for an offer (`process_new_packets().tls_bytes_to_write()`,
    which does no I/O here) with the plaintext it took. Any record is longer than its plaintext
    by a 5-byte header and a 16-byte tag, so less than that means the data was dropped, and the
    offer fails. The data is not confirmed, so the B1 and B2 hand-back returns it, and the
    reconnect has new keys. `rustls` offers no public flag for a queued `close_notify`; the
    check also needs no constant for the record count, which the cipher suite supplies. One
    limit: a refused piece of 10 bytes or fewer is smaller than the 31-byte `close_notify`
    record that takes its place and can pass once; the next offer queues nothing and fails.
    The encoder never produces a frame that small. The tests lower the limit through the cipher
    suite (`TlsKit::with_record_limit`), which is the library's own setting, so the product code
    has no test seam.
13. **Fuzzing.** `src/spool/fuzzing.rs` now ends some batches in a failed link, at points the
    input chooses (at most eight): it delivers half of the frames and rewinds.
14. **Output at stop.** `Spool::seal` writes the cursor file, and a failed write is counted and
    logged like any spool I/O error, as a failed STOP write always was. The rule that nothing is
    added at stop concerns receiver problems; this is a disk fault.

### B2 tests

**New.**

1. **Spool, rewind.** Read three batches across a segment end, deliver the first, rewind.
   The next read starts at the second batch. Frames are in order and none repeats.
2. **Spool, cut frame.** Deliver a prefix that ends inside a batch, rewind. The next read
   starts at the first unaccepted frame. `replayed` counts the accepted frames only.
3. **Spool, deletion waits.** After the last read, the file and `has_backlog` stay.
   `delivered` deletes an ended segment when D passes it, and the active segment when R is
   at its writer position. A segment with no frame to deliver is swept by the read.
4. **Spool, eviction during flight.** Hold three segments, append to the cap. The held
   segments stay, the victim is the first segment with no read byte, `spool_dropped` is the
   victim's frames, and `replayed` + `spool_dropped` + on disk equals appended. At a cap of
   64 MiB with 127 held batches of the largest frames, a victim exists.
5. **Spool, cursor.** The file is not written by a read. It holds D after `checkpoint`.
   Reopen after a crash: the held batch comes again, and the frames before D do not. Reopen
   after `seal`: nothing repeats. A cursor older than every segment starts at the first
   segment. A failed write leaves the older position, and reads go on.
6. **Spool, a loss counted once.** A cut tail and an unreadable segment are counted at the
   first meeting only, across a rewind.
7. **Spool, property.** Extend `src/spool/property_tests.rs` with `Deliver` (all or a
   prefix) and `Rewind` steps among append, read, evict, and reopen. Check conservation
   (appended = `replayed` + `spool_dropped` + on disk), order, no repeat, and the gauges.
   `src/spool/fuzzing.rs` calls `delivered` and rewinds at random points.
8. **Link, replay and live.** A cut replay batch reports the accepted prefix, counts
   nothing, and returns nothing. A TLS replay piece that is unconfirmed is not counted, and
   the report ends at the last confirmed frame. The unconfirmed piece of a live block comes
   back with the frames after it, and nothing is counted. A whole batch reports its
   totals. `Split::sent_end` has its own cases.
9. **Router, one wake.** On the manual scheduler, one `turn` queues 16 batches (103-byte
   frames) and eight turns queue 127. The link then writes all of them with `step`, with
   no router turn between. The router takes 127 `ReplayDone`, and the spool is empty.
10. **Router, return channel.** The link finishes 127 batches while the router does not run.
    The link never blocks on the return channel.
11. **Router, hand-back.** 100 batches in the FIFO and one cut in hand: D moves, the credit
    comes back, `disconnected_dropped` is 0, and the next read starts at the cut frame. The
    front list still goes first.
12. **Router, `link_vanished` and `seal`.** A vanished link rewinds and saves D. `seal` saves D
    and the next run starts there. STOP follows.
13. **Threads, a failure with many batches out.** After the reconnect every frame arrives
    once and in order, and no counter shows a loss. The scripted wire has no kernel buffer.
14. **Threads, TLS replay.** A failure in a replay piece: the frames arrive once.
15. **Threads, shutdown with batches out.** The link is stalled with batches out: after
    `finish`, the segments hold every unsent frame in order, `cursor.bin` equals D, the next
    run replays from D, and nothing is counted.
16. **Spool, crash.** Drop the spool without `seal` after a `checkpoint` and reopen the
    folder: the spool resumes at the checkpoint, and the held and uncheckpointed frames come
    again. A kill leaves the same files. The threads cannot simulate a kill, because `Drop`
    shuts them down in order.
17. **Test 13 (existing proptest, see above).** Its oracle becomes stronger: with a spool
    large enough, delivered frames equal the offered frames, in order and without repeats.
    Add a step that fills the FIFO with replay batches before a failure.
18. **The cursor per pass.** A crash copy of the folder shows what a crash would leave. Reading
    saves nothing, a report alone saves nothing, a pass that applied a report saves D once,
    and the repeat after a crash is at most the batches of the last pass. A hand-back and a
    vanished link save D before they return, without a pass.
19. **TLS record limit.** A TLS 1.2 session whose cipher limit is 8 records (the ring AES-GCM
    suite with `confidentiality_limit` lowered) fails at the limit instead of dropping data:
    every byte that `tls_offer` reports taken reaches the receiver. A live block and a replay
    batch at the limit come back or report only the confirmed frames, and nothing is counted.
    A piece smaller than a `close_notify` record can pass once, and the next offer fails.
20. **Counted batches.** A replay batch that reaches a link with no session, and one that the
    router cannot offer, are counted once and not read again, and `replayed` does not count them.

**Existing tests whose expectation changes because a loss becomes a rewind.**

| File | Test | Change |
| --- | --- | --- |
| `src/sender/router_tests.rs` | `a_replay_batch_in_the_fifo_at_a_hand_back_is_counted_and_gives_its_credit_back` | 0 frames counted; the data is read again. |
| `src/sender/router_tests.rs` | `the_router_reads_two_replay_batches_and_a_third_only_when_the_link_reports_one_done` | Credit 127; at most `REPLAY_TURN_BYTES` per call; `ReplayDone` carries the counts. |
| `src/sender/router_tests.rs` | `live_blocks_go_to_the_spool_until_the_last_replay_credit_is_back` | The segment is deleted at `ReplayDone`, not at the next read. |
| `src/sender/router_tests.rs` | `the_router_reads_no_replay_batch_once_the_link_thread_has_cleared_up` | `replayed` counts at delivery. |
| `src/sender/link_tests.rs` | `a_cut_replay_batch_is_counted_from_the_cut_frame_and_never_returned` | Nothing is counted; the report carries the accepted prefix. |
| `src/sender/link_tests.rs` | `a_tls_piece_whose_delivery_is_unconfirmed_is_counted_and_the_frames_after_it_go_back` | The piece goes back with the frames after it: nothing is counted. |
| `src/sender/fault_tests.rs` | `a_tls_piece_whose_delivery_is_unconfirmed_counts_and_later_frames_arrive_once` | The same, over the threads: all six frames arrive, once. |
| `src/sender/link_tests.rs` | `a_block_splits_into_sent_unknown_and_kept_frames_by_the_bytes_taken` | `Split::sent_end`. |
| `src/sender/link_tests.rs` | `a_live_block_and_a_replay_batch_are_written_whole_and_the_batch_reports_done` | The report carries the totals. |
| `src/sender/fault_tests.rs` | `a_failure_at_the_end_of_a_drain_loses_only_the_rest_of_the_last_replay_batch` | The cut frame is sent again: `lost()` 0, and session 1 starts with the cut frame. |
| `src/sender/saturation_tests.rs` | `a_receiver_that_accepts_nothing_for_the_stall_timeout_loses_the_session_and_only_the_replay_in_flight` | The `<= REPLAY_CREDIT` bound becomes 0. |
| `src/sender/e2e_tests.rs` | `a_link_that_fails_mid_replay_loses_the_failed_batch_and_sends_nothing_twice` | `disconnected_dropped` is 0; only frames in the kernel buffer can be missing. |
| `src/sender/e2e_tests.rs` | `a_run_stopped_in_the_middle_of_a_replay_leaves_a_cursor_the_next_run_resumes_from` | `disconnected_dropped >= 1` becomes 0; the boundary equals the delivered count. |
| `src/spool/spool_tests.rs` | The `drain` helper and the tests that read to the end and expect a deleted segment, a moved cursor, or `replayed` (among them `reading_to_the_writer_position_empties_the_spool_and_moves_the_cursor`, `unread_bytes_fall_as_the_replay_reads_while_the_gauge_waits_for_the_segment`, `a_segment_that_cannot_be_deleted_is_not_sent_twice`, `a_cursor_write_failure_consumes_nothing_and_pauses_reads`, and the sharing-violation tests) | The helper calls `delivered` after each batch. |
| `src/spool/property_tests.rs`, `src/spool/fuzzing.rs` | `read` and the fuzz loop | They call `delivered`; `replayed` is checked against deliveries. |

**Throughput acceptance.** The V3 harness (release, loopback, 103-byte frames) shows a replay
drain of at least 41 MB/s at 2 busy threads per CPU in every run, and a drain at 1 busy
thread per CPU that stays within the 130 to 454 MB/s that review V3 measured. During a
drain, `send_queue_depth_max` rises by no more than the 7 blocks that the turn budget
allows. Lab task L repeats it on Windows Server. The result on the build host is in
[As built (B2)](#as-built-b2), note 11: it meets the first line (51 to 368 MB/s at 2 busy threads
per CPU, 10 of 10 runs).

**Documents that change with the code.** `docs/guide/monitoring.md` (`replayed`,
`disconnected_dropped`), `docs/guide/configuration.md` (Order, Delivery is at most once),
`docs/guide/tuning.md` (spool sizing: the cap includes the held data),
`docs/design/measurement-terms.md`, `docs/design/data-path.md`, the comments in
`src/pipeline/supervisor.rs` and `src/sender.rs`, and `CHANGELOG.md`.

## Tests

### Characterization tests to flip (C1)

| Behavior today | Assertion after task I1 |
| --- | --- |
| F1: a write stall drops frames in `queue_dropped` while the spool has room. | `queue_dropped` is 0. Every frame arrives in capture order after the stall. |
| F2: a stall longer than 2 s disconnects and counts in-flight frames in `disconnected_dropped`. | A stall shorter than the stall timeout: `disconnects`, `reconnects`, and `disconnected_dropped` are 0, and every frame arrives in order. With the timeout set to 2 s it disconnects, the unsent live frames go to the spool, and `disconnected_dropped` counts at most the replay batches the router had out (two, in Draining). B2: 0. |
| F3: a sustained slow receiver keeps every block on the spool path. | Unchanged: gate G2 chose H3a. |

The existing sender, e2e, and backpressure tests must still pass. Under D, tests that
fill the FIFO need 128 blocks, not 2; tests that count a hand-back now see the router
drain the FIFO; and tests that assume a replay credit of 1 change to 2.

### New tests

All fault tests check produced = delivered + `queue_dropped` + `disconnected_dropped` +
`spool_dropped` + still spooled, no complete frame twice at the receiver, and delivered
frames in capture order.

1. **Hand-back without a backlog.** A live block in hand (cut) and 40 in the FIFO: no
   `disconnected_dropped`; the cut rest, the 40, then newer blocks head the spool and
   arrive first after the reconnect.
2. **End of a drain.** Two replay batches out: `disconnected_dropped` equals their unsent
   rest only (less than 268 KiB). B2: 0, and the rest is read again ([B2 tests](#b2-tests)).
3. **Spill drain race.** The link is held at the take lock (test hook) while the router
   spills a full FIFO; then the link fails. Assert: spool order equals capture order;
   the front list holds at most 1 block; after the reconnect it arrives before the
   backlog.
4. **Hand-back races.** (a) A reconnect finishes before the `Fence`: nothing is written
   before it, and `up` turns true only after it. (b) The shutdown deadline arrives during
   a hand-back: `finish` returns in less than 4 s and each frame ends once.
5. **Batching (deterministic).** A scripted link and router run under a manual
   scheduler. The link runs only every T of virtual time; between runs the router queues
   R_in × T. Assert: each link run empties the FIFO, each router run empties the parser
   channel, and no spill happens while R_in × T ≤ the FIFO (as built: 120 blocks per
   link run, about one Server quantum at 41 MB/s, and 234 blocks, about two quanta). With a 2-block FIFO the same script
   must spill (the V2 collapse).
6. **Cut frame resent.** A reset inside frame F: the old receiver discards the
   truncated F; the new session delivers F in full, once.
7. **TLS unconfirmed piece.** A TLS failure during a piece: its frames count in
   `disconnected_dropped`, later frames go back, nothing arrives twice. B2: the piece goes
   back with the later frames, and nothing is counted.
8. **Partial writes.** 1 byte, then `WouldBlock`, in turn; plaintext and TLS arrive intact.
9. **Stall timer.** With a 2 s timeout, 1 byte a second keeps the session; none for 2 s
   ends it.
10. **Shutdown under a stall.** 256 live blocks queued (and one in the link's hand), link
    never writable: `finish` returns in less than 4 s, the blocks are on disk in order,
    `disconnected_dropped` is 0.
11. **STOP rules.** At a block boundary STOP/FINISH runs; mid-frame, no STOP.
12. **Pause shorter than the FIFO.** At 50,000 queries per second, 300 ms with no bytes
    accepted (less than 0.81 s): nothing spooled, no loss, order kept. As built, the test
    pauses for 118 blocks, one Server quantum at 41 MB/s.
13. **Backlog to live.** A `proptest` over stall, failure, and resume sequences keeps order.
    B2 makes the oracle stronger ([B2 tests](#b2-tests)).
14. **No spool, pool, flag.** A full FIFO backs up into `queue_dropped` only; the parser
    never takes the `Block::new` fallback; the flag accepts 2 and 300.

**Throughput acceptance (V2 harness, release, loopback, 103-byte frames, 24 logical
CPUs).** At 1 and at 2 busy threads per CPU, a 41 MB/s paced capture is delivered at
40 MB/s or more, with less than 1 % of frames spooled. At 1 busy thread per CPU, replay
drains faster than 41 MB/s. Lab task L repeats this on Windows Server, where the quantum
is longer.

## Decisions, risks, and open questions

### Decisions

- **Stall timeout.** The user accepted the 15 s default and the name
  `--receiver-stall-seconds` on 2026-11-03. The MSI needs a matching property.
  > **As built.** The MSI has no property for any option except `RECEIVER` and
  > `SESSION_NAME`. Every other option, `--spool-max-mib` included, goes in `SERVICE_ARGS`,
  > which the MSI passes on unchecked. `--receiver-stall-seconds` follows that rule, so the
  > MSI is unchanged. `docs/guide/configuration.md` lists the option.
- **Unsent data at a failure.** User chose B1 on 2026-11-03: unsent live data goes back
  to the router, and only replay data whose cursor moved can be lost. B2 reads that replay
  data again.
- **Parser channel.** It was 128 blocks. The user raised it to 256 on 2026-11-04, with the FIFO (risk 9).
- **Batched hand-offs.** User chose D on 2026-11-03; thread priority rejected (the
  service must not compete with `dns.exe`). Review V2 measured (release, loopback,
  103-byte frames, 24 logical CPUs, 2-block FIFO, credit 1):

  | Load | Live, 41 MB/s capture | Replay drain |
  | --- | --- | --- |
  | Idle | fine | about 550 MB/s |
  | 1 busy thread per CPU | 26-40 MB/s, 93-99 % of frames spooled | 54-347 MB/s |
  | 2 busy threads per CPU | 1.9 MB/s, all spooled | 2.9 MB/s |
  | 2 busy threads per CPU, old single sender (128-block queue) | 40.9 MB/s, 0 spooled | 118 MB/s |

  A FIFO of 8 with credit 1 gave 10 MB/s but collapsed at 41. Credit 2 made replay
  about 6 times faster (16.8 MB/s at 2 busy threads per CPU). Above-normal thread
  priority fixed every case.
- **Saturated link.** User chose H3a on 2026-11-03: strict capture order, with the
  oldest spool data deleted at the cap. With H4, the fixed `SO_SNDBUF` goes, so Windows
  send autotuning stays on. H3b, H3c, and the other shedding options are not built.
- **Spool rewind (B2).** The user approved the design on 2026-11-04 and decided four
  points. (1) Across a crash or kill the read-ahead is sent again: `cursor.bin` holds the
  delivered position, so delivery across a crash is at least once. A stop repeats nothing.
  (2) `replayed` counts a spool frame when the connection accepts it. (3) The unconfirmed
  TLS piece of a live block goes back to the router like the cut frame. (4) While batches
  are in flight, eviction deletes the oldest segment that has not been read; this is still
  H3a.

### Risks and open questions

1. **Half-open receiver.** 15 s delays its detection from 2 s: disk with a spool, up to
   15 s of `queue_dropped` without one.
2. **Disk is the new limit.** `queue_dropped` can still occur if spool appends are slower
   than R_in. Lab task L should measure the append rate.
3. **Counter meaning.** Until task I1, plan task D keeps today's meaning of `write_max_us` and
   `slow_writes`: one blocking socket write. Task I1 redefines them as the readiness wait
   per block, because no send blocks after it.
4. **H4.** Without the fixed `SO_SNDBUF`, the kernel buffer can grow. More bytes then
   count as sent and can be lost unseen at a failure, as the 1 MiB buffer is today.
   > **As built.** I2 removed the `SO_SNDBUF` call, so the loss bound is no longer 1 MiB.
   > Windows grows the buffer with the link (Microsoft Learn: dynamic send buffering is
   > on unless an application sets `SO_SNDBUF`). The unseen loss at a failure grows with
   > the buffer. Tests no longer assume a fixed buffer size.
5. **Trickle receiver.** A receiver that accepts a few bytes before each stall timeout
   keeps the session forever. Liveness does not detect it. Plan detection D4 (link
   saturated) reports it.
6. **Receiver handling of a cut frame.** B1 assumes that a receiver drops a frame cut by
   a closed connection. fstrm readers do. Lab task L should confirm it for the lab
   collector.
7. **rustls API.** The data path relies on rustls 0.23 `writer()`, `write_tls`, and
   `wants_write`, and on the default fragment size. A version change reruns tests 7 and 8.
8. **B2, spool rewind.** Built; see [B2](#b2-spool-rewind). It sets the read
   position back to the delivered position at a failure and defers segment deletion until
   the link has taken the data. Risks 12 to 17 are its costs.
9. **Server quantum at full rate.** On Windows Server at 41 MB/s, a link thread that
   loses a full 187.5 ms quantum needs about 118 blocks of FIFO. With a FIFO of 128 blocks
   (8 MiB) the margin was 9 %, and the lab measured late wakes of 195 to 216 ms on a busy
   guest, which are 123 to 136 blocks, so it was not enough. With a FIFO of 96 it would
   spill about 22 blocks, and the spill is sticky until replay catches up.
   **Fixed on 2026-11-04** without a priority change: `LINK_FIFO_BLOCKS` = 256 (16 MiB, 0.41 s
   at 41.2 MB/s, two Server quanta) and `SEND_QUEUE_BLOCKS` = 256. A late wake of 216 ms
   leaves about 120 blocks of room. Three lost quanta in a row, or a rate above 44.7 MB/s for
   two quanta, still spill. Lab task L repeats risk 9 on the new sizes; until then it is not
   confirmed by a run.
10. **Replay on a saturated host.** At 2 busy threads per CPU, replay stayed below the
    capture rate, so a backlog could grow (H3a then evicts the oldest data). Review V3
    measured a drain of 5.6 to 40.7 MB/s on that load, against 110 to 120 MB/s at `HEAD`.
    A credit of 4 measured only 11 to 19 MB/s. The user accepted this as a known limit
    on 2026-11-03. B2 is the fix: the spool is rewound at a failure, so unsent replay data
    goes back and the credit is 127, about half the FIFO depth since the risk 9 fix. One router wake queues the whole
    credit, and the link needs no router for 0.20 s of replay at 41.2 MB/s. Measured on
    2026-11-04 on a client host with 24 logical CPUs (see [As built (B2)](#as-built-b2)):
    51 to 368 MB/s at 2 busy threads per CPU in 10 of 10 runs, against 4.7 to 9.0 MB/s
    before B2 on the same host. Lab task L repeats it on Windows Server, where the quantum
    is longer; until then the risk is closed on this host only.
11. **Take lock.** If the router is preempted while it holds the take lock, the link
    waits up to one quantum. The lock is held only for queue pops during a spill.
12. **Duplicates after a crash.** Delivery across a crash or kill is at least once (decided
    2026-11-04). The receiver can see up to 127 replay batches twice (8.0 MiB, 8.7 MiB at the
    worst) and the frames of the last router pass. A normal stop repeats nothing. A receiver
    that cannot tolerate a repeat has to deduplicate, as it must for any TCP restart.
13. **Disk in use.** Held data stays on disk until delivered: about 8 MiB, and 8.7 MiB at the
    worst. It stays in whole segments, and eviction spares a segment with any read byte, so
    the bytes that the cap cannot delete reach P + 2S: 24.7 MiB at the minimum cap of 64 MiB
    (39 % of it), and about 137 MiB with 64 MiB segments (caps of 512 MiB and above).
    `spool_bytes` includes them. At the minimum cap an eviction victim always exists (B2,
    "The minimum cap").
14. **Live data waits behind read-ahead.** In Draining a live block goes through the spool and
    then waits behind the replay batches in the FIFO: up to 8 MiB at R_link, which is 0.2 s
    at 41.2 MB/s and 1.7 s at 5 MB/s. Before B2 it waited behind two batches. This is small
    against the backlog delay that Draining already means.
15. **Router turns.** A replay read adds under 1 MiB + 70 KiB to a turn, about 10 ms on a disk
    that reads 100 MB/s. The 100 MB/s figure is an assumption. If the lab shows
    `send_queue_depth_max` rising during a drain, lower `REPLAY_TURN_BYTES`.
16. **Counter meaning.** `replayed` counts at delivery, not at the read (decided 2026-11-04).
    `disconnected_dropped` no longer holds replay frames or the unconfirmed TLS piece, except
    in the two defensive paths and with no spool. `bytes_sent` counts the accepted prefix of a
    cut frame again when the frame is sent again, as it already does for live data. Task W
    updates the guides.
17. **Duplicates at a failure.** None are expected, except in risk 19. D is the end of the last
    frame the kernel fully accepted, and an unconfirmed TLS piece cannot have arrived whole. A fault in this
    argument would show as a frame twice at the receiver; B2 tests 13 to 17 check it.
18. **Eviction victim.** B2 deletes the first segment with no read byte. It is newer than the
    held data by at most P + 2S. Decided as H3a on 2026-11-04.
19. **A duplicate in the TLS 1.3 key update window.** If rustls queues a `KeyUpdate` after a
    data record and the connection resets while it is pending, the receiver holds the record
    and the service sends its frames again. The window is about 22 bytes of writing, once per
    2^23 or more records, and the service never asks for a key update. The chance is
    negligible; no code changes. See the exception in "TLS, live and replay".
