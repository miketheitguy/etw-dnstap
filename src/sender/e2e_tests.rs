//! End-to-end tests: frames go through `QueuedSink`, the sender thread, and the
//! spool to a fake receiver on the loopback interface.
//!
//! Every frame carries its sequence number in the first four payload bytes, so
//! the receiver's list shows order, loss, and repeats. The tests wait for
//! conditions (`wait_until`) and never rely on a sleep for correctness.

use super::*;
use crate::spool::{Appended, test_support::framed};

/// True when `numbers` is strictly increasing: in order, with no repeat.
fn strictly_increasing(numbers: &[u32]) -> bool {
    numbers.windows(2).all(|pair| pair[0] < pair[1])
}

/// The numbers in `0..total` that `received` lacks.
fn missing(received: &[u32], total: u32) -> Vec<u32> {
    (0..total)
        .filter(|number| !received.contains(number))
        .collect()
}

/// Frames in the receiver list that are not probes.
fn arrived_without_probes(server: &FakeServer, probes: &[u32]) -> usize {
    server
        .received
        .frames()
        .iter()
        .filter(|number| !probes.contains(number))
        .count()
}

#[test]
fn several_outages_and_reconnects_in_one_run_deliver_everything_that_was_spooled() {
    const ROUNDS: u32 = 3;
    let dir = TestDir::new("e2e-outages");
    let server = FakeServer::up();
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 64 * MIB, &stats);
    let mut sink = start_sink(&server, fast_policy(), Some(spool), &stats);
    assert!(stats.connected.load(Ordering::Relaxed));

    let mut next = 0_u32;
    // Frames that must arrive: sent while healthy and confirmed, or sent after the
    // sender saw the outage (the spool holds them).
    let mut must_arrive: Vec<u32> = Vec::new();
    // Frames sent while the sender did not know yet that the link was dead. They
    // may be lost, silently or counted, as with any TCP send.
    let mut probes: Vec<u32> = Vec::new();
    for _ in 0..ROUNDS {
        let healthy = next..next + 50;
        send(&mut sink, healthy.clone());
        must_arrive.extend(healthy);
        next += 50;
        // Probes of earlier rounds are in the receiver list too: count without them.
        wait_until("the healthy frames to arrive", || {
            arrived_without_probes(&server, &probes) >= must_arrive.len()
        });

        server.outage();
        wait_until("the sender to notice the outage", || {
            if !stats.connected.load(Ordering::Relaxed) {
                return true;
            }
            probes.push(next);
            send(&mut sink, next..next + 1);
            next += 1;
            false
        });

        // The link is down and the receiver refuses sessions: these go to disk.
        let spooled_before = get(&stats.spooled);
        let during = next..next + 40;
        send(&mut sink, during.clone());
        must_arrive.extend(during);
        next += 40;
        wait_until("the outage frames to be spooled", || {
            get(&stats.spooled) >= spooled_before + 40
        });

        server.serve_now();
        wait_until("the sender to reconnect", || {
            stats.connected.load(Ordering::Relaxed)
        });
        wait_until("the spooled frames to arrive", || {
            arrived_without_probes(&server, &probes) >= must_arrive.len()
        });
    }
    sink.finish().expect("clean STOP/FINISH");

    let got = server.received.frames();
    assert!(strictly_increasing(&got), "order or repeat: {got:?}");
    let delivered: Vec<u32> = got
        .iter()
        .copied()
        .filter(|number| !probes.contains(number))
        .collect();
    assert_eq!(delivered, must_arrive);
    // Every counted frame is missing at the receiver, and only probes can be.
    let lost_probes = probes.iter().filter(|number| !got.contains(number)).count() as u64;
    assert!(
        get(&stats.disconnected_dropped) <= lost_probes,
        "{} counted, {lost_probes} probes missing",
        get(&stats.disconnected_dropped)
    );
    assert!(get(&stats.disconnects) >= u64::from(ROUNDS));
    assert!(get(&stats.reconnects) >= u64::from(ROUNDS));
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.queue_dropped), 0);
    assert_eq!(
        get(&stats.replayed),
        get(&stats.spooled),
        "everything spooled was replayed"
    );
}

