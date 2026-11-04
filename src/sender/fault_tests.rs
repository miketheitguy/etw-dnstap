//! Fault tests for the router and the link thread together: `QueuedSink`, a real
//! spool on disk, and a scripted transport that decides every answer of the
//! socket. They check the claims of `docs/design/slow-receiver.md`: no frame is
//! lost silently, none arrives twice, the capture order holds, and each lost
//! frame is in exactly one counter.
//!
//! For every test, produced = delivered + `queue_dropped` + `disconnected_dropped`
//! plus `spool_dropped` plus still spooled, and the delivered frames are in
//! capture order with no complete frame twice. The tests wait on conditions: the
//! router publishes its state (`Probe`), and the script counts the link's waits.

use std::{
    fs,
    ops::Range,
    sync::{Condvar, Mutex, atomic::AtomicUsize},
    thread,
};

use proptest::prelude::*;

use super::*;
use crate::{
    dnstap::MAX_ENCODED_LENGTH,
    network::test_support::TlsKit,
    sender::{
        BLOCK_SEND_BYTES, FRONT_LIST_BLOCKS, REPLAY_CREDIT,
        script::{Mode, Script, ScriptUplink, trailing_partial},
        sender_tests::{MIB, get, numbers_of, open_spool, queued, sized_frame, wait_until},
    },
    spool::test_support::{TestDir, parse_fstrm},
};

/// The payload of the small frames: a frame with its length is 104 bytes.
const SMALL: usize = 100;
/// The payload of a large frame, which fills a replay batch by itself.
const LARGE: usize = MAX_ENCODED_LENGTH;
/// Bytes of a frame with this payload on the wire.
const fn frame_bytes(payload: usize) -> usize {
    payload + 4
}

#[derive(Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent option of a test harness"
)]
struct Options {
    spool: bool,
    manual: bool,
    tls: bool,
    connected: bool,
    /// The first session cannot enter link mode.
    fail_link_mode: bool,
    stall: Duration,
    /// Runs on the link thread right after it clears `up`.
    hook: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Runs on the link thread while it holds the take lock, after it took an item.
    take_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Options {
    fn new() -> Self {
        Self {
            spool: true,
            manual: false,
            tls: false,
            connected: true,
            fail_link_mode: false,
            stall: Duration::from_secs(600),
            hook: None,
            take_hook: None,
        }
    }
}

/// The threads under test, with the test's ends of the transport.
struct Harness {
    sink: Option<QueuedSink>,
    script: Arc<Script>,
    stats: Arc<SinkStats>,
    shared: Arc<LinkShared>,
}

impl Harness {
    /// Starts the router and the link thread over a scripted transport. The spool
    /// is in `dir` when the options ask for one.
    fn start(dir: &TestDir, options: &Options) -> Self {
        let stats = Arc::new(SinkStats::default());
        let script = Script::new(options.manual);
        if !options.connected {
            script.allow_connect(false);
        }
        let tls = options.tls.then(TlsKit::new);
        let mut uplink = ScriptUplink::new(&script, &stats, tls, options.connected);
        if options.fail_link_mode {
            uplink = uplink.failing_link_mode();
        }
        let spool = options.spool.then(|| open_spool(dir, 256 * MIB, &stats));
        let mut settings = script.settings(options.stall);
        settings.hooks.after_up_cleared.clone_from(&options.hook);
        settings.hooks.in_take.clone_from(&options.take_hook);
        let sink = QueuedSink::start_with(uplink, spool, Arc::clone(&stats), settings)
            .expect("the threads start");
        let shared = Arc::clone(&sink.shared);
        Self {
            sink: Some(sink),
            script,
            stats,
            shared,
        }
    }

    fn sink(&mut self) -> &mut QueuedSink {
        self.sink.as_mut().expect("the sink is running")
    }

    /// Offers one frame per number, each in a block of its own. `payload` is the
    /// frame payload, at least 4 bytes. The parser channel is kept short.
    fn offer(&mut self, numbers: Range<u32>, payload: usize) {
        for number in numbers {
            wait_until("room in the send queue", || {
                queued(self.sink.as_ref().expect("running")) < SEND_QUEUE_BLOCKS - 8
            });
            let sink = self.sink();
            sink.write_frame(&sized_frame(number, payload))
                .expect("frame is queued");
            sink.flush().expect("block is sent");
        }
    }

    fn queued(&self) -> usize {
        queued(self.sink.as_ref().expect("running"))
    }

    fn probe_fifo(&self) -> usize {
        self.shared.probe.fifo.load(Ordering::Acquire)
    }

    fn probe_front(&self) -> usize {
        self.shared.probe.front.load(Ordering::Acquire)
    }

    fn replay_out(&self) -> usize {
        self.shared.probe.replay_out.load(Ordering::Acquire)
    }

    fn spills(&self) -> usize {
        self.shared.probe.spills.load(Ordering::Acquire)
    }

    fn front_max(&self) -> usize {
        self.shared.probe.front_max.load(Ordering::Acquire)
    }

    fn up(&self) -> bool {
        self.shared.up.load(Ordering::Acquire)
    }

    /// Waits until the socket is blocked with one block in the link's hand and
    /// `in_fifo` in the FIFO, and the router has taken every block.
    fn wait_for_stall(&self, in_fifo: usize) {
        wait_until("the link and the FIFO to fill", || {
            self.queued() == 0 && self.script.waits() >= 1 && self.probe_fifo() == in_fifo
        });
    }

    /// Lets the next session accept everything, and allows the reconnect.
    fn reconnect(&self) {
        self.script.set_next_mode(Mode::open());
        self.script.allow_connect(true);
    }

    fn delivered(&self) -> Vec<u32> {
        self.script.frames()
    }

    fn lost(&self) -> u64 {
        get(&self.stats.queue_dropped)
            + get(&self.stats.disconnected_dropped)
            + get(&self.stats.spool_dropped)
    }

