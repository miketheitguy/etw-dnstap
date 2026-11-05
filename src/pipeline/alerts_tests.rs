//! The loss and risk checks: deltas, thresholds, edges, and the text written.

use std::{
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

use super::*;
use crate::spool::test_support::capture_logs_at;

const CAPACITY: usize = 1000;
const MAX_BYTES: u64 = 800 * MIB;
const MIN_FREE_BYTES: u64 = 100 * MIB;
const SEGMENT_BYTES: u64 = 64 * MIB;

fn limits() -> Limits {
    Limits {
        capacity: CAPACITY,
        jumbo_capacity: 64,
        spool: Some(SpoolLimits {
            max: MAX_BYTES,
            min_free: MIN_FREE_BYTES,
            segment: SEGMENT_BYTES,
        }),
    }
}

/// A healthy reading: receiver up, spool in use and empty, plenty of room.
fn healthy() -> Readings {
    Readings {
        connected: true,
        spool_available: true,
        etw_stats_valid: true,
        etw_buffers_maximum: 100,
        etw_buffers_allocated: 10,
        volume_free: Some(100 * 1024 * MIB),
        ..Readings::default()
    }
}

fn checker() -> (Alerts, Instant) {
    let start = Instant::now();
    (Alerts::new(limits(), start, healthy()), start)
}

fn seconds(count: u64) -> Duration {
    Duration::from_secs(count)
}

/// Checks `readings` one interval after `start`.
fn check(alerts: &mut Alerts, start: Instant, readings: &Readings) -> Vec<Notice> {
    alerts.check(start + seconds(60), readings)
}

fn losses(notices: &[Notice]) -> Vec<Loss> {
    notices
        .iter()
        .filter_map(|notice| match notice {
            Notice::Loss(loss) => Some(*loss),
            _ => None,
        })
        .collect()
}

fn raised(notices: &[Notice]) -> Vec<Risk> {
    notices
        .iter()
        .filter_map(|notice| match notice {
            Notice::Raised(risk) => Some(*risk),
            _ => None,
        })
        .collect()
}

fn cleared(notices: &[Notice]) -> Vec<Risk> {
    notices
        .iter()
        .filter_map(|notice| match notice {
            Notice::Cleared(risk) => Some(*risk),
            _ => None,
        })
        .collect()
}

#[test]
fn a_quiet_interval_writes_nothing() {
    let (mut alerts, start) = checker();
    assert_eq!(check(&mut alerts, start, &healthy()), []);
    assert_eq!(alerts.check(start + seconds(120), &healthy()), []);
}

#[test]
fn each_cause_has_its_own_loss_notice() {
    type Raise = fn(&mut Readings);
    let causes: [(&str, Raise); 7] = [
        ("etw events", |r| r.etw_lost = 5),
        ("etw buffers", |r| r.etw_buffers_lost = 5),
        ("channel", |r| r.ring_dropped = 5),
        ("regular pool", |r| r.pool_exhausted_regular = 5),
        ("jumbo pool", |r| r.pool_exhausted_jumbo = 5),
        ("disconnected", |r| r.disconnected_dropped = 5),
        ("send queue", |r| r.queue_dropped = 5),
    ];
    for (name, raise) in causes {
        let (mut alerts, start) = checker();
        let mut current = healthy();
        raise(&mut current);
        let found = losses(&check(&mut alerts, start, &current));
        assert_eq!(found.len(), 1, "{name}: {found:?}");
    }
    let spool: [(&str, Raise); 2] = [
        ("spool frames", |r| r.spool_dropped = 5),
        ("spool bytes", |r| r.spool_dropped_bytes = 5),
    ];
    let decode: [(&str, Raise); 2] = [
        ("rejected", |r| r.rejected = 5),
        ("schema rejected", |r| r.schema_rejected = 5),
    ];
    let internal: [(&str, Raise); 3] = [
        ("callback", |r| r.callback_errors = 5),
        ("closed channel", |r| r.receiver_disconnected = 5),
        ("sender stopped", |r| r.output_failed_events = 5),
    ];
    for (name, raise) in spool.into_iter().chain(decode).chain(internal) {
        let (mut alerts, start) = checker();
        let mut current = healthy();
        raise(&mut current);
        let found = losses(&check(&mut alerts, start, &current));
        assert_eq!(found.len(), 1, "{name}: {found:?}");
    }
}

#[test]
fn the_loss_notices_match_their_causes() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.etw_lost = 7;
    current.ring_dropped = 8;
    current.pool_exhausted_jumbo = 9;
    current.queue_dropped = 10;
    current.spool_dropped = 11;
    current.rejected = 12;
    current.callback_errors = 13;
    let found = losses(&check(&mut alerts, start, &current));
    assert!(matches!(
        found[0],
        Loss::Etw { events, buffers, .. }
            if events == Rise { by: 7, total: 7 } && !buffers.any()
    ));
    assert!(matches!(
        found[1],
        Loss::Channel { dropped, capacity: CAPACITY, .. } if dropped.by == 8
    ));
    assert!(matches!(
        found[2],
        Loss::Pool { regular, jumbo, jumbo_capacity: 64, .. }
            if !regular.any() && jumbo.by == 9
    ));
    assert!(matches!(
        found[3],
        Loss::Network { disconnected, queue, .. } if !disconnected.any() && queue.by == 10
    ));
    assert!(matches!(found[4], Loss::Spool { frames, .. } if frames.by == 11));
    assert!(matches!(found[5], Loss::Decode { rejected, .. } if rejected.by == 12));
    assert!(matches!(
        found[6],
        Loss::Internal { callback_errors, .. } if callback_errors.by == 13
    ));
    assert_eq!(found.len(), 7);
}

