//! Helpers shared by the network test modules.

use super::*;

/// Completes the receiver side of the Frame Streams handshake (READY, ACCEPT,
/// START) and returns the stream for the rest of the exchange.
pub(super) fn handshake(stream: TcpStream) -> DeadlineTcp {
    handshake_over(DeadlineTcp::new(stream, Duration::from_secs(10)))
}

/// The receiver side of the handshake over any stream, such as a TLS server
/// connection.
pub(super) fn handshake_over<S: Read + Write>(mut stream: S) -> S {
    read_control(&mut stream, CONTROL_READY, true).expect("READY");
    write_control(&mut stream, CONTROL_ACCEPT, true).expect("ACCEPT");
    read_control(&mut stream, 2, true).expect("START");
    stream
}

/// A loopback port with no listener, so a connection to it is refused, held
/// until the value is dropped.
///
/// The port is the client end of a loopback connection. While that connection
/// is open, no other socket is given the port, so a test running in parallel
/// cannot bind it and accept the connection. A port taken from a dropped
/// listener has no such guard.
pub(super) struct RefusedPort {
    address: SocketAddr,
    _connection: (TcpStream, TcpStream),
}

impl RefusedPort {
    pub(super) fn new() -> Self {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let client =
            TcpStream::connect(listener.local_addr().expect("listener address")).expect("connect");
        let (server, _) = listener.accept().expect("accept");
        Self {
            address: client.local_addr().expect("client address"),
            _connection: (client, server),
        }
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }
}

/// A reconnect policy with the given delays in milliseconds. Use a small pair to
/// reconnect quickly, or a very large one so no retry happens within a test.
pub(super) fn retry_policy(initial_ms: u64, max_ms: u64) -> BackoffPolicy {
    BackoffPolicy {
        initial: Duration::from_millis(initial_ms),
        max: Duration::from_millis(max_ms),
    }
}

/// How long [`wait_until`] waits before failing. Generous: the reconnect
/// policies in these tests retry within tens of milliseconds, and the bound
/// only matters when a test fails.
pub(super) const WAIT_TIMEOUT: Duration = Duration::from_secs(20);

/// Polls `condition` until it holds. Fails the test when `timeout` elapses
/// first, naming `what` was waited for. The condition may do work on each poll,
/// such as offering frames to a sink so it notices a reconnect.
pub(super) fn wait_until(timeout: Duration, what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "timed out after {timeout:?} waiting for {what}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}
