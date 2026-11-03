//! The queued sink and the `dnstap-sender` thread.
//!
//! [`QueuedSink`] is the [`FrameSink`] the parser thread writes to. It appends
//! each frame to a [`Block`] of pre-framed bytes and hands full blocks to the
//! sender thread through a bounded channel. The parser thread never blocks and,
//! after startup, never allocates: blocks come from a fixed pool, and a block
//! that does not fit in the channel is discarded and counted in
//! `queue_dropped`.
//!
//! The sender thread is the only thread that does socket or spool I/O. It owns
//! the link ([`NetworkSink`]) and the optional [`Spool`]. With a spool, a block
//! goes to the spool while the spool holds a backlog, the link is down, or the
//! channel is nearly full (so frames stay in order and a slow receiver spills to
//! disk), and to the link otherwise. While the link is up it replays the
//! backlog, oldest first. Delivery is at most once: a frame handed to a failed
//! connection is lost and counted in `disconnected_dropped`.

use std::{
    io, mem,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_channel::{
    Receiver, RecvTimeoutError, SendTimeoutError, Sender, TryRecvError, TrySendError, bounded,
};
use crossbeam_queue::ArrayQueue;

use crate::{
    dnstap::MAX_ENCODED_LENGTH,
    network::NetworkSink,
    sink::{FrameSink, SinkStats},
    spool::{Appended, Spool},
};

/// Capacity of a block. A block holds less than [`BLOCK_SEND_BYTES`] before a
/// frame is appended, and a frame adds at most `MAX_ENCODED_LENGTH` plus its
/// 4-byte length, so a frame always fits and the buffer never grows.
const BLOCK_CAPACITY: usize = 136 * 1024;
/// The parser sends a block once it holds at least this many bytes.
const BLOCK_SEND_BYTES: usize = 64 * 1024;
/// Blocks the channel holds.
const SEND_QUEUE_BLOCKS: usize = 128;
/// With a spool, blocks go to it instead of the link while the channel holds at
/// least this many, so a slow receiver spills to disk before the queue fills.
const SPILL_BLOCKS: usize = 96;
/// Blocks in circulation: the channel, the one the sender thread is handling,
/// and the one the parser is filling.
const POOL_BLOCKS: usize = SEND_QUEUE_BLOCKS + 2;
/// The most spool bytes read in one replay step.
const REPLAY_BATCH_BYTES: usize = 256 * 1024;
/// How long the sender thread waits for a block when it has nothing else to do.
/// It also bounds the delay before a reconnect or a paused spool read is retried.
const SENDER_POLL: Duration = Duration::from_millis(50);
/// Data written to the link is flushed at least this often while blocks keep
/// arriving; it is flushed at once when the channel is empty.
const FLUSH_INTERVAL: Duration = Duration::from_millis(100);
/// How long `finish` waits for room in the channel for the last partial block.
const FINAL_SEND_TIMEOUT: Duration = Duration::from_millis(100);
/// Consecutive replay steps that return nothing although reads are not paused
/// (control frames skipped) before the sender waits for a block instead.
const MAX_IDLE_READS: u8 = 3;
/// Once shutdown starts, the sender stops writing to the link after this long and
/// routes the blocks still queued as if the link were down. The worst case from
/// `QueuedSink::finish` to the end of the thread is this budget, plus one link
/// write that started just inside it (the 2 s I/O deadline), plus the
/// STOP/FINISH exchange (another 2 s), plus sealing the spool: about 5 s, inside
/// the 10 s watchdog.
const SHUTDOWN_LINK_BUDGET: Duration = Duration::from_secs(1);
const _: () = assert!(BLOCK_SEND_BYTES + MAX_ENCODED_LENGTH + 4 <= BLOCK_CAPACITY);
const _: () = assert!(SPILL_BLOCKS < SEND_QUEUE_BLOCKS);

/// Complete Frame Streams data frames (a 4-byte big-endian length, then the
/// payload) and how many there are.
#[derive(Default)]
struct Block {
    bytes: Vec<u8>,
    frames: u32,
}

impl Block {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(BLOCK_CAPACITY),
            frames: 0,
        }
    }

    fn clear(&mut self) {
        self.bytes.clear();
        self.frames = 0;
    }
}

/// Returns a block to the pool. The pool holds every block in circulation, so it
/// is never full; a block that did not fit would only be freed.
fn release(free: &ArrayQueue<Block>, mut block: Block) {
    block.clear();
    let _ = free.push(block);
}

