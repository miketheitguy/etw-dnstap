//! Tests for the router's own rules, on a router that is not running: where each
//! block goes, the spill of a full FIFO, the replay credit, and how the blocks
//! that come back from the link are placed. The threads together are tested in
//! `fault_tests.rs`.

use std::{sync::mpsc, thread, time::Duration};

use crossbeam_channel::bounded;

use super::*;
use crate::{
    sender::{
        REPLAY_CREDIT,
        link::{LinkThread, Step},
        link_channels,
        script::{Script, ScriptUplink, complete_frames},
        sender_tests::{MIB, get, open_spool, sized_frame},
    },
    spool::test_support::{TestDir, capture_logs_at, framed},
};

/// A router that is not running, with the link thread's ends of its channels.
struct Rig {
    router: Router,
    fifo: Receiver<Item>,
    fences: Receiver<()>,
    returns: Sender<Return>,
    stats: Arc<SinkStats>,
    shared: Arc<LinkShared>,
}

fn rig(spool: Option<Spool>, stats: &Arc<SinkStats>, up: bool) -> Rig {
    let free = Arc::new(ArrayQueue::new(super::super::POOL_BLOCKS));
    for _ in 0..16 {
        release(&free, Block::default());
    }
    let shared = Arc::new(LinkShared::new(up));
    let (ports, link_ports) = link_channels(LINK_FIFO_BLOCKS);
    let router = Router::new(
        spool,
        Arc::clone(stats),
        free,
        Arc::clone(&shared),
        ports,
        thread::spawn(|| Ok(())),
    );
    Rig {
        router,
        fifo: link_ports.fifo,
        fences: link_ports.fences,
        returns: link_ports.returns,
        stats: Arc::clone(stats),
        shared,
    }
}

/// A count of blocks as a sequence number.
fn blocks(count: usize) -> u32 {
    u32::try_from(count).expect("small")
}

/// A block of one frame: a 4-byte length, then the sequence number.
fn block(number: u32) -> Block {
    let mut bytes = 4_u32.to_be_bytes().to_vec();
    bytes.extend_from_slice(&number.to_be_bytes());
    Block { bytes, frames: 1 }
}

fn numbers(blocks: impl IntoIterator<Item = Block>) -> Vec<u32> {
    blocks
        .into_iter()
        .flat_map(|block| complete_frames(&block.bytes))
        .collect()
}

/// Reads everything the spool holds, as sequence numbers, oldest first.
fn drain_spool(spool: &mut Spool) -> Vec<u32> {
    let mut all = Vec::new();
    loop {
        let mut out = Vec::new();
        let frames = spool.read_batch(&mut out, 1 << 20).expect("batch is read");
        if frames == 0 {
            return all;
        }
        all.extend(complete_frames(&out));
    }
}

fn fifo_numbers(fifo: &Receiver<Item>) -> Vec<u32> {
    let mut all = Vec::new();
    while let Ok(item) = fifo.try_recv() {
        match item {
            Item::Live(block) | Item::Replay(block) => all.extend(numbers([block])),
        }
    }
    all
}

fn front_numbers(router: &Router) -> Vec<u32> {
    router
        .front
        .iter()
        .flat_map(|block| complete_frames(&block.bytes))
        .collect()
}

fn spills(shared: &LinkShared) -> usize {
    shared.probe.spills.load(Ordering::Acquire)
}

