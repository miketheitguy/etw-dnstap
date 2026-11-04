//! Tests for a receiver that stays connected but is slow or stalled, on real
//! loopback sockets. They began as characterization tests of the sender before
//! the link thread (task C1): each assertion that the link thread (H1) or the
//! progress rule (H2) changes now holds its new value, and a comment that starts
//! with `Changes with H1:` or `Changes with H2:` says what it was.
//!
//! The router never waits on the socket and the link thread ends a session only
//! when the receiver accepts no bytes for the stall timeout. The tests that
//! stall the link find the stall from the router's own spill: it spills only when
//! the link FIFO is full, which needs a link that has stopped taking blocks
//! (`stall_the_link`). They wait on conditions. The pauses that
//! must last (longer than the old 2-second I/O deadline) are conditions too: the
//! link's own wait for the blocked block reaches the length.
//!
//! Every frame carries its sequence number in its first four bytes. The large
//! frames (`MAX_ENCODED_LENGTH`) each fill one block, so a block holds exactly one
//! frame and the block counters equal the frame counters.

use super::{
    sender_tests::{
        FakeServer, MIB, fast_policy, get, no_retry, open_spool, queued, send, sized_frame,
        stall_the_link, start_sink, start_sink_with_stall, wait_until,
    },
    *,
};
use crate::{network::MIN_STALL_TIMEOUT, spool::test_support::TestDir};

/// Spool size that holds every frame these tests offer many times over.
const SPOOL_MIB: u64 = 64;

/// True when `numbers` is strictly increasing: in order, with no repeat.
fn strictly_increasing(numbers: &[u32]) -> bool {
    numbers.windows(2).all(|pair| pair[0] < pair[1])
}

/// Frames lost for any reason, as the counters report them.
fn counted_lost(stats: &SinkStats) -> u64 {
    get(&stats.queue_dropped) + get(&stats.disconnected_dropped) + get(&stats.spool_dropped)
}

/// Offers one large frame numbered `number`, once the channel has room.
fn offer_large(sink: &mut QueuedSink, number: u32) {
    wait_until("room in the send queue", || {
        queued(sink) < SEND_QUEUE_BLOCKS - 8
    });
    sink.write_frame(&sized_frame(number, MAX_ENCODED_LENGTH))
        .expect("a full queue is not an error");
}

fn resume(server: &FakeServer) {
    server.received.paused.store(false, Ordering::Release);
}

