//! Loss and risk warnings, checked on the supervisor's tick.
//!
//! [`Alerts::check`] compares one [`Readings`] with the previous one and returns
//! the [`Notice`]s that the change calls for. It only observes: nothing it
//! returns feeds back into capture, parsing, or sending.
//!
//! - **Loss** notices (Event IDs 10 to 16) say that data was lost. Each is
//!   written when its counters rose since the previous check, so a quiet
//!   interval writes nothing. One notice per cause and check.
//! - **Risk** notices (Event IDs 20 to 28) say that loss is likely or possible.
//!   A risk is raised when its condition starts and cleared (an information
//!   event, ID 2) when it ends. It is not repeated while it holds. Percentage
//!   thresholds clear [`CLEAR_MARGIN_PERCENT`] points below the level that
//!   raised them. The channel risk (ID 20) uses a peak that restarts every
//!   check, so it also clears only after [`CHANNEL_CLEAR_CHECKS`] checks in a
//!   row below that line. A load that bursts above the line at least every
//!   other check can still hold it raised, but a single quiet check does not
//!   clear it.
//!
//! The supervisor owns the clock and never calls [`Alerts::check`] once
//! shutdown has begun, so nothing here is written at stop. The IDs are in
//! `event_ids`; the guide tables are in `docs/guide/monitoring.md`.

use std::{sync::atomic::Ordering, time::Instant};

use super::parser::ParseCounters;
use crate::{etw::CaptureCounters, event_ids, sender::SEND_QUEUE_BLOCKS, sink::SinkStats};

/// Percent of `--capacity` that the deepest channel sample of an interval
/// reaches to raise the channel risk (ID 20).
pub(super) const CHANNEL_PERCENT: u64 = 80;
/// Percent of the applied ETW maximum that the allocated buffers reach to raise
/// the buffer risk (ID 21).
pub(super) const ETW_BUFFERS_PERCENT: u64 = 90;
/// Percent of the send queue (256 blocks) that its peak reaches to raise the
/// send queue risk (ID 22).
pub(super) const SEND_QUEUE_PERCENT: u64 = 80;
/// Percent of `--spool-max-mib` that the spool holds to raise the spool risk
/// (ID 25).
pub(super) const SPOOL_PERCENT: u64 = 80;
/// Segments above the free-space floor at which the spool volume is near the
/// floor: free space below `--spool-min-free-mib` plus this many segments.
pub(super) const FLOOR_RAISE_SEGMENTS: u64 = 2;
/// Segments above the floor at which a raised spool risk clears.
pub(super) const FLOOR_CLEAR_SEGMENTS: u64 = 3;
/// Consecutive checks below the clear line before the channel risk (ID 20)
/// clears. The channel peak restarts every check, so one quiet check proves
/// little.
pub(super) const CHANNEL_CLEAR_CHECKS: u8 = 2;
/// Points below the raising percentage at which a risk clears.
pub(super) const CLEAR_MARGIN_PERCENT: u64 = 10;

const MIB: u64 = 1024 * 1024;

/// One reading of the counters that the checks use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Readings {
    pub(super) etw_lost: u64,
    pub(super) etw_buffers_lost: u64,
    pub(super) ring_dropped: u64,
    /// Every pool exhaustion, regular and jumbo. This one counter decides the
    /// pool loss (ID 12); the split below is for the message only.
    pub(super) pool_exhausted: u64,
    pub(super) pool_exhausted_regular: u64,
    pub(super) pool_exhausted_jumbo: u64,
    pub(super) disconnected_dropped: u64,
    pub(super) queue_dropped: u64,
    pub(super) spool_dropped: u64,
    pub(super) spool_dropped_bytes: u64,
    pub(super) rejected: u64,
    pub(super) schema_rejected: u64,
    pub(super) callback_errors: u64,
    pub(super) receiver_disconnected: u64,
    pub(super) output_failed_events: u64,
    pub(super) etw_stats_errors: u64,
    /// The deepest event channel sample since the previous reading.
    pub(super) channel_peak: u64,
    pub(super) etw_buffers_allocated: u64,
    pub(super) etw_buffers_maximum: u64,
    /// `send_queue_depth_max`: the peak of the last 60 seconds, in blocks.
    pub(super) send_queue_peak: u64,
    pub(super) spool_bytes: u64,
    pub(super) spool_segments: u64,
    pub(super) connected: bool,
    pub(super) spool_available: bool,
    pub(super) etw_stats_valid: bool,
    /// Free bytes on the spool volume, or `None` with no spool or when the
    /// probe failed.
    pub(super) volume_free: Option<u64>,
}