#[test]
fn a_loss_is_reported_by_its_increase_and_not_again_when_flat() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.ring_dropped = 30;
    let first = losses(&check(&mut alerts, start, &current));
    assert!(matches!(
        first[..],
        [Loss::Channel { dropped, .. }] if dropped == Rise { by: 30, total: 30 }
    ));
    // Flat: the total is unchanged, so nothing is written.
    assert_eq!(alerts.check(start + seconds(120), &current), []);
    current.ring_dropped = 35;
    let second = losses(&alerts.check(start + seconds(180), &current));
    assert!(matches!(
        second[..],
        [Loss::Channel { dropped, .. }] if dropped == Rise { by: 5, total: 35 }
    ));
}

#[test]
fn two_counters_of_one_cause_make_one_notice() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.disconnected_dropped = 4;
    current.queue_dropped = 6;
    current.etw_lost = 1;
    current.etw_buffers_lost = 2;
    let notices = check(&mut alerts, start, &current);
    let found = losses(&notices);
    assert_eq!(found.len(), 2, "{found:?}");
    assert!(matches!(
        found[0],
        Loss::Etw { events, buffers, .. } if events.by == 1 && buffers.by == 2
    ));
    assert!(matches!(
        found[1],
        Loss::Network { disconnected, queue, .. } if disconnected.by == 4 && queue.by == 6
    ));
}

#[test]
fn the_baseline_is_the_first_reading() {
    // Counters that were already high at the start are not new loss.
    let start = Instant::now();
    let mut high = healthy();
    high.ring_dropped = 1_000;
    high.etw_lost = 50;
    let mut alerts = Alerts::new(limits(), start, high);
    assert_eq!(check(&mut alerts, start, &high), []);
    high.ring_dropped = 1_001;
    let found = losses(&alerts.check(start + seconds(120), &high));
    assert!(matches!(
        found[..],
        [Loss::Channel { dropped, .. }] if dropped == Rise { by: 1, total: 1_001 }
    ));
}

#[test]
fn a_counter_that_falls_is_not_a_loss() {
    // `etw_lost` is ETW's own count and can reset when the session is replaced.
    let start = Instant::now();
    let mut before = healthy();
    before.etw_lost = 500;
    let mut alerts = Alerts::new(limits(), start, before);
    let mut after = healthy();
    after.etw_lost = 3;
    assert_eq!(check(&mut alerts, start, &after), []);
}

