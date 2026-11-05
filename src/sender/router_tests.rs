//! Tests for the router's own rules, on a router that is not running: where each
//! block goes, the spill of a full FIFO, the replay credit, and how the blocks
//! that come back from the link are placed. The threads together are tested in
//! `fault_tests.rs`.

use std::{fs, sync::mpsc, thread, time::Duration};

use crossbeam_channel::bounded;

use super::*;
use crate::{
    sender::{
        REPLAY_CREDIT, REPLAY_TURN_BYTES,
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

/// What a crash right now would leave behind: a copy of the spool folder, opened
/// by a new run. Returns the first frame that run reads.
fn first_frame_after_a_crash(dir: &TestDir) -> Option<u32> {
    let copy = TestDir::new("crash-copy");
    for entry in fs::read_dir(dir.path()).expect("the folder is listed") {
        let entry = entry.expect("an entry");
        fs::copy(entry.path(), copy.path().join(entry.file_name())).expect("a file is copied");
    }
    let stats = Arc::new(SinkStats::default());
    let mut spool = open_spool(&copy, 64 * MIB, &stats);
    let mut out = Vec::new();
    spool.read_batch(&mut out, 1 << 20).expect("batch is read");
    complete_frames(&out).first().copied()
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
        let frames = spool
            .read_and_deliver(&mut out, 1 << 20)
            .expect("batch is read");
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

/// The bytes of a replay batch of one maximum-length frame.
const BIG_BATCH_BYTES: usize = crate::dnstap::MAX_ENCODED_LENGTH + 4;

/// A report that the link took a whole batch of one maximum-length frame.
fn big_batch_done() -> Return {
    Return::ReplayDone {
        bytes: BIG_BATCH_BYTES,
        frames: 1,
    }
}

#[test]
fn the_router_reads_up_to_the_credit_and_a_batch_more_only_as_the_link_reports_one_done() {
    const FRAMES: u32 = REPLAY_CREDIT as u32 + 3;
    let dir = TestDir::new("credit");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(
        Some(spool_of_big_frames(&dir, &stats, FRAMES)),
        &stats,
        true,
    );
    let (_input, parser) = bounded::<Block>(1);

    // One pass reads about 1 MiB: 15 batches of one maximum-length frame.
    let per_pass = REPLAY_TURN_BYTES.div_ceil(BIG_BATCH_BYTES);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(usize::from(rig.router.replay_out), per_pass);
    // The credit fills in a few passes and not beyond, however often the router runs.
    for _ in 0..20 {
        let _ = rig.router.serve(&parser, false);
    }
    assert_eq!(rig.router.replay_out, REPLAY_CREDIT);
    assert_eq!(rig.fifo.len(), usize::from(REPLAY_CREDIT));
    let first = rig.fifo.try_recv().expect("the first batch");
    assert!(matches!(&first, Item::Replay(block) if block.frames == 1));

    rig.returns.send(big_batch_done()).expect("done");
    rig.router.collect_returns();
    assert_eq!(rig.router.replay_out, REPLAY_CREDIT - 1);
    assert_eq!(get(&stats.replayed), 1, "replayed counts at delivery");
    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, REPLAY_CREDIT);
    let last = rig.fifo.try_iter().last().expect("the newest batch");
    assert_eq!(numbers([last.into_block()]), vec![u32::from(REPLAY_CREDIT)]);
    // Closing: no replay at all.
    for _ in 0..3 {
        rig.returns.send(big_batch_done()).expect("done");
    }
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
    // The spool keeps the segment while the batch is out, so it still has a
    // backlog, and a live block must not be sent ahead of the batch: it waits in
    // the spool until the credit is back.
    let _ = rig.router.serve(&parser, false);
    assert!(rig.router.has_backlog());
    assert!(!rig.router.has_unread());
    assert_eq!(rig.router.destination(), Route::Spool);
    rig.router.route_live(block(7));
    assert!(rig.router.has_unread());
    rig.returns.send(big_batch_done()).expect("done");
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
    // The link thread cleared `up`, and the router has not noticed yet. A batch
    // read now would only be read again after the hand-back, so reading it is
    // wasted work, not a loss; the router does not read it.
    rig.shared.up.store(false, Ordering::Release);
    for _ in 0..3 {
        let _ = rig.router.serve(&parser, false);
    }
    assert!(rig.fifo.is_empty(), "no batch reached the FIFO");
    assert_eq!(rig.router.replay_out, 0);
    assert_eq!(get(&stats.replayed), 0, "nothing was read or delivered");
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
fn replay_batches_in_the_fifo_at_a_hand_back_give_their_credit_back_and_are_read_again() {
    let dir = TestDir::new("hand-back-replay");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(spool_of_big_frames(&dir, &stats, 4)), &stats, true);
    let (_input, parser) = bounded::<Block>(1);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, 4);
    // The link fails with all four batches still in the FIFO.
    rig.returns.send(Return::HandBackDone).expect("done");
    rig.shared.up.store(false, Ordering::Release);
    rig.router.follow_link();
    assert_eq!(rig.router.replay_out, 0);
    assert_eq!(get(&stats.disconnected_dropped), 0, "nothing is lost");
    assert_eq!(get(&stats.replayed), 0);
    assert!(rig.fifo.is_empty());
    assert!(rig.fences.try_recv().is_ok(), "the fence follows the drain");
    // The link comes back: the same four frames are read again, in order.
    rig.shared.up.store(true, Ordering::Release);
    rig.router.follow_link();
    assert!(rig.router.serve(&parser, false));
    assert_eq!(fifo_numbers(&rig.fifo), vec![0, 1, 2, 3]);
}

#[test]
fn a_batch_that_the_link_counted_is_passed_without_counting_it_as_replayed() {
    let dir = TestDir::new("counted-batch");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(spool_of_big_frames(&dir, &stats, 3)), &stats, true);
    let (_input, parser) = bounded::<Block>(1);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, 3);
    // The link counted the first batch itself (no session) and says so.
    rig.returns.send(Return::ReplayCounted).expect("counted");
    rig.router.collect_returns();
    assert_eq!(rig.router.replay_out, 2);
    assert_eq!(get(&stats.replayed), 0, "a counted frame is not replayed");
    // A failed hand-back later reads the other two again, and not the first.
    rig.returns.send(Return::HandBackDone).expect("done");
    rig.shared.up.store(false, Ordering::Release);
    rig.router.follow_link();
    rig.shared.up.store(true, Ordering::Release);
    rig.router.follow_link();
    assert!(rig.router.serve(&parser, false));
    assert_eq!(fifo_numbers(&rig.fifo), vec![1, 2]);
}

