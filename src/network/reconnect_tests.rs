use super::test_support::{RefusedPort, WAIT_TIMEOUT, handshake, retry_policy, wait_until};
use super::*;
use crate::spool::test_support::capture_logs;
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
    let refused = RefusedPort::new();
    let stats = Arc::new(SinkStats::default());
    let mut sink = NetworkSink::start_with(refused.address(), Arc::clone(&stats), fast_policy())
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
fn reconnect_failures_are_logged_four_times_then_every_60th_or_on_a_new_cause() {
    let logged: Vec<u64> = (0..130)
        .filter(|&prior| reconnect_log_due(prior, false))
        .collect();
    assert_eq!(logged, [0, 1, 2, 3, 60, 120]);
    // The first failure of a cause in an outage is logged whatever the count.
    assert!(reconnect_log_due(61, true));
    assert!(!reconnect_log_due(61, false));
}

/// Applies the reconnect log rule to failures with these classes, whose prior
/// counts start at `first_prior`, and returns the prior counts that are logged.
fn logged_failures(
    seen: &mut SeenClasses,
    first_prior: u64,
    classes: impl IntoIterator<Item = FailureClass>,
) -> Vec<u64> {
    let mut logged = Vec::new();
    for (prior, class) in (first_prior..).zip(classes) {
        let new_class = seen.insert(class);
        if reconnect_log_due(prior, new_class) {
            logged.push(prior);
        }
    }
    logged
}

fn has(seen: SeenClasses, class: FailureClass) -> bool {
    seen.0 & class.bit() != 0
}

#[test]
fn alternating_causes_add_at_most_one_line_per_cause() {
    use FailureClass::{Connect, Timeout, Tls};
    // Started while down: the startup warning already reported `connect`.
    let mut seen = SeenClasses::default();
    seen.insert(Connect);
    // Failures with prior counts 10 to 199. The cause alternates between
    // connect and timeout, then from prior count 150 between connect and tls.
    let classes = (10..200_u64).map(|prior| {
        if prior.is_multiple_of(2) {
            Connect
        } else if prior < 150 {
            Timeout
        } else {
            Tls
        }
    });
    // 11 is the first timeout and 151 the first tls; 60, 120, and 180 are
    // due by the count. A rule that logs every change of cause would log
    // every one of these 190 failures.
    assert_eq!(
        logged_failures(&mut seen, 10, classes),
        [11, 60, 120, 151, 180]
    );
}

#[test]
fn a_session_clears_the_seen_causes_so_the_next_outage_reports_each_once() {
    use FailureClass::{Connect, Protocol, Timeout};
    let mut seen = SeenClasses::default();
    assert_eq!(
        logged_failures(&mut seen, 10, [Connect, Timeout, Connect, Timeout]),
        [10, 11]
    );
    assert!(has(seen, Connect) && has(seen, Timeout) && !has(seen, Protocol));
    // `NetworkSink::poll` clears the set when a session is established.
    seen.clear();
    assert_eq!(seen, SeenClasses::default());
    assert_eq!(
        logged_failures(
            &mut seen,
            14,
            [Timeout, Connect, Timeout, Protocol, Protocol]
        ),
        [14, 15, 17]
    );
}

#[test]
fn failure_classes_follow_the_error_kind_and_the_tls_payload() {
    let tls = |kind| io::Error::new(kind, TlsFailure("TLS handshake: bad".into()));
    let alert = |description| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            rustls::Error::AlertReceived(description),
        )
    };
    let cases = [
        (
            io::Error::from(io::ErrorKind::ConnectionRefused),
            FailureClass::Connect,
        ),
        (
            io::Error::from(io::ErrorKind::ConnectionReset),
            FailureClass::Connect,
        ),
        (
            io::Error::from(io::ErrorKind::TimedOut),
            FailureClass::Timeout,
        ),
        (
            io::Error::from(io::ErrorKind::WouldBlock),
            FailureClass::Timeout,
        ),
        (invalid_control("bad ACCEPT"), FailureClass::Protocol),
        (tls(io::ErrorKind::InvalidData), FailureClass::Tls),
        (tls(io::ErrorKind::ConnectionReset), FailureClass::Tls),
        (tls(io::ErrorKind::TimedOut), FailureClass::Timeout),
        // rustls reports a rejected record or a received alert after the
        // handshake (a later read or write) as `InvalidData` with this payload.
        (
            alert(rustls::AlertDescription::CertificateRequired),
            FailureClass::Tls,
        ),
        (
            alert(rustls::AlertDescription::HandshakeFailure),
            FailureClass::Tls,
        ),
    ];
    for (error, class) in cases {
        assert_eq!(FailureClass::of(&error), class, "{error:?}");
    }
    // The payload keeps the message text the log showed before.
    assert_eq!(
        tls(io::ErrorKind::InvalidData).to_string(),
        "TLS handshake: bad"
    );
    assert_eq!(
        alert(rustls::AlertDescription::CertificateRequired).to_string(),
        "received fatal alert: CertificateRequired"
    );
}

