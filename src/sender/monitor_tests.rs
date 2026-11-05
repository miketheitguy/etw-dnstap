//! Tests for the monitor. They give the monitor the time and the spool state, so
//! none of them waits for a sample period.

use std::sync::atomic::AtomicU64;

use tracing::Level;

use super::*;
use crate::spool::test_support::capture_logs_at;

fn get(counter: &AtomicU64) -> u64 {
    counter.load(Ordering::Relaxed)
}

fn flag(stats: &SinkStats) -> bool {
    stats.link_saturated.load(Ordering::Relaxed)
}

/// The observation at `second` seconds after `base`.
fn observation(base: Instant, second: u64, backlog_bytes: u64) -> Observation {
    Observation {
        at: base + Duration::from_secs(second),
        link_up: true,
        reads_paused: false,
        backlog_bytes,
        disconnects: 0,
        oldest_age: Duration::ZERO,
    }
}

fn monitor() -> (LinkMonitor, Arc<SinkStats>, Instant) {
    let stats = Arc::new(SinkStats::default());
    let base = Instant::now();
    (LinkMonitor::new(Arc::clone(&stats), base), stats, base)
}

// ---- the peak window ----

#[test]
fn a_peak_stays_for_the_window_and_then_leaves_it() {
    let mut window = PeakWindow::new();
    window.record(7);
    window.record(3);
    assert_eq!(window.peak(), 7, "the largest value counts");
    for _ in 0..PEAK_WINDOW - 1 {
        window.roll();
        assert_eq!(window.peak(), 7, "still inside the window");
    }
    window.roll();
    assert_eq!(window.peak(), 0, "the 60th roll drops it");
}

#[test]
fn a_later_smaller_peak_replaces_an_expired_larger_one() {
    let mut window = PeakWindow::new();
    window.record(9);
    window.roll();
    window.record(4);
    for _ in 0..PEAK_WINDOW - 1 {
        window.roll();
    }
    assert_eq!(window.peak(), 4, "the 9 has left, the 4 has not");
}

// ---- the sample clock ----

#[test]
fn a_sample_is_due_one_period_after_the_last() {
    let (mut monitor, _stats, base) = monitor();
    assert!(!monitor.due(base));
    assert!(!monitor.due(base + Duration::from_millis(999)));
    assert!(monitor.due(base + SAMPLE_PERIOD));
    // A sample that runs late moves the next one a period after itself, so
    // a long block on the sender thread does not cause a burst.
    monitor.sample(&observation(base, 5, 0));
    assert!(!monitor.due(base + Duration::from_secs(5)));
    assert!(monitor.due(base + Duration::from_secs(6)));
}

// ---- D1: queue depth ----

#[test]
fn the_send_queue_peak_is_the_largest_depth_noted_in_the_window() {
    let (mut monitor, stats, base) = monitor();
    monitor.note_queue_depth(3);
    monitor.note_queue_depth(41);
    monitor.note_queue_depth(12);
    assert_eq!(
        get(&stats.send_queue_depth_max),
        0,
        "nothing before a sample"
    );
    monitor.sample(&observation(base, 1, 0));
    assert_eq!(get(&stats.send_queue_depth_max), 41);
    // Reading the value does not reset it: it holds until the window passes.
    assert_eq!(get(&stats.send_queue_depth_max), 41);
    monitor.note_queue_depth(5);
    monitor.sample(&observation(base, 2, 0));
    assert_eq!(get(&stats.send_queue_depth_max), 41);
    // The window is 60 samples: the 41 of sample 1 is in it through sample 60.
    for second in 3..=60 {
        monitor.sample(&observation(base, second, 0));
    }
    assert_eq!(get(&stats.send_queue_depth_max), 41);
    monitor.sample(&observation(base, 61, 0));
    assert_eq!(get(&stats.send_queue_depth_max), 5, "the 41 has left");
    monitor.sample(&observation(base, 62, 0));
    assert_eq!(get(&stats.send_queue_depth_max), 0, "the 5 has left");
}

// ---- D2: write peak per window ----

