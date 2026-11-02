//! Publishes capture health as Windows performance counters (`PerfLib` V2).
//!
//! The counter set is defined by `packaging/perfcounters/etw-dnstap.man`. An
//! administrator registers that manifest with `lodctr`; this module provides the
//! values, in two ways:
//!
//! - By reference: the raw totals and peaks (see [`Source::Reference`]) are the
//!   existing `AtomicU64` counters. [`PerfPublisher::start`] hands their
//!   addresses to `PerfLib` once, and `PerfLib` reads them when a consumer
//!   collects. The ETW callback and the parser loop do no extra work.
//! - By value: the derived values (see [`snapshot`]) are set from the
//!   supervisor thread once per publication tick.
//!
//! Failure policy: when registration fails, [`PerfPublisher::start_or_warn`]
//! logs one warning and capture continues without counters. A failed value
//! update is ignored.

use std::{
    ffi::c_void,
    ptr::null_mut,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Result, bail};
use windows_sys::{
    Win32::{
        Foundation::{ERROR_SUCCESS, GetLastError, HANDLE},
        System::Performance::{
            PERF_ATTRIB_BY_REFERENCE, PERF_ATTRIB_NO_DISPLAYABLE, PERF_COUNTER_INFO,
            PERF_COUNTERSET_INFO, PERF_COUNTERSET_INSTANCE, PERF_COUNTERSET_MULTI_INSTANCES,
            PERF_DETAIL_ADVANCED, PERF_DETAIL_NOVICE, PERF_PROVIDER_CONTEXT, PerfCreateInstance,
            PerfDeleteInstance, PerfSetCounterRefValue, PerfSetCounterSetInfo,
            PerfSetULongLongCounterValue, PerfStartProviderEx, PerfStopProvider,
        },
    },
    core::GUID,
};

use crate::{etw::CaptureCounters, pipeline::ParseCounters, sink::SinkStats};

// PerfLib reads the referenced counters as 64-bit variables from its own
// thread, so the module needs 64-bit atomics that are 8-byte aligned.
#[cfg(not(target_pointer_width = "64"))]
compile_error!("performance counters read AtomicU64 values by reference and need a 64-bit target");
const _: () = assert!(size_of::<AtomicU64>() == 8);
const _: () = assert!(align_of::<AtomicU64>() == 8);

// Counter types from winperf.h. windows-sys omits these composite values.
/// A 64-bit total that the consumer turns into a rate per second.
const PERF_COUNTER_BULK_COUNT: u32 = 0x1041_0500;
/// A 64-bit value shown as is.
const PERF_COUNTER_LARGE_RAWCOUNT: u32 = 0x0001_0100;
/// A 64-bit numerator shown as a percentage of the base counter that follows.
const PERF_LARGE_RAW_FRACTION: u32 = 0x2002_0500;
/// The 64-bit denominator of the fraction counter before it.
const PERF_LARGE_RAW_BASE: u32 = 0x4003_0500;

/// `providerGuid` in the manifest.
const PROVIDER_ID: u128 = 0xa7f0_b1f4_c87d_4267_979f_594a_4ab2_145d;
/// The counter set `guid` in the manifest.
const COUNTER_SET_ID: u128 = 0xb02e_dfa2_b961_4e06_89d2_56c9_5ecb_336c;

/// Bytes of one counter value in the instance block.
const VALUE_SIZE: usize = size_of::<u64>();

/// The shared counters that the publisher reads, and the channel capacity.
/// The publisher holds its own references, so the counters outlive every read
/// that `PerfLib` makes.
#[derive(Clone)]
pub(crate) struct CounterSources {
    pub(crate) capture: Arc<CaptureCounters>,
    pub(crate) parse: Arc<ParseCounters>,
    pub(crate) sink: Arc<SinkStats>,
    pub(crate) capacity: usize,
}

/// The derived values that each publication tick sets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CounterValues {
    pub(crate) channel_depth: u64,
    pub(crate) channel_capacity: u64,
    pub(crate) etw_buffers_allocated: u64,
    pub(crate) etw_buffers_in_use: u64,
    pub(crate) etw_buffers_maximum: u64,
    pub(crate) receiver_connected: u64,
}

