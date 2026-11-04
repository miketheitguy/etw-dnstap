# Slow receiver: link writer and progress liveness

> **Status:** Proposed · **Describes:** the `dnstap-sender` routing and the link in `main` at commit `a06f6fd`, and a change to them (tasks H1 and H2 of the [slow and saturated receiver plan](../plans/slow-receiver-plan.md)) · **Evidence dated:** not applicable (source analysis; no lab run) · **Reviewed:** 2026-11-03

This design keeps the sender thread free of socket waits, and ends a session only when
the receiver makes no progress. On a failure, unsent live data goes back to the router
(decision B1). Hand-offs between threads are batched through a deep FIFO, so the design
keeps its throughput on a CPU-saturated host (decision D). It compares two designs,
recommends one, and gives its state machines, constants, and tests. For a saturated link it keeps today's strict order (H3a, decided
at gate G2).

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
| Threads and ownership | Router owns the spool, the parser channel, and the FIFO sender. Link thread owns the session, the reconnect state, and the FIFO receiver. A block has one owner at a time; unsent live blocks come back by a return channel. | One thread owns everything, as today. |
| Waiting | Router waits with one `crossbeam_channel::Select` (parser channel, return channel). Link thread waits on FIFO readiness or socket readiness. Each drains all it can per wake (D). | The thread must wait for "a block arrived" or "the socket is writable". No call waits on a crossbeam channel and a socket together, so it must alternate short waits. |
| Loss on a link failure | Unsent live frames go back to the router. Only the unsent rest of two replay batches (less than 268 KiB, in Draining) and, with TLS, one unconfirmed piece are lost. | The tail can go back by the same rules; the hand-back is simpler, but the tail stays in the routing loop. |
| How the router learns of a failure | The link thread returns its block in hand and sends `HandBackDone`. The router drains the FIFO itself. | Directly: the write returns the error. |
| Memory | Pool grows by 130 blocks (about 17.3 MiB): the 128-block FIFO, the front list, and a block for the link thread. | No change. |

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
5. Unsent live data returns to the router (B1); only two replay batches are at risk.

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
channel (capacity 4) carries `Returned(Block)`, `ReplayDone`, and `HandBackDone`. Both
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
   the unconfirmed piece counts in `disconnected_dropped`. A replay batch is never
   returned: its unsent rest counts there, then `ReplayDone`.
3. It sends `HandBackDone` (at shutdown it drops the return sender instead). It may
   reconnect, but it neither publishes `up` nor writes until it receives a `Fence`.
4. The router stops sending when it sees `up` false or `HandBackDone`. After
   `HandBackDone` it drains the FIFO with its clone: live blocks follow the returned one;
   a replay batch counts in `disconnected_dropped` and returns its credit. It takes no
   parser block and appends nothing to the spool until step 5.
5. It places the returned block, the drained live blocks, then newer blocks: on the
   spool with no backlog; on the front list (at most 1 block) with a backlog, sent
   first after the reconnect; counted once in `disconnected_dropped` with no spool.
6. It sends a `Fence` (its own channel, As built 6), with the FIFO empty, and enters Down.
   The link may publish `up`.

**Proofs.**

- **No deadlock.** The link has at most 4 unread messages out (2 `ReplayDone`, 1
  `Returned`, 1 `HandBackDone`), and the return channel holds 4. The `Fence` channel
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
| Draining | `up`, backlog | Append to the spool. | Front list first. Then, per free credit (2): `read_batch` into a pool block (64 KiB), `send` it to the FIFO. | A read finds nothing and both credits are back → Direct. `up` false or `HandBackDone` → HandBack. |
| HandBack | The link ended its session | Not taken: blocks wait in the parser channel. | None. | Wait for `HandBackDone`, drain the FIFO, place the blocks (steps 4 to 5), send `Fence` → Down. |
| Down | not `up`, hand-back done | Append to the spool. With no spool, count in `disconnected_dropped`. | None. | `up` true → Direct or Draining. |
| Closing | Shutdown started, within the link budget | As in the states above, but no replay. | None. | Budget ends, or channel closed and empty → Sealing. |
| Sealing | Past the budget, or the channel closed and empty | Drop the FIFO sender. Wait until the return channel closes. Drain the FIFO. Then the returned block, the FIFO blocks, and the channel: to the spool. | None. | `seal`, join the link thread → Done. |