#[test]
fn routing_follows_the_backlog_the_link_and_the_front_list() {
    let stats = Arc::new(SinkStats::default());
    // No spool: the link while it is up, a counted discard while it is down.
    assert_eq!(rig(None, &stats, true).router.destination(), Route::Link);
    assert_eq!(
        rig(None, &stats, false).router.destination(),
        Route::Discard
    );

    // A spool without a backlog: the link while it is up. A block waits in the
    // spool behind the front list, behind a replay batch that is out, and while
    // the link is down.
    let empty_dir = TestDir::new("routing-empty");
    let mut empty = rig(Some(open_spool(&empty_dir, 8 * MIB, &stats)), &stats, true);
    assert_eq!(empty.router.destination(), Route::Link);
    empty.router.front.push_back(block(1));
    assert_eq!(empty.router.destination(), Route::Spool);
    empty.router.front.clear();
    empty.router.replay_out = 1;
    assert_eq!(empty.router.destination(), Route::Spool);
    let down_dir = TestDir::new("routing-spool-down");
    let down = rig(Some(open_spool(&down_dir, 8 * MIB, &stats)), &stats, false);
    assert_eq!(down.router.destination(), Route::Spool);

    // A backlog keeps every block in the spool, up or down.
    let dir = TestDir::new("routing");
    let mut spool = open_spool(&dir, 8 * MIB, &stats);
    assert_eq!(spool.append(&[0, 0, 0, 1, 9], 1), Appended::Stored);
    let backlog = rig(Some(spool), &stats, true);
    assert_eq!(backlog.router.destination(), Route::Spool);
    let dir_down = TestDir::new("routing-down");
    let mut down_spool = open_spool(&dir_down, 8 * MIB, &stats);
    assert_eq!(down_spool.append(&[0, 0, 0, 1, 9], 1), Appended::Stored);
    assert_eq!(
        rig(Some(down_spool), &stats, false).router.destination(),
        Route::Spool
    );
}

#[test]
fn a_block_the_spool_refuses_during_a_backlog_is_counted_once_as_spool_dropped() {
    let dir = TestDir::new("refused");
    let stats = Arc::new(SinkStats::default());
    // Room for one tiny block, not for the large one below.
    let mut spool = open_spool(&dir, 400, &stats);
    assert_eq!(spool.append(&[0, 0, 0, 1, 9], 1), Appended::Stored);
    let mut rig = rig(Some(spool), &stats, true);

    let mut refused = Block::default();
    refused.bytes.resize(1000, 0);
    refused.frames = 5;
    // The link is up, but the backlog forces the block to the spool, which
    // refuses it and counts its frames.
    rig.router.route_live(refused);
    assert_eq!(get(&stats.spool_dropped), 5);
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert_eq!(get(&stats.queue_dropped), 0);
    assert!(rig.fifo.is_empty(), "nothing was offered to the link");
}

#[test]
fn a_full_fifo_spills_to_the_spool_oldest_first_and_the_new_block_follows() {
    let dir = TestDir::new("fifo-spill");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(open_spool(&dir, 8 * MIB, &stats)), &stats, true);
    // The FIFO takes its capacity, and the spool takes nothing.
    for number in 0..blocks(LINK_FIFO_BLOCKS) {
        rig.router.route_live(block(number));
    }
    assert_eq!(rig.fifo.len(), LINK_FIFO_BLOCKS);
    assert_eq!(get(&stats.spooled), 0);
    assert_eq!(spills(&rig.shared), 0);
    assert_eq!(rig.router.destination(), Route::Link);
    // The next block finds the FIFO full. The router takes the whole FIFO to the
    // spool, oldest first, and then this block.
    rig.router.route_live(block(blocks(LINK_FIFO_BLOCKS)));
    assert!(rig.fifo.is_empty(), "the spill left the FIFO empty");
    assert_eq!(spills(&rig.shared), 1);
    assert_eq!(get(&stats.spooled), LINK_FIFO_BLOCKS as u64 + 1);
    assert_eq!(rig.router.destination(), Route::Spool);
    let mut spool = rig.router.spool.take().expect("spool");
    assert_eq!(
        drain_spool(&mut spool),
        (0..=blocks(LINK_FIFO_BLOCKS)).collect::<Vec<_>>()
    );
    assert_eq!(get(&stats.disconnected_dropped), 0);
}

