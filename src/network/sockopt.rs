//! The socket call std does not expose: a wait for writability.

use std::{io, net::TcpStream, os::windows::io::AsRawSocket, time::Duration};

use windows_sys::Win32::Networking::WinSock::{POLLWRNORM, SOCKET_ERROR, WSAPOLLFD, WSAPoll};

/// Waits until `stream` can take a send, or until `timeout` passes (rounded up
/// to whole milliseconds, and capped at `i32::MAX` of them). Returns after a
/// timeout, after readiness, and after a socket error or hang-up alike: the
/// caller's next send reports which one it was, so this call does not.
///
/// This is a readiness wait, not a send, so it carries no socket timeout and
/// queues nothing.
pub(super) fn wait_writable(stream: &TcpStream, timeout: Duration) -> io::Result<()> {
    let milliseconds = i32::try_from(timeout.as_micros().div_ceil(1000)).unwrap_or(i32::MAX);
    let mut poll = WSAPOLLFD {
        fd: usize::try_from(stream.as_raw_socket())
            .map_err(|_| io::Error::other("socket handle exceeds usize"))?,
        events: POLLWRNORM,
        revents: 0,
    };
    // SAFETY: `poll` is one live, initialised `WSAPOLLFD`, and the count passed
    // is 1, so WSAPoll reads and writes exactly that element. The socket is a
    // valid open socket borrowed from `stream` for this call. The timeout is a
    // plain millisecond count.
    let result = unsafe { WSAPoll(&raw mut poll, 1, milliseconds) };
    if result == SOCKET_ERROR {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
