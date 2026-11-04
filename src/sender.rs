//! The queued sink and the threads behind it.
//!
//! [`QueuedSink`] is the [`FrameSink`] the parser thread writes to. It appends
//! each frame to a [`Block`] of pre-framed bytes and hands full blocks to the
//! router thread through a bounded channel. The parser thread never blocks and,
//! after startup, never allocates: blocks come from a fixed pool, and a block
//! that does not fit in the channel is discarded and counted in
//! `queue_dropped`.
//!
//! Two threads sit behind the channel, so that no socket wait ever stops the
//! spill to disk:
//!
//! - The **router** (`dnstap-sender`, [`router`]) owns the optional [`Spool`] and
//!   never waits on the socket. It sends blocks to a FIFO of
//!   [`LINK_FIFO_BLOCKS`] blocks, which is the in-memory slack that absorbs a
//!   slow link or a link thread that the scheduler has not run for a while. Only a
//!   full FIFO spills to the spool: the router takes the blocks out of the FIFO and
//!   appends them to the spool, oldest first. Once the spool holds a backlog,
//!   every block goes to it until the router has replayed the backlog, so frames
//!   stay in order.
//! - The **link thread** (`dnstap-link`, [`link`]) owns the [`NetworkSink`]. It
//!   writes with non-blocking sends and a readiness wait, ends the session only
//!   when the receiver accepts no bytes for the stall timeout, and on any failure
//!   hands the unsent live data back to the router.
//!
//! Each thread moves everything that is available each time it runs: the link
//! thread takes the next block as soon as it finishes one, and the router routes
//! every block in the parser channel before it waits.
//!
//! A frame the socket fully accepted is never resent by a running service. A
//! frame the socket did not fully accept goes back and is sent again on the
//! next session; the receiver saw at most a truncated copy of it. A live frame
//! goes back to the router. A replay frame goes back to the spool: the router
//! holds each batch it reads until the link reports it, and a failure or a
//! shutdown rewinds the spool to the last frame the link took. Nothing is lost
//! at a failure except what the kernel already held. Across a crash the held
//! replay data is sent again.
//! See `docs/design/slow-receiver.md`.