impl Readings {
    /// Reads the counters. It takes (resets) the channel peak of the interval,
    /// so one supervisor calls it and no other reader may.
    pub(super) fn read(
        capture: &CaptureCounters,
        parse: &ParseCounters,
        sink: &SinkStats,
        volume_free: Option<u64>,
    ) -> Self {
        // The writer increments the total first, then the jumbo count. Reading the
        // jumbo count first keeps it from exceeding the total.
        let pool_exhausted_jumbo = capture.pool_exhausted_jumbo.load(Ordering::Relaxed);
        let pool_exhausted = capture.pool_exhausted.load(Ordering::Relaxed);
        Self {
            etw_lost: capture.etw_lost.load(Ordering::Relaxed),
            etw_buffers_lost: capture.etw_buffers_lost.load(Ordering::Relaxed),
            ring_dropped: capture.dropped.load(Ordering::Relaxed),
            pool_exhausted,
            pool_exhausted_regular: pool_exhausted.saturating_sub(pool_exhausted_jumbo),
            pool_exhausted_jumbo,
            disconnected_dropped: sink.disconnected_dropped.load(Ordering::Relaxed),
            queue_dropped: sink.queue_dropped.load(Ordering::Relaxed),
            spool_dropped: sink.spool_dropped.load(Ordering::Relaxed),
            spool_dropped_bytes: sink.spool_dropped_bytes.load(Ordering::Relaxed),
            rejected: parse.rejected.load(Ordering::Relaxed),
            schema_rejected: parse.schema_rejected.load(Ordering::Relaxed),
            callback_errors: capture.callback_errors.load(Ordering::Relaxed),
            receiver_disconnected: capture.receiver_disconnected.load(Ordering::Relaxed),
            output_failed_events: parse.output_failed_events.load(Ordering::Relaxed),
            etw_stats_errors: capture.stats_errors.load(Ordering::Relaxed),
            channel_peak: parse.queue_depth_interval_max.swap(0, Ordering::Relaxed),
            etw_buffers_allocated: capture.etw_buffers_allocated.load(Ordering::Relaxed),
            etw_buffers_maximum: capture.etw_buffers_maximum.load(Ordering::Relaxed),
            send_queue_peak: sink.send_queue_depth_max.load(Ordering::Relaxed),
            spool_bytes: sink.spool_bytes.load(Ordering::Relaxed),
            spool_segments: sink.spool_segments.load(Ordering::Relaxed),
            connected: sink.connected.load(Ordering::Relaxed),
            spool_available: sink.spool_available.load(Ordering::Relaxed),
            etw_stats_valid: capture.stats_valid.load(Ordering::Relaxed),
            volume_free,
        }
    }
}

/// The spool limits that the spool risk (ID 25) measures against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SpoolLimits {
    /// `--spool-max-mib`, in bytes.
    pub(super) max: u64,
    /// `--spool-min-free-mib`, in bytes.
    pub(super) min_free: u64,
    /// The size of one segment in bytes. The free-space floor adds one to the
    /// minimum.
    pub(super) segment: u64,
}

impl SpoolLimits {
    /// The free space below which the spool refuses blocks.
    const fn floor(&self) -> u64 {
        self.min_free.saturating_add(self.segment)
    }
}

/// The settings that the thresholds are a share of.
#[derive(Clone, Copy, Debug)]
pub(super) struct Limits {
    /// `--capacity`.
    pub(super) capacity: usize,
    /// `--jumbo-capacity`.
    pub(super) jumbo_capacity: u16,
    /// `None` with `--no-spool`.
    pub(super) spool: Option<SpoolLimits>,
}

/// An increase of a counter since the previous check, and its run total.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Rise {
    pub(super) by: u64,
    pub(super) total: u64,
}

impl Rise {
    const fn between(before: u64, now: u64) -> Self {
        Self {
            by: now.saturating_sub(before),
            total: now,
        }
    }