#[test]
fn a_batch_that_cannot_enter_the_fifo_is_counted_once_and_not_read_again() {
    let dir = TestDir::new("offer-fails");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(spool_of_big_frames(&dir, &stats, 3)), &stats, true);
    // The router has no FIFO sender, so the offer fails (it cannot, in production).
    let saved = rig.router.fifo.take();
    assert_eq!(rig.router.replay_step(), ReplayStep::Sent(BIG_BATCH_BYTES));
    assert_eq!(rig.router.replay_out, 0);
    assert_eq!(get(&stats.disconnected_dropped), 1);
    assert_eq!(get(&stats.replayed), 0, "one counter only");
    // The next batch is the second frame: the counted one is not read again.
    rig.router.fifo = saved;
    assert_eq!(rig.router.replay_step(), ReplayStep::Sent(BIG_BATCH_BYTES));
    assert_eq!(fifo_numbers(&rig.fifo), vec![1]);
}

/// A spool of `count` frames of 1,000 bytes, numbered from 0. A replay batch
/// holds 65 of them.
fn spool_of_small_frames(dir: &TestDir, stats: &Arc<SinkStats>, count: u32) -> Spool {
    let mut spool = open_spool(dir, 64 * MIB, stats);
    for chunk in (0..count).collect::<Vec<_>>().chunks(50) {
        let frames: Vec<Vec<u8>> = chunk
            .iter()
            .map(|number| sized_frame(*number, 1_000))
            .collect();
        let added = u32::try_from(frames.len()).expect("small");
        assert_eq!(spool.append(&framed(&frames), added), Appended::Stored);
    }
    spool
}

/// Bytes of one frame of `spool_of_small_frames` on the wire.
const SMALL_FRAME_BYTES: usize = 1_004;
/// Frames in one replay batch of `spool_of_small_frames`.
const SMALL_PER_BATCH: u32 = 65;