#[test]
fn the_spill_waits_for_the_take_lock_so_the_link_cannot_slip_a_block_past_it() {
    let dir = TestDir::new("spill-lock");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(open_spool(&dir, 8 * MIB, &stats)), &stats, true);
    for number in 0..blocks(LINK_FIFO_BLOCKS) {
        rig.router.route_live(block(number));
    }
    // The link thread holds the lock, as when it takes an item.
    let shared = Arc::clone(&rig.shared);
    let lock = shared.take.lock().expect("lock");
    let (started_sender, started) = mpsc::channel();
    let Rig { mut router, .. } = rig;
    let router_thread = thread::spawn(move || {
        started_sender.send(()).expect("test is waiting");
        router.route_live(block(blocks(LINK_FIFO_BLOCKS)));
        router
    });
    started.recv().expect("the router thread started");
    thread::sleep(Duration::from_millis(100));
    assert_eq!(get(&stats.spooled), 0, "the router waits for the lock");
    drop(lock);
    let mut finished = router_thread.join().expect("no panic");
    assert_eq!(get(&stats.spooled), LINK_FIFO_BLOCKS as u64 + 1);
    let mut spool = finished.spool.take().expect("spool");
    assert_eq!(
        drain_spool(&mut spool),
        (0..=blocks(LINK_FIFO_BLOCKS)).collect::<Vec<_>>()
    );
}

#[test]
fn a_full_send_that_finds_room_under_the_lock_sends_instead_of_spilling() {
    let dir = TestDir::new("spill-retry");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(open_spool(&dir, 8 * MIB, &stats)), &stats, true);
    for number in 0..blocks(LINK_FIFO_BLOCKS) {
        rig.router.route_live(block(number));
    }
    // The router saw the FIFO full, and then the link thread took an item.
    let shared = Arc::clone(&rig.shared);
    let lock = shared.take.lock().expect("lock");
    let _taken = rig.fifo.try_recv().expect("an item");
    let before = spills(&rig.shared);
    let Rig {
        mut router, fifo, ..
    } = rig;
    let router_thread = thread::spawn(move || {
        router.fifo_full(block(blocks(LINK_FIFO_BLOCKS)));
        router
    });
    drop(lock);
    let finished = router_thread.join().expect("no panic");
    assert_eq!(spills(&shared), before, "no spill");
    assert_eq!(get(&stats.spooled), 0);
    assert_eq!(fifo.len(), LINK_FIFO_BLOCKS, "the block went into the FIFO");
    assert_eq!(finished.destination(), Route::Link);
}

#[test]
fn without_a_spool_a_full_fifo_stops_intake_and_drops_nothing() {
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(None, &stats, true);
    for number in 0..blocks(LINK_FIFO_BLOCKS) {
        assert!(rig.router.can_take(), "block {number}");
        rig.router.route_live(block(number));
    }
    assert_eq!(rig.fifo.len(), LINK_FIFO_BLOCKS);
    assert!(
        !rig.router.can_take(),
        "the parser channel backs up instead"
    );
    assert_eq!(get(&stats.disconnected_dropped), 0);
    // Room in the FIFO lets intake resume.
    let _ = rig.fifo.try_recv().expect("an item");
    assert!(rig.router.can_take());
}

#[test]
fn the_front_list_goes_into_the_fifo_first_and_live_blocks_wait_in_the_spool_behind_it() {
    let dir = TestDir::new("front-first");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(open_spool(&dir, 8 * MIB, &stats)), &stats, true);
    rig.router.front.push_back(block(1));
    // A newer block cannot pass the front list: it goes to the spool.
    rig.router.route_live(block(2));
    assert!(rig.fifo.is_empty());
    assert_eq!(get(&stats.spooled), 1);
    assert!(rig.router.send_front());
    assert_eq!(fifo_numbers(&rig.fifo), vec![1]);
    assert!(rig.router.front.is_empty());
    let mut spool = rig.router.spool.take().expect("spool");
    assert_eq!(drain_spool(&mut spool), vec![2]);
}

#[test]
fn returned_blocks_without_a_backlog_become_the_head_of_the_spool_in_order() {
    let dir = TestDir::new("place-head");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(open_spool(&dir, 8 * MIB, &stats)), &stats, true);
    // The block the link had in hand, then the blocks that were in the FIFO.
    rig.router
        .older
        .extend([block(1), block(2), block(3), block(4), block(5)]);
    rig.router.replay_out = 1;
    rig.router.place_older();
    assert_eq!(rig.router.replay_out, 0);
    assert!(rig.router.older.is_empty() && rig.router.front.is_empty());
    let mut spool = rig.router.spool.take().expect("spool");
    assert_eq!(drain_spool(&mut spool), vec![1, 2, 3, 4, 5]);
    assert_eq!(get(&stats.disconnected_dropped), 0);
}