#[test]
fn the_notice_names_the_seconds_it_covers() {
    let start = Instant::now();
    let mut alerts = Alerts::new(limits(), start, healthy());
    let mut current = healthy();
    current.rejected = 1;
    let found = losses(&alerts.check(start + Duration::from_millis(60_400), &current));
    assert!(matches!(found[..], [Loss::Decode { seconds: 60, .. }]));
    current.rejected = 2;
    let later = losses(&alerts.check(start + Duration::from_millis(90_900), &current));
    // 30.5 s after the previous check, rounded to the nearest second.
    assert!(matches!(later[..], [Loss::Decode { seconds: 31, .. }]));
}

#[test]
fn whole_seconds_round_to_the_nearest() {
    assert_eq!(whole_seconds(Duration::ZERO), 0);
    assert_eq!(whole_seconds(Duration::from_millis(499)), 0);
    assert_eq!(whole_seconds(Duration::from_millis(500)), 1);
    assert_eq!(whole_seconds(Duration::from_millis(59_499)), 59);
    assert_eq!(whole_seconds(Duration::from_millis(59_500)), 60);
}

#[test]
fn a_loss_does_not_raise_a_risk() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.ring_dropped = 9;
    let notices = check(&mut alerts, start, &current);
    assert_eq!(raised(&notices), []);
    assert_eq!(cleared(&notices), []);
}

#[test]
fn rise_by_if_any_hides_a_counter_that_did_not_rise() {
    let flat = Rise::between(4, 4);
    assert_eq!((flat.by_if_any(), flat.total_if_any()), (None, None));
    let rose = Rise::between(4, 9);
    assert_eq!((rose.by_if_any(), rose.total_if_any()), (Some(5), Some(9)));
}

// Thresholds.

#[test]
fn near_is_true_at_the_threshold_and_not_below() {
    assert!(near(false, 800, 1000, 80));
    assert!(!near(false, 799, 1000, 80));
    assert!(near(false, 1000, 1000, 80));
    assert!(near(false, 1500, 1000, 80), "above the whole");
}

#[test]
fn near_clears_ten_points_below_the_raising_level() {
    assert!(near(true, 700, 1000, 80));
    assert!(!near(true, 699, 1000, 80));
    assert!(!near(false, 700, 1000, 80), "not yet raised");
}

#[test]
fn near_is_false_when_the_whole_is_unknown() {
    assert!(!near(false, 5, 0, 80));
    assert!(!near(true, 5, 0, 80));
}

#[test]
fn near_does_not_overflow() {
    assert!(near(false, u64::MAX, u64::MAX, 90));
    assert!(!near(false, u64::MAX / 2, u64::MAX, 90));
}

#[test]
fn the_send_queue_threshold_is_205_of_256_blocks() {
    assert_eq!(SEND_QUEUE_BLOCKS, 256, "the doc states 256 blocks");
    assert!(!near(false, 204, 256, SEND_QUEUE_PERCENT));
    assert!(near(false, 205, 256, SEND_QUEUE_PERCENT));
}

#[test]
fn the_documented_thresholds_are_these() {
    assert_eq!(CHANNEL_PERCENT, 80);
    assert_eq!(ETW_BUFFERS_PERCENT, 90);
    assert_eq!(SEND_QUEUE_PERCENT, 80);
    assert_eq!(SPOOL_PERCENT, 80);
    assert_eq!(CLEAR_MARGIN_PERCENT, 10);
}

// Risks.

#[test]
fn the_channel_risk_rises_at_80_percent_of_the_capacity() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.channel_peak = 799;
    assert_eq!(check(&mut alerts, start, &current), []);
    current.channel_peak = 800;
    let notices = alerts.check(start + seconds(120), &current);
    assert_eq!(
        notices,
        [Notice::Raised(Risk::Channel {
            peak: 800,
            capacity: CAPACITY
        })]
    );
}