The router is the only FIFO producer, with at most two replay batches out. In Draining
the FIFO holds only those and the front list, so a replay `send` always has a slot and a
batch whose cursor moved reaches the link (I3). With no spool, a full FIFO stops the
router, and the parser counts `queue_dropped`, as today. `SPILL_BLOCKS` goes. In Sealing
with a backlog, live blocks older than it count in `disconnected_dropped`: at most 1
block (134 KiB) by the front-list bound.

As-built notes 1 to 3 were written before decision D and are corrected below for it:
note 2's bound is 1 block, and note 3's hand-back is the one in steps 1 to 6. Notes 6 to
11 record what the build under D does differently from the text above.

> **As built, 1: the end of a drain.** `Spool::read_batch` does not delete the last segment in
> the call that returns the last data. The next call returns 0 and deletes it, and the router
> makes that call only after `ReplayDone` (the replay credit). Live blocks that arrive in
> between go to the spool, in order, and the router reads them in that next call, so Draining
> turns into Direct only at a read that finds nothing. Order is kept, but a state with a replay
> batch in hand and live blocks behind it in the FIFO cannot arise. Test 1 below therefore
> covers the hand-back without a backlog (returned blocks at the head of the spool, before the
> blocks that were in the FIFO) and the end of a drain (only the unsent rest of the last batch is lost) as two tests.
>
> **As built, 2: Sealing with a backlog.** The loss in the paragraph above needs only a spill
> and a shutdown (a stalled link at shutdown), not also a failure. Under D the bound is 1
> block (about 134 KiB), counted once in `disconnected_dropped`: a spill empties the FIFO into
> the spool, so the only live block older than the backlog is the one the link thread held.
> (Before D it was the block in hand and two FIFO blocks, at most 3.) The replay batches that
> the router has out at that moment, at most two, are counted too, as at any failure in
> Draining.
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
> and the hand-back still count a replay batch if they find one.)
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
| The kernel accepted all its bytes. | nobody | none: sent, as today. Kernel-buffered bytes are the only live data still lost at a failure, and they stay uncounted. |
| Live, cut, not yet accepted, or still in the FIFO. | the router, after the hand-back, as an ordinary block | `spool_dropped` if the spool refuses it; `disconnected_dropped` with no spool or in the Sealing case |
| Live, in the unconfirmed TLS piece. | the link thread | `disconnected_dropped` |
| Replay, not fully accepted. | the link thread (in hand) or the router (in the FIFO) | `disconnected_dropped` |

The router also counts the front list. The parser counts only a full channel
(`queue_dropped`). Each block has one owner at a time, so I2 holds.

### Constants

| Constant | Value | Arithmetic |
| --- | --- | --- |
| `LINK_FIFO_BLOCKS` | 128 (was 2) | A thread that waits can lose one quantum: 2 clock ticks (31.25 ms) on Windows client, 12 ticks (187.5 ms) on Windows Server. At 41.2 MB/s that is 1.29 MB (20 blocks) and 7.7 MB (118 blocks). 128 blocks (8 MiB of frames) cover one Server quantum at 41.2 MB/s with a 9 % margin (up to 44.7 MB/s, about 217,000 queries per second) and six client quanta. No hold remains, so this is the only in-memory slack. |
| Loss at a failure | less than 268 KiB, only in Draining | The unsent rest of two replay batches, each below 64 KiB + 70 KiB + 4 bytes. With TLS, add one piece of at most 16 KiB. Live blocks in the FIFO cost memory, not loss: they go back. Today the worst case is 320 KiB. |
| `TLS_FEED_BYTES` | 16 KiB | One TLS record at the default fragment size, so the unconfirmed plaintext is one piece. |
| Link pause held in memory | — | FIFO 8 MiB / R_in: about 4.1 s at 10,000, 0.81 s at 50,000, and 0.20 s at 200,000 queries per second. The kernel send buffer adds more; it is autotuned (as built, H4), so these are lower bounds. |
| Router pause held in memory | — | Parser channel 8 MiB / R_in: 0.20 s at 41.2 MB/s, one Server quantum. |
| `REPLAY_BATCH_BYTES` | 64 KiB | `read_batch` stops after the frame that crosses the limit, so a batch is below 64 KiB + 70 KiB + 4 bytes. It fits `BLOCK_CAPACITY` (136 KiB) with no reallocation. |
| Replay credit | 2 batches (was 1) | V2: credit 2 made replay about 6 times faster on a saturated host. Two batches bound the replay loss above. |
| `SEND_QUEUE_BLOCKS` | 128, unchanged | User decision. |
| `POOL_BLOCKS` | `SEND_QUEUE_BLOCKS + LINK_FIFO_BLOCKS + FRONT_LIST_BLOCKS + 3` = 260 | One block each for the parser, the router, and the link thread, and the front list (1 block, finding m3). The blocks the router takes out of the FIFO, for a spill or a hand-back, were in the FIFO, and the block the link thread returns was in its hand, so they add none. 260 × 136 KiB = 34.5 MiB, against 17.3 MiB at `a06f6fd`. |
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
| Returned block, FIFO, and channel to the spool, then seal | up to 128 blocks, after the link | up to 257 blocks (about 16 MiB), at the same time as STOP/FINISH |
| Total | about 5 s | about 3.2 s, or longer only if the seal takes longer than STOP/FINISH |