/// When shutdown began, shared by the parser end and the sender thread.
struct Shutdown {
    base: Instant,
    /// Nanoseconds from `base` to the start of shutdown, plus one; 0 until it
    /// starts.
    started: AtomicU64,
}

impl Shutdown {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            started: AtomicU64::new(0),
        }
    }

    /// Records the start of shutdown. Only the first call counts.
    fn begin(&self) {
        let nanos = u64::try_from(self.base.elapsed().as_nanos()).unwrap_or(u64::MAX - 1);
        let _ = self
            .started
            .compare_exchange(0, nanos + 1, Ordering::AcqRel, Ordering::Acquire);
    }

    /// The instant the link budget ends, or `None` before shutdown starts.
    fn link_deadline(&self) -> Option<Instant> {
        match self.started.load(Ordering::Acquire) {
            0 => None,
            started => Some(self.base + Duration::from_nanos(started - 1) + SHUTDOWN_LINK_BUDGET),
        }
    }
}

/// The parser-thread end of the send queue.
pub(crate) struct QueuedSink {
    /// The block being filled.
    current: Block,
    /// `None` once `finish` closed the channel.
    queue: Option<Sender<Block>>,
    free: Arc<ArrayQueue<Block>>,
    stats: Arc<SinkStats>,
    shutdown: Arc<Shutdown>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl QueuedSink {
    /// Allocates the block pool and starts the `dnstap-sender` thread, which
    /// takes over the link and the spool. `stats` is the counter set the link and
    /// the spool already write to.
    ///
    /// # Errors
    /// Returns the error if the thread cannot be started.
    pub(crate) fn start(
        link: NetworkSink,
        spool: Option<Spool>,
        stats: Arc<SinkStats>,
    ) -> io::Result<Self> {
        let (queue, receiver) = bounded(SEND_QUEUE_BLOCKS);
        let free = Arc::new(ArrayQueue::new(POOL_BLOCKS));
        for _ in 0..POOL_BLOCKS - 1 {
            release(&free, Block::new());
        }
        let shutdown = Arc::new(Shutdown::new());
        let worker = SenderThread {
            link,
            spool,
            stats: Arc::clone(&stats),
            free: Arc::clone(&free),
            shutdown: Arc::clone(&shutdown),
            replay: Vec::with_capacity(REPLAY_BATCH_BYTES + MAX_ENCODED_LENGTH + 4),
            idle_reads: 0,
            dirty: false,
            last_flush: Instant::now(),
        };
        let thread = thread::Builder::new()
            .name("dnstap-sender".into())
            .spawn(move || worker.run(&receiver))?;
        Ok(Self {
            current: Block::new(),
            queue: Some(queue),
            free,
            stats,
            shutdown,
            thread: Some(thread),
        })
    }

    /// Hands the current block to the sender thread without blocking. A full
    /// queue discards the block's frames and counts them in `queue_dropped`; the
    /// block is then reused.
    ///
    /// If the sender thread has stopped, the block is discarded and an error is
    /// returned. The last `uncounted` frames of the block are not counted then,
    /// because the caller reports them as failed (the parser counts a failed
    /// `write_frame` itself).
    fn send_current(&mut self, uncounted: u32) -> io::Result<()> {
        if self.current.frames == 0 {
            return Ok(());
        }
        let Some(queue) = &self.queue else {
            return Err(io::Error::other("the dnstap send queue is closed"));
        };
        // An empty `Vec` does not allocate, so taking the block is free.
        let block = mem::take(&mut self.current);
        match queue.try_send(block) {
            Ok(()) => {
                // The pool holds a spare while fewer than `POOL_BLOCKS` are in
                // flight, which the queue bound guarantees; allocating here would
                // mean that invariant broke.
                if let Some(spare) = self.free.pop() {
                    self.current = spare;
                } else {
                    self.current = Block::new();
                }
                Ok(())
            }
            Err(TrySendError::Full(mut block)) => {
                self.count_queue_dropped(block.frames);
                block.clear();
                self.current = block;
                Ok(())
            }
            Err(TrySendError::Disconnected(mut block)) => {
                self.count_queue_dropped(block.frames.saturating_sub(uncounted));
                block.clear();
                self.current = block;
                Err(io::Error::other("the dnstap sender thread stopped"))
            }
        }
    }

    fn count_queue_dropped(&self, frames: u32) {
        self.stats
            .queue_dropped
            .fetch_add(u64::from(frames), Ordering::Relaxed);
    }
}

impl FrameSink for QueuedSink {
    /// Appends the frame to the current block and sends the block when it holds
    /// [`BLOCK_SEND_BYTES`]. Makes no allocation, lock, or system call except
    /// the channel send, once per block.
    ///
    /// # Errors
    /// Returns `InvalidInput` for an empty or oversized frame, leaving the block
    /// unchanged, and an error if the sender thread has stopped.
    fn write_frame(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.is_empty() || payload.len() > MAX_ENCODED_LENGTH {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "data frame length must be between 1 and the encoder maximum",
            ));
        }
        let length = u32::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame length exceeds u32"))?;
        self.current.bytes.extend_from_slice(&length.to_be_bytes());
        self.current.bytes.extend_from_slice(payload);
        self.current.frames += 1;
        if self.current.bytes.len() >= BLOCK_SEND_BYTES {
            self.send_current(1)?;
        }
        Ok(())
    }

    /// Sends the partial block. The sender thread flushes the link.
    fn flush(&mut self) -> io::Result<()> {
        self.send_current(0)
    }

    /// Sends the last partial block, closes the channel, and waits for the sender
    /// thread to finish. The thread routes the blocks already queued, seals the
    /// spool segment, and runs the STOP/FINISH exchange if the link is up. It
    /// does not drain a spool backlog: that stays on disk for the next run.
    fn finish(mut self) -> Result<()> {
        let last = mem::take(&mut self.current);
        if last.frames > 0 {
            if let Some(queue) = &self.queue {
                // Waits briefly for room, then counts the frames as dropped.
                match queue.send_timeout(last, FINAL_SEND_TIMEOUT) {
                    Ok(()) => {}
                    Err(
                        SendTimeoutError::Timeout(block) | SendTimeoutError::Disconnected(block),
                    ) => self.count_queue_dropped(block.frames),
                }
            } else {
                self.count_queue_dropped(last.frames);
            }
        }
        self.shutdown.begin();
        drop(self.queue.take());
        match self.thread.take() {
            Some(thread) => thread
                .join()
                .map_err(|_| anyhow::anyhow!("dnstap sender thread panicked"))?,
            None => Ok(()),
        }
    }
}