    fn finish(&mut self) -> Result<()> {
        self.sink.take().expect("running").finish()
    }

    /// The state of the threads, for a failure message.
    fn describe(&self, offered: u32) -> String {
        format!(
            "offered {offered}, delivered {}, counters (spooled {}, replayed {}, \
             disconnected_dropped {}, queue_dropped {}, spool_dropped {}, disconnects {}, \
             reconnects {}), router (spills {}, front {}, fifo {}, replay_out {}), up {}, \
             queued {}, sessions {}, wire bytes {:?}",
            self.delivered().len(),
            get(&self.stats.spooled),
            get(&self.stats.replayed),
            get(&self.stats.disconnected_dropped),
            get(&self.stats.queue_dropped),
            get(&self.stats.spool_dropped),
            get(&self.stats.disconnects),
            get(&self.stats.reconnects),
            self.spills(),
            self.probe_front(),
            self.probe_fifo(),
            self.replay_out(),
            self.up(),
            self.queued(),
            self.script.sessions(),
            (0..self.script.sessions())
                .map(|index| self.script.wire_len(index))
                .collect::<Vec<_>>(),
        )
    }
}

/// True when `numbers` is strictly increasing: in order, with no repeat.
fn strictly_increasing(numbers: &[u32]) -> bool {
    numbers.windows(2).all(|pair| pair[0] < pair[1])
}

/// The frames in the spool files of `dir`, oldest file first.
fn frames_on_disk(dir: &TestDir) -> Vec<u32> {
    let mut names: Vec<String> = fs::read_dir(dir.path())
        .expect("folder is listed")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("seg-"))
        .collect();
    names.sort();
    names
        .into_iter()
        .flat_map(|name| {
            let file = fs::read(dir.path().join(name)).expect("segment is readable");
            numbers_of(&parse_fstrm(&file))
        })
        .collect()
}

// ---- hand-back at a failure ----

#[test]
fn a_failure_without_a_backlog_puts_the_returned_blocks_at_the_head_of_the_spool() {
    let dir = TestDir::new("fault-head");
    let mut h = Harness::start(&dir, &Options::new());
    h.script.set_mode(Mode::Blocked);
    // Block 0 is in the link's hand and blocks 1 to 5 in the FIFO. The spool is
    // empty.
    h.offer(0..6, LARGE);
    h.wait_for_stall(5);
    assert_eq!(get(&h.stats.spooled), 0);

    // The socket takes half a frame of block 0, then resets.
    h.script.set_mode(Mode::then_reset(frame_bytes(LARGE) / 2));
    // The hand-back puts the returned block first, then the FIFO, in the spool.
    // Blocks that arrive after the failure follow them.
    wait_until("the hand-back to reach the spool", || {
        get(&h.stats.spooled) == 6
    });
    h.offer(6..8, LARGE);
    wait_until("the newer blocks to reach the spool", || {
        get(&h.stats.spooled) == 8
    });
    assert_eq!(get(&h.stats.disconnects), 1);
    assert_eq!(get(&h.stats.disconnected_dropped), 0);

    h.reconnect();
    wait_until("every frame to arrive", || h.delivered().len() == 8);
    assert_eq!(h.delivered(), (0..8).collect::<Vec<_>>());
    // The old session's receiver saw half a frame and discarded it; the new one
    // got frame 0 whole.
    assert_eq!(h.script.session_frames(0), Vec::<u32>::new());
    assert_eq!(h.script.trailing_partial(0), frame_bytes(LARGE) / 2);
    assert_eq!(h.script.session_frames(1), (0..8).collect::<Vec<_>>());
    assert_eq!(h.lost(), 0);
    // The router counts a frame when it applies the link's report, a moment after
    // the receiver has the frame.
    wait_until("the reports to be applied", || get(&h.stats.replayed) == 8);
    h.finish().expect("clean STOP");
}

#[test]
fn a_failure_at_the_end_of_a_drain_loses_nothing_and_the_cut_frame_is_sent_again() {
    let dir = TestDir::new("fault-drain");
    let mut h = Harness::start(
        &dir,
        &Options {
            connected: false,
            ..Options::new()
        },
    );
    // Five large frames wait in the spool: one replay batch each.
    h.offer(0..5, LARGE);
    wait_until("the frames to be spooled", || get(&h.stats.spooled) == 5);
    // The session takes four whole frames and 1,000 bytes of the fifth.
    h.script
        .set_next_mode(Mode::then_reset(4 * frame_bytes(LARGE) + 1_000));
    h.script.allow_connect(true);
    wait_until("the replay to fail the link", || {
        get(&h.stats.disconnects) == 1
    });
    // The spool is rewound to the end of the fourth frame: the fifth is read
    // again, not counted.
    assert_eq!(get(&h.stats.disconnected_dropped), 0);
    // Live data goes to the spool while the link is down.
    h.offer(100..102, LARGE);
    wait_until("the live frames to be spooled", || {
        get(&h.stats.spooled) == 7
    });
    h.reconnect();
    wait_until("the live frames to arrive", || h.delivered().len() == 7);
    assert_eq!(h.script.session_frames(0), vec![0, 1, 2, 3]);
    assert_eq!(h.script.session_frames(1), vec![4, 100, 101]);
    assert_eq!(h.lost(), 0);
    // Every frame the spool handed to the link is counted once, when the router
    // applies the report.
    wait_until("the reports to be applied", || {
        get(&h.stats.replayed) == get(&h.stats.spooled)
    });
    h.finish().expect("clean STOP");
}

