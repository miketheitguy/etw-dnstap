//! Owns the real-time DNS ETW session and copies borrowed records into pooled storage.

use std::{
    cell::RefCell,
    mem::{offset_of, size_of},
    net::IpAddr,
    ptr::{null, null_mut},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use crossbeam_channel::{Sender, TrySendError};

mod decode;
mod pool;
use decode::{DecodePlan, FieldSpec};
use pool::PooledBuffer;
pub(crate) use pool::{MAX_CAPACITY, MIN_CAPACITY, PoolManager};
use windows_sys::{
    Win32::{
        Foundation::{
            ERROR_ALREADY_EXISTS, ERROR_CANCELLED, ERROR_CTX_CLOSE_PENDING,
            ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_HANDLE, ERROR_MORE_DATA, ERROR_SUCCESS,
            ERROR_WMI_INSTANCE_NOT_FOUND, GetLastError,
        },
        System::Diagnostics::Etw::{
            CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, DecodingSourceXMLFile,
            ENABLE_TRACE_PARAMETERS, ENABLE_TRACE_PARAMETERS_VERSION_2, ETW_BUFFER_CONTEXT,
            EVENT_CONTROL_CODE_ENABLE_PROVIDER, EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0,
            EVENT_FILTER_DESCRIPTOR, EVENT_FILTER_EVENT_ID, EVENT_FILTER_TYPE_EVENT_ID,
            EVENT_HEADER, EVENT_HEADER_EXTENDED_DATA_ITEM, EVENT_PROPERTY_INFO, EVENT_RECORD,
            EVENT_TRACE_BUFFERING_MODE, EVENT_TRACE_CONTROL_FLUSH, EVENT_TRACE_CONTROL_QUERY,
            EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_FILE_MODE_APPEND, EVENT_TRACE_FILE_MODE_CIRCULAR,
            EVENT_TRACE_FILE_MODE_NEWFILE, EVENT_TRACE_FILE_MODE_SEQUENTIAL, EVENT_TRACE_LOGFILEW,
            EVENT_TRACE_PRIVATE_LOGGER_MODE, EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE,
            EVENT_TRACE_SYSTEM_LOGGER_MODE, EnableTraceEx2, EventKeywordInformation, OpenTraceW,
            PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_REAL_TIME, PROCESSTRACE_HANDLE,
            PROPERTY_DATA_DESCRIPTOR, PROVIDER_FIELD_INFO, PROVIDER_FIELD_INFOARRAY, ProcessTrace,
            PropertyHasCustomSchema, PropertyParamCount, PropertyParamLength, PropertyStruct,
            StartTraceW, TDH_INTYPE_ANSISTRING, TDH_INTYPE_BINARY, TDH_INTYPE_BOOLEAN,
            TDH_INTYPE_UINT8, TDH_INTYPE_UINT16, TDH_INTYPE_UINT32, TDH_INTYPE_UNICODESTRING,
            TDH_OUTTYPE_IPV4, TDH_OUTTYPE_IPV6, TDH_OUTTYPE_PORT, TRACE_EVENT_INFO,
            TdhEnumerateProviderFieldInformation, TdhGetEventInformation, TdhGetProperty,
            TdhGetPropertySize, WNODE_FLAG_TRACED_GUID,
        },
    },
    core::GUID,
};

use crate::dnstap::{Encoder, RawDnsEvent, Transport};

/// Diagnostic thread priority levels; compiled only for the diagnostic build.
#[cfg(feature = "diag-timing")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum ThreadPriority {
    AboveNormal,
    Highest,
}

#[cfg(feature = "diag-timing")]
impl ThreadPriority {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AboveNormal => "above-normal",
            Self::Highest => "highest",
        }
    }
}

/// Sets the calling thread's priority. Failure is logged once and ignored,
/// because the setting is a diagnostic experiment, not a capture requirement.
#[cfg(feature = "diag-timing")]
pub(crate) fn set_current_thread_priority(priority: ThreadPriority) {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL, THREAD_PRIORITY_HIGHEST,
    };
    let native = match priority {
        ThreadPriority::AboveNormal => THREAD_PRIORITY_ABOVE_NORMAL,
        ThreadPriority::Highest => THREAD_PRIORITY_HIGHEST,
    };
    // SAFETY: GetCurrentThread returns a pseudo-handle valid only for the
    // calling thread, which is the thread being adjusted; it needs no closing.
    let succeeded = unsafe { SetThreadPriority(GetCurrentThread(), native) } != 0;
    if !succeeded {
        // SAFETY: GetLastError is read immediately after the failing API call.
        let error = unsafe { GetLastError() };
        tracing::warn!(
            priority = priority.as_str(),
            error = %std::io::Error::from_raw_os_error(error.cast_signed()),
            "SetThreadPriority failed; continuing at the default priority"
        );
    }
}

/// Raises the process timer resolution request to `milliseconds`. Returns the
/// `MMRESULT`, which is zero on success.
#[cfg(feature = "diag-timing")]
pub(crate) fn time_begin_period(milliseconds: u32) -> u32 {
    // SAFETY: timeBeginPeriod takes a plain integer and has no pointer arguments.
    unsafe { windows_sys::Win32::Media::timeBeginPeriod(milliseconds) }
}

/// Withdraws a request made by `time_begin_period` with the same value.
#[cfg(feature = "diag-timing")]
pub(crate) fn time_end_period(milliseconds: u32) -> u32 {
    // SAFETY: timeEndPeriod takes a plain integer and has no pointer arguments.
    unsafe { windows_sys::Win32::Media::timeEndPeriod(milliseconds) }
}

/// The success value of the `timeBeginPeriod` family.
#[cfg(feature = "diag-timing")]
pub(crate) const TIME_NO_ERROR: u32 = windows_sys::Win32::Media::TIMERR_NOERROR;

const PROVIDER: GUID = GUID::from_u128(0xeb79061a_a566_4698_9119_3ed2807060e7);
const SUBSCRIPTION_LEVEL: u8 = 4;
/// Fallback `MatchAnyKeyword`: the Analytical channel bit, used when the
/// provider's keyword metadata cannot be resolved.
const SUBSCRIPTION_MATCH_ANY_KEYWORD: u64 = 0x8000_0000_0000_0000;
/// `MatchAllKeyword` in both the narrowed and fallback subscriptions. The
/// channel bit is not required, because only one build's channel bits are known.
const SUBSCRIPTION_MATCH_ALL_KEYWORD: u64 = 0;
/// Provider-defined keyword bits; bits 48-63 are reserved for channels and
/// Microsoft.
const PROVIDER_KEYWORD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
/// Manifest keyword names whose bits select events 256 and 257.
const KEYWORD_QUERY_RECEIVED: &str = "QUERY_RECEIVED";
const KEYWORD_RESPONSE_SUCCESS: &str = "RESPONSE_SUCCESS";
/// Event IDs the ETW runtime event-ID filter admits and the callback accepts.
const SUPPORTED_EVENT_IDS: [u16; 2] = [256, 257];
// Windows defines fewer than 20 extended-data item types, each at most once per
// event, and this session enables no optional enable properties that add
// extended data. Records with more items are rejected as malformed.
const MAX_EXTENDED_ITEMS: usize = 16;
/// Schemas cached per parser; the cache is cleared when it fills.
const MAX_CACHED_SCHEMAS: usize = 8;
const MAX_METADATA: usize = 1_048_576;

#[derive(Default)]
pub(crate) struct CaptureCounters {
    pub(crate) received: AtomicU64,
    pub(crate) dropped: AtomicU64,
    pub(crate) intake_dropped: AtomicU64,
    pub(crate) pool_exhausted: AtomicU64,
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

/// Owns the native metadata and an exclusive lease, without borrowed ETW pointers.
pub(crate) struct RawEtwEvent {
    header: EVENT_HEADER,
    buffer_context: ETW_BUFFER_CONTEXT,
    data_length: u16,
    extended_count: u16,
    buffer: PooledBuffer,
}

struct CallbackContext {
    pool: Arc<PoolManager>,
    counters: Arc<CaptureCounters>,
}

struct CallbackProducer {
    context: usize,
    sender: Sender<RawEtwEvent>,
    pool: Arc<PoolManager>,
}

thread_local! {
    // Eagerly installed on the ProcessTrace worker before native delivery starts.
    static PRODUCER: RefCell<Option<CallbackProducer>> = const { RefCell::new(None) };
}

/// ETW session buffer settings, validated by `SessionConfig::new`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionConfig {
    /// Buffer size in KiB (4..=128, a multiple of 4; see [`SessionConfig::new`]).
    pub(crate) buffer_kib: u32,
    /// Maximum buffer count (4..=4096). A defensive 1 GiB guard (unreachable with these ranges, at most 512 MiB) bounds the requested
    /// `buffer_kib * maximum_buffers` only: ETW reserves at least two buffers per
    /// logical processor and raises `MaximumBuffers` to at least that adjusted
    /// minimum, so actual peak memory can be higher.
    pub(crate) maximum_buffers: u32,
    /// Real-time flush interval in seconds (1..=60); bounds delivery latency.
    pub(crate) flush_seconds: u32,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            buffer_kib: 16,
            maximum_buffers: 12,
            flush_seconds: 1,
        }
    }
}

impl SessionConfig {
    /// Validates each range, that the buffer size is a multiple of 4 KiB, and the
    /// requested maximum buffer memory (defensive guard at 1 GiB). A page multiple is expected
    /// not to be adjusted for real-time-only sessions, which keeps the reclaim
    /// fingerprint exact; the elevated reclaim tests verify this live.
    pub(crate) fn new(buffer_kib: u32, maximum_buffers: u32, flush_seconds: u32) -> Result<Self> {
        ensure!(
            (4..=128).contains(&buffer_kib),
            "ETW buffer size must be 4..=128 KiB, got {buffer_kib}"
        );
        ensure!(
            (4..=4096).contains(&maximum_buffers),
            "ETW maximum buffers must be 4..=4096, got {maximum_buffers}"
        );
        ensure!(
            (1..=60).contains(&flush_seconds),
            "ETW flush interval must be 1..=60 seconds, got {flush_seconds}"
        );
        ensure!(
            buffer_kib.is_multiple_of(4),
            "ETW buffer size must be a multiple of 4 KiB, got {buffer_kib}"
        );
        let peak_kib = u64::from(buffer_kib) * u64::from(maximum_buffers);
        ensure!(
            peak_kib <= 1_048_576,
            "ETW buffer memory {buffer_kib} KiB x {maximum_buffers} buffers = {peak_kib} KiB \
             exceeds the 1 GiB limit (1048576 KiB)"
        );
        Ok(Self {
            buffer_kib,
            maximum_buffers,
            flush_seconds,
        })
    }
}

#[repr(C)]
struct SessionProperties {
    properties: EVENT_TRACE_PROPERTIES,
    logger_name: [u16; 1025],
}

impl SessionProperties {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "The fixed native header plus 1025 UTF-16 units fits u32."
    )]
    fn new(config: &SessionConfig) -> Self {
        let mut result = Self {
            properties: EVENT_TRACE_PROPERTIES::default(),
            logger_name: [0; 1025],
        };
        result.properties.Wnode.BufferSize = size_of::<Self>() as u32;
        result.properties.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        // System time, in 100 ns FILETIME units. ProcessTrace also converts timestamps
        // to this representation because RAW_TIMESTAMP is deliberately omitted.
        result.properties.Wnode.ClientContext = 2;
        result.properties.BufferSize = config.buffer_kib;
        result.properties.MinimumBuffers = 4;
        result.properties.MaximumBuffers = config.maximum_buffers;
        result.properties.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        result.properties.FlushTimer = config.flush_seconds;
        result.properties.LoggerNameOffset = offset_of!(Self, logger_name) as u32;
        result
    }
}

/// Buffer for `ControlTraceW` QUERY and STOP-by-name. Both name offsets are set
/// because `ControlTrace` reports the log-file name through `LogFileNameOffset`.
#[repr(C)]
struct QueryProperties {
    properties: EVENT_TRACE_PROPERTIES,
    logger_name: [u16; 1025],
    log_file_name: [u16; 1025],
}

impl QueryProperties {
    #[expect(
        clippy::cast_possible_truncation,
        reason = "The fixed native header plus 2050 UTF-16 units fits u32."
    )]
    fn new() -> Self {
        let mut result = Self {
            properties: EVENT_TRACE_PROPERTIES::default(),
            logger_name: [0; 1025],
            log_file_name: [0; 1025],
        };
        result.properties.Wnode.BufferSize = size_of::<Self>() as u32;
        result.properties.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        result.properties.LoggerNameOffset = offset_of!(Self, logger_name) as u32;
        result.properties.LogFileNameOffset = offset_of!(Self, log_file_name) as u32;
        result
    }
}

const FILE_MODE_BITS: u32 = EVENT_TRACE_FILE_MODE_SEQUENTIAL
    | EVENT_TRACE_FILE_MODE_CIRCULAR
    | EVENT_TRACE_FILE_MODE_APPEND
    | EVENT_TRACE_FILE_MODE_NEWFILE;

/// Whether a queried session has the shape this program creates: real-time
/// only, no file or other logger modes, no log file, and the configured buffer
/// size. Buffer counts, flush timer, and clock type are ignored because ETW may
/// normalize them.
fn reclaimable(query: &QueryProperties, config: &SessionConfig) -> bool {
    let properties = &query.properties;
    let forbidden_modes = FILE_MODE_BITS
        | EVENT_TRACE_SYSTEM_LOGGER_MODE
        | EVENT_TRACE_BUFFERING_MODE
        | EVENT_TRACE_PRIVATE_LOGGER_MODE;
    // A zero offset means no log file. Otherwise the offset must be the unchanged
    // log-file field and its name must be empty.
    let no_log_file = properties.LogFileNameOffset == 0
        || (properties.LogFileNameOffset as usize == offset_of!(QueryProperties, log_file_name)
            && query.log_file_name[0] == 0);
    properties.LogFileMode & EVENT_TRACE_REAL_TIME_MODE != 0
        && properties.LogFileMode & forbidden_modes == 0
        && properties.BufferSize == config.buffer_kib
        && no_log_file
}

/// Result of a `ControlTraceW` stop call.
#[derive(Debug, PartialEq, Eq)]
enum StopOutcome {
    /// The session no longer exists (stopped, already stopped, or vanished).
    Stopped,
    Failed,
}

fn stop_outcome(status: u32) -> StopOutcome {
    match status {
        // MORE_DATA means ETW stopped the session but the properties buffer was
        // too small for everything it wanted to return.
        ERROR_SUCCESS | ERROR_MORE_DATA | ERROR_WMI_INSTANCE_NOT_FOUND => StopOutcome::Stopped,
        _ => StopOutcome::Failed,
    }
}

/// Result of a `ControlTraceW` query call.
#[derive(Debug, PartialEq, Eq)]
enum QueryOutcome {
    /// The session exists; its properties can be inspected.
    Inspect,
    /// The session disappeared after `StartTraceW` reported it.
    Vanished,
    Failed,
}

fn query_outcome(status: u32) -> QueryOutcome {
    match status {
        ERROR_SUCCESS => QueryOutcome::Inspect,
        ERROR_WMI_INSTANCE_NOT_FOUND => QueryOutcome::Vanished,
        _ => QueryOutcome::Failed,
    }
}

/// Requests an `EVENT_TRACE_CONTROL_FLUSH` on a session handle. `ERROR_MORE_DATA`
/// only means the properties buffer was too small, which a flush ignores.
fn flush_session(session: CONTROLTRACE_HANDLE) -> Result<(), u32> {
    let mut properties = QueryProperties::new();
    // SAFETY: The properties buffer is aligned, sized (Wnode.BufferSize and both
    // offsets set), and outlives this synchronous call. The handle is only a
    // value: a stopped or externally stopped session yields an error status, and
    // the call does not dereference stale memory. A null name selects the handle.
    let status = unsafe {
        ControlTraceW(
            session,
            null(),
            &raw mut properties.properties,
            EVENT_TRACE_CONTROL_FLUSH,
        )
    };
    if matches!(status, ERROR_SUCCESS | ERROR_MORE_DATA) {
        Ok(())
    } else {
        Err(status)
    }
}

/// Calls `StartTraceW` with fresh properties and returns its status and handle.
fn start_session(name: &[u16], config: &SessionConfig) -> (u32, CONTROLTRACE_HANDLE) {
    let mut properties = SessionProperties::new(config);
    let mut session = CONTROLTRACE_HANDLE::default();
    // SAFETY: `name` is NUL-terminated by the caller and, with the aligned
    // properties (including trailing name storage), outlives this synchronous call.
    let status = unsafe {
        StartTraceW(
            &raw mut session,
            name.as_ptr(),
            &raw mut properties.properties,
        )
    };
    (status, session)
}

/// Stops a leaked session of the same name, but only if it has this program's
/// shape. A session that vanishes before the query returns is treated as already
/// gone so the caller's single retry can succeed. No handle is owned yet, so
/// every failure leaves nothing to clean up.
fn reclaim_stale_session(
    name: &[u16],
    display_name: &str,
    counters: &CaptureCounters,
    config: &SessionConfig,
) -> Result<()> {
    let control = |properties: &mut QueryProperties, code: u32| {
        // SAFETY: `name` is NUL-terminated and the aligned, sized properties outlive
        // this synchronous call; a zero handle selects lookup by name.
        unsafe {
            ControlTraceW(
                CONTROLTRACE_HANDLE::default(),
                name.as_ptr(),
                &raw mut properties.properties,
                code,
            )
        }
    };
    reclaim_decision(
        display_name,
        counters,
        config,
        |properties| control(properties, EVENT_TRACE_CONTROL_QUERY),
        |properties| control(properties, EVENT_TRACE_CONTROL_STOP),
    )
}

/// The reclaim decision, with the query and stop operations injected so it is
/// testable without ETW. Each closure receives fresh properties and returns the
/// `ControlTraceW` status. The stale-session counter increments only when the stop
/// itself ended the session; a session that vanished on its own is not counted.
fn reclaim_decision(
    display_name: &str,
    counters: &CaptureCounters,
    config: &SessionConfig,
    query: impl FnOnce(&mut QueryProperties) -> u32,
    stop: impl FnOnce(&mut QueryProperties) -> u32,
) -> Result<()> {
    let mut queried = QueryProperties::new();
    let status = query(&mut queried);
    match query_outcome(status) {
        QueryOutcome::Inspect => {}
        QueryOutcome::Vanished => return Ok(()),
        QueryOutcome::Failed => bail!(
            "query existing ETW session '{display_name}' failed: {} (code {status})",
            std::io::Error::from_raw_os_error(status.cast_signed())
        ),
    }
    ensure!(
        reclaimable(&queried, config),
        "ETW session '{display_name}' already exists and does not match this program's session \
         shape; choose another --session-name or stop it manually, for example: logman stop \
         {display_name} -ets"
    );
    let mut stopping = QueryProperties::new();
    let status = stop(&mut stopping);
    ensure!(
        stop_outcome(status) == StopOutcome::Stopped,
        "stopping stale ETW session '{display_name}' failed: {} (code {status})",
        std::io::Error::from_raw_os_error(status.cast_signed())
    );
    if status == ERROR_WMI_INSTANCE_NOT_FOUND {
        // The session vanished between QUERY and STOP; the retry can proceed.
        return Ok(());
    }
    counters
        .stale_sessions_reclaimed
        .fetch_add(1, Ordering::Relaxed);
    tracing::warn!(session = display_name, "reclaimed stale ETW session");
    Ok(())
}