#[test]
fn a_stalled_link_does_not_overflow_the_queue_while_the_spool_has_room() {
    let dir = TestDir::new("sat-queue");
    let server = FakeServer::up();
    // The receiver keeps the connection but reads nothing, so the kernel buffers
    // fill and the link thread waits for the socket to become writable. A
    // receiver that reads very slowly stalls the link the same way, for as long
    // as one block takes; this is the limiting case.
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, SPOOL_MIB * MIB, &stats);
    let mut sink = start_sink(&server, no_retry(), Some(spool), &stats);

    let mut offered = stall_the_link(&mut sink);
    // The parser goes on producing. The router is free, so it takes every block
    // and spills it to the spool.
    for _ in 0..50 {
        offer_large(&mut sink, offered);
        offered += 1;
    }
    // Hold the stall for 100 ms by the link's own clock.
    wait_until("the link to wait for the blocked socket", || {
        get(&stats.write_max_us) >= 100_000
    });

    // The stall is far shorter than the stall timeout: the session is intact.
    assert_eq!(get(&stats.disconnects), 0);
    // Changes with H1: this was `queue_dropped == first_drop + 50` and
    // `spooled == 0`. The router keeps routing while the link waits, so the
    // blocks the link could not take went to the spool and none was dropped.
    assert_eq!(get(&stats.queue_dropped), 0);
    // The 50 frames are in the parser channel or on their way to the spool; the
    // router takes them at disk speed.
    wait_until("the router to spool the 50 later frames", || {
        get(&stats.spooled) >= LINK_FIFO_BLOCKS as u64 + 50
    });
    assert!(get(&stats.spool_bytes) > 0);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.spool_errors), 0);
    assert!(
        stats.spool_available.load(Ordering::Relaxed),
        "the spool is in use and accepting data"
    );

    // The receiver reads again before any stall timeout: no session was lost.
    // The spooled blocks replay in order.
    resume(&server);
    let total = usize::try_from(offered).expect("small");
    wait_until("every frame to arrive", || server.received.len() >= total);
    // The monitor shows the stall: the longest readiness wait of the window is
    // the blocked block's. The parser channel never filled. Wait for a published
    // sample: the monitor samples once a second.
    wait_until("the monitor to publish the blocked write", || {
        get(&stats.write_max_recent_us) >= 100_000 && get(&stats.send_queue_depth_max) >= 1
    });
    // Changes with H1: this was `send_queue_depth_max == SEND_QUEUE_BLOCKS`.
    assert!(get(&stats.send_queue_depth_max) < SEND_QUEUE_BLOCKS as u64);
    assert!(get(&stats.write_max_recent_us) <= get(&stats.write_max_us));
    assert!(get(&stats.bytes_sent) > 0);
    sink.finish().expect("clean STOP/FINISH");
    // Changes with H1: nothing was dropped, so every offered frame arrived.
    assert_eq!(
        server.received.frames(),
        (0..offered).collect::<Vec<_>>(),
        "frames arrive in capture order"
    );
    assert_eq!(get(&stats.replayed), get(&stats.spooled));
    assert_eq!(get(&stats.disconnects), 0);
    assert_eq!(counted_lost(&stats), 0);
}

#[test]
fn a_stall_shorter_than_the_stall_timeout_keeps_the_session() {
    let dir = TestDir::new("sat-session-kept");
    let server = FakeServer::up();
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, SPOOL_MIB * MIB, &stats);
    let mut sink = start_sink(&server, fast_policy(), Some(spool), &stats);

    let mut offered = stall_the_link(&mut sink);
    // Hold the stall longer than the old 2-second I/O deadline, which ended the
    // session. The link's own wait for the blocked block measures it.
    wait_until("a stall longer than the old I/O deadline", || {
        get(&stats.write_max_us) >= 2_500_000
    });
    // Changes with H2: this was `disconnects == 1` and `disconnected_dropped >= 1`.
    assert_eq!(get(&stats.disconnects), 0);
    assert_eq!(get(&stats.disconnected_dropped), 0);

    resume(&server);
    let newest = offered + 20;
    send(&mut sink, offered..newest + 1);
    offered = newest + 1;
    wait_until("the newest frame to arrive", || {
        server.received.frames().last() == Some(&newest)
    });
    sink.finish().expect("clean STOP/FINISH");

    assert_eq!(
        server.received.frames(),
        (0..offered).collect::<Vec<_>>(),
        "every frame arrives, in order"
    );
    // Changes with H2: this was `reconnects == 1`. The session was never lost.
    assert_eq!(get(&stats.disconnects), 0);
    assert_eq!(get(&stats.reconnects), 0);
    assert_eq!(counted_lost(&stats), 0);
    assert_eq!(get(&stats.replayed), get(&stats.spooled));
}