#[test]
fn a_raised_risk_is_not_repeated_and_clears_once() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.channel_peak = 900;
    let mut at = start;
    let mut next = |state: &mut Alerts, readings: &Readings| {
        at += seconds(60);
        state.check(at, readings)
    };
    assert_eq!(raised(&next(&mut alerts, &current)).len(), 1);
    assert_eq!(next(&mut alerts, &current), [], "still near: no repeat");
    current.channel_peak = 750;
    assert_eq!(next(&mut alerts, &current), [], "inside the margin: holds");
    current.channel_peak = 699;
    let notices = next(&mut alerts, &current);
    assert_eq!(cleared(&notices).len(), 1, "{notices:?}");
    assert_eq!(next(&mut alerts, &current), [], "cleared once");
    current.channel_peak = 850;
    assert_eq!(
        raised(&next(&mut alerts, &current)).len(),
        1,
        "raises again"
    );
}

#[test]
fn the_etw_buffer_risk_rises_at_90_percent_of_the_applied_maximum() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.etw_buffers_allocated = 89;
    assert_eq!(check(&mut alerts, start, &current), []);
    current.etw_buffers_allocated = 90;
    let notices = alerts.check(start + seconds(120), &current);
    assert_eq!(
        notices,
        [Notice::Raised(Risk::EtwBuffers {
            allocated: 90,
            maximum: 100
        })]
    );
}

#[test]
fn the_etw_buffer_risk_needs_a_known_maximum() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.etw_buffers_maximum = 0;
    current.etw_buffers_allocated = 50;
    assert_eq!(check(&mut alerts, start, &current), []);
}

#[test]
fn the_send_queue_risk_rises_at_80_percent_of_the_queue() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.send_queue_peak = 204;
    assert_eq!(check(&mut alerts, start, &current), []);
    current.send_queue_peak = 205;
    let notices = alerts.check(start + seconds(120), &current);
    assert_eq!(
        notices,
        [Notice::Raised(Risk::SendQueue {
            peak: 205,
            blocks: SEND_QUEUE_BLOCKS
        })]
    );
}

#[test]
fn the_receiver_down_risk_needs_the_receiver_down_and_no_spool_taking_frames() {
    let (mut alerts, start) = checker();
    let mut at = start;
    let mut step = |state: &mut Alerts, connected: bool, spool: bool| {
        let mut current = healthy();
        current.connected = connected;
        current.spool_available = spool;
        at += seconds(60);
        state.check(at, &current)
    };
    // Down with a spool: the frames are kept, no loss risk of this kind.
    assert_eq!(step(&mut alerts, false, true), []);
    // Connected without a spool (--no-spool, healthy link): fine.
    assert_eq!(step(&mut alerts, true, false), []);
    // Down and no spool: frames are lost.
    assert_eq!(
        step(&mut alerts, false, false),
        [Notice::Raised(Risk::ReceiverDownUnspooled)]
    );
    assert_eq!(step(&mut alerts, false, false), [], "once per outage");
    // The receiver returns.
    assert_eq!(
        step(&mut alerts, true, false),
        [Notice::Cleared(Risk::ReceiverDownUnspooled)]
    );
}

#[test]
fn the_spool_risk_rises_at_80_percent_of_the_cap() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.spool_segments = 3;
    current.spool_bytes = MAX_BYTES / 100 * 80 - 1;
    assert_eq!(check(&mut alerts, start, &current), []);
    current.spool_bytes = MAX_BYTES / 100 * 80;
    let notices = alerts.check(start + seconds(120), &current);
    assert!(matches!(
        raised(&notices)[..],
        [Risk::SpoolLimit {
            spool_max_bytes: MAX_BYTES,
            ..
        }]
    ));
    // Clears 10 points lower.
    current.spool_bytes = MAX_BYTES / 100 * 70 - 1;
    let cleared_at = alerts.check(start + seconds(180), &current);
    assert_eq!(cleared(&cleared_at).len(), 1, "{cleared_at:?}");
}

