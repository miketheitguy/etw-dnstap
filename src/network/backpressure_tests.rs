//! Slow and blackhole receivers: bounded waits and exact loss accounting.

use super::*;
use std::net::TcpListener;

/// A policy that never retries within a test, so counts are deterministic.
fn no_retry() -> BackoffPolicy {
    BackoffPolicy {
        initial: Duration::from_secs(600),
        max: Duration::from_secs(600),
    }
}

fn complete_handshake(stream: TcpStream) -> DeadlineTcp {
    let mut stream = DeadlineTcp::new(stream, Duration::from_secs(10));
    read_control(&mut stream, CONTROL_READY, true).expect("READY");
    write_control(&mut stream, CONTROL_ACCEPT, true).expect("ACCEPT");
    read_control(&mut stream, 2, true).expect("START");
    stream
}

#[test]
fn blackhole_receiver_at_startup_is_bounded_and_not_fatal() {
    // Accepts the connection (the kernel does) but never reads or answers.
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let stats = Arc::new(SinkStats::default());
    let started = Instant::now();
    let mut sink =
        NetworkSink::start_with(address, Arc::clone(&stats), no_retry()).expect("not fatal");
    let waited = started.elapsed();
    assert!(
        waited < IO_TIMEOUT + Duration::from_secs(3),
        "the handshake deadline must bound the wait, took {waited:?}"
    );
    assert!(!stats.connected.load(Ordering::Relaxed));
    // Every frame while down is counted, never an error, and never resent.
    for _ in 0..5 {
        sink.write_frame(&[1, 2, 3]).expect("drops are not errors");
    }
    sink.flush().expect("flush while down is a no-op");
    assert_eq!(stats.disconnected_dropped.load(Ordering::Relaxed), 5);
    sink.finish().expect("shutdown while down is clean");
    drop(listener);
}

#[test]
fn receiver_that_stops_reading_causes_one_bounded_stall_then_counted_drops() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let (release, hold) = mpsc::channel::<()>();
    let receiver = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("sender connects");
        let stream = complete_handshake(stream);
        // Never read again: the socket buffers fill and the sender must block.
        hold.recv_timeout(Duration::from_secs(60)).ok();
        drop(stream);
    });
    let stats = Arc::new(SinkStats::default());
    let mut sink =
        NetworkSink::start_with(address, Arc::clone(&stats), no_retry()).expect("connects");
    assert!(stats.connected.load(Ordering::Relaxed));
    let frame = vec![0x5a; 32 * 1024];
    let started = Instant::now();
    let mut offered = 0_u64;
    while stats.disconnects.load(Ordering::Relaxed) == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "a stalled receiver must end the session within the write deadline"
        );
        assert!(offered < 4096, "kernel buffering absorbed 128 MiB");
        sink.write_frame(&frame).expect("loss is never an error");
        offered += 1;
    }
    // One stall of at most IO_TIMEOUT (plus scheduling) ended the session.
    assert!(started.elapsed() < Duration::from_secs(20));
    assert!(!stats.connected.load(Ordering::Relaxed));
    assert_eq!(stats.disconnects.load(Ordering::Relaxed), 1);
    assert!(stats.slow_writes.load(Ordering::Relaxed) >= 1);
    assert!(stats.write_max_us.load(Ordering::Relaxed) >= 1_000_000);
    let dropped_at_loss = stats.disconnected_dropped.load(Ordering::Relaxed);
    assert!(dropped_at_loss >= 1, "the frame that failed is counted");
    assert!(dropped_at_loss <= offered, "never more drops than frames");
    // Afterwards each frame is counted exactly once, immediately, with no wait.
    let before = Instant::now();
    for _ in 0..10 {
        sink.write_frame(&frame).expect("drop, not error");
    }
    sink.flush().expect("flush while down");
    assert!(before.elapsed() < Duration::from_millis(500));
    assert_eq!(
        stats.disconnected_dropped.load(Ordering::Relaxed),
        dropped_at_loss + 10
    );
    sink.finish().expect("shutdown while disconnected is clean");
    release.send(()).ok();
    receiver.join().expect("receiver ends");
}
