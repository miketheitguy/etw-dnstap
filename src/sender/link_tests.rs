//! Tests for the link thread alone, on a scripted transport and a hand-driven
//! FIFO: the byte rules, the stall timer, the take lock, and the hand-back
//! protocol. The router and the link thread together are tested in
//! `fault_tests.rs`.
//!
//! The stall tests use a manual clock, so the timer runs without real waiting
//! and the results do not depend on how busy the machine is.

use std::{
    ops::Range,
    thread::{self, JoinHandle},
};

use crossbeam_channel::RecvTimeoutError;

use super::*;
use crate::{
    network::test_support::TlsKit,
    sender::{
        LINK_FIFO_BLOCKS, POOL_BLOCKS, link_channels,
        script::{Mode, Script, ScriptUplink, trailing_partial},
        sender_tests::{get, wait_until},
    },
};

/// A block of one frame per number, each with `payload` bytes that start with
/// the number.
fn numbered_block(numbers: Range<u32>, payload: usize) -> Block {
    let mut bytes = Vec::new();
    let length = u32::try_from(payload).expect("small");
    for number in numbers.clone() {
        bytes.extend_from_slice(&length.to_be_bytes());
        let mut body = vec![0x5a; payload];
        body[..4].copy_from_slice(&number.to_be_bytes());
        bytes.extend_from_slice(&body);
    }
    Block {
        bytes,
        frames: u32::try_from(numbers.len()).expect("small"),
    }
}

fn numbers_in(block: &Block) -> Vec<u32> {
    crate::sender::script::complete_frames(&block.bytes)
}