#[test]
fn a_failure_soon_after_a_spill_sends_the_live_rest_first_then_the_backlog() {
    let dir = TestDir::new("fault-front");
    let mut h = Harness::start(&dir, &Options::new());
    h.script.set_mode(Mode::Blocked);
    // Block 0 in hand, and blocks 1 to 128 fill the FIFO. Block 129 finds it full:
    // the router moves blocks 1 to 129 to the spool, which now holds a backlog.
    // Each step waits for the one before, so a link thread that the machine
    // starves for a while cannot reorder them.
    h.offer(0..1, LARGE);
    wait_until("the link to wait with block 0 in hand", || {
        h.script.waits() >= 1
    });
    h.offer(1..129, LARGE);
    wait_until("the FIFO to fill", || h.probe_fifo() == LINK_FIFO_BLOCKS);
    assert_eq!(h.spills(), 0);
    h.offer(129..130, LARGE);
    wait_until("the FIFO to spill", || {
        get(&h.stats.spooled) == LINK_FIFO_BLOCKS as u64 + 1
    });
    assert_eq!(h.spills(), 1);
    // The router reads replay batches (frames 1 to 127, one frame each) into the
    // FIFO up to its credit, and the link, still blocked on block 0, takes none.
    let credit = usize::from(REPLAY_CREDIT);
    wait_until("the replay batches to enter the FIFO", || {
        h.replay_out() == credit && h.probe_fifo() == credit
    });

    // The link fails holding live block 0 (nothing of it was taken). It goes
    // back to the front list. The replay batches go back to the spool: nothing
    // is lost.
    h.script
        .set_mode(Mode::Fail(io::ErrorKind::ConnectionReset));
    wait_until("the live block to wait on the front list", || {
        h.probe_front() == 1
    });
    assert_eq!(get(&h.stats.disconnected_dropped), 0);
    assert_eq!(get(&h.stats.disconnects), 1);
    // Newer frames follow the backlog in the spool.
    h.offer(130..132, LARGE);

    h.reconnect();
    wait_until("the last frame to arrive", || {
        h.delivered().last() == Some(&131)
    });
    // Session 1 got the live block (0) first, then the whole backlog (1 to 129,
    // read again from the start), then the newer frames.
    assert_eq!(h.script.session_frames(0), Vec::<u32>::new());
    let expected: Vec<u32> = [0].into_iter().chain(1..132).collect();
    assert_eq!(h.script.session_frames(1), expected);
    assert_eq!(h.lost(), 0);
    assert_eq!(h.front_max(), FRONT_LIST_BLOCKS);
    assert_eq!(get(&h.shared.pool_fallbacks), 0, "the pool bound holds");
    h.finish().expect("clean STOP");
}

/// A hook that holds the link thread the first time it runs, until the gate
/// opens, and counts its runs.
struct FirstRunGate {
    hook: Arc<dyn Fn() + Send + Sync>,
    gate: Arc<(Mutex<bool>, Condvar)>,
    runs: Arc<AtomicUsize>,
}

impl FirstRunGate {
    fn new() -> Self {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let runs = Arc::new(AtomicUsize::new(0));
        let hook_gate = Arc::clone(&gate);
        let hook_runs = Arc::clone(&runs);
        let hook: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if hook_runs.fetch_add(1, Ordering::AcqRel) > 0 {
                return;
            }
            let (open, changed) = &*hook_gate;
            let mut open = open.lock().expect("gate lock");
            while !*open {
                open = changed.wait(open).expect("gate lock");
            }
        });
        Self { hook, gate, runs }
    }

    /// Times the hook has run.
    fn runs(&self) -> usize {
        self.runs.load(Ordering::Acquire)
    }

    /// Lets the held thread go.
    fn open(&self) {
        let (open, changed) = &*self.gate;
        *open.lock().expect("gate lock") = true;
        changed.notify_all();
    }
}

#[test]
fn a_spill_while_the_link_holds_the_take_lock_keeps_capture_order_and_one_front_block() {
    let dir = TestDir::new("fault-spill-race");
    let gate = FirstRunGate::new();
    let mut h = Harness::start(
        &dir,
        &Options {
            take_hook: Some(Arc::clone(&gate.hook)),
            ..Options::new()
        },
    );
    // The socket will not take a byte once the link writes.
    h.script.set_mode(Mode::Blocked);
    // The link takes block 0 and stops inside the take lock.
    h.offer(0..1, LARGE);
    wait_until("the link to take block 0 under the lock", || {
        gate.runs() == 1
    });
    // Blocks 1 to 128 fill the FIFO, and block 129 finds it full: the router
    // needs the lock for the spill, and the link holds it.
    h.offer(1..130, LARGE);
    wait_until("the router to take every block", || h.queued() == 0);
    thread::sleep(Duration::from_millis(100));
    assert_eq!(h.spills(), 0, "the spill waits for the lock");
    assert_eq!(get(&h.stats.spooled), 0);

    // The link lets go. It writes block 0, which blocks, so it takes nothing
    // more, and the router takes the whole FIFO to the spool, oldest first.
    gate.open();
    wait_until("the FIFO to spill", || {
        get(&h.stats.spooled) == LINK_FIFO_BLOCKS as u64 + 1
    });
    assert_eq!(h.spills(), 1);
    let credit = usize::from(REPLAY_CREDIT);
    wait_until("the replay batches to enter the FIFO", || {
        h.replay_out() == credit && h.probe_fifo() == credit
    });

    // Then the link fails with block 0 in hand: the only live block older than
    // the backlog.
    h.script
        .set_mode(Mode::Fail(io::ErrorKind::ConnectionReset));
    wait_until("block 0 to wait on the front list", || h.probe_front() == 1);
    assert_eq!(h.front_max(), FRONT_LIST_BLOCKS);
    h.reconnect();
    wait_until("the last frame to arrive", || {
        h.delivered().last() == Some(&129)
    });
    // Block 0 goes first, then the whole backlog. The replay batches the router
    // had out went back to the spool, so none is lost.
    let expected: Vec<u32> = [0].into_iter().chain(1..130).collect();
    assert_eq!(h.script.session_frames(1), expected);
    assert_eq!(get(&h.stats.disconnected_dropped), 0);
    assert_eq!(h.lost(), 0);
    h.finish().expect("clean STOP");
}

