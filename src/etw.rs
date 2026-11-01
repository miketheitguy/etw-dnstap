//! Owns the real-time DNS ETW session and copies borrowed records into pooled storage.
//!
//! Module map:
//! - `session`: `StartTrace`/`ControlTrace`, stale-session reclaim, and [`Capture`].
//! - `target`: the DNS Server service process ID used by the PID filter.
//! - `keywords`: provider keyword metadata and the subscription masks.
//! - `intake`: the native callback and the deep copy into pooled storage.
//! - `schema`: TDH schema loading, plan compilation, and [`Parser`], which
//!   yields a format-neutral [`crate::event::DecodedDnsEvent`].
//! - `decode`: the single-pass decoder driven by a compiled plan.
//! - `pool`: the bounded pool of reusable record buffers (leases).
//! - `session_guid`: the per-run session GUID.
//! - `diag` (feature `diag-timing`): timer and thread-priority diagnostics.
//! - `fuzzing` (feature `fuzzing`): entry points for the fuzz targets.
//!
//! Only the modules that call native APIs allow `unsafe` code.

use std::sync::atomic::{AtomicBool, AtomicU64};

use anyhow::{Result, bail};
use windows_sys::{Win32::Foundation::ERROR_SUCCESS, core::GUID};

mod decode;
#[cfg(feature = "diag-timing")]
#[allow(unsafe_code)]
mod diag;
#[cfg(feature = "fuzzing")]
pub(crate) mod fuzzing;
#[allow(unsafe_code)]
mod intake;
#[allow(unsafe_code)]
mod keywords;
// Production code in `pool` is safe; only its tests read raw lease memory.
#[cfg_attr(test, allow(unsafe_code))]
mod pool;
#[allow(unsafe_code)]
mod schema;
#[allow(unsafe_code)]
mod session;
#[allow(unsafe_code)]
mod session_guid;
#[allow(unsafe_code)]
mod target;
#[cfg(test)]
#[allow(unsafe_code)]
mod test_support;

#[cfg(feature = "diag-timing")]
pub(crate) use diag::{
    TIME_NO_ERROR, ThreadPriority, set_current_thread_priority, time_begin_period, time_end_period,
};
pub(crate) use intake::RawEtwEvent;
pub(crate) use keywords::KeywordSelection;
pub(crate) use pool::{MAX_CAPACITY, MAX_JUMBO_CAPACITY, MIN_CAPACITY, PoolManager};
pub(crate) use schema::Parser;
pub(crate) use session::{Capture, SessionConfig};

/// The service's ETW session GUID for `name`, as text for logs.
pub(crate) fn session_guid_text(name: &str) -> String {
    session_guid::session_guid(name).map_or_else(
        |error| format!("unavailable ({error})"),
        |guid| session_guid::format_guid(&guid),
    )
}
#[cfg(test)]
pub(crate) use test_support::assert_no_allocations;

const PROVIDER: GUID = GUID::from_u128(0xeb79061a_a566_4698_9119_3ed2807060e7);
/// Event IDs the ETW runtime event-ID filter admits and the callback accepts.
const SUPPORTED_EVENT_IDS: [u16; 2] = [256, 257];
// Windows defines fewer than 20 extended-data item types, each at most once per
// event, and this session enables no optional enable properties that add
// extended data. Records with more items are rejected as malformed.
const MAX_EXTENDED_ITEMS: usize = 16;
/// Upper bound on any TDH metadata block (keywords and event schemas).
const MAX_METADATA: usize = 1_048_576;

#[derive(Default)]
pub(crate) struct CaptureCounters {
    pub(crate) received: AtomicU64,
    /// Events dropped because the channel was full. A subset of `intake_dropped`;
    /// never add the two together.
    pub(crate) dropped: AtomicU64,
    /// Every event the callback accepted but did not enqueue (full channel, pool
    /// exhaustion, or a closed channel).
    pub(crate) intake_dropped: AtomicU64,
    /// Events dropped because no pool slot was free, regular and jumbo together.
    /// Also counted in `intake_dropped`; the report splits it into regular and jumbo.
    pub(crate) pool_exhausted: AtomicU64,
    /// The jumbo share of `pool_exhausted`.
    pub(crate) pool_exhausted_jumbo: AtomicU64,
    /// Records whose `EventHeader.ProcessId` was not the DNS Server service
    /// process. They are not counted as `received`.
    pub(crate) foreign_pid_rejected: AtomicU64,
    pub(crate) receiver_disconnected: AtomicU64,
    pub(crate) etw_lost: AtomicU64,
    pub(crate) etw_buffers_lost: AtomicU64,
    pub(crate) callback_errors: AtomicU64,
    pub(crate) stats_valid: AtomicBool,
    pub(crate) stats_errors: AtomicU64,
    /// Successful periodic `EVENT_TRACE_CONTROL_FLUSH` requests.
    pub(crate) etw_flushes: AtomicU64,
    /// Failed periodic flush requests.
    pub(crate) etw_flush_errors: AtomicU64,
    /// Longest wall-clock duration of one flush call in microseconds, measured
    /// by the caller. It includes scheduling delay on the supervisor thread.
    pub(crate) etw_flush_max_us: AtomicU64,
    /// Set once a flush has been reported as slow.
    pub(crate) etw_flush_slow_warned: AtomicBool,
    /// Leaked sessions from an earlier instance that startup stopped and replaced.
    pub(crate) stale_sessions_reclaimed: AtomicU64,
}

fn supported(id: u16) -> bool {
    SUPPORTED_EVENT_IDS.contains(&id)
}

/// Field-by-field GUID equality (`GUID` has no `PartialEq`).
fn same_guid(a: &GUID, b: &GUID) -> bool {
    a.data1 == b.data1 && a.data2 == b.data2 && a.data3 == b.data3 && a.data4 == b.data4
}

fn dns_provider(guid: &GUID) -> bool {
    same_guid(guid, &PROVIDER)
}

fn check_status(status: u32, operation: &str) -> Result<()> {
    if status != ERROR_SUCCESS {
        bail!(
            "{operation}: {} (code {status})",
            std::io::Error::from_raw_os_error(status.cast_signed())
        );
    }
    Ok(())
}
