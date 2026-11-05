//! Tests for the queued sink and the sender thread, against a fake dnstap
//! receiver on the loopback interface.

use std::{
    fs,
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    ops::Range,
    path::Path,
    sync::{Mutex, atomic::AtomicBool},
};

use super::*;
use crate::{
    framestream::{CONTENT_TYPE, CONTROL_START, CONTROL_STOP},
    network::BackoffPolicy,
    spool::{
        KeyOrigin, SpoolConfig,
        test_support::{TestDir, parse_segment, test_key},
    },
};

/// The stall timeout of the tests that do not test it: far longer than any pause
/// they hold, so only a test that means to end a session by a stall does.
pub(super) const TEST_STALL: Duration = Duration::from_secs(60);

const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
pub(super) const MIB: u64 = 1024 * 1024;

const CONTROL_ACCEPT: u32 = 1;
const CONTROL_READY: u32 = 4;
const CONTROL_FINISH: u32 = 5;

/// Retries within tens of milliseconds, so reconnect tests finish quickly.
pub(super) fn fast_policy() -> BackoffPolicy {
    BackoffPolicy {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(40),
    }
}

/// Never retries within a test, so counts are deterministic.
pub(super) fn no_retry() -> BackoffPolicy {
    BackoffPolicy {
        initial: Duration::from_secs(600),
        max: Duration::from_secs(600),
    }
}