#[test]
fn a_receiver_that_accepts_nothing_for_the_stall_timeout_loses_the_session_and_only_the_replay_in_flight()
 {
    let dir = TestDir::new("sat-session");
    let server = FakeServer::up();
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, SPOOL_MIB * MIB, &stats);
    // The shortest stall timeout, and a fast reconnect policy, so the session
    // returns soon after the stall ends.
    let mut sink = start_sink_with_stall(
        &server,
        fast_policy(),
        Some(spool),
        &stats,
        MIN_STALL_TIMEOUT,
    );

    let started = Instant::now();
    let mut offered = stall_the_link(&mut sink);
    wait_until("the stall timeout to end the session", || {
        get(&stats.disconnects) >= 1
    });
    assert!(
        started.elapsed() >= MIN_STALL_TIMEOUT,
        "the session ended after only {:?}",
        started.elapsed()
    );
    assert_eq!(get(&stats.disconnects), 1);
    // The block the link gave back, the FIFO, and the blocks behind them reach
    // the spool or the front list. Wait for the router to take them all.
    wait_until("the router to take the queued blocks", || {
        queued(&sink) == 0
    });
    assert_eq!(get(&stats.reconnects), 0, "the receiver is still paused");
    // Changes with H2: this was `disconnected_dropped >= 1`. The unsent live
    // frames go back to the router, and none is abandoned. The only frames that
    // cannot go back are the replay batches the router had out (at most two, one
    // large frame each here): the spill left the router in Draining, and the
    // spool cursor had moved past them.
    assert!(
        get(&stats.disconnected_dropped) <= u64::from(REPLAY_CREDIT),
        "{} frames counted",
        get(&stats.disconnected_dropped)
    );

    // The receiver comes back and accepts the new session.
    resume(&server);
    wait_until("the sender to reconnect", || get(&stats.reconnects) >= 1);
    // Live data follows the replay.
    let newest = offered + 20;
    send(&mut sink, offered..newest + 1);
    offered = newest + 1;
    wait_until("the newest frame to arrive", || {
        server.received.frames().last() == Some(&newest)
    });
    sink.finish().expect("clean STOP/FINISH");

    let got = server.received.frames();
    assert!(strictly_increasing(&got), "order or repeat: {got:?}");
    assert_eq!(get(&stats.disconnects), 1);
    assert_eq!(get(&stats.reconnects), 1);
    assert_eq!(get(&stats.queue_dropped), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.replayed), get(&stats.spooled));
    // Changes with H2: this was a bound, because a write that timed out may
    // still complete in the kernel and the counted frames may also arrive. Now
    // the frames the kernel accepted before the stall reach the receiver, the cut
    // frame and every unsent live frame are sent again on the new session, and
    // the receiver gets each frame once. Only the replay batches in flight are
    // counted, and the receiver does not get them.
    assert!(counted_lost(&stats) <= u64::from(REPLAY_CREDIT));
    assert_eq!(
        got.len() as u64 + counted_lost(&stats),
        u64::from(offered),
        "each frame is delivered or counted once"
    );
}

#[test]
fn after_a_spill_every_later_block_takes_the_spool_path_until_the_backlog_is_empty() {
    let dir = TestDir::new("sat-delay-line");
    let server = FakeServer::up();
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, SPOOL_MIB * MIB, &stats);
    let mut sink = start_sink(&server, no_retry(), Some(spool), &stats);

    // The link stalls, and the link FIFO fills. The router takes the FIFO to the
    // spool, which now holds a backlog.
    // Changes with H1: the spill used to start when the sender thread found 96
    // blocks queued behind a blocked write. The router now spills a full FIFO.
    let mut offered = stall_the_link(&mut sink);
    let spooled_at_spill = get(&stats.spooled);
    let later_from = offered;

    // The receiver reads again, but slower than the input: 5 ms per large frame.
    // The input is offered as fast as the queue accepts it.
    server.received.frame_delay_ms.store(5, Ordering::Release);
    resume(&server);
    for _ in 0..40 {
        offer_large(&mut sink, offered);
        offered += 1;
    }
    let total = usize::try_from(offered).expect("small");
    wait_until("every frame to arrive", || server.received.len() >= total);

    // Every block offered after the spill went to the spool, including the 40
    // offered while the receiver was connected and reading, because the backlog
    // never emptied in between.
    assert!(get(&stats.spooled) >= spooled_at_spill + u64::from(offered - later_from));
    assert_eq!(get(&stats.replayed), get(&stats.spooled));
    assert_eq!(
        server.received.frames(),
        (0..offered).collect::<Vec<_>>(),
        "frames arrive in capture order"
    );
    // The receiver stayed connected, and nothing was lost.
    assert_eq!(get(&stats.disconnects), 0);
    assert_eq!(counted_lost(&stats), 0);

    // With the backlog gone the next block goes straight to the link again. The
    // router finds the backlog empty at its first read after the last batch, so
    // wait for the spool to drop its last segment.
    // Changes with H3b only: a live-first policy sends new frames to the link
    // while the backlog drains, out of order.
    wait_until("the backlog to empty", || get(&stats.spool_segments) == 0);
    let spooled = get(&stats.spooled);
    send(&mut sink, offered..offered + 1);
    wait_until("the probe frame to arrive", || {
        server.received.len() > total
    });
    sink.finish().expect("clean STOP/FINISH");
    assert_eq!(get(&stats.spooled), spooled, "the probe skipped the spool");
    assert_eq!(server.received.frames().last(), Some(&offered));
    // Every offered frame, the probe included, is accounted exactly once.
    assert_eq!(
        (total as u64 + 1) + counted_lost(&stats),
        u64::from(offered) + 1
    );
}