    /// The increase when the counter rose; for a log field that appears only
    /// for a counter that rose.
    pub(super) const fn by_if_any(self) -> Option<u64> {
        if self.by > 0 { Some(self.by) } else { None }
    }

    /// The run total when the counter rose.
    pub(super) const fn total_if_any(self) -> Option<u64> {
        if self.by > 0 { Some(self.total) } else { None }
    }

    const fn any(self) -> bool {
        self.by > 0
    }
}

/// Data was lost since the previous check. `seconds` is the time that the
/// check covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Loss {
    /// ID 10.
    Etw {
        events: Rise,
        buffers: Rise,
        seconds: u64,
    },
    /// ID 11.
    Channel {
        dropped: Rise,
        capacity: usize,
        seconds: u64,
    },
    /// ID 12.
    Pool {
        regular: Rise,
        jumbo: Rise,
        jumbo_capacity: u16,
        seconds: u64,
    },
    /// ID 13.
    Network {
        disconnected: Rise,
        queue: Rise,
        seconds: u64,
    },
    /// ID 14.
    Spool {
        frames: Rise,
        bytes: Rise,
        seconds: u64,
    },
    /// ID 15.
    Decode {
        rejected: Rise,
        schema: Rise,
        seconds: u64,
    },
    /// ID 16.
    Internal {
        callback_errors: Rise,
        receiver_disconnected: Rise,
        output_failed: Rise,
        seconds: u64,
    },
}

/// A condition that makes loss likely or possible. A risk is raised once when
/// it starts and cleared once when it ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Risk {
    /// ID 20.
    Channel { peak: u64, capacity: usize },
    /// ID 21.
    EtwBuffers { allocated: u64, maximum: u64 },
    /// ID 22.
    SendQueue { peak: u64, blocks: usize },
    /// ID 24: the receiver is down and no spool takes the frames.
    ReceiverDownUnspooled,
    /// ID 25.
    SpoolLimit {
        spool_bytes: u64,
        spool_max_bytes: u64,
        volume_free: Option<u64>,
        floor_bytes: u64,
    },
    /// ID 28.
    EtwStats { errors: Rise },
}

/// What a check returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Notice {
    Loss(Loss),
    Raised(Risk),
    Cleared(Risk),
}

/// Which risks are raised now.
#[derive(Clone, Copy, Debug, Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one independent flag for each risk"
)]
struct Raised {
    channel: bool,
    etw_buffers: bool,
    send_queue: bool,
    /// Checks in a row, while the channel risk is raised, that the peak was
    /// below the clear line.
    channel_low_checks: u8,
    receiver_down: bool,
    spool_limit: bool,
    etw_stats: bool,
}

/// The state of the loss and risk checks.
pub(super) struct Alerts {
    limits: Limits,
    before: Readings,
    checked_at: Instant,
    raised: Raised,
}

impl Alerts {
    /// Starts the checks. `baseline` is the reading at `now`, so the first
    /// check reports what changed since then. The supervisor passes the zero
    /// reading, so the first check covers everything since capture started.
    pub(super) fn new(limits: Limits, now: Instant, baseline: Readings) -> Self {
        Self {
            limits,
            before: baseline,
            checked_at: now,
            raised: Raised::default(),
        }
    }

    /// Compares `current` with the previous reading. Returns the loss notices
    /// first, then the risk notices, at most one of each kind.
    pub(super) fn check(&mut self, now: Instant, current: &Readings) -> Vec<Notice> {
        let seconds = whole_seconds(now.saturating_duration_since(self.checked_at));
        let mut notices = Vec::new();
        self.losses(current, seconds, &mut notices);
        self.risks(current, &mut notices);
        self.before = *current;
        self.checked_at = now;
        notices
    }

