//! The `dnstap-sender` thread: the router. It owns the spool and never waits on
//! the socket.
//!
//! The router takes blocks from the parser channel and sends each to one of three
//! places:
//!
//! - the link FIFO ([`Route::Link`]), while the link takes blocks and the spool
//!   holds no backlog. The FIFO holds [`LINK_FIFO_BLOCKS`] blocks, which is the
//!   in-memory slack that keeps link jitter out of the spool;
//! - the spool ([`Route::Spool`]), while the link is down, while the spool holds a
//!   backlog (so frames stay in order), and when the FIFO is full; or
//! - nowhere ([`Route::Discard`]), counted in `disconnected_dropped`, when the
//!   link is down and there is no spool.
//!
//! **The spill is passive.** Only a send that finds the FIFO full spills. The
//! router then takes the take lock, takes every item out of the FIFO, and appends
//! those blocks to the spool, oldest first, and then the block that did not fit.
//! The link thread takes an item only under the same lock, so it cannot take one
//! in the middle of the spill and write it ahead of an older block that is on its
//! way to the spool. No routing decision reads the state of the socket.
//!
//! While the spool holds a backlog the router replays it: batches of the spool
//! head go into the same FIFO, so replay and live data keep one order. The router
//! has at most [`REPLAY_CREDIT`] batches out at once; the link thread returns a
//! credit with `ReplayDone`.
//!
//! **States.** The router believes the link is up or down (`up`). It follows the
//! shared `up` flag, which only the link thread writes:
//!
//! - *Direct*: `up`, no backlog. Live blocks go to the FIFO.
//! - *Draining*: `up`, a backlog or a replay batch out. Live blocks go to the
//!   spool. The front list goes first, then replay batches. The router leaves
//!   Draining only when a read finds nothing and every credit is back, so the
//!   FIFO holds no replay batch in Direct.
//! - *Hand-back*: the link thread cleared `up`. The router takes nothing from the
//!   parser channel and appends nothing to the spool. It waits for `HandBackDone`,
//!   takes the FIFO, places the blocks, and sends a fence (see
//!   [`Router::place_older`]).
//! - *Down*: blocks go to the spool.
//! - *Sealing*: shutdown. The router closes the FIFO and the fence channel,
//!   collects what comes back, takes the FIFO, sends everything still queued to
//!   the spool, seals it, and joins the link thread.

use std::{
    collections::VecDeque,
    sync::{Arc, PoisonError, atomic::Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime},
};

use anyhow::Result;
use crossbeam_channel::{Receiver, RecvTimeoutError, Select, Sender, TryRecvError, TrySendError};
use crossbeam_queue::ArrayQueue;

use super::{
    Block, FRONT_LIST_BLOCKS, Item, LINK_FIFO_BLOCKS, LinkShared, MAX_IDLE_READS,
    REPLAY_BATCH_BYTES, REPLAY_CREDIT, Return, RouterPorts, SEND_QUEUE_BLOCKS, SENDER_POLL,
    monitor::{LinkMonitor, Observation},
    release,
};
use crate::{
    sink::SinkStats,
    spool::{Appended, Spool},
};

/// Where the router sends a live block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    /// Offer to the link FIFO; a full FIFO spills to the spool.
    Link,
    /// Append to the spool.
    Spool,
    /// Count in `disconnected_dropped`.
    Discard,
}

/// What a replay step did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReplayStep {
    /// A batch went to the FIFO.
    Sent,
    /// Nothing was read; call again at once.
    Again,
    /// Nothing to read now; wait for a block.
    Idle,
}

/// What one pass of the router did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Turn {
    /// Nothing is left to route, or the shutdown link budget ended.
    Done,
    /// Something moved; call again at once.
    Moved,
    /// Nothing moved; wait for something to do.
    Idle,
}

/// What the router believes about the link thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkState {
    /// The link thread takes blocks.
    Up,
    /// The link thread cleared `up` and has handed its block back, or has not
    /// connected yet. Blocks go to the spool.
    Down,
    /// The link thread is gone (it dropped the return channel), so no block goes
    /// to the link again.
    Gone,
}

