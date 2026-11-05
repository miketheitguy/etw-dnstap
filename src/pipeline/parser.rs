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

#[cfg(feature = "diag-timing")]
#[path = "parser/timing_on.rs"]
mod timing;
#[cfg(not(feature = "diag-timing"))]
#[path = "parser/timing_off.rs"]
mod timing;

use self::timing::{DiagTiming, Stamp};
use super::config::DiagContext;
use crate::{
    dnstap::Encoder,
    etw::{self, RawEtwEvent},
    sink::FrameSink,
    threadcpu::ThreadCpu,
};

#[derive(Default)]
pub(crate) struct ParseCounters {
    /// Shutdown has begun: the parser logs nothing more. Rejections stay
    /// counted.
    pub(super) stopping: AtomicBool,
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
    /// The deepest sample since the supervisor last took it (a swap to 0). Only
    /// the supervisor's loss and risk check reads and resets it, so
    /// `queue_depth_max` keeps its meaning. Not reported.
    pub(super) queue_depth_interval_max: AtomicU64,
    /// Wait attribution; empty unless `diag-timing` is enabled.
    pub(super) timing: DiagTiming,
    /// CPU time of the `dnstap-parser` thread.
    pub(crate) cpu: ThreadCpu,
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
    let cpu_counters = Arc::clone(&counters);
    let parser = thread::Builder::new()
        .name("dnstap-parser".into())
        .spawn(move || parse_loop(&queue, &producer_done, &counters, &identity, writer, diag))
        .context("start dnstap parser thread")?;
    cpu_counters.cpu.register(&parser);
    Ok(parser)
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
                let sampled = depth.saturating_add(1);
                counters
                    .queue_depth_max
                    .fetch_max(sampled, Ordering::Relaxed);
                counters
                    .queue_depth_interval_max
                    .fetch_max(sampled, Ordering::Relaxed);
            }
            // The decoded view borrows the pooled record. Encoding copies the packet
            // into encoder storage, so the record is released before network I/O and
            // the returned payload borrows only reusable encoder storage.
            let stopping = counters.stopping.load(Ordering::Acquire);
            parser.set_quiet(stopping);
            let result = parse_and_encode(&mut parser, event, &mut encoder);
            if result.is_err() {
                counters
                    .schema_rejected
                    .store(parser.schema_rejected_events(), Ordering::Relaxed);
            }
            match result {
                Ok(Some(payload)) => {
                    let write_started = Stamp::now();
                    let sent = writer.write_frame(payload);
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
                    // Same cadence as the other counters: the first four, then the
                    // 61st, 121st, and so on. The schema cache logs its own per-key
                    // load failure; this line covers every rejected event.
                    if should_log_rejection(previous) && !stopping {
                        tracing::warn!(
                            %error,
                            rejected = previous.saturating_add(1),
                            "rejected ETW event; the first four and then every 60th are logged, all are counted"
                        );
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
            let flush_started = Stamp::now();
            let flushed = writer.flush();
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
    drain_queue(queue, producer_done, &counters.timing, consume)?;
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

/// Whether the rejection that follows `previous` earlier rejections is logged:
/// the first four, then the 61st, 121st, and so on.
const fn should_log_rejection(previous: u64) -> bool {
    previous < 4 || previous.is_multiple_of(60)
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
fn drain_queue<T>(
    queue: &Receiver<T>,
    producer_done: &AtomicBool,
    timing: &DiagTiming,
    mut consume: impl FnMut(Option<T>) -> Result<()>,
) -> Result<()> {
    let mut drain = timing.drain();
    let mut idle_poll = IDLE_POLL_MIN;
    // True after an event was consumed since the last park or linger.
    let mut had_data = false;
    // Off after a linger that found no data, back on after a park that timed out
    // with nothing queued. When ETW delivers buffers one by one and further apart
    // than the linger window (high, steady rates), the parser then parks between
    // buffers instead of spinning through most of each gap.
    let mut linger_armed = true;
    loop {
        drain.wake(queue);
        if let Ok(event) = queue.try_recv() {
            idle_poll = idle_poll_after(true, idle_poll);
            had_data = true;
            drain.event();
            consume(Some(event))?;
        } else if producer_done.load(Ordering::Acquire) {
            while let Ok(event) = queue.try_recv() {
                drain.event();
                consume(Some(event))?;
            }
            drain.finish();
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
            let parked = Stamp::now();
            thread::park_timeout(idle_poll);
            if queue.is_empty() {
                // A quiet period (or a spurious wake): the next burst may linger.
                linger_armed = true;
            }
            drain.parked(parked, idle_poll);
            idle_poll = idle_poll_after(false, idle_poll);
        }
    }
}

#[cfg(test)]
#[path = "parser_tests.rs"]
mod parser_tests;