#[test]
fn an_outage_longer_than_the_cap_drops_the_oldest_frames_and_delivers_the_newest() {
    const FRAMES_PER_BLOCK: u32 = 10;
    const BLOCKS: u32 = 60;
    const TOTAL: u32 = FRAMES_PER_BLOCK * BLOCKS;
    let dir = TestDir::new("e2e-cap");
    let server = FakeServer::down();
    let stats = Arc::new(SinkStats::default());
    // About eight 5 KiB blocks fit, each in a segment of its own.
    let spool = open_spool(&dir, 40 * 1024, &stats);
    let mut sink = start_sink(&server, fast_policy(), Some(spool), &stats);

    for block in 0..BLOCKS {
        for number in block * FRAMES_PER_BLOCK..(block + 1) * FRAMES_PER_BLOCK {
            sink.write_frame(&sized_frame(number, 500))
                .expect("frame is queued");
        }
        sink.flush().expect("block is sent");
        // Keep the queue short: this is pacing, not what the test checks.
        wait_until("the sender to keep up", || queued(&sink) < 32);
    }
    wait_until("every block to reach the spool", || {
        get(&stats.spooled) == u64::from(TOTAL)
    });
    let dropped = get(&stats.spool_dropped);
    assert!(dropped > 0, "the cap evicted nothing");
    assert!(dropped < u64::from(TOTAL), "the cap evicted everything");
    assert_eq!(dropped % u64::from(FRAMES_PER_BLOCK), 0, "whole blocks go");
    assert!(get(&stats.spool_dropped_bytes) > 0);
    assert!(get(&stats.spool_bytes) <= 40 * 1024);

    server.serve_now();
    let kept = usize::try_from(u64::from(TOTAL) - dropped).expect("small");
    wait_until("the kept frames to arrive", || {
        server.received.len() >= kept
    });
    // New data follows the replayed frames.
    send(&mut sink, TOTAL..TOTAL + 10);
    wait_until("the live frames to arrive", || {
        server.received.len() >= kept + 10
    });
    sink.finish().expect("clean STOP/FINISH");

    let first_kept = u32::try_from(dropped).expect("small");
    assert_eq!(
        server.received.frames(),
        (first_kept..TOTAL + 10).collect::<Vec<_>>(),
        "the oldest frames are gone and the newest arrived in order"
    );
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert_eq!(get(&stats.queue_dropped), 0);
}