#[test]
fn the_recent_write_peak_is_rolled_by_the_sender_not_by_a_reader() {
    let (mut monitor, stats, base) = monitor();
    stats
        .write_window_max_us
        .fetch_max(4_000, Ordering::Relaxed);
    stats.write_window_max_us.fetch_max(900, Ordering::Relaxed);
    monitor.sample(&observation(base, 1, 0));
    assert_eq!(get(&stats.write_max_recent_us), 4_000);
    assert_eq!(
        get(&stats.write_window_max_us),
        0,
        "the sender took the accumulator"
    );
    // Two readers load the gauge any number of times; it does not change.
    for _ in 0..3 {
        assert_eq!(get(&stats.write_max_recent_us), 4_000);
    }
    // A quieter second does not lower the peak inside the window.
    stats.write_window_max_us.fetch_max(100, Ordering::Relaxed);
    monitor.sample(&observation(base, 2, 0));
    assert_eq!(get(&stats.write_max_recent_us), 4_000);
    for second in 3..=60 {
        monitor.sample(&observation(base, second, 0));
    }
    assert_eq!(get(&stats.write_max_recent_us), 4_000);
    monitor.sample(&observation(base, 61, 0));
    assert_eq!(get(&stats.write_max_recent_us), 100, "the 4 ms has left");
}

#[test]
fn the_since_start_write_maximum_is_not_touched() {
    let (mut monitor, stats, base) = monitor();
    stats.write_max_us.fetch_max(9_000, Ordering::Relaxed);
    stats.slow_writes.fetch_add(2, Ordering::Relaxed);
    stats
        .write_window_max_us
        .fetch_max(9_000, Ordering::Relaxed);
    for second in 1..=70 {
        monitor.sample(&observation(base, second, 0));
    }
    assert_eq!(get(&stats.write_max_recent_us), 0);
    assert_eq!(get(&stats.write_max_us), 9_000);
    assert_eq!(get(&stats.slow_writes), 2);
}

#[test]
fn the_router_peaks_are_rolled_by_the_sender_like_the_write_peak() {
    let (mut monitor, stats, base) = monitor();
    stats
        .router_wait_window_max_us
        .fetch_max(52_000, Ordering::Relaxed);
    stats
        .router_pass_window_max_us
        .fetch_max(700, Ordering::Relaxed);
    stats
        .router_pass_window_max_us
        .fetch_max(300, Ordering::Relaxed);
    monitor.sample(&observation(base, 1, 0));
    assert_eq!(get(&stats.router_wait_max_recent_us), 52_000);
    assert_eq!(get(&stats.router_pass_max_recent_us), 700);
    assert_eq!(get(&stats.router_wait_window_max_us), 0);
    assert_eq!(get(&stats.router_pass_window_max_us), 0);
    // The two peaks are independent of each other and of the write peak.
    assert_eq!(get(&stats.write_max_recent_us), 0);
    stats
        .router_pass_window_max_us
        .fetch_max(50, Ordering::Relaxed);
    for second in 2..=60 {
        monitor.sample(&observation(base, second, 0));
    }
    assert_eq!(get(&stats.router_wait_max_recent_us), 52_000);
    assert_eq!(get(&stats.router_pass_max_recent_us), 700);
    monitor.sample(&observation(base, 61, 0));
    assert_eq!(get(&stats.router_wait_max_recent_us), 0, "the 52 ms left");
    assert_eq!(get(&stats.router_pass_max_recent_us), 50);
}

// ---- D1: oldest age ----

#[test]
fn the_oldest_age_is_published_in_milliseconds_and_is_zero_without_a_backlog() {
    let (mut monitor, stats, base) = monitor();
    let mut sample = observation(base, 1, 4_096);
    sample.oldest_age = Duration::from_millis(93_250);
    monitor.sample(&sample);
    assert_eq!(get(&stats.spool_oldest_age_ms), 93_250);
    monitor.sample(&observation(base, 2, 0));
    assert_eq!(get(&stats.spool_oldest_age_ms), 0);
}

// ---- D4: the saturated state ----

/// Backlogs that start at `start` and add the `steps` in turn, over and over,
/// for `samples` samples (the first is `start`).
fn series(start: u64, steps: &[i64], samples: usize) -> Vec<u64> {
    let mut backlog = start;
    let mut values = vec![start];
    for step in steps.iter().cycle().take(samples - 1) {
        backlog = backlog.checked_add_signed(*step).expect("stays positive");
        values.push(backlog);
    }
    values
}