/// Owns only the session created by its successful `StartTrace` call.
pub(crate) struct Capture {
    session: Option<CONTROLTRACE_HANDLE>,
    consumer: Option<PROCESSTRACE_HANDLE>,
    worker: Option<JoinHandle<u32>>,
    counters: Arc<CaptureCounters>,
    keywords: KeywordSelection,
}

impl Capture {
    /// Starts an independent real-time session. If the name is held by a leaked
    /// session with this program's shape, reclaims it once and retries; any other
    /// existing session with the name is an error.
    pub(crate) fn start(
        name: &str,
        sender: Sender<RawEtwEvent>,
        pool: Arc<PoolManager>,
        counters: Arc<CaptureCounters>,
        config: SessionConfig,
        #[cfg(feature = "diag-timing")] consumer_priority: Option<ThreadPriority>,
    ) -> Result<Self> {
        ensure!(
            !name.is_empty() && !name.contains('\0'),
            "invalid ETW session name"
        );
        let display_name = name;
        let mut name: Vec<u16> = name.encode_utf16().collect();
        ensure!(
            name.len() <= 1024,
            "ETW session name exceeds 1024 UTF-16 units"
        );
        name.push(0);
        // Metadata problems select the fallback; they never stop capture.
        let choice = resolve_keywords();
        if let KeywordChoice::Fallback { reason } = &choice {
            tracing::warn!(
                reason = %reason,
                "ETW keyword narrowing unavailable; subscribing to the full Analytical channel"
            );
        }
        let (mut status, mut session) = start_session(&name, &config);
        if status == ERROR_ALREADY_EXISTS {
            // One reclaim and one retry; a second failure is a startup error.
            reclaim_stale_session(&name, display_name, &counters, &config)?;
            (status, session) = start_session(&name, &config);
        }
        check_status(
            status,
            "StartTraceW (run elevated; use an unused session name)",
        )?;
        let keywords = choice.selection();
        let mut capture = Self {
            session: Some(session),
            consumer: None,
            worker: None,
            counters,
            keywords,
        };
        // The payload, descriptor, and parameters are locals that outlive the
        // synchronous EnableTraceEx2 call below.
        let filter_payload = EventIdFilterPayload::new();
        let mut filter_descriptor = event_id_descriptor(&filter_payload);
        let parameters = enable_parameters(&raw mut filter_descriptor);
        // SAFETY: The successful StartTrace call owns this handle. The provider GUID
        // is a constant GUID. `parameters` points at `filter_descriptor`, which points at
        // `filter_payload`; all three are stack locals that remain alive and
        // unmoved for this synchronous call, and ETW copies them before returning.
        let status = unsafe {
            EnableTraceEx2(
                session,
                &PROVIDER,
                EVENT_CONTROL_CODE_ENABLE_PROVIDER,
                SUBSCRIPTION_LEVEL,
                keywords.match_any,
                keywords.match_all,
                5000,
                &raw const parameters,
            )
        };
        check_status(status, "EnableTraceEx2")?;

        let mut context = Box::new(CallbackContext {
            pool: Arc::clone(&pool),
            counters: Arc::clone(&capture.counters),
        });
        let mut logfile = EVENT_TRACE_LOGFILEW {
            LoggerName: name.as_mut_ptr(),
            Context: (&raw mut *context).cast(),
            ..Default::default()
        };
        logfile.Anonymous1.ProcessTraceMode =
            PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
        logfile.Anonymous2.EventRecordCallback = Some(native_etw_callback);
        // SAFETY: All pointers in logfile remain valid throughout OpenTrace. Context
        // has a stable Box address, retained by the worker until ProcessTrace ends.
        let consumer = unsafe { OpenTraceW(&raw mut logfile) };
        if consumer.Value == u64::MAX {
            // SAFETY: GetLastError is read immediately after the failing API call.
            let error = unsafe { GetLastError() };
            bail!(
                "OpenTraceW failed: {}",
                std::io::Error::from_raw_os_error(error.cast_signed())
            );
        }
        capture.consumer = Some(consumer);
        capture.worker = Some(
            thread::Builder::new()
                .name("etw-consumer".into())
                .spawn(move || {
                    // Keep the callback's pointee alive through the entire native call.
                    let context_address = (&raw const *context) as usize;
                    PRODUCER.with(|state| {
                        *state.borrow_mut() = Some(CallbackProducer {
                            context: context_address,
                            sender,
                            pool,
                        });
                    });
                    let _context = context;
                    #[cfg(feature = "diag-timing")]
                    if let Some(priority) = consumer_priority {
                        set_current_thread_priority(priority);
                    }
                    // SAFETY: consumer is open. It is closed by Capture to request shutdown;
                    // CloseTrace explicitly permits concurrent real-time ProcessTrace calls.
                    let status = unsafe { ProcessTrace(&raw const consumer, 1, null(), null()) };
                    // No callback can recur after ProcessTrace returns. Dropping
                    // the final sender closes the channel. The supervisor publishes
                    // completion and unparks the polling parser after joining us.
                    PRODUCER.with(|state| {
                        state.borrow_mut().take();
                    });
                    status
                })
                .context("spawn ETW consumer")?,
        );
        Ok(capture)
    }

    /// The keyword masks this session enabled and whether they are the fallback.
    pub(crate) fn keywords(&self) -> KeywordSelection {
        self.keywords
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }

    fn store_statistics(&self, status: u32, properties: &EVENT_TRACE_PROPERTIES) {
        self.counters
            .stats_valid
            .store(status == ERROR_SUCCESS, Ordering::Relaxed);
        if status == ERROR_SUCCESS {
            self.counters
                .etw_lost
                .store(u64::from(properties.EventsLost), Ordering::Relaxed);
            self.counters
                .etw_buffers_lost
                .store(u64::from(properties.RealTimeBuffersLost), Ordering::Relaxed);
        } else {
            self.counters.stats_errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Asks ETW to deliver buffered real-time events now. The error carries the
    /// Win32 status; the caller counts outcomes. Statistics are not stored.
    pub(crate) fn flush(&self) -> Result<(), u32> {
        let Some(session) = self.session else {
            return Err(ERROR_INVALID_HANDLE);
        };
        flush_session(session)
    }

    /// Stops collection, waits for native callbacks to end, and reports loss.
    pub(crate) fn stop(&mut self) -> Result<()> {
        let mut failure = None;
        if let Some(session) = self.session.take() {
            let mut properties = SessionProperties::new(&SessionConfig::default());
            // SAFETY: Only the handle successfully created above is stopped; null
            // name forces handle lookup. The output buffer is aligned and sized.
            let status = unsafe {
                ControlTraceW(
                    session,
                    null(),
                    &raw mut properties.properties,
                    EVENT_TRACE_CONTROL_STOP,
                )
            };
            self.store_statistics(status, &properties.properties);
            if stop_outcome(status) == StopOutcome::Failed {
                // Preserve ownership so an explicit retry or Drop can make a
                // second stop attempt after the consumer has been closed.
                self.session = Some(session);
                failure = Some(anyhow!("ControlTraceW(stop) failed: {status}"));
            }
        }
        if let Some(consumer) = self.consumer.take() {
            // SAFETY: This is the OpenTrace handle, closed exactly once. Native
            // callbacks and their Box remain alive until the worker joins below.
            let status = unsafe { CloseTrace(consumer) };
            if status != ERROR_SUCCESS && status != ERROR_CTX_CLOSE_PENDING && failure.is_none() {
                failure = Some(anyhow!("CloseTrace failed: {status}"));
            }
        }
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(ERROR_SUCCESS | ERROR_CANCELLED) => {}
                Ok(status) if failure.is_none() => {
                    failure = Some(anyhow!("ProcessTrace failed: {status}"));
                }
                Err(_) if failure.is_none() => {
                    failure = Some(anyhow!("ETW consumer thread panicked"));
                }
                _ => {}
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // stop is also needed on partially initialized startup failures.
        if let Err(error) = self.stop() {
            eprintln!("ETW cleanup: {error:#}");
        }
    }
}

/// The `MatchAnyKeyword` and `MatchAllKeyword` masks a session enables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct KeywordSelection {
    pub(crate) match_any: u64,
    pub(crate) match_all: u64,
    /// True when keyword metadata could not be resolved and the full channel
    /// mask is in use.
    pub(crate) fallback: bool,
}

/// The outcome of resolving the provider's keywords by name.
#[derive(Debug, PartialEq, Eq)]
enum KeywordChoice {
    Narrowed { match_any: u64 },
    Fallback { reason: String },
}

impl KeywordChoice {
    fn selection(&self) -> KeywordSelection {
        match self {
            Self::Narrowed { match_any } => KeywordSelection {
                match_any: *match_any,
                match_all: SUBSCRIPTION_MATCH_ALL_KEYWORD,
                fallback: false,
            },
            Self::Fallback { .. } => KeywordSelection {
                match_any: SUBSCRIPTION_MATCH_ANY_KEYWORD,
                match_all: SUBSCRIPTION_MATCH_ALL_KEYWORD,
                fallback: true,
            },
        }
    }
}

/// Looks up one keyword by exact name and requires a single provider-defined bit.
fn keyword_bit(keywords: &[(String, u64)], name: &str) -> Result<u64, String> {
    let mut matches = keywords
        .iter()
        .filter(|(candidate, _)| candidate == name)
        .map(|&(_, value)| value);
    let value = matches
        .next()
        .ok_or_else(|| format!("keyword {name} is not defined"))?;
    if matches.any(|other| other != value) {
        return Err(format!("keyword {name} is defined with conflicting values"));
    }
    if value & !PROVIDER_KEYWORD_MASK != 0 {
        return Err(format!(
            "keyword {name} value {value:#x} is outside the provider-defined range"
        ));
    }
    if !value.is_power_of_two() {
        return Err(format!(
            "keyword {name} value {value:#x} is not a single bit"
        ));
    }
    Ok(value)
}

/// Chooses the narrowed mask from resolved keywords, or the fallback when the
/// lookup failed or the two keywords are missing, malformed, or share a bit.
fn select_keywords(lookup: Result<&[(String, u64)], &anyhow::Error>) -> KeywordChoice {
    let keywords = match lookup {
        Ok(keywords) => keywords,
        Err(error) => {
            return KeywordChoice::Fallback {
                reason: format!("keyword lookup failed: {error:#}"),
            };
        }
    };
    let bits = keyword_bit(keywords, KEYWORD_QUERY_RECEIVED)
        .and_then(|query| Ok((query, keyword_bit(keywords, KEYWORD_RESPONSE_SUCCESS)?)));
    match bits {
        Ok((query, response)) if query == response => KeywordChoice::Fallback {
            reason: format!(
                "{KEYWORD_QUERY_RECEIVED} and {KEYWORD_RESPONSE_SUCCESS} share bit {query:#x}"
            ),
        },
        Ok((query, response)) => KeywordChoice::Narrowed {
            match_any: query | response,
        },
        Err(reason) => KeywordChoice::Fallback { reason },
    }
}

/// Resolves the installed provider's keywords and applies [`select_keywords`].
fn resolve_keywords() -> KeywordChoice {
    let lookup = query_provider_keywords(&PROVIDER);
    select_keywords(lookup.as_deref())
}

/// Reads the provider's keyword names and values from TDH.
fn query_provider_keywords(provider: &GUID) -> Result<Vec<(String, u64)>> {
    let header = offset_of!(PROVIDER_FIELD_INFOARRAY, FieldInfoArray);
    let mut size = 0;
    // SAFETY: PROVIDER is a constant GUID. A null buffer with a zero size is the
    // documented size query, so TDH writes only `size`.
    let status = unsafe {
        TdhEnumerateProviderFieldInformation(
            provider,
            EventKeywordInformation,
            null_mut(),
            &raw mut size,
        )
    };
    ensure!(
        status == ERROR_INSUFFICIENT_BUFFER,
        "TDH keyword size query failed: {}",
        std::io::Error::from_raw_os_error(status.cast_signed())
    );
    ensure!(
        (header..=MAX_METADATA).contains(&(size as usize)),
        "invalid TDH keyword metadata size"
    );
    // u64 storage supplies the alignment of PROVIDER_FIELD_INFOARRAY, whose
    // entries hold u64 values.
    let mut storage = vec![0u64; (size as usize).div_ceil(8)];
    let capacity = size;
    // SAFETY: storage is aligned, initialized, and at least size bytes long.
    let status = unsafe {
        TdhEnumerateProviderFieldInformation(
            provider,
            EventKeywordInformation,
            storage.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    check_status(status, "TdhEnumerateProviderFieldInformation")?;
    ensure!(
        size <= capacity && size as usize >= header,
        "invalid TDH keyword result size"
    );
    // SAFETY: `storage` is zero-initialized, at least `capacity >= size` bytes
    // long, and outlives `bytes` without being mutated.
    let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), size as usize) };
    parse_keyword_array(bytes)
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let end = offset.checked_add(size_of::<u32>())?;
    Some(u32::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], offset: usize) -> Option<u64> {
    let end = offset.checked_add(size_of::<u64>())?;
    Some(u64::from_le_bytes(bytes.get(offset..end)?.try_into().ok()?))
}

/// Reads a NUL-terminated UTF-16 string that must end inside `bytes`.
fn read_utf16_name(bytes: &[u8], offset: usize) -> Option<String> {
    let mut units = Vec::new();
    let mut position = offset;
    loop {
        let end = position.checked_add(size_of::<u16>())?;
        let unit = u16::from_le_bytes(bytes.get(position..end)?.try_into().ok()?);
        if unit == 0 {
            return Some(String::from_utf16_lossy(&units));
        }
        units.push(unit);
        position = end;
    }
}

/// Parses a `PROVIDER_FIELD_INFOARRAY` into `(name, value)` pairs, checking the
/// element count and every name against the buffer length. Offsets are relative
/// to the start of the buffer.
fn parse_keyword_array(bytes: &[u8]) -> Result<Vec<(String, u64)>> {
    let count = read_u32(
        bytes,
        offset_of!(PROVIDER_FIELD_INFOARRAY, NumberOfElements),
    )
    .context("truncated TDH keyword header")? as usize;
    let first = offset_of!(PROVIDER_FIELD_INFOARRAY, FieldInfoArray);
    let entries_end = count
        .checked_mul(size_of::<PROVIDER_FIELD_INFO>())
        .and_then(|length| length.checked_add(first))
        .context("TDH keyword count overflows")?;
    ensure!(
        entries_end <= bytes.len(),
        "TDH keyword count {count} exceeds the {}-byte buffer",
        bytes.len()
    );
    (0..count)
        .map(|index| {
            let entry = first + index * size_of::<PROVIDER_FIELD_INFO>();
            let name_offset = read_u32(bytes, entry + offset_of!(PROVIDER_FIELD_INFO, NameOffset))
                .context("truncated TDH keyword entry")? as usize;
            let value = read_u64(bytes, entry + offset_of!(PROVIDER_FIELD_INFO, Value))
                .context("truncated TDH keyword entry")?;
            let name = read_utf16_name(bytes, name_offset)
                .with_context(|| format!("TDH keyword {index} name is outside the buffer"))?;
            Ok((name, value))
        })
        .collect()
}

fn supported(id: u16) -> bool {
    SUPPORTED_EVENT_IDS.contains(&id)
}

/// `EVENT_FILTER_EVENT_ID` with room for every supported ID; the windows-sys
/// definition declares a one-element flexible array.
#[repr(C)]
struct EventIdFilterPayload {
    filter_in: bool,
    reserved: u8,
    count: u16,
    events: [u16; SUPPORTED_EVENT_IDS.len()],
}

const _: () = {
    assert!(
        offset_of!(EventIdFilterPayload, filter_in) == offset_of!(EVENT_FILTER_EVENT_ID, FilterIn)
    );
    assert!(
        offset_of!(EventIdFilterPayload, reserved) == offset_of!(EVENT_FILTER_EVENT_ID, Reserved)
    );
    assert!(offset_of!(EventIdFilterPayload, count) == offset_of!(EVENT_FILTER_EVENT_ID, Count));
    assert!(offset_of!(EventIdFilterPayload, events) == offset_of!(EVENT_FILTER_EVENT_ID, Events));
    assert!(
        size_of::<EventIdFilterPayload>()
            == offset_of!(EVENT_FILTER_EVENT_ID, Events)
                + SUPPORTED_EVENT_IDS.len() * size_of::<u16>()
    );
    assert!(size_of::<EventIdFilterPayload>() == 8);
};

impl EventIdFilterPayload {
    /// Builds an include-only filter for [`SUPPORTED_EVENT_IDS`].
    const fn new() -> Self {
        Self {
            filter_in: true,
            reserved: 0,
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the supported ID list has two entries"
            )]
            count: SUPPORTED_EVENT_IDS.len() as u16,
            events: SUPPORTED_EVENT_IDS,
        }
    }
}

/// Describes `payload` as an event-ID filter; the caller keeps `payload` alive.
fn event_id_descriptor(payload: &EventIdFilterPayload) -> EVENT_FILTER_DESCRIPTOR {
    EVENT_FILTER_DESCRIPTOR {
        Ptr: std::ptr::from_ref(payload) as u64,
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the payload is 8 bytes, asserted at compile time"
        )]
        Size: size_of::<EventIdFilterPayload>() as u32,
        Type: EVENT_FILTER_TYPE_EVENT_ID,
    }
}

/// Builds version 2 enable parameters carrying one filter descriptor.
fn enable_parameters(descriptor: *mut EVENT_FILTER_DESCRIPTOR) -> ENABLE_TRACE_PARAMETERS {
    ENABLE_TRACE_PARAMETERS {
        Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
        // Drop keyword-0 events, which pass any keyword mask by default.
        EnableProperty: EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0,
        EnableFilterDesc: descriptor,
        FilterDescCount: 1,
        ..Default::default()
    }
}

fn dns_provider(guid: &GUID) -> bool {
    guid.data1 == PROVIDER.data1
        && guid.data2 == PROVIDER.data2
        && guid.data3 == PROVIDER.data3
        && guid.data4 == PROVIDER.data4
}