pub(super) fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT_TIMEOUT;
    while !condition() {
        assert!(
            Instant::now() < deadline,
            "timed out after {WAIT_TIMEOUT:?} waiting for {what}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

pub(super) fn get(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

/// What a fake receiver saw, and how it behaves.
#[derive(Default)]
pub(super) struct Received {
    /// The sequence number in each data frame, in arrival order.
    pub(super) frames: Mutex<Vec<u32>>,
    /// STOP arrived and FINISH was sent.
    pub(super) stopped: AtomicBool,
    /// While set, the receiver stops reading but keeps the connection.
    pub(super) paused: AtomicBool,
    /// While set, the receiver never answers STOP with FINISH. It keeps the
    /// connection open until the server is dropped.
    pub(super) silent_finish: AtomicBool,
    /// While set, the receiver sleeps after each large frame.
    slow: AtomicBool,
    /// Milliseconds the receiver sleeps after each large frame, on top of
    /// `slow`: a receiver that reads at a set pace.
    pub(super) frame_delay_ms: AtomicU64,
    /// Non-zero: the receiver hangs up after this many data frames on one
    /// connection and clears the value, so the next connection is served fully.
    hang_up_after: AtomicU64,
    /// While set, the receiver drops the connection at its next frame without
    /// recording that frame. The flag clears itself.
    kick: AtomicBool,
    /// Milliseconds the receiver waits after accepting a connection before it
    /// answers the handshake (a receiver that is slow to accept).
    accept_delay_ms: AtomicU64,
}

impl Received {
    pub(super) fn len(&self) -> usize {
        self.frames.lock().expect("receiver state").len()
    }

    pub(super) fn frames(&self) -> Vec<u32> {
        self.frames.lock().expect("receiver state").clone()
    }
}

/// A fake receiver that keeps one listener for the whole test. Until
/// [`Self::serve_now`], it accepts each connection and drops it at once, so the
/// sender sees a network error and retries. Afterwards it serves connections one
/// at a time.
pub(super) struct FakeServer {
    address: SocketAddr,
    pub(super) received: Arc<Received>,
    serving: Arc<AtomicBool>,
    quit: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl FakeServer {
    fn new(serving: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        let address = listener.local_addr().expect("listener address");
        let received = Arc::new(Received::default());
        let serving = Arc::new(AtomicBool::new(serving));
        let quit = Arc::new(AtomicBool::new(false));
        let thread = {
            let (received, serving, quit) = (
                Arc::clone(&received),
                Arc::clone(&serving),
                Arc::clone(&quit),
            );
            thread::spawn(move || {
                while let Ok((stream, _)) = listener.accept() {
                    if quit.load(Ordering::Acquire) {
                        break;
                    }
                    if serving.load(Ordering::Acquire) {
                        // A connection that ends early is the sender's business.
                        let _ = serve(stream, &received, &quit);
                    }
                }
            })
        };
        Self {
            address,
            received,
            serving,
            quit,
            thread: Some(thread),
        }
    }

    /// Accepts and drops every connection.
    pub(super) fn down() -> Self {
        Self::new(false)
    }

    pub(super) fn up() -> Self {
        Self::new(true)
    }

    pub(super) fn serve_now(&self) {
        self.serving.store(true, Ordering::Release);
    }

    /// Starts an outage: the current connection ends at its next frame, and new
    /// ones are accepted and dropped until [`Self::serve_now`].
    fn outage(&self) {
        self.serving.store(false, Ordering::Release);
        self.received.kick.store(true, Ordering::Release);
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Release);
        self.received.paused.store(false, Ordering::Release);
        // Wake the accept loop.
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_u32(stream: &mut TcpStream) -> io::Result<u32> {
    let mut bytes = [0; 4];
    stream.read_exact(&mut bytes)?;
    Ok(u32::from_be_bytes(bytes))
}

/// One frame from the sender: a data payload, or a control frame body.
enum Item {
    Data(Vec<u8>),
    Control(Vec<u8>),
}

fn read_item(stream: &mut TcpStream) -> io::Result<Item> {
    let length = read_u32(stream)?;
    let (control, length) = if length == 0 {
        (true, read_u32(stream)?)
    } else {
        (false, length)
    };
    let mut body = vec![0; length as usize];
    stream.read_exact(&mut body)?;
    Ok(if control {
        Item::Control(body)
    } else {
        Item::Data(body)
    })
}

fn write_control(stream: &mut TcpStream, kind: u32, content_type: bool) -> io::Result<()> {
    let mut frame = Vec::new();
    frame.extend_from_slice(&0_u32.to_be_bytes());
    if content_type {
        let content_length = u32::try_from(CONTENT_TYPE.len()).expect("short");
        frame.extend_from_slice(&(12 + content_length).to_be_bytes());
        frame.extend_from_slice(&kind.to_be_bytes());
        frame.extend_from_slice(&1_u32.to_be_bytes());
        frame.extend_from_slice(&content_length.to_be_bytes());
        frame.extend_from_slice(CONTENT_TYPE);
    } else {
        frame.extend_from_slice(&4_u32.to_be_bytes());
        frame.extend_from_slice(&kind.to_be_bytes());
    }
    stream.write_all(&frame)
}

fn control_kind(body: &[u8]) -> u32 {
    u32::from_be_bytes(body[..4].try_into().expect("control type"))
}

/// The receiver side of the handshake: READY, ACCEPT, START.
fn handshake(stream: &mut TcpStream) -> io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    match read_item(stream)? {
        Item::Control(body) if control_kind(&body) == CONTROL_READY => {}
        _ => return Err(io::Error::other("expected READY")),
    }
    write_control(stream, CONTROL_ACCEPT, true)?;
    match read_item(stream)? {
        Item::Control(body) if control_kind(&body) == CONTROL_START => Ok(()),
        _ => Err(io::Error::other("expected START")),
    }
}

/// Reads data until STOP, then answers FINISH.
fn serve(mut stream: TcpStream, received: &Received, quit: &AtomicBool) -> io::Result<()> {
    let delay = received.accept_delay_ms.load(Ordering::Acquire);
    if delay > 0 {
        thread::sleep(Duration::from_millis(delay));
    }
    handshake(&mut stream)?;
    // An outage requested while no session was live must not discard the first
    // frame of this one.
    received.kick.store(false, Ordering::Release);
    let mut on_this_connection = 0_u64;
    loop {
        while received.paused.load(Ordering::Acquire) && !quit.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
        let item = read_item(&mut stream)?;
        if received.kick.swap(false, Ordering::AcqRel) {
            return Ok(());
        }
        match item {
            Item::Data(payload) => {
                let number = u32::from_be_bytes(payload[..4].try_into().expect("numbered payload"));
                received.frames.lock().expect("receiver state").push(number);
                on_this_connection += 1;
                let limit = received.hang_up_after.load(Ordering::Acquire);
                if limit > 0 && on_this_connection >= limit {
                    received.hang_up_after.store(0, Ordering::Release);
                    return Ok(());
                }
                if payload.len() > 1024 && received.slow.load(Ordering::Acquire) {
                    thread::sleep(Duration::from_millis(1));
                }
                let pace = received.frame_delay_ms.load(Ordering::Acquire);
                if payload.len() > 1024 && pace > 0 {
                    thread::sleep(Duration::from_millis(pace));
                }
            }
            Item::Control(body) if control_kind(&body) == CONTROL_STOP => {
                if received.silent_finish.load(Ordering::Acquire) {
                    while !quit.load(Ordering::Acquire) {
                        thread::sleep(Duration::from_millis(1));
                    }
                    return Ok(());
                }
                write_control(&mut stream, CONTROL_FINISH, false)?;
                received.stopped.store(true, Ordering::Release);
                return Ok(());
            }
            Item::Control(_) => return Err(io::Error::other("unexpected control frame")),
        }
    }
}

/// A fake volume with unlimited free space.
#[expect(
    clippy::unnecessary_wraps,
    reason = "the spool takes a fn pointer that returns a Result"
)]
fn plenty(_dir: &Path) -> io::Result<u64> {
    Ok(u64::MAX)
}

pub(super) fn open_spool(dir: &TestDir, max_bytes: u64, stats: &Arc<SinkStats>) -> Spool {
    Spool::open(
        SpoolConfig {
            dir: dir.path().to_path_buf(),
            max_bytes,
            min_free_bytes: 0,
        },
        test_key(),
        KeyOrigin::Loaded,
        Arc::clone(stats),
        plenty,
    )
    .expect("spool opens")
}

pub(super) fn start_sink(
    server: &FakeServer,
    policy: BackoffPolicy,
    spool: Option<Spool>,
    stats: &Arc<SinkStats>,
) -> QueuedSink {
    start_sink_with_stall(server, policy, spool, stats, TEST_STALL)
}

pub(super) fn start_sink_with_stall(
    server: &FakeServer,
    policy: BackoffPolicy,
    spool: Option<Spool>,
    stats: &Arc<SinkStats>,
    stall: Duration,
) -> QueuedSink {
    let link =
        NetworkSink::start_with(server.address, Arc::clone(stats), policy).expect("link starts");
    QueuedSink::start(link, spool, Arc::clone(stats), stall).expect("sender threads start")
}

pub(super) fn frame(number: u32) -> [u8; 4] {
    number.to_be_bytes()
}

/// Writes one 4-byte frame per number, then flushes.
pub(super) fn send(sink: &mut QueuedSink, numbers: Range<u32>) {
    for number in numbers {
        sink.write_frame(&frame(number)).expect("frame is queued");
    }
    sink.flush().expect("partial block is sent");
}

/// A frame of `size` bytes that starts with its number.
pub(super) fn sized_frame(number: u32, size: usize) -> Vec<u8> {
    let mut payload = vec![0x5a; size];
    payload[..4].copy_from_slice(&number.to_be_bytes());
    payload
}

pub(super) fn queued(sink: &QueuedSink) -> usize {
    sink.queue.as_ref().map_or(0, Sender::len)
}

/// Waits until the sender thread has taken every queued block.
fn wait_drained(sink: &QueuedSink) {
    wait_until("the send queue to empty", || queued(sink) == 0);
}

/// How long the link's own wait for the socket must last before the stall is
/// taken as real: 50 ms, one `LINK_POLL`.
const REAL_STALL_US: u64 = 50_000;
/// The bytes of one large frame on the wire, which is also one block.
const LARGE_FRAME_BYTES: u64 = MAX_ENCODED_LENGTH as u64 + 4;

/// Offers large frames (one block each) until the link is stalled for real and
/// the router has spilled to the spool. Returns the number of frames offered.
///
/// The router spills only when the link FIFO is full. A link thread that a busy
/// machine schedules late looks the same to the router, and the spill could then
/// start before any stall, with the link taking blocks again afterwards. So the
/// first phase offers a frame only while at most 3 blocks are ahead of what the
/// link has written, until the link's own readiness wait on the socket has lasted
/// [`REAL_STALL_US`]: a link that is merely late cannot make that wait. From then
/// on the receiver, which is paused, takes nothing, so the second phase fills the
/// FIFO and spills, and the FIFO never has room again. The parser channel is
/// kept short throughout, as a router that never waits on the socket allows.
pub(super) fn stall_the_link(sink: &mut QueuedSink) -> u32 {
    let stats = Arc::clone(&sink.stats);
    let mut offered = 0;
    while get(&stats.write_max_us) < REAL_STALL_US {
        assert!(offered < 3000, "no stall after {offered} frames");
        wait_until("the link to keep up or wait on the socket", || {
            u64::from(offered) <= get(&stats.bytes_sent) / LARGE_FRAME_BYTES + 3
                || get(&stats.write_max_us) >= REAL_STALL_US
        });
        sink.write_frame(&sized_frame(offered, MAX_ENCODED_LENGTH))
            .expect("frame is queued");
        offered += 1;
    }
    // The spill moves the whole FIFO and the block that did not fit.
    while get(&stats.spooled) <= LINK_FIFO_BLOCKS as u64 {
        assert!(offered < 3000, "no spill after {offered} frames");
        wait_until("room in the send queue", || {
            queued(sink) < SEND_QUEUE_BLOCKS - 8
        });
        sink.write_frame(&sized_frame(offered, MAX_ENCODED_LENGTH))
            .expect("frame is queued");
        offered += 1;
    }
    offered
}

pub(super) fn queue_tiny_blocks(sink: &mut QueuedSink, tiny: u32, mut offered: u32) -> u32 {
    for _ in 0..tiny {
        // The router appends to the spool at disk speed, so keep the channel
        // short: a full channel drops, and these tests count no drop.
        wait_until("room in the send queue", || {
            queued(sink) < SEND_QUEUE_BLOCKS - 8
        });
        sink.write_frame(&frame(offered)).expect("frame is queued");
        sink.flush().expect("block is sent");
        offered += 1;
    }
    offered
}

fn segment_files(dir: &TestDir) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.path())
        .expect("folder is listed")
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("seg-"))
        .collect();
    names.sort();
    names
}

