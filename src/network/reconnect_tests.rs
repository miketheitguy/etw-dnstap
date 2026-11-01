use super::*;
use std::net::TcpListener;

fn fast_policy() -> BackoffPolicy {
    BackoffPolicy {
        initial: Duration::from_millis(10),
        max: Duration::from_millis(40),
    }
}

/// Completes the receiver side of the Frame Streams handshake.
fn handshake(stream: TcpStream) -> DeadlineTcp {
    let mut stream = DeadlineTcp::new(stream, Duration::from_secs(10));
    read_control(&mut stream, CONTROL_READY, true).expect("READY");
    write_control(&mut stream, CONTROL_ACCEPT, true).expect("ACCEPT");
    read_control(&mut stream, 2, true).expect("START");
    stream
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
    // Keep offering frames through the outage until the sink is back.
    let give_up = Instant::now() + Duration::from_secs(20);
    let mut sequence = 1000u32;
    let mut first_after_reconnect;
    loop {
        assert!(Instant::now() < give_up, "sink never reconnected");
        first_after_reconnect = sequence;
        sink.write_frame(&sequence.to_be_bytes()).expect("write");
        sink.flush().expect("flush");
        sequence += 1;
        if stats.reconnects.load(Ordering::Relaxed) >= 1 {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    for sequence in 100..103u32 {
        sink.write_frame(&sequence.to_be_bytes()).expect("write");
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
    thread::sleep(Duration::from_millis(80));
    assert!(transport.remaining().is_err(), "the armed window expired");
    transport.rearm();
    thread::sleep(Duration::from_millis(80));
    // Idle time before the operation's first I/O does not count against it.
    assert!(transport.remaining().is_ok());
    assert!(transport.armed);
}
