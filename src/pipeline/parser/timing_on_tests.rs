//! Wait attribution, run lengths, and the slow-call counters.

use super::*;
use crate::pipeline::parser::drain_queue;
use std::sync::atomic::AtomicBool;

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
    drain_queue(&queue, &done, &timing, |value| {
        if value.is_none() {
            idle_calls += 1;
            match idle_calls {
                1 => {}
                2 => {
                    for item in 0..3 {
                        send.try_send(item).expect("space available");
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

#[test]
fn idle_backoff_counts_and_event_resets_it() {
    let (send, queue) = crossbeam_channel::bounded(4);
    let done = AtomicBool::new(false);
    let timing = DiagTiming::default();
    let mut idle_calls = 0;
    let mut before_event = 0;
    drain_queue(&queue, &done, &timing, |value| {
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
