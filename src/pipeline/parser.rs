//! The dnstap parser thread: drains the event channel, encodes each record,
//! writes it to the sink, and flushes on idle and busy deadlines.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;

use super::config::DiagContext;
use crate::{
    dnstap::Encoder,
    etw::{self, RawEtwEvent},
    sink::FrameSink,
};

#[derive(Default)]
pub(crate) struct ParseCounters {
    pub(crate) dequeued: AtomicU64,
    pub(crate) encoded: AtomicU64,
    pub(super) ignored: AtomicU64,
    pub(crate) rejected: AtomicU64,
    pub(super) output_failed_events: AtomicU64,
    pub(crate) output_errors: AtomicU64,
    /// Events rejected because their schema failed to load (first failure and
    /// every cached rejection). Also counted in `rejected`.
    pub(super) schema_rejected: AtomicU64,
    /// Deepest the event channel was seen: sampled by the parser on the first
    /// event after each idle wake and every 256 events, so a lower bound.
    pub(crate) queue_depth_max: AtomicU64,
    #[cfg(feature = "diag-timing")]
    pub(super) timing: DiagTiming,
}

/// Parser-thread wait attribution, compiled only for the diagnostic build. Only
/// the parser thread writes these; the final report reads them after the join.
#[cfg(feature = "diag-timing")]
#[derive(Default)]
pub(super) struct DiagTiming {
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

#[cfg(feature = "diag-timing")]
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