pub(super) fn numbers_of(payloads: &[Vec<u8>]) -> Vec<u32> {
    payloads
        .iter()
        .map(|payload| u32::from_be_bytes(payload[..4].try_into().expect("numbered payload")))
        .collect()
}

#[test]
fn connected_without_a_spool_delivers_in_order_and_ends_with_stop_finish() {
    let server = FakeServer::up();
    let stats = Arc::new(SinkStats::default());
    let mut sink = start_sink(&server, no_retry(), None, &stats);
    for start in (0..5000).step_by(1000) {
        send(&mut sink, start..start + 1000);
    }
    sink.finish().expect("clean STOP/FINISH");
    wait_until("STOP", || server.received.stopped.load(Ordering::Acquire));
    assert_eq!(server.received.frames(), (0..5000).collect::<Vec<_>>());
    for counter in [
        &stats.queue_dropped,
        &stats.disconnected_dropped,
        &stats.spooled,
        &stats.spool_dropped,
    ] {
        assert_eq!(get(counter), 0);
    }
}

#[test]
fn frames_spooled_while_the_receiver_is_down_replay_in_order_before_newer_ones() {
    let dir = TestDir::new("outage");
    let server = FakeServer::down();
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 8 * MIB, &stats);
    let mut sink = start_sink(&server, fast_policy(), Some(spool), &stats);
    assert!(!stats.connected.load(Ordering::Relaxed));
    send(&mut sink, 0..50);
    wait_until("the frames to be spooled", || get(&stats.spooled) == 50);

    server.serve_now();
    wait_until("the sender to reconnect", || {
        stats.connected.load(Ordering::Relaxed)
    });
    // These may join the backlog or go straight to the link, depending on how far
    // the replay has come; either way they follow the older frames.
    send(&mut sink, 50..60);
    wait_until("all frames to arrive", || server.received.len() >= 60);
    sink.finish().expect("clean STOP/FINISH");

    assert_eq!(server.received.frames(), (0..60).collect::<Vec<_>>());
    assert!(get(&stats.spooled) >= 50);
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(get(&stats.queue_dropped), 0);
}

