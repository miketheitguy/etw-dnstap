//! The output seam between the parser thread and a dnstap Frame Streams
//! transport.

use std::{
    io,
    sync::atomic::{AtomicBool, AtomicU64},
};

/// Transport health and loss counters shared between a sink and the supervisor.
///
/// The sink writes them; the supervisor reads them for periodic and final
/// reports. Every counter is monotonic except the gauges (`spool_bytes`,
/// `spool_segments`, `send_queue_depth_max`, `write_max_recent_us`,
/// `spool_oldest_age_ms`) and the flags (`connected`, `spool_available`,
/// `link_saturated`).
#[derive(Default)]
pub(crate) struct SinkStats {
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
    /// - the unsent rest of a replay batch when a connection failed;
    /// - with TLS, the frames of the one piece whose delivery was unconfirmed;
    /// - with a spool, at shutdown after a spill, the live block (at most 1)
    ///   that came back from the link: it is older than the spool backlog, so it
    ///   cannot go in front of it.
    ///
    /// With a spool, live frames that a failed connection did not fully accept
    /// are not lost: they go to the spool and are sent again. A counted frame may
    /// still have arrived, so this is an upper bound on the loss. Distinct from
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
    /// Frames read from the spool and handed to the link.
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