/// The link thread on a scripted transport, with the router's ends of its
/// channels.
struct Rig {
    fifo: Option<Sender<Item>>,
    /// The router's own end of the FIFO, to see what the link left in it.
    fifo_end: Receiver<Item>,
    fences: Option<Sender<()>>,
    returns: Receiver<Return>,
    shared: Arc<LinkShared>,
    stats: Arc<SinkStats>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl Rig {
    fn new(script: &Arc<Script>, stall: Duration, tls: Option<Arc<TlsKit>>) -> Self {
        Self::with_hooks(script, stall, tls, LinkHooks::default())
    }

    fn with_hooks(
        script: &Arc<Script>,
        stall: Duration,
        tls: Option<Arc<TlsKit>>,
        hooks: LinkHooks,
    ) -> Self {
        let stats = Arc::new(SinkStats::default());
        let uplink = ScriptUplink::new(script, &stats, tls, true);
        let shared = Arc::new(LinkShared::new(true));
        let free = Arc::new(ArrayQueue::new(POOL_BLOCKS));
        for _ in 0..8 {
            release(&free, Block::default());
        }
        let (ports, link_ports) = link_channels(LINK_FIFO_BLOCKS);
        let mut settings = script.settings(stall);
        settings.hooks = hooks;
        let link = LinkThread::new(
            uplink,
            link_ports,
            Arc::clone(&shared),
            Arc::clone(&stats),
            free,
            settings,
        );
        Self {
            fifo: Some(ports.fifo),
            fifo_end: ports.fifo_end,
            fences: Some(ports.fences),
            returns: ports.returns,
            shared,
            stats,
            thread: Some(thread::spawn(move || link.run())),
        }
    }

    fn send(&self, item: Item) {
        self.fifo
            .as_ref()
            .expect("the FIFO is open")
            .send(item)
            .expect("the link thread takes items");
    }

    /// The router's answer to `HandBackDone`.
    fn fence(&self) {
        self.fences
            .as_ref()
            .expect("the fence channel is open")
            .send(())
            .expect("the link thread waits for the fence");
    }

    fn next_return(&self) -> Return {
        self.returns
            .recv_timeout(Duration::from_secs(30))
            .expect("a message from the link thread")
    }

    fn up(&self) -> bool {
        self.shared.up.load(Ordering::Acquire)
    }

    /// Closes the FIFO and the fence channel and waits for the link thread to end
    /// the stream.
    fn close(&mut self) -> Result<()> {
        drop(self.fifo.take());
        drop(self.fences.take());
        self.thread
            .take()
            .expect("running")
            .join()
            .expect("the link thread does not panic")
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        drop(self.fifo.take());
        drop(self.fences.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

// ---- the pure helpers ----

fn framed_lengths(lengths: &[usize]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for length in lengths {
        bytes.extend_from_slice(&u32::try_from(*length).expect("small").to_be_bytes());
        bytes.resize(bytes.len() + length, 7);
    }
    bytes
}

#[test]
fn frame_end_finds_each_frame_and_clamps_a_malformed_tail() {
    let bytes = framed_lengths(&[10, 3, 0]);
    assert_eq!(frame_end(&bytes, 0), 14);
    assert_eq!(frame_end(&bytes, 14), 21);
    assert_eq!(frame_end(&bytes, 21), 25);
    // A header that does not fit, a length past the end, and a start past the end
    // all end at the end of the bytes.
    assert_eq!(frame_end(&bytes[..2], 0), 2);
    assert_eq!(frame_end(&[0, 0, 1, 0, 9], 0), 5);
    assert_eq!(frame_end(&[0, 0, 0, 1, 9], 9), 5);
    assert_eq!(frame_end(&[255, 255, 255, 255, 1], 0), 5);
}

#[test]
fn a_piece_ends_at_a_frame_boundary_and_holds_whole_frames_up_to_the_limit() {
    let bytes = framed_lengths(&[6_000, 6_000, 6_000, 6_000, 100, 40_000, 8]);
    let mut scan = 0;
    // Plaintext has no limit: the rest of the block.
    assert_eq!(piece_end(&bytes, &mut scan, 0, None), bytes.len());
    let limit = Some(16 * 1024);
    // Two frames fit in 16 KiB, the third does not.
    assert_eq!(piece_end(&bytes, &mut scan, 0, limit), 2 * 6_004);
    // The next piece starts at the boundary.
    assert_eq!(
        piece_end(&bytes, &mut scan, 2 * 6_004, limit),
        4 * 6_004 + 104
    );
    // A frame longer than the limit is a piece of its own.
    let big = 4 * 6_004 + 104;
    assert_eq!(piece_end(&bytes, &mut scan, big, limit), big + 40_004);
    assert_eq!(
        piece_end(&bytes, &mut scan, big + 40_004, limit),
        bytes.len()
    );
}

#[test]
fn a_piece_that_starts_inside_a_frame_ends_with_that_frame_at_least() {
    // rustls took part of a frame, so the next piece starts inside it.
    let bytes = framed_lengths(&[10_000, 100]);
    let mut scan = 0;
    let limit = Some(4_000);
    let inside = 3_000;
    assert_eq!(piece_end(&bytes, &mut scan, inside, limit), 10_004);
    assert_eq!(scan, 0, "the scan stays at the frame that holds the offset");
    assert_eq!(piece_end(&bytes, &mut scan, 10_004, limit), bytes.len());
    assert_eq!(scan, 10_004);
}

#[test]
fn a_block_splits_into_sent_unknown_and_kept_frames_by_the_bytes_taken() {
    let bytes = framed_lengths(&[10, 10, 10, 10]);
    // 14 bytes per frame. Nothing taken: everything is kept.
    assert_eq!(
        split_block(&bytes, 0, 0),
        Split {
            sent: 0,
            unknown: 0,
            kept: 4,
            keep_from: 0
        }
    );
    // The socket took one frame and part of the second (plaintext: the two are
    // the same): the second is cut and kept whole.
    assert_eq!(
        split_block(&bytes, 20, 20),
        Split {
            sent: 1,
            unknown: 0,
            kept: 3,
            keep_from: 14
        }
    );
    // TLS: two frames confirmed, one fed but unconfirmed, one not fed.
    assert_eq!(
        split_block(&bytes, 28, 42),
        Split {
            sent: 2,
            unknown: 1,
            kept: 1,
            keep_from: 42
        }
    );
    // TLS: the piece ends inside a frame: that frame was not fully fed, so it is
    // kept, and it cannot have arrived whole.
    assert_eq!(
        split_block(&bytes, 28, 35),
        Split {
            sent: 2,
            unknown: 0,
            kept: 2,
            keep_from: 28
        }
    );
    // Everything taken: nothing is kept.
    assert_eq!(
        split_block(&bytes, 56, 56),
        Split {
            sent: 4,
            unknown: 0,
            kept: 0,
            keep_from: 56
        }
    );
}

// ---- writing ----

#[test]
fn a_live_block_and_a_replay_batch_are_written_whole_and_the_batch_reports_done() {
    let script = Script::new(false);
    let mut rig = Rig::new(&script, Duration::from_secs(60), None);
    let live = numbered_block(0..3, 100);
    let replay = numbered_block(3..5, 100);
    let total = (live.bytes.len() + replay.bytes.len()) as u64;
    rig.send(Item::Live(live));
    rig.send(Item::Replay(replay));
    assert!(matches!(rig.next_return(), Return::ReplayDone));
    assert_eq!(script.session_frames(0), vec![0, 1, 2, 3, 4]);
    assert_eq!(get(&rig.stats.bytes_sent), total);
    rig.close().expect("clean STOP");
    assert_eq!(script.finished(), Some(true));
}

#[test]
fn partial_plaintext_writes_deliver_every_frame_intact() {
    // The socket takes one byte, then would block, in turn.
    let script = Script::new(true);
    script.set_mode(Mode::Alternate { took_one: false });
    let mut rig = Rig::new(&script, Duration::from_secs(2), None);
    let block = numbered_block(0..20, 20);
    let length = block.bytes.len();
    rig.send(Item::Live(block));
    wait_until("every byte to be accepted", || script.wire_len(0) == length);
    assert_eq!(script.session_frames(0), (0..20).collect::<Vec<_>>());
    assert_eq!(get(&rig.stats.bytes_sent), length as u64);
    assert_eq!(get(&rig.stats.disconnects), 0);
    rig.close().expect("clean STOP");
}

#[test]
fn partial_tls_writes_deliver_every_frame_intact() {
    // The socket takes one byte, then would block, in turn: every record is
    // written in as many calls as it has bytes, and none is cut.
    let script = Script::new(true);
    script.set_mode(Mode::Alternate { took_one: false });
    let mut rig = Rig::new(&script, Duration::from_secs(2), Some(TlsKit::new()));
    let block = numbered_block(0..12, 3_000);
    rig.send(Item::Live(block));
    wait_until("every frame to be decrypted", || {
        script.session_frames(0).len() == 12
    });
    assert_eq!(script.session_frames(0), (0..12).collect::<Vec<_>>());
    assert_eq!(get(&rig.stats.disconnects), 0);
    // The bytes sent are the records' bytes: the plaintext and a header and tag
    // for each of the three pieces.
    let plain = 12 * 3_004;
    let sent = usize::try_from(get(&rig.stats.bytes_sent)).expect("small");
    assert!(sent > plain);
    assert_eq!(sent, script.wire_len(0));
    rig.close().expect("clean STOP");
}

#[test]
fn the_readiness_wait_of_a_block_is_recorded_as_a_slow_write() {
    // Real clock: the wait is real. The socket is blocked until the test opens it.
    let script = Script::new(false);
    script.set_mode(Mode::Blocked);
    let mut rig = Rig::new(&script, Duration::from_secs(60), None);
    rig.send(Item::Live(numbered_block(0..1, 100)));
    wait_until("a wait of 20 ms", || get(&rig.stats.write_max_us) >= 20_000);
    script.set_mode(Mode::open());
    wait_until("the block to be written", || script.wire_len(0) == 104);
    assert_eq!(get(&rig.stats.slow_writes), 1, "one block, counted once");
    assert!(get(&rig.stats.write_window_max_us) >= 20_000);
    rig.close().expect("clean STOP");
}

// ---- liveness ----

#[test]
fn one_byte_each_second_keeps_a_session_with_a_two_second_stall_timeout() {
    let script = Script::new(true);
    script.set_mode(Mode::Paced {
        period: Duration::from_secs(1),
        next: Duration::ZERO,
    });
    let rig = Rig::new(&script, Duration::from_secs(2), None);
    // Three frames of 8 bytes need 24 seconds of the manual clock at this pace.
    rig.send(Item::Live(numbered_block(0..3, 4)));
    wait_until("every byte to be accepted", || script.wire_len(0) == 24);
    assert!(script.elapsed() >= Duration::from_secs(23));
    assert_eq!(get(&rig.stats.disconnects), 0);
    assert_eq!(script.session_frames(0), vec![0, 1, 2]);

    // No bytes for the stall timeout ends the session, once.
    script.set_mode(Mode::Blocked);
    rig.send(Item::Live(numbered_block(3..4, 4)));
    wait_until("the stall to end the session", || {
        get(&rig.stats.disconnects) == 1
    });
    let gap = script
        .lost_at()
        .expect("the session ended")
        .saturating_sub(script.last_accept());
    assert!(
        gap >= Duration::from_secs(2) && gap < Duration::from_secs(2) + LINK_POLL,
        "the session ended {gap:?} after the last byte"
    );
    // The link gave nothing up silently: the block comes back.
    assert!(!rig.up());
    let Return::Returned(block) = rig.next_return() else {
        panic!("the block comes back");
    };
    assert_eq!(numbers_in(&block), vec![3]);
    assert!(matches!(rig.next_return(), Return::HandBackDone));
    assert_eq!(get(&rig.stats.disconnected_dropped), 0);
}

#[test]
fn an_idle_link_never_stalls() {
    let script = Script::new(true);
    // The socket takes one byte a second.
    script.set_mode(Mode::Paced {
        period: Duration::from_secs(1),
        next: Duration::ZERO,
    });
    let mut rig = Rig::new(&script, Duration::from_secs(2), None);
    // Ten minutes pass with nothing to write.
    script.advance(Duration::from_secs(600));
    // A block now starts its own timer. A timer that had run while the link was
    // idle would have expired, and the first wait would end the session.
    rig.send(Item::Live(numbered_block(0..3, 4)));
    wait_until("every byte to be accepted", || script.wire_len(0) == 24);
    assert_eq!(get(&rig.stats.disconnects), 0);
    rig.close().expect("clean STOP");
}

// ---- hand-back ----

#[test]
fn a_failure_returns_only_the_block_in_hand_and_leaves_the_fifo_to_the_router() {
    let script = Script::new(false);
    script.set_mode(Mode::Blocked);
    let rig = Rig::new(&script, Duration::from_secs(60), None);
    // In hand: three frames of 104 bytes. In the FIFO: a live block and a replay
    // batch.
    rig.send(Item::Live(numbered_block(10..13, 100)));
    wait_until("the link to take the block", || script.waits() >= 1);
    rig.send(Item::Live(numbered_block(20..22, 100)));
    rig.send(Item::Replay(numbered_block(30..33, 100)));
    // The socket takes one frame and 50 bytes of the next, then resets.
    script.set_mode(Mode::then_reset(104 + 50));
    wait_until("the link to clear up", || !rig.up());

    let Return::Returned(rest) = rig.next_return() else {
        panic!("the cut block comes back");
    };
    assert_eq!(numbers_in(&rest), vec![11, 12], "from the cut frame on");
    assert_eq!(rest.frames, 2);
    assert!(matches!(rig.next_return(), Return::HandBackDone));

    // The link took nothing else and counted nothing: the router takes the FIFO.
    assert_eq!(get(&rig.stats.disconnected_dropped), 0);
    assert_eq!(get(&rig.stats.disconnects), 1);
    let left: Vec<bool> = rig
        .fifo_end
        .try_iter()
        .map(|item| matches!(item, Item::Replay(_)))
        .collect();
    assert_eq!(left, vec![false, true], "the live block and the batch");
    // The receiver saw one whole frame and a truncated one.
    assert_eq!(script.session_frames(0), vec![10]);
    assert_eq!(script.trailing_partial(0), 50);
    // The session is gone. The link may reconnect, but it publishes `up` only
    // after the fence.
    assert!(!rig.up());
    script.allow_connect(true);
    wait_until("the link to reconnect", || script.sessions() == 2);
    assert!(!rig.up(), "up waits for the fence");
    rig.fence();
    wait_until("the link to come back up", || rig.up());
}

#[test]
fn a_cut_replay_batch_is_counted_from_the_cut_frame_and_never_returned() {
    let script = Script::new(false);
    script.set_mode(Mode::then_reset(104 + 10));
    let rig = Rig::new(&script, Duration::from_secs(60), None);
    rig.send(Item::Replay(numbered_block(0..4, 100)));
    wait_until("the link to clear up", || !rig.up());
    assert!(matches!(rig.next_return(), Return::ReplayDone));
    assert!(matches!(rig.next_return(), Return::HandBackDone));
    // One frame was sent; the cut frame and the two after it are lost.
    assert_eq!(get(&rig.stats.disconnected_dropped), 3);
    assert_eq!(script.session_frames(0), vec![0]);
}

#[test]
fn a_tls_piece_whose_delivery_is_unconfirmed_is_counted_and_the_frames_after_it_go_back() {
    let script = Script::new(false);
    // Six frames of 6,004 bytes make three pieces of two frames. The socket takes
    // all of the first record (12,008 bytes of plaintext and at most 29 of
    // overhead) and 50 bytes of the second, then resets.
    script.set_mode(Mode::then_reset(12_008 + 29 + 50));
    let rig = Rig::new(&script, Duration::from_secs(60), Some(TlsKit::new()));
    rig.send(Item::Live(numbered_block(0..6, 6_000)));
    wait_until("the link to clear up", || !rig.up());
    let Return::Returned(rest) = rig.next_return() else {
        panic!("the frames after the unconfirmed piece come back");
    };
    assert_eq!(numbers_in(&rest), vec![4, 5]);
    assert!(matches!(rig.next_return(), Return::HandBackDone));
    // Frames 2 and 3 were fed to rustls and may or may not have arrived: counted.
    assert_eq!(get(&rig.stats.disconnected_dropped), 2);
    // The receiver decrypted the first record only.
    assert_eq!(script.session_frames(0), vec![0, 1]);
}

#[test]
fn a_session_that_finishes_before_the_fence_stays_unused_until_the_fence() {
    let script = Script::new(false);
    script.set_mode(Mode::Fail(io::ErrorKind::ConnectionReset));
    // A reconnect is possible at once, but the link must not use it before the
    // router has placed every older block.
    let rig = Rig::new(&script, Duration::from_secs(60), None);
    rig.send(Item::Live(numbered_block(0..1, 100)));
    wait_until("the link to clear up", || !rig.up());
    script.allow_connect(true);
    script.set_next_mode(Mode::open());
    let Return::Returned(block) = rig.next_return() else {
        panic!("the block comes back");
    };
    assert_eq!(numbers_in(&block), vec![0]);
    assert!(matches!(rig.next_return(), Return::HandBackDone));
    // The new session is ready, and still the link does not publish `up`. An item
    // that reaches the FIFO now (a router that broke the protocol) is not taken.
    wait_until("the link to reconnect", || script.sessions() == 2);
    rig.send(Item::Live(numbered_block(1..2, 100)));
    thread::sleep(Duration::from_millis(100));
    assert!(!rig.up());
    assert_eq!(script.wire_len(1), 0, "nothing is written before the fence");
    assert_eq!(rig.fifo_end.len(), 1, "the item is still in the FIFO");
    rig.fence();
    wait_until("the link to take the item", || script.wire_len(1) == 104);
    assert!(rig.up());
    assert_eq!(script.session_frames(1), vec![1]);
}

#[test]
fn the_link_takes_items_only_under_the_take_lock() {
    let script = Script::new(false);
    let mut rig = Rig::new(&script, Duration::from_secs(60), None);
    let shared = Arc::clone(&rig.shared);
    let lock = shared.take.lock().expect("lock");
    rig.send(Item::Live(numbered_block(0..1, 100)));
    thread::sleep(Duration::from_millis(100));
    assert_eq!(script.wire_len(0), 0, "the router holds the lock");
    assert_eq!(rig.fifo_end.len(), 1);
    drop(lock);
    wait_until("the link to write the block", || script.wire_len(0) == 104);
    rig.close().expect("clean STOP");
}

/// A link thread that the test steps by hand, with the router's ends of its
/// channels.
struct Stepper {
    link: LinkThread<ScriptUplink>,
    fifo: Sender<Item>,
    returns: Receiver<Return>,
    script: Arc<Script>,
}

fn stepper() -> Stepper {
    let script = Script::new(true);
    let stats = Arc::new(SinkStats::default());
    let uplink = ScriptUplink::new(&script, &stats, None, true);
    let shared = Arc::new(LinkShared::new(false));
    let free = Arc::new(ArrayQueue::new(POOL_BLOCKS));
    let (ports, link_ports) = link_channels(LINK_FIFO_BLOCKS);
    let mut link = LinkThread::new(
        uplink,
        link_ports,
        shared,
        stats,
        free,
        script.settings(Duration::from_secs(60)),
    );
    link.begin();
    Stepper {
        link,
        fifo: ports.fifo,
        returns: ports.returns,
        script,
    }
}

#[test]
fn the_link_takes_the_next_item_at_once_and_waits_only_when_the_fifo_is_empty() {
    let mut stepper = stepper();
    for number in 0..5 {
        stepper
            .fifo
            .send(Item::Live(numbered_block(number..number + 1, 100)))
            .expect("room");
    }
    // One step writes one block, and the next step takes the next: five steps for
    // five blocks, with no wait between them.
    for _ in 0..5 {
        assert_eq!(stepper.link.step(), Step::Worked);
    }
    assert_eq!(stepper.link.step(), Step::Idle, "the FIFO is empty");
    assert_eq!(stepper.script.session_frames(0), vec![0, 1, 2, 3, 4]);
    assert_eq!(stepper.script.waits(), 0, "the socket never made it wait");
    // Closing the FIFO ends the thread at the next step.
    drop(stepper.fifo);
    assert_eq!(stepper.link.step(), Step::End(End::Closed));
    assert!(stepper.returns.is_empty());
}

#[test]
fn a_link_that_waits_for_the_fence_ends_when_the_router_closes_the_fence_channel() {
    let script = Script::new(false);
    script.set_mode(Mode::Fail(io::ErrorKind::ConnectionReset));
    let mut rig = Rig::new(&script, Duration::from_secs(60), None);
    rig.send(Item::Live(numbered_block(0..1, 100)));
    wait_until("the link to clear up", || !rig.up());
    assert!(matches!(rig.next_return(), Return::Returned(_)));
    assert!(matches!(rig.next_return(), Return::HandBackDone));
    // The router seals instead of fencing: it drops the fence sender (and the
    // FIFO sender stays, so only the fence channel can end the wait).
    drop(rig.fences.take());
    let result = rig
        .thread
        .take()
        .expect("running")
        .join()
        .expect("no panic");
    result.expect("clean end");
    assert_eq!(script.finished(), Some(true));
}

#[test]
fn a_link_that_waits_for_the_fence_ends_at_the_shutdown_deadline() {
    let script = Script::new(false);
    script.set_mode(Mode::Fail(io::ErrorKind::ConnectionReset));
    let mut rig = Rig::new(&script, Duration::from_secs(60), None);
    rig.send(Item::Live(numbered_block(0..1, 100)));
    wait_until("the link to clear up", || !rig.up());
    assert!(matches!(rig.next_return(), Return::Returned(_)));
    assert!(matches!(rig.next_return(), Return::HandBackDone));
    // The router never answers (it is busy sealing), and the shutdown begins: the
    // link ends by its own deadline, with the fence channel and the FIFO open.
    let started = Instant::now();
    rig.shared.shutdown.begin();
    let result = rig
        .thread
        .take()
        .expect("running")
        .join()
        .expect("no panic");
    result.expect("clean end");
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(4),
        "the link took {elapsed:?} to end"
    );
    assert!(!rig.up());
}

#[test]
fn the_shutdown_deadline_at_a_frame_boundary_hands_back_and_closes_with_no_stop() {
    let script = Script::new(false);
    // The socket takes the first frame of the block and nothing more.
    script.set_mode(Mode::then_block(104));
    let mut rig = Rig::new(&script, Duration::from_secs(60), None);
    rig.send(Item::Live(numbered_block(0..3, 100)));
    wait_until("the link to wait for the socket", || script.waits() >= 1);
    rig.shared.shutdown.begin();
    // At the deadline the link clears `up`, hands back its block in hand, and ends.
    // It does not wait for the FIFO to close: the router takes the FIFO.
    wait_until("the link to clear up", || !rig.up());
    let Return::Returned(rest) = rig.next_return() else {
        panic!("the unsent frames come back");
    };
    assert_eq!(numbers_in(&rest), vec![1, 2]);
    // At shutdown the link drops the return channel instead of sending
    // `HandBackDone`.
    assert!(matches!(
        rig.returns.recv_timeout(Duration::from_secs(30)),
        Err(RecvTimeoutError::Disconnected)
    ));
    let result = rig
        .thread
        .take()
        .expect("running")
        .join()
        .expect("no panic");
    result.expect("clean end");
    // The stream stopped at a frame boundary, but the deadline interrupted a
    // block: the receiver is not keeping up, and STOP would queue behind the
    // unsent bytes and wait for the whole I/O timeout. No STOP follows.
    assert_eq!(script.finished(), Some(false));
    assert_eq!(get(&rig.stats.disconnected_dropped), 0);
}

#[test]
fn the_shutdown_deadline_inside_a_frame_closes_the_stream_with_no_stop() {
    let script = Script::new(false);
    script.set_mode(Mode::then_block(104 + 30));
    let mut rig = Rig::new(&script, Duration::from_secs(60), None);
    rig.send(Item::Live(numbered_block(0..3, 100)));
    wait_until("the link to wait for the socket", || script.waits() >= 1);
    rig.shared.shutdown.begin();
    wait_until("the link to clear up", || !rig.up());
    let Return::Returned(rest) = rig.next_return() else {
        panic!("the cut frame and the rest come back");
    };
    assert_eq!(numbers_in(&rest), vec![1, 2], "the cut frame is sent again");
    let _ = rig.returns.recv_timeout(Duration::from_secs(30));
    rig.thread
        .take()
        .expect("running")
        .join()
        .expect("no panic")
        .expect("clean end");
    assert_eq!(script.finished(), Some(false), "no STOP inside a frame");
    assert_eq!(script.session_frames(0), vec![0]);
    assert_eq!(script.trailing_partial(0), 30);
}

#[test]
fn trailing_partial_counts_the_bytes_of_an_incomplete_frame() {
    assert_eq!(trailing_partial(&framed_lengths(&[10, 10])), 0);
    let mut bytes = framed_lengths(&[10, 10]);
    bytes.truncate(bytes.len() - 3);
    assert_eq!(trailing_partial(&bytes), 11);
}