unsafe extern "system" fn native_etw_callback(record: *mut EVENT_RECORD) {
    // SAFETY: ETW invokes this callback with an EVENT_RECORD valid for the duration
    // of this call. A null defensive check precedes dereferencing the pointer.
    let Some(record) = (unsafe { record.as_ref() }) else {
        return;
    };
    if !dns_provider(&record.EventHeader.ProviderId)
        || !supported(record.EventHeader.EventDescriptor.Id)
    {
        return;
    }
    // SAFETY: OpenTrace received this stable Box pointee, retained until all
    // ProcessTrace callbacks finish; it is shared immutably across callbacks.
    let Some(context) = (unsafe { record.UserContext.cast::<CallbackContext>().as_ref() }) else {
        return;
    };
    context.counters.received.fetch_add(1, Ordering::Relaxed);
    let handled = PRODUCER
        .try_with(|state| {
            let Ok(mut state) = state.try_borrow_mut() else {
                return false;
            };
            let Some(producer) = state.as_mut() else {
                return false;
            };
            if producer.context != record.UserContext as usize
                || !Arc::ptr_eq(&producer.pool, &context.pool)
            {
                return false;
            }
            // SAFETY: ETW owns the readable native record and each advertised
            // pointer/length pair for this call. The pool is an independent allocation.
            let owned = match unsafe { copy_record(record, &producer.pool) } {
                Ok(owned) => owned,
                Err(CopyError::Malformed) => return false,
                Err(CopyError::Exhausted) => {
                    context
                        .counters
                        .pool_exhausted
                        .fetch_add(1, Ordering::Relaxed);
                    context
                        .counters
                        .intake_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    return true;
                }
            };
            match producer.sender.try_send(owned) {
                Ok(()) => {}
                Err(TrySendError::Full(_owned)) => {
                    context.counters.dropped.fetch_add(1, Ordering::Relaxed);
                    context
                        .counters
                        .intake_dropped
                        .fetch_add(1, Ordering::Relaxed);
                    // The returned lease recycles immediately on this path.
                }
                Err(TrySendError::Disconnected(_owned)) => {
                    context
                        .counters
                        .receiver_disconnected
                        .fetch_add(1, Ordering::Relaxed);
                    context
                        .counters
                        .intake_dropped
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            true
        })
        .unwrap_or(false);
    if !handled {
        context
            .counters
            .callback_errors
            .fetch_add(1, Ordering::Relaxed);
    }
}

#[derive(Debug)]
enum CopyError {
    Malformed,
    Exhausted,
}

fn align_payload(length: usize) -> Option<usize> {
    length.checked_add(7).map(|length| length & !7)
}

/// Deep-copies the record into an exclusive preallocated slot.
///
/// # Safety
/// The native record and its nonempty buffers must be readable for their stated
/// lengths until return, and must not overlap pool allocations. ETW guarantees
/// these native input lifetimes; size/null checks cannot validate arbitrary pointers.
unsafe fn copy_record(
    record: &EVENT_RECORD,
    pool: &Arc<PoolManager>,
) -> std::result::Result<RawEtwEvent, CopyError> {
    let invalid = || CopyError::Malformed;
    let data_length = usize::from(record.UserDataLength);
    let count = usize::from(record.ExtendedDataCount);
    if (data_length != 0 && record.UserData.is_null())
        || count > MAX_EXTENDED_ITEMS
        || (count != 0 && record.ExtendedData.is_null())
    {
        return Err(CopyError::Malformed);
    }
    let mut items = [EVENT_HEADER_EXTENDED_DATA_ITEM::default(); MAX_EXTENDED_ITEMS];
    let mut offsets = [0; MAX_EXTENDED_ITEMS];
    let mut length = data_length;
    let mut extended_length = 0usize;
    for index in 0..count {
        // SAFETY: Caller supplies count initialized descriptors. Reading unaligned
        // also handles defensive fixtures without creating an unaligned reference.
        let item = unsafe { record.ExtendedData.add(index).read_unaligned() };
        let size = usize::from(item.DataSize);
        if size != 0 && item.DataPtr == 0 {
            return Err(CopyError::Malformed);
        }
        extended_length = extended_length.checked_add(size).ok_or_else(invalid)?;
        if extended_length > 65535 {
            return Err(CopyError::Malformed);
        }
        length = align_payload(length).ok_or_else(invalid)?;
        offsets[index] = length;
        length = length.checked_add(size).ok_or_else(invalid)?;
        items[index] = item;
    }
    let mut buffer = pool.acquire(length).ok_or(CopyError::Exhausted)?;
    if data_length != 0 {
        // SAFETY: The validated nonempty source is readable by caller contract.
        // The exclusive pool slot has at least length >= data_length bytes;
        // native source and owned slot do not overlap. u8 requires alignment 1.
        unsafe {
            std::ptr::copy_nonoverlapping(
                record.UserData.cast::<u8>(),
                buffer.bytes_mut(),
                data_length,
            );
        }
    }
    for index in 0..count {
        let mut item = items[index];
        let size = usize::from(item.DataSize);
        if size != 0 {
            // SAFETY: Caller guarantees readable item storage. Preflight checked
            // offset + size <= acquired capacity, and each offset is 8-aligned.
            // The independent slot cannot overlap a native ETW input buffer.
            unsafe {
                std::ptr::copy_nonoverlapping(
                    item.DataPtr as *const u8,
                    buffer.bytes_mut().add(offsets[index]),
                    size,
                );
            }
        }
        item.DataPtr = 0;
        buffer.set_item(index, item, offsets[index]);
    }
    Ok(RawEtwEvent {
        header: record.EventHeader,
        buffer_context: record.BufferContext,
        data_length: record.UserDataLength,
        extended_count: record.ExtendedDataCount,
        buffer,
    })
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

#[derive(Clone, Copy)]
struct PropertyType {
    input: i32,
    output: i32,
}
struct Schema {
    plan: Option<DecodePlan>,
    tcp: PropertyType,
    local: PropertyType,
    peer: PropertyType,
    port: PropertyType,
    packet: PropertyType,
}

/// Reuses initialized property and text storage on the parser thread.
pub(crate) struct Parser {
    // Owned plans permit borrowing cache hits without allocation or refcount work.
    // Only events 256 and 257 reach this cache, usually with one version each, so a
    // linear scan over at most MAX_CACHED_SCHEMAS entries beats hashing.
    schemas: Vec<((u16, u8), Schema)>,
    property: [u8; 512],
    // At most 255 nonterminating UTF-16 units, each requiring at most 3 UTF-8 bytes.
    text: [u8; 768],
}

impl Parser {
    pub(crate) fn new() -> Self {
        Self {
            schemas: Vec::with_capacity(MAX_CACHED_SCHEMAS),
            property: [0; 512],
            text: [0; 768],
        }
    }

    /// Stores a schema, clearing the cache first when it is full.
    /// Capacity was reserved at construction, so insertion never reallocates.
    fn insert_schema(&mut self, key: (u16, u8), schema: Schema) {
        if self.schemas.len() >= MAX_CACHED_SCHEMAS {
            self.schemas.clear();
        }
        self.schemas.push((key, schema));
    }

    /// Fills encoder storage from named properties while consuming the event lease.
    /// The returned bytes borrow only the encoder; the lease ends before return.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "Consumes the queued record and returns its lease at the parser boundary."
    )]
    pub(crate) fn parse<'enc>(
        &mut self,
        event: RawEtwEvent,
        encoder: &'enc mut Encoder,
    ) -> Result<Option<&'enc [u8]>> {
        // Metadata and filters can fail before encode_with gets a chance to clear.
        encoder.clear();
        let id = event.header.EventDescriptor.Id;
        if !dns_provider(&event.header.ProviderId) || !supported(id) {
            return Ok(None);
        }
        let peer_name = if id == 256 {
            PropertyName::Source
        } else {
            PropertyName::Destination
        };
        let key = (id, event.header.EventDescriptor.Version);
        if let Some((_, schema)) = self.schemas.iter().find(|(cached, _)| *cached == key) {
            return parse_schema(
                schema,
                &event,
                peer_name,
                &mut self.property,
                &mut self.text,
                encoder,
            )
            .map(Some);
        }
        let schema = event
            .with_record(|record| load_schema(record, peer_name))
            .with_context(|| format!("DNS event {id} version {} schema", key.1))?;
        self.insert_schema(key, schema);
        let schema = self
            .schemas
            .last()
            .map(|(_, schema)| schema)
            .context("missing cached DNS schema")?;
        parse_schema(
            schema,
            &event,
            peer_name,
            &mut self.property,
            &mut self.text,
            encoder,
        )
        .map(Some)
    }
}

impl RawEtwEvent {
    fn payload(&self) -> &[u8] {
        // SAFETY: copy_record initialized precisely data_length bytes at offset zero
        // of this exclusive lease. The borrow cannot outlive the retained event.
        unsafe { std::slice::from_raw_parts(self.buffer.bytes(), usize::from(self.data_length)) }
    }

    /// Builds a scoped TDH view while this event retains its exclusive lease.
    /// The record and repaired raw pointers are valid only during `operation`.
    fn with_record<T>(&self, operation: impl FnOnce(&EVENT_RECORD) -> T) -> T {
        let count = usize::from(self.extended_count);
        let mut extended = *self.buffer.items();
        for (index, item) in extended.iter_mut().take(count).enumerate() {
            item.DataPtr = if item.DataSize == 0 {
                0
            } else {
                // SAFETY: copy_record checked and stored this aligned in-bounds offset;
                // self retains the exclusive lease throughout every synchronous TDH call.
                unsafe { self.buffer.bytes().add(self.buffer.offset(index)) as u64 }
            };
        }
        let record = EVENT_RECORD {
            EventHeader: self.header,
            BufferContext: self.buffer_context,
            ExtendedDataCount: self.extended_count,
            UserDataLength: self.data_length,
            ExtendedData: if count == 0 {
                null_mut()
            } else {
                extended.as_mut_ptr()
            },
            UserData: if self.data_length == 0 {
                null_mut()
            } else {
                self.buffer.bytes().cast_mut().cast()
            },
            UserContext: null_mut(),
        };
        operation(&record)
    }
}

fn parse_schema<'enc>(
    schema: &Schema,
    event: &RawEtwEvent,
    peer_name: PropertyName,
    property: &mut [u8; 512],
    text: &mut [u8; 768],
    encoder: &'enc mut Encoder,
) -> Result<&'enc [u8]> {
    if let Some(plan) = &schema.plan {
        parse_fast(schema, plan, &event.header, event.payload(), text, encoder)
    } else {
        event.with_record(|record| {
            parse_fallback(schema, record, peer_name, property, text, encoder)
        })
    }
}

fn parse_fast<'enc>(
    schema: &Schema,
    plan: &DecodePlan,
    header: &EVENT_HEADER,
    payload: &[u8],
    text: &mut [u8; 768],
    encoder: &'enc mut Encoder,
) -> Result<&'enc [u8]> {
    // A supported malformed payload is rejected here, never retried with TDH.
    let decoded = plan
        .decode(payload)
        .map_err(|error| anyhow!("invalid DNS payload: {error:?}"))?;
    let tcp = integer(decoded.tcp, schema.tcp)?;
    ensure!(tcp <= 1, "invalid TCP Boolean");
    let port = u16::try_from(integer(decoded.port, schema.port)?).context("Port exceeds u16")?;
    ensure!(
        decoded.local.len() <= 512 && decoded.peer.len() <= 512,
        "DNS address exceeds size limit"
    );
    let local = address(decoded.local, schema.local, text)?;
    let peer = address(decoded.peer, schema.peer, text)?;
    let metadata = RawDnsEvent {
        event_id: header.EventDescriptor.Id,
        timestamp: timestamp(header.TimeStamp)?,
        transport: if tcp == 1 {
            Transport::Tcp
        } else {
            Transport::Udp
        },
        peer_address: observed_address(peer),
        peer_port: observed_port(port),
        local_address: observed_address(local),
        local_port: None,
    };
    // The event lease outlives this copy into the final protobuf range;
    // parse consumes/releases the event before returning bytes to output.
    encoder.encode_with(metadata, decoded.packet.len(), |destination| {
        destination.copy_from_slice(decoded.packet);
        Ok(())
    })
}

fn parse_fallback<'enc>(
    schema: &Schema,
    record: &EVENT_RECORD,
    peer_name: PropertyName,
    scratch: &mut [u8; 512],
    text: &mut [u8; 768],
    encoder: &'enc mut Encoder,
) -> Result<&'enc [u8]> {
    let id = record.EventHeader.EventDescriptor.Id;
    let tcp = integer(property(record, PropertyName::Tcp, 4, scratch)?, schema.tcp)?;
    ensure!(tcp <= 1, "invalid TCP Boolean");
    let port = integer(
        property(record, PropertyName::Port, 4, scratch)?,
        schema.port,
    )?;
    let port = u16::try_from(port).context("Port exceeds u16")?;
    let local = address(
        property(record, PropertyName::InterfaceIp, 512, scratch)?,
        schema.local,
        text,
    )?;
    let peer = address(
        property(record, peer_name, 512, scratch)?,
        schema.peer,
        text,
    )?;
    ensure!(
        schema.packet.input == TDH_INTYPE_BINARY,
        "PacketData is not binary"
    );
    let packet_length = property_size(record, PropertyName::PacketData, 65535)?;
    let metadata = RawDnsEvent {
        event_id: id,
        timestamp: timestamp(record.EventHeader.TimeStamp)?,
        transport: if tcp == 1 {
            Transport::Tcp
        } else {
            Transport::Udp
        },
        peer_address: observed_address(peer),
        peer_port: observed_port(port),
        local_address: observed_address(local),
        local_port: None,
    };
    // Both the native view and pooled backing storage outlive this synchronous
    // fill. TDH writes directly into the initialized final protobuf range.
    encoder.encode_with(metadata, packet_length, |destination| {
        read_property(record, PropertyName::PacketData, destination)
    })
}

// DNS Server can report wildcard interface addresses and Port=0 for recursive
// traffic. These mean unavailable metadata, not observed socket endpoints.
fn observed_address(address: IpAddr) -> Option<IpAddr> {
    (!address.is_unspecified()).then_some(address)
}

fn observed_port(port: u16) -> Option<u16> {
    (port != 0).then_some(port)
}

fn load_schema(record: &EVENT_RECORD, peer_name: PropertyName) -> Result<Schema> {
    let mut size = 0;
    #[cfg(test)]
    tdh_tick(0);
    // SAFETY: record and all its backing buffers remain alive for this TDH call.
    let status = unsafe { TdhGetEventInformation(record, 0, null(), null_mut(), &raw mut size) };
    ensure!(
        status == ERROR_INSUFFICIENT_BUFFER,
        "TDH metadata size query failed: {}",
        std::io::Error::from_raw_os_error(status.cast_signed())
    );
    ensure!(
        (size_of::<TRACE_EVENT_INFO>()..=MAX_METADATA).contains(&(size as usize)),
        "invalid TDH metadata size"
    );
    // u64 storage supplies the alignment required by TRACE_EVENT_INFO and its
    // flexible EVENT_PROPERTY_INFO array, unlike a cast from a byte Vec.
    let mut storage = vec![0u64; (size as usize).div_ceil(8)];
    let capacity = size;
    #[cfg(test)]
    tdh_tick(0);
    // SAFETY: storage is aligned, initialized, and at least size bytes long.
    let status = unsafe {
        TdhGetEventInformation(
            record,
            0,
            null(),
            storage.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    check_status(status, "TdhGetEventInformation")?;
    ensure!(
        size <= capacity && size as usize >= size_of::<TRACE_EVENT_INFO>(),
        "invalid TDH metadata result size"
    );
    // SAFETY: The successful API initialized this aligned header in bounds.
    let info = unsafe { &*storage.as_ptr().cast::<TRACE_EVENT_INFO>() };
    ensure!(
        info.DecodingSource == DecodingSourceXMLFile,
        "DNS provider requires a manifest schema"
    );
    ensure!(
        dns_provider(&info.ProviderGuid),
        "TDH returned a different provider schema"
    );
    ensure!(
        info.EventDescriptor.Id == record.EventHeader.EventDescriptor.Id
            && info.EventDescriptor.Version == record.EventHeader.EventDescriptor.Version,
        "TDH returned a different event schema"
    );
    let array_offset = offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray);
    let count = info.TopLevelPropertyCount as usize;
    validate_property_array(info.PropertyCount as usize, count, size as usize)?;
    // SAFETY: The offset has native struct alignment, and the count fits the
    // initialized TDH allocation as checked above.
    let properties = unsafe {
        std::slice::from_raw_parts(
            storage
                .as_ptr()
                .byte_add(array_offset)
                .cast::<EVENT_PROPERTY_INFO>(),
            count,
        )
    };
    // SAFETY: The successful TDH result initialized size bytes inside storage.
    let bytes = unsafe { std::slice::from_raw_parts(storage.as_ptr().cast::<u8>(), size as usize) };
    let find = |name| find_property(properties, bytes, name);
    let plan = normalize_fields(properties, bytes)?
        .and_then(|fields| DecodePlan::compile(&fields, peer_name.name()));
    Ok(Schema {
        plan,
        tcp: find(PropertyName::Tcp.name())?,
        local: find(PropertyName::InterfaceIp.name())?,
        peer: find(peer_name.name())?,
        port: find(PropertyName::Port.name())?,
        packet: find(PropertyName::PacketData.name())?,
    })
}

fn validate_property_array(total: usize, top_level: usize, size: usize) -> Result<()> {
    ensure!(top_level <= total, "invalid TDH property count");
    let end = total
        .checked_mul(size_of::<EVENT_PROPERTY_INFO>())
        .and_then(|length| offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray).checked_add(length))
        .context("TDH property array overflow")?;
    ensure!(end <= size, "TDH property array exceeds metadata");
    Ok(())
}

/// Normalize only known union layouts. Unsupported schemas retain TDH fallback.
fn normalize_fields(
    properties: &[EVENT_PROPERTY_INFO],
    bytes: &[u8],
) -> Result<Option<Vec<FieldSpec>>> {
    // Match the compiler's bounded grammar without allocating unbounded descriptors.
    if properties.len() > 256 {
        return Ok(None);
    }
    let mut fields = Vec::with_capacity(properties.len());
    for prop in properties {
        let offset = prop.NameOffset as usize;
        ensure!(
            offset < bytes.len() && offset.is_multiple_of(2),
            "invalid TDH property name offset"
        );
        // At most 256 UTF-16 units per normalized name; a longer native name is
        // legacy-decodable but cannot grow the bounded cold-path plan storage.
        let units = bytes[offset..].as_chunks::<2>().0;
        if !units.iter().take(257).any(|unit| *unit == [0, 0]) {
            if units.len() <= 256 {
                bail!("unterminated TDH property name");
            }
            return Ok(None);
        }
        let name = property_name(bytes, offset)?;
        if prop.Flags & (PropertyStruct | PropertyHasCustomSchema) != 0
            || prop.Flags.cast_unsigned() & !0x36 != 0
        {
            // Keep the name and unsupported flags so the compiler can check
            // duplicate targets while ignoring layouts strictly after its prefix.
            // No native union arm is interpreted for these descriptors.
            fields.push(FieldSpec {
                name,
                input: -1,
                output: -1,
                flags: prop.Flags.cast_unsigned(),
                count: 0,
                length: 0,
            });
            continue;
        }
        // SAFETY: The flags exclude struct/custom union layouts. Count and length
        // arms are selected independently according to their discriminating flags.
        let (native, count, length) = unsafe {
            (
                prop.Anonymous1.nonStructType,
                if prop.Flags & PropertyParamCount != 0 {
                    prop.Anonymous2.countPropertyIndex
                } else {
                    prop.Anonymous2.count
                },
                if prop.Flags & PropertyParamLength != 0 {
                    prop.Anonymous3.lengthPropertyIndex
                } else {
                    prop.Anonymous3.length
                },
            )
        };
        fields.push(FieldSpec {
            name,
            input: i32::from(native.InType),
            output: i32::from(native.OutType),
            flags: prop.Flags.cast_unsigned(),
            count,
            length,
        });
    }
    Ok(Some(fields))
}

