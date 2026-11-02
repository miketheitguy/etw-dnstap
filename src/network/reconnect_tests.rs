use super::test_support::{WAIT_TIMEOUT, handshake, retry_policy, wait_until};
use super::*;
use std::net::TcpListener;

/// Retries within tens of milliseconds, so reconnect tests finish quickly.
fn fast_policy() -> BackoffPolicy {
    retry_policy(10, 40)
}

enum Item {
    Data(u32),
    Control(u32),
}

/// Reads one frame whose data payload is a big-endian sequence number.
fn next_item(stream: &mut DeadlineTcp) -> Item {
    let mut length = [0; 4];
    stream.read_exact(&mut length).expect("frame length");
    if length == [0; 4] {
        let mut control = [0; 4];
        stream.read_exact(&mut control).expect("control length");
        let mut body = vec![0; u32::from_be_bytes(control) as usize];
        stream.read_exact(&mut body).expect("control body");
        return Item::Control(u32::from_be_bytes([body[0], body[1], body[2], body[3]]));
    }
    assert_eq!(u32::from_be_bytes(length), 4, "data frames are 4 bytes");
    let mut payload = [0; 4];
    stream.read_exact(&mut payload).expect("frame payload");
    Item::Data(u32::from_be_bytes(payload))
}

#[test]
fn reconnects_with_a_fresh_handshake_and_never_resends() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let receiver = thread::spawn(move || {
        let (first_stream, _) = listener.accept().expect("first connection");
        let mut first_stream = handshake(first_stream);
        let first: Vec<u32> = (0..3)
            .map(|_| match next_item(&mut first_stream) {
                Item::Data(sequence) => sequence,
                Item::Control(kind) => panic!("unexpected control {kind}"),
            })
            .collect();
        // The receiver goes away: the sender's session is dead.
        drop(first_stream);
        let (second_stream, _) = listener.accept().expect("reconnection");
        let mut second_stream = handshake(second_stream);
        let mut second = Vec::new();
        loop {
            match next_item(&mut second_stream) {
                Item::Data(sequence) => second.push(sequence),
                Item::Control(3) => {
                    write_control(&mut second_stream, CONTROL_FINISH, false).expect("FINISH");
                    break;
                }
                Item::Control(kind) => panic!("unexpected control {kind}"),
            }
        }
        (first, second)
    });
    let stats = Arc::new(SinkStats::default());
    let mut sink =
        NetworkSink::start_with(address, Arc::clone(&stats), fast_policy()).expect("connects");
    assert!(stats.connected.load(Ordering::Relaxed));
    for sequence in 0..3u32 {
        sink.write_frame(&sequence.to_be_bytes()).expect("write");
    }
    sink.flush().expect("flush");
    // Keep offering frames through the outage until the sink is back; the sink
    // notices the outage and the finished reconnect only on a write or flush.
    let mut sequence = 1000u32;
    let mut first_after_reconnect = sequence;
    let mut reconnected_by_last_flush = false;
    wait_until(WAIT_TIMEOUT, "the sink to reconnect", || {
        first_after_reconnect = sequence;
        sink.write_frame(&sequence.to_be_bytes()).expect("write");
        // This frame went out on the new session only if the reconnect had
        // finished before or during the write. A reconnect that finishes inside
        // flush() below dropped the frame, so it is acted on by the next
        // iteration, whose write goes out on the new session.
        let on_new_session =
            reconnected_by_last_flush || stats.reconnects.load(Ordering::Relaxed) >= 1;
        sink.flush().expect("flush");
        reconnected_by_last_flush = stats.reconnects.load(Ordering::Relaxed) >= 1;
        sequence += 1;
        on_new_session
    });
    for frame_number in 100..103u32 {
        sink.write_frame(&frame_number.to_be_bytes())
            .expect("write");
    }
    sink.finish().expect("clean STOP/FINISH on the new session");
    let (first, second) = receiver.join().expect("receiver assertions pass");
    assert_eq!(first, [0, 1, 2]);
    // Only frames offered after the reconnect arrive; nothing written while the
    // session was dead (or buffered on it) is resent.
    assert_eq!(second, [first_after_reconnect, 100, 101, 102]);
    assert_eq!(stats.disconnects.load(Ordering::Relaxed), 1);
    assert_eq!(stats.reconnects.load(Ordering::Relaxed), 1);
    assert!(stats.disconnected_dropped.load(Ordering::Relaxed) >= 1);
    assert!(stats.connected.load(Ordering::Relaxed));
}