/// The state of the `dnstap-sender` thread.
pub(super) struct Router {
    spool: Option<Spool>,
    stats: Arc<SinkStats>,
    free: Arc<ArrayQueue<Block>>,
    shared: Arc<LinkShared>,
    /// `None` once the router closed the FIFO at shutdown.
    fifo: Option<Sender<Item>>,
    /// The router's own consumer end of the FIFO (see `RouterPorts`).
    fifo_end: Receiver<Item>,
    /// `None` once the router closed the fence channel: at shutdown, or after the
    /// link thread vanished.
    fences: Option<Sender<()>>,
    returns: Receiver<Return>,
    link: Option<JoinHandle<Result<()>>>,
    /// Live blocks that the link thread gave back and that are older than the
    /// spool backlog: they are sent first after the reconnect, before any replay.
    /// At most [`FRONT_LIST_BLOCKS`].
    front: VecDeque<Block>,
    /// Live blocks the router has taken out of the FIFO, or that the link thread
    /// gave back, oldest first, until it places or spills them. Preallocated for
    /// a full FIFO and the block in hand.
    older: VecDeque<Block>,
    /// Replay batches in the FIFO or in the link thread's hand.
    replay_out: u8,
    /// The link thread's `HandBackDone` has arrived and the router has not yet
    /// acted on it.
    handed_back: bool,
    /// What the router believes about the link thread.
    link_state: LinkState,
    /// The parser channel is closed and empty.
    input_done: bool,
    /// Consecutive replay steps that returned no frames.
    idle_reads: u8,
    /// Observes the link and the spool for the operator. It changes no routing.
    monitor: LinkMonitor,
}

impl Router {
    pub(super) fn new(
        spool: Option<Spool>,
        stats: Arc<SinkStats>,
        free: Arc<ArrayQueue<Block>>,
        shared: Arc<LinkShared>,
        ports: RouterPorts,
        link: JoinHandle<Result<()>>,
    ) -> Self {
        let link_state = if shared.up.load(Ordering::Acquire) {
            LinkState::Up
        } else {
            LinkState::Down
        };
        let monitor = LinkMonitor::new(Arc::clone(&stats), Instant::now());
        Self {
            spool,
            stats,
            free,
            shared,
            fifo: Some(ports.fifo),
            fifo_end: ports.fifo_end,
            fences: Some(ports.fences),
            returns: ports.returns,
            link: Some(link),
            front: VecDeque::with_capacity(FRONT_LIST_BLOCKS + 1),
            older: VecDeque::with_capacity(LINK_FIFO_BLOCKS + FRONT_LIST_BLOCKS + 1),
            replay_out: 0,
            handed_back: false,
            link_state,
            input_done: false,
            idle_reads: 0,
            monitor,
        }
    }

    /// Runs until the parser channel is closed and empty and everything handed to
    /// the link is out of the front list, or the shutdown link budget ends, then
    /// shuts down.
    pub(super) fn run(mut self, parser: &Receiver<Block>) -> Result<()> {
        loop {
            match self.turn(parser) {
                Turn::Done => break,
                Turn::Moved => {}
                Turn::Idle => self.wait(parser),
            }
        }
        self.seal(parser)
    }

