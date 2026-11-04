//! The `dnstap-link` thread: the only thread that touches the socket.
//!
//! It owns the [`Uplink`] and takes [`Item`]s from the router's FIFO, one at a
//! time, as soon as it has finished the one before and without waiting while the
//! FIFO has more. It takes an item only under the take lock, which the router
//! holds to spill the FIFO. A block is written with non-blocking sends. When the
//! socket takes no more, the thread waits for writability for at most
//! [`LINK_POLL`], then checks the stall timer and the shutdown deadline and
//! writes again. No data send has a socket timeout, so no send can end in an
//! indeterminate state.
//!
//! **Liveness (H2).** A session ends only when the link has bytes to write and
//! the socket accepted none for the stall timeout. A write that accepts at least
//! one byte is progress; with TLS only the bytes `write_tls` put on the socket
//! count, and rustls taking plaintext is not progress. An idle link never stalls.
//! The rule reads only the byte counts of the link's own calls.
//!
//! **Hand-back (B1).** When a session ends (a write error, a stall, or the
//! shutdown deadline), the thread clears `up`, stops taking FIFO items, and splits
//! the block in hand by the exact byte counts. The router takes the FIFO itself.
//!
//! - Frames that the socket fully accepted are sent and forgotten.
//! - Live frames that it did not fully accept (the cut frame and every later
//!   one) go back to the router in order, and are sent again on the next session.
//!   The receiver saw only a truncated copy of the cut frame, which it discards.
//! - With TLS, the frames of the piece that was fed to rustls but not confirmed
//!   on the socket go back the same way. The piece is one TLS record (or one long
//!   frame), and the socket has taken only part of its ciphertext, which the
//!   receiver cannot decrypt, so it holds no complete copy of those frames. (One
//!   exception: a TLS 1.3 `KeyUpdate` queued after the record; see
//!   `src/network/wire.rs`.)
//! - A replay batch is not returned: the spool holds it. The thread reports the
//!   frames the socket fully accepted in `ReplayDone`, and the router sets the
//!   spool back to the frame after them. Nothing is counted.
//!
//! Then the thread sends `HandBackDone`. It may reconnect at once, but it neither
//! publishes `up` nor writes until the router's fence arrives: the router sends
//! it after it has emptied the FIFO and placed every older block.