#[test]
fn the_router_appends_nothing_between_the_failure_and_the_end_of_the_hand_back() {
    let dir = TestDir::new("fault-fence");
    // A gate holds the link thread right after it clears `up`.
    let gate = FirstRunGate::new();
    let mut h = Harness::start(
        &dir,
        &Options {
            hook: Some(Arc::clone(&gate.hook)),
            ..Options::new()
        },
    );
    h.script.set_mode(Mode::Blocked);
    h.offer(0..6, LARGE);
    h.wait_for_stall(5);

    h.script.set_mode(Mode::then_reset(100));
    wait_until("the link to clear up", || !h.up());
    // The router routes while the link fails: more blocks arrive.
    h.offer(6..8, LARGE);
    // The router has seen `up` clear and waits in the hand-back for the link
    // thread, which the gate holds. Wait for several passes of that loop.
    wait_until("the router to wait in the hand-back", || {
        h.shared.probe.hand_back_polls.load(Ordering::Acquire) >= 3
    });
    // It took nothing from the parser channel and appended nothing to the spool.
    assert_eq!(h.queued(), 2);
    assert_eq!(get(&h.stats.spooled), 0);

    // The link finishes the hand-back: the returned block comes first, then the
    // FIFO, then the newer blocks.
    gate.open();
    wait_until("every block to reach the spool", || {
        get(&h.stats.spooled) == 8
    });
    h.reconnect();
    wait_until("every frame to arrive", || h.delivered().len() == 8);
    assert_eq!(h.delivered(), (0..8).collect::<Vec<_>>());
    assert_eq!(h.lost(), 0);
    h.finish().expect("clean STOP");
}

/// A failure, and then the start of the shutdown, while the link thread is still
/// in its hand-back. The gate opens `open_after` after `finish` starts.
fn shutdown_races_a_hand_back(open_after: Duration) {
    let dir = TestDir::new("fault-shutdown-race");
    let gate = FirstRunGate::new();
    let mut h = Harness::start(
        &dir,
        &Options {
            hook: Some(Arc::clone(&gate.hook)),
            ..Options::new()
        },
    );
    h.script.set_mode(Mode::Blocked);
    h.offer(0..6, SMALL);
    h.wait_for_stall(5);
    h.script.set_mode(Mode::then_reset(10));
    wait_until("the link to clear up", || gate.runs() == 1 && !h.up());

    let opener = thread::spawn(move || {
        thread::sleep(open_after);
        gate.open();
    });
    let started = Instant::now();
    h.finish().expect("shutdown during a hand-back is clean");
    let elapsed = started.elapsed();
    opener.join().expect("the opener ends");
    eprintln!("shutdown {open_after:?} into a hand-back: {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(4),
        "shutdown took {elapsed:?}"
    );
    // Each frame ends once: all six are on disk, in order, and none is counted.
    assert_eq!(frames_on_disk(&dir), (0..6).collect::<Vec<_>>());
    assert_eq!(h.lost(), 0);
    assert_eq!(h.delivered(), Vec::<u32>::new());
}

#[test]
fn shutdown_that_starts_during_a_hand_back_and_ends_after_it_loses_nothing() {
    // The link finishes its hand-back within the budget: the router places the
    // blocks, and closes the fence channel instead of sending a fence.
    shutdown_races_a_hand_back(Duration::from_millis(200));
}

#[test]
fn shutdown_that_starts_during_a_hand_back_and_outlasts_the_budget_loses_nothing() {
    // The link finishes its hand-back after the budget ended.
    shutdown_races_a_hand_back(Duration::from_millis(1_300));
}

#[test]
fn a_frame_cut_by_a_failure_is_sent_again_in_full_on_the_new_session_and_arrives_once() {
    let dir = TestDir::new("fault-cut");
    let mut h = Harness::start(&dir, &Options::new());
    // One block of three small frames. The socket takes the first and three bytes
    // of the second, then resets.
    h.script.set_mode(Mode::then_reset(8 + 3));
    for number in 10..13 {
        h.sink()
            .write_frame(&u32::to_be_bytes(number))
            .expect("frame is queued");
    }
    h.sink().flush().expect("block is sent");
    wait_until("the session to fail", || get(&h.stats.disconnects) == 1);
    wait_until("the cut block to reach the spool", || {
        get(&h.stats.spooled) == 2
    });
    h.reconnect();
    wait_until("the rest to arrive", || h.delivered().len() == 3);
    // The old session's receiver saw frame 10 and three bytes of frame 11, which
    // it discards. The new session delivers 11 and 12 in full, once.
    assert_eq!(h.script.session_frames(0), vec![10]);
    assert_eq!(trailing_partial(&h.script.plain(0)), 3);
    assert_eq!(h.script.session_frames(1), vec![11, 12]);
    assert_eq!(h.delivered(), vec![10, 11, 12]);
    assert_eq!(h.lost(), 0);
    h.finish().expect("clean STOP");
}

