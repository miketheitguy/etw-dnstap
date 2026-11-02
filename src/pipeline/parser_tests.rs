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
            stream.set_read_timeout(Some(Duration::from_secs(5)))?;
            stream.set_write_timeout(Some(Duration::from_secs(5)))?;
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
    drain_queue(&queue, &done, |value| {
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
    drain_queue(&queue, &done, |value| {
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
            let result = drain_queue(&queue, &done, |value| {
                if let Some(value) = value {
                    values.push(value);
                } else if let Some(send) = idle_send.take() {
                    send.send(()).context("report idle parser")?;
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
    drain_queue(&queue, &done, |_| {
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
    let error = drain_queue(&queue, &AtomicBool::new(true), |_| bail!("output failed"))
        .expect_err("output error must escape");
    assert_eq!(error.to_string(), "output failed");
}

#[cfg(feature = "diag-timing")]
#[test]
fn diag_timing_counts_parks_runs_and_wake_lengths() {
    let (send, queue) = crossbeam_channel::bounded(16);
    for value in 0..5 {
        send.try_send(value).expect("space available");
    }
    let done = AtomicBool::new(false);
    let timing = DiagTiming::default();
    let mut idle_calls = 0;
    // Idle call 1 follows data: the linger finds nothing and the parser parks
    // (an empty wake). Idle call 2 enqueues before the second park. Idle call 3
    // follows data again and completes, which ends the linger without a park.
    drain_queue_inner(&queue, &done, Some(&timing), |value| {
        if value.is_none() {
            idle_calls += 1;
            match idle_calls {
                1 => {}
                2 => {
                    for value in 0..3 {
                        send.try_send(value).expect("space available");
                    }
                }
                _ => done.store(true, Ordering::Release),
            }
        }
        Ok(())
    })
    .expect("drain succeeds");
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    assert_eq!(idle_calls, 3);
    assert_eq!(load(&timing.parks), 2);
    // The second park follows an empty one, so it backs off to 4 ms.
    assert_eq!(load(&timing.backoff_parks), 1);
    assert!(load(&timing.park_ns) > 0);
    assert_eq!(load(&timing.max_run), 5);
    assert_eq!(load(&timing.wake_len_max), 3);
    assert_eq!(load(&timing.wake_len_0), 1);
    assert_eq!(load(&timing.wake_len_lt_1k), 1);
    let histogram = load(&timing.wake_len_0)
        + load(&timing.wake_len_lt_1k)
        + load(&timing.wake_len_lt_4k)
        + load(&timing.wake_len_lt_8k)
        + load(&timing.wake_len_ge_8k_or_full);
    assert_eq!(histogram, load(&timing.parks));
}

#[cfg(feature = "diag-timing")]
#[test]
fn idle_backoff_counts_and_event_resets_it() {
    let (send, queue) = crossbeam_channel::bounded(4);
    let done = AtomicBool::new(false);
    let timing = DiagTiming::default();
    let mut idle_calls = 0;
    let mut before_event = 0;
    drain_queue_inner(&queue, &done, Some(&timing), |value| {
        if value.is_none() {
            idle_calls += 1;
            match idle_calls {
                3 => {
                    before_event = timing.backoff_parks.load(Ordering::Relaxed);
                    send.try_send(1).expect("space available");
                }
                5 => done.store(true, Ordering::Release),
                _ => {}
            }
        }
        Ok(())
    })
    .expect("drain succeeds");
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    // Parks 1 and 2 requested 2 ms and 4 ms, so one backoff park precedes the
    // event. Park 3 (8 ms) backs off; the event resets the poll, so park 4
    // (2 ms) is not counted; park 5 (4 ms) is.
    assert_eq!(before_event, 1);
    assert_eq!(load(&timing.parks), 5);
    assert_eq!(load(&timing.backoff_parks), 3);
}

#[cfg(feature = "diag-timing")]
#[test]
fn diag_timing_buckets_and_slow_calls() {
    let timing = DiagTiming::default();
    for (len, capacity) in [
        (0, Some(4)),
        (3, Some(4)),
        (4, Some(4)),
        (1023, Some(16_384)),
        (1024, Some(16_384)),
        (4095, None),
        (4096, None),
        (8191, None),
        (8192, None),
    ] {
        timing.record_wake(len, capacity);
    }
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    assert_eq!(load(&timing.wake_len_0), 1);
    assert_eq!(load(&timing.wake_len_lt_1k), 2);
    assert_eq!(load(&timing.wake_len_lt_4k), 2);
    assert_eq!(load(&timing.wake_len_lt_8k), 2);
    assert_eq!(load(&timing.wake_len_ge_8k_or_full), 2);
    assert_eq!(load(&timing.wake_len_max), 8192);
    timing.record_write(Duration::from_millis(1));
    timing.record_write(Duration::from_millis(2));
    timing.record_flush(Duration::from_micros(10));
    timing.record_flush(Duration::from_millis(3));
    assert_eq!(load(&timing.writes_over_1ms), 1);
    assert_eq!(load(&timing.write_ns), 3_000_000);
    assert_eq!(load(&timing.flushes_over_1ms), 1);
    assert_eq!(load(&timing.flush_ns), 3_010_000);
}
