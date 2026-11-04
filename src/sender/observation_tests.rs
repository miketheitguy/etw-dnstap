//! The signals that show a slow or saturated receiver, end to end through the
//! sender thread: bytes sent, the send queue peak, and the monitor wiring. The
//! state machine and the windows have their own tests in `monitor_tests.rs`.

use super::{
    sender_tests::{
        FakeServer, MIB, fast_policy, get, idle_sender, no_retry, open_spool, send, start_sink,
        wait_until,
    },
    *,
};
use crate::spool::test_support::{TestDir, capture_logs_at};

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

/// Appends a block of `bytes` bytes to the spool of `sender`, so its backlog grows.
fn grow_backlog(sender: &mut SenderThread, bytes: usize) {
    let spool = sender.spool.as_mut().expect("the sender has a spool");
    let mut frames = Vec::new();
    while frames.len() < bytes {
        frames.extend_from_slice(&[0, 0, 0, 4, 1, 2, 3, 4]);
    }
    let count = u32::try_from(frames.len() / 8).expect("small");
    assert_eq!(spool.append(&frames, count), Appended::Stored);
}

#[test]
fn the_sender_sets_and_clears_link_saturated_from_the_spool_it_owns() {
    let dir = TestDir::new("observe-saturated");
    let server = FakeServer::up();
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 64 * MIB, &stats);
    let mut sender = idle_sender(Some(spool), &stats, &server);
    assert!(sender.link.is_connected());

    let logs = capture_logs_at(tracing::Level::INFO, || {
        // The backlog grows at every sample while the link is connected.
        for _ in 0..=monitor::TREND_SAMPLES {
            grow_backlog(&mut sender, 4_096);
            sender.monitor.force_due();
            sender.sample_monitor();
        }
        assert!(stats.link_saturated.load(Ordering::Relaxed));
        assert!(
            get(&stats.spool_oldest_age_ms) < 5_000,
            "a new segment is young"
        );
        // Routing does not read the flag: the destination follows the backlog
        // and the queue, as before.
        assert_eq!(sender.destination(true, 0), Route::Spool);

        // The link drains the backlog: the state clears at once.
        let mut out = Vec::new();
        let backlog = sender.spool.as_mut().expect("spool");
        while backlog.read_batch(&mut out, 1 << 20).expect("read") > 0 {}
        assert!(!sender.has_backlog());
        sender.monitor.force_due();
        sender.sample_monitor();
        assert!(!stats.link_saturated.load(Ordering::Relaxed));
        assert_eq!(get(&stats.spool_oldest_age_ms), 0);
    });
    assert_eq!(logs.count("dnstap link is saturated"), 1);
    assert_eq!(logs.count("dnstap link is no longer saturated"), 1);
}