#[test]
fn a_tls_piece_whose_delivery_is_unconfirmed_goes_back_and_arrives_once() {
    let dir = TestDir::new("fault-tls");
    let mut h = Harness::start(
        &dir,
        &Options {
            tls: true,
            ..Options::new()
        },
    );
    // Six frames of 6,004 bytes are three pieces of two. The socket takes the
    // first record and the first bytes of the second, then resets.
    h.script.set_mode(Mode::then_reset(12_008 + 29 + 50));
    for number in 0..6 {
        h.sink()
            .write_frame(&sized_frame(number, 6_000))
            .expect("frame is queued");
    }
    h.sink().flush().expect("block is sent");
    wait_until("the session to fail", || get(&h.stats.disconnects) == 1);
    // Frames 2 and 3 were fed to rustls, and the socket took only part of their
    // record, which the receiver cannot decrypt. They go back with 4 and 5.
    wait_until("the unsent frames to reach the spool", || {
        get(&h.stats.spooled) == 4
    });
    assert_eq!(get(&h.stats.disconnected_dropped), 0);
    h.reconnect();
    wait_until("the unsent frames to arrive", || h.delivered().len() == 6);
    assert_eq!(h.script.session_frames(0), vec![0, 1]);
    assert_eq!(h.script.session_frames(1), vec![2, 3, 4, 5]);
    assert_eq!(h.delivered(), vec![0, 1, 2, 3, 4, 5]);
    assert_eq!(h.lost(), 0);
    h.finish().expect("clean STOP");
}

#[test]
fn a_tls_replay_piece_that_is_unconfirmed_is_read_again_and_arrives_once() {
    let dir = TestDir::new("fault-tls-replay");
    let mut h = Harness::start(
        &dir,
        &Options {
            tls: true,
            connected: false,
            ..Options::new()
        },
    );
    // Six frames of 6,004 bytes wait in the spool: one replay batch. The socket
    // takes the first record and the first bytes of the second, then resets.
    for number in 0..6 {
        h.sink()
            .write_frame(&sized_frame(number, 6_000))
            .expect("frame is queued");
    }
    h.sink().flush().expect("block is sent");
    wait_until("the frames to be spooled", || get(&h.stats.spooled) == 6);
    h.script.set_next_mode(Mode::then_reset(12_008 + 29 + 50));
    h.script.allow_connect(true);
    wait_until("the replay to fail the link", || {
        get(&h.stats.disconnects) == 1
    });
    assert_eq!(get(&h.stats.disconnected_dropped), 0);
    assert_eq!(h.script.session_frames(0), vec![0, 1]);
    h.reconnect();
    wait_until("the rest to arrive", || h.delivered().len() == 6);
    assert_eq!(h.script.session_frames(1), vec![2, 3, 4, 5]);
    assert_eq!(h.lost(), 0);
    wait_until("each frame to be counted once", || {
        get(&h.stats.replayed) == 6
    });
    h.finish().expect("clean STOP");
}

// ---- replay batches out at a failure and at a stop ----

/// Frames in the backlog of the tests below: five replay batches at 104 bytes a
/// frame (630 frames, 65,520 bytes, in each of the first four).
const BACKLOG_FRAMES: u32 = 3_000;
/// Bytes of a full replay batch of small frames.
const SMALL_BATCH_BYTES: usize = 630 * 104;

/// Builds the backlog while the receiver is down, then lets the link in with a
/// session that takes nothing. When every replay batch is out (one in the link's
/// hand and four in the FIFO), the session's mode becomes `then`.
fn replay_with_five_batches_out(h: &mut Harness, then: Mode) {
    h.offer(0..BACKLOG_FRAMES, SMALL);
    wait_until("the backlog to be spooled", || {
        get(&h.stats.spooled) == u64::from(BACKLOG_FRAMES)
    });
    h.script.set_next_mode(Mode::Blocked);
    h.script.allow_connect(true);
    wait_until("the five batches to be out", || {
        h.replay_out() == 5 && h.probe_fifo() == 4 && h.script.waits() >= 1
    });
    h.script.set_mode(then);
}

#[test]
fn a_failure_with_many_replay_batches_out_loses_nothing_and_repeats_nothing() {
    let dir = TestDir::new("fault-many-batches");
    let mut h = Harness::start(
        &dir,
        &Options {
            connected: false,
            ..Options::new()
        },
    );
    // The session takes two whole batches and part of the third, then resets with
    // two batches in the FIFO.
    replay_with_five_batches_out(&mut h, Mode::then_reset(2 * SMALL_BATCH_BYTES + 5_000));
    wait_until("the link to fail", || get(&h.stats.disconnects) == 1);
    assert_eq!(get(&h.stats.disconnected_dropped), 0);
    let first = h.script.session_frames(0);
    assert!(
        first.len() > 2 * 630,
        "the session took {} frames",
        first.len()
    );
    h.reconnect();
    wait_until("every frame to arrive", || {
        h.delivered().len() == BACKLOG_FRAMES as usize
    });
    assert_eq!(h.delivered(), (0..BACKLOG_FRAMES).collect::<Vec<_>>());
    assert_eq!(
        h.script.session_frames(1)[0],
        u32::try_from(first.len()).expect("small")
    );
    assert_eq!(h.lost(), 0);
    wait_until("each frame to be counted once", || {
        get(&h.stats.replayed) == u64::from(BACKLOG_FRAMES)
    });
    assert_eq!(get(&h.shared.pool_fallbacks), 0);
    h.finish().expect("clean STOP");
}

#[test]
fn shutdown_with_replay_batches_out_keeps_the_data_and_the_next_run_starts_after_what_was_taken() {
    let dir = TestDir::new("fault-shutdown-replay");
    let mut h = Harness::start(
        &dir,
        &Options {
            connected: false,
            ..Options::new()
        },
    );
    // The session takes 30,000 bytes of the first batch and then stops.
    replay_with_five_batches_out(&mut h, Mode::then_block(30_000));
    wait_until("the link to take its 30,000 bytes", || {
        h.script.wire_len(0) == 30_000
    });
    let started = Instant::now();
    h.finish().expect("shutdown with a stalled link is clean");
    assert!(started.elapsed() < Duration::from_secs(4));
    assert_eq!(h.lost(), 0, "nothing is counted");
    let first = h.delivered();
    assert_eq!(
        first,
        (0..288).collect::<Vec<_>>(),
        "30,000 / 104 whole frames"
    );
    // Every frame is still on disk. The cut frame is in the file too.
    assert_eq!(
        frames_on_disk(&dir),
        (0..BACKLOG_FRAMES).collect::<Vec<_>>()
    );

    // The next run starts after the last frame the link took: the cut frame and
    // everything after it, once.
    let mut next = Harness::start(&dir, &Options::new());
    wait_until("the rest to arrive", || {
        next.delivered().len() == BACKLOG_FRAMES as usize - first.len()
    });
    assert_eq!(next.delivered(), (288..BACKLOG_FRAMES).collect::<Vec<_>>());
    assert_eq!(next.lost(), 0);
    next.finish().expect("clean STOP");
}