    /// One pass: takes the link thread's messages, follows its `up` flag, and
    /// routes everything that is available, without waiting.
    pub(super) fn turn(&mut self, parser: &Receiver<Block>) -> Turn {
        if let Some(spool) = &self.spool {
            // Keeps the `spool_available` flag current.
            let _usable = spool.is_usable();
        }
        let deadline = self.shared.shutdown.link_deadline();
        self.collect_returns();
        self.follow_link();
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) || self.finished() {
            return Turn::Done;
        }
        let progressed = self.serve(parser, deadline.is_some());
        self.sample_monitor();
        #[cfg(test)]
        self.publish_probe();
        if progressed || self.finished() {
            Turn::Moved
        } else {
            Turn::Idle
        }
    }

    /// Nothing is left to route: the parser channel is closed and empty and, if
    /// the link takes blocks, the front list is in the FIFO.
    fn finished(&self) -> bool {
        self.input_done && (!self.is_up() || self.front.is_empty())
    }

    /// Publishes the queue sizes and the replay state for tests to wait on.
    #[cfg(test)]
    fn publish_probe(&self) {
        let probe = &self.shared.probe;
        probe.front.store(self.front.len(), Ordering::Release);
        probe
            .front_max
            .fetch_max(self.front.len(), Ordering::AcqRel);
        probe.fifo.store(self.fifo_end.len(), Ordering::Release);
        probe
            .replay_out
            .store(usize::from(self.replay_out), Ordering::Release);
        probe.turns.fetch_add(1, Ordering::AcqRel);
    }

    const fn is_up(&self) -> bool {
        matches!(self.link_state, LinkState::Up)
    }

    const fn is_gone(&self) -> bool {
        matches!(self.link_state, LinkState::Gone)
    }

    fn has_backlog(&self) -> bool {
        self.spool.as_ref().is_some_and(Spool::has_backlog)
    }

    /// Whether the router may take another block from the parser channel. With no
    /// spool, a full FIFO stops intake: the channel fills, and the parser counts
    /// `queue_dropped`. With a spool, a full FIFO spills instead.
    fn can_take(&self) -> bool {
        !self.is_up() || self.spool.is_some() || self.fifo_has_room()
    }

    fn fifo_has_room(&self) -> bool {
        self.fifo.as_ref().is_some_and(|fifo| !fifo.is_full())
    }

    /// Chooses where a live block goes. With a spool, a block joins it while the
    /// spool holds a backlog or a replay batch is out (frames stay in order, even
    /// if the spool then refuses the block), while the front list holds older
    /// blocks, and while the link is down.
    pub(super) fn destination(&self) -> Route {
        match &self.spool {
            Some(spool)
                if spool.has_backlog()
                    || self.replay_out > 0
                    || !self.front.is_empty()
                    || !self.is_up() =>
            {
                Route::Spool
            }
            None if !self.is_up() => Route::Discard,
            _ => Route::Link,
        }
    }

    /// Follows the link thread's `up` flag. A drop from up to down starts a
    /// hand-back; the link back up is simply believed.
    fn follow_link(&mut self) {
        if self.is_gone() {
            return;
        }
        let up = self.shared.up.load(Ordering::Acquire);
        if self.is_up() && !up {
            self.hand_back();
        } else if !self.is_up() && up {
            self.link_state = LinkState::Up;
        }
    }

    // ---- routing ----

    /// Routes one live block.
    pub(super) fn route_live(&mut self, block: Block) {
        if block.frames == 0 {
            release(&self.free, block);
            return;
        }
        match self.destination() {
            Route::Spool => self.append(block),
            Route::Discard => self.discard(block),
            Route::Link => {
                if let Err(block) = self.offer(Item::Live(block)) {
                    self.fifo_full(block);
                }
            }
        }
    }

    /// Offers an item to the FIFO without waiting. Gives the block back if the
    /// FIFO is full or closed. The router's own receiver keeps the channel open,
    /// so a send never reports that the link thread is gone; the router learns
    /// that from the return channel.
    fn offer(&self, item: Item) -> Result<(), Block> {
        let Some(fifo) = self.fifo.as_ref() else {
            return Err(item.into_block());
        };
        match fifo.try_send(item) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(item) | TrySendError::Disconnected(item)) => {
                Err(item.into_block())
            }
        }
    }

    /// A send found the FIFO full: the spill. With no spool the block is counted;
    /// that cannot happen, because with no spool the router takes a block only
    /// when the FIFO has room, and only the router fills it.
    ///
    /// With a spool: under the take lock, the router takes every item out of the
    /// FIFO. Let x1..xn be those items. The link thread has finished every older
    /// item or holds exactly one, and it cannot take an `xi` while the router
    /// holds the lock, so x1..xn and the new block reach the spool in capture
    /// order. The spool then holds a backlog, and the router is in Draining.
    fn fifo_full(&mut self, block: Block) {
        if self.spool.is_none() {
            self.discard(block);
            return;
        }
        let shared = Arc::clone(&self.shared);
        let lock = shared.take.lock().unwrap_or_else(PoisonError::into_inner);
        // The link thread may have taken items since the send failed. The router
        // is the only producer, so a send under the lock settles whether the FIFO
        // is still full.
        let block = match self.offer(Item::Live(block)) {
            Ok(()) => return,
            Err(block) => block,
        };
        self.drain_fifo();
        drop(lock);
        #[cfg(test)]
        self.shared.probe.spills.fetch_add(1, Ordering::AcqRel);
        while let Some(older) = self.older.pop_front() {
            self.append(older);
        }
        self.append(block);
    }

    /// Takes every item out of the FIFO, oldest first, into [`Self::older`]. The
    /// caller holds the take lock, or knows that the link thread takes nothing:
    /// it has stopped, or it is waiting for the fence.
    ///
    /// A replay batch cannot go back to the spool, because its cursor has moved:
    /// it is counted in `disconnected_dropped`, and its credit comes back.
    fn drain_fifo(&mut self) {
        while let Ok(item) = self.fifo_end.try_recv() {
            match item {
                Item::Live(block) => self.older.push_back(block),
                Item::Replay(block) => {
                    self.replay_out = self.replay_out.saturating_sub(1);
                    self.discard(block);
                }
            }
        }
    }

    /// [`Self::drain_fifo`] under the take lock.
    fn take_fifo(&mut self) {
        let shared = Arc::clone(&self.shared);
        let _lock = shared.take.lock().unwrap_or_else(PoisonError::into_inner);
        self.drain_fifo();
    }

    /// Appends a block to the spool and returns it to the pool. The spool counts
    /// a refused block in `spool_dropped`.
    fn append(&mut self, block: Block) {
        let Some(spool) = self.spool.as_mut() else {
            self.discard(block);
            return;
        };
        match spool.append(&block.bytes, block.frames) {
            Appended::Stored | Appended::Dropped => {}
        }
        release(&self.free, block);
    }

    /// Counts a block that has nowhere to go in `disconnected_dropped`.
    fn discard(&self, block: Block) {
        self.stats
            .disconnected_dropped
            .fetch_add(u64::from(block.frames), Ordering::Relaxed);
        release(&self.free, block);
    }

    // ---- the up and down states ----

    /// One pass of routing and replay: everything the parser channel holds, then
    /// the front list and the replay batches the credit allows. Returns whether
    /// anything moved.
    fn serve(&mut self, parser: &Receiver<Block>, closing: bool) -> bool {
        let mut progressed = self.is_up() && self.send_front();
        // Live data never waits behind replay: everything queued goes to the
        // spool (it has a backlog) before the next batch is read.
        for _ in 0..SEND_QUEUE_BLOCKS {
            if !self.may_route() {
                break;
            }
            match parser.try_recv() {
                Ok(block) => {
                    self.monitor.note_queue_depth(parser.len() + 1);
                    self.route_live(block);
                    progressed = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.input_done = true;
                    break;
                }
            }
        }
        // A batch whose cursor moved must reach a link that is still up: read after
        // the link thread cleared `up`, it would be counted without a write attempt.
        if !closing {
            progressed |= self.replay();
        }
        progressed
    }

    /// Whether the router may route another block now. While the link is up it
    /// needs the FIFO to have room (or a spool to spill to) and the link thread to
    /// still be up. Otherwise it routes until the link thread publishes `up`.
    fn may_route(&self) -> bool {
        if self.is_up() {
            self.can_take() && self.shared.up.load(Ordering::Acquire)
        } else {
            self.is_gone() || !self.shared.up.load(Ordering::Acquire)
        }
    }

    /// Moves the front list into the FIFO. The FIFO was empty at the fence and
    /// holds only the front list and replay batches since, so it has room.
    fn send_front(&mut self) -> bool {
        let mut moved = false;
        while let Some(block) = self.front.pop_front() {
            match self.offer(Item::Live(block)) {
                Ok(()) => moved = true,
                Err(block) => {
                    // Still the oldest.
                    self.front.push_front(block);
                    break;
                }
            }
        }
        moved
    }

    /// Reads replay batches from the spool head and sends them to the FIFO, up to
    /// the credit. Returns whether anything moved.
    fn replay(&mut self) -> bool {
        let mut progressed = false;
        while self.is_up()
            && self.shared.up.load(Ordering::Acquire)
            && self.has_backlog()
            && self.front.is_empty()
            && self.replay_out < REPLAY_CREDIT
            && self.fifo_has_room()
        {
            match self.replay_step() {
                ReplayStep::Sent => progressed = true,
                ReplayStep::Again => {
                    progressed = true;
                    break;
                }
                ReplayStep::Idle => break,
            }
        }
        progressed
    }

    /// Reads one batch from the spool head and sends it to the FIFO. The caller
    /// has checked that the FIFO has room and that a credit is free, so the send
    /// cannot wait, and a batch whose cursor moved always reaches the link thread.
    fn replay_step(&mut self) -> ReplayStep {
        let mut block = self.shared.take_block(&self.free);
        let Some(spool) = self.spool.as_mut() else {
            release(&self.free, block);
            return ReplayStep::Idle;
        };
        let read = spool.read_batch(&mut block.bytes, REPLAY_BATCH_BYTES);
        if let Ok(frames @ 1..) = read {
            self.idle_reads = 0;
            block.frames = frames;
            self.replay_out += 1;
            if let Err(block) = self.offer(Item::Replay(block)) {
                // The FIFO had room, so this does not happen. The spool consumed
                // the batch when it returned it: it is lost, and it is counted.
                self.replay_out -= 1;
                self.discard(block);
            }
            return ReplayStep::Sent;
        }
        release(&self.free, block);
        // Nothing read: an error, a pause, or only control frames skipped. Call
        // again only in the last case, and not forever.
        self.idle_reads = self.idle_reads.saturating_add(1);
        let again = self.spool.as_ref().is_none_or(|current| {
            !current.has_backlog() || (!current.reads_paused() && self.idle_reads < MAX_IDLE_READS)
        });
        if again {
            ReplayStep::Again
        } else {
            ReplayStep::Idle
        }
    }

    /// Waits until a block, a message from the link thread, or room in the FIFO
    /// is ready, for at most [`SENDER_POLL`]. The timeout bounds the delay before
    /// the router notices that `up` changed or that shutdown began.
    fn wait(&self, parser: &Receiver<Block>) {
        if !self.is_gone() && self.shared.up.load(Ordering::Acquire) != self.is_up() {
            return;
        }
        let mut select = Select::new();
        let mut operations = 0;
        if !self.input_done && self.can_take() {
            select.recv(parser);
            operations += 1;
        }
        if !self.is_gone() {
            select.recv(&self.returns);
            operations += 1;
        }
        // Room in the FIFO: the router has no spool and a full FIFO stops intake,
        // or the front list waits.
        if self.is_up()
            && (!self.front.is_empty() || !self.can_take())
            && let Some(fifo) = &self.fifo
        {
            select.send(fifo);
            operations += 1;
        }
        if operations == 0 {
            thread::sleep(SENDER_POLL);
            return;
        }
        let _ = select.ready_timeout(SENDER_POLL);
    }

    // ---- messages from the link thread ----

    /// Takes every message the link thread has sent, without waiting.
    fn collect_returns(&mut self) {
        loop {
            match self.returns.try_recv() {
                Ok(message) => self.take_return(message),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.link_vanished();
                    break;
                }
            }
        }
    }

    /// Applies one message.
    fn take_return(&mut self, message: Return) {
        match message {
            Return::Returned(block) => self.older.push_back(block),
            Return::ReplayDone => self.replay_out = self.replay_out.saturating_sub(1),
            Return::HandBackDone => self.handed_back = true,
        }
    }

    /// Hand-back, router side. The link thread has cleared `up`, has stopped
    /// taking FIFO items, and returns the block it had in hand and then
    /// `HandBackDone`. The router waits for that, takes the FIFO itself (the live
    /// blocks follow the returned one in order), places the blocks, and sends the
    /// fence, which lets the link thread publish `up` again. It takes nothing from
    /// the parser channel and appends nothing to the spool meanwhile, so no newer
    /// block can pass an older one.
    fn hand_back(&mut self) {
        while !self.handed_back {
            #[cfg(test)]
            self.shared
                .probe
                .hand_back_polls
                .fetch_add(1, Ordering::AcqRel);
            match self.returns.recv_timeout(SENDER_POLL) {
                Ok(message) => self.take_return(message),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    // At shutdown the link thread drops the return channel instead
                    // of sending `HandBackDone`.
                    self.link_vanished();
                    return;
                }
            }
        }
        self.handed_back = false;
        self.take_fifo();
        self.link_state = LinkState::Down;
        self.place_older();
        self.send_fence();
        #[cfg(test)]
        self.shared.probe.hand_backs.fetch_add(1, Ordering::AcqRel);
    }

    /// Sends the fence, or at shutdown closes the fence channel instead, which
    /// ends the link thread's wait the same way.
    fn send_fence(&mut self) {
        let past = self
            .shared
            .shutdown
            .link_deadline()
            .is_some_and(|deadline| Instant::now() >= deadline);
        if past {
            self.fences = None;
            return;
        }
        let Some(fences) = self.fences.as_ref() else {
            return;
        };
        match fences.try_send(()) {
            // A full channel already holds a fence that the link has not read.
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => self.fences = None,
        }
    }

    /// The link thread is gone: it dropped the return channel. No block goes to
    /// the link again; what the router holds, and what is in the FIFO, goes to the
    /// spool or is counted. At shutdown this is the normal end of the link.
    fn link_vanished(&mut self) {
        if self.is_gone() {
            return;
        }
        self.link_state = LinkState::Gone;
        self.handed_back = false;
        self.fences = None;
        self.take_fifo();
        self.place_older();
    }

    /// Places the blocks that are older than every block routed after the
    /// failure: the one the link thread gave back, the live blocks that were in
    /// the FIFO, and the front list, in that order.
    ///
    /// - No spool: they are counted in `disconnected_dropped`.
    /// - No backlog: they are appended to the spool in that order, so they are
    ///   its head, before every newer block.
    /// - A backlog (the failure came soon after a spill, or in Draining): the
    ///   blocks are older than the backlog and cannot go behind it. They join the
    ///   front list and are sent first after the reconnect, before any replay. In
    ///   Draining the only live block that can be in hand or in the FIFO is the
    ///   front list's own, and after a spill the only one older than the backlog
    ///   is the one the link thread held, so the list holds at most
    ///   [`FRONT_LIST_BLOCKS`] block.
    fn place_older(&mut self) {
        self.replay_out = 0;
        if self.spool.is_none() {
            while let Some(block) = self.older.pop_front().or_else(|| self.front.pop_front()) {
                self.discard(block);
            }
        } else if self.has_backlog() {
            while let Some(block) = self.older.pop_back() {
                self.front.push_front(block);
            }
            #[cfg(test)]
            self.shared
                .probe
                .front_max
                .fetch_max(self.front.len(), Ordering::AcqRel);
        } else {
            while let Some(block) = self.older.pop_front().or_else(|| self.front.pop_front()) {
                self.append(block);
            }
        }
    }

    // ---- observation and shutdown ----

    /// Takes a monitor sample when one is due. Reads the spool and the link
    /// state only; routing never looks at the result.
    fn sample_monitor(&mut self) {
        let at = Instant::now();
        if !self.monitor.due(at) {
            return;
        }
        let (backlog_bytes, oldest_age, reads_paused) =
            self.spool
                .as_ref()
                .map_or((0, Duration::ZERO, false), |spool| {
                    (
                        spool.unread_bytes(),
                        spool.oldest_age(at, SystemTime::now()),
                        spool.reads_paused(),
                    )
                });
        self.monitor.sample(&Observation {
            at,
            link_up: self.stats.connected.load(Ordering::Relaxed),
            reads_paused,
            backlog_bytes,
            disconnects: self.stats.disconnects.load(Ordering::Relaxed),
            oldest_age,
        });
    }

    /// Shutdown. Closes the FIFO and the fence channel, so the link thread writes
    /// what is left within its budget, hands back what it holds, and ends. Then,
    /// in order, the block it gave back, the FIFO, the front list, and what is
    /// left in the parser channel go to the spool, which is sealed. The link
    /// thread runs STOP/FINISH meanwhile; the result is its result.
    ///
    /// Blocks older than a backlog cannot go in front of it. They are counted in
    /// `disconnected_dropped`: at most [`FRONT_LIST_BLOCKS`] block, because a
    /// backlog comes from a spill, and a spill empties the FIFO.
    fn seal(mut self, parser: &Receiver<Block>) -> Result<()> {
        drop(self.fifo.take());
        drop(self.fences.take());
        while !self.is_gone() {
            match self.returns.recv() {
                Ok(message) => self.take_return(message),
                Err(_) => break,
            }
        }
        // The link thread has ended its session: it takes nothing more.
        self.take_fifo();
        let keep_order = self.spool.is_some() && !self.has_backlog();
        while let Some(block) = self.older.pop_front().or_else(|| self.front.pop_front()) {
            if keep_order {
                self.append(block);
            } else {
                self.discard(block);
            }
        }
        while let Ok(block) = parser.try_recv() {
            self.route_sealed(block);
        }
        if let Some(spool) = self.spool.as_mut() {
            let backlog = spool.has_backlog();
            if let Err(error) = spool.seal() {
                tracing::warn!(%error, "could not seal the dnstap spool segment");
            } else if backlog {
                tracing::info!("dnstap spool backlog kept on disk for the next run");
            }
        }
        match self.link.take() {
            Some(link) => link
                .join()
                .map_err(|_| anyhow::anyhow!("dnstap link thread panicked"))?,
            None => Ok(()),
        }
    }

    /// Routes a block that was still in the parser channel at shutdown.
    fn route_sealed(&mut self, block: Block) {
        if block.frames == 0 {
            release(&self.free, block);
        } else {
            self.append(block);
        }
    }
}

#[cfg(test)]
#[path = "router_tests.rs"]
mod router_tests;
