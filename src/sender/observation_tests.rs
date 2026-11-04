//! The signals that show a slow or saturated receiver, end to end through the
//! sender threads: bytes sent and the monitor wiring. The state machine and the
//! windows have their own tests in `monitor_tests.rs`, and the router's own
//! sampling of the spool is tested in `router_tests.rs`.

use super::{
    sender_tests::{
        FakeServer, MIB, fast_policy, get, no_retry, open_spool, send, start_sink, wait_until,
    },
    *,
};
use crate::spool::test_support::TestDir;

/// The bytes of one `send` frame: a 4-byte length and a 4-byte payload.
const FRAME_BYTES: u64 = 8;

#[test]
fn bytes_sent_counts_the_framed_bytes_the_link_accepted() {
    let server = FakeServer::up();
    let stats = Arc::new(SinkStats::default());
    let mut sink = start_sink(&server, no_retry(), None, &stats);
    send(&mut sink, 0..100);
    sink.finish().expect("clean STOP/FINISH");
    assert_eq!(server.received.frames().len(), 100);
    assert_eq!(get(&stats.bytes_sent), 100 * FRAME_BYTES);
}

#[test]
fn frames_discarded_while_the_receiver_is_down_are_not_bytes_sent() {
    let server = FakeServer::down();
    let stats = Arc::new(SinkStats::default());
    let mut sink = start_sink(&server, no_retry(), None, &stats);
    send(&mut sink, 0..10);
    sink.finish().expect("shutdown while disconnected is clean");
    assert_eq!(get(&stats.disconnected_dropped), 10);
    assert_eq!(get(&stats.bytes_sent), 0);
}

#[test]
fn a_replay_counts_its_bytes_when_it_is_sent_and_not_when_it_is_spooled() {
    let dir = TestDir::new("bytes-replay");
    let server = FakeServer::down();
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 8 * MIB, &stats);
    let mut sink = start_sink(&server, fast_policy(), Some(spool), &stats);
    send(&mut sink, 0..50);
    wait_until_spooled(&stats, 50);
    assert_eq!(get(&stats.bytes_sent), 0, "spooled, not sent");
    server.serve_now();
    wait_until("the replay", || server.received.frames().len() == 50);
    sink.finish().expect("clean STOP/FINISH");
    assert_eq!(get(&stats.bytes_sent), 50 * FRAME_BYTES);
    assert_eq!(get(&stats.replayed), 50);
}

fn wait_until_spooled(stats: &SinkStats, frames: u64) {
    wait_until("the spool to take the frames", || {
        get(&stats.spooled) >= frames
    });
}
