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

/// A reconnect policy with the given delays in milliseconds. Use a small pair to
/// reconnect quickly, or a very large one so no retry happens within a test.
pub(super) fn retry_policy(initial_ms: u64, max_ms: u64) -> BackoffPolicy {
    BackoffPolicy {
        initial: Duration::from_millis(initial_ms),
        max: Duration::from_millis(max_ms),
    }
}