use std::{
    io,
    sync::{Arc, PoisonError, atomic::Ordering, mpsc},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_channel::{Receiver, Select, SendError, Sender, TryRecvError};
use crossbeam_queue::ArrayQueue;

use super::{Block, Item, LINK_POLL, LinkPorts, LinkShared, Return, release};
use crate::{
    network::{SLOW_WRITE, Uplink},
    sink::SinkStats,
};

/// A readiness wait that returns sooner than this while the socket still takes
/// nothing is suspect (a spurious wake); see [`SPURIOUS_WAITS`].
const QUICK_WAIT: Duration = Duration::from_micros(200);
/// After this many quick waits in a row with no progress, the thread sleeps for
/// a millisecond before it waits again, so a poll that reports a socket writable
/// when a send still would block cannot spin a core for the whole stall.
const SPURIOUS_WAITS: u32 = 16;

/// A source of time. Production reads the system clock; tests supply a manual
/// one, so the stall timer needs no real waiting.
pub(super) type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;

/// Test hooks on the link thread.
#[cfg(test)]
#[derive(Clone, Default)]
pub(super) struct LinkHooks {
    /// Runs on the link thread right after a hand-back clears `up`.
    pub(super) after_up_cleared: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Runs on the link thread while it holds the take lock, right after it took
    /// an item.
    pub(super) in_take: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// How the link thread behaves.
pub(crate) struct LinkSettings {
    /// A session whose socket accepts no byte for this long, while the link has
    /// bytes to write, is ended.
    pub(super) stall_timeout: Duration,
    pub(super) clock: Clock,
    #[cfg(test)]
    pub(super) hooks: LinkHooks,
}

impl LinkSettings {
    /// Settings that read the system clock.
    pub(crate) fn new(stall_timeout: Duration) -> Self {
        Self {
            stall_timeout,
            clock: Arc::new(Instant::now),
            #[cfg(test)]
            hooks: LinkHooks::default(),
        }
    }
}

/// What kind of item a block came as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Live,
    Replay,
}

/// How far a block got. `confirmed` bytes are on the socket. `fed_end` bytes
/// were taken by the session; with TLS the bytes between the two sit in rustls
/// and have not reached the socket whole. For plaintext the two are equal. A
/// failure settles the block by `confirmed` alone. `scan` is a frame start at or
/// before `confirmed`, so locating a piece costs little.
#[derive(Clone, Copy, Debug, Default)]
struct Progress {
    confirmed: usize,
    fed_end: usize,
    scan: usize,
}

/// A block the link thread was writing when the session ended.
struct InHand {
    block: Block,
    kind: Kind,
    progress: Progress,
}

enum Outcome {
    /// The socket accepted the whole block.
    Done,
    /// The session failed, or the receiver accepted nothing for the stall
    /// timeout.
    Failed(io::Error),
    /// The shutdown deadline passed.
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum End {
    /// The FIFO closed and is empty, or the router closed the fence channel: the
    /// router has nothing more to send.
    Closed,
    /// The shutdown deadline passed.
    Deadline,
}

/// What one step of the link thread did.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// A block was written or an item rejected, or the fence arrived: step again.
    Worked,
    /// Nothing to do now: park until something changes.
    Idle,
    /// The thread ends.
    End(End),
}

/// The state of the `dnstap-link` thread.
pub(super) struct LinkThread<U: Uplink> {
    uplink: U,
    fifo: Receiver<Item>,
    /// The router's answer to `HandBackDone`.
    fences: Receiver<()>,
    /// `None` once the link has told the router that it is done.
    returns: Option<Sender<Return>>,
    shared: Arc<LinkShared>,
    stats: Arc<SinkStats>,
    free: Arc<ArrayQueue<Block>>,
    settings: LinkSettings,
    /// The shutdown deadline interrupted a block, so no STOP follows. The
    /// receiver was not keeping up: STOP would queue behind the unsent bytes
    /// (Windows non-blocking sends take a whole block or nothing, so the stall is
    /// usually at a block boundary) and wait for the whole I/O timeout, and
    /// inside a frame it would corrupt the stream. The connection closes with no
    /// STOP, and the unsent frames go back to the router.
    cut: bool,
    /// The shutdown hand-back is done.
    handed_back: bool,
    /// A hand-back sent `HandBackDone` and the router's fence has not arrived. The
    /// thread takes nothing from the FIFO and does not publish `up` until it does.
    awaiting_fence: bool,
    /// Told once, when the thread has entered link mode and published `up`, so
    /// that the router starts from a state the link thread has confirmed.
    ready: Option<mpsc::Sender<()>>,
}

impl<U: Uplink> LinkThread<U> {
    pub(super) fn new(
        uplink: U,
        ports: LinkPorts,
        shared: Arc<LinkShared>,
        stats: Arc<SinkStats>,
        free: Arc<ArrayQueue<Block>>,
        settings: LinkSettings,
    ) -> Self {
        Self {
            uplink,
            fifo: ports.fifo,
            fences: ports.fences,
            returns: Some(ports.returns),
            shared,
            stats,
            free,
            settings,
            cut: false,
            handed_back: false,
            awaiting_fence: false,
            ready: None,
        }
    }

    /// Makes the thread signal `ready` when it has published its first state.
    pub(super) fn with_ready(mut self, ready: mpsc::Sender<()>) -> Self {
        self.ready = Some(ready);
        self
    }

    /// Runs until the FIFO closes and empties, or the shutdown deadline passes,
    /// then ends the stream. Returns the result of the closing exchange.
    pub(super) fn run(mut self) -> Result<()> {
        self.begin();
        let end = self.serve();
        self.finish(end)
    }

    /// Enters link mode on the first session, and only then publishes `up`. If
    /// the session cannot enter link mode it is lost here, `up` stays clear, and
    /// the router sends everything to the spool. Then tells `start` that the
    /// first state is published.
    pub(super) fn begin(&mut self) {
        self.uplink.poll(Instant::now());
        if self.uplink.is_connected() {
            self.shared.up.store(true, Ordering::Release);
        }
        if let Some(ready) = self.ready.take() {
            // The receiver is gone if `start` already gave up; nothing to tell.
            let _ = ready.send(());
        }
    }

    /// Ends the stream after [`Self::serve`] returned `end`. Returns the result
    /// of the closing exchange.
    pub(super) fn finish(mut self, end: End) -> Result<()> {
        if matches!(end, End::Deadline) && !self.handed_back {
            self.hand_back(None, None, true);
        }
        // The router stops waiting for returned blocks.
        self.returns = None;
        let clean = !self.cut;
        self.uplink.close(clean)
    }

    fn now(&self) -> Instant {
        (self.settings.clock)()
    }

    fn past_deadline(&self) -> bool {
        self.shared
            .shutdown
            .link_deadline()
            .is_some_and(|deadline| self.now() >= deadline)
    }

    /// Steps and parks until the thread ends.
    pub(super) fn serve(&mut self) -> End {
        loop {
            match self.step() {
                Step::Worked => {}
                Step::Idle => self.park(),
                Step::End(end) => return end,
            }
        }
    }

    /// One step, without waiting: the work that is available. After a block the
    /// next call takes the next FIFO item at once, so the thread waits only when
    /// the FIFO is empty.
    pub(super) fn step(&mut self) -> Step {
        if self.past_deadline() {
            return Step::End(End::Deadline);
        }
        self.uplink.poll(Instant::now());
        if self.awaiting_fence {
            // Nothing is taken or written before the router's fence, so no block
            // can pass an older one that the router is still placing.
            return match self.fences.try_recv() {
                Ok(()) => {
                    self.awaiting_fence = false;
                    Step::Worked
                }
                Err(TryRecvError::Empty) => Step::Idle,
                Err(TryRecvError::Disconnected) => Step::End(End::Closed),
            };
        }
        let connected = self.uplink.is_connected();
        if connected && !self.shared.up.load(Ordering::Relaxed) {
            // The hand-back is finished before this point, so the router has what
            // it needs to take blocks again.
            self.shared.up.store(true, Ordering::Release);
        }
        let item = match self.take() {
            Ok(item) => item,
            Err(TryRecvError::Empty) => return Step::Idle,
            Err(TryRecvError::Disconnected) => return Step::End(End::Closed),
        };
        if !connected {
            self.reject(item);
            return Step::Worked;
        }
        let (block, kind) = match item {
            Item::Live(block) => (block, Kind::Live),
            Item::Replay(block) => (block, Kind::Replay),
        };
        if self.write(block, kind).is_break() {
            return Step::End(End::Deadline);
        }
        Step::Worked
    }

    /// Takes the next FIFO item under the take lock, without waiting.
    fn take(&self) -> Result<Item, TryRecvError> {
        let _lock = self
            .shared
            .take
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let item = self.fifo.try_recv();
        #[cfg(test)]
        if item.is_ok()
            && let Some(hook) = self.settings.hooks.in_take.clone()
        {
            hook();
        }
        item
    }

    /// Waits until there is something to do, for at most [`LINK_POLL`]: an item,
    /// the close of the FIFO, or the fence. Takes nothing.
    fn park(&self) {
        let mut select = Select::new();
        if self.awaiting_fence {
            select.recv(&self.fences);
        } else {
            select.recv(&self.fifo);
        }
        let _ = select.ready_timeout(LINK_POLL);
    }

    /// An item that arrives while the link is down. The router sends only while
    /// `up` is set, so this should not happen; the frames are counted, not lost
    /// silently.
    fn reject(&mut self, item: Item) {
        match item {
            Item::Live(block) => self.drop_block(block),
            Item::Replay(block) => {
                // Counted, and so passed: the spool must not read it again, and
                // must not count it as replayed too.
                self.drop_block(block);
                self.send(Return::ReplayCounted);
            }
        }
    }

    /// Writes one block. `Break` means the shutdown deadline ended the link.
    fn write(&mut self, block: Block, kind: Kind) -> std::ops::ControlFlow<()> {
        let mut progress = Progress::default();
        match self.write_block(&block.bytes, &mut progress) {
            Outcome::Done => {
                self.complete(block, kind);
                std::ops::ControlFlow::Continue(())
            }
            Outcome::Failed(error) => {
                self.hand_back(
                    Some(InHand {
                        block,
                        kind,
                        progress,
                    }),
                    Some(&error),
                    false,
                );
                std::ops::ControlFlow::Continue(())
            }
            Outcome::Interrupted => {
                self.hand_back(
                    Some(InHand {
                        block,
                        kind,
                        progress,
                    }),
                    None,
                    true,
                );
                std::ops::ControlFlow::Break(())
            }
        }
    }

    /// Writes `bytes` to the session until it is all on the socket, the session
    /// fails, the receiver stalls, or the shutdown deadline passes.
    fn write_block(&mut self, bytes: &[u8], progress: &mut Progress) -> Outcome {
        let clock = Arc::clone(&self.settings.clock);
        let stall = self.settings.stall_timeout;
        let stats = &self.stats;
        let shared = &self.shared;
        let Some(wire) = self.uplink.wire() else {
            return Outcome::Failed(io::Error::new(
                io::ErrorKind::NotConnected,
                "no dnstap session",
            ));
        };
        // The stall timer runs only while this block has bytes to write.
        let mut last_progress = clock();
        let mut waited = Duration::ZERO;
        let mut counted_slow = false;
        let mut quick_waits = 0_u32;
        loop {
            let now = clock();
            let deadline = shared.shutdown.link_deadline();
            if deadline.is_some_and(|deadline| now >= deadline) {
                return Outcome::Interrupted;
            }
            let idle = now.saturating_duration_since(last_progress);
            if idle >= stall {
                return Outcome::Failed(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!(
                        "the receiver accepted no bytes for {} ms while data waited",
                        idle.as_millis()
                    ),
                ));
            }
            // Bytes the socket accepted count as soon as a call reports them, so a
            // later call in the same pass that fails does not lose them.
            let mut wrote = 0;
            let mut took = 0;
            if wire.pending() {
                match wire.drain() {
                    Ok(moved) => {
                        wrote += moved.written;
                        stats
                            .bytes_sent
                            .fetch_add(moved.written as u64, Ordering::Relaxed);
                    }
                    Err(error) => return Outcome::Failed(error),
                }
                if !wire.pending() {
                    progress.confirmed = progress.fed_end;
                }
            }
            if !wire.pending() {
                if progress.confirmed >= bytes.len() {
                    return Outcome::Done;
                }
                let end = piece_end(
                    bytes,
                    &mut progress.scan,
                    progress.confirmed,
                    wire.piece_limit(),
                );
                match wire.offer(&bytes[progress.confirmed..end]) {
                    Ok(moved) => {
                        wrote += moved.written;
                        stats
                            .bytes_sent
                            .fetch_add(moved.written as u64, Ordering::Relaxed);
                        took = moved.taken;
                        progress.fed_end = progress.confirmed + moved.taken;
                        if !wire.pending() {
                            progress.confirmed = progress.fed_end;
                        }
                    }
                    Err(error) => return Outcome::Failed(error),
                }
            }
            if wrote > 0 {
                last_progress = clock();
                quick_waits = 0;
            }
            if wrote > 0 || took > 0 {
                continue;
            }
            // The socket takes nothing now: wait for it, but not past the stall
            // timeout or the shutdown deadline.
            let mut wait = LINK_POLL.min(stall.saturating_sub(idle));
            if let Some(deadline) = deadline {
                wait = wait.min(deadline.saturating_duration_since(now));
            }
            let started = clock();
            if let Err(error) = wire.wait_writable(wait) {
                return Outcome::Failed(error);
            }
            let mut spent = clock().saturating_duration_since(started);
            if spent < QUICK_WAIT {
                quick_waits += 1;
                if quick_waits >= SPURIOUS_WAITS {
                    thread::sleep(Duration::from_millis(1));
                    spent += Duration::from_millis(1);
                }
            } else {
                quick_waits = 0;
            }
            waited += spent;
            record_wait(stats, waited, &mut counted_slow);
        }
    }

    /// A block the socket fully accepted.
    fn complete(&mut self, block: Block, kind: Kind) {
        if kind == Kind::Replay {
            self.send(Return::ReplayDone {
                bytes: block.bytes.len(),
                frames: block.frames,
            });
        }
        release(&self.free, block);
    }

    /// Ends a session: clears `up`, ends the session after `error`, and settles
    /// the block in hand. Then it tells the router with `HandBackDone` and waits
    /// for the fence, or, at shutdown, drops the return channel instead. It takes
    /// nothing from the FIFO: the router empties it.
    fn hand_back(&mut self, in_hand: Option<InHand>, error: Option<&io::Error>, ending: bool) {
        self.shared.up.store(false, Ordering::Release);
        #[cfg(test)]
        if let Some(hook) = self.settings.hooks.after_up_cleared.clone() {
            hook();
        }
        if let Some(error) = error {
            self.uplink.lost(error);
        }
        if let Some(hand) = in_hand {
            self.settle(hand, ending);
        }
        if ending {
            self.handed_back = true;
            self.returns = None;
        } else {
            self.awaiting_fence = true;
            self.send(Return::HandBackDone);
        }
    }

    /// Splits the block in hand by what the session took, and sends back, counts,
    /// or forgets each frame.
    fn settle(&mut self, hand: InHand, ending: bool) {
        let InHand {
            mut block,
            kind,
            progress,
        } = hand;
        let split = split_block(&block.bytes, progress.confirmed);
        // `ending` means the shutdown deadline interrupted this block: close with
        // no STOP (see `cut`).
        self.cut = ending;
        match kind {
            Kind::Replay => {
                // The spool holds the batch. It learns how much the socket took
                // and sets its read position back to the frame after that.
                release(&self.free, block);
                self.send(Return::ReplayDone {
                    bytes: split.sent_end,
                    frames: split.sent,
                });
            }
            Kind::Live => {
                if split.unsent == 0 {
                    release(&self.free, block);
                    return;
                }
                // The cut frame and every later one move to the start of the same
                // buffer; nothing is allocated.
                block.bytes.copy_within(split.sent_end.., 0);
                let kept_bytes = block.bytes.len() - split.sent_end;
                block.bytes.truncate(kept_bytes);
                block.frames = split.unsent;
                self.send(Return::Returned(block));
            }
        }
    }

    /// Counts every frame of a block that will not be sent, and frees the block.
    fn drop_block(&self, block: Block) {
        if block.frames > 0 {
            self.stats
                .disconnected_dropped
                .fetch_add(u64::from(block.frames), Ordering::Relaxed);
        }
        release(&self.free, block);
    }

    fn send(&mut self, message: Return) {
        let Some(returns) = &self.returns else {
            self.discard(message);
            return;
        };
        if let Err(SendError(message)) = returns.send(message) {
            // The router has stopped: nothing is waiting for the blocks.
            self.returns = None;
            self.discard(message);
        }
    }

    fn discard(&self, message: Return) {
        if let Return::Returned(block) = message {
            self.drop_block(block);
        }
    }
}