// ---- liveness and the router ----

#[test]
fn a_link_that_is_never_writable_does_not_stop_the_router() {
    let dir = TestDir::new("fault-free");
    let mut h = Harness::start(&dir, &Options::new());
    h.script.set_mode(Mode::Blocked);
    // 300 blocks of about 64 KiB: more than the FIFO takes. `offer` waits for room
    // in the parser channel; if the router waited on the socket, the channel
    // would stay full and that wait would time out.
    let blocks = 300;
    for number in 0..blocks {
        h.offer(number..number + 1, BLOCK_SEND_BYTES);
    }
    wait_until("the router to take every block", || h.queued() == 0);
    assert_eq!(get(&h.stats.queue_dropped), 0, "the parser dropped nothing");
    assert!(
        get(&h.stats.spooled) >= 150,
        "the spool grew: {}",
        get(&h.stats.spooled)
    );
    assert!(get(&h.stats.spool_bytes) > 0);
    assert_eq!(get(&h.shared.pool_fallbacks), 0, "the pool bound holds");
    assert_eq!(get(&h.stats.disconnects), 0);

    // When the link opens, every block arrives in order.
    h.script.set_mode(Mode::open());
    wait_until("every frame to arrive", || {
        h.delivered().len() == blocks as usize
    });
    assert_eq!(h.delivered(), (0..blocks).collect::<Vec<_>>());
    assert_eq!(h.lost(), 0);
    h.finish().expect("clean STOP");
}

#[test]
fn a_pause_of_one_server_quantum_never_touches_the_spool() {
    let dir = TestDir::new("fault-pause");
    let mut h = Harness::start(&dir, &Options::new());
    h.script.set_mode(Mode::Blocked);
    // At 41 MB/s the parser makes about 630 blocks of 64 KiB a second, so one
    // Windows Server quantum (187.5 ms) is 118 blocks: fewer than the FIFO takes.
    let blocks = 118;
    h.offer(0..blocks, BLOCK_SEND_BYTES);
    wait_until("the FIFO to take the pause", || {
        h.queued() == 0
            && h.script.waits() >= 1
            && h.probe_fifo() == usize::try_from(blocks).expect("small") - 1
    });
    h.script.set_mode(Mode::open());
    wait_until("every frame to arrive", || {
        h.delivered().len() == blocks as usize
    });
    assert_eq!(h.delivered(), (0..blocks).collect::<Vec<_>>());
    assert_eq!(get(&h.stats.spooled), 0);
    assert_eq!(h.spills(), 0);
    assert_eq!(h.lost(), 0);
    assert_eq!(get(&h.stats.disconnects), 0);
    h.finish().expect("clean STOP");
}

#[test]
fn without_a_spool_a_full_fifo_backs_up_into_queue_dropped_only() {
    let dir = TestDir::new("fault-nospool");
    let mut h = Harness::start(
        &dir,
        &Options {
            spool: false,
            ..Options::new()
        },
    );
    h.script.set_mode(Mode::Blocked);
    // The link holds one block, the FIFO 128, and the channel 128. The first two
    // are filled at the router's pace, so a router that a busy machine runs late
    // cannot make the parser drop before they are full.
    let in_link = u32::try_from(1 + LINK_FIFO_BLOCKS).expect("small");
    h.offer(0..in_link, SMALL);
    h.wait_for_stall(LINK_FIFO_BLOCKS);
    // The router cannot take another block, so the channel fills and the parser
    // drops the rest: 40 blocks past the channel.
    let channel = u32::try_from(SEND_QUEUE_BLOCKS).expect("small");
    let offered = in_link + channel + 40;
    for number in in_link..offered {
        let sink = h.sink();
        sink.write_frame(&sized_frame(number, SMALL))
            .expect("frame is queued");
        sink.flush().expect("block is sent");
    }
    // Only the parser drops, and only because the channel is full.
    assert_eq!(get(&h.stats.queue_dropped), 40);
    assert_eq!(get(&h.stats.disconnected_dropped), 0);
    assert_eq!(get(&h.shared.pool_fallbacks), 0, "the pool bound holds");

    // A failure counts the frames that came back, the FIFO, and the channel once.
    h.script
        .set_mode(Mode::Fail(io::ErrorKind::ConnectionReset));
    wait_until("every frame to be accounted", || {
        get(&h.stats.queue_dropped) + get(&h.stats.disconnected_dropped) == u64::from(offered)
    });
    assert_eq!(h.delivered(), Vec::<u32>::new());
    assert_eq!(get(&h.stats.spool_dropped), 0);
    h.finish().expect("shutdown while disconnected is clean");
}

// ---- shutdown ----