/// Feeds one backlog a second from `second`. Returns the next second.
fn feed(monitor: &mut LinkMonitor, base: Instant, second: u64, backlogs: &[u64]) -> u64 {
    for (offset, backlog) in (0..).zip(backlogs) {
        monitor.sample(&observation(base, second + offset, *backlog));
    }
    second + backlogs.len() as u64
}

/// Sets the state with ten steady rises, 11 samples from second 1. Returns the
/// next second and the last backlog.
fn saturate(monitor: &mut LinkMonitor, stats: &SinkStats, base: Instant) -> (u64, u64) {
    let values = series(1_000_000, &[500_000], 11);
    let second = feed(monitor, base, 1, &values);
    assert!(flag(stats));
    (second, values[10])
}

#[test]
fn ten_steady_rises_set_the_state_at_the_eleventh_sample_and_not_before() {
    let (mut monitor, stats, base) = monitor();
    let values = series(1_000_000, &[500_000], 11);
    let second = feed(&mut monitor, base, 1, &values[..10]);
    assert!(!flag(&stats), "ten samples are nine steps");
    feed(&mut monitor, base, second, &values[10..]);
    assert!(flag(&stats), "the 11th sample completes ten steps");
}

#[test]
fn a_link_just_below_the_capture_rate_sets_the_state_despite_flat_samples() {
    // Capture 5.5 MB/s, link 5.3 MB/s: the backlog gains 0.2 MB a second, but
    // the replay reads in batches, so every fifth sample shows no rise.
    let (mut monitor, stats, base) = monitor();
    let steps = [250_000, 250_000, 250_000, 250_000, 0];
    let values = series(10_000_000, &steps, 120);
    let logs = capture_logs_at(Level::INFO, || {
        feed(&mut monitor, base, 1, &values[..11]);
        assert!(flag(&stats), "8 of the first 10 steps rose");
        feed(&mut monitor, base, 12, &values[11..]);
        assert!(flag(&stats), "and it holds while the pattern lasts");
    });
    assert_eq!(logs.count("dnstap link is saturated"), 1);
    assert_eq!(logs.count("no longer saturated"), 0);
    // The warning carries the link-saturated ID, event 23.
    assert_eq!(logs.count("etw_dnstap::event::23"), 1, "{}", logs.text());
}

#[test]
fn seven_rises_of_ten_set_the_state_and_six_do_not() {
    for (rises, expected) in [(7, true), (6, false)] {
        let (mut monitor, stats, base) = monitor();
        // The flat steps come first, so the window shows `rises` rises in all.
        let mut steps = vec![0; 10 - rises];
        steps.extend(vec![100_000; rises]);
        feed(&mut monitor, base, 1, &series(1_000_000, &steps, 11));
        assert_eq!(flag(&stats), expected, "{rises} rises");
    }
}

#[test]
fn one_burst_followed_by_a_flat_backlog_does_not_set_the_state() {
    let (mut monitor, stats, base) = monitor();
    let mut steps = vec![5_000_000];
    steps.extend([0; 9]);
    feed(&mut monitor, base, 1, &series(1_000_000, &steps, 11));
    assert!(!flag(&stats), "one rise is not a trend");
}

#[test]
fn a_backlog_that_rises_often_but_is_not_larger_than_ten_samples_ago_does_not_set() {
    let (mut monitor, stats, base) = monitor();
    // Three rises of 250 KB and one fall of 1.2 MB: 7 of 10 steps rise in every
    // window, and the backlog falls overall.
    let steps = [250_000, 250_000, 250_000, -1_200_000];
    let values = series(50_000_000, &steps, 200);
    feed(&mut monitor, base, 1, &values);
    assert!(!flag(&stats));
}

#[test]
fn a_drain_after_an_outage_never_sets_the_state() {
    let (mut monitor, stats, base) = monitor();
    let logs = capture_logs_at(Level::INFO, || {
        // The outage: the link is down and the backlog grows.
        let mut second = 1;
        let mut backlog = 1_000_000;
        for _ in 0..60 {
            backlog += 2_000_000;
            let mut down = observation(base, second, backlog);
            down.link_up = false;
            down.disconnects = 1;
            monitor.sample(&down);
            second += 1;
        }
        // The reconnect: the replay outruns the capture, with jitter. The
        // backlog falls by 1 MB a second on average and rises in some steps.
        let steps = [-3_000_000, -2_000_000, 250_000, -1_000_000, 250_000];
        for value in series(backlog, &steps, 100) {
            let mut up = observation(base, second, value);
            up.disconnects = 1;
            monitor.sample(&up);
            second += 1;
        }
    });
    assert!(!flag(&stats));
    assert_eq!(logs.count("saturated"), 0);
}