    fn losses(&self, now: &Readings, seconds: u64, notices: &mut Vec<Notice>) {
        let before = &self.before;
        let mut push = |loss: Loss, rose: bool| {
            if rose {
                notices.push(Notice::Loss(loss));
            }
        };
        let events = Rise::between(before.etw_lost, now.etw_lost);
        let buffers = Rise::between(before.etw_buffers_lost, now.etw_buffers_lost);
        push(
            Loss::Etw {
                events,
                buffers,
                seconds,
            },
            events.any() || buffers.any(),
        );
        let dropped = Rise::between(before.ring_dropped, now.ring_dropped);
        push(
            Loss::Channel {
                dropped,
                capacity: self.limits.capacity,
                seconds,
            },
            dropped.any(),
        );
        // Only the total decides. A read that fell between the writer's two
        // increments shifts the split by one, so the split is clamped to never
        // go backwards and is shown for the message only.
        let total = Rise::between(before.pool_exhausted, now.pool_exhausted);
        let regular = Rise {
            by: now
                .pool_exhausted_regular
                .saturating_sub(before.pool_exhausted_regular),
            total: now
                .pool_exhausted_regular
                .max(before.pool_exhausted_regular),
        };
        let jumbo = Rise {
            by: now
                .pool_exhausted_jumbo
                .saturating_sub(before.pool_exhausted_jumbo),
            total: now.pool_exhausted_jumbo.max(before.pool_exhausted_jumbo),
        };
        push(
            Loss::Pool {
                regular,
                jumbo,
                jumbo_capacity: self.limits.jumbo_capacity,
                seconds,
            },
            total.any(),
        );
        let disconnected = Rise::between(before.disconnected_dropped, now.disconnected_dropped);
        let queue = Rise::between(before.queue_dropped, now.queue_dropped);
        push(
            Loss::Network {
                disconnected,
                queue,
                seconds,
            },
            disconnected.any() || queue.any(),
        );
        let frames = Rise::between(before.spool_dropped, now.spool_dropped);
        let bytes = Rise::between(before.spool_dropped_bytes, now.spool_dropped_bytes);
        push(
            Loss::Spool {
                frames,
                bytes,
                seconds,
            },
            frames.any() || bytes.any(),
        );
        let rejected = Rise::between(before.rejected, now.rejected);
        let schema = Rise::between(before.schema_rejected, now.schema_rejected);
        push(
            Loss::Decode {
                rejected,
                schema,
                seconds,
            },
            rejected.any() || schema.any(),
        );
        let callback_errors = Rise::between(before.callback_errors, now.callback_errors);
        let receiver_disconnected =
            Rise::between(before.receiver_disconnected, now.receiver_disconnected);
        let output_failed = Rise::between(before.output_failed_events, now.output_failed_events);
        push(
            Loss::Internal {
                callback_errors,
                receiver_disconnected,
                output_failed,
                seconds,
            },
            callback_errors.any() || receiver_disconnected.any() || output_failed.any(),
        );
    }

    fn risks(&mut self, now: &Readings, notices: &mut Vec<Notice>) {
        let limits = self.limits;
        let was = self.raised;
        let mut next = was;
        let mut change = |raised: &mut bool, near: bool, risk: Risk| {
            if near && !*raised {
                notices.push(Notice::Raised(risk));
            } else if !near && *raised {
                notices.push(Notice::Cleared(risk));
            }
            *raised = near;
        };

        let channel_near = near(
            was.channel,
            now.channel_peak,
            widen(limits.capacity),
            CHANNEL_PERCENT,
        );
        // Raise at once; clear only after CHANNEL_CLEAR_CHECKS checks in a row
        // below the clear line.
        if channel_near || !was.channel {
            next.channel_low_checks = 0;
        } else {
            next.channel_low_checks = was.channel_low_checks.saturating_add(1);
        }
        let channel_holds = was.channel && next.channel_low_checks < CHANNEL_CLEAR_CHECKS;
        if !channel_near && !channel_holds {
            next.channel_low_checks = 0;
        }
        change(
            &mut next.channel,
            channel_near || channel_holds,
            Risk::Channel {
                peak: now.channel_peak,
                capacity: limits.capacity,
            },
        );
        change(
            &mut next.etw_buffers,
            near(
                was.etw_buffers,
                now.etw_buffers_allocated,
                now.etw_buffers_maximum,
                ETW_BUFFERS_PERCENT,
            ),
            Risk::EtwBuffers {
                allocated: now.etw_buffers_allocated,
                maximum: now.etw_buffers_maximum,
            },
        );
        change(
            &mut next.send_queue,
            near(
                was.send_queue,
                now.send_queue_peak,
                widen(SEND_QUEUE_BLOCKS),
                SEND_QUEUE_PERCENT,
            ),
            Risk::SendQueue {
                peak: now.send_queue_peak,
                blocks: SEND_QUEUE_BLOCKS,
            },
        );
        change(
            &mut next.receiver_down,
            !now.connected && !now.spool_available,
            Risk::ReceiverDownUnspooled,
        );
        if let Some(spool) = limits.spool {
            // A spool that never opened (no segment, not available) has no limit
            // to reach.
            let in_use = now.spool_available || now.spool_segments > 0;
            change(
                &mut next.spool_limit,
                in_use && spool_near_limit(was.spool_limit, now, &spool),
                Risk::SpoolLimit {
                    spool_bytes: now.spool_bytes,
                    spool_max_bytes: spool.max,
                    volume_free: now.volume_free,
                    floor_bytes: spool.floor(),
                },
            );
        }
        let errors = Rise::between(self.before.etw_stats_errors, now.etw_stats_errors);
        // Raised by a failed query in the interval and cleared by a good one.
        let failing = !now.etw_stats_valid && (was.etw_stats || errors.any());
        change(&mut next.etw_stats, failing, Risk::EtwStats { errors });
        self.raised = next;
    }
}