/// Reads the counters into the values to publish. Loads are relaxed and not
/// taken at one instant, so related values can disagree by a few events.
///
/// The channel depth follows the intake ledger: every received event is
/// dropped, a callback error, dequeued, or still queued. Relaxed loads give no
/// ordering between the counters, so the difference can be briefly too low or
/// too high. The clamp to `0..=capacity` keeps the result in range.
pub(crate) fn snapshot(
    capture: &CaptureCounters,
    parse: &ParseCounters,
    sink: &SinkStats,
    capacity: usize,
) -> CounterValues {
    let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
    let capacity = u64::try_from(capacity).unwrap_or(u64::MAX);
    let received = load(&capture.received);
    let channel_depth = received
        .saturating_sub(load(&capture.intake_dropped))
        .saturating_sub(load(&capture.callback_errors))
        .saturating_sub(load(&parse.dequeued))
        .min(capacity);
    let etw_buffers_allocated = load(&capture.etw_buffers_allocated);
    CounterValues {
        channel_depth,
        channel_capacity: capacity,
        etw_buffers_allocated,
        etw_buffers_in_use: etw_buffers_allocated.saturating_sub(load(&capture.etw_buffers_free)),
        etw_buffers_maximum: load(&capture.etw_buffers_maximum),
        receiver_connected: u64::from(sink.connected.load(Ordering::Relaxed)),
    }
}

/// Where a counter's value comes from.
enum Source {
    /// `PerfLib` reads this atomic itself; the address is set once at start.
    Reference(fn(&CounterSources) -> &AtomicU64),
    /// The supervisor sets this value on each publication tick.
    Value(fn(&CounterValues) -> u64),
}

/// One counter of the set, in manifest order.
struct CounterDef {
    /// The `id` attribute in the manifest. Frozen once released.
    id: u32,
    counter_type: u32,
    detail: u32,
    source: Source,
}

impl CounterDef {
    /// True when the manifest marks the counter `reference`.
    const fn by_reference(&self) -> bool {
        matches!(self.source, Source::Reference(_))
    }

    /// The `Attrib` of the counter: by-reference counters are read through a
    /// pointer, and base counters are hidden from consumers.
    const fn attributes(&self) -> u64 {
        let mut attributes = 0;
        if self.by_reference() {
            attributes |= PERF_ATTRIB_BY_REFERENCE;
        }
        if self.counter_type == PERF_LARGE_RAW_BASE {
            attributes |= PERF_ATTRIB_NO_DISPLAYABLE;
        }
        attributes
    }
}

const COUNTER_COUNT: usize = 27;

static COUNTERS: [CounterDef; COUNTER_COUNT] = [
    // Throughput rates.
    by_ref(1, PERF_COUNTER_BULK_COUNT, PERF_DETAIL_NOVICE, |s| {
        &s.capture.received
    }),
    by_ref(2, PERF_COUNTER_BULK_COUNT, PERF_DETAIL_NOVICE, |s| {
        &s.parse.encoded
    }),
    by_ref(3, PERF_COUNTER_BULK_COUNT, PERF_DETAIL_NOVICE, |s| {
        &s.capture.intake_dropped
    }),
    // Totals since the service started.
    by_ref(10, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |s| {
        &s.capture.received
    }),
    by_ref(11, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |s| {
        &s.parse.encoded
    }),
    by_ref(12, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |s| {
        &s.capture.intake_dropped
    }),
    by_ref(13, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |s| {
        &s.capture.dropped
    }),
    by_ref(14, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_ADVANCED, |s| {
        &s.capture.pool_exhausted
    }),
    by_ref(15, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_ADVANCED, |s| {
        &s.capture.pool_exhausted_jumbo
    }),
    by_ref(16, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |s| {
        &s.capture.etw_lost
    }),
    by_ref(17, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |s| {
        &s.capture.etw_buffers_lost
    }),
    by_ref(18, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |s| {
        &s.sink.disconnected_dropped
    }),
    by_ref(19, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_ADVANCED, |s| {
        &s.parse.rejected
    }),
    by_ref(20, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_ADVANCED, |s| {
        &s.capture.callback_errors
    }),
    by_ref(21, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |s| {
        &s.sink.disconnects
    }),
    // Channel gauges.
    by_value(30, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |v| {
        v.channel_depth
    }),
    by_value(31, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |v| {
        v.channel_capacity
    }),
    by_ref(32, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |s| {
        &s.parse.queue_depth_max
    }),
    by_value(33, PERF_LARGE_RAW_FRACTION, PERF_DETAIL_NOVICE, |v| {
        v.channel_depth
    }),
    by_value(34, PERF_LARGE_RAW_BASE, PERF_DETAIL_NOVICE, |v| {
        v.channel_capacity
    }),
    // ETW buffer gauges.
    by_value(40, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |v| {
        v.etw_buffers_allocated
    }),
    by_value(41, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |v| {
        v.etw_buffers_in_use
    }),
    by_value(42, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |v| {
        v.etw_buffers_maximum
    }),
    by_value(43, PERF_LARGE_RAW_FRACTION, PERF_DETAIL_NOVICE, |v| {
        v.etw_buffers_in_use
    }),
    by_value(44, PERF_LARGE_RAW_BASE, PERF_DETAIL_NOVICE, |v| {
        v.etw_buffers_maximum
    }),
    // Receiver connection.
    by_value(50, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_NOVICE, |v| {
        v.receiver_connected
    }),
    by_ref(51, PERF_COUNTER_LARGE_RAWCOUNT, PERF_DETAIL_ADVANCED, |s| {
        &s.sink.reconnects
    }),
];