#[test]
fn growth_while_the_link_is_down_does_not_count_and_the_window_restarts() {
    let (mut monitor, stats, base) = monitor();
    let mut backlog = 1_000;
    for second in 1..=60 {
        backlog += 1_000;
        let mut down = observation(base, second, backlog);
        down.link_up = false;
        monitor.sample(&down);
    }
    assert!(!flag(&stats), "an outage is not a saturated link");
    // After the reconnect the window starts empty: ten samples are not enough.
    let values = series(backlog, &[1_000], 11);
    let second = feed(&mut monitor, base, 61, &values[..10]);
    assert!(!flag(&stats));
    feed(&mut monitor, base, second, &values[10..]);
    assert!(flag(&stats));
}

#[test]
fn a_disconnect_between_two_samples_restarts_the_window() {
    // The first reconnect backoff is 250 ms, so a session can drop and return
    // between two samples. The count of lost sessions shows it.
    let (mut monitor, stats, base) = monitor();
    let values = series(1_000_000, &[500_000], 30);
    for (offset, backlog) in (0..).zip(&values) {
        let mut sample = observation(base, 1 + offset, *backlog);
        // The session is lost before the sixth sample and not seen down.
        sample.disconnects = u64::from(offset >= 5);
        monitor.sample(&sample);
        // Samples 1 to 5 are the old session; the window restarts at the 6th,
        // which does not count. The 11 samples from the 7th (index 6) to the
        // 17th (index 16) set the state, and not the first 11.
        assert_eq!(flag(&stats), offset >= 16, "sample {}", offset + 1);
    }
}

#[test]
fn paused_spool_reads_do_not_count_as_a_saturated_link() {
    let (mut monitor, stats, base) = monitor();
    for second in 1..=60 {
        let mut sample = observation(base, second, 1_000 * second);
        sample.reads_paused = true;
        monitor.sample(&sample);
    }
    assert!(!flag(&stats));
}

#[test]
fn an_empty_backlog_clears_the_state_at_once() {
    let (mut monitor, stats, base) = monitor();
    let (second, last) = saturate(&mut monitor, &stats, base);
    monitor.sample(&observation(base, second, last + 1));
    assert!(flag(&stats), "a rising sample does not clear it");
    monitor.sample(&observation(base, second + 1, 0));
    assert!(!flag(&stats));
}

#[test]
fn a_backlog_that_stops_growing_clears_the_state_when_the_window_has_passed() {
    let (mut monitor, stats, base) = monitor();
    let (second, last) = saturate(&mut monitor, &stats, base);
    // The window still holds the rising samples, so the flat backlog is larger
    // than ten samples ago until the window holds only flat samples.
    feed(&mut monitor, base, second, &[last; 9]);
    assert!(flag(&stats), "nine flat samples are not enough");
    monitor.sample(&observation(base, second + 9, last));
    assert!(!flag(&stats), "the tenth flat sample clears it");
}

#[test]
fn a_falling_backlog_clears_the_state() {
    let (mut monitor, stats, base) = monitor();
    let (second, last) = saturate(&mut monitor, &stats, base);
    // Falls with a rise every third sample.
    let values = series(last, &[-200_000, -200_000, 50_000], 20);
    let logs = capture_logs_at(Level::INFO, || {
        feed(&mut monitor, base, second, &values[1..]);
    });
    assert!(!flag(&stats));
    assert_eq!(logs.count("no longer saturated"), 1);
    // The clearing event is information (ID 2), so it has no warning target.
    assert_eq!(logs.count("etw_dnstap::event::23"), 0, "{}", logs.text());
}

