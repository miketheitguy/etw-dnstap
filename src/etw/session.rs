//! Creates, queries, reclaims, and stops the ETW session, and owns [`Capture`].

use std::{
    mem::{offset_of, size_of},
    ptr::null,
    sync::{Arc, atomic::Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use crossbeam_channel::Sender;
use windows_sys::Win32::{
    Foundation::{
        ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_CANCELLED, ERROR_CTX_CLOSE_PENDING,
        ERROR_INVALID_HANDLE, ERROR_MORE_DATA, ERROR_SUCCESS, ERROR_WMI_INSTANCE_NOT_FOUND,
        GetLastError,
    },
    System::Diagnostics::Etw::{
        CONTROLTRACE_HANDLE, CloseTrace, ControlTraceW, ENABLE_TRACE_PARAMETERS,
        ENABLE_TRACE_PARAMETERS_VERSION_2, EVENT_CONTROL_CODE_ENABLE_PROVIDER,
        EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0, EVENT_FILTER_DESCRIPTOR, EVENT_FILTER_EVENT_ID,
        EVENT_FILTER_TYPE_EVENT_ID, EVENT_FILTER_TYPE_PID, EVENT_TRACE_BUFFERING_MODE,
        EVENT_TRACE_CONTROL_FLUSH, EVENT_TRACE_CONTROL_QUERY, EVENT_TRACE_CONTROL_STOP,
        EVENT_TRACE_FILE_MODE_APPEND, EVENT_TRACE_FILE_MODE_CIRCULAR,
        EVENT_TRACE_FILE_MODE_NEWFILE, EVENT_TRACE_FILE_MODE_SEQUENTIAL, EVENT_TRACE_LOGFILEW,
        EVENT_TRACE_PRIVATE_LOGGER_MODE, EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE,
        EVENT_TRACE_SYSTEM_LOGGER_MODE, EnableTraceEx2, OpenTraceW,
        PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_REAL_TIME, PROCESSTRACE_HANDLE,
        ProcessTrace, StartTraceW, WNODE_FLAG_TRACED_GUID,
    },
};

use super::{
    CaptureCounters, PROVIDER, PoolManager, RawEtwEvent, SUPPORTED_EVENT_IDS, check_status,
    instance::InstanceLock,
    intake::{self, CallbackContext, native_etw_callback},
    keywords::{KeywordChoice, KeywordSelection, resolve_keywords},
    target::{DNS_SERVICE, service_process_id, service_process_id_raw},
};
#[cfg(feature = "diag-timing")]
use super::{ThreadPriority, set_current_thread_priority};

const SUBSCRIPTION_LEVEL: u8 = 4;
/// How long `Capture::stop` waits for `ProcessTrace` to return on its own after
/// the session stops, before `CloseTrace` forces it. Half of the pipeline's
/// 10 s shutdown watchdog, so the parser join and Frame Streams FINISH keep the
/// rest.
const WORKER_EXIT_WAIT: Duration = Duration::from_secs(5);

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

/// ETW session buffer settings, validated by `SessionConfig::new`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
/// only, no file or other logger modes, and no log file. Buffer size, buffer
/// counts, flush timer, and clock type are ignored: the first three are tuning
/// the operator may have changed since a crash (a mismatch must not block
/// restart), and ETW may normalize them anyway. The instance mutex, not this
/// fingerprint, protects a live instance.
fn reclaimable(query: &QueryProperties) -> bool {
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
        reclaimable(&queried),
        "ETW session '{display_name}' already exists and does not match this program's session \
         shape; choose another --session-name or stop it manually, for example: logman stop \
         {display_name} -ets"
    );
    if queried.properties.BufferSize != config.buffer_kib {
        tracing::warn!(
            session = display_name,
            stale_buffer_kib = queried.properties.BufferSize,
            configured_buffer_kib = config.buffer_kib,
            "stale ETW session used a different buffer size; reclaiming anyway"
        );
    }
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
    /// The DNS Server service process, resolved at start. If it changes, the
    /// supervisor ends capture so SCM can restart this service with the new PID.
    dns_pid: u32,
    // Last, so the instance lock outlives the session teardown in `stop`.
    _instance: InstanceLock,
}

/// What the supervisor concluded from re-reading the DNS service process ID.
/// `observed` is `None` when the query itself failed, which proves nothing.
fn classify_target(expected: u32, observed: Option<u32>) -> Option<String> {
    match observed {
        None => None,
        Some(pid) if pid == expected => None,
        Some(0) => Some(format!(
            "the DNS Server service stopped (it ran as process ID {expected})"
        )),
        Some(pid) => Some(format!(
            "the DNS Server service restarted (process ID {expected} -> {pid})"
        )),
    }
}

/// Polls `done` every `poll` until it holds or `budget` elapses.
fn wait_until(mut done: impl FnMut() -> bool, budget: Duration, poll: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if done() {
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        thread::sleep(poll.min(deadline - now));
    }
}

/// `CloseTrace` statuses that are not failures. After a successful STOP the
/// consumer handle may already be invalid because `ProcessTrace` returned and
/// released it.
fn close_trace_benign(status: u32, session_stopped: bool) -> bool {
    matches!(status, ERROR_SUCCESS | ERROR_CTX_CLOSE_PENDING)
        || (session_stopped && status == ERROR_INVALID_HANDLE)
}

/// `ProcessTrace` return statuses that are not failures; see
/// [`close_trace_benign`] for `ERROR_INVALID_HANDLE`.
fn process_trace_benign(status: u32, session_stopped: bool) -> bool {
    matches!(status, ERROR_SUCCESS | ERROR_CANCELLED)
        || (session_stopped && status == ERROR_INVALID_HANDLE)
}

/// Turns an `EnableTraceEx2` status into an error that names the access right a
/// non-administrator service account may be missing.
fn check_enable_status(status: u32) -> Result<()> {
    if status == ERROR_ACCESS_DENIED {
        bail!(
            "EnableTraceEx2: access denied enabling the Microsoft-Windows-DNSServer provider \
             {{eb79061a-a566-4698-9119-3ed2807060e7}}. The service SID NT SERVICE\\EtwDnstap \
             needs TRACELOG_GUID_ENABLE (0x80) granted directly on that provider GUID: the \
             write-restricted token (restricted service SID) only matches restricting SIDs, so \
             'Performance Log Users' alone may not satisfy it. Run \
             scripts\\provision\\Grant-EtwDnstapEtwAccess.ps1 as an administrator, then restart \
             the service (code {status})"
        );
    }
    check_status(status, "EnableTraceEx2")
}

/// Turns a `StartTraceW` status into an error that names the access rights the
/// restricted service SID may be missing.
fn check_start_status(status: u32) -> Result<()> {
    if status == ERROR_ACCESS_DENIED {
        bail!(
            "StartTraceW: access denied creating the real-time session. The service SID \
             NT SERVICE\\EtwDnstap needs WMIGUID_QUERY (0x1), TRACELOG_CREATE_REALTIME (0x20) \
             and TRACELOG_ACCESS_REALTIME (0x400) on the default WMI security GUID \
             {{0811c1af-7a07-4a06-82ed-869455cdf713}}, and the token must be in \
             'Performance Log Users'. A write-restricted token only matches restricting SIDs, \
             so the grant must name the service SID. Run \
             scripts\\provision\\Grant-EtwDnstapEtwAccess.ps1 as an administrator and confirm \
             'net localgroup \"Performance Log Users\"' lists NT SERVICE\\EtwDnstap, then \
             restart the service (code {status})"
        );
    }
    check_status(
        status,
        "StartTraceW (use an unused session name; the service SID needs ETW session rights, see scripts\\provision\\README.md)",
    )
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
        // First, so a second instance never reaches the reclaim path below.
        let instance = InstanceLock::acquire(display_name)?;
        let dns_pid = service_process_id(DNS_SERVICE)
            .context("resolve the DNS Server service process ID for the PID filter")?;
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
        check_start_status(status)?;
        let keywords = choice.selection();
        let mut capture = Self {
            session: Some(session),
            consumer: None,
            worker: None,
            counters,
            keywords,
            dns_pid,
            _instance: instance,
        };
        // The payloads, descriptors, and parameters are locals that outlive the
        // synchronous EnableTraceEx2 call below. Two filters: event IDs and the
        // DNS Server process ID.
        let filter_payload = EventIdFilterPayload::new();
        let pid_payload = PidFilterPayload::new(dns_pid);
        let mut descriptors = [
            event_id_descriptor(&filter_payload),
            pid_descriptor(&pid_payload),
        ];
        let parameters = enable_parameters(descriptors.as_mut_ptr(), 2);
        // SAFETY: The successful StartTrace call owns this handle. The provider GUID
        // is a constant GUID. `parameters` points at the two-element `descriptors`
        // array, whose entries point at `filter_payload` and `pid_payload`; all of
        // them are stack locals that remain alive and unmoved for this synchronous
        // call, and ETW copies them before returning.
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
        check_enable_status(status)?;

        let mut context = Box::new(
            CallbackContext::new(Arc::clone(&pool), Arc::clone(&capture.counters))
                .with_expected_pid(dns_pid),
        );
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
                    intake::install_producer(&context, sender, pool);
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
                    intake::clear_producer();
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

    /// Queries the live session and stores ETW's lost-event counters. The error
    /// carries the Win32 status; failures are also counted in `stats_errors`.
    pub(crate) fn query_statistics(&self) -> Result<(), u32> {
        let Some(session) = self.session else {
            return Err(ERROR_INVALID_HANDLE);
        };
        let mut properties = QueryProperties::new();
        // SAFETY: The properties buffer is aligned, sized (Wnode.BufferSize and both
        // offsets set), and outlives this synchronous call; a null name selects the
        // session by handle, and a stale handle only yields an error status.
        let status = unsafe {
            ControlTraceW(
                session,
                null(),
                &raw mut properties.properties,
                EVENT_TRACE_CONTROL_QUERY,
            )
        };
        let status = if status == ERROR_MORE_DATA {
            ERROR_SUCCESS
        } else {
            status
        };
        self.store_statistics(status, &properties.properties);
        if status == ERROR_SUCCESS {
            Ok(())
        } else {
            Err(status)
        }
    }

    /// Re-reads the DNS Server service process ID. Returns a reason when the
    /// service stopped or restarted, which invalidates the PID filter. A failed
    /// query is inconclusive and returns `None`.
    pub(crate) fn target_change(&self) -> Option<String> {
        classify_target(self.dns_pid, service_process_id_raw(DNS_SERVICE).ok())
    }

    /// Stops collection, waits for native callbacks to end, and reports loss.
    ///
    /// Order: `ControlTrace(STOP)`, then a bounded wait for `ProcessTrace` to
    /// deliver its remaining buffers and return on its own, then `CloseTrace`
    /// (which only forces the worker out if the wait expired), then the join.
    pub(crate) fn stop(&mut self) -> Result<()> {
        let mut failure = None;
        let mut session_stopped = false;
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
            } else {
                session_stopped = true;
            }
        }
        if session_stopped {
            // The stopped session flushes its buffers to the consumer, and
            // ProcessTrace returns once they are delivered. Closing the handle
            // first could cut that final delivery short.
            let exited = wait_until(
                || self.worker.as_ref().is_none_or(JoinHandle::is_finished),
                WORKER_EXIT_WAIT,
                Duration::from_millis(5),
            );
            if !exited {
                tracing::warn!(
                    "ETW consumer did not return within {} s of STOP; closing the trace handle",
                    WORKER_EXIT_WAIT.as_secs()
                );
            }
        }
        if let Some(consumer) = self.consumer.take() {
            // SAFETY: This is the OpenTrace handle, closed exactly once. Native
            // callbacks and their Box remain alive until the worker joins below.
            let status = unsafe { CloseTrace(consumer) };
            if !close_trace_benign(status, session_stopped) && failure.is_none() {
                failure = Some(anyhow!("CloseTrace failed: {status}"));
            }
        }
        if let Some(worker) = self.worker.take() {
            match worker.join() {
                Ok(status) if process_trace_benign(status, session_stopped) => {}
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

/// An `EVENT_FILTER_TYPE_PID` payload: a packed array of process IDs (ETW allows
/// at most 8; this program supplies one).
#[repr(C)]
struct PidFilterPayload {
    pids: [u32; 1],
}

impl PidFilterPayload {
    const fn new(pid: u32) -> Self {
        Self { pids: [pid] }
    }
}

/// Describes `payload` as a process-ID filter; the caller keeps `payload` alive.
fn pid_descriptor(payload: &PidFilterPayload) -> EVENT_FILTER_DESCRIPTOR {
    EVENT_FILTER_DESCRIPTOR {
        Ptr: std::ptr::from_ref(payload) as u64,
        #[expect(clippy::cast_possible_truncation, reason = "the payload is 4 bytes")]
        Size: size_of::<PidFilterPayload>() as u32,
        Type: EVENT_FILTER_TYPE_PID,
    }
}

/// Builds version 2 enable parameters carrying `count` filter descriptors.
fn enable_parameters(
    descriptors: *mut EVENT_FILTER_DESCRIPTOR,
    count: u32,
) -> ENABLE_TRACE_PARAMETERS {
    ENABLE_TRACE_PARAMETERS {
        Version: ENABLE_TRACE_PARAMETERS_VERSION_2,
        // Drop keyword-0 events, which pass any keyword mask by default.
        EnableProperty: EVENT_ENABLE_PROPERTY_IGNORE_KEYWORD_0,
        EnableFilterDesc: descriptors,
        FilterDescCount: count,
        ..Default::default()
    }
}