#[test]
fn returned_blocks_older_than_a_backlog_wait_in_the_front_list_in_order() {
    let dir = TestDir::new("place-front");
    let stats = Arc::new(SinkStats::default());
    let mut backlog = open_spool(&dir, 8 * MIB, &stats);
    assert_eq!(
        backlog.append(&framed(&[100_u32.to_be_bytes().to_vec()]), 1),
        Appended::Stored
    );
    let mut rig = rig(Some(backlog), &stats, true);
    // A block still on the front list from an earlier failure is older than none
    // of the blocks that just came back, and they are older than the backlog.
    rig.router.front.push_back(block(3));
    rig.router.older.extend([block(1), block(2)]);
    rig.router.place_older();
    assert_eq!(front_numbers(&rig.router), vec![1, 2, 3]);
    // The spool still holds only its backlog.
    let mut spool = rig.router.spool.take().expect("spool");
    assert_eq!(drain_spool(&mut spool), vec![100]);
    assert_eq!(get(&stats.disconnected_dropped), 0);
}

#[test]
fn without_a_spool_the_blocks_that_come_back_are_counted_once() {
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(None, &stats, true);
    rig.router
        .older
        .extend([block(1), block(2), block(3), block(5), block(6)]);
    rig.router.front.push_back(block(4));
    rig.router.place_older();
    assert_eq!(get(&stats.disconnected_dropped), 6);
    assert!(rig.router.older.is_empty() && rig.router.front.is_empty());
}

/// A spool of `count` frames of the maximum length: each alone crosses the batch
/// limit, so each replay batch holds one frame.
fn spool_of_big_frames(dir: &TestDir, stats: &Arc<SinkStats>, count: u32) -> Spool {
    let mut spool = open_spool(dir, 64 * MIB, stats);
    for number in 0..count {
        let frame = sized_frame(number, crate::dnstap::MAX_ENCODED_LENGTH);
        assert_eq!(spool.append(&framed(&[frame]), 1), Appended::Stored);
    }
    spool
}

#[test]
fn the_router_reads_two_replay_batches_and_a_third_only_when_the_link_reports_one_done() {
    let dir = TestDir::new("credit");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(spool_of_big_frames(&dir, &stats, 4)), &stats, true);
    let (_input, parser) = bounded::<Block>(1);

    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, REPLAY_CREDIT);
    // No third batch while two are out, however often the router runs.
    for _ in 0..3 {
        let _ = rig.router.serve(&parser, false);
    }
    assert_eq!(rig.fifo.len(), 2, "two batches at a time");
    let first = rig.fifo.try_recv().expect("the first batch");
    assert!(matches!(&first, Item::Replay(block) if block.frames == 1));

    rig.returns.send(Return::ReplayDone).expect("done");
    rig.router.collect_returns();
    assert_eq!(rig.router.replay_out, 1);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(fifo_numbers(&rig.fifo), vec![1, 2]);
    // Closing: no replay at all.
    rig.returns.send(Return::ReplayDone).expect("done");
    rig.returns.send(Return::ReplayDone).expect("done");
    rig.router.collect_returns();
    let _ = rig.router.serve(&parser, true);
    assert!(rig.fifo.is_empty());
}

