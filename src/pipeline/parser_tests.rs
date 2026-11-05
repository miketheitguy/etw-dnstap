//! The parser thread loop, the channel drain, and the idle back-off.

use super::*;
use anyhow::bail;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
};

#[test]
fn completed_parser_finishes_tcp_and_reports_receiver_failure() -> Result<()> {
    for acknowledge_finish in [true, false] {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?;
        let receiver = thread::spawn(move || -> std::io::Result<()> {
            let (mut stream, _) = listener.accept()?;
            stream.set_read_timeout(Some(Duration::from_secs(20)))?;
            stream.set_write_timeout(Some(Duration::from_secs(20)))?;
            let mut control = [0; 42];
            stream.read_exact(&mut control)?;
            assert_eq!(&control[8..12], &4_u32.to_be_bytes(), "READY");
            control[8..12].copy_from_slice(&1_u32.to_be_bytes());
            stream.write_all(&control)?;
            stream.read_exact(&mut control)?;
            assert_eq!(&control[8..12], &2_u32.to_be_bytes(), "START");
            let mut stop = [0; 12];
            stream.read_exact(&mut stop)?;
            assert_eq!(stop, [0, 0, 0, 0, 0, 0, 0, 4, 0, 0, 0, 3]);
            if acknowledge_finish {
                stop[8..12].copy_from_slice(&5_u32.to_be_bytes());
                stream.write_all(&stop)?;
            }
            Ok(())
        });
        let writer = crate::network::Connection::connect(address)?;
        let (_send, queue) = crossbeam_channel::bounded(1);
        let counters = ParseCounters::default();
        let result = parse_loop(
            &queue,
            &AtomicBool::new(true),
            &counters,
            "test",
            writer,
            DiagContext::default(),
        );
        receiver.join().expect("receiver completes")?;
        assert_eq!(result.is_ok(), acknowledge_finish);
        assert_eq!(
            counters.output_errors.load(Ordering::Relaxed),
            u64::from(!acknowledge_finish)
        );
        assert_eq!(counters.encoded.load(Ordering::Relaxed), 0);
        assert_eq!(counters.output_failed_events.load(Ordering::Relaxed), 0);
        if let Err(error) = result {
            assert!(error.to_string().contains("STOP/FINISH"));
        }
    }
    Ok(())
}

#[test]
fn ring_drops_newest_and_drains_in_order_after_completion() {
    let (send, queue) = crossbeam_channel::bounded(2);
    send.try_send(1).expect("space available");
    send.try_send(2).expect("space available");
    assert_eq!(
        send.try_send(3),
        Err(crossbeam_channel::TrySendError::Full(3))
    );
    let done = AtomicBool::new(true);
    let mut values = Vec::new();
    drain_queue(&queue, &done, &DiagTiming::default(), |value| {
        values.extend(value);
        Ok(())
    })
    .expect("drain succeeds");
    assert_eq!(values, [1, 2]);
}

#[test]
fn completion_publication_keeps_final_event() {
    let (send, queue) = crossbeam_channel::bounded(1);
    let done = AtomicBool::new(false);
    let mut values = Vec::new();
    drain_queue(&queue, &done, &DiagTiming::default(), |value| {
        if let Some(value) = value {
            values.push(value);
        } else {
            send.try_send(42).expect("empty queue");
            done.store(true, Ordering::Release);
        }
        Ok(())
    })
    .expect("drain succeeds");
    assert_eq!(values, [42]);
}

#[test]
fn idle_parser_drains_final_enqueue_before_joined_producer_completion() -> Result<()> {
    let (send, queue) = crossbeam_channel::bounded(1);
    let done = Arc::new(AtomicBool::new(false));
    let (idle_send, idle_receive) = mpsc::channel();
    let (finished_send, finished_receive) = mpsc::channel();
    let parser = {
        let queue = queue.clone();
        let done = Arc::clone(&done);
        thread::spawn(move || {
            let mut idle_send = Some(idle_send);
            let mut values = Vec::new();
            let result = drain_queue(&queue, &done, &DiagTiming::default(), |value| {
                if let Some(value) = value {
                    values.push(value);
                } else if let Some(idle_sender) = idle_send.take() {
                    idle_sender.send(()).context("report idle parser")?;
                }
                Ok(())
            });
            let _sent = finished_send.send((result, values));
        })
    };
    let idle = idle_receive.recv_timeout(Duration::from_secs(5));
    if idle.is_ok() {
        let producer = {
            let send = send.clone();
            let parser = parser.thread().clone();
            thread::spawn(move || {
                send.try_send(42).expect("final event fits");
                parser.unpark();
            })
        };
        producer
            .join()
            .expect("producer joins before done publication");
    }
    done.store(true, Ordering::Release);
    parser.thread().unpark();
    let finished = finished_receive.recv_timeout(Duration::from_secs(5));
    parser.join().expect("parser joins after completion");
    idle.context("parser did not become idle")?;
    let (result, values) = finished.context("parser did not finish")?;
    result?;
    assert_eq!(values, [42]);
    Ok(())
}

#[test]
fn startup_failure_completion_finishes_an_empty_queue() -> Result<()> {
    let (_send, queue) = crossbeam_channel::bounded::<u8>(1);
    let done = AtomicBool::new(true);
    drain_queue(&queue, &done, &DiagTiming::default(), |_| {
        bail!("empty completed queue must not consume")
    })
}

#[test]
fn idle_poll_backoff_doubles_and_clamps() {
    assert_eq!(
        idle_poll_after(false, Duration::from_millis(2)),
        Duration::from_millis(4)
    );
    assert_eq!(
        idle_poll_after(false, Duration::from_millis(4)),
        Duration::from_millis(8)
    );
    assert_eq!(
        idle_poll_after(false, Duration::from_millis(8)),
        Duration::from_millis(16)
    );
    assert_eq!(
        idle_poll_after(false, Duration::from_millis(16)),
        IDLE_POLL_MAX
    );
    assert_eq!(idle_poll_after(false, IDLE_POLL_MAX), IDLE_POLL_MAX);
}

#[test]
fn idle_poll_resets_to_minimum_after_an_event() {
    assert_eq!(idle_poll_after(true, IDLE_POLL_MAX), IDLE_POLL_MIN);
    assert_eq!(
        idle_poll_after(true, Duration::from_millis(8)),
        IDLE_POLL_MIN
    );
    assert_eq!(idle_poll_after(true, IDLE_POLL_MIN), IDLE_POLL_MIN);
}

#[test]
fn consumer_failure_is_propagated() {
    let (send, queue) = crossbeam_channel::bounded(1);
    send.try_send(1).expect("space available");
    let error = drain_queue(
        &queue,
        &AtomicBool::new(true),
        &DiagTiming::default(),
        |_| bail!("output failed"),
    )
    .expect_err("output error must escape");
    assert_eq!(error.to_string(), "output failed");
}

#[test]
fn rejection_log_cadence_is_first_four_then_every_sixtieth() {
    let logged: Vec<u64> = (0..200)
        .filter(|previous| should_log_rejection(*previous))
        .map(|previous| previous + 1)
        .collect();
    assert_eq!(logged, [1, 2, 3, 4, 61, 121, 181]);
}