#[test]
fn shutdown_under_a_stall_sends_the_blocks_to_the_spool_in_order_and_the_next_run_replays_them() {
    let dir = TestDir::new("fault-shutdown");
    let mut h = Harness::start(&dir, &Options::new());
    h.script.set_mode(Mode::Blocked);
    h.offer(0..6, SMALL);
    h.wait_for_stall(5);
    // The link stopped at a block boundary (nothing of block 0 was taken). The
    // deadline interrupts the block, so the link closes with no STOP and does not
    // wait for the closing exchange, whose 2-second bound is simulated here.
    h.script.set_finish_delay(Duration::from_secs(2));

    let started = Instant::now();
    h.finish().expect("shutdown with a stalled link is clean");
    let elapsed = started.elapsed();
    eprintln!("shutdown under a stall (no STOP): {elapsed:?}");
    // The budget (1 s) plus a poll. A STOP would add its 2 seconds.
    assert!(
        elapsed < Duration::from_secs(3),
        "shutdown took {elapsed:?}"
    );
    assert_eq!(
        h.script.finished(),
        Some(false),
        "no STOP into a full buffer"
    );
    assert_eq!(h.lost(), 0);
    // The block the link gave back and the FIFO are on disk, in order.
    assert_eq!(frames_on_disk(&dir), (0..6).collect::<Vec<_>>());
    assert_eq!(get(&h.stats.spooled), 6);

    // The next run replays them in order, then sends newer frames.
    let mut next = Harness::start(&dir, &Options::new());
    next.offer(100..102, SMALL);
    wait_until("the replay to arrive", || next.delivered().len() == 8);
    assert_eq!(next.delivered(), vec![0, 1, 2, 3, 4, 5, 100, 101]);
    assert_eq!(next.lost(), 0);
    next.finish().expect("clean STOP");
}

#[test]
fn shutdown_with_a_full_fifo_under_a_stall_keeps_every_block_on_disk_in_order() {
    let dir = TestDir::new("fault-shutdown-full");
    let mut h = Harness::start(&dir, &Options::new());
    h.script.set_mode(Mode::Blocked);
    // One block in the link's hand and a full FIFO. The link takes block 0 before
    // the rest arrive: a link that a busy machine runs late would otherwise see a
    // full FIFO first, and the router would spill it.
    let live = u32::try_from(1 + LINK_FIFO_BLOCKS).expect("small");
    h.offer(0..1, SMALL);
    wait_until("the link to wait with block 0 in hand", || {
        h.script.waits() >= 1
    });
    h.offer(1..live, SMALL);
    h.wait_for_stall(LINK_FIFO_BLOCKS);
    assert_eq!(h.spills(), 0);

    let started = Instant::now();
    h.finish().expect("shutdown with a stalled link is clean");
    let elapsed = started.elapsed();
    eprintln!("shutdown with a full FIFO under a stall: {elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(4),
        "shutdown took {elapsed:?}"
    );
    assert_eq!(get(&h.stats.disconnected_dropped), 0);
    assert_eq!(h.lost(), 0);
    assert_eq!(frames_on_disk(&dir), (0..live).collect::<Vec<_>>());
}

#[test]
fn shutdown_at_a_block_boundary_sends_stop_and_shutdown_inside_a_frame_does_not() {
    // At a boundary: the link took everything, so STOP follows.
    let clean_dir = TestDir::new("fault-stop-clean");
    let mut clean = Harness::start(&clean_dir, &Options::new());
    clean.offer(0..3, SMALL);
    wait_until("every frame to arrive", || clean.delivered().len() == 3);
    clean.finish().expect("clean STOP");
    assert_eq!(clean.script.finished(), Some(true));

    // Inside a frame: the socket took the first frame and 30 bytes of the second
    // and then blocked. The cut frame is on disk and no STOP follows.
    let dir = TestDir::new("fault-stop-cut");
    let mut cut = Harness::start(&dir, &Options::new());
    cut.script
        .set_mode(Mode::then_block(frame_bytes(SMALL) + 30));
    for number in 0..3 {
        cut.sink()
            .write_frame(&sized_frame(number, SMALL))
            .expect("frame is queued");
    }
    cut.sink().flush().expect("block is sent");
    wait_until("the socket to take its share", || {
        cut.script.wire_len(0) == frame_bytes(SMALL) + 30 && cut.script.waits() >= 1
    });
    cut.finish().expect("shutdown inside a frame is clean");
    assert_eq!(cut.script.finished(), Some(false), "no STOP inside a frame");
    assert_eq!(cut.script.session_frames(0), vec![0]);
    assert_eq!(cut.script.trailing_partial(0), 30);
    assert_eq!(frames_on_disk(&dir), vec![1, 2], "the cut frame is on disk");
    assert_eq!(cut.lost(), 0);
}

// ---- capture order across arbitrary stalls, failures, and resumes ----

#[derive(Clone, Debug)]
enum Op {
    /// This many small one-frame blocks.
    Burst(u32),
    /// This many large one-frame blocks.
    Big(u32),
    /// The link stops accepting bytes.
    Block,
    /// The link accepts everything again.
    Open,
    /// The link accepts this many more bytes, then stops.
    Partial(usize),
    /// The session resets after this many more bytes.
    Fail(usize),
    /// A reconnect may happen.
    Reconnect,
    /// This many small one-frame blocks at once: a backlog of several replay
    /// batches when the link is down or behind.
    Backlog(u32),
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (1_u32..=130).prop_map(Op::Burst),
        1 => (1_u32..=6).prop_map(Op::Big),
        2 => Just(Op::Block),
        2 => Just(Op::Open),
        2 => (0_usize..200_000).prop_map(Op::Partial),
        2 => (0_usize..200_000).prop_map(Op::Fail),
        3 => Just(Op::Reconnect),
        1 => (300_u32..=1500).prop_map(Op::Backlog),
    ]
}

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 24,
        max_shrink_iters: 30,
        // No regression files in the source tree.
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

