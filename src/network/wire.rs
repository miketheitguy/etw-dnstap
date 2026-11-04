//! The data path of a session as the link thread drives it: non-blocking, and
//! with exact byte counts.
//!
//! [`Wire`] is one established session's data path. [`Uplink`] is the
//! reconnecting provider of sessions, which the link thread owns. Both are
//! traits so the link writer in `crate::sender` can run against a scripted
//! transport in tests, with the same code as in production.
//!
//! A data send never has a socket timeout: `WouldBlock` is a defined result
//! that queued nothing, and the caller waits for writability with
//! [`Wire::wait_writable`]. The functions here hold the byte-level rules for
//! each transport:
//!
//! - **Plaintext.** `write` reports exactly how many bytes the socket took.
//! - **TLS.** The plaintext goes to rustls through `writer()` in pieces, and
//!   `write_tls` moves the records to the socket. `write_tls` keeps what the
//!   socket did not take inside rustls, so `WouldBlock` never cuts a record.
//!   Neither `write_all`, `StreamOwned::write`, nor a `BufWriter` is used on
//!   the data path: each hides how much of a write was accepted.

use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

use anyhow::Result;
use rustls::ClientConnection;

/// The most plaintext fed to rustls in one piece: one TLS record at the default
/// fragment size. Between pieces the caller drains `write_tls` until
/// [`Wire::pending`] is false, so the plaintext whose fate is unknown at a
/// failure is one piece.
pub(crate) const TLS_FEED_BYTES: usize = 16 * 1024;

/// What one call moved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Moved {
    /// Bytes of the offered data that the session took. For plaintext these are
    /// the bytes the socket accepted; for TLS, the plaintext rustls accepted,
    /// which has not reached the socket yet.
    pub(crate) taken: usize,
    /// Bytes the socket accepted in this call. For TLS these are the bytes of
    /// the records, including their headers and tags.
    pub(crate) written: usize,
}

impl Moved {
    /// Nothing moved: the socket is not writable.
    pub(crate) const NONE: Self = Self {
        taken: 0,
        written: 0,
    };
}

/// One session's data path, in non-blocking mode.
pub(crate) trait Wire {
    /// The most bytes to hand to [`Self::offer`] at a time (TLS), or `None` for
    /// no limit (plaintext). The caller ends a piece at a frame boundary.
    fn piece_limit(&self) -> Option<usize>;

    /// Hands `bytes` (not empty) to the session and reports what it took. A
    /// socket that cannot take a byte yet is `Ok(Moved::NONE)`, not an error.
    ///
    /// # Errors
    /// Returns the error that ended the session.
    fn offer(&mut self, bytes: &[u8]) -> io::Result<Moved>;

    /// True while bytes that [`Self::offer`] took have not all reached the socket
    /// (TLS records waiting inside rustls). Always false for plaintext.
    fn pending(&self) -> bool;

    /// Moves pending bytes to the socket. `Ok(Moved::NONE)` means the socket is
    /// not writable.
    ///
    /// # Errors
    /// Returns the error that ended the session.
    fn drain(&mut self) -> io::Result<Moved>;

    /// Waits until the socket is writable or `timeout` passes, whichever comes
    /// first. A socket error also ends the wait; the next write reports it.
    ///
    /// # Errors
    /// Returns the error of the wait call itself.
    fn wait_writable(&mut self, timeout: Duration) -> io::Result<()>;
}

/// The reconnecting provider of sessions that the link thread owns.
pub(crate) trait Uplink: Send + 'static {
    /// Advances the reconnect. Never blocks. Makes a new session ready for
    /// [`Self::wire`].
    fn poll(&mut self, now: Instant);

    /// True while a session is established.
    fn is_connected(&self) -> bool;

    /// The current session's data path, or `None` while disconnected.
    fn wire(&mut self) -> Option<&mut dyn Wire>;

    /// Ends the current session after `error` and starts the backoff. Counts
    /// the disconnect. It counts no frames: the link thread does that.
    fn lost(&mut self, error: &io::Error);

    /// Ends the link. With `clean`, the data path stopped at a frame boundary
    /// and the stream ends with STOP and FINISH; otherwise the connection is
    /// closed with no STOP.
    ///
    /// # Errors
    /// Returns the error of the closing exchange.
    fn close(self, clean: bool) -> Result<()>
    where
        Self: Sized;
}

/// Offers `bytes` to a plaintext socket. The socket may take part of them.
///
/// # Errors
/// Returns a socket error other than `WouldBlock` and `Interrupted`.
pub(crate) fn plain_offer(socket: &mut impl Write, bytes: &[u8]) -> io::Result<Moved> {
    match socket.write(bytes) {
        Ok(taken) => Ok(Moved {
            taken,
            written: taken,
        }),
        Err(error) if retry_later(&error) => Ok(Moved::NONE),
        Err(error) => Err(error),
    }
}

/// Feeds rustls with `bytes`. Nothing reaches the socket here.
///
/// # Errors
/// Returns the error rustls reports for the connection (a fatal alert, for
/// example).
pub(crate) fn tls_offer(connection: &mut ClientConnection, bytes: &[u8]) -> io::Result<Moved> {
    let taken = connection.writer().write(bytes)?;
    Ok(Moved { taken, written: 0 })
}

/// Writes the records waiting inside rustls to `socket` until it is empty or
/// the socket stops taking bytes.
///
/// # Errors
/// Returns a socket error other than `WouldBlock` and `Interrupted`.
pub(crate) fn tls_drain(
    connection: &mut ClientConnection,
    socket: &mut impl Write,
) -> io::Result<Moved> {
    let mut written = 0;
    while connection.wants_write() {
        match connection.write_tls(socket) {
            Ok(0) => break,
            Ok(count) => written += count,
            Err(error) if retry_later(&error) => break,
            Err(error) => return Err(error),
        }
    }
    Ok(Moved { taken: 0, written })
}

/// A result that queued nothing and may succeed later.
fn retry_later(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
    )
}

#[cfg(test)]
#[path = "wire_tests.rs"]
mod wire_tests;
