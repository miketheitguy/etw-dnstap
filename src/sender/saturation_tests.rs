//! Characterization tests for a receiver that stays connected but is slow or
//! stalled. They pin down what the sender does today, so a later change can flip
//! single assertions on purpose.
//!
//! The sender thread does every socket write and every spool append. While one
//! socket write blocks, it routes no block, so it cannot spill. Each assertion
//! that the planned changes alter carries a comment that starts with
//! `Changes with H1:` (the sender thread no longer waits on the socket) or
//! `Changes with H2:` (a session ends only after a long stall with no progress),
//! and says the expected new value.
//!
//! Every frame carries its sequence number in its first four bytes. The large
//! frames (`MAX_ENCODED_LENGTH`) each fill one block, so a block holds exactly one
//! frame and the block counters equal the frame counters. The tests wait for
//! conditions. Two of them find the stall with `stall_the_sender`, as the
//! existing tests do: the queue stays long for 100 ms while the receiver is
//! paused. They also assume that the test thread is not held up for about a
//! second while it holds a stall, which would let the 2-second write deadline
//! end the session early. The session-loss test offers frames until the session
//! ends instead, so it needs no stall detector.

use super::{
    sender_tests::{
        FakeServer, MIB, fast_policy, get, no_retry, open_spool, queued, send, sized_frame,
        stall_the_sender, start_sink, wait_until,
    },
    *,
};
use crate::{network::TEST_IO_TIMEOUT, spool::test_support::TestDir};

/// Spool size that holds every frame these tests offer many times over.
const SPOOL_MIB: u64 = 64;
/// The bytes of one large frame in a block: the payload and its 4-byte length.
const BLOCK_BYTES: u64 = MAX_ENCODED_LENGTH as u64 + 4;

/// True when `numbers` is strictly increasing: in order, with no repeat.
fn strictly_increasing(numbers: &[u32]) -> bool {
    numbers.windows(2).all(|pair| pair[0] < pair[1])
}

/// Frames lost for any reason, as the counters report them.
fn counted_lost(stats: &SinkStats) -> u64 {
    get(&stats.queue_dropped) + get(&stats.disconnected_dropped) + get(&stats.spool_dropped)
}

/// Offers one large frame numbered `number`.
fn offer_large(sink: &mut QueuedSink, number: u32) {
    sink.write_frame(&sized_frame(number, MAX_ENCODED_LENGTH))
        .expect("a full queue is not an error");
}

fn resume(server: &FakeServer) {
    server.received.paused.store(false, Ordering::Release);
}