fn property_name(bytes: &[u8], offset: usize) -> Result<String> {
    ensure!(
        offset < bytes.len() && offset.is_multiple_of(2),
        "invalid TDH property name offset"
    );
    let units = bytes[offset..].as_chunks::<2>().0;
    let length = units
        .iter()
        .position(|unit| *unit == [0, 0])
        .context("unterminated TDH property name")?;
    let units: Vec<u16> = units[..length]
        .iter()
        .map(|unit| u16::from_le_bytes(*unit))
        .collect();
    String::from_utf16(&units).context("invalid TDH property name UTF-16")
}

fn find_property(
    properties: &[EVENT_PROPERTY_INFO],
    bytes: &[u8],
    name: &str,
) -> Result<PropertyType> {
    for prop in properties {
        let offset = prop.NameOffset as usize;
        ensure!(
            offset < bytes.len() && offset.is_multiple_of(2),
            "invalid TDH property name offset"
        );
        let units: Vec<u16> = bytes[offset..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .take_while(|u| *u != 0)
            .collect();
        ensure!(
            offset + units.len() * 2 + 2 <= bytes.len(),
            "unterminated TDH property name"
        );
        if units.iter().copied().eq(name.encode_utf16()) {
            ensure!(
                prop.Flags & (PropertyStruct | PropertyHasCustomSchema | PropertyParamCount) == 0,
                "unsupported structured property {name}"
            );
            // SAFETY: Flags above select the nonStructType and fixed-count
            // union members in the documented EVENT_PROPERTY_INFO layout.
            let (native, count) = unsafe { (prop.Anonymous1.nonStructType, prop.Anonymous2.count) };
            ensure!(count <= 1, "unsupported property array {name}");
            return Ok(PropertyType {
                input: i32::from(native.InType),
                output: i32::from(native.OutType),
            });
        }
    }
    bail!("missing property {name}")
}

/// Closed set of manifest names; callers cannot supply an unterminated pointer.
#[derive(Clone, Copy)]
enum PropertyName {
    Tcp,
    Port,
    InterfaceIp,
    Source,
    Destination,
    PacketData,
}

impl PropertyName {
    fn name(self) -> &'static str {
        self.native().0
    }

    fn native(self) -> (&'static str, *const u16) {
        // w! emits static, nul-terminated UTF-16 storage at compile time. Each
        // arm binds the display name and native spelling as one private value.
        match self {
            Self::Tcp => ("TCP", windows_sys::core::w!("TCP")),
            Self::Port => ("Port", windows_sys::core::w!("Port")),
            Self::InterfaceIp => ("InterfaceIP", windows_sys::core::w!("InterfaceIP")),
            Self::Source => ("Source", windows_sys::core::w!("Source")),
            Self::Destination => ("Destination", windows_sys::core::w!("Destination")),
            Self::PacketData => ("PacketData", windows_sys::core::w!("PacketData")),
        }
    }

    fn descriptor(self) -> PROPERTY_DATA_DESCRIPTOR {
        PROPERTY_DATA_DESCRIPTOR {
            PropertyName: self.native().1 as u64,
            ArrayIndex: u32::MAX,
            Reserved: 0,
        }
    }
}

fn property_size(record: &EVENT_RECORD, name: PropertyName, maximum: usize) -> Result<usize> {
    let descriptor = name.descriptor();
    let mut size = 0;
    #[cfg(test)]
    tdh_tick(1);
    // SAFETY: The event lease retains all record backing data, the descriptor uses
    // a static terminated name, and the initialized size output lives through this call.
    let status =
        unsafe { TdhGetPropertySize(record, 0, null(), 1, &raw const descriptor, &raw mut size) };
    check_status(status, "TdhGetPropertySize")?;
    ensure!(
        size as usize <= maximum,
        "TDH property exceeds its size limit"
    );
    Ok(size as usize)
}

fn read_property(record: &EVENT_RECORD, name: PropertyName, bytes: &mut [u8]) -> Result<()> {
    let descriptor = name.descriptor();
    let size = u32::try_from(bytes.len()).context("TDH property length exceeds u32")?;
    #[cfg(test)]
    tdh_tick(2);
    // SAFETY: The exclusive initialized byte slice has exactly the validated TDH
    // length, alignment 1 suffices, and its owner and the event lease remain alive
    // through the synchronous native call. The property name is static and terminated.
    let status = unsafe {
        TdhGetProperty(
            record,
            0,
            null(),
            1,
            &raw const descriptor,
            size,
            bytes.as_mut_ptr(),
        )
    };
    check_status(status, "TdhGetProperty")
}

fn scratch_property(
    scratch: &mut [u8; 512],
    size: usize,
    read: impl FnOnce(&mut [u8]) -> Result<()>,
) -> Result<&[u8]> {
    let bytes = scratch
        .get_mut(..size)
        .context("TDH property exceeds scratch size")?;
    bytes.fill(0);
    read(bytes)?;
    Ok(bytes)
}

fn property<'a>(
    record: &EVENT_RECORD,
    name: PropertyName,
    maximum: usize,
    scratch: &'a mut [u8; 512],
) -> Result<&'a [u8]> {
    let size = property_size(record, name, maximum)?;
    scratch_property(scratch, size, |bytes| read_property(record, name, bytes))
}

fn integer(bytes: &[u8], kind: PropertyType) -> Result<u32> {
    // TDH's Port output type is explicitly network byte order.
    if kind.output == TDH_OUTTYPE_PORT {
        ensure!(kind.input == TDH_INTYPE_UINT16, "invalid Port input type");
        let bytes = <[u8; 2]>::try_from(bytes).context("invalid Port byte length")?;
        return Ok(u32::from(u16::from_be_bytes(bytes)));
    }
    match (kind.input, bytes) {
        (TDH_INTYPE_UINT8, [value]) => Ok(u32::from(*value)),
        (TDH_INTYPE_UINT16, [a, b]) => Ok(u32::from(u16::from_le_bytes([*a, *b]))),
        (TDH_INTYPE_UINT32 | TDH_INTYPE_BOOLEAN, [a, b, c, d]) => {
            Ok(u32::from_le_bytes([*a, *b, *c, *d]))
        }
        _ => bail!("unexpected integer property type or length"),
    }
}

fn address(bytes: &[u8], kind: PropertyType, scratch: &mut [u8; 768]) -> Result<IpAddr> {
    if kind.output == TDH_OUTTYPE_IPV4 && bytes.len() == 4 {
        return Ok(IpAddr::from(<[u8; 4]>::try_from(bytes)?));
    }
    if kind.output == TDH_OUTTYPE_IPV6 && bytes.len() == 16 {
        return Ok(IpAddr::from(<[u8; 16]>::try_from(bytes)?));
    }
    let text = match kind.input {
        TDH_INTYPE_UNICODESTRING => {
            ensure!(bytes.len().is_multiple_of(2), "odd Unicode address length");
            ensure!(bytes.len() <= 512, "Unicode address exceeds size limit");
            ensure!(bytes.ends_with(&[0, 0]), "unterminated Unicode address");
            let units = bytes[..bytes.len() - 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]));
            let mut length = 0;
            for character in char::decode_utf16(units) {
                let character = character.context("invalid address UTF-16")?;
                length += character.encode_utf8(&mut scratch[length..]).len();
            }
            std::str::from_utf8(&scratch[..length])?
        }
        TDH_INTYPE_ANSISTRING => {
            ensure!(bytes.last() == Some(&0), "unterminated address");
            std::str::from_utf8(&bytes[..bytes.len() - 1])?
        }
        _ => bail!("unsupported address property type"),
    };
    // Validate the complete string before ignoring routing context after `%`.
    // Interior NULs in the address prefix reach IP parsing and are rejected;
    // the suffix retains the manifest parser's existing opaque-text behavior.
    let address = text.split_once('%').map_or(text, |(address, _)| address);
    address.parse().context("invalid DNS event IP address")
}

fn timestamp(filetime: i64) -> Result<SystemTime> {
    const EPOCH: i64 = 116_444_736_000_000_000;
    let ticks = u64::try_from(filetime.checked_sub(EPOCH).context("timestamp overflow")?)
        .context("event timestamp predates Unix epoch")?;
    UNIX_EPOCH
        .checked_add(Duration::new(
            ticks / 10_000_000,
            (ticks % 10_000_000) as u32 * 100,
        ))
        .context("event timestamp exceeds SystemTime")
}

#[cfg(test)]
thread_local! {
    static TDH_CALLS: std::cell::Cell<[usize; 3]> = const { std::cell::Cell::new([0; 3]) };
}

#[cfg(test)]
fn tdh_tick(kind: usize) {
    TDH_CALLS.with(|calls| {
        let mut counts = calls.get();
        counts[kind] += 1;
        calls.set(counts);
    });
}