#[test]
fn the_spool_risk_rises_when_the_volume_nears_the_free_space_floor() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    let line = MIN_FREE_BYTES + 2 * SEGMENT_BYTES;
    current.volume_free = Some(line);
    assert_eq!(
        check(&mut alerts, start, &current),
        [],
        "at the line: not yet"
    );
    current.volume_free = Some(line - 1);
    let notices = alerts.check(start + seconds(120), &current);
    assert!(matches!(
        raised(&notices)[..],
        [Risk::SpoolLimit { volume_free: Some(free), floor_bytes, .. }]
            if free == line - 1 && floor_bytes == MIN_FREE_BYTES + SEGMENT_BYTES
    ));
    // Back above the line but inside the clearing margin of one more segment.
    current.volume_free = Some(line + 1);
    assert_eq!(alerts.check(start + seconds(180), &current), []);
    current.volume_free = Some(MIN_FREE_BYTES + 3 * SEGMENT_BYTES);
    let cleared_at = alerts.check(start + seconds(240), &current);
    assert_eq!(cleared(&cleared_at).len(), 1, "{cleared_at:?}");
}

#[test]
fn an_unreadable_volume_does_not_raise_the_spool_risk() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.volume_free = None;
    assert_eq!(check(&mut alerts, start, &current), []);
}

#[test]
fn the_spool_risk_needs_a_spool_that_is_in_use() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.spool_available = false;
    current.connected = true;
    current.spool_segments = 0;
    current.spool_bytes = MAX_BYTES;
    current.volume_free = Some(0);
    assert_eq!(check(&mut alerts, start, &current), []);
}

#[test]
fn with_no_spool_the_spool_risk_never_rises() {
    let start = Instant::now();
    let mut no_spool = limits();
    no_spool.spool = None;
    let mut alerts = Alerts::new(no_spool, start, healthy());
    let mut current = healthy();
    current.spool_bytes = MAX_BYTES;
    current.spool_segments = 4;
    current.volume_free = Some(0);
    assert_eq!(check(&mut alerts, start, &current), []);
}

#[test]
fn the_etw_statistics_risk_rises_on_a_failed_query_and_clears_on_a_good_one() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.etw_stats_valid = false;
    current.etw_stats_errors = 2;
    let notices = check(&mut alerts, start, &current);
    assert_eq!(
        notices,
        [Notice::Raised(Risk::EtwStats {
            errors: Rise { by: 2, total: 2 }
        })]
    );
    // Still failing, no new errors counted in this interval: holds, no repeat.
    assert_eq!(alerts.check(start + seconds(120), &current), []);
    current.etw_stats_valid = true;
    let cleared_at = alerts.check(start + seconds(180), &current);
    assert_eq!(cleared(&cleared_at).len(), 1, "{cleared_at:?}");
}

#[test]
fn an_invalid_flag_without_new_errors_is_not_a_risk() {
    // Before the first query the flag is false and no error has been counted.
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.etw_stats_valid = false;
    assert_eq!(check(&mut alerts, start, &current), []);
}

#[test]
fn several_risks_can_rise_in_one_check_but_each_only_once() {
    let (mut alerts, start) = checker();
    let mut current = healthy();
    current.channel_peak = 1000;
    current.send_queue_peak = 256;
    current.etw_buffers_allocated = 100;
    let notices = check(&mut alerts, start, &current);
    assert_eq!(raised(&notices).len(), 3, "{notices:?}");
    assert_eq!(alerts.check(start + seconds(120), &current), []);
}

// Reading the counters.