const fn by_ref(
    id: u32,
    counter_type: u32,
    detail: u32,
    atomic: fn(&CounterSources) -> &AtomicU64,
) -> CounterDef {
    CounterDef {
        id,
        counter_type,
        detail,
        source: Source::Reference(atomic),
    }
}

const fn by_value(
    id: u32,
    counter_type: u32,
    detail: u32,
    value: fn(&CounterValues) -> u64,
) -> CounterDef {
    CounterDef {
        id,
        counter_type,
        detail,
        source: Source::Value(value),
    }
}

/// `size` as the `u32` that the native structures use. Fails at compile time
/// in a constant, and the callers pass only fixed structure sizes.
#[expect(
    clippy::cast_possible_truncation,
    reason = "The assertion rejects any size above u32::MAX first."
)]
const fn size_u32(size: usize) -> u32 {
    assert!(size <= u32::MAX as usize);
    size as u32
}

/// The argument of `PerfSetCounterSetInfo`: the counter set header followed by
/// one entry for each counter.
#[repr(C)]
struct Template {
    set: PERF_COUNTERSET_INFO,
    counters: [PERF_COUNTER_INFO; COUNTER_COUNT],
}

impl Template {
    fn new() -> Self {
        let mut counters = [PERF_COUNTER_INFO::default(); COUNTER_COUNT];
        for (info, def) in counters.iter_mut().zip(&COUNTERS) {
            *info = PERF_COUNTER_INFO {
                CounterId: def.id,
                Type: def.counter_type,
                Attrib: def.attributes(),
                Size: size_u32(VALUE_SIZE),
                DetailLevel: def.detail,
                Scale: 0,
                Offset: 0,
            };
        }
        Self {
            set: PERF_COUNTERSET_INFO {
                CounterSetGuid: GUID::from_u128(COUNTER_SET_ID),
                ProviderGuid: GUID::from_u128(PROVIDER_ID),
                NumCounters: size_u32(COUNTER_COUNT),
                InstanceType: PERF_COUNTERSET_MULTI_INSTANCES,
            },
            counters,
        }
    }
}

/// Receives each snapshot. [`PerfPublisher`] is the production implementation;
/// tests supply fakes. Dropping the sink ends publication.
pub(crate) trait CounterSink {
    fn publish(&self, values: &CounterValues);
}

/// One `PerfLib` provider with one instance of the counter set.
///
/// The raw handles make the type neither `Send` nor `Sync`: it lives on the
/// supervisor thread that creates it.
///
/// Drop order matters because `PerfLib` reads the referenced counters until the
/// instance is gone. The `Drop` impl deletes the instance and stops the
/// provider; only afterwards does Rust drop the fields, and `sources` is the
/// last field so the counters are released after those calls.
pub(crate) struct PerfPublisher {
    provider: HANDLE,
    /// Null until `PerfCreateInstance` succeeds.
    instance: *mut PERF_COUNTERSET_INSTANCE,
    /// Keeps the referenced counters alive. Keep this field last.
    sources: CounterSources,
}

impl PerfPublisher {
    /// Registers the provider, creates the instance `instance_name`, points the
    /// by-reference counters at their atomics, and sets the first values.
    ///
    /// # Errors
    /// Returns the failing call and its Win32 status. On error the instance and
    /// the provider are already released.
    pub(crate) fn start(instance_name: &str, sources: CounterSources) -> Result<Self> {
        let context = PERF_PROVIDER_CONTEXT {
            ContextSize: size_u32(size_of::<PERF_PROVIDER_CONTEXT>()),
            ..PERF_PROVIDER_CONTEXT::default()
        };
        let guid = GUID::from_u128(PROVIDER_ID);
        let mut provider: HANDLE = null_mut();
        // SAFETY: The GUID and the context outlive this synchronous call, the
        // context has no callbacks or allocators, and `provider` is a valid
        // output location.
        let start_status =
            unsafe { PerfStartProviderEx(&raw const guid, &raw const context, &raw mut provider) };
        check(start_status, "PerfStartProviderEx")?;
        // From here the value stops the provider when an error drops it.
        let mut publisher = Self {
            provider,
            instance: null_mut(),
            sources,
        };

        let mut template = Template::new();
        // SAFETY: The handle came from PerfStartProviderEx. The pointer covers
        // the whole `#[repr(C)]` template, whose size is passed. PerfLib may
        // write the template (it computes the instance offsets); that is sound
        // because the template is a mutable local that nothing else borrows.
        let set_info_status = unsafe {
            PerfSetCounterSetInfo(
                publisher.provider,
                (&raw mut template).cast::<PERF_COUNTERSET_INFO>(),
                size_u32(size_of::<Template>()),
            )
        };
        check(set_info_status, "PerfSetCounterSetInfo")?;

        let name: Vec<u16> = instance_name
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let counter_set = GUID::from_u128(COUNTER_SET_ID);
        // SAFETY: The handle is valid, the GUID is the one just registered, and
        // `name` is NUL terminated and outlives the call. PerfLib copies it.
        let instance = unsafe {
            PerfCreateInstance(publisher.provider, &raw const counter_set, name.as_ptr(), 0)
        };
        if instance.is_null() {
            // SAFETY: Reads the calling thread's last error; nothing ran since the call.
            let create_error = unsafe { GetLastError() };
            check(create_error, "PerfCreateInstance")?;
            bail!("PerfCreateInstance returned no instance");
        }
        publisher.instance = instance;

        publisher.set_references()?;
        publisher.publish(&snapshot(
            &publisher.sources.capture,
            &publisher.sources.parse,
            &publisher.sources.sink,
            publisher.sources.capacity,
        ));
        Ok(publisher)
    }