#[test]
fn a_restart_with_leftovers_and_a_cursor_inside_a_segment_resumes_after_the_cursor() {
    const FRAME_BYTES: usize = 100;
    const PER_BLOCK: u32 = 40;
    const BLOCKS: u32 = 3;
    const LEFTOVER: u32 = PER_BLOCK * BLOCKS;
    let dir = TestDir::new("e2e-restart");
    // A cap of 64 KiB makes 8 KiB segments: one block each.
    let cap = 64 * 1024;

    // The first run: spool three blocks, hand out part of the first, shut down.
    let consumed = {
        let first_stats = Arc::new(SinkStats::default());
        let mut spool = open_spool(&dir, cap, &first_stats);
        for block in 0..BLOCKS {
            let payloads: Vec<Vec<u8>> = (block * PER_BLOCK..(block + 1) * PER_BLOCK)
                .map(|number| sized_frame(number, FRAME_BYTES))
                .collect();
            assert_eq!(
                spool.append(&framed(&payloads), PER_BLOCK),
                Appended::Stored
            );
        }
        let mut out = Vec::new();
        let consumed = spool.read_batch(&mut out, 2500).expect("batch is read");
        assert!(
            (1..PER_BLOCK).contains(&consumed),
            "the batch must end inside the first segment, got {consumed}"
        );
        // The link took the batch; the stop saves that position.
        spool.delivered(out.len(), consumed);
        spool.seal().expect("segment is sealed");
        consumed
    };
    assert_eq!(
        segment_files(&dir).len(),
        usize::try_from(BLOCKS).expect("small")
    );

    // The second run: the receiver is up. The leftovers go first, then live data.
    let server = FakeServer::up();
    let second_stats = Arc::new(SinkStats::default());
    let second_spool = open_spool(&dir, cap, &second_stats);
    let mut sink = start_sink(&server, no_retry(), Some(second_spool), &second_stats);
    send(&mut sink, 1000..1010);
    let expected: Vec<u32> = (consumed..LEFTOVER).chain(1000..1010).collect();
    wait_until("the leftovers and the live frames to arrive", || {
        server.received.len() >= expected.len()
    });
    sink.finish().expect("clean STOP/FINISH");
    assert_eq!(server.received.frames(), expected);
    assert_eq!(
        get(&second_stats.replayed) + u64::from(consumed) - u64::from(LEFTOVER),
        get(&second_stats.spooled),
        "every leftover after the cursor, and every live frame that took the spool, was replayed"
    );
    assert_eq!(get(&second_stats.spool_dropped), 0);
    assert_eq!(get(&second_stats.spool_dropped_bytes), 0);
    assert_eq!(get(&second_stats.disconnected_dropped), 0);
}

#[test]
fn a_slowly_reading_receiver_makes_blocks_spool_and_then_drain_in_order() {
    let dir = TestDir::new("e2e-slow");
    let server = FakeServer::up();
    // The receiver pauses after every large frame, so the sender falls behind.
    server.received.slow.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 256 * MIB, &stats);
    let mut sink = start_sink(&server, no_retry(), Some(spool), &stats);

    // One frame this size fills a block. Offer frames until the spool has taken
    // some, then a few more, holding back only while the queue is nearly full.
    let mut offered = 0_u32;
    let mut after_spill = 0_u32;
    while after_spill < 50 {
        assert!(
            offered < 3000,
            "no block was spooled after {offered} frames"
        );
        wait_until("room in the send queue", || {
            queued(&sink) < SEND_QUEUE_BLOCKS - 8
        });
        sink.write_frame(&sized_frame(offered, MAX_ENCODED_LENGTH))
            .expect("frame is queued");
        offered += 1;
        if get(&stats.spooled) > 0 {
            after_spill += 1;
        }
    }
    sink.flush().expect("block is sent");

    let total = usize::try_from(offered).expect("small");
    wait_until("every frame to arrive", || server.received.len() >= total);
    sink.finish().expect("clean STOP/FINISH");
    assert_eq!(server.received.frames(), (0..offered).collect::<Vec<_>>());
    assert!(get(&stats.spooled) > 0, "blocks went through the spool");
    assert_eq!(get(&stats.replayed), get(&stats.spooled));
    assert_eq!(get(&stats.queue_dropped), 0);
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert_eq!(get(&stats.disconnects), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
}