/// A count as `u64`.
fn widen(count: usize) -> u64 {
    u64::try_from(count).unwrap_or(u64::MAX)
}

/// True when `value` is at least `percent` of `whole`; once `raised`, true
/// until it falls [`CLEAR_MARGIN_PERCENT`] points below. `whole` of 0 is never
/// near.
fn near(raised: bool, value: u64, whole: u64, percent: u64) -> bool {
    let percent = if raised {
        percent.saturating_sub(CLEAR_MARGIN_PERCENT)
    } else {
        percent
    };
    whole > 0 && u128::from(value) * 100 >= u128::from(whole) * u128::from(percent)
}

/// True when the spool holds [`SPOOL_PERCENT`] of its cap, or the volume has
/// less than [`FLOOR_RAISE_SEGMENTS`] segments above the free-space floor
/// (clearing at [`FLOOR_CLEAR_SEGMENTS`]).
fn spool_near_limit(raised: bool, now: &Readings, spool: &SpoolLimits) -> bool {
    if near(raised, now.spool_bytes, spool.max, SPOOL_PERCENT) {
        return true;
    }
    let segments = if raised {
        FLOOR_CLEAR_SEGMENTS
    } else {
        FLOOR_RAISE_SEGMENTS
    };
    let line = spool
        .min_free
        .saturating_add(spool.segment.saturating_mul(segments));
    now.volume_free.is_some_and(|free| free < line)
}

/// Whole seconds, rounded to the nearest.
fn whole_seconds(elapsed: std::time::Duration) -> u64 {
    elapsed.as_secs() + u64::from(elapsed.subsec_millis() >= 500)
}

/// The percent that `value` is of `whole`, rounded down.
fn percent_of(value: u64, whole: u64) -> u64 {
    if whole == 0 {
        return 0;
    }
    u64::try_from(u128::from(value) * 100 / u128::from(whole)).unwrap_or(u64::MAX)
}