#[test]
fn a_hand_back_with_a_cut_replay_batch_reads_again_from_the_cut_frame() {
    let dir = TestDir::new("hand-back-cut");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(
        Some(spool_of_small_frames(&dir, &stats, 4 * SMALL_PER_BATCH)),
        &stats,
        true,
    );
    let (_input, parser) = bounded::<Block>(1);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, 4);
    // The link takes the first batch and 10 frames of the second, then fails with
    // the other two in the FIFO.
    let _taken = [rig.fifo.try_recv(), rig.fifo.try_recv()];
    rig.returns
        .send(Return::ReplayDone {
            bytes: SMALL_PER_BATCH as usize * SMALL_FRAME_BYTES,
            frames: SMALL_PER_BATCH,
        })
        .expect("done");
    rig.returns
        .send(Return::ReplayDone {
            bytes: 10 * SMALL_FRAME_BYTES,
            frames: 10,
        })
        .expect("done");
    rig.returns.send(Return::HandBackDone).expect("done");
    rig.shared.up.store(false, Ordering::Release);
    rig.router.follow_link();
    assert_eq!(rig.router.replay_out, 0);
    assert_eq!(get(&stats.replayed), u64::from(SMALL_PER_BATCH) + 10);
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert!(rig.fifo.is_empty());
    // The next read starts at the cut frame.
    rig.shared.up.store(true, Ordering::Release);
    rig.router.follow_link();
    assert!(rig.router.serve(&parser, false));
    let batch = rig.fifo.try_recv().expect("a batch");
    let seen = numbers([batch.into_block()]);
    assert_eq!(seen.first(), Some(&(SMALL_PER_BATCH + 10)));
    assert!(strictly_consecutive(&seen));
}

fn strictly_consecutive(numbers: &[u32]) -> bool {
    numbers.windows(2).all(|pair| pair[1] == pair[0] + 1)
}

#[test]
fn a_hand_back_with_a_backlog_keeps_the_returned_block_on_the_front_list() {
    let dir = TestDir::new("hand-back-front");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(Some(open_spool(&dir, 8 * MIB, &stats)), &stats, true);
    for number in 0..blocks(LINK_FIFO_BLOCKS) {
        rig.router.route_live(block(number));
    }
    // The FIFO is full, so this block spills it: the spool holds all of them.
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

#[test]
fn the_router_saves_the_delivered_position_once_a_pass_has_applied_the_reports() {
    let dir = TestDir::new("checkpoint-pass");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(
        Some(spool_of_small_frames(&dir, &stats, 3 * SMALL_PER_BATCH)),
        &stats,
        true,
    );
    let (_input, parser) = bounded::<Block>(1);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, 3);
    let bytes = SMALL_PER_BATCH as usize * SMALL_FRAME_BYTES;
    let frames = SMALL_PER_BATCH;
    // Reading saves nothing: a crash now sends everything again.
    assert_eq!(first_frame_after_a_crash(&dir), Some(0));

    // A report alone saves nothing. The pass saves once, after it applied it.
    rig.returns
        .send(Return::ReplayDone { bytes, frames })
        .expect("done");
    rig.router.collect_returns();
    assert_eq!(first_frame_after_a_crash(&dir), Some(0));
    let _ = rig.router.turn(&parser);
    assert_eq!(first_frame_after_a_crash(&dir), Some(SMALL_PER_BATCH));

    // The next report is delivered but not saved until the next pass ends: a
    // crash in between repeats at most that batch.
    rig.returns
        .send(Return::ReplayDone { bytes, frames })
        .expect("done");
    rig.router.collect_returns();
    assert_eq!(first_frame_after_a_crash(&dir), Some(SMALL_PER_BATCH));
    let _ = rig.router.turn(&parser);
    assert_eq!(first_frame_after_a_crash(&dir), Some(2 * SMALL_PER_BATCH));
}

