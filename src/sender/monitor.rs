//! What the operator sees of a slow or saturated receiver.
//!
//! [`LinkMonitor`] only observes. Nothing it computes feeds back into routing:
//! the spill to the spool stays passive, driven by the link queue alone. The router thread owns the monitor and samples it on its own clock,
//! once per [`SAMPLE_PERIOD`], whatever `--stats-interval-seconds` is. At each
//! sample it
//!
//! - rolls the per-window peaks (send queue depth and socket write time) and
//!   publishes the peak of the last [`PEAK_WINDOW`] samples,
//! - publishes the age of the oldest spooled data, and
//! - updates the `link_saturated` state (a trend of the unread backlog over
//!   [`TREND_SAMPLES`] samples) and logs a change.
//!
//! The published values are plain stores of one writer. A reader (the periodic
//! stats log, the performance counters) only loads them, so no reader resets a
//! value under another. The window is the same for every reader.
//!
//! The monitor takes everything it needs in an [`Observation`] and the shared
//! counters, so a test supplies the time and the spool state and does not wait.
//! A later link thread can feed it the same way from the thread that owns the
//! spool.

use std::{
    collections::VecDeque,
    sync::{Arc, atomic::Ordering},
    time::{Duration, Instant},
};

use crate::sink::SinkStats;

/// How often the router thread samples. This is the unit of every window below.
pub(super) const SAMPLE_PERIOD: Duration = Duration::from_secs(1);

/// Samples in the window of a published peak: about a minute. It matches the
/// default `--stats-interval-seconds`, so the periodic log shows the peak of
/// the interval it closes.
pub(super) const PEAK_WINDOW: usize = 60;

/// Samples that the saturation trend spans: the state compares the backlog now
/// with the backlog this many samples ago (at least this many seconds).
///
/// Ten samples are longer than the memory slack that absorbs a receiver pause
/// before the spool starts (about 0.9 s at 50,000 queries per second to about
/// 4.6 s at 10,000: the 8 MiB link queue and the kernel send buffer, taken as
/// 1 MiB, which Windows can grow), so a hiccup that the link queue absorbs does
/// not count. They are
/// short against the minutes a saturated link needs to fill the spool to
/// `--spool-max-mib` (about 14 minutes at the 4 GiB default and a 5 MB/s link).
pub(super) const TREND_SAMPLES: usize = 10;

/// Steps of the [`TREND_SAMPLES`] that must show a larger backlog for the state
/// to set, besides a larger backlog than [`TREND_SAMPLES`] samples ago.
///
/// The replay reads in 64 KiB batches, one at a time, and the router appends
/// 64 KiB blocks, so a backlog that grows slowly has flat or falling samples between
/// the rises. With a capture of 5.5 MB/s and a link of 5.3 MB/s about one step
/// in five is flat, so 8 of 10 rise. Seven leaves room for three flat steps,
/// and it still rejects one burst (a single jump, then flat). A draining
/// backlog is not larger than ten samples ago, so it fails the first condition
/// whatever its steps do.
pub(super) const RISING_STEPS: usize = 7;
// The log message and the guides state both numbers in their text.
const _: () = assert!(TREND_SAMPLES == 10 && RISING_STEPS == 7);

/// The largest value of the last [`PEAK_WINDOW`] samples.
struct PeakWindow {
    buckets: [u64; PEAK_WINDOW],
    current: usize,
}

impl PeakWindow {
    const fn new() -> Self {
        Self {
            buckets: [0; PEAK_WINDOW],
            current: 0,
        }
    }

    /// Notes a value in the current sample.
    fn record(&mut self, value: u64) {
        self.buckets[self.current] = self.buckets[self.current].max(value);
    }

    /// The largest value in the window, the current sample included.
    fn peak(&self) -> u64 {
        self.buckets.iter().copied().max().unwrap_or(0)
    }

    /// Ends the current sample; the oldest one leaves the window.
    const fn roll(&mut self) {
        self.current = (self.current + 1) % PEAK_WINDOW;
        self.buckets[self.current] = 0;
    }
}

