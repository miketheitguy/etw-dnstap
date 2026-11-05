//! The output seam between the parser thread and a dnstap Frame Streams
//! transport.

use std::{
    io,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use crate::threadcpu::ThreadCpu;

/// Transport health and loss counters shared between a sink and the supervisor.
///
/// The sink writes them; the supervisor reads them for periodic and final
/// reports. Every counter is monotonic except the gauges (`spool_bytes`,
/// `spool_segments`, `send_queue_depth_max`, `write_max_recent_us`,
/// `router_wait_max_recent_us`, `router_pass_max_recent_us`,
/// `spool_oldest_age_ms`) and the flags (`connected`, `spool_available`,
/// `link_saturated`).
#[derive(Default)]
pub(crate) struct SinkStats {
    /// CPU time of the `dnstap-sender` thread (the router).
    pub(crate) cpu_sender: ThreadCpu,
    /// CPU time of the `dnstap-link` thread (the socket writes).
    pub(crate) cpu_link: ThreadCpu,
    /// True while a Frame Streams session is established.
    pub(crate) connected: AtomicBool,
    /// Connections lost after having been established.
    pub(crate) disconnects: AtomicU64,
    /// Successful connections after the first.
    pub(crate) reconnects: AtomicU64,
    /// Reconnect attempts that failed (connect, handshake, or protocol error).
    pub(crate) reconnect_failures: AtomicU64,
    /// Frames discarded because the transport was down or failed:
    /// - with no spool, every live frame that the link had not fully sent when a
    ///   connection failed (the block in hand and the link queue), and
    ///   every frame that arrives while the link is down;
    /// - with a spool, at shutdown after a spill, the live block (at most 1)
    ///   that came back from the link: it is older than the spool backlog, so it
    ///   cannot go in front of it.
    ///
    /// With a spool, a failed connection counts nothing here. Live frames that it
    /// did not fully accept (with TLS, including the frames of the one piece whose
    /// delivery was unconfirmed) go back to the spool and are sent again, and the
    /// spool keeps the replay frames that it did not fully accept. Bytes the
    /// kernel accepted before a failure are not counted, even if the connection
    /// then loses them. A counted frame did not reach the receiver whole, except
    /// in the TLS 1.3 key update window. Distinct from
    /// the intake counters, which count events dropped before reaching the parser.
    pub(crate) disconnected_dropped: AtomicU64,
    /// Blocks (about 64 KiB of frames) whose wait for the socket to become
    /// writable reached the slow-write threshold. The link thread's sends never
    /// block; it waits for writability and this counts the waits that were long.
    pub(crate) slow_writes: AtomicU64,
    /// Longest wait for writability while writing one block, in microseconds,
    /// since the service started.
    pub(crate) write_max_us: AtomicU64,
    /// Accumulator for `write_max_recent_us`: the longest wait for writability
    /// since the router last took it. Only the router resets it (with a swap);
    /// no reader does. Not reported.
    pub(crate) write_window_max_us: AtomicU64,
    /// Gauge: the longest wait for writability while writing one block, in
    /// microseconds, in the last 60 seconds. The router recomputes it every
    /// second, so every reader sees the same value.
    pub(crate) write_max_recent_us: AtomicU64,
    /// Accumulator for `router_wait_max_recent_us`: the longest router wait
    /// since the router last took it. Only the router writes and resets it.
    /// Not reported.
    pub(crate) router_wait_window_max_us: AtomicU64,
    /// Gauge: the longest single call of the router's wait, from entry to
    /// return, in microseconds, in the last 60 seconds. Normally at most about
    /// 50 ms plus scheduler jitter. A larger value means the router thread did
    /// not run when its wait ended (a late wake). The router recomputes it every
    /// second.
    pub(crate) router_wait_max_recent_us: AtomicU64,
    /// Accumulator for `router_pass_max_recent_us`: the longest router pass
    /// since the router last took it. Only the router writes and resets it.
    /// Not reported.
    pub(crate) router_pass_window_max_us: AtomicU64,
    /// Gauge: the longest router pass outside its wait, in microseconds, in the
    /// last 60 seconds. A pass is the rest of one loop iteration: the link
    /// thread's messages, the follow of the link, the checkpoint, routing and
    /// replay, and the monitor sample. A large value means a call in the pass
    /// blocked or ran long. The router recomputes it every second.
    pub(crate) router_pass_max_recent_us: AtomicU64,
    /// Bytes that the socket accepted from the link thread, whether live or
    /// replayed. With plaintext these are the framed bytes (length prefix and
    /// payload). With TLS they are the bytes `write_tls` wrote, so they include
    /// the record headers and tags. Bytes the kernel still holds, and so lost
    /// if the connection then fails, are included. Excludes the Frame Streams
    /// control frames.
    pub(crate) bytes_sent: AtomicU64,
    /// Gauge: the most blocks (about 64 KiB each) waiting in the parser channel
    /// in the last 60 seconds, sampled by the router after each receive. A lower
    /// bound.
    pub(crate) send_queue_depth_max: AtomicU64,
    /// Gauge: milliseconds since the oldest unsent spool segment was created, 0
    /// with no backlog. Updated every second. An upper bound on the age of the
    /// oldest unsent frame: the segment is older than any frame in it.
    pub(crate) spool_oldest_age_ms: AtomicU64,
    /// Flag: true while the spool backlog trends up with the link connected
    /// (larger than ten samples ago, with at least seven of the ten steps
    /// rising), which means the link carries less than the capture rate. Cleared
    /// when the backlog is empty or no longer larger than ten samples ago. Holds
    /// its value while the link is down.
    pub(crate) link_saturated: AtomicBool,
    /// Frames discarded on the parser thread because the send queue was full.
    pub(crate) queue_dropped: AtomicU64,
    /// Frames appended to the spool.
    pub(crate) spooled: AtomicU64,
    /// Frames from the spool that the link fully handed to the socket, counted
    /// when it does. A frame that a failed connection did not take is read again
    /// and counts then.
    pub(crate) replayed: AtomicU64,
    /// Frames removed from the spool unsent, or refused by it: eviction, corrupt
    /// or cut-off data, floor suspension, I/O errors.
    pub(crate) spool_dropped: AtomicU64,
    /// Bytes of segment files removed or skipped unsent, control frames included:
    /// from the first unread byte (the start of the file when nothing was read)
    /// to the end. Leftover segments with an unknown frame count count here only.
    pub(crate) spool_dropped_bytes: AtomicU64,
    /// Spool I/O errors.
    pub(crate) spool_errors: AtomicU64,
    /// Gauge: bytes in segment files now, including files waiting for a delete
    /// to succeed.
    pub(crate) spool_bytes: AtomicU64,
    /// Gauge: segment files now, including files waiting for a delete to succeed.
    pub(crate) spool_segments: AtomicU64,
    /// Flag: true while a spool is in use and accepting data. False with
    /// `--no-spool`, with an unusable folder, and while the spool is paused by an
    /// I/O error or by the free-space floor.
    pub(crate) spool_available: AtomicBool,
    /// Flag: shutdown has begun. From then on the sink threads write no log
    /// line; the counters and the three shutdown results below carry what they
    /// would have said, and the `output stopped` report shows them.
    pub(crate) stopping: AtomicBool,
    /// Shutdown result: true once the STOP frame was written and flushed to the
    /// receiver. FINISH may still be missing: the wait for it does not clear the
    /// flag. False when no session was up, a data frame was cut, the socket did
    /// not return to blocking mode, or the final data flush or the STOP write
    /// failed. Meaningful only in the `output stopped` report.
    pub(crate) stop_sent: AtomicBool,
    /// Shutdown result: true when unsent frames stay in the spool for the next
    /// run. Independent of the seal: a failed seal leaves them on disk too.
    pub(crate) spool_backlog_kept: AtomicBool,
    /// Shutdown result: the error text when the active spool segment could not
    /// be sealed, `None` otherwise.
    pub(crate) spool_seal_error: Mutex<Option<String>>,
}

impl SinkStats {
    /// Marks the start of shutdown. Idempotent.
    pub(crate) fn begin_stop(&self) {
        self.stopping.store(true, Ordering::Release);
    }

    /// True once shutdown has begun, so a log line must not be written.
    pub(crate) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    /// Records that the spool segment could not be sealed.
    pub(crate) fn set_spool_seal_error(&self, error: String) {
        *self
            .spool_seal_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(error);
    }

    /// The error recorded by [`Self::set_spool_seal_error`], if any.
    pub(crate) fn spool_seal_error(&self) -> Option<String> {
        self.spool_seal_error
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Destination for encoded dnstap frames.
///
/// The parser thread owns one sink and calls it from a single thread. In
/// production the sink is the `QueuedSink`, which hands frames to the
/// `dnstap-sender` thread. Frames are never resent after a failure; the
/// implementation decides whether an error is fatal (a single connection) or
/// recoverable (the reconnecting network sink, which discards and counts frames
/// while disconnected).
pub(crate) trait FrameSink {
    /// Queues one complete dnstap frame, possibly buffering it.
    ///
    /// # Errors
    /// Returns the transport error; the frame must be treated as not delivered.
    fn write_frame(&mut self, payload: &[u8]) -> io::Result<()>;

    /// Pushes buffered frames to the transport.
    ///
    /// # Errors
    /// Returns the transport error.
    fn flush(&mut self) -> io::Result<()>;

    /// Ends the stream cleanly, flushing buffered data first.
    ///
    /// # Errors
    /// Returns an error if the final flush or the closing exchange fails.
    fn finish(self) -> anyhow::Result<()>
    where
        Self: Sized;
}