#[test]
fn live_blocks_go_to_the_spool_until_the_last_replay_credit_is_back() {
    let dir = TestDir::new("leave-draining");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(spool_of_big_frames(&dir, &stats, 1)), &stats, true);
    let (_input, parser) = bounded::<Block>(1);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, 1);
    // The next read finds nothing and deletes the last segment, so the spool has
    // no backlog; but a replay batch is still out, and a live block must not be
    // sent ahead of nothing but behind it in the FIFO. It waits in the spool
    // until the credit is back.
    let _ = rig.router.serve(&parser, false);
    assert!(!rig.router.has_backlog());
    assert_eq!(rig.router.destination(), Route::Spool);
    rig.router.route_live(block(7));
    assert!(rig.router.has_backlog());
    rig.returns.send(Return::ReplayDone).expect("done");
    rig.router.collect_returns();
    assert_eq!(rig.router.replay_out, 0);
    // A backlog again: the router stays in Draining and replays the live block.
    assert!(rig.router.serve(&parser, false));
    let items: Vec<u32> = rig
        .fifo
        .try_iter()
        .flat_map(|item| numbers([item.into_block()]))
        .collect();
    assert_eq!(items.last(), Some(&7), "the live block follows the batch");
}

#[test]
fn the_router_reads_no_replay_batch_once_the_link_thread_has_cleared_up() {
    let dir = TestDir::new("replay-after-down");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(spool_of_big_frames(&dir, &stats, 3)), &stats, true);
    let (_input, parser) = bounded::<Block>(1);
    // The link thread cleared `up`, and the router has not noticed yet: a batch
    // read now would move the spool cursor and be counted without a write attempt.
    rig.shared.up.store(false, Ordering::Release);
    for _ in 0..3 {
        let _ = rig.router.serve(&parser, false);
    }
    assert!(rig.fifo.is_empty(), "no batch reached the FIFO");
    assert_eq!(rig.router.replay_out, 0);
    assert_eq!(get(&stats.replayed), 0, "the cursor did not move");
    // Once the link is up again, the same router reads from the first frame.
    rig.shared.up.store(true, Ordering::Release);
    assert!(rig.router.serve(&parser, false));
    let Item::Replay(first) = rig.fifo.try_recv().expect("the first batch") else {
        panic!("a replay batch");
    };
    assert_eq!(complete_frames(&first.bytes), vec![0]);
}

#[test]
fn the_hand_back_takes_the_fifo_places_the_blocks_in_order_and_then_sends_the_fence() {
    let dir = TestDir::new("hand-back");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(open_spool(&dir, 8 * MIB, &stats)), &stats, true);
    // Two blocks are in the FIFO when the link thread fails.
    rig.router.route_live(block(8));
    rig.router.route_live(block(9));
    // The link: returns the block in hand, says it is done, and waits for the
    // fence. It takes nothing from the FIFO.
    let fences = rig.fences.clone();
    let returns = rig.returns.clone();
    let link = thread::spawn(move || {
        returns.send(Return::Returned(block(1))).expect("returned");
        returns.send(Return::HandBackDone).expect("done");
        fences.recv_timeout(Duration::from_secs(30)).is_ok()
    });
    // The link cleared `up`: the router follows it.
    rig.shared.up.store(false, Ordering::Release);
    rig.router.follow_link();
    assert!(link.join().expect("link ends"), "the fence arrived");
    assert!(!rig.router.is_up());
    assert!(rig.fifo.is_empty());
    assert_eq!(get(&rig.stats.spooled), 3);
    let mut spool = rig.router.spool.take().expect("spool");
    assert_eq!(drain_spool(&mut spool), vec![1, 8, 9]);
}

#[test]
fn a_replay_batch_in_the_fifo_at_a_hand_back_is_counted_and_gives_its_credit_back() {
    let dir = TestDir::new("hand-back-replay");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(spool_of_big_frames(&dir, &stats, 4)), &stats, true);
    let (_input, parser) = bounded::<Block>(1);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, 2);
    // The link fails with both batches still in the FIFO.
    rig.returns.send(Return::HandBackDone).expect("done");
    rig.shared.up.store(false, Ordering::Release);
    rig.router.follow_link();
    assert_eq!(rig.router.replay_out, 0);
    assert_eq!(
        get(&stats.disconnected_dropped),
        2,
        "one frame in each batch"
    );
    assert!(rig.fifo.is_empty());
    assert!(rig.fences.try_recv().is_ok(), "the fence follows the drain");
}