/// What the router thread reads at a sample.
#[derive(Clone, Copy, Debug)]
pub(super) struct Observation {
    /// The sample time.
    pub(super) at: Instant,
    /// A session is established.
    pub(super) link_up: bool,
    /// The spool cannot be read now (an error paused it), so the backlog cannot
    /// fall for a reason that has nothing to do with the link.
    pub(super) reads_paused: bool,
    /// Unread spool bytes; 0 with no backlog or no spool.
    pub(super) backlog_bytes: u64,
    /// Sessions lost since the service started. A change means the link went
    /// down and came back since the last sample, even if no sample saw it down.
    pub(super) disconnects: u64,
    /// Age of the oldest spooled data; zero with no backlog.
    pub(super) oldest_age: Duration,
}

/// Why `link_saturated` went back to false.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClearReason {
    /// The spool has no backlog.
    BacklogEmpty,
    /// The backlog is not larger than [`TREND_SAMPLES`] samples ago.
    BacklogStopped,
}

impl ClearReason {
    const fn text(self) -> &'static str {
        match self {
            Self::BacklogEmpty => "the backlog is empty",
            Self::BacklogStopped => "the backlog stopped growing",
        }
    }
}

/// A change of the `link_saturated` state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Change {
    Saturated,
    Cleared(ClearReason),
}

/// The `link_saturated` state machine. It reads the backlog once per sample.
struct Saturation {
    saturated: bool,
    /// The backlog at the last [`TREND_SAMPLES`] + 1 counted samples, oldest
    /// first. Cleared when a sample does not count.
    window: VecDeque<u64>,
    /// Counted samples since the state last changed. A change is not undone
    /// before a whole trend window has passed, so events are at least
    /// [`TREND_SAMPLES`] samples apart.
    since_change: usize,
    /// `disconnects` at the last sample.
    disconnects: Option<u64>,
}

impl Saturation {
    fn new() -> Self {
        Self {
            saturated: false,
            window: VecDeque::with_capacity(TREND_SAMPLES + 2),
            since_change: TREND_SAMPLES,
            disconnects: None,
        }
    }

    /// Takes one sample and returns the change it causes, if any.
    ///
    /// - With no backlog the state clears at once.
    /// - While the link is down, reads are paused, or a session was lost since
    ///   the last sample, the window restarts and the state holds (a reconnect
    ///   that flaps does not log). The growth of an outage is not a saturated
    ///   link, even when the outage ended between two samples.
    /// - Otherwise the state sets when the backlog is larger than
    ///   [`TREND_SAMPLES`] samples ago and at least [`RISING_STEPS`] of those
    ///   steps rose. It clears when the backlog is not larger than
    ///   [`TREND_SAMPLES`] samples ago.
    fn observe(&mut self, observation: &Observation) -> Option<Change> {
        let session_lost = self
            .disconnects
            .replace(observation.disconnects)
            .is_some_and(|before| before != observation.disconnects);
        if observation.backlog_bytes == 0 {
            self.window.clear();
            self.since_change = 0;
            return std::mem::take(&mut self.saturated)
                .then_some(Change::Cleared(ClearReason::BacklogEmpty));
        }
        if !observation.link_up || observation.reads_paused || session_lost {
            self.window.clear();
            return None;
        }
        if self.window.len() > TREND_SAMPLES {
            self.window.pop_front();
        }
        self.window.push_back(observation.backlog_bytes);
        self.since_change = self.since_change.saturating_add(1);
        if self.window.len() <= TREND_SAMPLES || self.since_change < TREND_SAMPLES {
            return None;
        }
        let (oldest, newest) = (self.window[0], observation.backlog_bytes);
        if self.saturated {
            if newest <= oldest {
                self.saturated = false;
                self.since_change = 0;
                return Some(Change::Cleared(ClearReason::BacklogStopped));
            }
            return None;
        }
        let rising = self
            .window
            .iter()
            .zip(self.window.iter().skip(1))
            .filter(|(before, after)| after > before)
            .count();
        if newest > oldest && rising >= RISING_STEPS {
            self.saturated = true;
            self.since_change = 0;
            return Some(Change::Saturated);
        }
        None
    }
}

/// Publishes the observed values and owns the clock that samples them.
pub(super) struct LinkMonitor {
    stats: Arc<SinkStats>,
    /// When the next sample is due.
    next_sample: Instant,
    queue_peak: PeakWindow,
    write_peak: PeakWindow,
    saturation: Saturation,
    /// `(time, bytes_sent)` at the last [`TREND_SAMPLES`] + 1 samples, for
    /// the send rate in a log event.
    sent: VecDeque<(Instant, u64)>,
}