#[test]
fn a_hand_back_saves_the_delivered_position_before_the_router_goes_on() {
    let dir = TestDir::new("checkpoint-hand-back");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(
        Some(spool_of_small_frames(&dir, &stats, 4 * SMALL_PER_BATCH)),
        &stats,
        true,
    );
    let (_input, parser) = bounded::<Block>(1);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, 4);
    // The link takes the first batch and 10 frames of the second, and fails.
    let _taken = [rig.fifo.try_recv(), rig.fifo.try_recv()];
    rig.returns
        .send(Return::ReplayDone {
            bytes: SMALL_PER_BATCH as usize * SMALL_FRAME_BYTES,
            frames: SMALL_PER_BATCH,
        })
        .expect("done");
    rig.returns
        .send(Return::ReplayDone {
            bytes: 10 * SMALL_FRAME_BYTES,
            frames: 10,
        })
        .expect("done");
    rig.returns.send(Return::HandBackDone).expect("done");
    rig.shared.up.store(false, Ordering::Release);
    // No pass ends here: the hand-back itself saves the position.
    rig.router.follow_link();
    assert_eq!(first_frame_after_a_crash(&dir), Some(SMALL_PER_BATCH + 10));
}

#[test]
fn a_link_thread_that_vanishes_with_batches_out_gives_them_back_to_the_spool() {
    let dir = TestDir::new("vanish-replay");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(
        Some(spool_of_small_frames(&dir, &stats, 3 * SMALL_PER_BATCH)),
        &stats,
        true,
    );
    let (_input, parser) = bounded::<Block>(1);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, 3);
    // The link wrote the first batch, reported it, and vanished.
    let _taken = rig.fifo.try_recv();
    rig.returns
        .send(Return::ReplayDone {
            bytes: SMALL_PER_BATCH as usize * SMALL_FRAME_BYTES,
            frames: SMALL_PER_BATCH,
        })
        .expect("done");
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
    assert_eq!(get(&stats.disconnected_dropped), 0);
    // The router saved the position after the first batch before it went on.
    assert_eq!(first_frame_after_a_crash(&dir), Some(SMALL_PER_BATCH));
    let mut spool = router.spool.take().expect("spool");
    let rest = drain_spool(&mut spool);
    assert_eq!(
        rest,
        (SMALL_PER_BATCH..3 * SMALL_PER_BATCH).collect::<Vec<_>>()
    );
}

#[test]
fn the_router_seals_with_batches_out_and_the_next_run_reads_from_the_last_frame_taken() {
    let dir = TestDir::new("seal-replay");
    let stats = Arc::new(SinkStats::default());
    let mut rig = rig(
        Some(spool_of_small_frames(&dir, &stats, 3 * SMALL_PER_BATCH)),
        &stats,
        true,
    );
    let (_input, parser) = bounded::<Block>(1);
    assert!(rig.router.serve(&parser, false));
    assert_eq!(rig.router.replay_out, 3);
    // The link wrote the first batch and 7 frames of the second, then ended.
    let _taken = [rig.fifo.try_recv(), rig.fifo.try_recv()];
    rig.returns
        .send(Return::ReplayDone {
            bytes: SMALL_PER_BATCH as usize * SMALL_FRAME_BYTES,
            frames: SMALL_PER_BATCH,
        })
        .expect("done");
    rig.returns
        .send(Return::ReplayDone {
            bytes: 7 * SMALL_FRAME_BYTES,
            frames: 7,
        })
        .expect("done");
    let Rig {
        router, returns, ..
    } = rig;
    drop(returns);
    router.seal(&parser).expect("seal");
    assert_eq!(get(&stats.disconnected_dropped), 0, "nothing is counted");

    // The next run finds the rest of the second batch and the third.
    let next = Arc::new(SinkStats::default());
    let mut spool = open_spool(&dir, 64 * MIB, &next);
    assert_eq!(
        drain_spool(&mut spool),
        (SMALL_PER_BATCH + 7..3 * SMALL_PER_BATCH).collect::<Vec<_>>()
    );
}

#[test]
fn a_seal_with_a_backlog_writes_no_log_line_and_records_the_backlog() {
    use crate::spool::test_support::capture_logs_at;
    let dir = TestDir::new("seal-quiet");
    let stats = Arc::new(SinkStats::default());
    let rig = rig(
        Some(spool_of_small_frames(&dir, &stats, 3 * SMALL_PER_BATCH)),
        &stats,
        true,
    );
    let (_input, parser) = bounded::<Block>(1);
    let Rig {
        router, returns, ..
    } = rig;
    drop(returns);
    stats.begin_stop();
    let logs = capture_logs_at(tracing::Level::INFO, || {
        router.seal(&parser).expect("seal");
    });
    assert_eq!(logs.text(), "", "the service writes nothing at shutdown");
    assert!(stats.spool_backlog_kept.load(Ordering::Relaxed));
    assert_eq!(stats.spool_seal_error(), None);
}