**Interaction with the spool.** A shorter stall fills the FIFO and spills: no loss, no
reconnect. With no spool, the loss is `queue_dropped` until the receiver resumes.

## Tests

### Characterization tests to flip (C1)

| Behavior today | Assertion after task I1 |
| --- | --- |
| F1: a write stall drops frames in `queue_dropped` while the spool has room. | `queue_dropped` is 0. Every frame arrives in capture order after the stall. |
| F2: a stall longer than 2 s disconnects and counts in-flight frames in `disconnected_dropped`. | A stall shorter than the stall timeout: `disconnects`, `reconnects`, and `disconnected_dropped` are 0, and every frame arrives in order. With the timeout set to 2 s it disconnects, the unsent live frames go to the spool, and `disconnected_dropped` counts at most the replay batches the router had out (two, in Draining). |
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
   rest only (less than 268 KiB).
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
   channel, and no spill happens while R_in × T ≤ 128 blocks (as built: 120 blocks per
   link run, about one Server quantum at 41 MB/s). With a 2-block FIFO the same script
   must spill (the V2 collapse).
6. **Cut frame resent.** A reset inside frame F: the old receiver discards the
   truncated F; the new session delivers F in full, once.
7. **TLS unconfirmed piece.** A TLS failure during a piece: its frames count in
   `disconnected_dropped`, later frames go back, nothing arrives twice.
8. **Partial writes.** 1 byte, then `WouldBlock`, in turn; plaintext and TLS arrive intact.
9. **Stall timer.** With a 2 s timeout, 1 byte a second keeps the session; none for 2 s
   ends it.
10. **Shutdown under a stall.** 128 live blocks queued (and one in the link's hand), link
    never writable: `finish` returns in less than 4 s, the blocks are on disk in order,
    `disconnected_dropped` is 0.
11. **STOP rules.** At a block boundary STOP/FINISH runs; mid-frame, no STOP.
12. **Pause shorter than the FIFO.** At 50,000 queries per second, 300 ms with no bytes
    accepted (less than 0.81 s): nothing spooled, no loss, order kept. As built, the test
    pauses for 118 blocks, one Server quantum at 41 MB/s.
13. **Backlog to live.** A `proptest` over stall, failure, and resume sequences keeps order.
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
  to the router, and only replay data whose cursor moved can be lost.
- **Parser channel.** It stays at 128 blocks.
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
8. **Future work, B2.** Re-read replay batches after a failure (an in-memory rewind with
   deferred segment deletion), or keep their unsent rest on the front list. Deferred.
9. **Server quantum at full rate.** On Windows Server at 41 MB/s, a link thread that
   loses a full 187.5 ms quantum needs about 118 blocks of FIFO. `LINK_FIFO_BLOCKS` = 128
   (8 MiB) covers it with a 9 % margin. With a FIFO of 96 it would spill about 22 blocks,
   and the spill is sticky until replay catches up. Two lost quanta in a row, or above
   44.7 MB/s, still spill. Lab task L decides.
10. **Replay on a saturated host.** At 2 busy threads per CPU, replay may stay below the
    capture rate, so a backlog can grow (H3a then evicts the oldest data). Review V3
    measured a drain of 5.6 to 40.7 MB/s on that load, against 110 to 120 MB/s at `HEAD`.
    A credit of 4 measured only 11 to 19 MB/s. The user accepted this as a known limit
    on 2026-11-03. The planned fix is B2: rewind the spool on a failure. Unsent replay
    data then goes back to the spool, so the credit can grow to about the FIFO depth
    with no loss.
11. **Take lock.** If the router is preempted while it holds the take lock, the link
    waits up to one quantum. The lock is held only for queue pops during a spill.