impl LinkMonitor {
    pub(super) fn new(stats: Arc<SinkStats>, now: Instant) -> Self {
        Self {
            stats,
            next_sample: now + SAMPLE_PERIOD,
            queue_peak: PeakWindow::new(),
            write_peak: PeakWindow::new(),
            saturation: Saturation::new(),
            sent: VecDeque::with_capacity(TREND_SAMPLES + 2),
        }
    }

    /// True when a sample is due. Cheap: one comparison.
    pub(super) fn due(&self, now: Instant) -> bool {
        now >= self.next_sample
    }

    /// Makes the next [`Self::due`] call true, so a test need not wait a period.
    #[cfg(test)]
    pub(super) fn force_due(&mut self) {
        self.next_sample = Instant::now();
    }

    /// Notes the blocks waiting in the send queue. Call it where the sender
    /// thread takes a block, with the length of the queue plus that block.
    pub(super) fn note_queue_depth(&mut self, blocks: usize) {
        self.queue_peak.record(blocks as u64);
    }

    /// Takes a sample: publishes the peaks and the oldest age, updates the
    /// `link_saturated` state, and logs a change of it. The next sample is due
    /// one period after this one, so a long block on the router thread delays
    /// samples but never makes them run in a burst.
    pub(super) fn sample(&mut self, observation: &Observation) {
        self.next_sample = observation.at + SAMPLE_PERIOD;

        // Only this thread resets the accumulator; readers see the window peak.
        let written = self.stats.write_window_max_us.swap(0, Ordering::Relaxed);
        self.write_peak.record(written);
        self.stats
            .write_max_recent_us
            .store(self.write_peak.peak(), Ordering::Relaxed);
        self.write_peak.roll();

        self.stats
            .send_queue_depth_max
            .store(self.queue_peak.peak(), Ordering::Relaxed);
        self.queue_peak.roll();

        let age_ms = u64::try_from(observation.oldest_age.as_millis()).unwrap_or(u64::MAX);
        self.stats
            .spool_oldest_age_ms
            .store(age_ms, Ordering::Relaxed);

        if self.sent.len() > TREND_SAMPLES {
            self.sent.pop_front();
        }
        self.sent.push_back((
            observation.at,
            self.stats.bytes_sent.load(Ordering::Relaxed),
        ));

        if let Some(change) = self.saturation.observe(observation) {
            self.stats
                .link_saturated
                .store(change == Change::Saturated, Ordering::Relaxed);
            self.log(change, observation);
        }
    }

    /// Bytes per second that the link accepted over the retained samples.
    fn send_rate(&self) -> u64 {
        let (Some(&(first_at, first_bytes)), Some(&(last_at, last_bytes))) =
            (self.sent.front(), self.sent.back())
        else {
            return 0;
        };
        let micros = last_at.duration_since(first_at).as_micros();
        if micros == 0 {
            return 0;
        }
        let bytes = u128::from(last_bytes.saturating_sub(first_bytes));
        u64::try_from(bytes * 1_000_000 / micros).unwrap_or(u64::MAX)
    }

    fn log(&self, change: Change, observation: &Observation) {
        let backlog_bytes = observation.backlog_bytes;
        let oldest_age_ms = u64::try_from(observation.oldest_age.as_millis()).unwrap_or(u64::MAX);
        let bytes_per_second = self.send_rate();
        match change {
            Change::Saturated => tracing::warn!(
                backlog_bytes,
                oldest_age_ms,
                bytes_sent_per_second = bytes_per_second,
                "dnstap link is saturated: the spool backlog has grown steadily over the last \
                 10 seconds while the receiver is connected, so the receiver or the network \
                 path accepts less than the service captures; the spool keeps the data in \
                 order and, at --spool-max-mib, deletes its oldest segments unsent"
            ),
            Change::Cleared(reason) => tracing::info!(
                backlog_bytes,
                oldest_age_ms,
                bytes_sent_per_second = bytes_per_second,
                reason = reason.text(),
                "dnstap link is no longer saturated"
            ),
        }
    }
}

#[cfg(test)]
#[path = "monitor_tests.rs"]
mod monitor_tests;