#[test]
fn a_hand_back_with_a_backlog_keeps_the_returned_block_on_the_front_list() {
    let dir = TestDir::new("hand-back-front");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(open_spool(&dir, 8 * MIB, &stats)), &stats, true);
    for number in 0..blocks(LINK_FIFO_BLOCKS) {
        rig.router.route_live(block(number));
    }
    // The FIFO is full, so this block spills it: the spool holds 0..=128.
    rig.router.route_live(block(blocks(LINK_FIFO_BLOCKS)));
    assert!(rig.router.has_backlog());
    // The link thread fails holding block 99 (older than all of them).
    rig.returns
        .send(Return::Returned(block(99)))
        .expect("returned");
    rig.returns.send(Return::HandBackDone).expect("done");
    rig.shared.up.store(false, Ordering::Release);
    rig.router.follow_link();
    assert_eq!(front_numbers(&rig.router), vec![99]);
    assert_eq!(
        rig.shared.probe.front_max.load(Ordering::Acquire),
        FRONT_LIST_BLOCKS
    );
    let mut spool = rig.router.spool.take().expect("spool");
    assert_eq!(
        drain_spool(&mut spool),
        (0..=blocks(LINK_FIFO_BLOCKS)).collect::<Vec<_>>()
    );
    assert_eq!(get(&stats.disconnected_dropped), 0);
}

#[test]
fn a_link_thread_that_vanishes_sends_the_blocks_it_held_to_the_spool_and_stays_down() {
    let dir = TestDir::new("vanish");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(open_spool(&dir, 8 * MIB, &stats)), &stats, true);
    rig.router.route_live(block(1));
    rig.router.route_live(block(2));
    // The link thread's return sender goes away. The channel holds a message
    // first: the block that was in hand.
    rig.returns
        .send(Return::Returned(block(0)))
        .expect("returned");
    let Rig {
        mut router,
        returns,
        fifo,
        ..
    } = rig;
    drop(returns);
    router.collect_returns();
    assert!(router.is_gone());
    assert!(fifo.is_empty(), "the router took the FIFO");
    router.route_live(block(3));
    assert_eq!(router.destination(), Route::Spool);
    let mut spool = router.spool.take().expect("spool");
    assert_eq!(drain_spool(&mut spool), vec![0, 1, 2, 3]);
}

// ---- the deterministic batching test ----

/// Blocks that arrive between two runs of the router.
const ARRIVALS_PER_ROUTER_RUN: usize = 6;
/// Runs of the router between two runs of the link thread.
const ROUTER_RUNS_PER_LINK_RUN: usize = 20;
/// Rounds the test plays.
const ROUNDS: usize = 5;

/// Plays a scheduler that runs the router often and the link thread seldom: the
/// link thread loses `ROUTER_RUNS_PER_LINK_RUN` router periods (120 blocks, about
/// one Windows Server quantum at 41 MB/s) in each round. Returns the number of
/// spills and the blocks the receiver saw, with a FIFO of `fifo_blocks`.
///
/// The test thread runs both, one at a time, so the schedule is the same on every
/// machine and under any load.
fn play_scheduler(fifo_blocks: usize) -> (usize, Vec<u32>) {
    let dir = TestDir::new(&format!("batching-{fifo_blocks}"));
    let stats = Arc::new(SinkStats::default());
    let script = Script::new(true);
    let uplink = ScriptUplink::new(&script, &stats, None, true);
    let shared = Arc::new(LinkShared::new(false));
    let free = Arc::new(ArrayQueue::new(super::super::POOL_BLOCKS));
    for _ in 0..16 {
        release(&free, Block::default());
    }
    let (ports, link_ports) = link_channels(fifo_blocks);
    let mut link = LinkThread::new(
        uplink,
        link_ports,
        Arc::clone(&shared),
        Arc::clone(&stats),
        Arc::clone(&free),
        script.settings(Duration::from_secs(60)),
    );
    link.begin();
    assert!(shared.up.load(Ordering::Acquire));
    let mut router = Router::new(
        Some(open_spool(&dir, 64 * MIB, &stats)),
        Arc::clone(&stats),
        free,
        Arc::clone(&shared),
        ports,
        thread::spawn(|| Ok(())),
    );
    let (input, parser) = bounded::<Block>(SEND_QUEUE_BLOCKS);

    let mut number = 0;
    for _ in 0..ROUNDS {
        for _ in 0..ROUTER_RUNS_PER_LINK_RUN {
            // The router runs: it routes everything that arrived since it last ran.
            for _ in 0..ARRIVALS_PER_ROUTER_RUN {
                input.try_send(block(number)).expect("room in the channel");
                number += 1;
            }
            assert_eq!(router.turn(&parser), Turn::Moved);
            assert!(parser.is_empty(), "a router run empties the parser channel");
        }
        // The link thread runs: it writes everything the FIFO holds, and then
        // finds it empty.
        loop {
            match link.step() {
                Step::Worked => {}
                Step::Idle => break,
                Step::End(end) => panic!("the link thread ended: {end:?}"),
            }
        }
        assert!(
            router.fifo_end.is_empty(),
            "a link run empties the FIFO (nothing spilled: {})",
            spills(&shared) == 0
        );
    }
    drop(input);
    let seen = script.frames();
    (spills(&shared), seen)
}