#[test]
fn a_receiver_that_stops_reading_causes_one_bounded_stall_then_counted_drops() {
    let server = FakeServer::up();
    // The receiver finishes the handshake and never reads again: the socket
    // buffers fill and the link must wait.
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    // No spool, and the shortest stall timeout.
    let mut sink = start_sink_with_stall(&server, no_retry(), None, &stats, MIN_STALL_TIMEOUT);
    let started = Instant::now();
    let mut offered = 0_u32;
    wait_until("the stall timeout to end the session", || {
        if get(&stats.disconnects) >= 1 {
            return true;
        }
        // Two frames of 32 KiB make a block. The parser channel stays short, so
        // nothing is dropped there.
        if queued(&sink) < 64 {
            assert!(offered < 4096, "kernel buffering absorbed 128 MiB");
            sink.write_frame(&sized_frame(offered, 32 * 1024))
                .expect("loss is never an error");
            offered += 1;
        }
        false
    });
    // One stall of the stall timeout (plus scheduling) ended the session.
    assert!(started.elapsed() >= MIN_STALL_TIMEOUT);
    assert!(!stats.connected.load(Ordering::Relaxed));
    assert_eq!(get(&stats.disconnects), 1);
    // The wait for the blocked socket is what the slow-write counters show.
    assert!(get(&stats.slow_writes) >= 1);
    assert!(get(&stats.write_max_us) >= 1_000_000);
    // With no spool, the frames that came back, the FIFO, and the channel are
    // counted once, in `disconnected_dropped`, by the router. The router counts
    // the FIFO when it has the hand-back; the count is final after that.
    wait_until("the router to finish the hand-back", || {
        sink.shared.probe.hand_backs.load(Ordering::Acquire) >= 1
    });
    // A partial block that the parser still holds goes to the router now.
    sink.flush().expect("block is sent");
    wait_until("the router to take the queued blocks", || {
        queued(&sink) == 0
    });
    // The router counts a block in the pass that takes it. Two more passes have
    // finished when the count is final.
    let turns = sink.shared.probe.turns.load(Ordering::Acquire);
    wait_until("the router to count the frames", || {
        sink.shared.probe.turns.load(Ordering::Acquire) >= turns + 2
    });
    let dropped_at_loss = get(&stats.disconnected_dropped);
    assert!(dropped_at_loss >= 1, "the frames of the failed session");
    assert!(
        dropped_at_loss <= u64::from(offered),
        "never more than offered"
    );
    assert_eq!(get(&stats.queue_dropped), 0);
    // Afterwards each frame is counted exactly once, at once, with no wait.
    for number in 0..10 {
        sink.write_frame(&sized_frame(offered + number, 4096))
            .expect("drop, not error");
        sink.flush().expect("block is sent");
    }
    wait_until("the new frames to be counted", || {
        get(&stats.disconnected_dropped) == dropped_at_loss + 10
    });
    sink.finish().expect("shutdown while disconnected is clean");
}