#[test]
fn frames_are_counted_as_disconnected_dropped_without_a_spool() {
    let server = FakeServer::down();
    let stats = Arc::new(SinkStats::default());
    let mut sink = start_sink(&server, no_retry(), None, &stats);
    send(&mut sink, 0..12);
    send(&mut sink, 12..20);
    sink.finish().expect("shutdown while disconnected is clean");
    assert_eq!(get(&stats.disconnected_dropped), 20);
    assert_eq!(get(&stats.queue_dropped), 0);
    assert_eq!(get(&stats.spooled), 0);
}

#[test]
fn a_full_queue_counts_queue_dropped_and_never_blocks_the_parser() {
    let server = FakeServer::up();
    // The receiver finishes the handshake, then never reads: the socket buffers
    // fill and the sender thread stalls.
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let mut sink = start_sink(&server, no_retry(), None, &stats);
    assert!(stats.connected.load(Ordering::Relaxed));

    // One frame this size fills a block, so each write sends a block.
    let payload = vec![0x5a; MAX_ENCODED_LENGTH];
    let mut offered = 0_u64;
    let mut slowest = Duration::ZERO;
    while get(&stats.queue_dropped) == 0 {
        assert!(offered < 3000, "buffering absorbed {offered} large frames");
        let started = Instant::now();
        sink.write_frame(&payload)
            .expect("a full queue is not an error");
        slowest = slowest.max(started.elapsed());
        offered += 1;
    }
    assert!(
        slowest < Duration::from_secs(1),
        "the parser thread waited {slowest:?}"
    );
    // Every dropped block held one frame.
    assert!(get(&stats.queue_dropped) <= offered);

    let shared = Arc::clone(&sink.shared);
    let started = Instant::now();
    let result = sink.finish();
    let elapsed = started.elapsed();
    eprintln!("shutdown with a stalled receiver, no spool: {elapsed:?}");
    // The budget (1 s) and a poll. The deadline interrupts the block in hand, so
    // the link sends no STOP into the full buffer. The margin is for a busy
    // machine; the watchdog allows 10 s.
    assert!(
        elapsed < Duration::from_secs(4),
        "shutdown took {elapsed:?}"
    );
    // A stalled receiver at shutdown never fails the service: SCM would restart it.
    result.expect("shutdown with a stalled link is clean");
    assert!(!server.received.stopped.load(Ordering::Acquire));
    // The stall timeout (60 s here) was never reached, so the session was not
    // ended for a stall: the shutdown deadline stopped the link, which gave its
    // unsent blocks back, and with no spool the router counted them.
    assert_eq!(get(&stats.disconnects), 0);
    assert!(get(&stats.disconnected_dropped) >= 1);
    assert!(
        get(&stats.queue_dropped) + get(&stats.disconnected_dropped) <= offered,
        "no frame is counted twice"
    );
    assert_eq!(
        get(&shared.pool_fallbacks),
        0,
        "the parser never allocated a block"
    );
}