/// A receiver that answers `count` connections with an ACCEPT that lacks the
/// dnstap content type, then stops listening.
fn incompatible_receiver(listener: TcpListener, count: usize) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        for _ in 0..count {
            let (stream, _) = listener.accept().expect("sender connects");
            let mut stream = DeadlineTcp::new(stream, Duration::from_secs(3));
            read_control(&mut stream, CONTROL_READY, true).expect("READY arrives");
            write_control(&mut stream, CONTROL_ACCEPT, false).expect("incompatible ACCEPT");
        }
    })
}

/// A sink that started while its receiver was down (a refused connection),
/// now pointed at `address` for its reconnect attempts.
fn started_while_down(address: SocketAddr, stats: &Arc<SinkStats>) -> NetworkSink {
    let refused = RefusedPort::new();
    let mut sink = NetworkSink::start_with(refused.address(), Arc::clone(stats), fast_policy())
        .expect("a refused connection starts the sink disconnected");
    // Windows may report the refused loopback connect as a timeout instead.
    // Either way the startup warning reported that one cause.
    assert!(
        [FailureClass::Connect, FailureClass::Timeout]
            .iter()
            .any(|&class| sink.seen_failures == SeenClasses(class.bit())),
        "{:?}",
        sink.seen_failures
    );
    assert!(!sink.ever_connected);
    sink.config.address = address;
    sink
}

const RECONNECT_FAILED: &str = "dnstap reconnect failed";

#[test]
fn the_receiver_down_warnings_carry_the_receiver_down_id() {
    let refused = RefusedPort::new();
    let stats = Arc::new(SinkStats::default());
    let mut sink = None;
    let at_start = capture_logs(|| {
        sink = Some(
            NetworkSink::start_with(refused.address(), Arc::clone(&stats), fast_policy())
                .expect("a refused connection starts the sink disconnected"),
        );
    });
    assert_eq!(
        at_start.count("etw_dnstap::event::24"),
        1,
        "{}",
        at_start.text()
    );
    assert!(at_start.text().contains("unavailable at startup"));
    let mut sink = sink.expect("started");
    let lost = capture_logs(|| {
        sink.connection_lost(&io::Error::from(io::ErrorKind::ConnectionReset));
    });
    assert_eq!(lost.count("etw_dnstap::event::24"), 1, "{}", lost.text());
    assert!(lost.text().contains("connection lost"));
    // A failed attempt is a plain warning (ID 3), not the risk.
    assert_eq!(lost.count("dnstap reconnect failed"), 0);
}

#[test]
fn a_new_failure_cause_is_logged_late_in_an_outage() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let receiver = incompatible_receiver(listener, 2);
    let stats = Arc::new(SinkStats::default());
    let mut sink = started_while_down(address, &stats);
    // A link that had a session before, and a long outage: failure 11 would
    // not be logged by the count alone.
    sink.ever_connected = true;
    stats.reconnect_failures.store(10, Ordering::Relaxed);
    let lines = capture_logs(|| {
        wait_until(WAIT_TIMEOUT, "two refused handshakes", || {
            sink.write_frame(b"frame").expect("drops are not errors");
            stats.reconnect_failures.load(Ordering::Relaxed) >= 12
        });
    });
    receiver.join().expect("receiver assertions pass");
    // Failure 11 changes the cause from connect to protocol and is logged;
    // failure 12 has the same cause and is not.
    assert_eq!(lines.count(RECONNECT_FAILED), 1, "{}", lines.text());
    assert!(
        lines.text().contains("cause=\"protocol\""),
        "{}",
        lines.text()
    );
    assert!(lines.text().contains("failures=11"), "{}", lines.text());
    assert_eq!(lines.count("ERROR"), 0, "{}", lines.text());
    assert!(has(sink.seen_failures, FailureClass::Protocol));
    assert!(!sink.is_connected());
}

