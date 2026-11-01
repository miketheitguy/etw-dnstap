//! The output seam between the parser thread and a dnstap Frame Streams
//! transport.

use std::io;

/// Destination for encoded dnstap frames.
///
/// The parser thread owns one sink and calls it from a single thread. Frames
/// are never resent after a failure; the implementation decides whether an
/// error is fatal (the TCP sink) or recoverable (a future reconnecting sink).
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