#[test]
fn readings_come_from_the_counters_and_take_the_channel_peak() {
    let capture = CaptureCounters::default();
    let parse = ParseCounters::default();
    let sink = SinkStats::default();
    capture.etw_lost.store(1, Ordering::Relaxed);
    capture.etw_buffers_lost.store(2, Ordering::Relaxed);
    capture.dropped.store(3, Ordering::Relaxed);
    capture.pool_exhausted.store(10, Ordering::Relaxed);
    capture.pool_exhausted_jumbo.store(4, Ordering::Relaxed);
    capture.callback_errors.store(5, Ordering::Relaxed);
    capture.receiver_disconnected.store(6, Ordering::Relaxed);
    capture.stats_errors.store(7, Ordering::Relaxed);
    capture.etw_buffers_allocated.store(8, Ordering::Relaxed);
    capture.etw_buffers_maximum.store(9, Ordering::Relaxed);
    capture.stats_valid.store(true, Ordering::Relaxed);
    parse.rejected.store(11, Ordering::Relaxed);
    parse.schema_rejected.store(12, Ordering::Relaxed);
    parse.output_failed_events.store(13, Ordering::Relaxed);
    parse.queue_depth_interval_max.store(14, Ordering::Relaxed);
    sink.disconnected_dropped.store(15, Ordering::Relaxed);
    sink.queue_dropped.store(16, Ordering::Relaxed);
    sink.spool_dropped.store(17, Ordering::Relaxed);
    sink.spool_dropped_bytes.store(18, Ordering::Relaxed);
    sink.send_queue_depth_max.store(19, Ordering::Relaxed);
    sink.spool_bytes.store(20, Ordering::Relaxed);
    sink.spool_segments.store(21, Ordering::Relaxed);
    sink.connected.store(true, Ordering::Relaxed);
    sink.spool_available.store(true, Ordering::Relaxed);

    let read = Readings::read(&capture, &parse, &sink, Some(22));
    assert_eq!(
        read,
        Readings {
            etw_lost: 1,
            etw_buffers_lost: 2,
            ring_dropped: 3,
            pool_exhausted_regular: 6,
            pool_exhausted_jumbo: 4,
            disconnected_dropped: 15,
            queue_dropped: 16,
            spool_dropped: 17,
            spool_dropped_bytes: 18,
            rejected: 11,
            schema_rejected: 12,
            callback_errors: 5,
            receiver_disconnected: 6,
            output_failed_events: 13,
            etw_stats_errors: 7,
            channel_peak: 14,
            etw_buffers_allocated: 8,
            etw_buffers_maximum: 9,
            send_queue_peak: 19,
            spool_bytes: 20,
            spool_segments: 21,
            connected: true,
            spool_available: true,
            etw_stats_valid: true,
            volume_free: Some(22),
        }
    );
    // The reading took the peak of the interval; `queue_depth_max` is untouched.
    assert_eq!(parse.queue_depth_interval_max.load(Ordering::Relaxed), 0);
    assert_eq!(
        Readings::read(&capture, &parse, &sink, None).channel_peak,
        0
    );
}

// What is written.

fn text_of(notice: Notice) -> String {
    capture_logs_at(tracing::Level::INFO, || notice.emit()).text()
}

fn rise(by: u64, total: u64) -> Rise {
    Rise { by, total }
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "a table of one case for each loss ID"
)]
fn each_loss_is_a_warning_with_its_own_target_and_names_the_counters_that_rose() {
    let none = rise(0, 0);
    let cases = [
        (
            Loss::Etw {
                events: rise(5, 50),
                buffers: none,
                seconds: 60,
            },
            "event::10",
            &[
                "etw_lost_increase=5",
                "etw_lost=50",
                "seconds=60",
                "--etw-max-buffers",
            ][..],
            &["etw_buffers_lost"][..],
        ),
        (
            Loss::Channel {
                dropped: rise(6, 60),
                capacity: 131_072,
                seconds: 60,
            },
            "event::11",
            &[
                "ring_dropped_increase=6",
                "ring_dropped=60",
                "capacity=131072",
                "--capacity",
            ][..],
            &[][..],
        ),
        (
            Loss::Pool {
                regular: none,
                jumbo: rise(7, 70),
                jumbo_capacity: 64,
                seconds: 60,
            },
            "event::12",
            &[
                "pool_exhausted_jumbo_increase=7",
                "jumbo_capacity=64",
                "--jumbo-capacity",
            ][..],
            &["pool_exhausted_regular"][..],
        ),
        (
            Loss::Network {
                disconnected: none,
                queue: rise(8, 80),
                seconds: 60,
            },
            "event::13",
            &["queue_dropped_increase=8", "queue_dropped=80"][..],
            &["disconnected_dropped_increase", "disconnected_dropped="][..],
        ),
        (
            Loss::Spool {
                frames: rise(9, 90),
                bytes: rise(900, 9_000),
                seconds: 60,
            },
            "event::14",
            &[
                "spool_dropped_increase=9",
                "spool_dropped_bytes_increase=900",
            ][..],
            &[][..],
        ),
        (
            Loss::Decode {
                rejected: rise(1, 11),
                schema: rise(1, 2),
                seconds: 60,
            },
            "event::15",
            &["rejected_increase=1", "schema_rejected_increase=1"][..],
            &[][..],
        ),
        (
            Loss::Internal {
                callback_errors: rise(3, 33),
                receiver_disconnected: none,
                output_failed: none,
                seconds: 60,
            },
            "event::16",
            &["callback_errors_increase=3"][..],
            &["receiver_disconnected=", "output_failed_events="][..],
        ),
    ];
    for (loss, target, present, absent) in cases {
        let text = text_of(Notice::Loss(loss));
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("WARN"), "{text}");
        assert!(text.contains(target), "{target}: {text}");
        for needle in present {
            assert!(text.contains(needle), "{needle}: {text}");
        }
        for needle in absent {
            assert!(!text.contains(needle), "{needle}: {text}");
        }
        assert!(text.contains("docs/guide/"), "{text}");
    }
}

