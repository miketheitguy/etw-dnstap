//! Parser-thread wait attribution, compiled only for the diagnostic build
//! (`diag-timing`). `timing_off.rs` has the same method set as empty inline
//! methods on zero-sized types, so the parser loops call these unconditionally.

use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use crossbeam_channel::Receiver;

use super::IDLE_POLL_MIN;

/// Parser-thread wait attribution. Only the parser thread writes these; the
/// final report reads them after the join.
#[derive(Default)]
pub(crate) struct DiagTiming {
    parks: AtomicU64,
    backoff_parks: AtomicU64,
    park_ns: AtomicU64,
    wake_len_max: AtomicU64,
    wake_len_0: AtomicU64,
    wake_len_lt_1k: AtomicU64,
    wake_len_lt_4k: AtomicU64,
    wake_len_lt_8k: AtomicU64,
    wake_len_ge_8k_or_full: AtomicU64,
    max_run: AtomicU64,
    write_ns: AtomicU64,
    writes_over_1ms: AtomicU64,
    flush_ns: AtomicU64,
    flushes_over_1ms: AtomicU64,
}

impl DiagTiming {
    const SLOW_CALL: Duration = Duration::from_millis(1);

    fn nanos(elapsed: Duration) -> u64 {
        u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX)
    }

    fn record_park(&self, elapsed: Duration, requested: Duration) {
        self.parks.fetch_add(1, Ordering::Relaxed);
        if requested > IDLE_POLL_MIN {
            self.backoff_parks.fetch_add(1, Ordering::Relaxed);
        }
        self.park_ns
            .fetch_add(Self::nanos(elapsed), Ordering::Relaxed);
    }

    /// Records the queue length first seen after a park. "Full" is `len >=
    /// capacity` for a bounded queue.
    fn record_wake(&self, len: usize, capacity: Option<usize>) {
        let len = u64::try_from(len).unwrap_or(u64::MAX);
        self.wake_len_max.fetch_max(len, Ordering::Relaxed);
        let full =
            capacity.is_some_and(|capacity| len >= u64::try_from(capacity).unwrap_or(u64::MAX));
        let bucket = if len == 0 {
            &self.wake_len_0
        } else if len >= 8192 || full {
            &self.wake_len_ge_8k_or_full
        } else if len < 1024 {
            &self.wake_len_lt_1k
        } else if len < 4096 {
            &self.wake_len_lt_4k
        } else {
            &self.wake_len_lt_8k
        };
        bucket.fetch_add(1, Ordering::Relaxed);
    }

    fn record_run(&self, events: u64) {
        self.max_run.fetch_max(events, Ordering::Relaxed);
    }

    pub(crate) fn record_write(&self, elapsed: Duration) {
        self.write_ns
            .fetch_add(Self::nanos(elapsed), Ordering::Relaxed);
        if elapsed > Self::SLOW_CALL {
            self.writes_over_1ms.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub(crate) fn record_flush(&self, elapsed: Duration) {
        self.flush_ns
            .fetch_add(Self::nanos(elapsed), Ordering::Relaxed);
        if elapsed > Self::SLOW_CALL {
            self.flushes_over_1ms.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The per-drain-loop state that feeds this collector. Create one for each
    /// call of the channel drain.
    pub(crate) const fn drain(&self) -> DrainTiming<'_> {
        DrainTiming {
            timing: self,
            run: 0,
            just_parked: false,
        }
    }

    /// Logs the totals as the `parser timing` event.
    pub(crate) fn report(&self) {
        let parks = self.parks.load(Ordering::Relaxed);
        let park_ns = self.park_ns.load(Ordering::Relaxed);
        tracing::info!(
            parks,
            backoff_parks = self.backoff_parks.load(Ordering::Relaxed),
            park_ns,
            mean_park_ns = park_ns.checked_div(parks).unwrap_or(0),
            wake_len_max = self.wake_len_max.load(Ordering::Relaxed),
            wake_len_0 = self.wake_len_0.load(Ordering::Relaxed),
            wake_len_lt_1k = self.wake_len_lt_1k.load(Ordering::Relaxed),
            wake_len_lt_4k = self.wake_len_lt_4k.load(Ordering::Relaxed),
            wake_len_lt_8k = self.wake_len_lt_8k.load(Ordering::Relaxed),
            wake_len_ge_8k_or_full = self.wake_len_ge_8k_or_full.load(Ordering::Relaxed),
            max_run = self.max_run.load(Ordering::Relaxed),
            write_ns = self.write_ns.load(Ordering::Relaxed),
            writes_over_1ms = self.writes_over_1ms.load(Ordering::Relaxed),
            flush_ns = self.flush_ns.load(Ordering::Relaxed),
            flushes_over_1ms = self.flushes_over_1ms.load(Ordering::Relaxed),
            "parser timing"
        );
    }
}

/// A clock reading taken before a timed call. The diagnostic-off build keeps no
/// clock: its `Stamp` is empty and `elapsed` is zero.
#[derive(Clone, Copy)]
pub(crate) struct Stamp(Instant);

impl Stamp {
    pub(crate) fn now() -> Self {
        Self(Instant::now())
    }

    pub(crate) fn elapsed(self) -> Duration {
        self.0.elapsed()
    }
}

/// Run-length and wake bookkeeping for one drain loop. Only the parser thread
/// uses it; it writes into the shared [`DiagTiming`].
pub(crate) struct DrainTiming<'a> {
    timing: &'a DiagTiming,
    /// Events consumed since the last park.
    run: u64,
    /// True after a park, so the next pass records the queue length.
    just_parked: bool,
}

impl DrainTiming<'_> {
    /// Called at the top of each pass: after a park, records the queue length
    /// first seen on waking.
    pub(crate) fn wake<T>(&mut self, queue: &Receiver<T>) {
        if self.just_parked {
            self.just_parked = false;
            self.timing.record_wake(queue.len(), queue.capacity());
        }
    }

    /// Called for each event handed to the consumer.
    pub(crate) const fn event(&mut self) {
        self.run += 1;
    }

    /// Called when the drain completes.
    pub(crate) fn finish(&self) {
        self.timing.record_run(self.run);
    }

    /// Called after a park that began at `started` and asked for `requested`.
    pub(crate) fn parked(&mut self, started: Stamp, requested: Duration) {
        self.timing.record_park(started.elapsed(), requested);
        self.timing.record_run(self.run);
        self.run = 0;
        self.just_parked = true;
    }
}

#[cfg(test)]
#[path = "timing_on_tests.rs"]
mod timing_on_tests;
