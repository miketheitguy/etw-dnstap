//! The output seam between the parser thread and a dnstap Frame Streams
//! transport.

use std::{
    io,
    sync::atomic::{AtomicBool, AtomicU64},
};

/// Transport health and loss counters shared between a sink and the supervisor.
///
/// The sink writes them; the supervisor reads them for periodic and final
/// reports. Every counter is monotonic except `connected`.
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
}

/// Destination for encoded dnstap frames.
///
/// The parser thread owns one sink and calls it from a single thread. Frames
/// are never resent after a failure; the implementation decides whether an
/// error is fatal (a single connection) or recoverable (the reconnecting
/// network sink, which discards and counts frames while disconnected).
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