#[test]
fn each_raised_risk_is_a_warning_with_its_own_target() {
    let cases = [
        (
            Risk::Channel {
                peak: 900,
                capacity: 1000,
            },
            "event::20",
            "percent=90",
        ),
        (
            Risk::EtwBuffers {
                allocated: 95,
                maximum: 100,
            },
            "event::21",
            "percent=95",
        ),
        (
            Risk::SendQueue {
                peak: 230,
                blocks: 256,
            },
            "event::22",
            "send_queue_depth_max=230",
        ),
        (
            Risk::ReceiverDownUnspooled,
            "event::24",
            "spool_available=false",
        ),
        (
            Risk::SpoolLimit {
                spool_bytes: 700 * MIB,
                spool_max_bytes: 800 * MIB,
                volume_free: Some(150 * MIB),
                floor_bytes: 164 * MIB,
            },
            "event::25",
            "needed_mib=164",
        ),
        (
            Risk::EtwStats { errors: rise(2, 4) },
            "event::28",
            "etw_stats_errors=4",
        ),
    ];
    for (risk, target, field) in cases {
        let text = text_of(Notice::Raised(risk));
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("WARN"), "{text}");
        assert!(text.contains(target), "{target}: {text}");
        assert!(text.contains(field), "{field}: {text}");
    }
}

#[test]
fn a_cleared_risk_is_an_information_event_without_a_warning_target() {
    let risks = [
        Risk::Channel {
            peak: 1,
            capacity: 1000,
        },
        Risk::EtwBuffers {
            allocated: 1,
            maximum: 100,
        },
        Risk::SendQueue {
            peak: 1,
            blocks: 256,
        },
        Risk::ReceiverDownUnspooled,
        Risk::SpoolLimit {
            spool_bytes: MIB,
            spool_max_bytes: 800 * MIB,
            volume_free: None,
            floor_bytes: 164 * MIB,
        },
        Risk::EtwStats { errors: rise(0, 4) },
    ];
    for risk in risks {
        let text = text_of(Notice::Cleared(risk));
        assert_eq!(text.lines().count(), 1, "{text}");
        assert!(text.contains("INFO"), "{text}");
        assert!(!text.contains("WARN"), "{text}");
        assert!(!text.contains("etw_dnstap::event::"), "{text}");
        assert!(
            text.contains("no longer") || text.contains("again"),
            "{text}"
        );
    }
}

#[test]
fn the_text_names_no_host_path() {
    let notices = [
        Notice::Loss(Loss::Spool {
            frames: rise(1, 1),
            bytes: rise(1, 1),
            seconds: 1,
        }),
        Notice::Raised(Risk::SpoolLimit {
            spool_bytes: MIB,
            spool_max_bytes: MIB,
            volume_free: Some(MIB),
            floor_bytes: MIB,
        }),
    ];
    for notice in notices {
        let text = text_of(notice);
        for forbidden in ["C:\\", "ProgramData"] {
            assert!(!text.contains(forbidden), "{forbidden}: {text}");
        }
    }
}