proptest! {
    #![proptest_config(config())]

    /// Whatever the link does (stall, take a few bytes, reset, come back), the
    /// frames the receiver gets are in capture order with no complete frame twice,
    /// and every frame offered is delivered: none is lost or counted lost.
    #[test]
    fn backlog_to_live_keeps_capture_order_over_stalls_failures_and_resumes(
        ops in prop::collection::vec(op_strategy(), 1..14),
    ) {
        let dir = TestDir::new("fault-property");
        let mut h = Harness::start(&dir, &Options::new());
        let mut next = 0_u32;
        for op in &ops {
            match *op {
                Op::Burst(count) => {
                    // The payload varies so batches end at different places.
                    for number in next..next + count {
                        let payload = 8 + (number as usize * 37) % 400;
                        h.offer(number..number + 1, payload);
                    }
                    next += count;
                }
                Op::Big(count) => {
                    h.offer(next..next + count, LARGE);
                    next += count;
                }
                Op::Backlog(count) => {
                    h.offer(next..next + count, SMALL);
                    next += count;
                }
                Op::Block => {
                    h.script.set_mode(Mode::Blocked);
                    h.script.set_next_mode(Mode::Blocked);
                }
                Op::Open => {
                    h.script.set_mode(Mode::open());
                    h.script.set_next_mode(Mode::open());
                }
                Op::Partial(bytes) => {
                    h.script.set_mode(Mode::then_block(bytes));
                    h.script.set_next_mode(Mode::then_block(bytes));
                }
                Op::Fail(bytes) => {
                    h.script.set_mode(Mode::then_reset(bytes));
                    h.script.set_next_mode(Mode::open());
                }
                Op::Reconnect => h.script.allow_connect(true),
            }
        }
        // The link comes back and takes everything. A session that was already
        // failing when the mode changed ends after this point, and the script
        // refuses the reconnect after each loss, so the reconnect is allowed again
        // on each pass.
        h.script.set_next_mode(Mode::open());
        h.script.set_mode(Mode::open());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            h.script.allow_connect(true);
            if h.queued() == 0 && h.delivered().len() as u64 == u64::from(next) {
                break;
            }
            prop_assert!(Instant::now() < deadline, "frames are missing: {}", h.describe(next));
            thread::sleep(Duration::from_millis(5));
        }
        let got = h.delivered();
        prop_assert!(strictly_increasing(&got), "order or repeat: {got:?}");
        // The scripted socket has no kernel buffer, and the spool is large: with
        // B1 and B2 nothing is lost at a failure, so every frame arrives.
        prop_assert_eq!(got.len() as u64, u64::from(next));
        prop_assert_eq!(h.lost(), 0);
        prop_assert_eq!(get(&h.stats.disconnected_dropped), 0);
        prop_assert_eq!(get(&h.shared.pool_fallbacks), 0);
        prop_assert_eq!(get(&h.stats.spool_dropped), 0);
        prop_assert_eq!(get(&h.stats.queue_dropped), 0);
        h.finish().expect("clean end");
    }
}

// ---- the worst case of shutdown ----

#[test]
fn the_closing_exchange_alone_takes_at_most_its_bound() {
    let dir = TestDir::new("fault-closing");
    let mut h = Harness::start(&dir, &Options::new());
    h.offer(0..3, SMALL);
    wait_until("every frame to arrive", || h.delivered().len() == 3);
    // The receiver is slow to answer STOP: the exchange takes its 2-second bound.
    h.script.set_finish_delay(Duration::from_secs(2));
    let started = Instant::now();
    h.finish().expect("clean end");
    let elapsed = started.elapsed();
    eprintln!("shutdown with a closing exchange at its 2 s bound: {elapsed:?}");
    assert!(elapsed >= Duration::from_secs(2), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    assert_eq!(h.script.finished(), Some(true));
    assert_eq!(h.lost(), 0);
}

#[test]
fn the_worst_case_of_shutdown_is_the_budget_plus_the_closing_exchange() {
    let dir = TestDir::new("fault-worst");
    let mut h = Harness::start(&dir, &Options::new());
    h.script.set_mode(Mode::Blocked);
    // Fewer blocks than the FIFO takes, so the spool has no backlog.
    h.offer(0..60, SMALL);
    wait_until("the router to take every block", || {
        h.queued() == 0 && h.probe_fifo() == 59
    });
    h.script.set_finish_delay(Duration::from_secs(2));
    // The receiver wakes just inside the budget: the link drains its blocks, the
    // router has nothing to place, and the closing exchange then takes its bound. On a
    // machine busy enough to delay the wake past the budget, the link closes with
    // no STOP instead, which is faster. So this checks the upper bound only.
    let script = Arc::clone(&h.script);
    let waker = thread::spawn(move || {
        thread::sleep(Duration::from_millis(850));
        script.set_mode(Mode::open());
    });
    let started = Instant::now();
    h.finish().expect("clean end");
    let elapsed = started.elapsed();
    waker.join().expect("the waker ends");
    eprintln!(
        "worst case of shutdown (receiver wakes at 0.85 s, STOP at its 2 s bound): {elapsed:?}"
    );
    // 1 s of budget, one poll, and 2 s of closing exchange, with the seal of the
    // spool at the same time.
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    assert_eq!(h.lost(), 0);
}

#[test]
fn a_session_that_cannot_enter_link_mode_sends_the_blocks_to_the_spool() {
    let dir = TestDir::new("fault-link-mode");
    let mut h = Harness::start(
        &dir,
        &Options {
            fail_link_mode: true,
            ..Options::new()
        },
    );
    // The first poll lost the session, so `up` was never set: the router started
    // down, and nothing goes to the link, which would count it as dropped.
    assert!(!h.up());
    h.offer(0..5, SMALL);
    wait_until("the blocks to be spooled", || get(&h.stats.spooled) == 5);
    assert_eq!(get(&h.stats.disconnects), 1);
    assert_eq!(h.lost(), 0);
    h.reconnect();
    wait_until("every frame to arrive", || h.delivered().len() == 5);
    assert_eq!(h.delivered(), (0..5).collect::<Vec<_>>());
    assert_eq!(h.lost(), 0);
    h.finish().expect("clean STOP");
}