#[test]
fn a_slow_receiver_spills_to_the_spool_instead_of_dropping() {
    let dir = TestDir::new("spill");
    let server = FakeServer::up();
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 64 * MIB, &stats);
    let mut sink = start_sink(&server, no_retry(), Some(spool), &stats);

    // The link blocks on the receiver that stopped reading. The router never
    // waits on the socket, so the parser channel stays short, and once the link
    // FIFO is full the router spills to the spool.
    let offered = stall_the_link(&mut sink);
    assert!(get(&stats.spooled) > LINK_FIFO_BLOCKS as u64);
    let offered = queue_tiny_blocks(&mut sink, 100, offered);
    server.received.paused.store(false, Ordering::Release);

    let total = usize::try_from(offered).expect("small");
    wait_until("all frames to arrive", || server.received.len() >= total);
    sink.finish().expect("clean STOP/FINISH");
    assert_eq!(server.received.frames(), (0..offered).collect::<Vec<_>>());
    assert_eq!(get(&stats.queue_dropped), 0);
    assert_eq!(get(&stats.disconnects), 0);
    assert!(
        get(&stats.spooled) > 0,
        "the backlog went through the spool"
    );
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
}

#[test]
fn live_data_does_not_wait_behind_a_replay() {
    let dir = TestDir::new("live-replay");
    let server = FakeServer::down();
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 64 * MIB, &stats);
    let mut sink = start_sink(&server, fast_policy(), Some(spool), &stats);

    // A backlog of large frames, built while the receiver is down.
    let backlog = 150_u32;
    for number in 0..backlog {
        sink.write_frame(&sized_frame(number, 60_000))
            .expect("frame is queued");
        if number % 10 == 9 {
            wait_drained(&sink);
        }
    }
    sink.flush().expect("block is sent");
    wait_until("the backlog to be spooled", || {
        get(&stats.spooled) == u64::from(backlog)
    });

    // The receiver comes back but reads large frames slowly, so the replay
    // takes a while. Live frames arrive meanwhile, one block each.
    server.received.slow.store(true, Ordering::Release);
    server.serve_now();
    wait_until("the sender to reconnect", || {
        stats.connected.load(Ordering::Relaxed)
    });
    let live = 600_u32;
    for number in backlog..backlog + live {
        // The router appends to the spool at disk speed. On a busy machine it can
        // be descheduled for longer than the 256-block channel lasts at this rate,
        // so wait for room, as a stalled router would make this wait time out.
        wait_until("room in the send queue", || {
            queued(&sink) < SEND_QUEUE_BLOCKS - 8
        });
        sink.write_frame(&frame(number)).expect("frame is queued");
        sink.flush().expect("block is sent");
        let paced = Instant::now();
        while paced.elapsed() < Duration::from_micros(300) {
            std::hint::spin_loop();
        }
    }
    // A drop would leave the wait below to time out, so check it first.
    assert_eq!(get(&stats.queue_dropped), 0, "live data was not dropped");
    let total = usize::try_from(backlog + live).expect("small");
    wait_until("all frames to arrive", || server.received.len() >= total);
    sink.finish().expect("clean STOP/FINISH");

    assert_eq!(get(&stats.queue_dropped), 0, "live data was not dropped");
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
    assert_eq!(
        server.received.frames(),
        (0..backlog + live).collect::<Vec<_>>()
    );
}