impl Drop for QueuedSink {
    /// Starts the shutdown clock when the sink is dropped without `finish`, for
    /// example when the parser stops on an error. The channel then closes and the
    /// detached sender thread ends itself within the link budget. Does not join.
    fn drop(&mut self) {
        self.shutdown.begin();
    }
}

/// Where the sender thread sends a block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// Append to the spool.
    Spool,
    /// Write to the link.
    Link,
    /// Count in `disconnected_dropped`.
    Discard,
}

/// The state of the `dnstap-sender` thread.
struct SenderThread {
    link: NetworkSink,
    spool: Option<Spool>,
    stats: Arc<SinkStats>,
    free: Arc<ArrayQueue<Block>>,
    shutdown: Arc<Shutdown>,
    /// Scratch buffer for one replay batch.
    replay: Vec<u8>,
    /// Consecutive replay steps that returned no frames.
    idle_reads: u8,
    /// Data was written to the link since the last flush.
    dirty: bool,
    last_flush: Instant,
}

impl SenderThread {
    /// Runs until the channel is closed and empty, then shuts down.
    fn run(mut self, receiver: &Receiver<Block>) -> Result<()> {
        // False after a replay step found nothing to read; the loop then waits
        // for a block instead of polling the spool in a tight loop.
        let mut replay_progress = true;
        'serve: loop {
            self.link.poll(Instant::now());
            if let Some(spool) = &self.spool {
                // Keeps the `spool_available` flag current.
                let _usable = spool.is_usable();
            }
            let deadline = self.shutdown.link_deadline();
            let shutting_down = deadline.is_some();
            let draining = !shutting_down && self.link_up(deadline) && self.has_backlog();
            let next = if draining && replay_progress {
                match receiver.try_recv() {
                    Ok(block) => Some(block),
                    Err(TryRecvError::Empty) => None,
                    Err(TryRecvError::Disconnected) => break 'serve,
                }
            } else {
                match receiver.recv_timeout(SENDER_POLL) {
                    Ok(block) => Some(block),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break 'serve,
                }
            };
            if let Some(block) = next {
                // The link state may have changed while waiting.
                let link_up = self.link_up(deadline);
                self.route(block, link_up, receiver.len());
            }
            if !shutting_down && self.link_up(deadline) && self.has_backlog() {
                // Live data never waits behind replay: everything queued goes to
                // the spool (it has a backlog) before the next batch is read.
                for _ in 0..SEND_QUEUE_BLOCKS {
                    match receiver.try_recv() {
                        Ok(block) => {
                            let link_up = self.link_up(deadline);
                            self.route(block, link_up, receiver.len());
                        }
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => break 'serve,
                    }
                }
                replay_progress = self.replay_step();
            } else {
                replay_progress = true;
                self.idle_reads = 0;
            }
            // Past the link budget the flush is skipped; `finish` flushes anyway.
            if self.dirty
                && self.link_up(deadline)
                && (receiver.is_empty() || self.last_flush.elapsed() >= FLUSH_INTERVAL)
            {
                self.flush_link();
            }
        }
        self.shut_down()
    }

    /// True when the session is established and, during shutdown, the link
    /// budget has not run out.
    fn link_up(&self, deadline: Option<Instant>) -> bool {
        self.link.is_connected() && deadline.is_none_or(|deadline| Instant::now() < deadline)
    }

    fn has_backlog(&self) -> bool {
        self.spool.as_ref().is_some_and(Spool::has_backlog)
    }

    /// Chooses where a block goes. `queued` is the number of blocks still in the
    /// channel. With a spool, a block joins it while the spool holds a backlog
    /// (frames stay in order, even if the spool then refuses the block), while the
    /// link is down, and while the channel is nearly full (a slow receiver
    /// spills to disk instead of filling the queue).
    fn destination(&self, link_up: bool, queued: usize) -> Route {
        match &self.spool {
            Some(spool) if spool.has_backlog() || !link_up || queued >= SPILL_BLOCKS => {
                Route::Spool
            }
            _ if link_up => Route::Link,
            _ => Route::Discard,
        }
    }

    /// Sends one block to its destination and returns the block to the pool.
    fn route(&mut self, block: Block, link_up: bool, queued: usize) {
        if block.frames > 0 {
            match self.destination(link_up, queued) {
                Route::Spool => {
                    if let Some(spool) = self.spool.as_mut() {
                        match spool.append(&block.bytes, block.frames) {
                            // The spool has counted refused frames in `spool_dropped`.
                            Appended::Stored | Appended::Dropped => {}
                        }
                    }
                }
                Route::Link => {
                    if self.link.write_framed(&block.bytes, block.frames) {
                        self.dirty = true;
                    }
                }
                Route::Discard => {
                    self.stats
                        .disconnected_dropped
                        .fetch_add(u64::from(block.frames), Ordering::Relaxed);
                }
            }
        }
        release(&self.free, block);
    }

    /// Reads one batch from the spool head and writes it to the link. Returns
    /// whether to call again at once: false when the spool has nothing to read
    /// now (reads are paused after an error, which the spool has counted and
    /// logged), so the loop waits for a block first.
    fn replay_step(&mut self) -> bool {
        let Some(spool) = self.spool.as_mut() else {
            return false;
        };
        self.replay.clear();
        let read = spool.read_batch(&mut self.replay, REPLAY_BATCH_BYTES);
        if let Ok(frames @ 1..) = read {
            self.idle_reads = 0;
            // The spool consumed the batch when it returned it; a failed write
            // counts these frames in `disconnected_dropped`.
            if self.link.write_framed(&self.replay, frames) {
                self.dirty = true;
            }
            return true;
        }
        // Nothing read: an error, a pause, or only control frames skipped. Call
        // again only in the last case, and not forever.
        self.idle_reads = self.idle_reads.saturating_add(1);
        self.spool.as_ref().is_none_or(|current| {
            !current.has_backlog() || (!current.reads_paused() && self.idle_reads < MAX_IDLE_READS)
        })
    }

    fn flush_link(&mut self) {
        // The link counts and logs a failed flush itself.
        if let Err(error) = self.link.flush() {
            tracing::debug!(%error, "dnstap link flush failed");
        }
        self.dirty = false;
        self.last_flush = Instant::now();
    }

    /// Seals the spool segment and ends the link session. A backlog stays on disk.
    fn shut_down(mut self) -> Result<()> {
        if let Some(spool) = self.spool.as_mut() {
            let backlog = spool.has_backlog();
            if let Err(error) = spool.seal() {
                tracing::warn!(%error, "could not seal the dnstap spool segment");
            } else if backlog {
                tracing::info!("dnstap spool backlog kept on disk for the next run");
            }
        }
        // Flushes buffered data, then runs STOP/FINISH when connected. When
        // disconnected it logs the shutdown warning.
        self.link.finish()
    }
}

#[cfg(test)]
#[path = "sender/sender_tests.rs"]
mod sender_tests;