#[test]
fn a_stalled_write_overflows_the_queue_while_the_spool_has_room() {
    let dir = TestDir::new("sat-queue");
    let server = FakeServer::up();
    // The receiver keeps the connection but reads nothing, so the kernel buffers
    // fill and the sender thread blocks in a socket write. A receiver that reads
    // very slowly blocks the sender thread the same way, for as long as one
    // chunk takes; this is the limiting case.
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, SPOOL_MIB * MIB, &stats);
    let mut sink = start_sink(&server, no_retry(), Some(spool), &stats);

    let mut offered = stall_the_sender(&mut sink, 0);
    // The parser goes on producing. The sender thread is blocked, so it takes no
    // block from the queue and spills none.
    while get(&stats.queue_dropped) == 0 {
        assert!(offered < 3000, "no drop after {offered} frames");
        offer_large(&mut sink, offered);
        offered += 1;
    }
    let first_drop = get(&stats.queue_dropped);
    for _ in 0..50 {
        offer_large(&mut sink, offered);
        offered += 1;
    }

    // The stall has lasted well under the 2-second write deadline: the session
    // is intact and nothing has gone to the spool, which is empty.
    assert_eq!(get(&stats.disconnects), 0);
    assert_eq!(queued(&sink), SEND_QUEUE_BLOCKS, "the queue is full");
    // Each dropped block held one frame, and every frame offered after the queue
    // filled was dropped: the first offer to a full queue, then the 50 more.
    assert_eq!(first_drop, 1);
    // Changes with H1: queue_dropped == 0 here. The sender thread keeps routing
    // while the link write blocks, so it spills the queue to the spool instead.
    assert_eq!(get(&stats.queue_dropped), first_drop + 50);
    // Changes with H1: spooled > 0 here, the blocks the link writer could not take.
    assert_eq!(get(&stats.spooled), 0);
    assert_eq!(get(&stats.spool_bytes), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.spool_errors), 0);
    assert!(
        stats.spool_available.load(Ordering::Relaxed),
        "the spool is in use and accepting data"
    );
    // The spool had room for everything that was dropped, many times over.
    assert!((first_drop + 50) * BLOCK_BYTES < SPOOL_MIB * MIB);

    // The receiver reads again before the deadline: no session was lost. The
    // blocks that waited in the queue now go to the spool (the queue is deep),
    // then replay in order.
    resume(&server);
    let dropped = get(&stats.queue_dropped);
    let delivered = u64::from(offered) - dropped;
    let delivered_count = usize::try_from(delivered).expect("small");
    wait_until("every queued frame to arrive", || {
        server.received.len() >= delivered_count
    });
    // The monitor shows the stall: the send queue was full when the sender
    // thread came back, and the blocked write is the longest of the window.
    // Wait for the stall's own values: a peak from before the stall (the queue
    // stood at 10 blocks or more when it began) would end a wait for any value.
    wait_until("the monitor to publish the full queue", || {
        get(&stats.send_queue_depth_max) == SEND_QUEUE_BLOCKS as u64
    });
    wait_until("the monitor to publish the blocked write", || {
        get(&stats.write_max_recent_us) >= 50_000
    });
    assert!(get(&stats.write_max_recent_us) <= get(&stats.write_max_us));
    assert!(get(&stats.bytes_sent) > 0);
    sink.finish().expect("clean STOP/FINISH");
    // The dropped frames are the newest ones: the queue refused them after it
    // filled. Everything older arrived, in order.
    assert_eq!(
        server.received.frames(),
        (0..u32::try_from(delivered).expect("small")).collect::<Vec<_>>()
    );
    // Changes with H1: the spool absorbs the queue, so queue_dropped stays 0 and
    // delivered == offered. The exact spooled count then depends on the design.
    assert_eq!(
        get(&stats.spooled),
        SEND_QUEUE_BLOCKS as u64,
        "the full queue was spilled once the write returned"
    );
    assert_eq!(get(&stats.replayed), get(&stats.spooled));
    assert_eq!(get(&stats.disconnects), 0);
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
    // Every offered frame is accounted exactly once.
    assert_eq!(
        delivered + counted_lost(&stats),
        u64::from(offered),
        "delivered + queue_dropped + disconnected_dropped + spool_dropped == offered"
    );
}