/// Records the readiness wait of the block being written: `waited` is the total
/// so far. The statistics keep the longest wait of any block and count a block
/// once when its wait reaches [`SLOW_WRITE`].
fn record_wait(stats: &SinkStats, waited: Duration, counted_slow: &mut bool) {
    let micros = u64::try_from(waited.as_micros()).unwrap_or(u64::MAX);
    stats.write_max_us.fetch_max(micros, Ordering::Relaxed);
    stats
        .write_window_max_us
        .fetch_max(micros, Ordering::Relaxed);
    if !*counted_slow && waited >= SLOW_WRITE {
        *counted_slow = true;
        stats.slow_writes.fetch_add(1, Ordering::Relaxed);
    }
}

/// The end of the frame that starts at `start` in `bytes` (4-byte big-endian
/// length, then the payload). A header or payload that does not fit ends at the
/// end of `bytes`, so a malformed tail is one frame and a walk always advances.
fn frame_end(bytes: &[u8], start: usize) -> usize {
    let Some(header) = bytes.get(start..).and_then(|rest| rest.first_chunk::<4>()) else {
        return bytes.len();
    };
    usize::try_from(u32::from_be_bytes(*header))
        .ok()
        .and_then(|length| start.checked_add(4)?.checked_add(length))
        .map_or(bytes.len(), |end| end.min(bytes.len()))
}