#[test]
fn a_seal_with_no_spool_records_no_backlog() {
    let stats = Arc::new(SinkStats::default());
    let rig = rig(None, &stats, true);
    let (_input, parser) = bounded::<Block>(1);
    let Rig {
        router, returns, ..
    } = rig;
    drop(returns);
    router.seal(&parser).expect("seal");
    assert!(!stats.spool_backlog_kept.load(Ordering::Relaxed));
    assert_eq!(stats.spool_seal_error(), None);
}

// ---- the deterministic batching test ----

/// Blocks that arrive between two runs of the router.
const ARRIVALS_PER_ROUTER_RUN: usize = 6;
/// Runs of the router between two runs of the link thread: one Server quantum.
const ROUTER_RUNS_PER_LINK_RUN: usize = 20;
/// The same for two quanta (234 blocks).
const ROUTER_RUNS_PER_TWO_QUANTA: usize = 39;
/// Rounds the test plays.
const ROUNDS: usize = 5;

/// Plays a scheduler that runs the router often and the link thread seldom: the
/// link thread loses `runs_per_link_run` router periods in each round (20 are 120
/// blocks, about one Windows Server quantum at 41 MB/s). Returns the number of
/// spills and the blocks the receiver saw, with a FIFO of `fifo_blocks`.
///
/// The test thread runs both, one at a time, so the schedule is the same on every
/// machine and under any load.
fn play_scheduler(fifo_blocks: usize, runs_per_link_run: usize) -> (usize, Vec<u32>) {
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
        for _ in 0..runs_per_link_run {
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
    let (spills, seen) = play_scheduler(LINK_FIFO_BLOCKS, ROUTER_RUNS_PER_LINK_RUN);
    assert_eq!(spills, 0, "120 blocks fit in the FIFO");
    let total = blocks(ROUNDS * ROUTER_RUNS_PER_LINK_RUN * ARRIVALS_PER_ROUTER_RUN);
    assert_eq!(seen, (0..total).collect::<Vec<_>>(), "all live, in order");
}

#[test]
fn the_same_schedule_spills_with_a_two_block_fifo() {
    // The collapse that review V2 measured: a FIFO that holds less than the link
    // thread's absence cannot keep the spool empty.
    let (spills, _) = play_scheduler(2, ROUTER_RUNS_PER_LINK_RUN);
    assert!(spills > 0, "a FIFO of 2 cannot hold 120 blocks");
}

#[test]
fn a_link_thread_that_loses_two_server_quanta_causes_no_spill_with_the_deep_fifo() {
    let (spills, seen) = play_scheduler(LINK_FIFO_BLOCKS, ROUTER_RUNS_PER_TWO_QUANTA);
    assert_eq!(spills, 0, "234 blocks fit in the FIFO");
    let total = blocks(ROUNDS * ROUTER_RUNS_PER_TWO_QUANTA * ARRIVALS_PER_ROUTER_RUN);
    assert_eq!(seen, (0..total).collect::<Vec<_>>(), "all live, in order");
}

// ---- one router wake queues the whole credit ----

#[test]
fn one_router_wake_queues_the_whole_credit_and_the_link_drains_it_without_the_router() {
    const PER_BATCH: u32 = SMALL_PER_BATCH;
    let credit = usize::from(REPLAY_CREDIT);
    let dir = TestDir::new("one-wake");
    let stats = Arc::new(SinkStats::default());
    let script = Script::new(true);
    let uplink = ScriptUplink::new(&script, &stats, None, true);
    let shared = Arc::new(LinkShared::new(false));
    let free = Arc::new(ArrayQueue::new(super::super::POOL_BLOCKS));
    for _ in 0..super::super::POOL_BLOCKS - 1 {
        release(&free, Block::default());
    }
    let (ports, link_ports) = link_channels(LINK_FIFO_BLOCKS);
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
    // A backlog of three batches more than the credit.
    let spool = spool_of_small_frames(&dir, &stats, blocks(credit + 3) * PER_BATCH);
    let mut router = Router::new(
        Some(spool),
        Arc::clone(&stats),
        free,
        Arc::clone(&shared),
        ports,
        thread::spawn(|| Ok(())),
    );
    let (_input, parser) = bounded::<Block>(SEND_QUEUE_BLOCKS);

    // One pass reads about 1 MiB (16 batches) and asks to run again at once.
    assert_eq!(router.turn(&parser), Turn::Moved);
    assert_eq!(
        usize::from(router.replay_out),
        REPLAY_TURN_BYTES.div_ceil(usize::try_from(PER_BATCH).expect("small") * SMALL_FRAME_BYTES)
    );
    // The router runs again while it moves something. The link thread does not
    // run, and one wake queues the whole credit.
    let mut turns = 1;
    while router.turn(&parser) == Turn::Moved {
        turns += 1;
        assert!(turns <= 20, "the router keeps reading");
    }
    assert_eq!(router.replay_out, REPLAY_CREDIT);
    assert_eq!(router.fifo_end.len(), credit);
    assert!(
        turns >= credit.div_ceil(16),
        "a pass is bounded by its budget"
    );

    // The link thread drains every batch with no router pass between. It never
    // waits on the return channel, which has a place for each report.
    assert_eq!(router.returns.capacity(), Some(credit + 2));
    loop {
        assert!(
            router.returns.len() < credit + 2,
            "the return channel would block the link thread"
        );
        match link.step() {
            Step::Worked => {}
            Step::Idle => break,
            Step::End(end) => panic!("the link thread ended: {end:?}"),
        }
    }
    assert!(router.fifo_end.is_empty());
    assert_eq!(router.returns.len(), credit);
    assert_eq!(
        script.frames(),
        (0..blocks(credit) * PER_BATCH).collect::<Vec<_>>()
    );

    // The router takes the reports, the spool deletes what is delivered, and the
    // router reads the last three batches.
    assert_eq!(router.turn(&parser), Turn::Moved);
    assert_eq!(get(&stats.replayed), u64::from(blocks(credit) * PER_BATCH));
    // The last three batches, and a short one where the data crosses a segment.
    assert!(
        (3..=4).contains(&router.replay_out),
        "{}",
        router.replay_out
    );
    loop {
        match link.step() {
            Step::Worked => {}
            Step::Idle => break,
            Step::End(end) => panic!("the link thread ended: {end:?}"),
        }
    }
    let _ = router.turn(&parser);
    assert_eq!(router.replay_out, 0);
    assert!(!router.has_backlog(), "everything is delivered and deleted");
    assert_eq!(
        script.frames(),
        (0..blocks(credit + 3) * PER_BATCH).collect::<Vec<_>>()
    );
    assert_eq!(get(&shared.pool_fallbacks), 0, "no block beyond the pool");
    assert_eq!(get(&stats.disconnected_dropped), 0);
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
        while backlog.read_and_deliver(&mut out, 1 << 20).expect("read") > 0 {}
        assert!(!router.has_backlog());
        router.monitor.force_due();
        router.sample_monitor();
        assert!(!stats.link_saturated.load(Ordering::Relaxed));
        assert_eq!(get(&stats.spool_oldest_age_ms), 0);
    });
    assert_eq!(logs.count("dnstap link is saturated"), 1);
    assert_eq!(logs.count("dnstap link is no longer saturated"), 1);
}