#[test]
fn a_receiver_that_accepts_nothing_for_the_io_deadline_loses_the_session() {
    let dir = TestDir::new("sat-session");
    let server = FakeServer::up();
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, SPOOL_MIB * MIB, &stats);
    // A fast reconnect policy, so the session returns soon after the stall ends.
    let mut sink = start_sink(&server, fast_policy(), Some(spool), &stats);

    // The parser keeps offering large frames, a few queued at a time, until the
    // kernel buffers are full and a write blocks, and then until the write
    // deadline ends the session. The receiver never closes the connection.
    let started = Instant::now();
    let mut offered = 0_u32;
    wait_until("the stalled write to hit the I/O deadline", || {
        if get(&stats.disconnects) >= 1 {
            return true;
        }
        if queued(&sink) < 20 {
            assert!(offered < 3000, "kernel buffering absorbed {offered} frames");
            offer_large(&mut sink, offered);
            offered += 1;
        }
        false
    });
    let stalled_for = started.elapsed();
    assert!(
        stalled_for + Duration::from_millis(500) >= TEST_IO_TIMEOUT,
        "the session ended after only {stalled_for:?}"
    );
    // Changes with H2: no disconnect while the stall is shorter than the stall
    // timeout (10 to 30 s). disconnects stays 0 here, and this wait is for the
    // stall timeout instead of IO_TIMEOUT.
    assert_eq!(get(&stats.disconnects), 1);
    // The blocks that waited behind the stalled write find the link down and go
    // to the spool. Wait for the queue to empty before the receiver resumes.
    wait_until("the queued blocks to reach the spool", || {
        queued(&sink) == 0 && get(&stats.spooled) >= 1
    });
    assert_eq!(get(&stats.reconnects), 0, "the receiver is still paused");
    // Changes with H2: disconnected_dropped == 0. The write has no deadline of its
    // own, the frames are not abandoned, and the session continues.
    let in_flight = get(&stats.disconnected_dropped);
    assert!(
        in_flight >= 1,
        "the chunk that timed out is counted as dropped"
    );
    assert!(in_flight <= u64::from(offered));

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
    // Changes with H2: reconnects == 0, because the session was never lost.
    assert_eq!(get(&stats.reconnects), 1);
    assert_eq!(get(&stats.queue_dropped), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.replayed), get(&stats.spooled));
    // Nothing is lost silently: every frame the receiver lacks is in a counter.
    // An equality is not true here. The counter can exceed the loss: a write that
    // times out on Windows may still complete in the kernel, and the link counts
    // every frame written since its last flush as lost when the session fails,
    // although the kernel had accepted them. So the frame counted in
    // `disconnected_dropped` may also arrive, and delivered + counted can exceed
    // offered. The strongest true statement is a bound.
    let delivered = got.len() as u64;
    let missing = u64::from(offered) - delivered;
    assert!(
        missing <= counted_lost(&stats),
        "{missing} frames missing, {} counted",
        counted_lost(&stats)
    );
    assert!(delivered <= u64::from(offered), "no frame arrives twice");
}

#[test]
fn after_a_spill_every_later_block_takes_the_spool_path_until_the_backlog_is_empty() {
    let dir = TestDir::new("sat-delay-line");
    let server = FakeServer::up();
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, SPOOL_MIB * MIB, &stats);
    let mut sink = start_sink(&server, no_retry(), Some(spool), &stats);

    // The sender blocks on the receiver that stopped reading, and the queue grows
    // past the spill threshold without filling.
    let mut offered = stall_the_sender(&mut sink, 0);
    while queued(&sink) < SPILL_BLOCKS + 4 {
        offer_large(&mut sink, offered);
        offered += 1;
    }
    let behind_the_stall = queued(&sink);
    // Blocks the sender took off the queue before it blocked went to the link
    // directly: the queue was short then and the spool had no backlog.
    let direct = u64::from(offered) - behind_the_stall as u64;
    assert_eq!(get(&stats.spooled), 0);

    // The receiver reads again, but slower than the input: 5 ms per large frame.
    // The input is offered as fast as the queue accepts it.
    server.received.frame_delay_ms.store(5, Ordering::Release);
    resume(&server);
    for _ in 0..40 {
        wait_until("room in the send queue", || {
            queued(&sink) < SEND_QUEUE_BLOCKS - 8
        });
        offer_large(&mut sink, offered);
        offered += 1;
    }
    let total = usize::try_from(offered).expect("small");
    wait_until("every frame to arrive", || server.received.len() >= total);

    // Every block offered after the sender took the first one off a deep queue
    // went to the spool, including the 40 offered while the receiver was
    // connected and reading, because the backlog never emptied in between.
    // Changes with H1: the same count, but the first spill happens while the
    // link write blocks, so `direct` is what the link writer took before its FIFO
    // was deep. This test's setup needs a stall detector that does not rely on a
    // long queue then.
    assert_eq!(get(&stats.spooled), u64::from(offered) - direct);
    assert_eq!(get(&stats.replayed), get(&stats.spooled));
    assert_eq!(
        server.received.frames(),
        (0..offered).collect::<Vec<_>>(),
        "frames arrive in capture order"
    );
    // The receiver stayed connected, and nothing was lost.
    assert_eq!(get(&stats.disconnects), 0);
    assert_eq!(counted_lost(&stats), 0);

    // With the backlog gone the next block goes straight to the link again.
    // Changes with H3b only: a live-first policy sends new frames to the link
    // while the backlog drains, out of order.
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