#[test]
fn a_change_is_not_undone_inside_one_trend_window() {
    let (mut monitor, stats, base) = monitor();
    let logs = capture_logs_at(Level::INFO, || {
        let (second, _) = saturate(&mut monitor, &stats, base);
        // The backlog drops to a small, non-empty value and stays there. It is
        // not larger than ten samples ago at once, but the state holds until
        // ten samples have passed since it was set.
        feed(&mut monitor, base, second, &[1; 9]);
        assert!(flag(&stats));
        monitor.sample(&observation(base, second + 9, 1));
        assert!(!flag(&stats));
    });
    assert_eq!(logs.count("dnstap link is saturated"), 1);
    assert_eq!(logs.count("no longer saturated"), 1);
}

#[test]
fn the_state_holds_while_the_link_is_down_and_does_not_log_again() {
    let (mut monitor, stats, base) = monitor();
    let logs = capture_logs_at(Level::INFO, || {
        let (second, last) = saturate(&mut monitor, &stats, base);
        for offset in 0..30 {
            let mut down = observation(base, second + offset, last);
            down.link_up = false;
            monitor.sample(&down);
        }
        assert!(flag(&stats), "a flapping link does not clear the state");
    });
    assert_eq!(logs.count("dnstap link is saturated"), 1);
    assert_eq!(logs.count("no longer saturated"), 0);
}

// ---- D4: the events ----

#[test]
fn each_change_logs_one_event_with_the_backlog_the_age_and_the_send_rate() {
    let (mut monitor, stats, base) = monitor();
    let logs = capture_logs_at(Level::INFO, || {
        // The link accepts 2,000 bytes a second while the backlog grows.
        for second in 1..=11_u64 {
            stats.bytes_sent.store(2_000 * second, Ordering::Relaxed);
            let mut sample = observation(base, second, 1_000_000 + 500 * second);
            sample.oldest_age = Duration::from_secs(second + 100);
            monitor.sample(&sample);
        }
        assert!(flag(&stats));
        let mut sample = observation(base, 12, 0);
        sample.oldest_age = Duration::ZERO;
        monitor.sample(&sample);
        assert!(!flag(&stats));
    });
    assert_eq!(logs.count("dnstap link is saturated"), 1);
    assert_eq!(logs.count("dnstap link is no longer saturated"), 1);
    let text = logs.text();
    // The set event is the 11th sample.
    assert!(text.contains("backlog_bytes=1005500"), "{text}");
    assert!(text.contains("oldest_age_ms=111000"), "{text}");
    assert!(text.contains("bytes_sent_per_second=2000"), "{text}");
    assert!(text.contains("WARN"), "the set event is a warning: {text}");
    // The clear event.
    assert!(text.contains("backlog_bytes=0"), "{text}");
    assert!(text.contains("the backlog is empty"), "{text}");
}

#[test]
fn no_event_is_logged_once_a_stop_has_begun_but_the_state_still_changes() {
    let (mut monitor, stats, base) = monitor();
    stats.begin_stop();
    let logs = capture_logs_at(Level::INFO, || {
        saturate(&mut monitor, &stats, base);
    });
    assert!(flag(&stats), "the flag is set without a log line");
    assert_eq!(logs.text(), "");
}

#[test]
fn the_stopped_growing_event_names_its_reason() {
    let (mut monitor, stats, base) = monitor();
    let logs = capture_logs_at(Level::INFO, || {
        let (second, last) = saturate(&mut monitor, &stats, base);
        feed(&mut monitor, base, second, &[last; 10]);
        assert!(!flag(&stats));
    });
    assert!(logs.text().contains("the backlog stopped growing"));
    assert_eq!(logs.count("dnstap link is no longer saturated"), 1);
}

#[test]
fn no_event_is_logged_without_a_change() {
    let (mut monitor, _stats, base) = monitor();
    let logs = capture_logs_at(Level::INFO, || {
        for second in 1..=120 {
            monitor.sample(&observation(base, second, 0));
        }
    });
    assert_eq!(logs.count("saturated"), 0);
}

#[test]
fn the_send_rate_is_zero_before_two_samples_and_with_no_progress() {
    let (mut monitor, stats, base) = monitor();
    assert_eq!(monitor.send_rate(), 0);
    monitor.sample(&observation(base, 1, 0));
    assert_eq!(monitor.send_rate(), 0, "one sample has no span");
    stats.bytes_sent.store(5_000, Ordering::Relaxed);
    monitor.sample(&observation(base, 3, 0));
    assert_eq!(monitor.send_rate(), 2_500, "5,000 bytes over 2 s");
}