#[test]
fn a_link_that_fails_mid_replay_loses_only_what_the_kernel_held_and_sends_nothing_twice() {
    const BACKLOG: u32 = 150;
    const LIVE: u32 = 10;
    const DELIVERED_BEFORE_HANG_UP: u32 = 20;
    let dir = TestDir::new("e2e-mid-replay");
    let server = FakeServer::down();
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 64 * MIB, &stats);
    let mut sink = start_sink(&server, fast_policy(), Some(spool), &stats);

    // A backlog of large frames, built while the receiver refuses sessions.
    for number in 0..BACKLOG {
        sink.write_frame(&sized_frame(number, 60_000))
            .expect("frame is queued");
        if number % 10 == 9 {
            wait_drained(&sink);
        }
    }
    sink.flush().expect("block is sent");
    wait_until("the backlog to be spooled", || {
        get(&stats.spooled) == u64::from(BACKLOG)
    });

    // The receiver comes back, takes some frames, and hangs up. The next
    // connection is served to the end.
    server
        .received
        .hang_up_after
        .store(u64::from(DELIVERED_BEFORE_HANG_UP), Ordering::Release);
    server.serve_now();
    wait_until("the link to fail during the replay", || {
        get(&stats.disconnects) >= 1
    });
    wait_until("the sender to reconnect", || {
        stats.connected.load(Ordering::Relaxed)
    });
    send(&mut sink, BACKLOG..BACKLOG + LIVE);
    let newest = BACKLOG + LIVE - 1;
    wait_until("the newest frame to arrive", || {
        server.received.frames().last() == Some(&newest)
    });
    sink.finish().expect("clean STOP/FINISH");

    let got = server.received.frames();
    let total = BACKLOG + LIVE;
    assert!(strictly_increasing(&got), "order or repeat: {got:?}");
    assert_eq!(
        got[..DELIVERED_BEFORE_HANG_UP as usize],
        (0..DELIVERED_BEFORE_HANG_UP).collect::<Vec<_>>()
    );
    // The failed batch is not lost: the spool is rewound to the frame after the
    // last one the socket took, and those frames are sent again. Only the frames
    // that the kernel had already accepted when the receiver hung up can be
    // missing. They are one run of consecutive frames after the last frame the
    // receiver read, and nothing counts them.
    let lost = missing(&got, total);
    if let (Some(lowest_lost), Some(highest_lost)) = (lost.first(), lost.last()) {
        assert_eq!(
            lost,
            (*lowest_lost..=*highest_lost).collect::<Vec<_>>(),
            "the lost frames are consecutive"
        );
        assert!(*lowest_lost >= DELIVERED_BEFORE_HANG_UP);
    }
    assert!(get(&stats.disconnects) >= 1);
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.queue_dropped), 0);
}

#[test]
fn a_run_stopped_in_the_middle_of_a_replay_leaves_a_cursor_the_next_run_resumes_from() {
    const FRAMES: u32 = 400;
    const PER_BLOCK: u32 = 10;
    const FRAME_BYTES: usize = 60_000;
    let dir = TestDir::new("e2e-resume");
    let cap = 256 * MIB;

    // The first run leaves 400 large frames, the first few already handed out.
    let consumed_first = {
        let first_stats = Arc::new(SinkStats::default());
        let mut spool = open_spool(&dir, cap, &first_stats);
        for block in 0..FRAMES / PER_BLOCK {
            let payloads: Vec<Vec<u8>> = (block * PER_BLOCK..(block + 1) * PER_BLOCK)
                .map(|number| sized_frame(number, FRAME_BYTES))
                .collect();
            assert_eq!(
                spool.append(&framed(&payloads), PER_BLOCK),
                Appended::Stored
            );
        }
        let mut out = Vec::new();
        let consumed = spool.read_batch(&mut out, 200_000).expect("batch is read");
        assert!(consumed > 0);
        spool.delivered(out.len(), consumed);
        spool.seal().expect("segment is sealed");
        consumed
    };

    // The second run connects, but the receiver never reads. The replay fills the
    // socket buffers, the receiver accepts no bytes for the (shortest) stall
    // timeout, the link fails, and the run ends with most of the backlog still on
    // disk.
    let server = FakeServer::up();
    server.received.paused.store(true, Ordering::Release);
    let second_stats = Arc::new(SinkStats::default());
    let second_spool = open_spool(&dir, cap, &second_stats);
    let second = start_sink_with_stall(
        &server,
        no_retry(),
        Some(second_spool),
        &second_stats,
        crate::network::MIN_STALL_TIMEOUT,
    );
    wait_until("the stalled replay to fail the link", || {
        get(&second_stats.disconnects) >= 1
    });
    second
        .finish()
        .expect("shutdown while disconnected is clean");
    let boundary = consumed_first + u32::try_from(get(&second_stats.replayed)).expect("small");
    assert!(
        boundary < FRAMES,
        "socket buffering absorbed the whole backlog"
    );
    assert!(!segment_files(&dir).is_empty(), "the backlog stays on disk");
    // The replay batches the socket did not take went back to the spool: nothing
    // is counted, and the stop saved the position after the last frame it took.
    assert_eq!(get(&second_stats.disconnected_dropped), 0);
    server.received.paused.store(false, Ordering::Release);

    // The third run resumes at the cursor: everything from `boundary` on arrives
    // once and in order, and nothing before `consumed_first` is ever sent.
    let third_stats = Arc::new(SinkStats::default());
    let third_spool = open_spool(&dir, cap, &third_stats);
    let mut third = start_sink(&server, fast_policy(), Some(third_spool), &third_stats);
    send(&mut third, 5000..5010);
    let expected: Vec<u32> = (boundary..FRAMES).chain(5000..5010).collect();
    wait_until("the rest of the backlog to arrive", || {
        server
            .received
            .frames()
            .iter()
            .filter(|number| **number >= boundary)
            .count()
            >= expected.len()
    });
    third.finish().expect("clean STOP/FINISH");

    let got = server.received.frames();
    assert!(strictly_increasing(&got), "order or repeat");
    assert!(got.iter().all(|number| *number >= consumed_first));
    let tail: Vec<u32> = got
        .iter()
        .copied()
        .filter(|number| *number >= boundary)
        .collect();
    assert_eq!(tail, expected);
    assert_eq!(
        get(&third_stats.replayed) - get(&third_stats.spooled),
        u64::from(FRAMES - boundary),
        "the third run replayed exactly what the earlier runs had not consumed"
    );
    assert_eq!(get(&third_stats.spool_dropped), 0);
    assert_eq!(get(&third_stats.disconnected_dropped), 0);
}