#[test]
fn unreachable_receiver_at_startup_is_not_fatal_and_shutdown_is_prompt() {
    let address = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
        listener.local_addr().expect("listener address")
    };
    let stats = Arc::new(SinkStats::default());
    let mut sink = NetworkSink::start_with(address, Arc::clone(&stats), fast_policy())
        .expect("a refused connection starts the sink disconnected");
    assert!(!stats.connected.load(Ordering::Relaxed));
    for _ in 0..5 {
        sink.write_frame(b"frame").expect("drops are not errors");
    }
    sink.flush().expect("flush while disconnected succeeds");
    assert_eq!(stats.disconnected_dropped.load(Ordering::Relaxed), 5);
    let started = Instant::now();
    sink.finish().expect("finish while disconnected succeeds");
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn incompatible_receiver_at_startup_is_still_fatal() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let receiver = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("sender connects");
        let mut stream = DeadlineTcp::new(stream, Duration::from_secs(3));
        read_control(&mut stream, CONTROL_READY, true).expect("READY arrives");
        write_control(&mut stream, CONTROL_ACCEPT, false).expect("incompatible ACCEPT");
    });
    let result = NetworkSink::start_with(address, Arc::default(), fast_policy());
    assert!(matches!(result, Err(error) if error.kind() == io::ErrorKind::InvalidData));
    receiver.join().expect("receiver assertions pass");
}

#[test]
fn backoff_doubles_to_the_cap_with_bounded_jitter_and_resets() {
    let policy = BackoffPolicy::default();
    let mut backoff = Backoff::new(policy, 0x1234_5678);
    let mut base = policy.initial;
    for _ in 0..12 {
        let delay = backoff.next_delay();
        assert!(delay >= base / 2, "{delay:?} below half of {base:?}");
        assert!(delay <= base, "{delay:?} above {base:?}");
        assert!(delay <= policy.max);
        base = base.saturating_mul(2).min(policy.max);
    }
    assert_eq!(base, policy.max, "the cap is reached");
    backoff.reset();
    assert!(backoff.next_delay() <= policy.initial);
}

#[test]
fn jitter_spans_half_to_full_base() {
    let base = Duration::from_secs(8);
    assert_eq!(jittered(base, 0), Duration::from_secs(4));
    let high = jittered(base, u32::MAX);
    assert!(high < base && high > Duration::from_millis(7990));
    // Different seeds do not retry in lockstep.
    let mut a = Backoff::new(BackoffPolicy::default(), 1);
    let mut b = Backoff::new(BackoffPolicy::default(), 2);
    assert_ne!(a.next_delay(), b.next_delay());
}

fn loopback_stream() -> TcpStream {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let stream = TcpStream::connect(listener.local_addr().expect("address")).expect("connects");
    let _ = listener.accept();
    stream
}

#[test]
fn a_socket_timeout_is_kept_only_if_it_is_not_short_and_at_most_the_slack_long() {
    let slack = Duration::from_millis(5);
    let second = Duration::from_secs(1);
    // Nothing applied yet.
    assert!(timeout_needs_update(None, second, slack));
    // Applied equal, or longer by up to the slack: kept.
    assert!(!timeout_needs_update(Some(second), second, slack));
    assert!(!timeout_needs_update(Some(second + slack), second, slack));
    // Longer than the slack: the call could run past the deadline.
    assert!(timeout_needs_update(
        Some(second + slack + Duration::from_nanos(1)),
        second,
        slack
    ));
    // Shorter: the call could time out before the deadline.
    assert!(timeout_needs_update(
        Some(second.saturating_sub(Duration::from_nanos(1))),
        second,
        slack
    ));
}

#[test]
fn the_slack_is_the_smaller_of_five_milliseconds_and_a_32nd_of_the_timeout() {
    let slack = |timeout| DeadlineTcp::new(loopback_stream(), timeout).slack;
    assert_eq!(slack(IO_TIMEOUT), TIMEOUT_SLACK);
    assert_eq!(
        slack(Duration::from_millis(100)),
        Duration::from_micros(3125)
    );
    assert_eq!(
        slack(Duration::from_millis(50)),
        Duration::from_nanos(1_562_500)
    );
}

