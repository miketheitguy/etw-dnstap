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
/// `spool_segments`) and the flags (`connected`, `spool_available`).
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
    /// Frames discarded because the transport was down: events that arrived while
    /// disconnected plus frames buffered, and so possibly lost, when the
    /// connection failed. Never resent. Distinct from the intake counters, which
    /// count events dropped before reaching the parser.
    pub(crate) disconnected_dropped: AtomicU64,
    /// Socket writes that took at least the slow-write threshold (blocked on a
    /// slow receiver).
    pub(crate) slow_writes: AtomicU64,
    /// Longest single socket write in microseconds.
    pub(crate) write_max_us: AtomicU64,
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
