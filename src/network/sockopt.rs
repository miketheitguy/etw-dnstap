//! The one socket option std does not expose: the send buffer size.

use std::{io, net::TcpStream, os::windows::io::AsRawSocket};

use windows_sys::Win32::Networking::WinSock::{SO_SNDBUF, SOCKET_ERROR, SOL_SOCKET, setsockopt};

/// Requests a kernel send buffer of `bytes` so a briefly slow receiver stalls the
/// parser thread less. The kernel may round or cap the value.
pub(super) fn set_send_buffer(stream: &TcpStream, bytes: u32) -> io::Result<()> {
    let value = i32::try_from(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "send buffer exceeds i32"))?;
    // SAFETY: The socket is a valid open socket borrowed from `stream` for this
    // call. `value` is a live 4-byte int and the length passed is its size, as
    // SO_SNDBUF requires.
    let result = unsafe {
        setsockopt(
            usize::try_from(stream.as_raw_socket())
                .map_err(|_| io::Error::other("socket handle exceeds usize"))?,
            SOL_SOCKET,
            SO_SNDBUF,
            (&raw const value).cast(),
            i32::try_from(size_of::<i32>()).unwrap_or(4),
        )
    };
    if result == SOCKET_ERROR {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