#[test]
fn a_link_thread_that_loses_a_server_quantum_causes_no_spill_with_the_deep_fifo() {
    let (spills, seen) = play_scheduler(LINK_FIFO_BLOCKS);
    assert_eq!(spills, 0, "120 blocks fit in a FIFO of 128");
    let total = blocks(ROUNDS * ROUTER_RUNS_PER_LINK_RUN * ARRIVALS_PER_ROUTER_RUN);
    assert_eq!(seen, (0..total).collect::<Vec<_>>(), "all live, in order");
}

#[test]
fn the_same_schedule_spills_with_a_two_block_fifo() {
    // The collapse that review V2 measured: a FIFO that holds less than the link
    // thread's absence cannot keep the spool empty.
    let (spills, _) = play_scheduler(2);
    assert!(spills > 0, "a FIFO of 2 cannot hold 120 blocks");
}

/// Appends a block of `bytes` bytes to the spool of `router`, so its backlog grows.
fn grow_backlog(router: &mut Router, bytes: usize) {
    let spool = router.spool.as_mut().expect("the router has a spool");
    let mut frames = Vec::new();
    while frames.len() < bytes {
        frames.extend_from_slice(&[0, 0, 0, 4, 1, 2, 3, 4]);
    }
    let count = u32::try_from(frames.len() / 8).expect("small");
    assert_eq!(spool.append(&frames, count), Appended::Stored);
}

#[test]
fn the_router_sets_and_clears_link_saturated_from_the_spool_it_owns() {
    let dir = TestDir::new("observe-saturated");
    let stats = Arc::new(SinkStats::default());
    stats.connected.store(true, Ordering::Relaxed);
    let spool = open_spool(&dir, 64 * MIB, &stats);
    let mut router = rig(Some(spool), &stats, true).router;

    let logs = capture_logs_at(tracing::Level::INFO, || {
        // The backlog grows at every sample while the link is connected.
        for _ in 0..=super::super::monitor::TREND_SAMPLES {
            grow_backlog(&mut router, 4_096);
            router.monitor.force_due();
            router.sample_monitor();
        }
        assert!(stats.link_saturated.load(Ordering::Relaxed));
        assert!(
            get(&stats.spool_oldest_age_ms) < 5_000,
            "a new segment is young"
        );
        // Routing does not read the flag: the destination follows the backlog.
        assert_eq!(router.destination(), Route::Spool);

        // The link drains the backlog: the state clears at once.
        let mut out = Vec::new();
        let backlog = router.spool.as_mut().expect("spool");
        while backlog.read_batch(&mut out, 1 << 20).expect("read") > 0 {}
        assert!(!router.has_backlog());
        router.monitor.force_due();
        router.sample_monitor();
        assert!(!stats.link_saturated.load(Ordering::Relaxed));
        assert_eq!(get(&stats.spool_oldest_age_ms), 0);
    });
    assert_eq!(logs.count("dnstap link is saturated"), 1);
    assert_eq!(logs.count("dnstap link is no longer saturated"), 1);
}
