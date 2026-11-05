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
//!   rustls allocates one buffer per encrypted record. Its unbuffered API does
//!   not avoid that (it copies the allocated record into the caller's buffer);
//!   see "Allocation on the TLS path" in `docs/design/data-path.md`.
//!
//! Two cases leave a piece in doubt, and the rule that an unconfirmed piece
//! cannot have reached the receiver whole has one exception.
//!
//! - **The record limit.** At the confidentiality limit of its cipher (2^24
//!   records for TLS 1.2 with AES-GCM), rustls queues `close_notify`, refuses the
//!   record, and still reports the bytes as taken. [`tls_offer`] checks that the
//!   ciphertext it queued is larger than the plaintext it took, which any record
//!   makes it, and fails the session otherwise. The data is not confirmed, so
//!   the link gives it back, and the reconnect has new keys.
//! - **A key update.** In TLS 1.3 rustls can queue a `KeyUpdate` (about 22 bytes)
//!   after a data record, at the record limit or on `refresh_traffic_keys()`. If
//!   the socket takes the whole data record and the session fails while the
//!   `KeyUpdate` is still pending, [`Wire::pending`] is true, so the piece stays
//!   unconfirmed, although the receiver holds it. The frames are sent again and
//!   arrive twice. The window is the time to write 22 bytes, once in 2^23 or
//!   more records.

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

/// The least that one TLS record adds to the plaintext it carries: its 5-byte
/// header (RFC 5246 section 6.2.1, RFC 8446 section 5.1) and the 16-byte
/// authentication tag of every AEAD cipher that rustls offers.
const MIN_RECORD_OVERHEAD: usize = 5 + 16;

/// The bytes rustls has queued for the socket. `process_new_packets` does no I/O
/// here, because this path never reads from the socket; it returns the state,
/// which `CommonState` alone does not expose as a count.
fn queued_tls_bytes(connection: &mut ClientConnection) -> io::Result<usize> {
    connection
        .process_new_packets()
        .map(|state| state.tls_bytes_to_write())
        .map_err(io::Error::other)
}

/// Feeds rustls with `bytes`. Nothing reaches the socket here.
///
/// `Writer::write` reports the bytes taken even when rustls refuses to encrypt
/// them: at the record limit of the cipher it queues `close_notify`, drops the
/// plaintext, and returns the length. A record always makes the ciphertext
/// longer than its plaintext by a header and a tag, so ciphertext that is not
/// at least that much larger than `taken` means the data was dropped. That is an
/// error here, and the session ends.
///
/// A refused piece of 10 bytes or fewer (smaller than a `close_notify` record,
/// and not a frame the encoder produces) could pass once. The next offer queues
/// nothing and fails.
///
/// # Errors
/// Returns the error rustls reports for the connection (a fatal alert, for
/// example), or an error when rustls took bytes and queued no record for them.
pub(crate) fn tls_offer(connection: &mut ClientConnection, bytes: &[u8]) -> io::Result<Moved> {
    let before = queued_tls_bytes(connection)?;
    let taken = connection.writer().write(bytes)?;
    if taken > 0 {
        let added = queued_tls_bytes(connection)?.saturating_sub(before);
        if added < taken + MIN_RECORD_OVERHEAD {
            return Err(io::Error::other(
                "TLS took plaintext and queued no record for it: the cipher's record limit",
            ));
        }
    }
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