    /// Points each by-reference counter at its atomic, once.
    fn set_references(&self) -> Result<()> {
        for def in &COUNTERS {
            let Source::Reference(atomic) = def.source else {
                continue;
            };
            let address = atomic(&self.sources).as_ptr().cast_const().cast::<c_void>();
            // SAFETY: The handle and instance are live. The address is an
            // 8-byte aligned `AtomicU64` inside an `Arc` that `sources` keeps
            // alive until after PerfDeleteInstance, and the `Arc` contents never
            // move. Microsoft Learn (Interlocked Variable Access): "Simple reads
            // and writes to properly aligned 64-bit variables are atomic on
            // 64-bit Windows." PerfLib reads the value on its own thread while
            // Rust writes it only with atomic stores and read-modify-writes, so
            // a read sees an old or a new value, never a torn one.
            let status =
                unsafe { PerfSetCounterRefValue(self.provider, self.instance, def.id, address) };
            check(
                status,
                &format!("PerfSetCounterRefValue (counter {})", def.id),
            )?;
        }
        Ok(())
    }

    /// Starts the publisher, or logs one warning and returns `None`. Capture
    /// never depends on the counters.
    pub(crate) fn start_or_warn(
        instance_name: &str,
        sources: CounterSources,
    ) -> Option<Box<dyn CounterSink>> {
        match Self::start(instance_name, sources) {
            Ok(publisher) => Some(Box::new(publisher)),
            Err(error) => {
                tracing::warn!(
                    %error,
                    "performance counters are unavailable; capture continues without \
                     performance counters"
                );
                None
            }
        }
    }

    /// Sets every by-value counter. A failed update is ignored: the next tick
    /// sets the value again, and `PerfLib` keeps the last good one.
    pub(crate) fn publish(&self, values: &CounterValues) {
        for def in &COUNTERS {
            let Source::Value(value) = def.source else {
                continue;
            };
            // SAFETY: `provider` and `instance` are live until Drop, and
            // PerfLib validates the counter ID against the registered set.
            let _status = unsafe {
                PerfSetULongLongCounterValue(self.provider, self.instance, def.id, value(values))
            };
        }
    }
}

impl CounterSink for PerfPublisher {
    fn publish(&self, values: &CounterValues) {
        PerfPublisher::publish(self, values);
    }
}

impl Drop for PerfPublisher {
    /// Deletes the instance, then stops the provider. Rust drops `sources`
    /// after this function returns, so no read of a referenced counter can
    /// outlive the counters.
    fn drop(&mut self) {
        if !self.instance.is_null() {
            // SAFETY: The instance came from PerfCreateInstance on this provider
            // and is deleted once; nothing uses it afterwards.
            let status = unsafe { PerfDeleteInstance(self.provider, self.instance) };
            if status != ERROR_SUCCESS {
                // Stopping the provider below removes every instance anyway.
                tracing::debug!(code = status, "PerfDeleteInstance failed");
            }
        }
        // SAFETY: The handle came from PerfStartProviderEx and is stopped once.
        let status = unsafe { PerfStopProvider(self.provider) };
        if status != ERROR_SUCCESS {
            tracing::debug!(code = status, "PerfStopProvider failed");
            // A stopped provider can no longer be read, so a failed delete does
            // not matter. A failed stop leaves the provider live, and PerfLib
            // may still read the addresses. Leak one reference to each counter
            // group so a late read stays valid.
            std::mem::forget(self.sources.clone());
        }
    }
}