impl Notice {
    /// Writes the notice to the log, where the Event Log writer gives it its
    /// ID: a loss or a raised risk is a warning with its own ID, and a cleared
    /// risk is an information event (ID 2).
    pub(super) fn emit(&self) {
        match *self {
            Self::Loss(loss) => emit_loss(loss),
            Self::Raised(risk) => emit_raised(risk),
            Self::Cleared(risk) => emit_cleared(risk),
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one tracing call for each cause; the messages must be literals"
)]
fn emit_loss(loss: Loss) {
    match loss {
        Loss::Etw {
            events,
            buffers,
            seconds,
        } => tracing::warn!(
            target: event_ids::LOSS_ETW,
            etw_lost_increase = events.by_if_any(),
            etw_lost = events.total_if_any(),
            etw_buffers_lost_increase = buffers.by_if_any(),
            etw_buffers_lost = buffers.total_if_any(),
            seconds,
            "data loss in ETW: ETW dropped events or lost buffers before the service read \
             them, usually because its buffers filled or the consumer did not get enough CPU; \
             raise --etw-max-buffers (and --etw-buffer-kib), and check for CPU starvation, \
             see docs/guide/tuning.md"
        ),
        Loss::Channel {
            dropped,
            capacity,
            seconds,
        } => tracing::warn!(
            target: event_ids::LOSS_CHANNEL,
            ring_dropped_increase = dropped.by,
            ring_dropped = dropped.total,
            capacity,
            seconds,
            "data loss at capacity: the event channel was full, so events were dropped; the \
             parser did not keep up with the capture rate; raise --capacity, see \
             docs/guide/tuning.md"
        ),
        Loss::Pool {
            regular,
            jumbo,
            jumbo_capacity,
            seconds,
        } => tracing::warn!(
            target: event_ids::LOSS_POOL,
            pool_exhausted_regular_increase = regular.by_if_any(),
            pool_exhausted_regular = regular.total_if_any(),
            pool_exhausted_jumbo_increase = jumbo.by_if_any(),
            pool_exhausted_jumbo = jumbo.total_if_any(),
            jumbo_capacity,
            seconds,
            "data loss at capacity: no pool slot was free, so events were dropped; regular \
             slots follow --capacity, and jumbo slots (payloads over 2 KiB) follow \
             --jumbo-capacity; raise the one whose counter rose, see docs/guide/tuning.md"
        ),
        Loss::Network {
            disconnected,
            queue,
            seconds,
        } => tracing::warn!(
            target: event_ids::LOSS_NETWORK,
            disconnected_dropped_increase = disconnected.by_if_any(),
            disconnected_dropped = disconnected.total_if_any(),
            queue_dropped_increase = queue.by_if_any(),
            queue_dropped = queue.total_if_any(),
            seconds,
            "data loss on the network path: disconnected_dropped counts frames lost when the \
             receiver connection failed or was down and no spool took them, and queue_dropped \
             counts frames discarded because the send queue was full; check the receiver and \
             the network, and the spool, see docs/guide/troubleshooting.md"
        ),
        Loss::Spool {
            frames,
            bytes,
            seconds,
        } => tracing::warn!(
            target: event_ids::LOSS_SPOOL,
            spool_dropped_increase = frames.by_if_any(),
            spool_dropped = frames.total_if_any(),
            spool_dropped_bytes_increase = bytes.by_if_any(),
            spool_dropped_bytes = bytes.total_if_any(),
            seconds,
            "data loss in the spool: it removed, skipped, or refused frames unsent (size cap, \
             free-space floor, unreadable data, or an I/O error); free disk space, or raise \
             --spool-max-mib, and see docs/guide/troubleshooting.md and docs/guide/tuning.md"
        ),
        Loss::Decode {
            rejected,
            schema,
            seconds,
        } => tracing::warn!(
            target: event_ids::LOSS_DECODE,
            rejected_increase = rejected.by_if_any(),
            rejected = rejected.total_if_any(),
            schema_rejected_increase = schema.by_if_any(),
            schema_rejected = schema.total_if_any(),
            seconds,
            "data loss in decoding: events were received but rejected, so no frame was \
             encoded for them; the rejected-event and schema warnings give the cause, see \
             docs/guide/troubleshooting.md"
        ),
        Loss::Internal {
            callback_errors,
            receiver_disconnected,
            output_failed,
            seconds,
        } => tracing::warn!(
            target: event_ids::LOSS_INTERNAL,
            callback_errors_increase = callback_errors.by_if_any(),
            callback_errors = callback_errors.total_if_any(),
            receiver_disconnected_increase = receiver_disconnected.by_if_any(),
            receiver_disconnected = receiver_disconnected.total_if_any(),
            output_failed_events_increase = output_failed.by_if_any(),
            output_failed_events = output_failed.total_if_any(),
            seconds,
            "data loss inside the service: the ETW callback could not copy or hand off \
             events (callback_errors), the internal channel to the parser closed \
             (receiver_disconnected), or the sender stopped (output_failed_events); report it \
             with the steps that led to it, see docs/guide/troubleshooting.md"
        ),
    }
}

fn emit_raised(risk: Risk) {
    match risk {
        Risk::Channel { peak, capacity } => tracing::warn!(
            target: event_ids::RISK_CHANNEL,
            channel_peak = peak,
            capacity,
            percent = percent_of(peak, widen(capacity)),
            "the event channel is near full: its deepest sample in the last interval reached \
             {CHANNEL_PERCENT} percent of --capacity, and events are dropped when it is full; raise \
             --capacity, see docs/guide/tuning.md"
        ),
        Risk::EtwBuffers { allocated, maximum } => tracing::warn!(
            target: event_ids::RISK_ETW_BUFFERS,
            etw_buffers_allocated = allocated,
            etw_buffers_maximum = maximum,
            percent = percent_of(allocated, maximum),
            "the ETW buffers are near the maximum: {ETW_BUFFERS_PERCENT} percent or more of the \
             applied maximum is allocated, and ETW drops events when it has no free buffer; \
             raise --etw-max-buffers, see docs/guide/tuning.md"
        ),
        Risk::SendQueue { peak, blocks } => tracing::warn!(
            target: event_ids::RISK_SEND_QUEUE,
            send_queue_depth_max = peak,
            send_queue_blocks = blocks,
            percent = percent_of(peak, widen(blocks)),
            "the send queue is near full: its peak over the last 60 seconds reached \
             {SEND_QUEUE_PERCENT} percent of its blocks, and frames are dropped when it is full; a \
             slow spool disk or a router thread without CPU time causes it, see \
             docs/guide/monitoring.md and docs/guide/troubleshooting.md"
        ),
        Risk::ReceiverDownUnspooled => tracing::warn!(
            target: event_ids::RISK_RECEIVER_DOWN,
            receiver_connected = false,
            spool_available = false,
            "the dnstap receiver is down and no spool takes frames; frames are being dropped \
             and counted (disconnected_dropped) until it reconnects; with --no-spool this \
             loses frames in every outage, see docs/guide/troubleshooting.md"
        ),
        Risk::SpoolLimit {
            spool_bytes,
            spool_max_bytes,
            volume_free,
            floor_bytes,
        } => tracing::warn!(
            target: event_ids::RISK_SPOOL_LIMIT,
            spool_mib = spool_bytes / MIB,
            spool_max_mib = spool_max_bytes / MIB,
            free_mib = volume_free.map(|free| free / MIB),
            needed_mib = floor_bytes.div_ceil(MIB),
            "the spool is near its limit: it holds {SPOOL_PERCENT} percent or more of \
             --spool-max-mib, or the volume is within {FLOOR_RAISE_SEGMENTS} segments of the \
             free-space floor; at the limit the spool deletes its oldest segments unsent or \
             refuses blocks; free disk space or raise --spool-max-mib, see \
             docs/guide/tuning.md"
        ),
        Risk::EtwStats { errors } => tracing::warn!(
            target: event_ids::RISK_ETW_STATS,
            etw_stats_errors_increase = errors.by,
            etw_stats_errors = errors.total,
            "the ETW statistics query is failing, so ETW loss (etw_lost, etw_buffers_lost) is \
             not visible until it succeeds; check that the session is running, see \
             docs/guide/troubleshooting.md"
        ),
    }
}

fn emit_cleared(risk: Risk) {
    match risk {
        Risk::Channel { peak, capacity } => tracing::info!(
            channel_peak = peak,
            capacity,
            percent = percent_of(peak, widen(capacity)),
            "the event channel is no longer near full"
        ),
        Risk::EtwBuffers { allocated, maximum } => tracing::info!(
            etw_buffers_allocated = allocated,
            etw_buffers_maximum = maximum,
            percent = percent_of(allocated, maximum),
            "the ETW buffers are no longer near the maximum"
        ),
        Risk::SendQueue { peak, blocks } => tracing::info!(
            send_queue_depth_max = peak,
            send_queue_blocks = blocks,
            percent = percent_of(peak, widen(blocks)),
            "the send queue is no longer near full"
        ),
        Risk::ReceiverDownUnspooled => tracing::info!(
            "frames are no longer dropped for a receiver outage: the receiver is connected, \
             or the spool takes frames again"
        ),
        Risk::SpoolLimit {
            spool_bytes,
            spool_max_bytes,
            volume_free,
            floor_bytes,
        } => tracing::info!(
            spool_mib = spool_bytes / MIB,
            spool_max_mib = spool_max_bytes / MIB,
            free_mib = volume_free.map(|free| free / MIB),
            needed_mib = floor_bytes.div_ceil(MIB),
            "the spool is no longer near its limit"
        ),
        Risk::EtwStats { errors } => tracing::info!(
            etw_stats_errors = errors.total,
            "the ETW statistics query works again"
        ),
    }
}

#[cfg(test)]
#[path = "alerts_tests.rs"]
mod alerts_tests;