#[test]
fn a_refused_handshake_before_the_first_session_is_logged_once_as_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let receiver = incompatible_receiver(listener, 2);
    let stats = Arc::new(SinkStats::default());
    let mut sink = started_while_down(address, &stats);
    stats.reconnect_failures.store(10, Ordering::Relaxed);
    let lines = capture_logs(|| {
        wait_until(WAIT_TIMEOUT, "two refused handshakes", || {
            sink.write_frame(b"frame").expect("drops are not errors");
            stats.reconnect_failures.load(Ordering::Relaxed) >= 12
        });
    });
    receiver.join().expect("receiver assertions pass");
    assert_eq!(lines.count("ERROR"), 1, "{}", lines.text());
    assert_eq!(
        lines.count("refused the Frame Streams handshake before the first session"),
        1,
        "{}",
        lines.text()
    );
    assert!(
        lines.text().contains("dnstap content type"),
        "{}",
        lines.text()
    );
    // The second refusal has the same cause and a count that is not logged.
    assert_eq!(lines.count(RECONNECT_FAILED), 0, "{}", lines.text());
    assert!(sink.rejection_logged);
    assert!(!sink.is_connected(), "retries go on; nothing is fatal");
}

#[test]
fn a_cause_is_logged_again_in_the_outage_after_a_session() {
    let rejecting = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let rejecting_address = rejecting.local_addr().expect("listener address");
    let rejecter = incompatible_receiver(rejecting, 3);
    let accepting = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let accepting_address = accepting.local_addr().expect("listener address");
    let acceptor = thread::spawn(move || {
        let (stream, _) = accepting.accept().expect("reconnection");
        // One session, then the receiver goes away.
        drop(handshake(stream));
    });
    let stats = Arc::new(SinkStats::default());
    let mut sink = started_while_down(rejecting_address, &stats);
    stats.reconnect_failures.store(10, Ordering::Relaxed);
    let lines = capture_logs(|| {
        // Outage 1: failure 11 is the first rejection, logged once as an error.
        wait_until(WAIT_TIMEOUT, "the first refused handshake", || {
            sink.write_frame(b"frame").expect("drops are not errors");
            stats.reconnect_failures.load(Ordering::Relaxed) >= 11
        });
        // No attempt is in flight: the next one starts on a later poll.
        sink.config.address = accepting_address;
        wait_until(WAIT_TIMEOUT, "a session", || {
            sink.write_frame(b"frame").expect("drops are not errors");
            sink.flush().expect("flush");
            stats.reconnects.load(Ordering::Relaxed) >= 1
        });
        // Outage 2: the session ends and the receiver refuses again.
        sink.config.address = rejecting_address;
        wait_until(WAIT_TIMEOUT, "two more refused handshakes", || {
            sink.write_frame(b"frame").expect("drops are not errors");
            sink.flush().expect("flush");
            stats.reconnect_failures.load(Ordering::Relaxed) >= 13
        });
    });
    rejecter.join().expect("receiver assertions pass");
    acceptor.join().expect("receiver assertions pass");
    let text = lines.text();
    assert_eq!(lines.count("ERROR"), 1, "{text}");
    assert_eq!(stats.disconnects.load(Ordering::Relaxed), 1, "{text}");
    // Failure 12 is the first protocol failure of the new outage and is
    // logged, although outage 1 already reported that cause; failure 13 has
    // the same cause and is not logged.
    assert_eq!(lines.count(RECONNECT_FAILED), 1, "{text}");
    assert!(text.contains("cause=\"protocol\" failures=12"), "{text}");
    assert!(sink.ever_connected);
    assert_eq!(
        sink.seen_failures,
        SeenClasses(FailureClass::Protocol.bit())
    );
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