#[test]
fn shutdown_with_a_stalled_link_and_queued_blocks_is_bounded_and_accounted() {
    let dir = TestDir::new("stalled-shutdown");
    let server = FakeServer::up();
    server.received.paused.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 64 * MIB, &stats);
    let mut sink = start_sink(&server, no_retry(), Some(spool), &stats);
    let offered = stall_the_link(&mut sink);
    let offered = queue_tiny_blocks(&mut sink, 20, offered);

    let started = Instant::now();
    let result = sink.finish();
    let elapsed = started.elapsed();
    eprintln!("shutdown with a stalled receiver and a spool: {elapsed:?}");
    // About 1.1 s: the budget and a poll. Windows non-blocking sends take a whole
    // block or nothing, so the link usually stops at a block boundary, but the
    // deadline interrupts the block in hand either way, and no STOP follows. The
    // margin is for a busy machine; the watchdog allows 10 s.
    assert!(
        elapsed < Duration::from_secs(4),
        "shutdown took {elapsed:?}"
    );

    // A stalled receiver at shutdown never fails the service: SCM would restart
    // it. The receiver got no STOP.
    result.expect("shutdown with a stalled link is clean");
    assert!(!server.received.stopped.load(Ordering::Acquire));
    assert_eq!(get(&stats.disconnects), 0, "no stall timeout was reached");
    assert_eq!(get(&stats.queue_dropped), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
    // The spool held a backlog, so the live block the link gave back (the one in
    // hand) was older than it and could not go in front: it is counted, one large
    // frame. The replay batches in the FIFO (at most two) are counted too, because
    // their cursor moved.
    assert!(
        get(&stats.disconnected_dropped) <= 4,
        "{} frames counted",
        get(&stats.disconnected_dropped)
    );
    let mut on_disk = Vec::new();
    for name in segment_files(&dir) {
        let file = fs::read(dir.path().join(name)).expect("segment is readable");
        on_disk.extend(numbers_of(&parse_segment(&file)));
    }
    assert_eq!(on_disk.len() as u64, get(&stats.spooled));
    assert!(on_disk.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(
        get(&stats.spooled) + get(&stats.disconnected_dropped) <= u64::from(offered),
        "no frame is counted twice"
    );
}

#[test]
fn a_receiver_that_never_answers_finish_does_not_fail_the_shutdown() {
    let server = FakeServer::up();
    server.received.silent_finish.store(true, Ordering::Release);
    let stats = Arc::new(SinkStats::default());
    let mut sink = start_sink(&server, no_retry(), None, &stats);
    send(&mut sink, 0..100);
    wait_until("all frames to arrive", || server.received.len() >= 100);

    let started = Instant::now();
    let result = sink.finish();
    let elapsed = started.elapsed();
    eprintln!("shutdown with a receiver that never answers FINISH: {elapsed:?}");
    // The FINISH wait ends at the 2-second I/O deadline.
    assert!(
        elapsed < Duration::from_secs(5),
        "shutdown took {elapsed:?}"
    );
    result.expect("a missing FINISH is not a shutdown failure");
    // The failed closing exchange is neither reported nor counted. No frame is
    // lost or counted: all 100 arrived before the STOP.
    assert_eq!(get(&stats.disconnects), 0);
    assert_eq!(server.received.frames(), (0..100).collect::<Vec<_>>());
    assert_eq!(get(&stats.queue_dropped), 0);
    assert_eq!(get(&stats.disconnected_dropped), 0);
    assert_eq!(get(&stats.spool_dropped), 0);
}

#[test]
fn shutdown_with_a_backlog_seals_the_segment_and_the_next_run_replays_it() {
    let dir = TestDir::new("shutdown-backlog");
    let server = FakeServer::down();
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 8 * MIB, &stats);
    let mut sink = start_sink(&server, no_retry(), Some(spool), &stats);
    send(&mut sink, 0..30);
    wait_until("the frames to be spooled", || get(&stats.spooled) == 30);
    sink.finish().expect("shutdown while disconnected is clean");

    let files = segment_files(&dir);
    assert_eq!(files.len(), 1, "{files:?}");
    let file = fs::read(dir.path().join(&files[0])).expect("segment is readable");
    // `parse_segment` requires every record to decrypt and the end record last:
    // the segment is sealed.
    assert_eq!(
        numbers_of(&parse_segment(&file)),
        (0..30).collect::<Vec<_>>()
    );

    // The next run finds the leftover and sends it before newer frames.
    server.serve_now();
    let second_stats = Arc::new(SinkStats::default());
    let reopened = open_spool(&dir, 8 * MIB, &second_stats);
    let mut second_sink = start_sink(&server, no_retry(), Some(reopened), &second_stats);
    send(&mut second_sink, 100..110);
    wait_until("all frames to arrive", || server.received.len() >= 40);
    second_sink.finish().expect("clean STOP/FINISH");
    let expected: Vec<u32> = (0..30).chain(100..110).collect();
    assert_eq!(server.received.frames(), expected);
    assert!(get(&second_stats.replayed) >= 30);
    assert_eq!(get(&second_stats.disconnected_dropped), 0);
}