use std::{
    io, mem,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_channel::{Receiver, SendTimeoutError, Sender, TrySendError, bounded};
use crossbeam_queue::ArrayQueue;

use self::{link::LinkThread, router::Router};
use crate::{
    dnstap::MAX_ENCODED_LENGTH,
    network::{NetworkSink, Uplink},
    sink::{FrameSink, SinkStats},
    spool::Spool,
};

/// Capacity of a block. A block holds less than [`BLOCK_SEND_BYTES`] before a
/// frame is appended, and a frame adds at most `MAX_ENCODED_LENGTH` plus its
/// 4-byte length, so a frame always fits and the buffer never grows. A replay
/// batch is at most the larger of [`REPLAY_BATCH_BYTES`] and one frame, so it fits too.
const BLOCK_CAPACITY: usize = 136 * 1024;
/// The parser sends a block once it holds at least this many bytes.
const BLOCK_SEND_BYTES: usize = 64 * 1024;
/// Blocks the parser channel holds (16 MiB, 0.41 s at 41.2 MB/s). A router that
/// loses a Windows Server quantum (187.5 ms) leaves 118 blocks in it. A burst of
/// late ETW events can arrive at once, and the channel holds it.
const SEND_QUEUE_BLOCKS: usize = 256;
/// Blocks the link FIFO holds (16 MiB of frames). It is the only in-memory slack
/// between the router and the socket, and the router spills to the spool only
/// when it is full. A thread that waits for work can lose a scheduler quantum on a
/// busy host: 12 clock ticks (187.5 ms) on Windows Server, which is about 118
/// blocks at 41.2 MB/s. The lab saw late wakes of 195 to 216 ms (up to 136
/// blocks). The FIFO holds 256 blocks, 0.41 s at 41.2 MB/s, which is two
/// quanta, without a priority boost. With the replay credit, at most
/// [`REPLAY_CREDIT`] of its blocks are replay batches.
const LINK_FIFO_BLOCKS: usize = 256;
/// Replay batches the router can have out at once, in the FIFO or in the link
/// thread's hand. The spool holds them until the link reports them, so a failure
/// loses none. It is not the whole room in the FIFO: 127 batches are 0.20 s of
/// replay at 41.2 MB/s, which is more than the 118 that one Server quantum of
/// router pause needs, so one router wake queues enough for the link to run
/// without the router. A larger credit would only raise the data that a crash
/// repeats and the bytes that a failure reads again.
const REPLAY_CREDIT: u8 = 127;
/// Live blocks the router can keep for the next session after a failure while
/// the spool holds a backlog: the one block that the link thread had in hand.
/// Every other live block is in the spool already, or is not older than it.
const FRONT_LIST_BLOCKS: usize = 1;
/// Blocks in circulation: the parser channel, the link FIFO, the front list, and
/// one block each in the hands of the parser, the router, and the link thread.
/// The blocks the router takes out of the FIFO to spill it, or at a hand-back,
/// were in the FIFO, and the block that came back from the link thread was in its
/// hand, so they add none. 516 blocks of 136 KiB are 68.5 MiB.
const POOL_BLOCKS: usize = SEND_QUEUE_BLOCKS + LINK_FIFO_BLOCKS + FRONT_LIST_BLOCKS + 3;
/// The most spool bytes read in one replay step. A batch takes a frame only if it
/// fits the limit, and always takes the first, so it is at most the larger of
/// this and one maximum-length frame (71,684 bytes). The router has at most
/// [`REPLAY_CREDIT`] batches out.
const REPLAY_BATCH_BYTES: usize = 64 * 1024;
/// The most the router reads from the spool in one pass, before it serves the
/// parser channel again. The channel holds 16 MiB, 0.41 s at 41.2 MB/s; a read of
/// 1 MiB from a disk that is not cached, at 100 MB/s, takes 10.5 ms of that.
const REPLAY_TURN_BYTES: usize = 1024 * 1024;
/// How long the router waits for something to do. It bounds the delay before the
/// router notices that the link went down, that a paused spool read can be
/// retried, and that shutdown began.
const SENDER_POLL: Duration = Duration::from_millis(50);
/// The longest the link thread waits for a block, or for the socket to become
/// writable, before it checks the stall timer, the shutdown deadline, and the
/// reconnect. It is also how often a down link is polled for a reconnect.
const LINK_POLL: Duration = Duration::from_millis(50);
/// Messages the link thread can have waiting for the router: a `ReplayDone` for
/// each replay credit, the live block in hand at a failure, and `HandBackDone`.
/// The link thread never waits for room.
const RETURN_MESSAGES: usize = REPLAY_CREDIT as usize + 2;
/// How long `finish` waits for room in the channel for the last partial block.
const FINAL_SEND_TIMEOUT: Duration = Duration::from_millis(100);
/// Consecutive replay steps that return nothing although reads are not paused
/// (control frames skipped) before the router waits for a block instead.
const MAX_IDLE_READS: u8 = 3;
/// Once shutdown starts, the link thread stops writing after this long and hands
/// the unsent blocks back, and the router sends everything still queued to the
/// spool. The worst case from `QueuedSink::finish` to the end of the threads is
/// the last partial block (0.1 s), this budget, one `LINK_POLL`, and the
/// STOP/FINISH exchange (`IO_TIMEOUT`, 2 s), which runs at the same time as the
/// spool seal: about 3.2 s, inside the 10 s watchdog.
const SHUTDOWN_LINK_BUDGET: Duration = Duration::from_secs(1);
const _: () = assert!(BLOCK_SEND_BYTES + MAX_ENCODED_LENGTH + 4 <= BLOCK_CAPACITY);
const _: () = assert!(REPLAY_BATCH_BYTES + MAX_ENCODED_LENGTH + 4 <= BLOCK_CAPACITY);
const _: () = assert!(LINK_FIFO_BLOCKS >= 2 && REPLAY_CREDIT > 0);
// The FIFO must have room for the front list and every replay batch, so a send
// that the router has checked never waits.
const _: () = assert!(FRONT_LIST_BLOCKS + REPLAY_CREDIT as usize <= LINK_FIFO_BLOCKS);
const _: () = assert!(REPLAY_TURN_BYTES >= REPLAY_BATCH_BYTES);

pub(crate) use self::link::LinkSettings;

mod link;
mod monitor;
mod router;

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

/// What the router sends the link thread through the FIFO.
enum Item {
    /// A block of live frames, older than every block the router sends later.
    Live(Block),
    /// A batch read from the spool. The router has at most [`REPLAY_CREDIT`] out.
    Replay(Block),
}

impl Item {
    /// The block, whichever kind it is.
    fn into_block(self) -> Block {
        match self {
            Self::Live(block) | Self::Replay(block) => block,
        }
    }
}

/// What the link thread tells the router.
enum Return {
    /// Live frames that the socket did not fully accept, in order, to be sent
    /// again before anything newer. Only the block in hand comes back this way;
    /// the router takes the FIFO itself.
    Returned(Block),
    /// The link thread is done with a replay batch: the router has a credit
    /// back. `bytes` and `frames` are the prefix of the batch that the session
    /// fully handed to the socket, ending at a frame boundary: all of it for a
    /// batch written whole, less at a failure. The router reports it to the
    /// spool, which holds the rest until the router rewinds.
    ReplayDone { bytes: usize, frames: u32 },
    /// The link thread counted a whole replay batch in `disconnected_dropped`
    /// (the batch reached it with no session to send it on). The router has a
    /// credit back, and the spool passes the batch without counting it as
    /// replayed, so the frames are in one counter only.
    ReplayCounted,
    /// The link thread has ended its session and returned its block in hand. It
    /// takes nothing from the FIFO and publishes `up` again only after the router
    /// answers with a fence.
    HandBackDone,
}

/// The router's ends of its channels to the link thread.
struct RouterPorts {
    /// The only producer of the link FIFO.
    fifo: Sender<Item>,
    /// The router's own consumer end of the link FIFO. The channel is
    /// multi-consumer, so each item reaches exactly one of the router and the
    /// link thread. The router takes items only under the take lock, to spill the
    /// FIFO, and when the link thread is not taking, to hand back.
    fifo_end: Receiver<Item>,
    /// Answers a `HandBackDone`: everything older is placed and the FIFO is
    /// empty, so the link thread may use its next session. Its own channel,
    /// because the link thread must not look into the FIFO before the router has
    /// emptied it. Closing it ends the link thread's wait.
    fences: Sender<()>,
    returns: Receiver<Return>,
}

/// The link thread's ends of the same channels.
struct LinkPorts {
    fifo: Receiver<Item>,
    fences: Receiver<()>,
    returns: Sender<Return>,
}

/// Opens the channels between the router and the link thread. The FIFO holds
/// `fifo_blocks` items; production uses [`LINK_FIFO_BLOCKS`].
fn link_channels(fifo_blocks: usize) -> (RouterPorts, LinkPorts) {
    let (fifo, link_fifo) = bounded(fifo_blocks);
    let (fences, link_fences) = bounded(1);
    let (link_returns, returns) = bounded(RETURN_MESSAGES);
    (
        RouterPorts {
            fifo,
            fifo_end: link_fifo.clone(),
            fences,
            returns,
        },
        LinkPorts {
            fifo: link_fifo,
            fences: link_fences,
            returns: link_returns,
        },
    )
}

/// When shutdown began, shared by the parser end, the router, and the link
/// thread.
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

/// State shared by the parser end, the router, and the link thread.
struct LinkShared {
    /// True while the link thread can take blocks. Only the link thread writes
    /// it: it clears it when a session ends, and sets it again only after the
    /// router's fence and a session are established. The router reads it.
    up: AtomicBool,
    /// The take lock. The link thread takes a FIFO item only while it holds the
    /// lock. The router holds the lock while it takes every item out of the FIFO
    /// to spill it, so no item can slip to the link thread between two of the
    /// router's pops and reach the socket ahead of an older item that is on its
    /// way to the spool.
    take: Mutex<()>,
    shutdown: Shutdown,
    /// Times the pool was empty when a block was needed, and a new one was
    /// allocated instead. The pool bound makes this zero.
    #[cfg(test)]
    pool_fallbacks: AtomicU64,
    /// What the router publishes for tests to wait on.
    #[cfg(test)]
    probe: Probe,
}

/// The router's state, published at the end of each pass so a test can wait for
/// a state instead of a time.
#[cfg(test)]
#[derive(Default)]
struct Probe {
    /// Blocks in the front list.
    front: std::sync::atomic::AtomicUsize,
    /// The most blocks the front list has held.
    front_max: std::sync::atomic::AtomicUsize,
    /// Items in the link FIFO.
    fifo: std::sync::atomic::AtomicUsize,
    /// Replay batches in the FIFO or in the link thread's hand.
    replay_out: std::sync::atomic::AtomicUsize,
    /// Times the router took the FIFO to spill it.
    spills: std::sync::atomic::AtomicUsize,
    /// Passes the router has finished. A pass that starts after a test saw the
    /// channel empty has routed every block taken before it.
    turns: std::sync::atomic::AtomicUsize,
    /// Hand-backs the router has finished: the FIFO is taken, the blocks are
    /// placed, and the fence is sent (or the link thread has vanished).
    hand_backs: std::sync::atomic::AtomicUsize,
    /// Passes of the hand-back loop, which waits for the link thread.
    hand_back_polls: std::sync::atomic::AtomicUsize,
}

impl LinkShared {
    fn new(up: bool) -> Self {
        Self {
            up: AtomicBool::new(up),
            take: Mutex::new(()),
            shutdown: Shutdown::new(),
            #[cfg(test)]
            pool_fallbacks: AtomicU64::new(0),
            #[cfg(test)]
            probe: Probe::default(),
        }
    }

    /// A block of the pool, or a new one if the pool is empty (a bug in the
    /// pool bound, so tests count it).
    #[cfg_attr(
        not(test),
        expect(clippy::unused_self, reason = "only the test counter reads `self`")
    )]
    fn take_block(&self, free: &ArrayQueue<Block>) -> Block {
        free.pop().unwrap_or_else(|| {
            #[cfg(test)]
            self.pool_fallbacks.fetch_add(1, Ordering::Relaxed);
            Block::new()
        })
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
    shared: Arc<LinkShared>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl QueuedSink {
    /// Allocates the block pool and starts the `dnstap-link` thread, which takes
    /// over the link, and the `dnstap-sender` thread, which routes blocks and
    /// owns the spool. `stats` is the counter set the link and the spool already
    /// write to. A session that accepts no bytes for `stall_timeout` while the
    /// link has bytes to write is ended.
    ///
    /// # Errors
    /// Returns the error if a thread cannot be started.
    pub(crate) fn start(
        link: NetworkSink,
        spool: Option<Spool>,
        stats: Arc<SinkStats>,
        stall_timeout: Duration,
    ) -> io::Result<Self> {
        Self::start_with(link, spool, stats, LinkSettings::new(stall_timeout))
    }

    /// [`Self::start`] over any [`Uplink`] and link settings, so tests can run
    /// the threads against a scripted transport and a manual clock.
    fn start_with<U: Uplink>(
        link: U,
        spool: Option<Spool>,
        stats: Arc<SinkStats>,
        settings: LinkSettings,
    ) -> io::Result<Self> {
        let (queue, parser_end) = bounded(SEND_QUEUE_BLOCKS);
        let free = Arc::new(ArrayQueue::new(POOL_BLOCKS));
        for _ in 0..POOL_BLOCKS - 1 {
            release(&free, Block::new());
        }
        // `up` starts clear. The link thread enters link mode on the first session
        // and then sets it; a session that cannot enter link mode is lost, `up`
        // stays clear, and the router sends the blocks to the spool.
        let shared = Arc::new(LinkShared::new(false));
        let (router_ports, link_ports) = link_channels(LINK_FIFO_BLOCKS);
        let (ready_sender, ready) = mpsc::channel();
        let link_thread = LinkThread::new(
            link,
            link_ports,
            Arc::clone(&shared),
            Arc::clone(&stats),
            Arc::clone(&free),
            settings,
        )
        .with_ready(ready_sender);
        let link_handle = thread::Builder::new()
            .name("dnstap-link".into())
            .spawn(move || link_thread.run())?;
        // The router starts from the state the link thread published, so a block
        // that arrives at once is not sent to a link that is not in link mode. The
        // wait ends at once if the link thread has ended.
        let _ = ready.recv();
        // If the router cannot start, dropping it closes the FIFO and the fence
        // channel, and the link thread ends itself.
        let router = Router::new(
            spool,
            Arc::clone(&stats),
            Arc::clone(&free),
            Arc::clone(&shared),
            router_ports,
            link_handle,
        );
        let thread = thread::Builder::new()
            .name("dnstap-sender".into())
            .spawn(move || router.run(&parser_end))?;
        Ok(Self {
            current: Block::new(),
            queue: Some(queue),
            free,
            stats,
            shared,
            thread: Some(thread),
        })
    }

    /// Hands the current block to the router without blocking. A full queue
    /// discards the block's frames and counts them in `queue_dropped`; the block
    /// is then reused.
    ///
    /// If the router has stopped, the block is discarded and an error is
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
                // circulation, which the channel, FIFO, and front list bounds
                // guarantee; allocating here would mean that bound broke.
                self.current = self.shared.take_block(&self.free);
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
    /// unchanged, and an error if the router has stopped.
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

    /// Sends the partial block. The link thread writes it when the socket is
    /// ready; there is nothing to flush.
    fn flush(&mut self) -> io::Result<()> {
        self.send_current(0)
    }

    /// Sends the last partial block, closes the channel, and waits for the router
    /// to finish. The router and the link thread route the blocks already
    /// queued, seal the spool segment, and run the STOP/FINISH exchange if the
    /// link is up. They do not drain a spool backlog: that stays on disk for the
    /// next run.
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
        self.shared.shutdown.begin();
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
    /// detached threads end themselves within the link budget. Does not join.
    fn drop(&mut self) {
        self.shared.shutdown.begin();
    }
}

#[cfg(test)]
#[path = "sender/sender_tests.rs"]
mod sender_tests;

#[cfg(test)]
#[path = "sender/saturation_tests.rs"]
mod saturation_tests;

#[cfg(test)]
#[path = "sender/observation_tests.rs"]
mod observation_tests;

#[cfg(test)]
mod script;

#[cfg(test)]
#[path = "sender/fault_tests.rs"]
mod fault_tests;