fn check(status: u32, call: &str) -> Result<()> {
    if status != ERROR_SUCCESS {
        bail!(
            "{call} failed: {} (code {status})",
            std::io::Error::from_raw_os_error(status.cast_signed())
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::mem::offset_of;

    use windows_sys::Win32::System::Performance::{
        PERF_COUNTER_BASE, PERF_COUNTER_FRACTION, PERF_COUNTER_RATE, PERF_DELTA_COUNTER,
        PERF_DISPLAY_NO_SUFFIX, PERF_DISPLAY_NOSHOW, PERF_DISPLAY_PER_SEC, PERF_DISPLAY_PERCENT,
        PERF_NUMBER_DECIMAL, PERF_SIZE_LARGE, PERF_TIMER_TICK, PERF_TYPE_COUNTER, PERF_TYPE_NUMBER,
    };

    use super::*;

    const MANIFEST: &str = include_str!("../packaging/perfcounters/etw-dnstap.man");

    /// The counters that `PerfLib` reads through a pointer.
    const REFERENCE_IDS: [u32; 17] = [
        1, 2, 3, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 32, 51,
    ];
    /// The counters that the supervisor sets on each tick.
    const VALUE_IDS: [u32; 10] = [30, 31, 33, 34, 40, 41, 42, 43, 44, 50];

    #[test]
    fn counter_types_match_their_winperf_definitions() {
        assert_eq!(
            PERF_COUNTER_BULK_COUNT,
            PERF_SIZE_LARGE
                | PERF_TYPE_COUNTER
                | PERF_COUNTER_RATE
                | PERF_TIMER_TICK
                | PERF_DELTA_COUNTER
                | PERF_DISPLAY_PER_SEC
        );
        assert_eq!(
            PERF_COUNTER_LARGE_RAWCOUNT,
            PERF_SIZE_LARGE | PERF_TYPE_NUMBER | PERF_NUMBER_DECIMAL | PERF_DISPLAY_NO_SUFFIX
        );
        assert_eq!(
            PERF_LARGE_RAW_FRACTION,
            PERF_SIZE_LARGE | PERF_TYPE_COUNTER | PERF_COUNTER_FRACTION | PERF_DISPLAY_PERCENT
        );
        assert_eq!(
            PERF_LARGE_RAW_BASE,
            PERF_SIZE_LARGE | PERF_TYPE_COUNTER | PERF_COUNTER_BASE | PERF_DISPLAY_NOSHOW
        );
    }

    fn counters() -> (CaptureCounters, ParseCounters, SinkStats) {
        (
            CaptureCounters::default(),
            ParseCounters::default(),
            SinkStats::default(),
        )
    }

    fn sources(capacity: usize) -> CounterSources {
        CounterSources {
            capture: Arc::new(CaptureCounters::default()),
            parse: Arc::new(ParseCounters::default()),
            sink: Arc::new(SinkStats::default()),
            capacity,
        }
    }

    fn depth(received: u64, dropped: u64, errors: u64, dequeued: u64, capacity: usize) -> u64 {
        let (capture, parse, sink) = counters();
        capture.received.store(received, Ordering::Relaxed);
        capture.intake_dropped.store(dropped, Ordering::Relaxed);
        capture.callback_errors.store(errors, Ordering::Relaxed);
        parse.dequeued.store(dequeued, Ordering::Relaxed);
        snapshot(&capture, &parse, &sink, capacity).channel_depth
    }

    #[test]
    fn depth_follows_the_intake_ledger() {
        assert_eq!(depth(0, 0, 0, 0, 1024), 0);
        assert_eq!(depth(100, 0, 0, 0, 1024), 100);
        assert_eq!(depth(100, 10, 5, 60, 1024), 25);
        assert_eq!(depth(100, 40, 10, 50, 1024), 0);
    }

    #[test]
    fn depth_is_clamped_to_zero_and_capacity() {
        // Loads taken at different moments can leave the ledger negative.
        assert_eq!(depth(10, 5, 5, 5, 1024), 0);
        assert_eq!(depth(0, u64::MAX, u64::MAX, u64::MAX, 1024), 0);
        // Or larger than the channel can hold.
        assert_eq!(depth(5000, 0, 0, 0, 1024), 1024);
        assert_eq!(depth(1024, 0, 0, 0, 1024), 1024);
        assert_eq!(depth(u64::MAX, 0, 0, 0, 1024), 1024);
    }

    #[test]
    fn capacity_is_published_as_the_channel_base() {
        let (capture, parse, sink) = counters();
        let values = snapshot(&capture, &parse, &sink, 65_536);
        assert_eq!(values.channel_capacity, 65_536);
    }

    #[test]
    fn etw_buffers_in_use_saturates() {
        let (capture, parse, sink) = counters();
        capture.etw_buffers_allocated.store(16, Ordering::Relaxed);
        capture.etw_buffers_free.store(4, Ordering::Relaxed);
        capture.etw_buffers_maximum.store(12, Ordering::Relaxed);
        let values = snapshot(&capture, &parse, &sink, 1024);
        assert_eq!(values.etw_buffers_allocated, 16);
        assert_eq!(values.etw_buffers_in_use, 12);
        assert_eq!(values.etw_buffers_maximum, 12);

        capture.etw_buffers_free.store(20, Ordering::Relaxed);
        assert_eq!(
            snapshot(&capture, &parse, &sink, 1024).etw_buffers_in_use,
            0
        );
        // Before the first successful query every ETW gauge reads zero.
        let (capture, parse, sink) = counters();
        let values = snapshot(&capture, &parse, &sink, 1024);
        assert_eq!(
            (
                values.etw_buffers_allocated,
                values.etw_buffers_in_use,
                values.etw_buffers_maximum
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn receiver_connected_is_zero_or_one() {
        let (capture, parse, sink) = counters();
        assert_eq!(
            snapshot(&capture, &parse, &sink, 1024).receiver_connected,
            0
        );
        sink.connected.store(true, Ordering::Relaxed);
        assert_eq!(
            snapshot(&capture, &parse, &sink, 1024).receiver_connected,
            1
        );
    }

    #[test]
    fn the_table_selects_the_documented_value_sources() {
        let values = CounterValues {
            channel_depth: 30,
            channel_capacity: 31,
            etw_buffers_allocated: 40,
            etw_buffers_in_use: 41,
            etw_buffers_maximum: 42,
            receiver_connected: 50,
        };
        let read = |id: u32| {
            let def = COUNTERS
                .iter()
                .find(|def| def.id == id)
                .expect("counter is in the table");
            let Source::Value(value) = def.source else {
                panic!("counter {id} is not by value");
            };
            value(&values)
        };
        assert_eq!([30, 31, 40, 41, 42, 50].map(read), [30, 31, 40, 41, 42, 50]);
        // A fraction is the gauge over its base.
        assert_eq!((read(33), read(34)), (30, 31));
        assert_eq!((read(43), read(44)), (41, 42));
    }

    #[test]
    fn reference_counters_point_at_the_documented_atomics() {
        let sources = sources(1024);
        let address = |id: u32| {
            let def = COUNTERS
                .iter()
                .find(|def| def.id == id)
                .expect("counter is in the table");
            let Source::Reference(atomic) = def.source else {
                panic!("counter {id} is not by reference");
            };
            atomic(&sources).as_ptr()
        };
        let (capture, parse, sink) = (&sources.capture, &sources.parse, &sources.sink);
        let expected = [
            (1, capture.received.as_ptr()),
            (10, capture.received.as_ptr()),
            (2, parse.encoded.as_ptr()),
            (11, parse.encoded.as_ptr()),
            (3, capture.intake_dropped.as_ptr()),
            (12, capture.intake_dropped.as_ptr()),
            (13, capture.dropped.as_ptr()),
            (14, capture.pool_exhausted.as_ptr()),
            (15, capture.pool_exhausted_jumbo.as_ptr()),
            (16, capture.etw_lost.as_ptr()),
            (17, capture.etw_buffers_lost.as_ptr()),
            (18, sink.disconnected_dropped.as_ptr()),
            (19, parse.rejected.as_ptr()),
            (20, capture.callback_errors.as_ptr()),
            (21, sink.disconnects.as_ptr()),
            (32, parse.queue_depth_max.as_ptr()),
            (51, sink.reconnects.as_ptr()),
        ];
        assert_eq!(expected.len(), REFERENCE_IDS.len());
        for (id, pointer) in expected {
            assert_eq!(address(id), pointer, "counter {id}");
        }
    }

    #[test]
    fn the_table_splits_into_reference_and_value_counters() {
        let by_reference: Vec<u32> = COUNTERS
            .iter()
            .filter(|def| def.by_reference())
            .map(|def| def.id)
            .collect();
        let by_value: Vec<u32> = COUNTERS
            .iter()
            .filter(|def| !def.by_reference())
            .map(|def| def.id)
            .collect();
        assert_eq!(by_reference, REFERENCE_IDS);
        assert_eq!(by_value, VALUE_IDS);
    }

    #[test]
    fn referenced_counters_are_aligned_eight_byte_atomics() {
        let sources = sources(1024);
        for def in &COUNTERS {
            if let Source::Reference(atomic) = def.source {
                let pointer = atomic(&sources).as_ptr();
                assert_eq!(pointer.align_offset(8), 0, "counter {}", def.id);
            }
        }
    }

    #[test]
    fn template_layout_matches_the_native_structures() {
        assert_eq!(size_of::<PERF_COUNTERSET_INFO>(), 40);
        assert_eq!(size_of::<PERF_COUNTER_INFO>(), 32);
        assert_eq!(offset_of!(Template, set), 0);
        assert_eq!(offset_of!(Template, counters), 40);
        assert_eq!(size_of::<Template>(), 40 + COUNTER_COUNT * 32);
    }

    #[test]
    fn template_describes_every_counter() {
        let template = Template::new();
        assert_eq!(
            template.set.NumCounters,
            u32::try_from(COUNTERS.len()).expect("small count")
        );
        assert_eq!(template.set.InstanceType, PERF_COUNTERSET_MULTI_INSTANCES);
        assert_eq!(
            GUID::from_u128(PROVIDER_ID).data1,
            template.set.ProviderGuid.data1
        );
        assert_eq!(
            GUID::from_u128(COUNTER_SET_ID).data1,
            template.set.CounterSetGuid.data1
        );
        for (info, def) in template.counters.iter().zip(&COUNTERS) {
            assert_eq!(info.CounterId, def.id);
            assert_eq!(info.Type, def.counter_type);
            assert_eq!(info.DetailLevel, def.detail);
            assert_eq!(info.Size, 8);
            assert_eq!(info.Scale, 0);
            // PerfLib computes the instance offsets, as in the CTRPP header.
            assert_eq!(info.Offset, 0, "counter {}", def.id);
            let mut attributes = 0;
            if REFERENCE_IDS.contains(&def.id) {
                attributes |= PERF_ATTRIB_BY_REFERENCE;
            }
            if def.counter_type == PERF_LARGE_RAW_BASE {
                attributes |= PERF_ATTRIB_NO_DISPLAYABLE;
            }
            assert_eq!(info.Attrib, attributes, "counter {}", def.id);
        }
    }

    #[test]
    fn counter_ids_are_unique() {
        let mut ids: Vec<u32> = COUNTERS.iter().map(|def| def.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), COUNTERS.len());
    }

    /// A counter element from the manifest: its opening tag, and the text up to
    /// the next counter (which holds any child elements).
    struct ManifestCounter<'a> {
        tag: &'a str,
        body: &'a str,
    }

    impl ManifestCounter<'_> {
        fn attribute(&self, name: &str) -> Option<&str> {
            attribute(self.tag, name)
        }

        /// True when the element has `<counterAttribute name="...">`.
        fn has_counter_attribute(&self, name: &str) -> bool {
            self.body
                .contains(&format!("<counterAttribute name=\"{name}\""))
        }
    }

    /// The value of `name="..."` in `tag`, matching whole attribute names only
    /// (`id` must not match `baseID`).
    fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
        let pattern = format!("{name}=\"");
        tag.match_indices(&pattern).find_map(|(start, _)| {
            let whole = tag[..start]
                .chars()
                .next_back()
                .is_none_or(char::is_whitespace);
            let rest = &tag[start + pattern.len()..];
            whole.then(|| rest.split('"').next()).flatten()
        })
    }

    fn manifest_counters() -> Vec<ManifestCounter<'static>> {
        // "<counter " does not match <counterSet, <counterAttributes, or
        // <counterAttribute, which have no space after "counter".
        MANIFEST
            .split("<counter ")
            .skip(1)
            .map(|body| ManifestCounter {
                tag: body.split('>').next().expect("an element ends"),
                body,
            })
            .collect()
    }

    fn manifest_type(name: &str) -> u32 {
        match name {
            "perf_counter_bulk_count" => PERF_COUNTER_BULK_COUNT,
            "perf_counter_large_rawcount" => PERF_COUNTER_LARGE_RAWCOUNT,
            "perf_large_raw_fraction" => PERF_LARGE_RAW_FRACTION,
            "perf_large_raw_base" => PERF_LARGE_RAW_BASE,
            other => panic!("unmapped counter type {other}"),
        }
    }

    fn manifest_detail(name: &str) -> u32 {
        match name {
            "standard" => PERF_DETAIL_NOVICE,
            "advanced" => PERF_DETAIL_ADVANCED,
            other => panic!("unmapped detail level {other}"),
        }
    }

    fn guid_value(text: &str) -> u128 {
        let digits: String = text.chars().filter(char::is_ascii_hexdigit).collect();
        u128::from_str_radix(&digits, 16).expect("a GUID has 32 hex digits")
    }

    #[test]
    fn manifest_and_table_list_the_same_counters() {
        let manifest = manifest_counters();
        assert_eq!(manifest.len(), COUNTERS.len(), "counter count");
        for (index, (element, def)) in manifest.iter().zip(&COUNTERS).enumerate() {
            let id = element.attribute("id").expect("id");
            assert_eq!(id.parse::<u32>().expect("numeric id"), def.id, "{index}");
            let counter_type = element.attribute("type").expect("type");
            assert_eq!(
                manifest_type(counter_type),
                def.counter_type,
                "counter {id}"
            );
            let detail = element.attribute("detailLevel").expect("detailLevel");
            assert_eq!(manifest_detail(detail), def.detail, "counter {id}");
        }
    }

    #[test]
    fn manifest_marks_exactly_the_reference_counters() {
        let manifest = manifest_counters();
        let marked: Vec<u32> = manifest
            .iter()
            .zip(&COUNTERS)
            .filter(|(element, _)| element.has_counter_attribute("reference"))
            .map(|(_, def)| def.id)
            .collect();
        assert_eq!(marked, REFERENCE_IDS);
        for (element, def) in manifest.iter().zip(&COUNTERS) {
            assert_eq!(
                element.has_counter_attribute("reference"),
                def.by_reference(),
                "counter {}",
                def.id
            );
        }
    }

    #[test]
    fn manifest_pairs_each_fraction_with_a_hidden_base() {
        let manifest = manifest_counters();
        for (element, def) in manifest.iter().zip(&COUNTERS) {
            let base_id = element.attribute("baseID");
            let hidden = element.has_counter_attribute("noDisplay");
            let described = element
                .attribute("description")
                .is_some_and(|d| !d.is_empty());
            match def.counter_type {
                PERF_LARGE_RAW_FRACTION => {
                    let base = COUNTERS
                        .iter()
                        .find(|candidate| Some(candidate.id.to_string().as_str()) == base_id)
                        .expect("baseID names a counter");
                    assert_eq!(base.counter_type, PERF_LARGE_RAW_BASE, "{}", def.id);
                    assert!(described && !hidden, "counter {}", def.id);
                    let expected = match def.id {
                        33 => "34",
                        43 => "44",
                        other => panic!("unexpected fraction counter {other}"),
                    };
                    assert_eq!(base_id, Some(expected), "counter {}", def.id);
                }
                PERF_LARGE_RAW_BASE => {
                    assert!(hidden, "base counter {} is noDisplay", def.id);
                    assert_eq!(base_id, None);
                }
                _ => {
                    assert!(described && !hidden, "counter {}", def.id);
                    assert_eq!(base_id, None);
                }
            }
        }
    }

    /// The strings of the committed `.rc`, which CTRPP writes as UTF-16LE.
    fn resource_text() -> String {
        const RESOURCE: &[u8] = include_bytes!("../packaging/perfcounters/etw-dnstap-perf.rc");
        let units: Vec<u16> = RESOURCE
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes(*pair))
            .collect();
        // Skip the byte order mark.
        let units = units.strip_prefix(&[0xFEFF]).unwrap_or(&units);
        String::from_utf16(units).expect("the .rc is UTF-16")
    }

    #[test]
    fn resource_file_has_every_manifest_string() {
        let resource = resource_text();
        let mut checked = 0;
        let set = MANIFEST
            .split("<counterSet ")
            .nth(1)
            .and_then(|rest| rest.split('>').next())
            .expect("counterSet element");
        let tags = std::iter::once(set).chain(manifest_counters().into_iter().map(|c| c.tag));
        for tag in tags {
            for (id_name, text_name) in [("nameID", "name"), ("descriptionID", "description")] {
                let Some(text) = attribute(tag, text_name) else {
                    continue;
                };
                let id = attribute(tag, id_name).expect("a string ID");
                let line = format!("{id} \"{text}\"");
                assert!(resource.contains(&line), "missing from the .rc: {line}");
                checked += 1;
            }
        }
        // The set has a name and a description; 25 displayed counters have both,
        // and the 2 base counters have a name only.
        assert_eq!(checked, 2 + 25 * 2 + 2);
    }

    #[test]
    fn manifest_identifiers_match_the_code() {
        assert!(MANIFEST.contains("schemaVersion=\"2.0\""));
        let provider = MANIFEST
            .split("<provider ")
            .nth(1)
            .and_then(|rest| rest.split('>').next())
            .expect("provider element");
        assert_eq!(
            guid_value(attribute(provider, "providerGuid").expect("providerGuid")),
            PROVIDER_ID
        );
        assert_eq!(
            attribute(provider, "applicationIdentity"),
            Some("etw-dnstap.exe")
        );
        assert_eq!(attribute(provider, "providerType"), Some("userMode"));
        let set = MANIFEST
            .split("<counterSet ")
            .nth(1)
            .and_then(|rest| rest.split('>').next())
            .expect("counterSet element");
        assert_eq!(
            guid_value(attribute(set, "guid").expect("guid")),
            COUNTER_SET_ID
        );
        assert_eq!(attribute(set, "instances"), Some("multiple"));
    }

    #[test]
    #[ignore = "registers a provider with the production GUID on this machine"]
    fn start_fails_or_succeeds_without_panicking() {
        // Registration needs an installed manifest, so either result is valid.
        // The call must return and release everything it acquired.
        let sources = sources(1024);
        match PerfPublisher::start("EtwDnstapUnitTest", sources.clone()) {
            Ok(publisher) => {
                sources.capture.received.fetch_add(1, Ordering::Relaxed);
                publisher.publish(&CounterValues::default());
                drop(publisher);
                // The publisher released its references on drop.
                assert_eq!(Arc::strong_count(&sources.capture), 1);
            }
            Err(error) => {
                let text = format!("{error}");
                assert!(text.contains("failed"), "{text}");
                assert_eq!(Arc::strong_count(&sources.capture), 1);
            }
        }
    }
}