    fn record_write(&self, elapsed: Duration) {
        self.write_ns
            .fetch_add(Self::nanos(elapsed), Ordering::Relaxed);
        if elapsed > Self::SLOW_CALL {
            self.writes_over_1ms.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record_flush(&self, elapsed: Duration) {
        self.flush_ns
            .fetch_add(Self::nanos(elapsed), Ordering::Relaxed);
        if elapsed > Self::SLOW_CALL {
            self.flushes_over_1ms.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Applies the configured diagnostic priority to the calling thread.
#[cfg(feature = "diag-timing")]
fn apply_thread_priority(diag: DiagContext) {
    if let Some(priority) = diag.thread_priority {
        crate::etw::set_current_thread_priority(priority);
    }
}

#[cfg(not(feature = "diag-timing"))]
const fn apply_thread_priority(_diag: DiagContext) {}

pub(super) fn spawn_parser<S>(
    queue: Receiver<RawEtwEvent>,
    producer_done: Arc<AtomicBool>,
    counters: Arc<ParseCounters>,
    identity: String,
    writer: S,
    diag: DiagContext,
) -> Result<thread::JoinHandle<Result<()>>>
where
    S: FrameSink + Send + 'static,
{
    thread::Builder::new()
        .name("dnstap-parser".into())
        .spawn(move || parse_loop(&queue, &producer_done, &counters, &identity, writer, diag))
        .context("start dnstap parser thread")
}

pub(super) fn join_parser(parser: thread::JoinHandle<Result<()>>) -> Result<()> {
    parser
        .join()
        .map_err(|_| anyhow::anyhow!("dnstap parser thread panicked"))?
}

fn parse_loop<S: FrameSink>(
    queue: &Receiver<RawEtwEvent>,
    producer_done: &AtomicBool,
    counters: &ParseCounters,
    identity: &str,
    mut writer: S,
    diag: DiagContext,
) -> Result<()> {
    apply_thread_priority(diag);
    let mut parser = etw::Parser::new();
    let mut encoder = Encoder::new(identity.as_bytes())?;
    let mut last_flush = Instant::now();
    // Bytes may sit in the sink's buffer since the last flush.
    let mut dirty = false;
    // True after an idle poll, so the next event samples the queue depth.
    let mut sample_depth = true;
    let mut events_since_clock = 0u32;
    let consume = |event: Option<RawEtwEvent>| {
        let idle = event.is_none();
        if idle {
            sample_depth = true;
        }
        if let Some(event) = event {
            let dequeued = counters.dequeued.fetch_add(1, Ordering::Relaxed);
            if sample_depth || dequeued.is_multiple_of(256) {
                sample_depth = false;
                let depth = u64::try_from(queue.len()).unwrap_or(u64::MAX);
                counters
                    .queue_depth_max
                    .fetch_max(depth.saturating_add(1), Ordering::Relaxed);
            }
            // The decoded view borrows the pooled record. Encoding copies the packet
            // into encoder storage, so the record is released before network I/O and
            // the returned payload borrows only reusable encoder storage.
            let result = parse_and_encode(&mut parser, event, &mut encoder);
            if result.is_err() {
                counters
                    .schema_rejected
                    .store(parser.schema_rejected_events(), Ordering::Relaxed);
            }
            match result {
                Ok(Some(payload)) => {
                    #[cfg(feature = "diag-timing")]
                    let write_started = Instant::now();
                    let sent = writer.write_frame(payload);
                    #[cfg(feature = "diag-timing")]
                    counters.timing.record_write(write_started.elapsed());
                    sent.context("send dnstap packet").inspect_err(|_error| {
                        counters
                            .output_failed_events
                            .fetch_add(1, Ordering::Relaxed);
                        counters.output_errors.fetch_add(1, Ordering::Relaxed);
                    })?;
                    dirty = true;
                    counters.encoded.fetch_add(1, Ordering::Relaxed);
                }
                Ok(None) => {
                    counters.ignored.fetch_add(1, Ordering::Relaxed);
                }
                Err(error) => {
                    let previous = counters.rejected.fetch_add(1, Ordering::Relaxed);
                    // Bound diagnostics and never print event payloads or domain names.
                    if previous < 4 {
                        tracing::warn!(%error, "rejected ETW event; further errors counted");
                    }
                }
            }
        }
        // Read the clock only when it can matter: on every idle poll that has
        // unflushed bytes (so a trickle is not held for a second), and once per
        // CLOCK_CHECK_EVENTS events while busy.
        let flush_due = if idle {
            dirty && last_flush.elapsed() >= IDLE_FLUSH_AFTER
        } else {
            events_since_clock += 1;
            if events_since_clock >= CLOCK_CHECK_EVENTS {
                events_since_clock = 0;
                dirty && last_flush.elapsed() >= BUSY_FLUSH_AFTER
            } else {
                false
            }
        };
        if flush_due {
            #[cfg(feature = "diag-timing")]
            let flush_started = Instant::now();
            let flushed = writer.flush();
            #[cfg(feature = "diag-timing")]
            counters.timing.record_flush(flush_started.elapsed());
            flushed
                .context("flush dnstap TCP sender")
                .inspect_err(|_error| {
                    counters.output_errors.fetch_add(1, Ordering::Relaxed);
                })?;
            last_flush = Instant::now();
            dirty = false;
        }
        Ok(())
    };
    #[cfg(not(feature = "diag-timing"))]
    drain_queue(queue, producer_done, consume)?;
    #[cfg(feature = "diag-timing")]
    drain_queue_inner(queue, producer_done, Some(&counters.timing), consume)?;
    writer
        .finish()
        .context("finish dnstap STOP/FINISH exchange")
        .inspect_err(|_error| {
            counters.output_errors.fetch_add(1, Ordering::Relaxed);
        })
}

/// Decodes one record and encodes it to dnstap bytes, the production step of the
/// parser thread. The record (and its pool lease) is released before this
/// returns, so the bytes borrow only the encoder.
pub(crate) fn parse_and_encode<'e>(
    parser: &mut etw::Parser,
    event: RawEtwEvent,
    encoder: &'e mut Encoder,
) -> Result<Option<&'e [u8]>> {
    let result = match parser.parse(&event) {
        Ok(Some(decoded)) => encoder.encode(&decoded).map(Some),
        Ok(None) => Ok(None),
        Err(error) => Err(error),
    };
    drop(event);
    result
}

// The per-event ETW callback stays wake-free; see docs/history/ring-wakeup.md.
// A real-time session delivers events in bursts (about once per
// `--etw-flush-seconds`).
// The ETW buffer callback wakes the parser once per delivered buffer that
// enqueued events, so draining starts within microseconds of a burst. After
// data, the parser lingers for LINGER_AFTER_DATA before it parks, so a burst
// costs about one wake, not one per buffer. The timed polls below are a fallback
// for test producers, flushing the sink, and shutdown: each empty poll doubles
// the park from 2 ms to at most 20 ms, and any received event resets it to 2 ms.
// Windows' default ~15.6 ms timer granularity stretches these requests, so a
// 20 ms park can last about 31 ms. Shutdown is unaffected because completion is
// followed by unpark, which ends any park.
const IDLE_POLL_MIN: Duration = Duration::from_millis(2);
/// After a drain that found data, poll this long before parking.
const LINGER_AFTER_DATA: Duration = Duration::from_micros(200);
/// With bytes pending, an idle parser flushes if the last flush is older than this.
const IDLE_FLUSH_AFTER: Duration = Duration::from_millis(100);
/// While busy the sink's buffer fills and flushes itself; this bounds the wait
/// for a partly full buffer.
const BUSY_FLUSH_AFTER: Duration = Duration::from_secs(1);
/// While busy, the flush clock is read once per this many events.
const CLOCK_CHECK_EVENTS: u32 = 64;
const IDLE_POLL_MAX: Duration = Duration::from_millis(20);

/// Resets the idle poll interval after an event and otherwise doubles it, clamped
/// to `IDLE_POLL_MAX`.
fn idle_poll_after(received: bool, current: Duration) -> Duration {
    if received {
        IDLE_POLL_MIN
    } else {
        current.saturating_mul(2).min(IDLE_POLL_MAX)
    }
}

// The completion flag is separate from the queue: after observing completion,
// retry receive so a final send between the previous receive and flag load drains.
#[cfg(any(test, not(feature = "diag-timing")))]
fn drain_queue<T>(
    queue: &Receiver<T>,
    producer_done: &AtomicBool,
    consume: impl FnMut(Option<T>) -> Result<()>,
) -> Result<()> {
    #[cfg(feature = "diag-timing")]
    {
        drain_queue_inner(queue, producer_done, None, consume)
    }
    #[cfg(not(feature = "diag-timing"))]
    {
        drain_queue_inner(queue, producer_done, consume)
    }
}

fn drain_queue_inner<T>(
    queue: &Receiver<T>,
    producer_done: &AtomicBool,
    #[cfg(feature = "diag-timing")] diag: Option<&DiagTiming>,
    mut consume: impl FnMut(Option<T>) -> Result<()>,
) -> Result<()> {
    let mut idle_poll = IDLE_POLL_MIN;
    // True after an event was consumed since the last park or linger.
    let mut had_data = false;
    // Off after a linger that found no data, back on after a park that timed out
    // with nothing queued. When ETW delivers buffers one by one and further apart
    // than the linger window (high, steady rates), the parser then parks between
    // buffers instead of spinning through most of each gap.
    let mut linger_armed = true;
    #[cfg(feature = "diag-timing")]
    let (mut run, mut just_parked) = (0_u64, false);
    loop {
        #[cfg(feature = "diag-timing")]
        if let Some(diag) = diag
            && just_parked
        {
            just_parked = false;
            diag.record_wake(queue.len(), queue.capacity());
        }
        if let Ok(event) = queue.try_recv() {
            idle_poll = idle_poll_after(true, idle_poll);
            had_data = true;
            #[cfg(feature = "diag-timing")]
            {
                run += 1;
            }
            consume(Some(event))?;
        } else if producer_done.load(Ordering::Acquire) {
            while let Ok(event) = queue.try_recv() {
                #[cfg(feature = "diag-timing")]
                {
                    run += 1;
                }
                consume(Some(event))?;
            }
            #[cfg(feature = "diag-timing")]
            if let Some(diag) = diag {
                diag.record_run(run);
            }
            return Ok(());
        } else {
            consume(None)?;
            if had_data && linger_armed {
                had_data = false;
                // Inside an ETW delivery burst the next buffer usually follows
                // within microseconds. Polling briefly avoids a park and a costly
                // wake (about 17 us for the waker on the lab VM) per buffer.
                let linger = Instant::now();
                while queue.is_empty()
                    && !producer_done.load(Ordering::Acquire)
                    && linger.elapsed() < LINGER_AFTER_DATA
                {
                    std::hint::spin_loop();
                }
                // New data, or completion: loop without parking.
                if !queue.is_empty() || producer_done.load(Ordering::Acquire) {
                    continue;
                }
                linger_armed = false;
            }
            had_data = false;
            #[cfg(feature = "diag-timing")]
            let parked = Instant::now();
            thread::park_timeout(idle_poll);
            if queue.is_empty() {
                // A quiet period (or a spurious wake): the next burst may linger.
                linger_armed = true;
            }
            #[cfg(feature = "diag-timing")]
            if let Some(diag) = diag {
                diag.record_park(parked.elapsed(), idle_poll);
                diag.record_run(run);
                run = 0;
                just_parked = true;
            }
            idle_poll = idle_poll_after(false, idle_poll);
        }
    }
}

#[cfg(feature = "diag-timing")]
pub(super) fn report_timing(timing: &DiagTiming) {
    let parks = timing.parks.load(Ordering::Relaxed);
    let park_ns = timing.park_ns.load(Ordering::Relaxed);
    tracing::info!(
        parks,
        backoff_parks = timing.backoff_parks.load(Ordering::Relaxed),
        park_ns,
        mean_park_ns = park_ns.checked_div(parks).unwrap_or(0),
        wake_len_max = timing.wake_len_max.load(Ordering::Relaxed),
        wake_len_0 = timing.wake_len_0.load(Ordering::Relaxed),
        wake_len_lt_1k = timing.wake_len_lt_1k.load(Ordering::Relaxed),
        wake_len_lt_4k = timing.wake_len_lt_4k.load(Ordering::Relaxed),
        wake_len_lt_8k = timing.wake_len_lt_8k.load(Ordering::Relaxed),
        wake_len_ge_8k_or_full = timing.wake_len_ge_8k_or_full.load(Ordering::Relaxed),
        max_run = timing.max_run.load(Ordering::Relaxed),
        write_ns = timing.write_ns.load(Ordering::Relaxed),
        writes_over_1ms = timing.writes_over_1ms.load(Ordering::Relaxed),
        flush_ns = timing.flush_ns.load(Ordering::Relaxed),
        flushes_over_1ms = timing.flushes_over_1ms.load(Ordering::Relaxed),
        "parser timing"
    );
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod parser_tests;
