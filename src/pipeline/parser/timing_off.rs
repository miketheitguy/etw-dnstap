//! The diagnostic-off counterpart of `timing_on.rs`: the same method set as
//! empty inline methods on zero-sized types. The parser loops call these
//! unconditionally and the optimizer removes every call, so the normal build
//! carries no counters, no clock reads, and no extra code.

use std::time::Duration;

use crossbeam_channel::Receiver;

/// Zero-sized stand-in for the parser-thread wait attribution.
#[derive(Default)]
pub(crate) struct DiagTiming {}

#[expect(
    clippy::unused_self,
    reason = "The methods mirror the diagnostic build, which reads the collector."
)]
impl DiagTiming {
    #[inline]
    pub(crate) const fn record_write(&self, _elapsed: Duration) {}

    #[inline]
    pub(crate) const fn record_flush(&self, _elapsed: Duration) {}

    #[inline]
    pub(crate) const fn drain(&self) -> DrainTiming {
        DrainTiming
    }

    #[inline]
    pub(crate) const fn report(&self) {}
}

/// Zero-sized stand-in for a clock reading: no clock is read.
#[derive(Clone, Copy)]
pub(crate) struct Stamp;

#[expect(
    clippy::unused_self,
    reason = "The method mirrors the diagnostic build, which reads the clock."
)]
impl Stamp {
    #[inline]
    pub(crate) const fn now() -> Self {
        Self
    }

    #[inline]
    pub(crate) const fn elapsed(self) -> Duration {
        Duration::ZERO
    }
}

/// Zero-sized stand-in for the per-drain-loop bookkeeping.
pub(crate) struct DrainTiming;

#[expect(
    clippy::unused_self,
    reason = "The methods mirror the diagnostic build, which reads the collector."
)]
impl DrainTiming {
    #[inline]
    pub(crate) const fn wake<T>(&mut self, _queue: &Receiver<T>) {}

    #[inline]
    pub(crate) const fn event(&mut self) {}

    #[inline]
    pub(crate) const fn finish(&self) {}

    #[inline]
    pub(crate) const fn parked(&mut self, _started: Stamp, _requested: Duration) {}
}