// ---- a spool whose key is lost ----

/// A spool on `dir` with a key of its own (the test key's bytes and id), so a
/// test can break the key and a later run can still read the files.
fn spool_with_breakable_key(
    dir: &TestDir,
    stats: &Arc<SinkStats>,
) -> (Spool, Arc<crate::spool::crypt::SegmentKey>) {
    use crate::spool::{
        KeyOrigin, SpoolConfig,
        crypt::SegmentKey,
        test_support::{TEST_KEY_BYTES, TEST_KEY_ID},
    };
    let key =
        Arc::new(SegmentKey::from_bytes(&TEST_KEY_BYTES, TEST_KEY_ID).expect("the key page locks"));
    let spool = Spool::open(
        SpoolConfig {
            dir: dir.path().to_path_buf(),
            max_bytes: 8 * MIB,
            min_free_bytes: 0,
        },
        Arc::clone(&key),
        KeyOrigin::Loaded,
        Arc::clone(stats),
        |_| Ok(u64::MAX),
    )
    .expect("spool opens");
    (spool, key)
}

/// With the key lost, the backlog is frozen for the run: live frames go to the
/// link while it is up instead of waiting behind a backlog that is not read.
#[test]
fn with_a_lost_key_live_frames_go_to_the_link_while_it_is_up() {
    let dir = TestDir::new("key-lost-live");
    let stats = Arc::new(SinkStats::default());
    let (mut spool, key) = spool_with_breakable_key(&dir, &stats);
    assert_eq!(spool.append(&block(1).bytes, 1), Appended::Stored);
    let mut rig = rig(Some(spool), &stats, true);
    assert_eq!(rig.router.destination(), Route::Spool, "behind the backlog");

    key.zero_for_test();
    assert_eq!(rig.router.destination(), Route::Link);
    rig.router.route_live(block(2));
    assert_eq!(fifo_numbers(&rig.fifo), vec![2]);
    assert_eq!(get(&stats.spooled), 1, "nothing more is spooled");
    assert_eq!(get(&stats.spool_dropped), 0);
    assert!(
        !rig.router.has_unread(),
        "the frozen backlog is not replayed"
    );
}

