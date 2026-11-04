//! `Uplink::close` for `NetworkSink`: a closing STOP/FINISH exchange that does not
//! complete never fails the service, and the service writes nothing about it. It
//! returns `Ok` within the I/O deadline, because every data frame was already
//! accepted by the socket, handed back, or counted.

use super::test_support::{handshake, retry_policy};
use super::*;
use crate::spool::test_support::capture_logs_at;
use std::net::TcpListener;

/// A sink connected to a loopback receiver, with the socket in link mode.
fn linked_sink(address: SocketAddr, stats: &Arc<SinkStats>) -> NetworkSink {
    let mut sink =
        NetworkSink::start_with(address, Arc::clone(stats), retry_policy(600_000, 600_000))
            .expect("connects");
    Uplink::poll(&mut sink, Instant::now());
    assert!(
        Uplink::wire(&mut sink).is_some(),
        "the session is in link mode"
    );
    sink
}

#[test]
fn a_receiver_that_never_answers_finish_does_not_fail_the_close() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let (release, hold) = mpsc::channel::<()>();
    let receiver = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("sender connects");
        let mut stream = handshake(stream);
        read_control(&mut stream, 3, false).expect("STOP arrives");
        // FINISH never comes.
        hold.recv_timeout(Duration::from_secs(10)).ok();
    });
    let stats = Arc::new(SinkStats::default());
    let sink = linked_sink(address, &stats);

    let started = Instant::now();
    let mut result = None;
    let logs = capture_logs_at(tracing::Level::INFO, || {
        result = Some(Uplink::close(sink, true));
    });
    let elapsed = started.elapsed();
    release.send(()).ok();
    receiver.join().expect("receiver assertions pass");

    result
        .expect("close ran")
        .expect("a missing FINISH is not a shutdown failure");
    // The FINISH wait ends at the I/O deadline.
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    // Nothing is reported, and no frame is counted.
    assert_eq!(logs.text(), "", "the service writes nothing");
    assert_eq!(stats.disconnects.load(Ordering::Relaxed), 0);
    assert_eq!(stats.disconnected_dropped.load(Ordering::Relaxed), 0);
}

#[test]
fn a_receiver_that_stopped_reading_does_not_fail_the_close() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let (release, hold) = mpsc::channel::<()>();
    let receiver = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("sender connects");
        let stream = handshake(stream);
        // Never read again: the socket buffers fill.
        hold.recv_timeout(Duration::from_secs(30)).ok();
        drop(stream);
    });
    let stats = Arc::new(SinkStats::default());
    let mut sink = linked_sink(address, &stats);
    // Fill the send buffer with whole blocks, as the link thread does, until the
    // socket takes nothing: the stall of a paused receiver at shutdown.
    let block = vec![0x5a_u8; 64 * 1024];
    let wire = Uplink::wire(&mut sink).expect("link mode");
    let mut offered = 0;
    while wire.offer(&block).expect("offer").taken > 0 {
        offered += 1;
        assert!(offered < 4096, "kernel buffering absorbed 256 MiB");
    }

    // STOP cannot enter the full buffer. This used to fail the service.
    let started = Instant::now();
    let mut result = None;
    let logs = capture_logs_at(tracing::Level::INFO, || {
        result = Some(Uplink::close(sink, true));
    });
    let elapsed = started.elapsed();
    release.send(()).ok();
    receiver.join().expect("receiver ends");

    result
        .expect("close ran")
        .expect("a STOP that cannot be written is not a shutdown failure");
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    assert_eq!(logs.text(), "", "the service writes nothing");
    assert_eq!(stats.disconnects.load(Ordering::Relaxed), 0);
    assert_eq!(stats.disconnected_dropped.load(Ordering::Relaxed), 0);
}

#[test]
fn a_close_with_a_cut_frame_sends_no_stop_writes_nothing_and_counts_nothing() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let receiver = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("sender connects");
        let mut stream = handshake(stream);
        // The sender closes the connection with no STOP: the next read ends.
        let mut byte = [0_u8; 1];
        assert_eq!(stream.read(&mut byte).expect("a clean end"), 0);
    });
    let stats = Arc::new(SinkStats::default());
    let sink = linked_sink(address, &stats);
    let started = Instant::now();
    let mut result = None;
    let logs = capture_logs_at(tracing::Level::INFO, || {
        result = Some(Uplink::close(sink, false));
    });
    receiver
        .join()
        .expect("the receiver saw the close with no STOP");
    result.expect("close ran").expect("no STOP is not an error");
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(stats.disconnects.load(Ordering::Relaxed), 0);
    assert_eq!(logs.text(), "", "the service writes nothing");
}

#[test]
fn a_close_that_completes_writes_nothing_and_counts_nothing() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("loopback listener");
    let address = listener.local_addr().expect("listener address");
    let receiver = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("sender connects");
        let mut stream = handshake(stream);
        read_control(&mut stream, 3, false).expect("STOP arrives");
        write_control(&mut stream, CONTROL_FINISH, false).expect("FINISH");
    });
    let stats = Arc::new(SinkStats::default());
    let sink = linked_sink(address, &stats);
    let mut result = None;
    let logs = capture_logs_at(tracing::Level::INFO, || {
        result = Some(Uplink::close(sink, true));
    });
    receiver.join().expect("receiver assertions pass");
    result
        .expect("close ran")
        .expect("STOP and FINISH complete");
    assert_eq!(logs.text(), "", "{}", logs.text());
    assert_eq!(stats.disconnects.load(Ordering::Relaxed), 0);
}