#[test]
fn blocks_queue_while_the_receiver_is_slow_to_accept_and_then_all_arrive_in_order() {
    let dir = TestDir::new("e2e-slow-accept");
    let server = FakeServer::up();
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 64 * MIB, &stats);
    let mut sink = start_sink(&server, fast_policy(), Some(spool), &stats);

    send(&mut sink, 0..50);
    let mut next = 50_u32;
    let mut probes: Vec<u32> = Vec::new();
    let mut must_arrive: Vec<u32> = (0..50).collect();
    wait_until("the first frames to arrive", || server.received.len() >= 50);

    server.outage();
    wait_until("the sender to notice the outage", || {
        if !stats.connected.load(Ordering::Relaxed) {
            return true;
        }
        probes.push(next);
        send(&mut sink, next..next + 1);
        next += 1;
        false
    });

    // The receiver comes back but answers the handshake only after 700 ms. The
    // blocks sent meanwhile wait in the spool, not in a full queue.
    server
        .received
        .accept_delay_ms
        .store(700, Ordering::Release);
    server.serve_now();
    for _ in 0..20 {
        let block = next..next + 10;
        send(&mut sink, block.clone());
        must_arrive.extend(block);
        next += 10;
    }
    assert!(
        !stats.connected.load(Ordering::Relaxed),
        "the receiver answered before the blocks were queued"
    );
    wait_until("every frame to arrive", || {
        arrived_without_probes(&server, &probes) >= must_arrive.len()
    });
    sink.finish().expect("clean STOP/FINISH");

    let got = server.received.frames();
    assert!(strictly_increasing(&got), "order or repeat: {got:?}");
    let delivered: Vec<u32> = got
        .iter()
        .copied()
        .filter(|number| !probes.contains(number))
        .collect();
    assert_eq!(delivered, must_arrive);
    assert!(get(&stats.spooled) >= 200, "the blocks went to the spool");
    assert_eq!(get(&stats.queue_dropped), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
}