#[test]
fn the_write_timeout_is_set_once_per_window_and_again_when_the_time_left_moves() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let stream = TcpStream::connect(listener.local_addr().expect("address")).expect("connects");
    let (_peer, _) = listener.accept().expect("accepted");
    let mut transport = DeadlineTcp::new(stream, Duration::from_secs(3));
    assert_eq!(transport.write_applied, None);
    transport.write_all(&[1]).expect("first write");
    let first = transport
        .write_applied
        .expect("the first write sets a timeout");
    assert!(first <= Duration::from_secs(3) && first > Duration::from_millis(2900));
    // The time left shrank by microseconds: the timeout is not set again.
    transport.write_all(&[2]).expect("second write");
    assert_eq!(transport.write_applied, Some(first));
    // Well past the slack: it is set again, to the smaller time left.
    thread::sleep(Duration::from_millis(40));
    transport.write_all(&[3]).expect("third write");
    let later = transport.write_applied.expect("still set");
    assert!(
        later < first.saturating_sub(Duration::from_millis(30)),
        "{later:?} {first:?}"
    );
    // A new window has nearly the whole timeout left again, which is longer
    // than the timeout applied, so the timeout is set again.
    transport.rearm();
    transport.write_all(&[4]).expect("fourth write");
    assert!(transport.write_applied.expect("still set") > later);
    // The read timeout has its own cache.
    assert_eq!(transport.read_applied, None);
}

#[test]
fn the_deadline_still_ends_a_blocked_write_when_the_timeout_was_kept() {
    // A receiver that never reads fills the socket buffers; the write must
    // fail near the 150 ms deadline, not run on to a timeout kept from earlier.
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let stream = TcpStream::connect(listener.local_addr().expect("address")).expect("connects");
    let (_peer, _) = listener.accept().expect("accepted");
    let mut transport = DeadlineTcp::new(stream, Duration::from_millis(150));
    transport
        .write_all(&[0])
        .expect("a small write fits the buffers");
    transport.rearm();
    let chunk = vec![0_u8; 1 << 20];
    let started = Instant::now();
    let error = loop {
        match transport.write_all(&chunk) {
            Ok(()) => assert!(started.elapsed() < Duration::from_secs(5), "never blocked"),
            Err(error) => break error,
        }
    };
    assert!(
        matches!(
            error.kind(),
            io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
        ),
        "{error:?}"
    );
    assert!(started.elapsed() < Duration::from_millis(150) + Duration::from_secs(2));
}

#[test]
fn slow_socket_writes_are_counted_with_their_maximum() {
    let stats = Arc::new(SinkStats::default());
    let mut transport = DeadlineTcp::new(loopback_stream(), Duration::from_secs(1));
    transport.stats = Some(Arc::clone(&stats));
    transport.record_write(Duration::from_millis(1));
    assert_eq!(stats.slow_writes.load(Ordering::Relaxed), 0);
    transport.record_write(SLOW_WRITE);
    transport.record_write(Duration::from_millis(40));
    assert_eq!(stats.slow_writes.load(Ordering::Relaxed), 2);
    assert_eq!(stats.write_max_us.load(Ordering::Relaxed), 40_000);
}

#[test]
fn deadline_clock_starts_at_the_first_io_of_each_operation() {
    let mut transport = DeadlineTcp::new(loopback_stream(), Duration::from_millis(50));
    wait_until(WAIT_TIMEOUT, "the armed window to expire", || {
        transport.remaining().is_err()
    });
    transport.rearm();
    // This sleep is the scenario, not a wait for a condition: the operation
    // sits idle for longer than the 50 ms window before its first I/O. Idle
    // time does not count against the deadline; the clock starts at the first
    // remaining() call below, so nearly the whole window is still left.
    thread::sleep(Duration::from_millis(80));
    let remaining = transport
        .remaining()
        .expect("the clock starts at the first I/O");
    assert!(remaining > Duration::from_millis(40), "{remaining:?} left");
    assert!(transport.armed);
}