#[cfg(test)]
pub(crate) fn assert_no_allocations(operation: impl FnOnce()) {
    tests::assert_no_allocations(operation);
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Diagnostics::Etw::EVENT_DESCRIPTOR;

    fn offset_u32(offset: usize) -> u32 {
        u32::try_from(offset).expect("test offsets fit u32")
    }

    /// A query result shaped like this program's own real-time session.
    fn own_query(config: &SessionConfig) -> QueryProperties {
        let mut query = QueryProperties::new();
        query.properties.LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        query.properties.BufferSize = config.buffer_kib;
        query.properties.LogFileNameOffset = offset_u32(offset_of!(QueryProperties, log_file_name));
        query
    }

    fn query_result(edit: impl FnOnce(&mut QueryProperties)) -> QueryProperties {
        let mut query = own_query(&SessionConfig::default());
        edit(&mut query);
        query
    }

    /// Fingerprint check against the default configuration.
    fn reclaimable_by_default(query: &QueryProperties) -> bool {
        reclaimable(query, &SessionConfig::default())
    }

    #[test]
    fn query_properties_set_name_offsets_and_size() {
        let query = QueryProperties::new();
        assert_eq!(
            query.properties.Wnode.BufferSize,
            offset_u32(size_of::<QueryProperties>())
        );
        assert_eq!(query.properties.Wnode.Flags, WNODE_FLAG_TRACED_GUID);
        assert_eq!(
            query.properties.LoggerNameOffset,
            offset_u32(offset_of!(QueryProperties, logger_name))
        );
        assert_eq!(
            query.properties.LogFileNameOffset,
            offset_u32(offset_of!(QueryProperties, log_file_name))
        );
        assert!(query.logger_name.iter().all(|unit| *unit == 0));
        assert!(query.log_file_name.iter().all(|unit| *unit == 0));
    }

    #[test]
    fn fingerprint_accepts_own_session_shape() {
        assert!(reclaimable_by_default(
            &own_query(&SessionConfig::default())
        ));
        // Offset 0 is the other shape ETW may report for a session with no log file.
        assert!(reclaimable_by_default(&query_result(|q| {
            q.properties.LogFileNameOffset = 0;
        })));
        // ETW may normalize counts and flush timer.
        assert!(reclaimable_by_default(&query_result(|q| {
            q.properties.MinimumBuffers = 8;
            q.properties.NumberOfBuffers = 8;
            q.properties.FlushTimer = 0;
        })));
    }

    #[test]
    fn fingerprint_rejects_foreign_sessions() {
        // Nonempty log file name at the log-file field offset.
        assert!(!reclaimable_by_default(&query_result(|q| {
            q.log_file_name[0] = u16::from(b'C');
        })));
        // Offsets elsewhere: the logger-name field, the header, misaligned, or
        // beyond the buffer.
        for offset in [
            4,
            offset_of!(QueryProperties, logger_name),
            offset_of!(QueryProperties, log_file_name) + 1,
            size_of::<QueryProperties>(),
        ] {
            assert!(!reclaimable_by_default(&query_result(|q| {
                q.properties.LogFileNameOffset = offset_u32(offset);
            })));
        }
        // Real-time mode is required.
        assert!(!reclaimable_by_default(&query_result(|q| {
            q.properties.LogFileMode = 0;
        })));
        // Each file-mode bit, and each other logger mode, is forbidden.
        for mode in [
            EVENT_TRACE_FILE_MODE_SEQUENTIAL,
            EVENT_TRACE_FILE_MODE_CIRCULAR,
            EVENT_TRACE_FILE_MODE_APPEND,
            EVENT_TRACE_FILE_MODE_NEWFILE,
            EVENT_TRACE_SYSTEM_LOGGER_MODE,
            EVENT_TRACE_BUFFERING_MODE,
            EVENT_TRACE_PRIVATE_LOGGER_MODE,
        ] {
            assert!(!reclaimable_by_default(&query_result(|q| {
                q.properties.LogFileMode = EVENT_TRACE_REAL_TIME_MODE | mode;
            })));
        }
    }

    #[test]
    fn query_outcome_classifies_statuses() {
        assert_eq!(query_outcome(ERROR_SUCCESS), QueryOutcome::Inspect);
        assert_eq!(
            query_outcome(ERROR_WMI_INSTANCE_NOT_FOUND),
            QueryOutcome::Vanished
        );
        for status in [ERROR_ALREADY_EXISTS, ERROR_CANCELLED, 5, u32::MAX] {
            assert_eq!(query_outcome(status), QueryOutcome::Failed);
        }
    }

    #[test]
    fn session_properties_write_configured_values() {
        let config = SessionConfig::new(128, 512, 5).expect("valid configuration");
        let properties = SessionProperties::new(&config);
        assert_eq!(properties.properties.BufferSize, 128);
        assert_eq!(properties.properties.MaximumBuffers, 512);
        assert_eq!(properties.properties.FlushTimer, 5);
        assert_eq!(properties.properties.MinimumBuffers, 4);

        let defaults = SessionProperties::new(&SessionConfig::default());
        assert_eq!(defaults.properties.BufferSize, 16);
        assert_eq!(defaults.properties.MaximumBuffers, 12);
        assert_eq!(defaults.properties.FlushTimer, 1);
    }

    #[test]
    fn fingerprint_compares_against_configured_buffer_size() {
        let config = SessionConfig::new(128, 512, 5).expect("valid configuration");
        assert!(reclaimable(&own_query(&config), &config));
        // A session leaked under the default 16 KiB size is not reclaimed by a
        // process configured for 128 KiB, and vice versa.
        assert!(!reclaimable(&own_query(&SessionConfig::default()), &config));
        assert!(!reclaimable(&own_query(&config), &SessionConfig::default()));
    }

    fn utf16_nul(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Stops a session by name on drop, so ignored tests clean up even when an
    /// assertion fails. Optionally deletes the log file the session wrote.
    struct StopSessionOnDrop {
        name: Vec<u16>,
        log_file: Option<std::path::PathBuf>,
    }

    impl Drop for StopSessionOnDrop {
        fn drop(&mut self) {
            let mut properties = QueryProperties::new();
            // SAFETY: `name` is NUL-terminated and `properties` is a sized, aligned
            // buffer that outlives this synchronous call; a zero handle selects
            // lookup by name.
            unsafe {
                ControlTraceW(
                    CONTROLTRACE_HANDLE::default(),
                    self.name.as_ptr(),
                    &raw mut properties.properties,
                    EVENT_TRACE_CONTROL_STOP,
                );
            }
            if let Some(path) = &self.log_file {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    /// Start properties with logger-name and log-file-name storage, for tests
    /// that create a session with a log file.
    #[repr(C)]
    struct StartPropertiesWithLogFile {
        properties: EVENT_TRACE_PROPERTIES,
        logger_name: [u16; 1025],
        log_file_name: [u16; 1025],
    }

    /// Prints ETW's actual QUERY view of a session, so a live
    /// `--ignored --nocapture` run records the behavior the fingerprint relies on.
    fn print_query(label: &str, name: &[u16]) {
        let mut query = QueryProperties::new();
        // SAFETY: `name` is NUL-terminated and `query` is a sized, aligned buffer
        // that outlives this synchronous call; a zero handle selects lookup by name.
        let status = unsafe {
            ControlTraceW(
                CONTROLTRACE_HANDLE::default(),
                name.as_ptr(),
                &raw mut query.properties,
                EVENT_TRACE_CONTROL_QUERY,
            )
        };
        eprintln!(
            "{label}: status={status} LogFileMode={:#x} BufferSize={} \
             LogFileNameOffset={} log_file_name[0]={}",
            query.properties.LogFileMode,
            query.properties.BufferSize,
            query.properties.LogFileNameOffset,
            query.log_file_name[0]
        );
    }

    /// Starts a session with a log file and the given mode, returning the status.
    fn start_log_file_session(
        name: &[u16],
        config: &SessionConfig,
        mode: u32,
        log_units: &[u16],
    ) -> u32 {
        let mut properties = StartPropertiesWithLogFile {
            properties: EVENT_TRACE_PROPERTIES::default(),
            logger_name: [0; 1025],
            log_file_name: [0; 1025],
        };
        properties.properties.Wnode.BufferSize =
            offset_u32(size_of::<StartPropertiesWithLogFile>());
        properties.properties.Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        properties.properties.BufferSize = config.buffer_kib;
        properties.properties.LogFileMode = mode;
        properties.properties.LoggerNameOffset =
            offset_u32(offset_of!(StartPropertiesWithLogFile, logger_name));
        properties.properties.LogFileNameOffset =
            offset_u32(offset_of!(StartPropertiesWithLogFile, log_file_name));
        properties.log_file_name[..log_units.len()].copy_from_slice(log_units);
        let mut session = CONTROLTRACE_HANDLE::default();
        // SAFETY: `name` is NUL-terminated, and `properties` is a local, sized,
        // aligned buffer whose name storage holds the logger and log-file paths.
        // It outlives this synchronous call.
        unsafe {
            StartTraceW(
                &raw mut session,
                name.as_ptr(),
                &raw mut properties.properties,
            )
        }
    }

    /// Leaks a session that writes a log file and asserts reclaim refuses it.
    fn assert_log_file_session_refused(name_text: &str, mode: u32) {
        let name = utf16_nul(name_text);
        let config = SessionConfig::default();
        let log_path = std::env::temp_dir().join(format!("{name_text}.etl"));
        let log_units = utf16_nul(&log_path.to_string_lossy());
        assert!(log_units.len() <= 1025, "temp log path fits the buffer");
        let _cleanup = StopSessionOnDrop {
            name: name.clone(),
            log_file: Some(log_path),
        };

        let status = start_log_file_session(&name, &config, mode, &log_units);
        assert_eq!(status, ERROR_SUCCESS, "log-file session should start");
        print_query("log-file session", &name);

        let counters = CaptureCounters::default();
        let error = reclaim_stale_session(&name, name_text, &counters, &config)
            .expect_err("a session that writes a log file must not be reclaimed");
        assert!(
            error
                .to_string()
                .contains("does not match this program's session shape"),
            "unexpected error: {error}"
        );
        assert_eq!(counters.stale_sessions_reclaimed.load(Ordering::Relaxed), 0);
    }

    #[test]
    #[ignore = "requires an elevated session"]
    fn flush_succeeds_on_live_real_time_session() {
        const NAME: &str = "EtwDnstap-FlushTest";
        let name = utf16_nul(NAME);
        let config = SessionConfig::default();
        let _cleanup = StopSessionOnDrop {
            name: name.clone(),
            log_file: None,
        };
        let (status, session) = start_session(&name, &config);
        assert_eq!(status, ERROR_SUCCESS, "session should start");
        assert_eq!(flush_session(session), Ok(()), "flush should succeed");
        assert_eq!(
            flush_session(session),
            Ok(()),
            "repeat flush should succeed"
        );
    }

    #[test]
    #[ignore = "requires an elevated session"]
    fn reclaim_replaces_leaked_real_time_session() {
        const NAME: &str = "EtwDnstap-ReclaimTest-Replace";
        let name = utf16_nul(NAME);
        // A non-default size exercises exact BufferSize matching beyond 64 KiB.
        let config = SessionConfig::new(128, 256, 1).expect("valid configuration");
        let _cleanup = StopSessionOnDrop {
            name: name.clone(),
            log_file: None,
        };
        // Leak a real-time-only session with this program's shape.
        let (status, _leaked) = start_session(&name, &config);
        assert_eq!(status, ERROR_SUCCESS, "leaking a session should start");
        print_query("leaked session", &name);

        let (status, _duplicate) = start_session(&name, &config);
        assert_eq!(
            status, ERROR_ALREADY_EXISTS,
            "the leaked session must block a second start"
        );

        let counters = CaptureCounters::default();
        reclaim_stale_session(&name, NAME, &counters, &config).expect("reclaim succeeds");
        let (status, _session) = start_session(&name, &config);
        assert_eq!(status, ERROR_SUCCESS, "retry after reclaim should start");
        print_query("restarted session", &name);
        assert_eq!(counters.stale_sessions_reclaimed.load(Ordering::Relaxed), 1);
    }

    /// Real-time mode only plus a log-file name: the case where ETW writes a
    /// sequential file without any file-mode bit.
    #[test]
    #[ignore = "requires an elevated session"]
    fn reclaim_refuses_session_that_writes_log_file() {
        assert_log_file_session_refused(
            "EtwDnstap-ReclaimTest-LogFile",
            EVENT_TRACE_REAL_TIME_MODE,
        );
    }

    #[test]
    #[ignore = "requires an elevated session"]
    fn reclaim_refuses_sequential_log_file_session() {
        assert_log_file_session_refused(
            "EtwDnstap-ReclaimTest-Sequential",
            EVENT_TRACE_REAL_TIME_MODE | EVENT_TRACE_FILE_MODE_SEQUENTIAL,
        );
    }

    /// Runs the decision with canned statuses, recording whether stop ran.
    fn run_decision(
        query_status: u32,
        query_edit: impl FnOnce(&mut QueryProperties),
        stop_status: u32,
    ) -> (Result<()>, bool, u64) {
        let counters = CaptureCounters::default();
        let config = SessionConfig::default();
        let mut stopped = false;
        let result = reclaim_decision(
            "test",
            &counters,
            &config,
            |properties| {
                *properties = own_query(&config);
                query_edit(properties);
                query_status
            },
            |_| {
                stopped = true;
                stop_status
            },
        );
        let count = counters.stale_sessions_reclaimed.load(Ordering::Relaxed);
        (result, stopped, count)
    }

    #[test]
    fn reclaim_decision_vanished_session_neither_stops_nor_counts() {
        let (result, stopped, count) =
            run_decision(ERROR_WMI_INSTANCE_NOT_FOUND, |_| {}, ERROR_SUCCESS);
        assert!(result.is_ok());
        assert!(!stopped);
        assert_eq!(count, 0);
    }

    #[test]
    fn reclaim_decision_failed_query_errors_without_stop() {
        let (result, stopped, count) = run_decision(5, |_| {}, ERROR_SUCCESS);
        let error = result.expect_err("query failure is an error");
        assert!(error.to_string().contains("query existing ETW session"));
        assert!(!stopped);
        assert_eq!(count, 0);
    }

    #[test]
    fn reclaim_decision_shape_mismatch_errors_without_stop() {
        let (result, stopped, count) = run_decision(
            ERROR_SUCCESS,
            |q| q.log_file_name[0] = u16::from(b'C'),
            ERROR_SUCCESS,
        );
        let error = result.expect_err("mismatch is an error");
        assert!(
            error
                .to_string()
                .contains("does not match this program's session shape")
        );
        assert!(!stopped);
        assert_eq!(count, 0);
    }

    #[test]
    fn reclaim_decision_counts_stop_success_and_more_data() {
        for stop_status in [ERROR_SUCCESS, ERROR_MORE_DATA] {
            let (result, stopped, count) = run_decision(ERROR_SUCCESS, |_| {}, stop_status);
            assert!(result.is_ok());
            assert!(stopped);
            assert_eq!(count, 1, "stop status {stop_status}");
        }
    }

    #[test]
    fn reclaim_decision_does_not_count_session_that_vanished_before_stop() {
        let (result, stopped, count) =
            run_decision(ERROR_SUCCESS, |_| {}, ERROR_WMI_INSTANCE_NOT_FOUND);
        assert!(result.is_ok());
        assert!(stopped);
        assert_eq!(count, 0);
    }

    #[test]
    fn reclaim_decision_stop_failure_errors_without_count() {
        let (result, stopped, count) = run_decision(ERROR_SUCCESS, |_| {}, 5);
        let error = result.expect_err("stop failure is an error");
        assert!(error.to_string().contains("stopping stale ETW session"));
        assert!(stopped);
        assert_eq!(count, 0);
    }

    #[test]
    fn session_config_enforces_ranges_and_accepts_max_buffer_product() {
        assert!(SessionConfig::new(4, 4, 1).is_ok());
        assert!(SessionConfig::new(128, 4096, 1).is_ok(), "512 MiB peak");
        assert!(SessionConfig::new(128, 256, 1).is_ok());
        for rejected in [3, 132, 256, 1024] {
            assert!(SessionConfig::new(rejected, 256, 1).is_err(), "{rejected}");
        }
        assert!(SessionConfig::new(64, 3, 1).is_err());
        assert!(SessionConfig::new(64, 4097, 1).is_err());
        assert!(SessionConfig::new(64, 256, 0).is_err());
        assert!(SessionConfig::new(64, 256, 61).is_err());
        assert!(
            SessionConfig::new(65, 256, 1).is_err(),
            "not a multiple of 4"
        );
        assert!(
            SessionConfig::new(66, 256, 1).is_err(),
            "not a multiple of 4"
        );
        assert!(SessionConfig::new(68, 256, 1).is_ok());
    }

    #[test]
    fn stop_outcome_classifies_statuses() {
        for status in [ERROR_SUCCESS, ERROR_MORE_DATA, ERROR_WMI_INSTANCE_NOT_FOUND] {
            assert_eq!(stop_outcome(status), StopOutcome::Stopped);
        }
        for status in [ERROR_ALREADY_EXISTS, ERROR_CANCELLED, 5, u32::MAX] {
            assert_eq!(stop_outcome(status), StopOutcome::Failed);
        }
    }

    #[test]
    fn event_id_filter_payload_bytes_are_exact() {
        let payload = EventIdFilterPayload::new();
        // SAFETY: The payload is an 8-byte repr(C) struct without padding, so
        // every byte is initialized.
        let bytes = unsafe {
            std::slice::from_raw_parts(
                std::ptr::from_ref(&payload).cast::<u8>(),
                size_of::<EventIdFilterPayload>(),
            )
        };
        assert_eq!(bytes, [1, 0, 2, 0, 0x00, 0x01, 0x01, 0x01]);
    }

    #[test]
    fn event_id_descriptor_and_parameters_describe_payload() {
        let payload = EventIdFilterPayload::new();
        let mut descriptor = event_id_descriptor(&payload);
        assert_eq!(descriptor.Type, EVENT_FILTER_TYPE_EVENT_ID);
        assert_eq!(descriptor.Size, 8);
        assert_eq!(descriptor.Ptr, std::ptr::from_ref(&payload) as u64);

        let descriptor_ptr = &raw mut descriptor;
        let parameters = enable_parameters(descriptor_ptr);
        assert_eq!(parameters.Version, ENABLE_TRACE_PARAMETERS_VERSION_2);
        assert_eq!(parameters.EnableProperty, 0x10);
        assert_eq!(
            parameters.EnableProperty,
            EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0
        );
        assert_eq!(parameters.FilterDescCount, 1);
        assert_eq!(parameters.EnableFilterDesc, descriptor_ptr);
    }

    fn keyword_list(entries: &[(&str, u64)]) -> Vec<(String, u64)> {
        entries
            .iter()
            .map(|&(name, value)| (name.to_owned(), value))
            .collect()
    }

    fn assert_fallback(choice: &KeywordChoice, expected_reason: &str) {
        match choice {
            KeywordChoice::Fallback { reason } => {
                assert!(reason.contains(expected_reason), "reason: {reason}");
            }
            KeywordChoice::Narrowed { match_any } => {
                panic!("expected fallback, got narrowed {match_any:#x}")
            }
        }
        assert_eq!(
            choice.selection(),
            KeywordSelection {
                match_any: 0x8000_0000_0000_0000,
                match_all: 0,
                fallback: true,
            }
        );
    }

    #[test]
    fn select_keywords_narrows_two_single_bits() {
        let keywords = keyword_list(&[
            ("RECURSE_QUERY_OUT", 0x10),
            ("RESPONSE_SUCCESS", 0x2),
            ("QUERY_RECEIVED", 0x1),
            ("HIGH_PROVIDER_BIT", 0x0000_8000_0000_0000),
        ]);
        let choice = select_keywords(Ok(&keywords));
        assert_eq!(choice, KeywordChoice::Narrowed { match_any: 0x3 });
        assert_eq!(
            choice.selection(),
            KeywordSelection {
                match_any: 0x3,
                match_all: 0,
                fallback: false,
            }
        );

        let keywords = keyword_list(&[
            ("QUERY_RECEIVED", 0x0000_8000_0000_0000),
            ("RESPONSE_SUCCESS", 0x4),
        ]);
        assert_eq!(
            select_keywords(Ok(&keywords)),
            KeywordChoice::Narrowed {
                match_any: 0x0000_8000_0000_0004
            }
        );
    }

    #[test]
    fn select_keywords_falls_back_on_missing_name() {
        let keywords = keyword_list(&[("QUERY_RECEIVED", 0x1), ("query_received", 0x2)]);
        assert_fallback(
            &select_keywords(Ok(&keywords)),
            "RESPONSE_SUCCESS is not defined",
        );
        assert_fallback(&select_keywords(Ok(&[])), "QUERY_RECEIVED is not defined");
    }

    #[test]
    fn select_keywords_falls_back_on_multi_bit_or_zero_value() {
        let keywords = keyword_list(&[("QUERY_RECEIVED", 0x1), ("RESPONSE_SUCCESS", 0x6)]);
        assert_fallback(&select_keywords(Ok(&keywords)), "not a single bit");
        let keywords = keyword_list(&[("QUERY_RECEIVED", 0), ("RESPONSE_SUCCESS", 0x2)]);
        assert_fallback(&select_keywords(Ok(&keywords)), "not a single bit");
    }

    #[test]
    fn select_keywords_falls_back_on_reserved_range_value() {
        for value in [
            0x8000_0000_0000_0001,
            0x8000_0000_0000_0000,
            0x0001_0000_0000_0000,
        ] {
            let keywords = keyword_list(&[("QUERY_RECEIVED", value), ("RESPONSE_SUCCESS", 0x2)]);
            assert_fallback(&select_keywords(Ok(&keywords)), "provider-defined range");
        }
    }

    #[test]
    fn select_keywords_falls_back_on_duplicate_bits_or_names() {
        let keywords = keyword_list(&[("QUERY_RECEIVED", 0x2), ("RESPONSE_SUCCESS", 0x2)]);
        assert_fallback(&select_keywords(Ok(&keywords)), "share bit 0x2");
        let keywords = keyword_list(&[
            ("QUERY_RECEIVED", 0x1),
            ("QUERY_RECEIVED", 0x4),
            ("RESPONSE_SUCCESS", 0x2),
        ]);
        assert_fallback(&select_keywords(Ok(&keywords)), "conflicting values");
        // The same name repeated with the same value is not ambiguous.
        let keywords = keyword_list(&[
            ("QUERY_RECEIVED", 0x1),
            ("QUERY_RECEIVED", 0x1),
            ("RESPONSE_SUCCESS", 0x2),
        ]);
        assert_eq!(
            select_keywords(Ok(&keywords)),
            KeywordChoice::Narrowed { match_any: 0x3 }
        );
    }

    #[test]
    fn select_keywords_falls_back_on_lookup_error() {
        let error = anyhow!("TDH keyword size query failed: 1168");
        assert_fallback(&select_keywords(Err(&error)), "size query failed: 1168");
    }

    #[test]
    fn unregistered_provider_keyword_query_falls_back() {
        let unregistered = GUID::from_u128(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
        let error = query_provider_keywords(&unregistered)
            .expect_err("an unregistered provider has no keyword metadata");
        let choice = select_keywords(Err(&error));
        assert_fallback(&choice, "");
        assert_eq!(choice.selection().match_any, 0x8000_0000_0000_0000);
    }

    /// Builds a `PROVIDER_FIELD_INFOARRAY` byte image: header, entries, then
    /// NUL-terminated UTF-16 names. `name_offset` may override the real offset.
    fn keyword_image(entries: &[(&str, u64)], name_offset: Option<u32>) -> Vec<u8> {
        let first = offset_of!(PROVIDER_FIELD_INFOARRAY, FieldInfoArray);
        let entry_size = size_of::<PROVIDER_FIELD_INFO>();
        let mut names = Vec::new();
        let mut offsets = Vec::new();
        for (name, _) in entries {
            offsets.push(offset_u32(first + entries.len() * entry_size + names.len()));
            for unit in name.encode_utf16().chain([0]) {
                names.extend_from_slice(&unit.to_le_bytes());
            }
        }
        let mut image = vec![0u8; first];
        image[..4].copy_from_slice(&offset_u32(entries.len()).to_le_bytes());
        for ((_, value), offset) in entries.iter().zip(offsets) {
            let mut entry = vec![0u8; entry_size];
            let name_at = offset_of!(PROVIDER_FIELD_INFO, NameOffset);
            entry[name_at..name_at + 4]
                .copy_from_slice(&name_offset.unwrap_or(offset).to_le_bytes());
            let value_at = offset_of!(PROVIDER_FIELD_INFO, Value);
            entry[value_at..value_at + 8].copy_from_slice(&value.to_le_bytes());
            image.extend_from_slice(&entry);
        }
        image.extend_from_slice(&names);
        image
    }

    #[test]
    fn parse_keyword_array_reads_names_and_values() {
        let image = keyword_image(
            &[
                ("QUERY_RECEIVED", 0x1),
                ("RESPONSE_SUCCESS", 0x2),
                ("CHANNEL", 0x8000_0000_0000_0000),
            ],
            None,
        );
        assert_eq!(
            parse_keyword_array(&image).expect("well-formed image"),
            keyword_list(&[
                ("QUERY_RECEIVED", 0x1),
                ("RESPONSE_SUCCESS", 0x2),
                ("CHANNEL", 0x8000_0000_0000_0000),
            ])
        );
        let empty = keyword_image(&[], None);
        assert_eq!(
            parse_keyword_array(&empty).expect("empty image"),
            keyword_list(&[])
        );
    }

    #[test]
    fn parse_keyword_array_rejects_malformed_images() {
        // Name offset past the end of the buffer.
        let image = keyword_image(&[("QUERY_RECEIVED", 0x1)], Some(10_000));
        assert!(parse_keyword_array(&image).is_err());
        // Name offset whose terminator lies past the end.
        let image = keyword_image(&[("QUERY_RECEIVED", 0x1)], None);
        assert!(parse_keyword_array(&image[..image.len() - 2]).is_err());
        // Name offset one unit before the end, so no NUL follows its last unit.
        let last_unit = offset_u32(image.len() - 2);
        let mut unterminated = keyword_image(&[("QUERY_RECEIVED", 0x1)], Some(last_unit));
        let end = unterminated.len();
        unterminated[end - 2] = b'A';
        assert!(parse_keyword_array(&unterminated).is_err());
        // Element count larger than the buffer can hold.
        let mut image = keyword_image(&[("QUERY_RECEIVED", 0x1)], None);
        image[..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(parse_keyword_array(&image).is_err());
        // Truncated header.
        assert!(parse_keyword_array(&[1, 0]).is_err());
        assert!(parse_keyword_array(&[]).is_err());
    }

    #[test]
    #[ignore = "requires the DNS Server role"]
    fn resolves_real_dns_server_keywords() {
        let keywords = query_provider_keywords(&PROVIDER).expect("resolve DNS Server keywords");
        for (name, value) in &keywords {
            println!("{name} = {value:#x}");
        }
        let choice = select_keywords(Ok(&keywords));
        println!("{choice:?} -> {:?}", choice.selection());
        assert!(matches!(choice, KeywordChoice::Narrowed { .. }));
    }

    #[test]
    fn supported_accepts_only_event_ids_256_and_257() {
        for id in 0..=u16::MAX {
            assert_eq!(supported(id), id == 256 || id == 257, "event id {id}");
        }
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    struct AllocCounts {
        allocations: u64,
        reallocations: u64,
        deallocations: u64,
    }

    thread_local! {
        static ALLOC_COUNTS: std::cell::Cell<AllocCounts> = const { std::cell::Cell::new(AllocCounts { allocations: 0, reallocations: 0, deallocations: 0 }) };
        static TRACK_ALLOCATIONS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    struct TrackingAllocator;

    fn allocation_tick(kind: u8) {
        if TRACK_ALLOCATIONS
            .try_with(std::cell::Cell::get)
            .unwrap_or(false)
        {
            let _updated = ALLOC_COUNTS.try_with(|counts| {
                let mut value = counts.get();
                match kind {
                    0 => value.allocations += 1,
                    1 => value.reallocations += 1,
                    _ => value.deallocations += 1,
                }
                counts.set(value);
            });
        }
    }

    // SAFETY: Every allocation/deallocation is forwarded unchanged to System.
    // Thread-local counter updates allocate no storage and retain no pointers.
    unsafe impl std::alloc::GlobalAlloc for TrackingAllocator {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            allocation_tick(0);
            // SAFETY: The allocator caller supplies a valid layout.
            unsafe { std::alloc::GlobalAlloc::alloc(&std::alloc::System, layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
            allocation_tick(0);
            // SAFETY: The allocator caller supplies a valid layout.
            unsafe { std::alloc::GlobalAlloc::alloc_zeroed(&std::alloc::System, layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: std::alloc::Layout) {
            allocation_tick(2);
            // SAFETY: Caller supplies the matching live System allocation/layout.
            unsafe {
                std::alloc::GlobalAlloc::dealloc(&std::alloc::System, pointer, layout);
            }
        }
        unsafe fn realloc(
            &self,
            pointer: *mut u8,
            layout: std::alloc::Layout,
            size: usize,
        ) -> *mut u8 {
            allocation_tick(1);
            // SAFETY: Caller supplies the live allocation and a valid new size.
            unsafe { std::alloc::GlobalAlloc::realloc(&std::alloc::System, pointer, layout, size) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: TrackingAllocator = TrackingAllocator;

    struct TrackingGuard;

    impl Drop for TrackingGuard {
        fn drop(&mut self) {
            TRACK_ALLOCATIONS.with(|track| track.set(false));
        }
    }

    fn measured(operation: impl FnOnce()) -> AllocCounts {
        ALLOC_COUNTS.with(|counts| counts.set(AllocCounts::default()));
        TRACK_ALLOCATIONS.with(|track| track.set(true));
        let guard = TrackingGuard;
        operation();
        drop(guard);
        ALLOC_COUNTS.with(std::cell::Cell::get)
    }

    pub(super) fn assert_no_allocations(operation: impl FnOnce()) {
        let counts = measured(operation);
        assert_eq!(
            counts,
            AllocCounts::default(),
            "unexpected allocator activity"
        );
    }

    fn allocator_fixtures() -> Vec<(&'static str, AllocCounts)> {
        let mut results = Vec::new();
        let mut fixture = Fixture::new(1, 1);
        let mut payload = [0x55u8; 512];
        let extra = [0x33u8; 17];
        let mut item = EVENT_HEADER_EXTENDED_DATA_ITEM {
            DataPtr: extra.as_ptr() as u64,
            DataSize: 17,
            ..Default::default()
        };
        fixture.record.UserData = payload.as_mut_ptr().cast();
        fixture.record.UserDataLength = 512;
        fixture.record.ExtendedData = &raw mut item;
        fixture.record.ExtendedDataCount = 1;
        results.push(("success", measured(|| fixture.invoke())));
        results.push(("channel_full", measured(|| fixture.invoke())));
        let event = fixture.receive.try_recv().expect("accepted event");
        let pool = Arc::clone(&fixture.context.pool);
        let recycled = thread::spawn(move || measured(|| drop(event)))
            .join()
            .expect("recycler joined");
        results.push(("recycle", recycled));
        let held: Vec<_> = (0..3)
            .map(|_| pool.acquire(1).expect("regular lease"))
            .collect();
        results.push(("pool_exhausted", measured(|| fixture.invoke())));
        drop(held);
        let mut jumbo_payload = vec![0x77u8; 4096];
        fixture.record.UserData = jumbo_payload.as_mut_ptr().cast();
        fixture.record.UserDataLength = 4096;
        results.push(("jumbo_success", measured(|| fixture.invoke())));
        drop(fixture.receive.try_recv().expect("jumbo event"));
        let jumbo = pool.acquire(4096).expect("jumbo slot returned");
        results.push(("jumbo_exhausted", measured(|| fixture.invoke())));
        drop(jumbo);
        fixture.record.UserData = payload.as_mut_ptr().cast();
        fixture.record.UserDataLength = 512;
        let (_, replacement) = crossbeam_channel::bounded(1);
        drop(std::mem::replace(&mut fixture.receive, replacement));
        results.push(("disconnected", measured(|| fixture.invoke())));
        fixture.record.UserDataLength = 1;
        fixture.record.UserData = null_mut();
        results.push(("malformed", measured(|| fixture.invoke())));
        fixture.record.UserDataLength = 512;
        fixture.record.UserData = payload.as_mut_ptr().cast();
        PRODUCER.with(|state| {
            let _borrow = state.borrow_mut();
            results.push(("reentrant", measured(|| fixture.invoke())));
        });
        PRODUCER.with(|state| {
            state.borrow_mut().as_mut().expect("producer").context += 1;
        });
        results.push(("tls_wrong", measured(|| fixture.invoke())));
        PRODUCER.with(|state| {
            state.borrow_mut().take();
        });
        results.push(("tls_missing", measured(|| fixture.invoke())));
        for (name, count) in &results {
            assert_eq!(
                *count,
                AllocCounts::default(),
                "allocator activity in {name}"
            );
        }
        assert_eq!(
            fixture.context.counters.received.load(Ordering::Relaxed),
            10
        );
        assert_eq!(
            fixture
                .context
                .counters
                .intake_dropped
                .load(Ordering::Relaxed),
            4
        );
        assert_eq!(
            fixture
                .context
                .counters
                .callback_errors
                .load(Ordering::Relaxed),
            4
        );
        assert!(pool.acquire(1).is_some(), "all leases recycled");
        results
    }

    #[test]
    fn fixed_property_descriptors_are_terminated_static_and_allocation_free() {
        let names = [
            PropertyName::Tcp,
            PropertyName::Port,
            PropertyName::InterfaceIp,
            PropertyName::Source,
            PropertyName::Destination,
            PropertyName::PacketData,
        ];
        let counts = measured(|| {
            for name in names {
                let (display, wide) = name.native();
                let descriptor = name.descriptor();
                assert_eq!(descriptor.ArrayIndex, u32::MAX);
                assert_eq!(descriptor.Reserved, 0);
                // Constant UTF-16 literals can be duplicated after inlining;
                // correctness depends on static lifetime and content, not identity.
                for pointer in [wide, descriptor.PropertyName as *const u16] {
                    for (index, expected) in display.encode_utf16().chain(Some(0)).enumerate() {
                        // SAFETY: Both pointers come from terminated w! literals.
                        // Prior iterations matched nonzero units; a shorter native
                        // spelling fails at its terminator before advancing.
                        let actual = unsafe { pointer.add(index).read() };
                        assert_eq!(actual, expected, "static name spelling or terminator");
                    }
                }
            }
        });
        assert_eq!(counts, AllocCounts::default());
    }

    #[test]
    fn scoped_native_record_empty_view_is_null_and_keeps_lease_exclusive() {
        let pool = PoolManager::new(1, 0).expect("pool");
        let guards = [
            pool.acquire(1).expect("first guard"),
            pool.acquire(1).expect("second guard"),
        ];
        let source = EVENT_RECORD::default();
        // SAFETY: The empty record advertises no borrowed storage.
        let event = unsafe { copy_record(&source, &pool) }.expect("empty event");
        let counts = measured(|| {
            event.with_record(|record| {
                assert_eq!(record.ExtendedDataCount, 0);
                assert_eq!(record.UserDataLength, 0);
                assert!(record.ExtendedData.is_null());
                assert!(record.UserData.is_null());
                assert!(record.UserContext.is_null());
                assert!(
                    pool.acquire(1).is_none(),
                    "view retains the last leased slot"
                );
            });
        });
        assert_eq!(counts, AllocCounts::default());
        // Synthetic TDH-fill seam: exercise synchronous encoding with the scoped
        // native view alive, without claiming provider/native allocation coverage.
        let metadata = RawDnsEvent {
            event_id: 256,
            timestamp: UNIX_EPOCH,
            transport: Transport::Udp,
            peer_address: None,
            peer_port: None,
            local_address: None,
            local_port: None,
        };
        let mut encoder = Encoder::new(b"lease").expect("encoder");
        let mut fills = 0;
        let failed = event.with_record(|record| {
            encoder.encode_with(metadata, 12, |destination| {
                fills += 1;
                assert!(record.UserData.is_null());
                assert!(pool.acquire(1).is_none(), "fill retains event lease");
                assert!(destination.iter().all(|byte| *byte == 0));
                destination.fill(0xff);
                bail!("synthetic native extraction failure")
            })
        });
        assert!(failed.is_err());
        assert!(pool.acquire(1).is_none(), "failed fill retains event owner");
        let mut destination_address = 0;
        let payload = event
            .with_record(|record| {
                encoder.encode_with(metadata, 12, |destination| {
                    fills += 1;
                    assert!(record.UserData.is_null());
                    assert!(pool.acquire(1).is_none(), "fill retains event lease");
                    assert!(destination.iter().all(|byte| *byte == 0));
                    destination_address = destination.as_ptr() as usize;
                    destination.fill(0x53);
                    Ok(())
                })
            })
            .expect("successful extraction after failure");
        assert_eq!(fills, 2, "one synchronous fill per encoding attempt");
        drop(event);
        assert!(
            pool.acquire(1).is_some(),
            "slot returns after view and event end"
        );
        // This use after dropping the native owner proves the result borrows only
        // encoder storage. The filled range stays inside the final wire payload.
        let offset = destination_address - payload.as_ptr() as usize;
        assert_eq!(&payload[offset..offset + 12], &[0x53; 12]);
        drop(guards);
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "Keeps source destruction, native-view validation, and reuse of the same leased slot together."
    )]
    fn scoped_native_record_repairs_all_items_and_clears_stale_reused_slot() {
        let pool = PoolManager::new(1, 0).expect("pool");
        let guards = [
            pool.acquire(1).expect("first guard"),
            pool.acquire(1).expect("second guard"),
        ];
        let event = {
            let mut payload = vec![0x5au8; 37];
            let mut extra: Vec<Vec<u8>> = (0..MAX_EXTENDED_ITEMS)
                .map(|index| {
                    let index = u8::try_from(index).expect("bounded index");
                    let size = if index.is_multiple_of(8) {
                        0
                    } else {
                        usize::from(index % 5 + 1)
                    };
                    vec![index; size]
                })
                .collect();
            let mut items = [EVENT_HEADER_EXTENDED_DATA_ITEM::default(); MAX_EXTENDED_ITEMS];
            for (index, (item, bytes)) in items.iter_mut().zip(&extra).enumerate() {
                item.DataPtr = if bytes.is_empty() {
                    0
                } else {
                    bytes.as_ptr() as u64
                };
                item.DataSize = u16::try_from(bytes.len()).expect("small item");
                item.ExtType = u16::try_from(index + 100).expect("small type");
                item.Reserved1 = u16::try_from(index).expect("small metadata");
                item.Anonymous._bitfield = 1;
            }
            let mut source = EVENT_RECORD {
                EventHeader: EVENT_HEADER {
                    ProviderId: PROVIDER,
                    ThreadId: 42,
                    ProcessId: 314,
                    TimeStamp: 12345,
                    EventDescriptor: EVENT_DESCRIPTOR {
                        Id: 256,
                        Version: 7,
                        Opcode: 9,
                        Level: 5,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                UserData: payload.as_mut_ptr().cast(),
                UserDataLength: 37,
                ExtendedData: items.as_mut_ptr(),
                ExtendedDataCount: u16::try_from(MAX_EXTENDED_ITEMS).expect("bounded item count"),
                // The copy intentionally never retains or dereferences this value.
                UserContext: std::ptr::dangling_mut::<u8>().cast(),
                ..Default::default()
            };
            source.BufferContext.LoggerId = 0x5678;
            source.BufferContext.Anonymous.ProcessorIndex = 0x1234;
            // SAFETY: Live payloads and typed descriptors back every native range.
            let event = unsafe { copy_record(&source, &pool) }.expect("copied event");
            payload.fill(0);
            for bytes in &mut extra {
                bytes.fill(0);
            }
            event
        };
        // All original source allocations and descriptor storage have ended.
        let base = event.buffer.bytes() as usize;
        let counts = measured(|| {
            event.with_record(|record| {
                assert_eq!(record.EventHeader.ThreadId, 42);
                assert_eq!(record.EventHeader.ProcessId, 314);
                assert_eq!(record.EventHeader.TimeStamp, 12345);
                assert!(dns_provider(&record.EventHeader.ProviderId));
                assert_eq!(record.EventHeader.EventDescriptor.Id, 256);
                assert_eq!(record.EventHeader.EventDescriptor.Version, 7);
                assert_eq!(record.EventHeader.EventDescriptor.Opcode, 9);
                assert_eq!(record.EventHeader.EventDescriptor.Level, 5);
                assert_eq!(record.BufferContext.LoggerId, 0x5678);
                assert_eq!(
                    // SAFETY: The fixture initialized the ProcessorIndex union member.
                    unsafe { record.BufferContext.Anonymous.ProcessorIndex },
                    0x1234
                );
                assert_eq!(record.UserDataLength, 37);
                assert_eq!(record.UserData as usize, base);
                assert_eq!(
                    record.ExtendedDataCount,
                    u16::try_from(MAX_EXTENDED_ITEMS).expect("bounded item count")
                );
                assert!(record.UserContext.is_null());
                assert_eq!(
                    (record.ExtendedData as usize)
                        % std::mem::align_of::<EVENT_HEADER_EXTENDED_DATA_ITEM>(),
                    0
                );
                // SAFETY: with_record owns a typed, aligned descriptor array and the
                // lease's initialized user payload throughout this closure.
                let (payload, items) = unsafe {
                    (
                        std::slice::from_raw_parts(record.UserData.cast::<u8>(), 37),
                        std::slice::from_raw_parts(record.ExtendedData, MAX_EXTENDED_ITEMS),
                    )
                };
                assert!(payload.iter().all(|byte| *byte == 0x5a));
                for (index, item) in items.iter().enumerate() {
                    let value = u8::try_from(index).expect("bounded index");
                    let size = if value.is_multiple_of(8) {
                        0
                    } else {
                        usize::from(value % 5 + 1)
                    };
                    assert_eq!(usize::from(item.DataSize), size);
                    assert_eq!(usize::from(item.ExtType), index + 100);
                    assert_eq!(usize::from(item.Reserved1), index);
                    assert_eq!(item.Anonymous._bitfield, 1);
                    if size == 0 {
                        assert_eq!(item.DataPtr, 0);
                        continue;
                    }
                    let pointer =
                        usize::try_from(item.DataPtr).expect("native pointer fits target");
                    assert!(pointer >= base + 37 && pointer + size <= base + pool::REGULAR_BYTES);
                    assert_eq!(pointer % 8, 0);
                    // SAFETY: The pointer is repaired into the retained initialized
                    // lease range and checked in bounds immediately above.
                    let bytes =
                        unsafe { std::slice::from_raw_parts(item.DataPtr as *const u8, size) };
                    assert!(bytes.iter().all(|byte| *byte == value));
                }
                assert!(
                    pool.acquire(1).is_none(),
                    "native view cannot release or share its slot"
                );
            });
        });
        assert_eq!(counts, AllocCounts::default());
        assert!(
            event.buffer.items().iter().all(|item| item.DataPtr == 0),
            "view pointer repair never mutates stored descriptors"
        );
        drop(event);
        // Two guard leases force the next record to reuse exactly the same slot.
        let source = EVENT_RECORD::default();
        // SAFETY: Empty source has no advertised native ranges.
        let empty = unsafe { copy_record(&source, &pool) }.expect("reused empty event");
        assert_eq!(empty.buffer.bytes() as usize, base);
        empty.with_record(|record| {
            assert_eq!(record.UserDataLength, 0);
            assert_eq!(record.ExtendedDataCount, 0);
            assert!(record.UserData.is_null());
            assert!(
                record.ExtendedData.is_null(),
                "stale descriptors remain unexposed"
            );
            assert!(record.UserContext.is_null());
        });
        drop(empty);
        drop(guards);
    }

    #[test]
    fn callback_and_cross_thread_recycle_do_not_allocate_or_deallocate() {
        drop(allocator_fixtures());
    }

    #[test]
    fn filetime_conversion_preserves_100ns_precision() -> Result<()> {
        assert_eq!(
            timestamp(116_444_736_010_000_001)?,
            UNIX_EPOCH + Duration::new(1, 100)
        );
        assert!(timestamp(116_444_735_999_999_999).is_err());
        Ok(())
    }

    #[test]
    fn unknown_provider_endpoints_stay_absent_and_known_values_survive() -> Result<()> {
        assert_eq!(observed_address("0.0.0.0".parse()?), None);
        assert_eq!(observed_address("::".parse()?), None);
        assert_eq!(observed_port(0), None);
        let ipv4: IpAddr = "172.27.215.60".parse()?;
        let ipv6: IpAddr = "2001:db8::53".parse()?;
        assert_eq!(observed_address(ipv4), Some(ipv4));
        assert_eq!(observed_address(ipv6), Some(ipv6));
        assert_eq!(observed_port(53), Some(53));
        Ok(())
    }

    #[test]
    fn rejects_malformed_addresses_and_boolean_encodings() {
        let unicode = PropertyType {
            input: TDH_INTYPE_UNICODESTRING,
            output: 0,
        };
        assert!(address(&[0x31, 0], unicode, &mut [0; 768]).is_err());
        assert!(address(&[0], unicode, &mut [0; 768]).is_err());
        assert!(
            integer(
                &[1],
                PropertyType {
                    input: TDH_INTYPE_BOOLEAN,
                    output: 0
                }
            )
            .is_err()
        );
    }

    fn address_kind(input: i32) -> PropertyType {
        PropertyType { input, output: 0 }
    }

    fn utf16_bytes(text: &str) -> Vec<u8> {
        text.encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect()
    }

    #[test]
    fn address_reuse_validates_full_text_and_ignores_valid_zone_context() -> Result<()> {
        let mut scratch = [0; 768];
        let unicode = address_kind(TDH_INTYPE_UNICODESTRING);
        let ansi = address_kind(TDH_INTYPE_ANSISTRING);
        let long_zone = format!("fe80::1%{}", "界".repeat(247));
        let cases = [
            (utf16_bytes(&long_zone), unicode, "fe80::1"),
            (b"192.0.2.53\0".to_vec(), ansi, "192.0.2.53"),
            (utf16_bytes("::%接口"), unicode, "::"),
            (utf16_bytes("::%\0ignored"), unicode, "::"),
            (b"::%bad\0suffix\0".to_vec(), ansi, "::"),
            (b"fe80::1%interface\0".to_vec(), ansi, "fe80::1"),
            (utf16_bytes("2001:db8::53"), unicode, "2001:db8::53"),
        ];
        for (bytes, kind, expected) in &cases {
            assert_eq!(
                address(bytes, *kind, &mut scratch)?,
                expected.parse::<IpAddr>()?
            );
        }
        let malformed = [
            (vec![], unicode),
            (vec![0], unicode),
            (vec![b'1', 0], unicode),
            (vec![0, 0xd8, 0, 0], unicode),
            (utf16_bytes("::\0%bad"), unicode),
            (vec![b':', 0, b':', 0, b'%', 0, 0, 0xd8, 0, 0], unicode),
            (utf16_bytes(&format!("::%{}", "a".repeat(253))), unicode),
            (b"::%bad\xff\0".to_vec(), ansi),
            (b"::\0%bad\0".to_vec(), ansi),
            (b"192.0.2.1".to_vec(), ansi),
            (b"garbage\0".to_vec(), ansi),
        ];
        for (bytes, kind) in malformed {
            assert!(address(&bytes, kind, &mut scratch).is_err());
            assert_eq!(
                address(b"127.0.0.1\0", ansi, &mut scratch)?,
                IpAddr::from([127, 0, 0, 1])
            );
        }
        super::assert_no_allocations(|| {
            for _ in 0..32 {
                for (bytes, kind, _) in &cases {
                    let result = address(bytes, *kind, &mut scratch);
                    std::hint::black_box(result).expect("valid address");
                }
            }
        });
        Ok(())
    }

    #[test]
    fn binary_addresses_require_the_native_length() -> Result<()> {
        let mut scratch = [0; 768];
        let kind = PropertyType {
            input: TDH_INTYPE_BINARY,
            output: TDH_OUTTYPE_IPV6,
        };
        assert_eq!(
            address(&[0; 16], kind, &mut scratch)?,
            IpAddr::from([0u8; 16])
        );
        for length in [0, 4, 15, 17] {
            assert!(address(&[0; 17][..length], kind, &mut scratch).is_err());
        }
        Ok(())
    }

    #[test]
    fn bounded_property_seam_initializes_and_limits_every_native_view() -> Result<()> {
        // This seam covers buffer handling, not the DNS provider's native TDH implementation.
        let mut scratch = [0x55; 512];
        for size in [512, 1, 0, 4, 511] {
            let bytes = scratch_property(&mut scratch, size, |bytes| {
                assert_eq!(bytes.len(), size);
                assert!(bytes.iter().all(|byte| *byte == 0));
                bytes.fill(0xab);
                Ok(())
            })?;
            assert!(bytes.iter().all(|byte| *byte == 0xab));
        }
        assert!(scratch_property(&mut scratch, 513, |_| panic!("oversize extraction")).is_err());
        assert!(
            scratch_property(&mut scratch, 8, |bytes| {
                bytes.fill(0xff);
                bail!("injected native failure")
            })
            .is_err()
        );
        super::assert_no_allocations(|| {
            for size in [512, 1, 4, 0, 512] {
                let result = scratch_property(&mut scratch, size, |bytes| {
                    bytes.fill(0x53);
                    Ok(())
                });
                std::hint::black_box(result).expect("bounded extraction");
            }
        });
        Ok(())
    }

    #[test]
    fn copies_payload_and_extended_data_before_callback_returns() -> Result<()> {
        let pool = PoolManager::new(1, 1)?;
        let owned = {
            let mut payload = vec![1u8, 2, 3];
            let mut extra = [4u8, 5, 6];
            let mut item = EVENT_HEADER_EXTENDED_DATA_ITEM {
                DataPtr: extra.as_ptr() as u64,
                DataSize: 3,
                ..Default::default()
            };
            let record = EVENT_RECORD {
                UserData: payload.as_mut_ptr().cast(),
                UserDataLength: 3,
                ExtendedData: &raw mut item,
                ExtendedDataCount: 1,
                ..Default::default()
            };
            // SAFETY: These live Rust allocations back every pointer/length
            // pair for the duration of the synchronous deep copy.
            let owned = unsafe { copy_record(&record, &pool) }
                .map_err(|error| anyhow!("deep copy failed: {error:?}"))?;
            payload.fill(0);
            extra.fill(0);
            owned
        };
        // SAFETY: The lease retains both checked ranges after source destruction.
        unsafe {
            assert_eq!(
                std::slice::from_raw_parts(owned.buffer.bytes(), 3),
                [1, 2, 3]
            );
            assert_eq!(
                std::slice::from_raw_parts(owned.buffer.bytes().add(owned.buffer.offset(0)), 3),
                [4, 5, 6]
            );
        }
        assert_eq!(owned.buffer.offset(0) % 8, 0);
        assert_eq!(owned.buffer.items()[0].DataPtr, 0);
        Ok(())
    }

    struct Fixture {
        context: Box<CallbackContext>,
        record: EVENT_RECORD,
        receive: crossbeam_channel::Receiver<RawEtwEvent>,
    }

    impl Fixture {
        fn new(capacity: usize, jumbo: usize) -> Self {
            let pool = PoolManager::new(capacity, jumbo).expect("test pool initializes");
            let mut context = Box::new(CallbackContext {
                pool: Arc::clone(&pool),
                counters: Arc::default(),
            });
            let (sender, receive) = crossbeam_channel::bounded(capacity);
            let context_address = (&raw const *context) as usize;
            PRODUCER.with(|state| {
                *state.borrow_mut() = Some(CallbackProducer {
                    context: context_address,
                    sender,
                    pool,
                });
            });
            let record = EVENT_RECORD {
                EventHeader: EVENT_HEADER {
                    ProviderId: PROVIDER,
                    EventDescriptor: EVENT_DESCRIPTOR {
                        Id: 256,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                UserContext: (&raw mut *context).cast(),
                ..Default::default()
            };
            Self {
                context,
                record,
                receive,
            }
        }

        fn invoke(&mut self) {
            // SAFETY: Box keeps the context stable; test payloads/descriptors, when
            // present, remain live across this synchronous callback invocation.
            unsafe {
                native_etw_callback(&raw mut self.record);
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            PRODUCER.with(|state| {
                state.borrow_mut().take();
            });
        }
    }

    const EXCLUDED_EVENT_IDS: [u16; 9] = [0, 1, 255, 258, 259, 260, 261, 262, u16::MAX];

    fn assert_no_intake_activity(fixture: &Fixture) {
        assert!(fixture.receive.is_empty());
        let counters = &fixture.context.counters;
        for counter in [
            &counters.received,
            &counters.dropped,
            &counters.intake_dropped,
            &counters.pool_exhausted,
            &counters.receiver_disconnected,
            &counters.callback_errors,
            &counters.etw_lost,
            &counters.etw_buffers_lost,
            &counters.stats_errors,
        ] {
            assert_eq!(counter.load(Ordering::Relaxed), 0);
        }
        assert!(!counters.stats_valid.load(Ordering::Relaxed));
    }

    #[test]
    fn callback_accepts_only_client_query_and_successful_response() {
        let mut fixture = Fixture::new(2, 0);
        for id in [256, 257] {
            fixture.record.EventHeader.EventDescriptor.Id = id;
            assert_no_allocations(|| fixture.invoke());
        }
        for id in [256, 257] {
            let event = fixture.receive.try_recv().expect("accepted client event");
            assert_eq!(event.header.EventDescriptor.Id, id);
        }
        assert_eq!(fixture.context.counters.received.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn callback_excludes_other_ids_before_payload_pool_and_tls_access() {
        let mut fixture = Fixture::new(1, 0);
        for id in EXCLUDED_EVENT_IDS {
            fixture.record.EventHeader.EventDescriptor.Id = id;
            assert_no_allocations(|| fixture.invoke());
        }
        assert_no_intake_activity(&fixture);
        // All three regular slots are still available after excluded records.
        let held: Vec<_> = (0..3)
            .map(|_| fixture.context.pool.acquire(1).expect("unconsumed slot"))
            .collect();
        assert!(fixture.context.pool.acquire(1).is_none());
        for id in EXCLUDED_EVENT_IDS {
            fixture.record.EventHeader.EventDescriptor.Id = id;
            assert_no_allocations(|| fixture.invoke());
        }
        assert_no_intake_activity(&fixture);
        for id in EXCLUDED_EVENT_IDS {
            fixture.record.EventHeader.EventDescriptor.Id = id;
            // Malformed pointer/length pairs must never reach copying.
            fixture.record.UserDataLength = u16::MAX;
            fixture.record.ExtendedDataCount = u16::MAX;
            PRODUCER.with(|state| {
                let _borrow = state.borrow_mut();
                assert_no_allocations(|| fixture.invoke());
            });
            assert_no_intake_activity(&fixture);
        }
        drop(held);
        let returned: Vec<_> = (0..3)
            .map(|_| fixture.context.pool.acquire(1).expect("retained pool slot"))
            .collect();
        drop(returned);
    }

    #[test]
    fn callback_header_filter_handles_null_record_context_and_wrong_provider() {
        let mut fixture = Fixture::new(1, 0);
        // SAFETY: The callback explicitly accepts a null defensive input.
        assert_no_allocations(|| unsafe { native_etw_callback(null_mut()) });
        fixture.record.UserContext = null_mut();
        for id in EXCLUDED_EVENT_IDS {
            fixture.record.EventHeader.EventDescriptor.Id = id;
            assert_no_allocations(|| fixture.invoke());
        }
        fixture.record.EventHeader.EventDescriptor.Id = 256;
        fixture.record.EventHeader.ProviderId = GUID::from_u128(0);
        assert_no_allocations(|| fixture.invoke());
        assert_no_intake_activity(&fixture);
    }

    #[test]
    fn parser_excludes_other_ids_before_tdh_and_recycles_storage() -> Result<()> {
        let pool = PoolManager::new(1, 0)?;
        let mut parser = Parser::new();
        let mut encoder = Encoder::new(b"test")?;
        for id in EXCLUDED_EVENT_IDS {
            let source = EVENT_RECORD {
                EventHeader: EVENT_HEADER {
                    ProviderId: PROVIDER,
                    EventDescriptor: EVENT_DESCRIPTOR {
                        Id: id,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            };
            // SAFETY: The empty event advertises no native input ranges.
            let event = unsafe { copy_record(&source, &pool) }.expect("empty excluded event");
            assert_no_allocations(|| {
                assert!(
                    parser
                        .parse(event, &mut encoder)
                        .expect("filtered before TDH")
                        .is_none()
                );
            });
        }
        let returned: Vec<_> = (0..3)
            .map(|_| pool.acquire(1).expect("parser returns excluded lease"))
            .collect();
        drop(returned);
        Ok(())
    }

    #[test]
    fn callback_drops_new_record_when_ring_is_full() {
        let mut fixture = Fixture::new(1, 0);
        fixture.invoke();
        fixture.invoke();
        assert_eq!(fixture.receive.len(), 1);
        assert_eq!(fixture.context.counters.received.load(Ordering::Relaxed), 2);
        assert_eq!(fixture.context.counters.dropped.load(Ordering::Relaxed), 1);
        assert_eq!(
            fixture
                .context
                .counters
                .intake_dropped
                .load(Ordering::Relaxed),
            1
        );
    }

    #[test]
    fn pool_exhaustion_is_counted_and_cross_thread_recycle_survives_owner_exit() {
        let mut fixture = Fixture::new(1, 0);
        let pool = Arc::clone(&fixture.context.pool);
        let first = pool.acquire(1).expect("first slot");
        let second = pool.acquire(1).expect("second slot");
        let third = pool.acquire(1).expect("third slot");
        fixture.invoke();
        assert_eq!(
            fixture
                .context
                .counters
                .pool_exhausted
                .load(Ordering::Relaxed),
            1
        );
        assert_eq!(fixture.context.counters.dropped.load(Ordering::Relaxed), 0);
        drop(fixture);
        drop(pool);
        thread::spawn(move || {
            drop(first);
            drop(second);
            drop(third);
        })
        .join()
        .expect("leases survive producer owner");
    }

    #[test]
    fn native_bounds_and_jumbo_disable_are_explicit() {
        let mut fixture = Fixture::new(1, 0);
        fixture.record.UserDataLength = 1;
        fixture.invoke();
        assert_eq!(
            fixture
                .context
                .counters
                .callback_errors
                .load(Ordering::Relaxed),
            1
        );
        let mut payload = vec![1u8; 2049];
        fixture.record.UserData = payload.as_mut_ptr().cast();
        fixture.record.UserDataLength = 2049;
        fixture.invoke();
        assert_eq!(
            fixture
                .context
                .counters
                .pool_exhausted
                .load(Ordering::Relaxed),
            1
        );
        fixture.record.ExtendedDataCount =
            u16::try_from(MAX_EXTENDED_ITEMS + 1).expect("bounded item count");
        fixture.invoke();
        assert_eq!(
            fixture
                .context
                .counters
                .callback_errors
                .load(Ordering::Relaxed),
            2
        );
    }

    #[test]
    fn missing_or_reentrant_tls_is_rejected_without_panicking() {
        let mut fixture = Fixture::new(1, 0);
        PRODUCER.with(|state| {
            let _borrow = state.borrow_mut();
            fixture.invoke();
        });
        PRODUCER.with(|state| {
            state.borrow_mut().take();
        });
        fixture.invoke();
        assert_eq!(fixture.context.counters.received.load(Ordering::Relaxed), 2);
        assert_eq!(
            fixture
                .context
                .counters
                .callback_errors
                .load(Ordering::Relaxed),
            2
        );
    }

    #[test]
    fn queued_records_survive_native_owner_exit_and_drain_after_disconnect() {
        let mut fixture = Fixture::new(2, 0);
        let mut payload = [1u8, 2, 3];
        fixture.record.UserData = payload.as_mut_ptr().cast();
        fixture.record.UserDataLength = 3;
        fixture.record.EventHeader.ThreadId = 42;
        fixture.record.EventHeader.TimeStamp = 12345;
        fixture.invoke();
        fixture.invoke();
        payload.fill(0);
        let receive = fixture.receive.clone();
        let weak = Arc::downgrade(&fixture.context.pool);
        drop(fixture);
        assert!(
            weak.upgrade().is_some(),
            "queued leases retain pool after native owner exits"
        );
        let drained = thread::spawn(move || {
            let mut count = 0;
            while let Ok(event) = receive.try_recv() {
                assert_eq!(event.header.ThreadId, 42);
                assert_eq!(event.header.TimeStamp, 12345);
                assert_eq!(
                    // SAFETY: The retained lease owns the initialized three-byte range.
                    unsafe { std::slice::from_raw_parts(event.buffer.bytes(), 3) },
                    [1, 2, 3]
                );
                count += 1;
                drop(event);
            }
            assert!(matches!(
                receive.try_recv(),
                Err(crossbeam_channel::TryRecvError::Disconnected)
            ));
            count
        })
        .join()
        .expect("drain after native completion");
        assert_eq!(drained, 2);
        assert!(
            weak.upgrade().is_none(),
            "final lease releases the pool after shutdown"
        );
    }

    #[test]
    fn largest_record_and_all_extended_descriptors_fit_and_recycle() {
        let pool = PoolManager::new(1, 1).expect("pool");
        let mut payload = vec![0xaau8; 65535];
        let extra = vec![0xbbu8; 65535];
        let mut items = [EVENT_HEADER_EXTENDED_DATA_ITEM::default(); MAX_EXTENDED_ITEMS];
        // Keep the aggregate extended length at 65535 and exercise alignment gaps.
        let mut offset = 0usize;
        for (index, item) in items.iter_mut().enumerate() {
            let size = if index == MAX_EXTENDED_ITEMS - 1 {
                65535 - (MAX_EXTENDED_ITEMS - 1) * 1025
            } else {
                1025
            };
            item.DataSize = u16::try_from(size).expect("bounded item size");
            // SAFETY: Offset traverses the live allocation, including final item.
            item.DataPtr = unsafe { extra.as_ptr().add(offset) } as u64;
            offset += size;
        }
        let record = EVENT_RECORD {
            UserData: payload.as_mut_ptr().cast(),
            UserDataLength: 65535,
            ExtendedData: items.as_mut_ptr(),
            ExtendedDataCount: u16::try_from(MAX_EXTENDED_ITEMS).expect("bounded item count"),
            ..Default::default()
        };
        // SAFETY: All advertised native ranges are backed by live allocations.
        let owned = unsafe { copy_record(&record, &pool) }.expect("maximum record fits jumbo");
        assert!(
            pool.acquire(pool::REGULAR_BYTES + 1).is_none(),
            "jumbo lease remains exclusive"
        );
        for (index, item) in owned.buffer.items().iter().enumerate() {
            assert_eq!(item.DataPtr, 0);
            assert_eq!(owned.buffer.offset(index) % 8, 0);
            // SAFETY: Each checked item range is initialized in the retained lease.
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    owned.buffer.bytes().add(owned.buffer.offset(index)),
                    usize::from(item.DataSize),
                )
            };
            assert!(bytes.iter().all(|byte| *byte == 0xbb));
        }
        drop(owned);
        assert!(pool.acquire(pool::JUMBO_BYTES).is_some());
        items[0].DataSize = 65535;
        let malformed = EVENT_RECORD {
            ExtendedData: items.as_mut_ptr(),
            ..record
        };
        assert!(matches!(
            // SAFETY: The preflight rejects aggregate bounds before reading payloads.
            unsafe { copy_record(&malformed, &pool) },
            Err(CopyError::Malformed)
        ));
    }

    #[test]
    #[ignore = "manual synthetic callback benchmark; excludes TDH and output"]
    fn synthetic_pooled_intake_benchmark() {
        use std::time::Instant;
        let capacity = std::env::var("ETW_BENCH_CAPACITY")
            .ok()
            .map_or(8192, |v| v.parse().expect("capacity integer"));
        let offered: usize = std::env::var("ETW_BENCH_EVENTS")
            .ok()
            .map_or(200_000, |v| v.parse().expect("event count integer"));
        let mode = std::env::var("ETW_BENCH_MODE").unwrap_or_else(|_| "steady".into());
        assert!(matches!(mode.as_str(), "steady" | "bursty"));
        let fixtures = allocator_fixtures();
        let mut fixture = Fixture::new(capacity, 64);
        let mut payload = [0x5au8; 512];
        fixture.record.UserDataLength = 512;
        fixture.record.UserData = payload.as_mut_ptr().cast();
        let done = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(mode == "steady"));
        let consumer = {
            let receiver = fixture.receive.clone();
            let done = Arc::clone(&done);
            let release = Arc::clone(&release);
            thread::spawn(move || {
                let mut dequeued = 0u64;
                loop {
                    if release.load(Ordering::Acquire) {
                        match receiver.try_recv() {
                            Ok(event) => {
                                drop(event);
                                dequeued += 1;
                                continue;
                            }
                            Err(crossbeam_channel::TryRecvError::Disconnected) => break,
                            Err(crossbeam_channel::TryRecvError::Empty) => {}
                        }
                    }
                    if done.load(Ordering::Acquire) && receiver.is_empty() {
                        break;
                    }
                    thread::park_timeout(Duration::from_millis(2));
                }
                dequeued
            })
        };
        let started = Instant::now();
        for index in 0..offered {
            fixture.invoke();
            if mode == "bursty" && (index + 1).is_multiple_of(capacity + 2) {
                release.store(true, Ordering::Release);
                consumer.thread().unpark();
                while !fixture.receive.is_empty() {
                    thread::yield_now();
                }
                release.store(false, Ordering::Release);
            }
        }
        done.store(true, Ordering::Release);
        release.store(true, Ordering::Release);
        consumer.thread().unpark();
        let dequeued = consumer.join().expect("consumer completed");
        let elapsed = started.elapsed().as_nanos();
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        let counters = &fixture.context.counters;
        let intake_dropped = load(&counters.intake_dropped);
        let callback_errors = load(&counters.callback_errors);
        let accepted = offered as u64 - intake_dropped - callback_errors;
        assert_eq!(accepted, dequeued);
        assert_eq!(fixture.receive.len(), 0);
        let pool = &fixture.context.pool;
        let leases: Vec<_> = (0..capacity + 2)
            .map(|_| pool.acquire(1).expect("all regular slots returned"))
            .collect();
        assert!(pool.acquire(1).is_none());
        drop(leases);
        let fixture_json = fixtures
            .iter()
            .map(|(name, count)| {
                format!(
                    "\"{name}\":{{\"allocations\":{},\"reallocations\":{},\"deallocations\":{}}}",
                    count.allocations, count.reallocations, count.deallocations
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "ETW_BENCH_JSON={{\"schema\":1,\"capacity\":{capacity},\"regular_capacity\":{},\"regular_bytes\":{},\"jumbo_capacity\":64,\"jumbo_bytes\":{},\"mode\":\"{mode}\",\"offered\":{offered},\"accepted\":{accepted},\"dequeued\":{dequeued},\"intake_dropped\":{intake_dropped},\"ring_dropped\":{},\"pool_exhausted\":{},\"receiver_disconnected\":{},\"callback_errors\":{callback_errors},\"final_backlog\":0,\"elapsed_ns\":{elapsed},\"allocator_fixtures\":{{{fixture_json}}},\"recycle_verified\":true}}",
            capacity + 2,
            pool::REGULAR_BYTES,
            pool::JUMBO_BYTES,
            load(&counters.dropped),
            load(&counters.pool_exhausted),
            load(&counters.receiver_disconnected)
        );
    }

    fn cached_schema(parser: &Parser, key: (u16, u8)) -> Option<&Schema> {
        parser
            .schemas
            .iter()
            .find(|(cached, _)| *cached == key)
            .map(|(_, schema)| schema)
    }

    fn cached_schema_mut(parser: &mut Parser, key: (u16, u8)) -> Option<&mut Schema> {
        parser
            .schemas
            .iter_mut()
            .find(|(cached, _)| *cached == key)
            .map(|(_, schema)| schema)
    }

    fn cached_fixture_schema(fields: &[FieldSpec], peer_name: &str) -> Schema {
        let kind = |name: &str| {
            let field = fields
                .iter()
                .find(|field| field.name == name)
                .expect("fixture field");
            PropertyType {
                input: field.input,
                output: field.output,
            }
        };
        Schema {
            plan: DecodePlan::compile(fields, peer_name),
            tcp: kind("TCP"),
            port: kind("Port"),
            local: kind("InterfaceIP"),
            peer: kind(peer_name),
            packet: kind("PacketData"),
        }
    }

    fn pooled_fixture(
        payload: &mut [u8],
        id: u16,
        version: u8,
        pool: &Arc<PoolManager>,
    ) -> RawEtwEvent {
        let source = EVENT_RECORD {
            EventHeader: EVENT_HEADER {
                ProviderId: PROVIDER,
                TimeStamp: 116_444_736_000_000_001,
                EventDescriptor: EVENT_DESCRIPTOR {
                    Id: id,
                    Version: version,
                    ..Default::default()
                },
                ..Default::default()
            },
            UserDataLength: u16::try_from(payload.len()).expect("fixture size"),
            UserData: payload.as_mut_ptr().cast(),
            ..Default::default()
        };
        // SAFETY: The source owns the advertised payload until the copy completes.
        unsafe { copy_record(&source, pool) }.expect("fixture copied")
    }

    #[test]
    fn cached_supported_parser_has_zero_tdh_calls_and_no_allocations() -> Result<()> {
        let pool = PoolManager::new(1, 0)?;
        let mut parser = Parser::new();
        let mut encoder = Encoder::new(b"test")?;
        for id in [256, 257] {
            let mut fixture = decode::tests::fixture(id, true, true, 2);
            let schema = cached_fixture_schema(
                &fixture.fields,
                if id == 256 { "Source" } else { "Destination" },
            );
            assert!(schema.plan.is_some());
            parser.schemas.push(((id, 0), schema));
            // Warm final encoder capacity; subsequent pooled-buffer reuse is measured.
            let event = pooled_fixture(&mut fixture.payload, id, 0, &pool);
            assert!(parser.parse(event, &mut encoder)?.is_some());
            TDH_CALLS.with(|calls| calls.set([0; 3]));
            for _ in 0..4 {
                let event = pooled_fixture(&mut fixture.payload, id, 0, &pool);
                assert_no_allocations(|| {
                    assert!(
                        parser
                            .parse(event, &mut encoder)
                            .expect("valid cached event")
                            .is_some()
                    );
                });
            }
            assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
            // The consuming parser released every lease before returning encoder bytes.
            let leases: Vec<_> = (0..3)
                .map(|_| pool.acquire(1).expect("lease returned"))
                .collect();
            drop(leases);
        }
        Ok(())
    }

    #[test]
    fn full_schema_cache_hits_alternate_ids_and_versions_without_tdh() -> Result<()> {
        let pool = PoolManager::new(1, 0)?;
        let mut parser = Parser::new();
        let mut encoder = Encoder::new(b"cache boundary")?;
        let mut fixtures = Vec::new();
        let mut expected = Vec::new();
        for id in [256, 257] {
            for version in 0..4 {
                let wide_tcp = version % 2 != 0;
                let mut fixture = decode::tests::fixture(id, !wide_tcp, wide_tcp, 0);
                if wide_tcp {
                    fixture.fields[0].input = TDH_INTYPE_UINT32;
                    fixture.fields[0].output = 0;
                    fixture.payload.splice(0..1, 1u32.to_le_bytes());
                }
                let peer = if id == 256 { "Source" } else { "Destination" };
                parser
                    .schemas
                    .push(((id, version), cached_fixture_schema(&fixture.fields, peer)));
                let event = pooled_fixture(&mut fixture.payload, id, version, &pool);
                expected.push(
                    parser
                        .parse(event, &mut encoder)?
                        .expect("warm fixture")
                        .to_vec(),
                );
                fixtures.push((id, version, fixture));
            }
        }
        assert_eq!(parser.schemas.len(), MAX_CACHED_SCHEMAS);
        TDH_CALLS.with(|calls| calls.set([0; 3]));
        // Interleave IDs and versions so an accidentally incomplete key uses the
        // wrong TCP width or packet direction.
        for index in (0..4).flat_map(|version| [version, version + 4]) {
            let (id, version, fixture) = &mut fixtures[index];
            let event = pooled_fixture(&mut fixture.payload, *id, *version, &pool);
            assert_no_allocations(|| {
                assert_eq!(
                    parser
                        .parse(event, &mut encoder)
                        .expect("cached hit")
                        .expect("accepted"),
                    expected[index]
                );
            });
            assert_eq!(parser.schemas.len(), MAX_CACHED_SCHEMAS);
        }
        assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
        let mut fixture = decode::tests::fixture(256, false, false, 0);
        let event = pooled_fixture(&mut fixture.payload, 256, 249, &pool);
        assert!(parser.parse(event, &mut encoder).is_err());
        assert_eq!(TDH_CALLS.with(std::cell::Cell::get)[0], 1);
        assert_eq!(parser.schemas.len(), MAX_CACHED_SCHEMAS);
        assert!(cached_schema(&parser, (256, 249)).is_none());
        for (id, version, _) in &fixtures {
            assert!(cached_schema(&parser, (*id, *version)).is_some());
        }
        let (id, version, fixture) = &mut fixtures[0];
        let event = pooled_fixture(&mut fixture.payload, *id, *version, &pool);
        TDH_CALLS.with(|calls| calls.set([0; 3]));
        assert_eq!(
            parser.parse(event, &mut encoder)?.expect("recovery"),
            expected[0]
        );
        assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
        Ok(())
    }

    #[test]
    fn schema_cache_never_exceeds_bound_or_reallocates() {
        let fixture = decode::tests::fixture(256, false, false, 0);
        let mut parser = Parser::new();
        let capacity = parser.schemas.capacity();
        // MAX_CACHED_SCHEMAS + 1 distinct keys: the final insert clears a full cache.
        for version in 0..=u8::try_from(MAX_CACHED_SCHEMAS).expect("bounded cache") {
            let schema = cached_fixture_schema(&fixture.fields, "Source");
            parser.insert_schema((256, version), schema);
            assert!(parser.schemas.len() <= MAX_CACHED_SCHEMAS);
            assert_eq!(parser.schemas.capacity(), capacity);
        }
        assert_eq!(parser.schemas.len(), 1);
        assert!(cached_schema(&parser, (256, 0)).is_none());
        let last = u8::try_from(MAX_CACHED_SCHEMAS).expect("bounded cache");
        assert!(cached_schema(&parser, (256, last)).is_some());
    }

    #[test]
    fn supported_malformed_rejects_without_tdh_but_unsupported_schema_uses_fallback() -> Result<()>
    {
        let pool = PoolManager::new(1, 0)?;
        let mut parser = Parser::new();
        let mut encoder = Encoder::new(b"test")?;
        let mut fixture = decode::tests::fixture(256, false, false, 0);
        parser
            .schemas
            .push(((256, 0), cached_fixture_schema(&fixture.fields, "Source")));
        let event = pooled_fixture(&mut fixture.payload, 256, 0, &pool);
        assert!(parser.parse(event, &mut encoder)?.is_some());
        fixture.payload.truncate(fixture.packet.end - 1);
        TDH_CALLS.with(|calls| calls.set([0; 3]));
        let event = pooled_fixture(&mut fixture.payload, 256, 0, &pool);
        assert!(parser.parse(event, &mut encoder).is_err());
        assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
        let mut unsupported = cached_fixture_schema(&fixture.fields, "Source");
        unsupported.plan = None;
        parser.schemas.push(((256, 249), unsupported));
        let event = pooled_fixture(&mut fixture.payload, 256, 249, &pool);
        assert!(parser.parse(event, &mut encoder).is_err());
        let calls = TDH_CALLS.with(std::cell::Cell::get);
        assert_eq!(calls[0], 0);
        assert_eq!(calls[1], 1);
        Ok(())
    }

    #[test]
    fn cached_parser_rejects_semantic_mutations_and_recovers_without_tdh() -> Result<()> {
        let pool = PoolManager::new(1, 0)?;
        let mut parser = Parser::new();
        let mut encoder = Encoder::new(b"mutation recovery")?;
        for id in [256, 257] {
            let mut valid = decode::tests::fixture(id, false, false, 0);
            let peer = if id == 256 { "Source" } else { "Destination" };
            parser
                .schemas
                .push(((id, 0), cached_fixture_schema(&valid.fields, peer)));
            let event = pooled_fixture(&mut valid.payload, id, 0, &pool);
            let expected = parser
                .parse(event, &mut encoder)?
                .expect("valid fixture")
                .to_vec();
            for mutation in 0..7 {
                let mut fixture = decode::tests::fixture(id, false, false, 0);
                match mutation {
                    0 => fixture.payload[fixture.tcp.start] = 2,
                    1 => fixture.payload[fixture.port.clone()]
                        .copy_from_slice(&u32::MAX.to_le_bytes()),
                    2 => {
                        // Keep the layout internally consistent while exceeding the
                        // parser's shared TDH-compatible address extraction bound.
                        let mut oversized = vec![b'a'; 513];
                        oversized.push(0);
                        fixture.payload.splice(fixture.local.clone(), oversized);
                    }
                    3 => fixture.payload[fixture.local.start] = b'!',
                    4 => fixture.payload[fixture.length_offset..fixture.length_offset + 4]
                        .copy_from_slice(&11u32.to_le_bytes()),
                    5 => fixture.payload[fixture.packet.start + 2] ^= 0x80,
                    _ => {}
                }
                let mut event = pooled_fixture(&mut fixture.payload, id, 0, &pool);
                if mutation == 6 {
                    event.header.TimeStamp = 0;
                }
                TDH_CALLS.with(|calls| calls.set([0; 3]));
                assert!(
                    parser.parse(event, &mut encoder).is_err(),
                    "mutation {mutation}, event {id}"
                );
                assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
                let leases: Vec<_> = (0..3)
                    .map(|_| pool.acquire(1).expect("rejected lease recycled"))
                    .collect();
                drop(leases);
                let event = pooled_fixture(&mut valid.payload, id, 0, &pool);
                assert_eq!(
                    parser
                        .parse(event, &mut encoder)?
                        .expect("recovery accepted"),
                    expected
                );
                assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
            }
        }
        Ok(())
    }

    #[test]
    fn process_local_manifest_tdh_matches_borrowed_fields_and_parser_output() -> Result<()> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::Diagnostics::Etw::{TdhLoadManifest, TdhUnloadManifest};
        struct Manifest(Vec<u16>);
        impl Drop for Manifest {
            fn drop(&mut self) {
                // SAFETY: This exact terminated path remains owned through unload.
                let status = unsafe { TdhUnloadManifest(self.0.as_ptr()) };
                assert_eq!(status, ERROR_SUCCESS, "unload process-local test manifest");
            }
        }
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/etw/testdata/decoder-fixture.man");
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: The absolute terminated manifest path is live for the synchronous
        // process-local decoder registration; this does not register an ETW publisher.
        let status = unsafe { TdhLoadManifest(path.as_ptr()) };
        check_status(status, "load fixture manifest")?;
        let _manifest = Manifest(path);
        let pool = PoolManager::new(1, 0)?;
        let mut parser = Parser::new();
        let mut encoder = Encoder::new(b"native oracle")?;
        let fixture = decode::tests::fixture(256, false, false, 0);
        for version in 0..u8::try_from(MAX_CACHED_SCHEMAS).expect("bounded cache") {
            parser.schemas.push((
                (256, version),
                cached_fixture_schema(&fixture.fields, "Source"),
            ));
        }
        for id in [256, 257] {
            for (ipv6, tcp, variant) in [(false, false, 0), (true, true, 2), (false, true, 1)] {
                let mut fixture = decode::tests::fixture(id, ipv6, tcp, variant);
                let event = pooled_fixture(&mut fixture.payload, id, 250, &pool);
                event.with_record(|record| -> Result<()> {
                    let peer = if id == 256 {
                        PropertyName::Source
                    } else {
                        PropertyName::Destination
                    };
                    let mut scratch = [0; 512];
                    for (name, range) in [
                        (PropertyName::Tcp, fixture.tcp.clone()),
                        (PropertyName::Port, fixture.port.clone()),
                        (PropertyName::InterfaceIp, fixture.local.clone()),
                        (peer, fixture.peer.clone()),
                    ] {
                        assert_eq!(
                            property(record, name, 512, &mut scratch)?,
                            &fixture.payload[range]
                        );
                    }
                    let size = property_size(record, PropertyName::PacketData, 65535)?;
                    let mut packet = vec![0; size];
                    read_property(record, PropertyName::PacketData, &mut packet)?;
                    assert_eq!(packet, fixture.payload[fixture.packet.clone()]);
                    Ok(())
                })?;
                let was_missing = cached_schema(&parser, (id, 250)).is_none();
                let was_full = parser.schemas.len() == MAX_CACHED_SCHEMAS;
                TDH_CALLS.with(|calls| calls.set([0; 3]));
                let fast = parser
                    .parse(event, &mut encoder)?
                    .expect("fixture supported")
                    .to_vec();
                assert_eq!(
                    TDH_CALLS.with(std::cell::Cell::get)[0],
                    2 * usize::from(was_missing)
                );
                if was_full {
                    assert_eq!(parser.schemas.len(), 1);
                    assert!(cached_schema(&parser, (id, 250)).is_some());
                }
                let schema =
                    cached_schema_mut(&mut parser, (id, 250)).expect("native schema cached");
                let plan = schema.plan.take().expect("native template compiles");
                let event = pooled_fixture(&mut fixture.payload, id, 250, &pool);
                assert_eq!(
                    parser.parse(event, &mut encoder)?.expect("TDH fallback"),
                    fast
                );
                cached_schema_mut(&mut parser, (id, 250))
                    .expect("schema retained")
                    .plan = Some(plan);
                TDH_CALLS.with(|calls| calls.set([0; 3]));
                let event = pooled_fixture(&mut fixture.payload, id, 250, &pool);
                assert_no_allocations(|| {
                    assert_eq!(
                        parser
                            .parse(event, &mut encoder)
                            .expect("warm native plan")
                            .expect("accepted"),
                        fast
                    );
                });
                assert_eq!(TDH_CALLS.with(std::cell::Cell::get), [0; 3]);
            }
        }
        Ok(())
    }

    #[test]
    fn native_normalization_keeps_unsupported_trailing_names_without_reading_unions() -> Result<()>
    {
        use windows_sys::Win32::System::Diagnostics::Etw::EVENT_PROPERTY_INFO_0_0;
        let fixture = decode::tests::fixture(256, false, false, 0);
        let mut bytes = Vec::new();
        let mut properties = Vec::new();
        for field in &fixture.fields {
            let mut property = EVENT_PROPERTY_INFO {
                NameOffset: u32::try_from(bytes.len())?,
                Flags: field.flags.cast_signed(),
                ..Default::default()
            };
            bytes.extend(
                field
                    .name
                    .encode_utf16()
                    .chain(Some(0))
                    .flat_map(u16::to_le_bytes),
            );
            property.Anonymous1.nonStructType = EVENT_PROPERTY_INFO_0_0 {
                InType: u16::try_from(field.input)?,
                OutType: u16::try_from(field.output)?,
                MapNameOffset: 0,
            };
            property.Anonymous2.count = field.count;
            property.Anonymous3.length = field.length;
            properties.push(property);
        }
        let trailing = properties.len() - 1;
        for flags in [PropertyStruct, PropertyHasCustomSchema, i32::MIN] {
            properties[trailing].Flags = flags;
            let fields = normalize_fields(&properties, &bytes)?.expect("names retained");
            assert_eq!(fields[trailing].input, -1);
            assert!(DecodePlan::compile(&fields, "Source").is_some());
            properties[3].Flags = flags;
            let fields = normalize_fields(&properties, &bytes)?.expect("names retained");
            assert!(DecodePlan::compile(&fields, "Source").is_none());
            properties[3].Flags = fixture.fields[3].flags.cast_signed();
        }
        // An unsupported trailing target name must still disqualify ambiguity.
        properties[trailing].NameOffset = properties[2].NameOffset;
        let fields = normalize_fields(&properties, &bytes)?.expect("names retained");
        assert!(DecodePlan::compile(&fields, "Source").is_none());
        Ok(())
    }

    #[test]
    fn native_normalization_validates_names_and_union_selection() -> Result<()> {
        let array_offset = offset_of!(TRACE_EVENT_INFO, EventPropertyInfoArray);
        assert!(validate_property_array(0, 1, array_offset).is_err());
        assert!(
            validate_property_array(2, 1, array_offset + size_of::<EVENT_PROPERTY_INFO>()).is_err()
        );
        assert!(validate_property_array(usize::MAX, 0, usize::MAX).is_err());
        validate_property_array(2, 1, array_offset + 2 * size_of::<EVENT_PROPERTY_INFO>())?;
        let bytes: Vec<u8> = "PacketData\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut property = EVENT_PROPERTY_INFO::default();
        property.Anonymous1.nonStructType =
            windows_sys::Win32::System::Diagnostics::Etw::EVENT_PROPERTY_INFO_0_0 {
                InType: u16::try_from(TDH_INTYPE_BINARY)?,
                ..Default::default()
            };
        property.Flags = PropertyParamLength;
        property.Anonymous2.count = 1;
        property.Anonymous3.lengthPropertyIndex = 7;
        let fields = normalize_fields(&[property], &bytes)?.expect("known scalar metadata");
        assert_eq!(fields[0].name, "PacketData");
        assert_eq!(fields[0].length, 7);
        assert_eq!(fields[0].count, 1);
        for flags in [PropertyStruct, PropertyHasCustomSchema, i32::MIN] {
            property.Flags = flags;
            let fields = normalize_fields(&[property], &bytes)?.expect("names-only metadata");
            assert_eq!(fields[0].input, -1);
            assert_eq!(fields[0].flags, flags.cast_unsigned());
        }
        property.Flags = 0;
        for offset in [1, u32::MAX] {
            property.NameOffset = offset;
            assert!(normalize_fields(&[property], &bytes).is_err());
        }
        assert!(property_name(&[b'A', 0, b'B'], 0).is_err());
        assert!(property_name(&[0, 0xd8, 0, 0], 0).is_err());
        assert!(normalize_fields(&vec![EVENT_PROPERTY_INFO::default(); 257], &bytes)?.is_none());
        Ok(())
    }

    #[test]
    fn decodes_host_and_network_order_ports_and_binary_addresses() -> Result<()> {
        let native_port = PropertyType {
            input: TDH_INTYPE_UINT16,
            output: 0,
        };
        let network_port = PropertyType {
            output: TDH_OUTTYPE_PORT,
            ..native_port
        };
        assert_eq!(integer(&53u16.to_le_bytes(), native_port)?, 53);
        assert_eq!(integer(&53u16.to_be_bytes(), network_port)?, 53);
        assert_eq!(
            address(
                &[192, 0, 2, 1],
                PropertyType {
                    input: TDH_INTYPE_UINT32,
                    output: TDH_OUTTYPE_IPV4,
                },
                &mut [0; 768],
            )?,
            "192.0.2.1".parse::<IpAddr>()?
        );
        Ok(())
    }
}