/// With the key lost and the link down, live frames are refused by the spool
/// and counted in `spool_dropped`.
#[test]
fn with_a_lost_key_and_the_link_down_live_frames_are_refused_and_counted() {
    let dir = TestDir::new("key-lost-down");
    let stats = Arc::new(SinkStats::default());
    let (spool, key) = spool_with_breakable_key(&dir, &stats);
    let mut rig = rig(Some(spool), &stats, false);
    key.zero_for_test();
    assert_eq!(rig.router.destination(), Route::Spool);
    for number in 0..3 {
        rig.router.route_live(block(number));
    }
    assert_eq!(get(&stats.spool_dropped), 3);
    assert_eq!(get(&stats.spooled), 0);
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert!(rig.fifo.is_empty());
    assert_eq!(fs::read_dir(dir.path()).expect("listed").count(), 0);
}

/// With the key lost, a full FIFO stops intake, as with no spool: the router
/// does not spill the FIFO to a spool that refuses everything.
#[test]
fn with_a_lost_key_a_full_fifo_stops_intake_instead_of_spilling() {
    let dir = TestDir::new("key-lost-no-spill");
    let stats = Arc::new(SinkStats::default());
    let (spool, key) = spool_with_breakable_key(&dir, &stats);
    let mut rig = rig(Some(spool), &stats, true);
    key.zero_for_test();
    for number in 0..blocks(LINK_FIFO_BLOCKS) {
        assert!(rig.router.can_take(), "block {number}");
        rig.router.route_live(block(number));
    }
    assert_eq!(rig.fifo.len(), LINK_FIFO_BLOCKS);
    assert!(
        !rig.router.can_take(),
        "the parser channel backs up instead"
    );
    assert_eq!(spills(&rig.shared), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.disconnected_dropped), 0);
}

/// At shutdown a lost key is neither a seal error nor a spool error, and the
/// backlog it froze is reported as kept; the next run replays it.
#[test]
fn a_seal_with_a_lost_key_keeps_the_backlog_and_reports_no_error() {
    let dir = TestDir::new("key-lost-seal");
    let stats = Arc::new(SinkStats::default());
    let (mut spool, key) = spool_with_breakable_key(&dir, &stats);
    for number in 0..3 {
        assert_eq!(spool.append(&block(number).bytes, 1), Appended::Stored);
    }
    let rig = rig(Some(spool), &stats, true);
    key.zero_for_test();
    let (_input, parser) = bounded::<Block>(1);
    let Rig {
        router, returns, ..
    } = rig;
    drop(returns);
    stats.begin_stop();
    router.seal(&parser).expect("seal");
    assert!(stats.spool_backlog_kept.load(Ordering::Relaxed));
    assert_eq!(stats.spool_seal_error(), None);
    assert_eq!(get(&stats.spool_errors), 0);

    let next = Arc::new(SinkStats::default());
    let mut next_run = open_spool(&dir, 64 * MIB, &next);
    assert_eq!(drain_spool(&mut next_run), vec![0, 1, 2]);
}