/// The end of the next piece to feed the session, which starts at `from`
/// (`from < bytes.len()`). Without a `limit` it is the end of the block. With
/// one, the piece ends at a frame boundary and holds at most `limit` bytes,
/// unless its first frame (the frame that holds `from`) is longer. `scan` is a
/// frame start at or before `from`, and is moved up to the frame that holds it.
fn piece_end(bytes: &[u8], scan: &mut usize, from: usize, limit: Option<usize>) -> usize {
    let Some(limit) = limit else {
        return bytes.len();
    };
    let mut start = *scan;
    let mut end = frame_end(bytes, start);
    while end <= from && end < bytes.len() {
        start = end;
        end = frame_end(bytes, start);
    }
    *scan = start;
    while end < bytes.len() {
        let next = frame_end(bytes, end);
        if next - from > limit {
            break;
        }
        end = next;
    }
    end
}

/// How a block divides at a failure, by frame.
#[derive(Debug, PartialEq, Eq)]
struct Split {
    /// Frames the socket fully accepted.
    sent: u32,
    /// The offset after the last of them, or 0 if none: the first frame that
    /// goes back starts here, and the block length is the end of a block that was
    /// sent whole.
    sent_end: usize,
    /// Frames after them: the cut frame, the frames a TLS session took whole but
    /// the socket did not confirm, and every later frame. The receiver cannot
    /// have a complete copy of any of them.
    unsent: u32,
}

/// Divides `bytes` by frame at a failure: a frame that ends at or before
/// `confirmed` was sent, and every other frame was not, so the receiver cannot
/// have a complete copy of it.
fn split_block(bytes: &[u8], confirmed: usize) -> Split {
    let mut split = Split {
        sent: 0,
        sent_end: 0,
        unsent: 0,
    };
    let mut start = 0;
    while start < bytes.len() {
        let end = frame_end(bytes, start);
        if end <= confirmed {
            split.sent += 1;
            split.sent_end = end;
        } else {
            split.unsent += 1;
        }
        start = end;
    }
    split
}

#[cfg(test)]
#[path = "link_tests.rs"]
mod link_tests;