#[test]
fn shutdown_while_connected_with_no_backlog_completes_stop_finish() {
    let dir = TestDir::new("connected-spool");
    let server = FakeServer::up();
    let stats = Arc::new(SinkStats::default());
    let spool = open_spool(&dir, 8 * MIB, &stats);
    let mut sink = start_sink(&server, no_retry(), Some(spool), &stats);
    send(&mut sink, 0..100);
    wait_until("all frames to arrive", || server.received.len() >= 100);
    sink.finish().expect("STOP/FINISH completes");
    wait_until("STOP", || server.received.stopped.load(Ordering::Acquire));
    assert_eq!(server.received.frames(), (0..100).collect::<Vec<_>>());
    // Direct writes never touch the spool.
    assert_eq!(get(&stats.spooled), 0);
    assert_eq!(segment_files(&dir), Vec::<String>::new());
}

#[test]
fn write_frame_rejects_bad_lengths_without_changing_the_block() {
    let server = FakeServer::down();
    let stats = Arc::new(SinkStats::default());
    let mut sink = start_sink(&server, no_retry(), None, &stats);
    for payload in [vec![], vec![0; MAX_ENCODED_LENGTH + 1]] {
        let error = sink
            .write_frame(&payload)
            .expect_err("the length is invalid");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(sink.current.frames, 0);
        assert_eq!(sink.current.bytes.len(), 0);
    }
    sink.write_frame(&[7]).expect("one byte is valid");
    assert_eq!(sink.current.frames, 1);
    assert_eq!(sink.current.bytes, [0, 0, 0, 1, 7]);
    // A frame of the maximum length fills the block, which is sent at once.
    sink.write_frame(&vec![1; MAX_ENCODED_LENGTH])
        .expect("the maximum length is valid");
    assert_eq!(sink.current.frames, 0);
    assert_eq!(sink.current.bytes.len(), 0);
    assert!(sink.current.bytes.capacity() >= BLOCK_CAPACITY);
    sink.finish().expect("shutdown while disconnected is clean");
    assert_eq!(get(&stats.disconnected_dropped), 2);
}

#[path = "e2e_tests.rs"]
mod e2e;
